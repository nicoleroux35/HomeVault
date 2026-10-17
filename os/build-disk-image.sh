#!/bin/bash
set -euo pipefail

# HomeVaultOS disk image builder — the umbreLOS pattern.
#
# Builds a ready-to-flash raw disk image from the live-build chroot: GPT
# (1 MiB BIOS boot, 512 MiB ESP, root), full rootfs, GRUB installed for both
# BIOS and UEFI AT BUILD TIME (inside a chroot of the image itself — never on
# a live system, where grub-install fails with "failed to get canonical path
# of 'overlay'"). The selfinstall ISO embeds this image and flashes it with
# dd at first boot (see hv-firstboot); it is also published standalone for
# direct flashing with balenaEtcher/Rufus, like umbreLOS.
#
# No data partition is baked in: the flashed system claims the free space
# behind the OS partitions as HV-DATA on first boot (prepare_data_storage).
#
# Usage (root, e.g. inside the build container):
#   build-disk-image.sh <chroot-dir> <out.img.xz>
# Needs: parted dosfstools e2fsprogs rsync xz-utils util-linux(losetup).

CHROOT="${1:?chroot dir}"
OUT="${2:?output .img.xz}"

ROOT_SIZE_MIB=8192   # 8 GiB system partition (chroot ~4.2 GiB + upgrade headroom)
TGT=/tmp/hv-image-root

[ -d "$CHROOT/boot" ] || { echo "Not a chroot: $CHROOT" >&2; exit 1; }
KVER=$(basename "$(ls "$CHROOT"/boot/vmlinuz-* | head -n1)" | sed 's/^vmlinuz-//')
[ -n "$KVER" ] || { echo "No kernel in $CHROOT/boot" >&2; exit 1; }
[ -f "$CHROOT/usr/sbin/grub-install" ] || { echo "grub-install missing from the chroot (grub-pc-bin)" >&2; exit 1; }

echo "[hv-image] Creating the disk image (GPT: bios_grub 1M, ESP 512M, root ${ROOT_SIZE_MIB}M)…"
IMG=$(mktemp /tmp/homevaultos-amd64.XXXXXX.img)
truncate -s $((514 + ROOT_SIZE_MIB + 2))M "$IMG"
parted -s "$IMG" mklabel gpt
parted -s "$IMG" mkpart HV-BIOS 1MiB 2MiB
parted -s "$IMG" set 1 bios_grub on
parted -s "$IMG" mkpart HV-EFI fat32 2MiB 514MiB
parted -s "$IMG" set 2 esp on
parted -s "$IMG" mkpart HV-ROOT ext4 514MiB 100%

LOOP=$(losetup --find --show -P "$IMG")

# GPT integrity probe — dumps the primary header's `last usable LBA` so a
# corrupted table (seen once: an absurd backup-LBA written by a stray tool)
# is attributed to the exact build step.
gpt_check() {
    local lu bl
    lu=$(dd if="$IMG" bs=1 skip=$((512+70)) count=8 2>/dev/null | od -An -tu8 | tr -d ' ')
    bl=$(dd if="$IMG" bs=1 skip=$((512+80)) count=8 2>/dev/null | od -An -tu8 | tr -d ' ')
    echo "[hv-image] GPT check ($1): last-usable LBA=$lu backup-LBA=$bl"
}
gpt_check "after parted"
# Containers have no udev: create the partition nodes straight from sysfs.
for i in 1 2 3; do
    P="${LOOP}p$i"
    [ -b "$P" ] && continue
    MM=$(cat "/sys/block/$(basename "$LOOP")/$(basename "$LOOP")p$i/dev" 2>/dev/null) \
        || { echo "No sysfs entry for $P" >&2; exit 1; }
    mknod "$P" b "${MM%:*}" "${MM#*:}"
done

mkfs.vfat -F32 -n HV-EFI "${LOOP}p2"
mkfs.ext4 -F -L HV-ROOT "${LOOP}p3"
ROOT_UUID=$(blkid -s UUID -o value "${LOOP}p3")
EFI_UUID=$(blkid -s UUID -o value "${LOOP}p2")

echo "[hv-image] Copying the root filesystem (rsync from the live-build chroot)…"
rm -rf "$TGT"
mkdir -p "$TGT"
mount "${LOOP}p3" "$TGT"
# After the root mount, /boot/efi must exist ON the freshly mounted root —
# creating it beforehand on the underlying /tmp would be shadowed by the
# mount (the bug that killed the earlier live-clone approach).
mkdir -p "$TGT/boot/efi"
mount "${LOOP}p2" "$TGT/boot/efi"
rsync -aHAX \
    --exclude='/proc/*' --exclude='/sys/*' --exclude='/dev/*' \
    --exclude='/run/*' --exclude='/tmp/*' \
    "$CHROOT/" "$TGT/"

# Appliance identity + per-installation uniqueness (same as the live hooks).
{
    echo "UUID=$ROOT_UUID / ext4 errors=remount-ro 0 1"
    echo "UUID=$EFI_UUID /boot/efi vfat umask=0077 0 1"
} > "$TGT/etc/fstab"
echo homevault > "$TGT/etc/hostname"
truncate -s0 "$TGT/etc/machine-id"
rm -f "$TGT/var/lib/dbus/machine-id" "$TGT/etc/ssh/ssh_host_"* 2>/dev/null || true

mkdir -p "$TGT/boot/grub"
cat > "$TGT/boot/grub/grub.cfg" <<EOF
set default=0
set timeout=1

menuentry "HomeVaultOS" {
	search --no-floppy --fs-uuid --set=root $ROOT_UUID
	linux /boot/vmlinuz-$KVER root=UUID=$ROOT_UUID ro console=tty0 console=ttyS0,115200n8
	initrd /boot/initrd.img-$KVER
}
EOF

echo "[hv-image] Installing GRUB (BIOS + UEFI) from inside the image chroot…"
mount --bind /dev  "$TGT/dev"
mount -t proc proc "$TGT/proc"
mount --bind /sys  "$TGT/sys"
chroot "$TGT" grub-install --target=i386-pc "$LOOP"
gpt_check "after grub-install i386-pc"
chroot "$TGT" grub-install --target=x86_64-efi \
    --efi-directory=/boot/efi --removable --no-nvram
gpt_check "after grub-install x86_64-efi"

umount -R "$TGT"
rm -rf "$TGT"
losetup -d "$LOOP"
gpt_check "after losetup -d"

echo "[hv-image] Compressing ($OUT)…"
xz -T0 --verbose -6 "$IMG"
mv "${IMG}.xz" "$OUT"
echo "[hv-image] ✓ $OUT ($(du -h "$OUT" | cut -f1))"

#!/bin/bash
set -euo pipefail

# HomeVaultOS image builder — produces bootable, ready-to-use ISOs (like
# umbreLOS): everything is preinstalled, the first boot only provisions
# storage. See docs/guides/os-image.md for the architecture.
#
# Two variants share this builder:
#   persistent  (A)  live system on USB/ISO; data lives on an HV-DATA volume
#                    provisioned on first boot; reflash the stick = OS update
#   selfinstall (B)  same live system, but on first boot it clones itself onto
#                    a blank internal disk (>= 32 GiB), installs GRUB and
#                    reboots — no interactive installer, no questions asked
#
# Usage:
#   sudo ./os/build.sh                              # build both variants
#   sudo ./os/build.sh --variant persistent         # variant A only
#   sudo ./os/build.sh --variant selfinstall        # variant B only
#   sudo ./os/build.sh --fresh-package              # rebuild the app tarball
#   sudo ./os/build.sh --skip-clamav                # skip virus defs download
#   sudo ./os/build.sh --keep                       # keep the lb workspace
#
# Prerequisites: live-build, debootstrap, xorriso (apt install live-build),
# network access, ~15 GB of free disk space, and the application release
# tarball (built on demand via installer/install.sh --package).

ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
OS_DIR="$ROOT/os"
DIST_DIR="$OS_DIR/dist"

# The image targets amd64 (the x86_64 appliance/mini-PC class, like umbreLOS
# x86). aarch64 support would need a separate armhf/arm64 live-build profile.
ARCH="$(uname -m)"
if [ "$ARCH" != "x86_64" ]; then
    echo "HomeVaultOS images are built on/for x86_64 (found: $ARCH)." >&2
    exit 1
fi

VARIANT="all"
FRESH_PACKAGE=0
SKIP_CLAMAV=0
KEEP=0
while [ $# -gt 0 ]; do
    case "$1" in
        --variant) VARIANT="${2:?--variant needs a value}"; shift 2 ;;
        --fresh-package) FRESH_PACKAGE=1; shift ;;
        --skip-clamav) SKIP_CLAMAV=1; shift ;;
        --keep) KEEP=1; shift ;;
        -h|--help) sed -n '2,30p' "$0"; exit 0 ;;
        *) echo "Unknown option: $1" >&2; exit 1 ;;
    esac
done
case "$VARIANT" in persistent|selfinstall|all) ;; *)
    echo "--variant must be persistent, selfinstall or all" >&2; exit 1 ;;
esac

VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' "$ROOT/Cargo.toml" | head -n1)"
if [ -z "$VERSION" ]; then
    echo "Could not read the version from Cargo.toml" >&2
    exit 1
fi

# Root is required: live-build invokes debootstrap/mount in the chroot.
if [ "$(id -u)" -ne 0 ]; then
    echo "This script must run as root (live-build needs mount/debootstrap):" >&2
    echo "  sudo ./os/build.sh $*" >&2
    exit 1
fi

for tool in lb debootstrap xorriso parted mkfs.vfat rsync xz losetup; do
    if ! command -v "$tool" >/dev/null 2>&1; then
        echo "Missing build dependency: $tool  (apt install live-build xorriso parted dosfstools rsync xz-utils)" >&2
        exit 1
    fi
done

FREE_GB=$(df --output=avail -BG "$OS_DIR" | tail -n1 | tr -dc '0-9')
if [ -n "$FREE_GB" ] && [ "$FREE_GB" -lt 15 ]; then
    echo "WARNING: only ${FREE_GB}G free on $OS_DIR — a full build needs ~15G." >&2
fi

# ---------------------------------------------------------------------------
# Step 1 — the application tarball (one compressed `homevault` binary per
# architecture), exactly the asset installer/install.sh publishes.
# ---------------------------------------------------------------------------
TARBALL="$ROOT/installer/release/homevault-$ARCH.tar.gz"
if [ "$FRESH_PACKAGE" -eq 1 ] || [ ! -f "$TARBALL" ]; then
    echo "▶ Building the application tarball (installer/install.sh --package)…"
    (cd "$ROOT" && bash installer/install.sh --package)
fi
if [ ! -f "$TARBALL" ]; then
    echo "Missing $TARBALL" >&2
    exit 1
fi
if [ -f "$TARBALL.sha256" ]; then
    EXPECTED=$(awk '{print $1}' "$TARBALL.sha256")
    ACTUAL=$(sha256sum "$TARBALL" | awk '{print $1}')
    if [ "$ACTUAL" != "$EXPECTED" ]; then
        echo "SHA-256 mismatch for $TARBALL — rebuild with --fresh-package" >&2
        exit 1
    fi
fi
echo "✓ Application tarball: $TARBALL"

build_variant() {
    local V="$1"
    local WORK="$OS_DIR/build/$V"
    echo ""
    echo "▶ Building variant '$V' (version $VERSION)…"

    rm -rf "$WORK"
    mkdir -p "$WORK/config"
    cp -a "$OS_DIR/config/." "$WORK/config/"
    # Variant overlay — after the shared config so HV_OS_VARIANT wins.
    mkdir -p "$WORK/config/includes.chroot"
    cp -a "$OS_DIR/variants/$V/includes.chroot/." "$WORK/config/includes.chroot/"

    # The preinstalled application: extracted straight into the image rootfs
    # (the same binary the curl|bash installer would place in /opt/homevault).
    mkdir -p "$WORK/config/includes.chroot/opt/homevault"
    tar -xzf "$TARBALL" -C "$WORK/config/includes.chroot/opt/homevault"
    chmod 0755 "$WORK/config/includes.chroot/opt/homevault/homevault"

    if [ "$SKIP_CLAMAV" -eq 1 ]; then
        mkdir -p "$WORK/config/includes.chroot/root"
        : > "$WORK/config/includes.chroot/root/.hv-os-skip-clamav"
    fi

    # Live kernel command line:
    #  - hostname=homevault          → reachable as homevault.local (avahi)
    #  - persistence (variant A)     → live-boot keeps OS tweaks on a
    #                                  partition labelled "persistence" if
    #                                  the user creates one (optional)
    local BOOTAPPEND="boot=live components hostname=homevault quiet"
    if [ "$V" = "persistent" ]; then
        BOOTAPPEND="boot=live components hostname=homevault persistence quiet"
    fi

    local LB_OPTS=(
        noauto
        --mode debian
        --distribution bookworm
        --architecture amd64
        --archive-areas "main contrib non-free non-free-firmware"
        --mirror-bootstrap http://deb.debian.org/debian
        --parent-mirror-bootstrap http://deb.debian.org/debian
        --mirror-binary http://deb.debian.org/debian
        --parent-mirror-binary http://deb.debian.org/debian
        --mirror-binary-security http://security.debian.org/debian-security
        --parent-mirror-binary-security http://security.debian.org/debian-security
        --backports true
        --binary-images iso-hybrid
        --debian-installer none
        --iso-volume "HomeVaultOS $VERSION"
        --iso-application "HomeVaultOS"
        --iso-publisher "HomeVault; https://github.com/nicoleroux35/HomeVault"
        --bootappend-live "$BOOTAPPEND"
    )
    # zstd shrinks the squashfs noticeably; only pass it when this live-build
    # knows the option (older releases default to gzip silently).
    if lb config --help 2>&1 | grep -q -- --squashfs-compression; then
        LB_OPTS+=(--squashfs-compression zstd)
    fi

    (cd "$WORK" && lb config "${LB_OPTS[@]}" >/dev/null)

    if [ "$V" = "selfinstall" ]; then
        # The umbreLOS USB-installer pattern: the ISO embeds a PREBUILT disk
        # image and the first boot only flashes it with dd (hv-firstboot) —
        # no partitioning/GRUB/fstab at runtime, where it is fragile. Build
        # the live chroot, bake the disk image from it, then assemble the
        # binary ISO with the image on board. The .img.xz is also published
        # standalone (flashable directly with balenaEtcher/Rufus).
        (cd "$WORK" && lb bootstrap && lb chroot)
        mkdir -p "$DIST_DIR"
        local IMGXZ="$DIST_DIR/homevaultos-$VERSION-amd64.img.xz"
        bash "$OS_DIR/build-disk-image.sh" "$WORK/chroot" "$IMGXZ"
        mkdir -p "$WORK/config/includes.binary/homevaultos"
        cp "$IMGXZ" "$WORK/config/includes.binary/homevaultos/homevaultos-amd64.img.xz"
        (cd "$WORK" && lb binary)
    else
        (cd "$WORK" && lb build)
    fi

    mkdir -p "$DIST_DIR"
    local OUT="$DIST_DIR/homevaultos-$VERSION-$V-amd64.iso"
    mv "$WORK/live-image-amd64.hybrid.iso" "$OUT"
    sha256sum "$OUT" | awk '{print $1}' > "$OUT.sha256"
    echo "✓ $OUT"
    echo "  sha256: $(cat "$OUT.sha256")"
    if [ "$KEEP" -eq 0 ]; then
        rm -rf "$WORK"
    else
        echo "  (workspace kept: $WORK)"
    fi
}

if [ "$VARIANT" = "all" ]; then
    build_variant persistent
    build_variant selfinstall
else
    build_variant "$VARIANT"
fi

echo ""
echo "All done. Test with: ./os/test-qemu.sh $DIST_DIR/<iso> [--uefi]"

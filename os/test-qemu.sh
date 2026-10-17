#!/bin/bash
set -euo pipefail

# HomeVaultOS — QEMU smoke test.
#
#   ./os/test-qemu.sh os/dist/homevaultos-1.0.0-persistent-amd64.iso
#   ./os/test-qemu.sh os/dist/homevaultos-1.0.0-selfinstall-amd64.iso --uefi
#   ./os/test-qemu.sh <iso> --disk 64   # data/target disk size in GiB
#   ./os/test-qemu.sh <iso> --nographic # serial console only
#
# The VM gets one extra blank disk: variant A provisions it as HV-DATA,
# variant B clones the system onto it and reboots (watch it twice: once from
# the ISO, then from the installed disk).

ISO=""
DISK_GB=64
UEFI=0
NOGRAPHIC=0
while [ $# -gt 0 ]; do
    case "$1" in
        --uefi) UEFI=1; shift ;;
        --disk) DISK_GB="${2:?--disk needs a value}"; shift 2 ;;
        --nographic) NOGRAPHIC=1; shift ;;
        -h|--help) sed -n '2,14p' "$0"; exit 0 ;;
        *) if [ -z "$ISO" ]; then ISO="$1"; else echo "Unexpected argument: $1" >&2; exit 1; fi; shift ;;
    esac
done
[ -n "$ISO" ] && [ -f "$ISO" ] || { echo "Usage: $0 <iso> [--uefi] [--disk GB] [--nographic]" >&2; exit 1; }

HERE="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
BUILD_DIR="$HERE/build"
mkdir -p "$BUILD_DIR"
DISK="$BUILD_DIR/test-disk-$DISK_GB-GiB.qcow2"
if [ ! -f "$DISK" ] || [ "$(stat -c%s "$DISK")" -lt 1048576 ]; then
    qemu-img create -f qcow2 "$DISK" "${DISK_GB}G" >/dev/null
fi

ACCEL="tcg"
CPU="max"
if [ -w /dev/kvm ]; then
    ACCEL="kvm"
    CPU="host"
fi

ARGS=(
    -m 4096
    -smp 4
    -cpu "$CPU"
    -machine q35
    -accel "$ACCEL"
    -cdrom "$ISO"
    -drive file="$DISK",if=virtio,format=qcow2
    -netdev user,id=n0 -device virtio-net-pci,netdev=n0
    -boot d
)

if [ "$UEFI" -eq 1 ]; then
    CODE=""
    VARS_SRC=""
    for f in /usr/share/OVMF/OVMF_CODE_4M.fd /usr/share/OVMF/OVMF_CODE.fd; do
        [ -f "$f" ] && CODE="$f" && break
    done
    for f in /usr/share/OVMF/OVMF_VARS_4M.fd /usr/share/OVMF/OVMF_VARS.fd; do
        [ -f "$f" ] && VARS_SRC="$f" && break
    done
    if [ -n "$CODE" ] && [ -n "$VARS_SRC" ]; then
        VARS="$BUILD_DIR/test-efi-vars.fd"
        cp "$VARS_SRC" "$VARS"
        ARGS+=(-drive if=pflash,format=raw,readonly=on,file="$CODE")
        ARGS+=(-drive if=pflash,format=raw,file="$VARS")
    elif [ -f /usr/share/ovmf/OVMF.fd ]; then
        # Combined firmware bundle (Debian's ovmf package on old releases).
        ARGS+=(-bios /usr/share/ovmf/OVMF.fd)
    else
        echo "OVMF firmware not found (apt install ovmf)" >&2
        exit 1
    fi
fi

if [ "$NOGRAPHIC" -eq 1 ]; then
    ARGS+=(-nographic)
fi

echo "▶ Booting $ISO (accel=$ACCEL, extra disk: $DISK)"
exec qemu-system-x86_64 "${ARGS[@]}"

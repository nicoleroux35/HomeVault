#!/usr/bin/env bash
# ---------------------------------------------------------------------------
# Real SMB/NFS write proof — run on a CLIENT machine that can mount the NAS.
#
# The API can create a share but cannot prove a share actually carries bytes;
# that requires a second machine with the kernel client. This script mounts
# the share, writes, reads back, renames, deletes and checks the share is
# empty afterwards, so a broken export or a read-only mount fails loudly.
#
# Usage:
#   scripts/hardware/smb-nfs-acl.sh smb //nas.local/family /mnt/hv-test
#   SMB_USER=alice SMB_PASS='…' scripts/hardware/smb-nfs-acl.sh smb //nas.local/family /mnt/hv-test
#   scripts/hardware/smb-nfs-acl.sh nfs nas.local:/volume1/family /mnt/hv-test
#
# Requirements: root (mount), cifs-utils or nfs-common.
# ---------------------------------------------------------------------------
set -euo pipefail

PROTO=${1:-}
SOURCE=${2:-}
TARGET=${3:-}

if [[ -z "$PROTO" || -z "$SOURCE" || -z "$TARGET" ]]; then
    echo "usage: $0 <smb|nfs> <source> <mountpoint>" >&2
    exit 2
fi
if [[ $EUID -ne 0 ]]; then
    echo "must run as root (mount)" >&2
    exit 1
fi
command -v mount >/dev/null || { echo "mount not found" >&2; exit 1; }
mkdir -p "$TARGET"

cleanup() {
    if mountpoint -q "$TARGET"; then umount "$TARGET" 2>/dev/null || true; fi
}
trap cleanup EXIT

case "$PROTO" in
    smb)
        OPTS="iocharset=utf8,vers=3.0"
        if [[ -n "${SMB_USER:-}" ]]; then
            OPTS="$OPTS,username=$SMB_USER,password=${SMB_PASS:-}"
        fi
        mount -t cifs "$SOURCE" "$TARGET" -o "$OPTS"
        ;;
    nfs)
        mount -t nfs "$SOURCE" "$TARGET" -o vers=4.2
        ;;
    *)
        echo "unknown protocol: $PROTO (use smb or nfs)" >&2
        exit 2
        ;;
esac

echo "mounted $SOURCE on $TARGET"

NAME="hv-write-proof-$$.txt"
BODY="homevault write proof $(date -u +%Y-%m-%dT%H:%M:%SZ)"

# Optional negative case: an account/share that must be read-only. The write
# has to fail; if it succeeds the ACL/export is wrong.
if [[ -n "${HV_EXPECT_READONLY:-}" ]]; then
    if printf '%s\n' "$BODY" >"$TARGET/$NAME" 2>/dev/null; then
        rm -f "$TARGET/$NAME" 2>/dev/null || true
        echo "expected a read-only share but the write succeeded — failing" >&2
        exit 1
    fi
    echo "ok: write refused as expected on $SOURCE"
    exit 0
fi

printf '%s\n' "$BODY" >"$TARGET/$NAME"
test -f "$TARGET/$NAME"
test "$(cat "$TARGET/$NAME")" = "$BODY"

mv "$TARGET/$NAME" "$TARGET/$NAME.renamed"
test -f "$TARGET/$NAME.renamed"
rm -f "$TARGET/$NAME.renamed"
test ! -e "$TARGET/$NAME.renamed"

# The share must be writable by the tested account, not merely listable.
# Pass HV_EXPECT_READONLY=1 to assert the opposite (the write must fail).

echo "ok: write/read/rename/delete round-trip on $SOURCE"

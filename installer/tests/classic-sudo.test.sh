#!/bin/bash
# ---------------------------------------------------------------------------
# Classic sudo selection (Ubuntu 26.04 ships sudo-rs).
#
# sudo-rs rejects wildcards in command arguments, which HomeVault's scoped
# grants rely on (`mount -t zfs * /tmp/restic-snap-*`, `smartctl -H /dev/*`…),
# so `--install-sudoers` would abort there. The installer must settle sudo
# before it writes the drop-in.
#
# Like the other installer suites, this sources `install.sh` with
# `HV_LIB_ONLY=1` and never runs an install step:
#
#   bash installer/tests/classic-sudo.test.sh
# ---------------------------------------------------------------------------
set -uo pipefail

HERE=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
ROOT=$(cd -- "$HERE/../.." && pwd)

HV_LIB_ONLY=1
# shellcheck disable=SC1090,SC1091
source "$ROOT/installer/install.sh"
set +e

PASS=0
FAIL=0
ok()  { PASS=$((PASS + 1)); printf 'ok   %s\n' "$1"; }
bad() { FAIL=$((FAIL + 1)); printf 'FAIL %s\n' "$1"; }

TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

mkdir -p "$TMP/classic" "$TMP/sudors" "$TMP/sudorsym"
printf '#!/bin/sh\necho "Sudo version 1.9.17p2"\nexit 0\n' > "$TMP/classic/sudo"
printf '#!/bin/sh\necho "sudo-rs 0.2.13"\nexit 0\n' > "$TMP/sudors/sudo"
printf '#!/bin/sh\nexit 0\n' > "$TMP/sudorsym/sudo-rs"
chmod +x "$TMP/classic/sudo" "$TMP/sudors/sudo" "$TMP/sudorsym/sudo-rs"
# A `sudo` symlinked to sudo-rs whose `-V` is silent: the resolved path must
# be enough to reject it.
ln -s "$TMP/sudorsym/sudo-rs" "$TMP/sudorsym/sudo"

if PATH="$TMP/classic:$PATH" sudo_is_classic; then
    ok "classic sudo is accepted"
else
    bad "classic sudo is accepted"
fi

if PATH="$TMP/sudors:$PATH" sudo_is_classic; then
    bad "sudo-rs banner is rejected"
else
    ok "sudo-rs banner is rejected"
fi

if PATH="$TMP/sudorsym:$PATH" sudo_is_classic; then
    bad "sudo-rs resolved path is rejected"
else
    ok "sudo-rs resolved path is rejected"
fi

# No apt call on a host that already has classic sudo.
if PATH="$TMP/classic:$PATH" ensure_classic_sudo >/dev/null 2>&1; then
    ok "classic sudo needs no change"
else
    bad "classic sudo needs no change"
fi

# The drop-in is only written once sudo can validate it.
if grep -B4 -- '--install-sudoers' "$ROOT/installer/install.sh" \
        | grep -q 'ensure_classic_sudo'; then
    ok "ensure_classic_sudo runs before --install-sudoers"
else
    bad "ensure_classic_sudo runs before --install-sudoers"
fi

printf '\n%d passed, %d failed\n' "$PASS" "$FAIL"
[[ "$FAIL" -eq 0 ]]

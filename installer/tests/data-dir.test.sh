#!/bin/bash
# ---------------------------------------------------------------------------
# Data-directory placement tests (FIAB-04).
#
# Sources `installer/install.sh` with `HV_LIB_ONLY=1` and exercises the pure
# validator and the device comparison. No root, no service, no network.
#
#   bash installer/tests/data-dir.test.sh
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
eq()  { if [[ "$2" == "$3" ]]; then ok "$1"; else bad "$1 (want='$2' got='$3')"; fi; }
rc()  {
    local desc="$1" want="$2"
    shift 2
    "$@"
    local got=$?
    if [[ "$got" -eq "$want" ]]; then ok "$desc"; else bad "$desc (want rc=$want got=$got)"; fi
}

# --- accepted data directories --------------------------------------------
for path in /srv/homevault /mnt/pool/homevault /var/lib/homevault; do
    if validate_data_dir "$path" >/dev/null; then
        ok "accepts $path"
    else
        bad "refused $path"
    fi
done

# --- refused shapes, with a named reason ----------------------------------
eq "refuses an empty path" "empty" "$(validate_data_dir "")"
eq "refuses a relative path" "not absolute" "$(validate_data_dir srv/homevault)"
eq "refuses /" "root" "$(validate_data_dir /)"
eq "refuses whitespace" "whitespace" "$(validate_data_dir '/srv/my vault')"
eq "refuses traversal" "traversal" "$(validate_data_dir '/srv/../etc')"
eq "refuses a double slash" "traversal" "$(validate_data_dir '/srv//homevault')"
rc "validator returns non-zero on refusal" 1 validate_data_dir "/"

# --- device comparison -----------------------------------------------------
rc "the root directory is on the system filesystem" 0 on_system_filesystem /
if on_system_filesystem /proc; then
    bad "/proc must not be reported as the system filesystem"
else
    ok "/proc is a different filesystem"
fi

printf '\n%d passed, %d failed\n' "$PASS" "$FAIL"
[[ "$FAIL" -eq 0 ]]

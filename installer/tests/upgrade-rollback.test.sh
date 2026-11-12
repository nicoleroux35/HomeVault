#!/bin/bash
# ---------------------------------------------------------------------------
# Upgrade → failed readiness → rollback, end to end at the shell level.
#
# The existing update-engine suite proves each helper in isolation. This one
# joins them into the exact sequence an automatic rollback runs:
#
#   1. snapshot the database (service stopped, WAL included);
#   2. swap the staged binary in, keeping one previous build;
#   3. the new build migrates the schema;
#   4. readiness never comes back;
#   5. restore the previous binary AND the pre-update database;
#   6. the previous build starts against the restored schema.
#
# No root, no systemd, no network: `install.sh` is sourced with
# `HV_LIB_ONLY=1`, and the "binaries" are tiny shell scripts.
#
#   bash installer/tests/upgrade-rollback.test.sh
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

WORK=$(mktemp -d "${TMPDIR:-/tmp}/hv-upgrade-test.XXXXXX")
SERVER_PID=""
cleanup() {
    [[ -n "$SERVER_PID" ]] && kill "$SERVER_PID" 2>/dev/null
    rm -rf "$WORK"
}
trap cleanup EXIT

INSTALL="$WORK/opt/homevault"
DATA="$WORK/data"
mkdir -p "$INSTALL" "$DATA/data/backups"

make_binary() {
    local path="$1" version="$2"
    printf '#!/bin/sh\necho "%s"\n' "$version" >"$path"
    chmod 0755 "$path"
}

# --- 1. A healthy v1 install with a real database + WAL -------------------
make_binary "$INSTALL/homevault" "v1"
printf 'schema=v1\nrows=v1\n' >"$DATA/data/homevault.db"
printf 'wal-from-v1\n' >"$DATA/data/homevault.db-wal"
printf 'shm-from-v1\n' >"$DATA/data/homevault.db-shm"

eq "start: current binary is v1" "v1" "$("$INSTALL/homevault")"

# The service is stopped before the snapshot; this is what install.sh does.
rc "snapshot the pre-update database" 0 hv_snapshot_database "$DATA"
if [[ -f "$DATA/data/backups/pre-update.db" ]]; then
    ok "snapshot: main database copied"
else
    bad "snapshot: main database missing"
fi
if [[ -f "$DATA/data/backups/pre-update.db-wal" ]]; then
    ok "snapshot: WAL copied with the database"
else
    bad "snapshot: WAL missing (committed transactions would be lost)"
fi

# --- 2. Swap the new build in --------------------------------------------
make_binary "$WORK/v2.staged" "v2"
rc "swap: stage and install v2" 0 hv_swap_binary "$WORK/v2.staged" "$INSTALL"
eq "swap: current is v2" "v2" "$("$INSTALL/homevault")"
eq "swap: previous is v1" "v1" "$("$INSTALL/homevault.previous")"
test -x "$INSTALL/homevault.previous" && ok "swap: previous is executable" || bad "swap: previous is not executable"

# --- 3. The new build migrates the schema, then readiness fails ----------
# The v2 process worked on the live database and did not cleanly checkpoint.
printf 'schema=v2\nrows=v2\n' >"$DATA/data/homevault.db"
printf 'wal-from-v2\n' >"$DATA/data/homevault.db-wal"
printf 'stale-shm\n' >"$DATA/data/homevault.db-shm"

# A real readiness probe: a local HTTP server that answers 200, then a port
# nothing listens on. This exercises hv_wait_healthy rather than assuming it.
mkdir -p "$WORK/www"
printf 'ok\n' >"$WORK/www/ready"
cat >"$WORK/ready_server.py" <<'PY'
import http.server
import os
import socketserver
import sys

os.chdir(sys.argv[1])
socketserver.TCPServer.allow_reuse_address = True
with socketserver.TCPServer(("127.0.0.1", 0), http.server.SimpleHTTPRequestHandler) as httpd:
    print(httpd.server_address[1], flush=True)
    httpd.serve_forever()
PY
python3 -u "$WORK/ready_server.py" "$WORK/www" >"$WORK/http.port" 2>"$WORK/http.log" &
SERVER_PID=$!
PORT=""
for _ in $(seq 1 50); do
    PORT=$(head -n1 "$WORK/http.port" 2>/dev/null | tr -dc '0-9')
    [[ -n "$PORT" ]] && break
    sleep 0.1
done
if [[ -n "$PORT" ]]; then
    rc "health: a ready endpoint passes" 0 hv_wait_healthy "http://127.0.0.1:${PORT}/ready" 3
    rc "health: the failed build never becomes ready" 1 hv_wait_healthy "http://127.0.0.1:1/ready" 1
else
    bad "health: the local HTTP probe server did not start"
fi
kill "$SERVER_PID" 2>/dev/null
SERVER_PID=""

# --- 4. Automatic rollback: binary + database ----------------------------
rc "rollback: restore the previous binary" 0 hv_rollback_binary "$INSTALL"
eq "rollback: current is v1 again" "v1" "$("$INSTALL/homevault")"
eq "rollback: the failed build is kept for inspection" "v2" "$("$INSTALL/homevault.failed")"
if [[ ! -e "$INSTALL/homevault.previous" ]]; then
    ok "rollback: previous slot consumed (no accidental double rollback)"
else
    bad "rollback: previous slot still present"
fi

rc "rollback: restore the pre-update database" 0 hv_restore_database "$DATA"

DB_CONTENT=$(cat "$DATA/data/homevault.db")
eq "database: v1 schema restored" "schema=v1
rows=v1" "$DB_CONTENT"
if [[ -f "$DATA/data/homevault.db-wal" ]]; then
    eq "database: v1 WAL restored" "wal-from-v1" "$(cat "$DATA/data/homevault.db-wal")"
else
    bad "database: v1 WAL missing (committed transactions lost)"
fi
if [[ ! -e "$DATA/data/homevault.db-shm" ]]; then
    ok "database: stale SHM removed (SQLite rebuilds it)"
else
    bad "database: stale SHM survived (it belongs to the failed build)"
fi

# The restored binary really starts against the restored schema.
STARTED=$("$INSTALL/homevault")
eq "start: the previous build runs after rollback" "v1" "$STARTED"
grep -q 'schema=v1' "$DATA/data/homevault.db" && ok "start: restored build sees the v1 schema" || bad "start: schema mismatch after rollback"

# --- 5. A second rollback is refused -------------------------------------
rc "rollback: a second rollback is refused (no previous build)" 1 hv_rollback_binary "$INSTALL"

# --- 6. A failed upgrade can be retried: swap a fresh build again --------
make_binary "$WORK/v3.staged" "v3"
rc "retry: a later upgrade still swaps" 0 hv_swap_binary "$WORK/v3.staged" "$INSTALL"
eq "retry: current is v3" "v3" "$("$INSTALL/homevault")"
eq "retry: previous is v1" "v1" "$("$INSTALL/homevault.previous")"

# ---------------------------------------------------------------------------
printf '\nupgrade/rollback: %d passed, %d failed\n' "$PASS" "$FAIL"
[[ "$FAIL" -eq 0 ]]

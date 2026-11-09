#!/bin/bash
# ---------------------------------------------------------------------------
# Update-engine unit tests (P2-8).
#
# These tests are intentionally *not* run by the installer or by CI: they
# exercise the shell functions by sourcing `installer/install.sh` with
# `HV_LIB_ONLY=1`. Run them by hand on a disposable host:
#
#   bash installer/tests/update-engine.test.sh
#
# They need bash, coreutils, openssl and python3 — the same dependencies the
# installer already requires. No root, no service, no network.
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

WORK=$(mktemp -d /tmp/hv-update-test.XXXXXX)
trap 'rm -rf "$WORK"' EXIT

# --- version comparison ----------------------------------------------------
rc "version: equal" 0 hv_version_ge 1.2.3 1.2.3
rc "version: newer patch" 0 hv_version_ge 1.2.4 1.2.3
rc "version: older patch" 1 hv_version_ge 1.2.3 1.2.4
rc "version: missing component" 0 hv_version_ge 1.2 1.2.0
rc "version: prerelease suffix" 0 hv_version_ge 1.2.3-beta 1.2.3
rc "version: major wins" 0 hv_version_ge 2.0.0 1.99.99
rc "version: older major" 1 hv_version_ge 1.99.99 2.0.0

# --- manifest parsing + signature -----------------------------------------
if ! openssl genpkey -algorithm ED25519 -out "$WORK/key.pem" 2>/dev/null; then
    echo "SKIP: this OpenSSL cannot generate an Ed25519 key" >&2
    exit 0
fi
openssl pkey -in "$WORK/key.pem" -pubout -out "$WORK/pub.pem" 2>/dev/null

write_manifest() {
    cat >"$WORK/manifest.json" <<'EOF'
{"version":"1.1.0","arch":"x86_64","url":"https://example.invalid/h.tar.gz","sha256":"deadbeef","size":42}
EOF
}

write_manifest
if ! openssl pkeyutl -sign -inkey "$WORK/key.pem" -rawin \
        -in "$WORK/manifest.json" -out "$WORK/manifest.json.sig" 2>/dev/null; then
    echo "SKIP: this OpenSSL cannot sign with pkeyutl -rawin" >&2
    exit 0
fi

rc "manifest: valid signature" 0 \
    hv_verify_signature "$WORK/manifest.json" "$WORK/manifest.json.sig" "$WORK/pub.pem"

# One appended byte is enough to invalidate the signature.
printf ' ' >>"$WORK/manifest.json"
rc "manifest: tamper detected" 1 \
    hv_verify_signature "$WORK/manifest.json" "$WORK/manifest.json.sig" "$WORK/pub.pem"
write_manifest

rc "manifest: invalid signature file" 1 \
    hv_verify_signature "$WORK/manifest.json" "$WORK/missing.sig" "$WORK/pub.pem"

eq "manifest: version field" "1.1.0" "$(hv_manifest_field "$WORK/manifest.json" version)"
eq "manifest: arch field" "x86_64" "$(hv_manifest_field "$WORK/manifest.json" arch)"
eq "manifest: absent field rc" "1" \
    "$(hv_manifest_field "$WORK/manifest.json" nope >/dev/null 2>&1; echo $?)"

# --- archive verification --------------------------------------------------
printf 'hello world\n' >"$WORK/archive.tar.gz"
SHA=$(hv_sha256 "$WORK/archive.tar.gz")
SIZE=$(hv_size "$WORK/archive.tar.gz")
rc "archive: size+sha match" 0 hv_verify_archive "$WORK/archive.tar.gz" "$SHA" "$SIZE"
rc "archive: wrong sha" 1 hv_verify_archive "$WORK/archive.tar.gz" "0000" "$SIZE"
rc "archive: wrong size" 1 hv_verify_archive "$WORK/archive.tar.gz" "$SHA" "$((SIZE + 1))"
rc "archive: missing file" 1 hv_verify_archive "$WORK/nope.tar.gz" "$SHA" "$SIZE"

# --- smoke test ------------------------------------------------------------
printf '#!/bin/sh\necho "homevault 1.1.0"\n' >"$WORK/fake-good"
chmod +x "$WORK/fake-good"
printf '#!/bin/sh\nexit 7\n' >"$WORK/fake-bad"
chmod +x "$WORK/fake-bad"

rc "smoke: version match" 0 hv_smoke_binary "$WORK/fake-good" "1.1.0"
eq "smoke: exposes staged version" "1.1.0" "${HV_STAGED_VERSION:-}"
rc "smoke: version mismatch" 1 hv_smoke_binary "$WORK/fake-good" "9.9.9"
rc "smoke: refuses a crashing binary" 1 hv_smoke_binary "$WORK/fake-bad" "1.1.0"
rc "smoke: refuses a non-executable" 1 hv_smoke_binary "$WORK/key.pem" "1.1.0"

# --- atomic swap + rollback ------------------------------------------------
mkdir -p "$WORK/install"
printf '#!/bin/sh\necho "homevault 1.0.0"\n' >"$WORK/install/homevault"
chmod +x "$WORK/install/homevault"
printf '#!/bin/sh\necho "homevault 1.1.0"\n' >"$WORK/new"
chmod +x "$WORK/new"

rc "swap: succeeds" 0 hv_swap_binary "$WORK/new" "$WORK/install"
eq "swap: current is the new build" "1.1.0" \
    "$("$WORK/install/homevault" --version | awk '{print $2}')"
eq "swap: previous is the old build" "1.0.0" \
    "$("$WORK/install/homevault.previous" --version | awk '{print $2}')"
# The live binary dir must not contain a half-written staging file after a swap.
eq "swap: no .new left behind" "0" \
    "$(find "$WORK/install" -maxdepth 1 -name '.homevault.new' | wc -l | tr -d ' ')"

rc "rollback: succeeds" 0 hv_rollback_binary "$WORK/install"
eq "rollback: current is the old build" "1.0.0" \
    "$("$WORK/install/homevault" --version | awk '{print $2}')"
eq "rollback: failed build kept" "1.1.0" \
    "$("$WORK/install/homevault.failed" --version | awk '{print $2}')"
rc "rollback: refuses without a previous build" 1 hv_rollback_binary "$WORK/empty-install"

# --- result + history ------------------------------------------------------
rc "result: writes" 0 hv_write_result "$WORK/data" succeeded 1.1.0 1.0.0 "ok"
eq "result: status" "succeeded" \
    "$(python3 -c 'import json,sys;print(json.load(open(sys.argv[1]))["status"])' \
        "$WORK/data/updates/last-result.json")"
eq "result: previous version" "1.0.0" \
    "$(python3 -c 'import json,sys;print(json.load(open(sys.argv[1]))["previous_version"])' \
        "$WORK/data/updates/last-result.json")"
eq "result: one history line" "1" \
    "$(wc -l <"$WORK/data/updates/history.jsonl" | tr -d ' ')"
# Each line must be a complete JSON object: `history.jsonl` is consumed line by
# line, unlike the pretty-printed `last-result.json`.
eq "result: history line is valid JSONL" "succeeded" \
    "$(python3 -c 'import json,sys; print(json.loads(open(sys.argv[1]).readline())["status"])' \
        "$WORK/data/updates/history.jsonl")"

# --- pre-update database snapshot/restore (FIAB-03) -----------------------
mkdir -p "$WORK/db/data"
printf 'old-db\n' >"$WORK/db/data/homevault.db"
printf 'old-wal\n' >"$WORK/db/data/homevault.db-wal"
rc "database: snapshot succeeds" 0 hv_snapshot_database "$WORK/db"
eq "database: main file copied" "old-db" \
    "$(cat "$WORK/db/data/backups/pre-update.db")"
eq "database: WAL copied" "old-wal" \
    "$(cat "$WORK/db/data/backups/pre-update.db-wal")"

printf 'new-db\n' >"$WORK/db/data/homevault.db"
printf 'new-wal\n' >"$WORK/db/data/homevault.db-wal"
printf 'new-shm\n' >"$WORK/db/data/homevault.db-shm"
rc "database: restore succeeds" 0 hv_restore_database "$WORK/db"
eq "database: main file restored" "old-db" \
    "$(cat "$WORK/db/data/homevault.db")"
eq "database: WAL restored" "old-wal" \
    "$(cat "$WORK/db/data/homevault.db-wal")"
eq "database: stale shm removed" "0" \
    "$(test -e "$WORK/db/data/homevault.db-shm" && echo 1 || echo 0)"
rc "database: restore without snapshot is a no-op" 0 hv_restore_database "$WORK/empty-db"

printf '\n%d passed, %d failed\n' "$PASS" "$FAIL"
[[ "$FAIL" -eq 0 ]]

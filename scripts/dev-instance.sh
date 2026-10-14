#!/usr/bin/env bash
# Start an isolated HomeVault instance for development.
#
# HomeVault is normally a single appliance on a LAN, but development needs
# several instances side by side (one per branch/agent) without them fighting
# over the same port, database or mDNS name. This script gives each instance:
#
#   * its own `DATA_DIRECTORY` (SQLite DB, uploads, logs),
#   * its own TCP port,
#   * its own mDNS hostname (`HV_MDNS_HOSTNAME`), so it answers as
#     `<name>.local` instead of `homevault.local` — see `src/mdns.rs`.
#
# Usage:
#   scripts/dev-instance.sh [NAME] [PORT]
#   scripts/dev-instance.sh dev-1 8081
#
# With no arguments it derives a stable name/port from the current git branch
# (or `default`), so two checkouts never collide. The instance runs in the
# foreground; Ctrl-C stops it. Data lives under
# `$HOME/.local/share/homevault-dev/<name>`.
#
# Prerequisites: a debug build (`cargo build`). The script builds if the
# binary is missing or older than the sources.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

# ── Derive a name + port ────────────────────────────────────────────────────
slugify() {
  printf '%s' "$1" \
    | tr '[:upper:]' '[:lower:]' \
    | sed -E 's#[^a-z0-9]+#-#g; s#^-+##; s#-+$##' \
    | cut -c1-32
}

if [[ $# -ge 1 && -n "${1:-}" ]]; then
  NAME="$(slugify "$1")"
else
  BRANCH="$(git rev-parse --abbrev-ref HEAD 2>/dev/null || echo default)"
  NAME="$(slugify "$BRANCH")"
  [[ -z "$NAME" ]] && NAME="default"
fi

# Stable port from the name (hash → 8100..8499), unless an explicit port is
# given. A fixed offset keeps the same branch on the same port across runs.
if [[ $# -ge 2 && -n "${2:-}" ]]; then
  PORT="$2"
else
  HASH="$(printf '%s' "$NAME" | cksum | awk '{print $1}')"
  PORT=$(( 8100 + (HASH % 400) ))
fi

DATA_DIR="${HOMEVAULT_DEV_DATA_ROOT:-$HOME/.local/share/homevault-dev}/$NAME"
mkdir -p "$DATA_DIR"

# ── Ensure a fresh-enough binary ────────────────────────────────────────────
BIN="$REPO_ROOT/target/debug/homevault"
if [[ ! -x "$BIN" ]] || [[ -n "$(find "$REPO_ROOT/src" "$REPO_ROOT/Cargo.toml" -newer "$BIN" -print -quit 2>/dev/null)" ]]; then
  echo "==> Building (target/debug/homevault)…"
  CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-2}" cargo build
fi

echo "==> HomeVault dev instance"
echo "    name:  $NAME"
echo "    url:   http://localhost:$PORT  (mDNS: http://$NAME.local:$PORT)"
echo "    data:  $DATA_DIR"
echo

exec env \
  PORT="$PORT" \
  DATA_DIRECTORY="$DATA_DIR" \
  HV_MDNS_HOSTNAME="$NAME" \
  LOG_LEVEL="${LOG_LEVEL:-debug}" \
  "$BIN"

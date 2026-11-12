#!/usr/bin/env bash
# ---------------------------------------------------------------------------
# Load / soak runner for the real HTTP server.
#
# The Rust suite (tests/soak_integration.rs) covers the in-process stack on
# every `cargo test`. This script is for the *deployed* binary: it drives many
# simultaneous sessions, large transfers and a deep tree over real sockets.
#
# Prerequisites:
#   - a running HomeVault instance with a seeded admin account
#   - curl, and optionally k6 for the HTTP profile
#
# Usage:
#   BASE_URL=http://localhost:8080 ./scripts/soak.sh
#   HV_SOAK_SECONDS=300 HV_SOAK_CONCURRENCY=64 ./scripts/soak.sh
#   ./scripts/soak.sh --k6          # also run scripts/load/homevault.k6.js
# ---------------------------------------------------------------------------
set -euo pipefail

ROOT=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
SECONDS_TO_RUN=${HV_SOAK_SECONDS:-30}
CONCURRENCY=${HV_SOAK_CONCURRENCY:-16}
FILE_MB=${HV_SOAK_FILE_MB:-64}
BASE_URL=${BASE_URL:-http://localhost:8080}
ADMIN_USER=${E2E_ADMIN_USER:-e2eadmin}
ADMIN_PASS=${E2E_ADMIN_PASS:-E2eSecret123}

echo "▶ In-process soak (${SECONDS_TO_RUN}s at concurrency ${CONCURRENCY})"
(
  cd "$ROOT"
  HV_SOAK_SECONDS="$SECONDS_TO_RUN" \
  HV_SOAK_CONCURRENCY="$CONCURRENCY" \
  HV_SOAK_FILE_MB="$FILE_MB" \
  cargo test --test soak_integration -- --nocapture
)

if [[ "${1:-}" == "--k6" ]]; then
  if ! command -v k6 >/dev/null 2>&1; then
    echo "k6 is not installed — skipping the HTTP profile (see scripts/load/README.md)." >&2
    exit 0
  fi
  echo "▶ k6 HTTP profile against $BASE_URL"
  BASE_URL="$BASE_URL" ADMIN_USER="$ADMIN_USER" ADMIN_PASS="$ADMIN_PASS" \
    SOAK_DURATION="${SECONDS_TO_RUN}s" SOAK_VUS="$CONCURRENCY" \
    k6 run "$ROOT/scripts/load/homevault.k6.js"
fi

echo "✓ soak complete"

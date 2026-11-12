#!/usr/bin/env bash
# ---------------------------------------------------------------------------
# Coverage for both halves of the product.
#
# Frontend: Vitest + v8 coverage, with the ratchet thresholds in
#           frontend/vitest.config.ts.
# Backend:  cargo-llvm-cov. `--lib` is the fast, always-on gate; set
#           HV_COVERAGE_ALL=1 to instrument the integration suites too (slow).
#
# Install once:
#   cargo install cargo-llvm-cov --locked
#   rustup component add llvm-tools-preview
#
# Usage:
#   ./scripts/coverage.sh
#   HV_COVERAGE_ALL=1 HV_COVERAGE_MIN_LINES=55 ./scripts/coverage.sh
# ---------------------------------------------------------------------------
set -euo pipefail

ROOT=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
MIN_LINES=${HV_COVERAGE_MIN_LINES:-0}
mkdir -p "$ROOT/target/coverage"

echo "▶ Frontend coverage (Vitest + v8)"
(cd "$ROOT/frontend" && npm run test:coverage)

echo "▶ Backend coverage (cargo-llvm-cov)"
if ! command -v cargo-llvm-cov >/dev/null 2>&1; then
    echo "cargo-llvm-cov is not installed — skipping the Rust half." >&2
    echo "Install it with: cargo install cargo-llvm-cov --locked && rustup component add llvm-tools-preview" >&2
    exit 0
fi

ARGS=(--lcov --output-path "$ROOT/target/coverage/lcov.info" --fail-under-lines "$MIN_LINES")
if [[ "${HV_COVERAGE_ALL:-0}" == "1" ]]; then
    ARGS+=(--tests --no-fail-fast)
else
    ARGS+=(--lib)
fi

(cd "$ROOT" && cargo llvm-cov "${ARGS[@]}")

echo
echo "✓ coverage written to target/coverage/ and frontend/coverage/"

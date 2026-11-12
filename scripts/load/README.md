# Load / soak profiles

Two complementary layers:

| Layer | File | Runs where |
|---|---|---|
| In-process stack (Axum router + in-memory SQLite + `tower::oneshot`) | `tests/soak_integration.rs` | every `cargo test`, fast and hermetic |
| Deployed HTTP server (real sockets, real TLS) | `scripts/load/homevault.k6.js` | a disposable host or staging VM, opt-in |

## What the Rust suite covers

- **Several simultaneous users** — one admin + seven members, each with its
  own session, 16 parallel list requests; p95 must stay under 5 s.
- **Large transfer** — an 8 MiB (configurable) multipart upload, then a
  streaming download compared byte-for-byte with SHA-256.
- **Deep tree** — 1 000 files across 40 nested directories; both the listing
  and the recursive search must stay bounded (`HV_SOAK_*` knobs below).
- **Endurance** — a mixed read workload for `HV_SOAK_SECONDS`; every request
  must return HTTP 200 with `success: true`.

## Knobs

| Variable | Default | Meaning |
|---|---|---|
| `HV_SOAK_SECONDS` | `2` | endurance window (max 120) |
| `HV_SOAK_CONCURRENCY` | `16` | parallel requests (max 128) |
| `HV_SOAK_FILE_MB` | `8` | large-file size (max 256) |

```bash
HV_SOAK_SECONDS=60 HV_SOAK_CONCURRENCY=32 cargo test --test soak_integration -- --nocapture
./scripts/soak.sh --k6          # both layers against a running server
```

## k6 profile

```bash
k6 run scripts/load/homevault.k6.js
BASE_URL=https://hv.example ADMIN_USER=admin ADMIN_PASS='…' \
  SOAK_VUS=64 SOAK_DURATION=10m k6 run scripts/load/homevault.k6.js
```

The run fails on more than 1 % failed requests or a p95 above 2 s. Run it on
a disposable host: it writes a temporary file into the first Explorer root.

## Not covered automatically

A real multi-hour soak with a real SMB/NFS client saturating a disk, and a
memory-leak verdict over days, still need a lab host — see
`docs/guides/hardware-testing.md`.

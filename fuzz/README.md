# cargo-fuzz targets

Seven targets for the hand-written parsers and validators that process
untrusted or semi-trusted input:

| Target | Surface |
|---|---|
| `proc_parsers` | `/proc` text, SMART attributes, mdadm `--detail`, write-cache |
| `remote_mounts` | CIFS/NFS name, server, source, options, credentials |
| `share_config` | SMB/NFS share names, paths, comments, options, JSON config |
| `app_manifest` | community/third-party TOML manifests |
| `docker_inputs` | image references, digests, `daemon.json` |
| `upload_filename` | multipart filenames (upload and `Content-Disposition`) |
| `ws_discovery` | host/workgroup strings spliced into the discovery unit |

## Run

```bash
rustup toolchain install nightly
cargo install cargo-fuzz --locked

cd fuzz
cargo +nightly fuzz run upload_filename
cargo +nightly fuzz run proc_parsers -- -max_total_time=300
```

Crashes land in `fuzz/artifacts/<target>/`. Reproduce with:

```bash
cargo +nightly fuzz run <target> fuzz/artifacts/<target>/<crash-file>
```

The nightly campaign is in `.github/workflows/fuzz.yml` (60 s per target).
For every push, `tests/fuzz_smoke.rs` runs the same public entry points with
deterministic hostile inputs on stable, so a regression is caught without
nightly.

## Invariants the targets assert

- `upload_filename`: a returned name is one path component, non-empty, with no
  control character; the download sanitizer removes quotes, backslashes and
  controls.
- `ws_discovery`: the rendered unit is non-empty and NUL-free.
- The parser targets are panic-free by construction: a panic is the finding.

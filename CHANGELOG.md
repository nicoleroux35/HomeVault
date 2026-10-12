# Changelog

All notable changes to HomeVault are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the versioning
follows [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added

- GitHub community health files: `LICENSE` (MIT), `SECURITY.md`,
  `CONTRIBUTING.md`, `CODE_OF_CONDUCT.md`, issue/PR templates, Dependabot,
  `CODEOWNERS`, `.editorconfig`, `.gitattributes`.
- SPDX metadata (`license`, `repository`, `homepage`) in `Cargo.toml`,
  `frontend/package.json`, and root `package.json`.

### Fixed

- **The AI assistant could not answer about 25 of HomeVault's own features.**
  The RAG knowledge base indexed 20 feature documents while `docs/guides/` had
  grown to 46 files, so a question about Syncthing, the downloader,
  notifications, the desktop shell, LUKS encryption, RAID, the cache tier,
  widgets or a dozen other shipped features got "I don't know about that". The
  knowledge base now documents all 45 indexable guides — including the account
  page, login and recovery, the setup wizard, groups, antivirus,
  anti-ransomware, the firewall, LUKS encryption, the RAID wizard, snapshots,
  the cache tier, Syncthing, the downloader, the AI chat and its knowledge
  base, notifications, widgets, the desktop shell, the app launcher, themes,
  wallpapers, transfer quotas, mDNS, server control and avatars — and guide
  coverage is enforced by a test that enumerates `docs/guides/` instead of
  relying on the written rule.
- **Cloud Sync read and wrote a different rclone config than the one it parsed.**
  `GET`/`POST /rclone/config` and the add/delete-remote handlers hardcoded
  `/etc/rclone.conf`, while the remote list honoured the `RCLONE_CONFIG`
  override — so an admin whose config lives elsewhere saw a list and an editor
  describing two different files, and saving failed with `Permission denied`
  against the root-owned default. All four now go through a single
  `rclone_config_path()` resolver, which `parse_rclone_config()` uses too.
- **`JobType::Custom` could not be serialized, which silently broke progress
  reporting for every feature job.** `JobType` was internally tagged
  (`#[serde(tag = "type")]`) with a `Custom(String)` newtype variant, which
  serde cannot serialize at all. `handle_progress_sse` falls back to an empty
  payload on a serialization error, so a directory copy/move, Docker install,
  antivirus scan, RAG reindex, VM create, rescue, cloud sync or Ollama pull
  emitted an empty SSE `data:` line — the client's `JSON.parse` threw, no
  progress or completion callback fired, and the Explorer's directory
  copy/move reported failure for a job that had succeeded. `GET
  /api/progress/jobs` also panicked while any such job was tracked. Now uses
  serde **adjacent** tagging: built-in kinds keep their exact wire shape
  (`{"type":"backup"}`), feature jobs become
  `{"type":"custom","name":"explorer_copy"}`.
- **`GET /api/progress/{job_id}/status` returned a placeholder.** It answered
  a hardcoded `Running` / 0 % / `"Waiting..."` for any tracked id, so a job at
  87 %, a completed job and a failed job all read back as "running, 0 %". It
  now returns the job's stored last event — the same snapshot as the SSE
  stream and the activity feed — and 400s for an unknown or removed id.
- **Deleting a user now revokes the share permissions it held directly.**
  `handle_user_delete` previously only *logged* an intent to revoke: the
  `share_permissions` rows (keyed by grantee name, with no foreign key to
  `users`) and the real POSIX ACLs were both left in place, so recreating an
  account with the same username silently inherited the deleted user's access.
  The handler now calls `PermissionService::revoke_all_user_permissions`, which
  removes the filesystem ACLs (best-effort) and the database rows. Group grants
  are untouched — they belong to the remaining members. Regression guard:
  `tests/users_integration.rs` →
  `test_user_delete_revokes_its_direct_share_permissions_only`.

### Changed

- **Static assets are now cached, validated and pre-compressed.** The
  `include_dir!`-embedded files used to be served as raw bytes with no
  `ETag` and no `Cache-Control` on `/wallpapers/*`, `/static/*` and the icon
  sprites, so a browser could never revalidate and re-downloaded several MB
  per visit. `spa.rs` now builds a `path → Asset` table once, sends an `ETag`
  (distinct per content coding), answers `If-None-Match` with `304`, and sets
  the cache lifetime per route — `immutable` for the content-hashed bundles,
  the wallpapers and `static/`; one day plus revalidation for the
  regenerated sprite; nothing for `index.html` / `sw.js`. The blanket
  `SetResponseHeaderLayer` for `/assets` and `/fonts` was replaced by this
  per-route policy. `frontend/scripts/precompress-dist.mjs` (npm `postbuild`)
  writes `.br` / `.gz` siblings that the server serves directly instead of
  re-compressing every response, and the `tower-http` `fs` + `set-header`
  features are no longer enabled.
- **The unified icon sprite is subset to what the frontend references**,
  dropping 7 750 glyphs to ~110 (3.6 MB → ~55 KB, and ~12 KB brotli), with
  `mapping-unified.ts` filtered to match (485 KB → 6 KB). Regeneration is now
  part of `npm run build` (`prebuild`); `HV_SPRITE_FULL=1 npm run
  icons:generate` restores the complete sprite.
- Release profile: `strip = true` and `panic = "abort"` (smaller binary; a
  panic now aborts the process instead of unwinding one task, so recovery
  relies on the systemd unit's `Restart=on-failure`).
- `installer/` is now a **single self-contained `install.sh`**. The systemd
  unit and the udiskie automount config are written inline; the sibling
  `homevault.service`, `setup-automount.sh`, `optional-packages.conf` and
  packaging `Makefile` are gone. The application binary is downloaded as a
  per-architecture asset (`homevault-<arch>.tar.gz`) from GitHub Releases
  (with optional SHA-256 verification), and `install.sh --package` builds
  those release assets from a checkout.

## [1.0.0] - 2026-10-10

### Added

- Rust (Axum) backend + Svelte 5 frontend NAS appliance: dashboard, users
  and groups, storage and RAID, shares (SMB/NFS/FTP/SFTP/WebDAV), file
  explorer with trash, restic backups, rclone cloud sync, network,
  antivirus and firewall, Docker with an app catalog, virtual machines
  (Incus), VPN (WireGuard/OpenVPN), Syncthing, aria2 downloader, local AI
  chat (Ollama) with a RAG knowledge base, widgets, health monitoring,
  system journal, transfer quotas, web terminal, and settings.
- SQLite migrations (`sqlx/`), installer (`installer/`), and reference
  documentation (`docs/`, `TESTING.md`, `ROADMAP.md`).

[Unreleased]: https://github.com/nicoleroux35/HomeVault/compare/v1.0.0...HEAD
[1.0.0]: https://github.com/nicoleroux35/HomeVault/releases/tag/v1.0.0

# Disk Quotas

**Purpose:** The per-user home-directory disk-space limit, applied to a real
filesystem via `setquota -u`.

> **Removed 2026-09-09 — per-group filesystem quotas.** The old
> `setquota -g` feature (admin CRUD under `/api/quotas`, `src/handlers/quotas.rs`,
> `src/storage/quota_repo.rs`, `tests/quotas_integration.rs`, the group-quota
> methods of `QuotaService`, and the Settings **"Disk Quota" card**) was
> deleted: disk quotas are managed per user here and per user/group (transfer
> metering) via [Transfer Quotas](./transfer-quotas.md). The `quotas` DB table
> from its migration is left in place (migrations are immutable) but unused.

This is distinct from **[Transfer Quotas](./transfer-quotas.md)**
(per-user/group upload/download byte caps enforced against File Explorer
transfers) — same word, different systems, different backing files.

## Backend

`src/quota.rs` — `QuotaService` is now just three associated helpers, no repo,
no state:

- `bytes_to_quota_kb(bytes) -> i64` — bytes → 1-KiB blocks, rounds down, clamps
  negatives to 0.
- `set_user_home_quota(system_username, filesystem, hard_bytes) -> Result<(), QuotaError>`
  — `setquota -u` with soft = hard, inode limits 0; `hard_bytes == 0` removes the
  quota. **Bare command** (no `sudo`; runs as root). Best-effort: returns `Err`
  when `setquota` is missing or the filesystem was not mounted with `usrquota`,
  so the caller records the limit and tells the admin it is not actively
  enforced.
- `format_size_kb(kb) -> String` — human-readable size, `"Unlimited"` for 0.

No dedicated endpoint — the limit rides on the Users home-folder calls
(`POST /users`, `PUT /users/{u}/full`, `PUT /users/{u}/home`); the value is
persisted in `user_homes.size_limit_bytes` (migration `032`). See
[Users](./users.md#home-directory-size-limit-2026-09-08) and
`src/handlers/user_homes.rs`.

## Frontend

The Users detail sheet's home-directory size slider (`Users.svelte`,
`saveHome()`). There is no Settings UI for disk quotas any more.

> **Naming collision — read this before opening "the Quotas app".** There is an
> app in the desktop registry with the id `quotas`
> (`frontend/src/routes/Quotas.svelte`). It is **not** this system: it is a
> read-only overview of *transfer* quotas, documented in
> [Transfer Quotas](./transfer-quotas.md#the-quotas-app--read-only-overview-2026-09-14).
> Disk quotas have no dedicated screen — they live on the Users home-folder card.

## Related

- [Users](./users.md), [Settings](./settings.md),
  [Transfer Quotas](./transfer-quotas.md)

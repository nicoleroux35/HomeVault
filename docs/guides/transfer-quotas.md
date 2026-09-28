# Transfer Quotas

**Purpose:** Admin-configurable upload/download byte caps per user or per
group (value + unit of MB/TB/PB, or disabled), enforced against real File
Explorer transfers. Settable from the Users and Groups pages.

This is **distinct from two other, unrelated systems** in this codebase:
the filesystem disk-space quota in [Disk Quotas](./quotas.md)
(`src/quota.rs`/`src/handlers/quotas.rs`, enforces how much a group may
*store*, via `setquota`/`repquota`) and any upload/download byte counters
shown elsewhere on dashboards. Same/similar naming, different systems.

## Scope (stated precisely so it can't be assumed away)

Enforcement only covers **File Explorer** uploads/downloads — the one
transfer channel this codebase controls end-to-end. SMB/NFS/FTP/WebDAV/SFTP
shares are served by separate system daemons with no per-request visibility
from this codebase, so a share transfer is **not** metered or capped by this
feature at all, regardless of what's configured. Do not describe this
feature as covering shares.

## Backend

- `src/transfer_quota.rs` — `applicable_upload_quotas`/
  `applicable_download_quotas` (a user's own quota **and** every group they
  belong to, each resolved independently), `check_quota` (rejects if any
  applicable quota would be exceeded), `record_usage` (debits after a transfer
  lands), `refund_usage` (undoes a reservation on an abandoned download), and
  `acquire_quota_locks` (per-subject in-process locks that serialise a
  transfer's check-then-debit against every concurrent transfer of the same
  subject — without them two parallel transfers could both pass the "would
  exceed?" check on stale usage and then both debit past the cap).
- `src/storage/transfer_quota_repo.rs` — `TransferQuotaRepository`: `get`,
  `list`, `set_limits`, `delete`, `add_upload_usage`, `add_download_usage`.
  One row per `(subject_type, subject_name)` pair in the `transfer_quotas`
  table (migration `sqlx/migrations/022_transfer_quotas.sql`), created on
  first save. Has no `rename()` method — one was added preemptively during
  the original build but neither Users nor Groups has a rename feature, so
  it was removed as dead code; don't re-add it without a real caller.
- `src/handlers/transfer_quotas.rs` — all handlers. `require_auth` for
  reads, `require_admin` for writes/deletes — a regular user can read their
  own quota but not grant themselves unlimited allowance.
- `src/handlers/explorer.rs` — the quota check in `handle_explorer_download`
  (a **reservation** of the bytes about to be streamed, made under the
  subject lock; the stream wrapper refunds it if the client disconnects
  before the body completes, so an aborted download is never left charged),
  the per-chunk check inside `handle_explorer_upload`'s write loop (not just
  once at the start), and the resumable-upload endpoints (`/upload/session`,
  `/upload/chunk`, `/upload/complete`) which re-check the declared size at
  session start, per chunk, and again under the lock at completion just
  before the debit.
- `src/handlers/users.rs`/`src/handlers/groups.rs` — `handle_user_delete`/
  `handle_group_delete` each delete the subject's `transfer_quotas` row in
  their success branch, after the underlying user/group deletion actually
  succeeds (not before — an OS-level deletion failure must not silently
  orphan-clean a quota for a subject that was never actually deleted).
  Without this, a deleted-then-recreated same-named account would silently
  inherit the old account's quota configuration and usage counter.

| Method | Path | Auth | Notes |
|--------|------|------|-------|
| GET | `/transfer-quotas/{subject_type}` | auth | list every configured quota for every user or every group (no current frontend caller — a built, working foundation for a possible future "quota overview" table) |
| GET | `/transfer-quotas/{subject_type}/{subject_name}` | auth | one subject's quota |
| PUT | `/transfer-quotas/{subject_type}/{subject_name}` | admin | set limits |
| DELETE | `/transfer-quotas/{subject_type}/{subject_name}` | admin | remove |

`subject_type` ∈ {`user`, `group`}.

## Frontend

- `frontend/src/routes/Users.svelte` — `quotaForm`/`quotaUsage`,
  `startQuota`/`handleQuota`, the quota sub-view markup (menu item
  "Transfer quotas").
- `frontend/src/routes/Groups.svelte` — the same pattern, for groups.
- `frontend/src/routes/Explorer.svelte` — `handleDownload` calls
  `api.downloadFile()`, which first mints a **download ticket**
  (`POST /explorer/download-ticket`) — the server checks the caller's quota
  there, so an over-quota download surfaces as a toast rather than a
  silently-failed download — then hands the ticket URL to the browser's
  download manager.
- `frontend/src/lib/services/api.ts`: `getTransferQuota`, `setTransferQuota`,
  `issueDownloadTicket`, `downloadFile`.

### The Quotas app — read-only overview (2026-09-14)

`frontend/src/routes/Quotas.svelte` (app id `quotas`, registered in
`Desktop.svelte`'s app registry with `--hv-app-quotas-grad` / `--hv-app-quotas-c`
and a tile glyph in `frontend/src/lib/assets/launchpadIcons.ts`). It is the
dedicated destination of the Users rail's **Quota view** button, and it fronts
*this* system — transfer quotas — not the filesystem quotas of
[Disk Quotas](./quotas.md).

- **Read-only by design.** No control on the screen writes. Editing stays in
  the Users app's "Transfer quotas" card (and Groups, for group quotas); the
  **Manage in Users** button hands off with
  `window.dispatchEvent(new CustomEvent('hv-open-app', { detail: { id: 'users' } }))`
  — the same event Explorer uses to reach Shares, resolved by
  `Desktop.svelte`'s `handleOpenAppRequest`. `openApp()` therefore stays the
  single owner of the app icon/colour/dedup logic.
- **No new endpoint.** It calls `api.listTransferQuotas('user')` and
  `('group')` (`GET /transfer-quotas/{subject_type}`, `require_auth`) in
  parallel, exactly as the Users overview modal does. The screen is
  nonetheless admin-gated like Users — least privilege is the default.
- Rows: subject type, subject name, last-updated, and one column per direction
  (upload, download). A direction with `*_enabled === false` reads
  **Unlimited**; otherwise it shows `used of limit` plus a real progress bar
  fed by `*_used_bytes` — the figure the Users overview modal ignores.
- Filters: all / users / groups. A counter line reports the row count and how
  many subjects actually carry a limit.
- **Deliberate duplication:** `formatBytes()` is a second copy of
  `Users.svelte`'s (both use `Intl.NumberFormat` with `style: 'unit'` so units
  and the decimal separator follow the UI language). Lifting it into a shared
  `$lib/utils` helper is the obvious follow-up; it was left duplicated here
  because the two call sites format different value sets and the Users copy is
  inside a file this change was asked not to restructure further.
- **The app id must stay in `OWN_HEADER_APPS`** (`WindowFrame.svelte`). The
  screen renders its own `<PageLayout>`/`<WindowHeader>`, and that set is what
  tells `WindowFrame` not to *also* stack its fallback header on top. `quotas`
  was missing from it on 2026-09-14 and the window shipped **two** bars — the
  real one plus a blank duplicate (the fallback's `win.icon` sprite name has no
  glyph). Fixed 2026-09-14 by adding `'quotas'`; E2E `desktop.spec.ts` `14.9`
  now walks a sample of the registry asserting one `.hv-winhdr` per window, so
  the next new app can't repeat it. **Add every new app id to that set.**

## Invariants

- A user's own quota and every group they belong to are each checked and
  debited **independently** — both apply simultaneously. Getting this
  interaction subtly wrong (e.g. only checking one, or double-debiting) is
  an easy regression.
- Group-membership resolution for the group side of this must stay
  consistent with `GroupService::get_user_groups` (display name vs. full
  system name) — the same class of mistake already caused a real
  regression on the Groups page once (see `CLAUDE.md`).
- `record_usage` runs only after a transfer lands; a rejected transfer is
  never debited. Downloads *reserve* the bytes at stream start under the
  per-subject lock and `refund_usage` if the client disconnects early, so a
  completed transfer records exactly its size and an abandoned one nets out.
- Check-then-debit for a subject is serialised by `acquire_quota_locks`
  (held for the duration of the transfer). Don't revert to a bare
  read-then-write, or concurrent transfers can overshoot the cap.

## Related

- [Disk Quotas](./quotas.md), [Users](./users.md), [Groups](./groups.md),
  [File Explorer](./explorer.md), [API Contract](../API.md)

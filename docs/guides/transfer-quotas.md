# Transfer Quotas

**Purpose:** Admin-configurable upload/download byte caps per user or per
group (value + unit of MB/TB/PB, or disabled), counted over a lifetime or a
calendar **day / week / month** window, enforced against real File Explorer
transfers. Settable from the Users and Groups pages.

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

## Counting periods (ROADMAP 23.2, 2026-10-23)

Before 2026-10-23 the two counters were plain lifetime accumulators, so
"100 GB/month" really meant "100 GB ever": once the cap was reached the
subject stayed blocked until an admin edited the row by hand. A quota now
carries a **period**:

| `period` | Window |
|----------|--------|
| `lifetime` (default) | never resets — the historical behaviour, and what every pre-existing row got |
| `day` | midnight → midnight |
| `week` | Monday 00:00 → next Monday 00:00 (ISO week) |
| `month` | the 1st at 00:00 → the 1st of the next month |

Windows are **calendar** windows in the appliance timezone (Settings →
General, `transfer_quota::appliance_timezone()`), not fixed durations: a
monthly quota resets on the 1st, not 30 days after the first upload. The
stored `period_started_at` is the instant the current window opened;
`period_resets_at` is computed for the API/UI and never stored.

Two paths close a window, and they agree by construction because both go
through `window_containing`:

- **Enforcement settles it** (`settle_window`, called from `load_applicable`):
  a row whose window elapsed is zeroed in the database and re-opened at the
  current calendar boundary the moment any transfer consults it. The common
  case writes nothing — the staleness check is pure.
- **A background loop** (`run_reset_loop`, spawned in `src/web/mod.rs`, every
  60 s with `MissedTickBehavior::Skip`) keeps the Quotas/Users screens honest
  when nobody transfers.

Enforcement therefore **never depends on the loop having run**: a stopped
scheduler delays the *display* of a reset, never the reset itself.

Changing a saved row's `period` zeroes both counters (`set_limits` does it in
SQL), because billing lifetime history against a newly chosen monthly cap
would immediately block the subject. Editing a *limit* on the same period
keeps the counters — raising a cap must not wipe what was already
transferred.

## Fail closed (ROADMAP 23.1, 2026-10-23)

Every File Explorer quota check goes through one helper,
`load_quotas_or_refuse` in `src/handlers/explorer.rs`, for all six call sites
(download, download-ticket, the legacy multipart `/upload`, and the resumable
session start / chunk / complete trio).
When the guard itself cannot answer — a database error, an unresolvable
membership lookup — the transfer is **refused** with HTTP `503` and
`transfer_quota::unavailable_message(...)`.

The previous behaviour was the opposite: the error was swallowed into an empty
quota list, i.e. "the database could not tell me" was read as "this subject
has no quota", so a transient failure silently disabled every cap in the
product. A cap that cannot be verified is a refused transfer, not an
unlimited one. The `503` is deliberate rather than `403`: the caller did
nothing wrong and retrying a moment later succeeds.

## Backend

- `src/transfer_quota.rs` — `applicable_upload_quotas`/
  `applicable_download_quotas` (a user's own quota **and** every group they
  belong to, each resolved independently and each settled against its current
  window), `check_quota` (rejects if any applicable quota would be exceeded),
  `record_usage` (debits after a transfer lands), `refund_usage` (undoes a
  reservation on an abandoned download), `acquire_quota_locks` (per-subject
  in-process locks that serialise a transfer's check-then-debit against every
  concurrent transfer of the same subject — without them two parallel
  transfers could both pass the "would exceed?" check on stale usage and then
  both debit past the cap), plus the period machinery: `Period`,
  `window_containing`, `current_window_start`, `window_elapsed`, `resets_at`,
  `settle_window`, `reset_expired_periods`, `run_reset_loop`.
- `src/storage/transfer_quota_repo.rs` — `TransferQuotaRepository`: `get`,
  `list`, `set_limits`, `delete`, `reset_window`, `reset_expired`,
  `add_upload_usage`, `add_download_usage`. One row per
  `(subject_type, subject_name)` pair in the `transfer_quotas` table
  (migration `sqlx/migrations/022_transfer_quotas.sql`, extended by
  `055_transfer_quota_period.sql` with `period` + `period_started_at`),
  created on first save. `set_limits` takes one `QuotaLimits` value rather
  than seven positional arguments (`QuotaLimits::lifetime(...)` is the
  convenience constructor for callers that don't care about periods). Has no
  `rename()` method — one was added preemptively during the original build but
  neither Users nor Groups has a rename feature, so it was removed as dead
  code; don't re-add it without a real caller.
- `src/handlers/transfer_quotas.rs` — all handlers. Reads are **self-or-admin**
  (see the API table), writes/deletes are `require_admin`. The `period` field
  is validated by `Period::parse`; an unknown value is a named error listing
  the accepted ones, never a silent fallback to `lifetime`. Like the rest of
  the module it answers HTTP 200 with `{"success":false,"error":…}` — the
  *authorisation* failures are the real status codes (`403`), because those
  come from `require_admin`/`require_auth` before the handler body runs.
- `src/handlers/explorer.rs` — `load_quotas_or_refuse` (the fail-closed gate),
  the quota check in `handle_explorer_download`
  (a **reservation** of the bytes about to be streamed, made under the
  subject lock; the stream wrapper refunds it if the client disconnects
  before the body completes, so an aborted download is never left charged),
  the same reservation in the download-ticket handler (the ticket is issued
  only if the size would fit), the per-chunk check inside
  `handle_explorer_upload`'s write loop (not just once at the start), and the
  resumable-upload endpoints (`/upload/session`, `/upload/chunk`,
  `/upload/complete`) which re-check the declared size at session start, per
  chunk, and again under the lock at completion just before the debit.
- `src/handlers/users.rs`/`src/handlers/groups.rs` — `handle_user_delete`/
  `handle_group_delete` each delete the subject's `transfer_quotas` row in
  their success branch, after the underlying user/group deletion actually
  succeeds (not before — an OS-level deletion failure must not silently
  orphan-clean a quota for a subject that was never actually deleted).
  Without this, a deleted-then-recreated same-named account would silently
  inherit the old account's quota configuration and usage counter.

| Method | Path | Auth | Notes |
|--------|------|------|-------|
| GET | `/transfer-quotas/{subject_type}` | **admin** | list every configured quota for every user or every group (consumed by the read-only **Quotas** app's two parallel calls and by the Users overview modal — see below). Admin-only since ROADMAP 23.3: the payload carries every subject's limits *and* consumed bytes, and the gate used to live only in the screen |
| GET | `/transfer-quotas/{subject_type}/{subject_name}` | **self or admin** | one subject's quota. "Self" means `subject_type = user` and the caller *is* `subject_name`; anything else — another user, any group — needs an admin |
| PUT | `/transfer-quotas/{subject_type}/{subject_name}` | admin | set limits **and** `period` (`lifetime`\|`day`\|`week`\|`month`, defaults to `lifetime` when the field is absent) |
| DELETE | `/transfer-quotas/{subject_type}/{subject_name}` | admin | remove |

`subject_type` ∈ {`user`, `group`}. A row serialises `period`,
`period_started_at` and the computed `period_resets_at` (absent for a
lifetime quota).

## Frontend

- `frontend/src/routes/Users.svelte` — `quotaForm`/`quotaUsage`,
  `startQuota`/`handleQuota`, the quota sub-view markup (menu item
  "Transfer quotas"), plus the **Counting period** select
  (`data-testid="users-quota-period"`) and the "Resets on …" line
  (`users-quota-resets`).
- `frontend/src/routes/Groups.svelte` — the same pattern, for groups
  (`groups-quota-period` / `groups-quota-resets`).
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
  `('group')` (`GET /transfer-quotas/{subject_type}`, **admin-only** since
  ROADMAP 23.3) in parallel, exactly as the Users overview modal does. The
  screen still redirects non-admins — but that redirect is a courtesy now, not
  the boundary: the endpoint enforces it.
- Rows: subject type, subject name, last-updated, the counting period
  (`data-testid="quotas-period"`, with its "resets on …" date when the quota
  is not a lifetime one), and one column per direction
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
- **Verified live + E2E-guarded (2026-10-07).** `tests/e2e/quotas/quotas.spec.ts`
  runs green end-to-end (38/38, Firefox, scratch `PORT=` + isolated
  `DATA_DIRECTORY`) and the screen was audited live on the same server: launcher
  open, icon+title-only header, the three mutually-exclusive filter chips, the
  seeded `used/limit` progress bar (50 Mo / 100 Mo → `aria-valuenow="50"`) plus
  *Unlimited* on a disabled direction, and the **Manage in Users** hand-off
  (`hv-open-app`) reaching the Users window without leaving `/desktop` or
  closing Quotas. The suite never edits through the UI — it seeds and restores
  its own rows through the admin API in `beforeAll`/`afterAll`. No dead
  selector needed fixing: the spec targets `data-testid` and was
  written after the 2026-09-12 Appliance migration.
  **+2026-10-23 (ROADMAP 23):** 38 → **40** `test(`. §15 renders the counting
  period and its reset date per row, re-reads the period/instants from the API,
  checks a lifetime row carries no reset, and asserts the read scoping at the
  API level (a member is refused the list and another user's quota, and can
  still read their own). The seed row became a **monthly** quota so §15 has a
  window to show; `afterAll` restores the period too. Written, not run.

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
  Usage counters are floored at 0 in SQL (`MAX(0, …)`) so a refund landing
  after its window was reset cannot make the new window negative.
- Check-then-debit for a subject is serialised by `acquire_quota_locks`
  (held for the duration of the transfer). Don't revert to a bare
  read-then-write, or concurrent transfers can overshoot the cap. The lock
  map prunes entries no transfer holds (`Arc::strong_count > 1`), so one
  mutex per subject does not accumulate for the life of the process.
- A quota load failure is a **refused transfer** (`503`), never an empty
  quota list. Do not reintroduce `unwrap_or_default()`/`Vec::new()` on a
  `applicable_*_quotas` error: that reads "the guard could not answer" as
  "no cap configured" and silently disables every quota in the product. The
  unit test `test_no_quota_load_bypasses_the_fail_closed_gate` guards this by
  asserting the only two calls to `applicable_*_quotas` in `explorer.rs` are
  inside `load_quotas_or_refuse`.
- `bytes as i64` must never be used on a transfer size. Sizes go through
  `bytes_to_i64` (saturating), and `check_quota` compares in `u128`: a plain
  cast turned any value above `i64::MAX` negative and silently passed the
  check.
- **A quota check must not fork once per chunk.** Group membership goes
  through `resolve_groups` (5 s TTL); bypassing the cache re-introduces the
  ~128-fork upload (ROADMAP 33.1).
- Reads are scoped in the **handler**, not in the screen. `GET
  /transfer-quotas/{subject_type}` is admin-only and the single-subject GET is
  self-or-admin; Quotas.svelte's non-admin redirect is a courtesy, not the
  boundary.

## Group-membership cache (ROADMAP 33.1, 2026-10-24)

`load_applicable` resolves the caller's groups to know which **group** quotas
apply. That resolution is `GroupService::get_user_groups`, i.e. one `id -nG`
fork — and the resumable-upload path re-checks the quota **per chunk**, so a
1 GB upload in 8 MB chunks forked ~128 times for one file. The call was always
inside `spawn_blocking` (it never blocked the runtime), so this was throughput
debt, not a stall.

`resolve_groups` now serves it from a process-wide cache keyed on the username:

| | |
|---|---|
| TTL | `GROUP_MEMBERSHIP_TTL` = **5 s** (`src/transfer_quota.rs`) |
| Miss | forks `id -nG` inside `spawn_blocking`, stores the result, prunes expired entries |
| Hit | pure in-memory lookup, no fork, no lock held across an await |
| Bound | one entry per account that checked recently; expired entries are pruned on every miss and by `prune_group_cache` |

**The bounded staleness, stated plainly:** a group added to (or removed from)
an account applies to quota enforcement within **one TTL** — up to five
seconds. That is far below the time a user takes to notice, and the trade is
deliberate: the alternative is either a fork per chunk or an unbounded cache
that makes a revoked group quota keep applying. The Groups screen calls
`invalidate_group_cache(username)` after a membership change, so that path is
**immediate** rather than "within five seconds"; `clear_group_cache` exists for
tests and a settings import.

The TTL is pinned by unit tests (`the_ttl_is_a_short_bound_that_is_actually_documented`
also asserts this section mentions the constant), and the cache's per-account
keying is tested explicitly: one user's cached groups can never answer for
another.

## Related

- [Disk Quotas](./quotas.md), [Users](./users.md), [Groups](./groups.md),
  [File Explorer](./explorer.md), [API Contract](../API.md)

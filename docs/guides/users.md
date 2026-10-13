# Users Management

**Purpose:** Admin CRUD for user accounts, groups, home folders, passwords, roles,
lock state, and admin-side 2FA disable.

## Backend

Handlers in `src/handlers/users.rs`; models in `src/handlers/users_models.rs`.

| Endpoint | Auth | Notes |
|----------|------|-------|
| `GET /users` | admin | list |
| `POST /users` | admin | create (random password if empty) |
| `GET /users/{username}` | admin | detail |
| `PUT /users/{username}` | admin | update |
| `PUT /users/{username}/full` | admin | full profile (first/last name) |
| `DELETE /users/{username}` | admin | delete (kills PTY sessions via `state.pty_manager.kill_session`; also revokes the account's direct share permissions, see Invariants) |
| `PUT /users/{username}/password` | admin | reset password |
| `PUT /users/{username}/role` | admin | change role |
| `PUT /users/{username}/locked` | admin | lock/unlock |
| `POST /users/{username}/2fa/disable` | admin | admin disable 2FA |
| `PUT /users/{username}/groups` | admin | set group membership |
| `GET/PUT/DELETE /users/{username}/home` | admin | home folder (enabled + disk + size limit) |
| `GET /users/{username}/avatar.svg` | auth | SVG avatar |
| `GET /users/count`, `/users/{username}/exists` | admin | |

Home disk picker data: `GET /storage/available-home-disks` (`handle_available_home_disks`),
returns each mount's `available_bytes` / `total_bytes`. Only **writable**
filesystems are offered (`user_homes.rs::is_writable_home_filesystem` filters
out `squashfs`/`iso9660`/`erofs`): read-only snap squashfs mounts report
`available_bytes: 0`, which used to collapse the size slider to `min == max == 0`.

### Home directory location — disk or explicit folder (2026-09-08)

Two ways to place a user's home folder, in the create form and the detail Home
card:

- **A disk** (`home_disk` = a mount point): the path is derived as
  `<mount>/users/<username>` (or `/home/homevault/<username>` for `/home`).
- **An explicit folder** (`home_path` = an absolute directory), chosen via the
  shared `FolderPicker`. Overrides `home_disk`; the server derives the real
  containing partition (`user_homes.rs::containing_partition`, longest
  mount-point prefix) for the quota + free-space check. Picking a disk button
  clears `home_path` and vice-versa (`pickHomeDisk` / `pickHomeFolder`).

`home_path` is untrusted admin input — `user_homes.rs::validate_custom_home_path`
is the gate (unit-tested): rejects relative paths, `.`/`..` components, control
characters, `/`, every `FORBIDDEN_HOME_ROOTS` entry
(`/bin /boot /dev /etc /lib* /proc /root /run /sbin /sys /usr /var`), and any
path inside the HomeVault data directory; the parent must already exist and is
canonicalized so a symlink can't smuggle the target elsewhere.

Wire: `home_path: Option<String>` on the create body, `PUT .../full`, and
`PUT .../home`; `GET .../home` already returns `path`. `saveProfile()` forwards
the current `home_path` + `home_size_bytes` untouched (the folder picker + slider
save via `saveHome()`).

### Home-directory size limit (2026-09-08)

When a home folder is enabled the admin sets its size on a slider (create form
and the detail Home card). Range `1 GiB … disk free space`, default
`min(10 GiB, free)`, plus an **"Unlimited"** checkbox. The readout shows
`allocated / remaining-on-disk` and both figures update live as the slider moves.
Offered for **every role** — the server applies the quota regardless (was role
`user` only until 2026-09-08).

- Wire: `home_size_bytes: Option<i64>` on the create body and `PUT .../full`;
  `size_bytes` on `PUT .../home` and the `GET .../home` response. `0`/`null` =
  no limit. Negative → 400. A size larger than the disk's free space → 400.
- Enforcement: `handlers/user_homes.rs::apply_home_folder` calls
  `quota::QuotaService::set_user_home_quota` → `setquota -u <user> <kb> <kb> 0 0
  <mount>`. **Best-effort** — if the filesystem is not mounted with `usrquota`
  the limit is still recorded and the handler returns a success message saying it
  could not be enforced. Stored in `user_homes.size_limit_bytes` (migration 032).
- `api.ts`: `setUserHome(username, enabled, disk, sizeBytes?, path?)`,
  `updateUserFull({ …, home_path, home_size_bytes })`.
- See [Disk quotas](./quotas.md).

### Per-member resource access (apps / folders / shares) — 2026-10-16

Roles stay coarse (`user` / `admin`), but the owner can now grant a member
access to **specific** apps, folders and SMB/NFS/FTP/WebDAV shares without
making them an admin. Grants live in `resource_shares`
(`sqlx/migrations/041_resource_shares.sql`) and are managed through
`src/handlers/resource_shares.rs`:

| Endpoint | Auth | Notes |
|----------|------|-------|
| `GET /shares/access` | admin | every grant (optional `?member=`) |
| `GET /shares/access/mine` | auth | the caller's own effective access |
| `POST /shares/access` | admin | grant/update `{member, kind, resource, permission?}` |
| `DELETE /shares/access` | admin | revoke `?member=&kind=&resource=` |

`kind` ∈ `app` \| `folder` \| `share`; `permission` ∈ `read` \| `write` \|
`full` (default `read`; `full ⊇ write ⊇ read`). Repository:
`src/storage/resource_share_repo.rs`.

**The filter is server-side.** An admin (the appliance "owner") sees
everything implicitly and needs no rows; a member sees exactly the
`(kind, resource)` pairs they hold. `handle_my_access` returns `all: true` +
empty lists for an admin and the grouped grant rows for a member. Other
modules must call `resource_shares::visible_app_ids` /
`can_access` before listing or acting on a resource — filtering in the UI is
not a boundary. `event_visible_to` is the WebSocket whitelist: an event tagged
with `resource_kind` / `resource` is only delivered to a member who can access
that resource; untagged events go to everyone.

The editor is `frontend/src/lib/components/users/MemberAccessCard.svelte`,
embedded in the Users detail view (admins see a read-only note instead).

## Frontend

- `frontend/src/routes/Users.svelte`.

### Master/detail layout (2026-09-14)

Rework onto the canonical master/detail shell, following the Figma design
`4pwNRZU5wQjSWQBODFViKz` node `3:99`. Markup-only: every handler (`handleCreate`,
`saveProfile`, `saveRoleAndGroups`, `saveHome`, `changePassword`, `saveQuota`,
`toggleLock`, `handleDisable2FA`, `handleDelete`, `openQuotaOverview`,
`loadQuota`, `handleDeleteHome`), every `bind:` and every i18n key is unchanged.
This **supersedes the single-view layout below** — the grid + stacked detail
sheet is gone.

- **Shell:** `PageLayout` with `sidebar` / `sidebarLabel` / `sidebarTestId`
  (`users-list`) / `sidebarFooter`, the same shape as `Groups.svelte`.
  `$: detailOpen = creating || !!selectedUser;` and
  `$: sidebarHidden = narrow && detailOpen;` — on a narrow window the rail and
  the pane **swap**, and `sidebarHidden` *unmounts* the rail (so
  `users-grid` reaches `toHaveCount(0)` for E2E 7.4) rather than hiding it.
- **The rail** (master list): the "New user" button, the search box, four
  filters (all / active / locked / admin), a count line, and one row per
  account — 36px avatar, name, role badge, `@username`, and a locked/active
  state badge. Its footer holds the **Quota view** button, which opens the
  Quotas app (see [quotas.md](./quotas.md)) through the shared `hv-open-app`
  window event — the same event Explorer uses to reach Shares, handled by
  `Desktop.svelte`'s `handleOpenAppRequest`, so `openApp()` stays the only
  owner of the app icon/colour/dedup logic.
- **The detail pane** is six `SectionCard divider` cards, each sized by its own
  content — no fixed height anywhere (the mockup's `152.69 / 216.23 / 173.09 /
  887.26px` are deliberately **not** copied; HomeVault carries more content
  than the mockup, notably transfer quotas and security):
  1. **Identity** — username read-only (mono; it is the account's key and is
     not editable here), full name, email. `<form>` → `saveProfile()`.
  2. **Role & Groups** — the role segmented control (`.users-seg-btn`,
     `users-seg-btn` — *not* the create form's `users-role` cards) plus the
     group chips as full-width rows. `<form>` → `saveRoleAndGroups()`.
  3. **Home folder** — enable toggle, disk grid or explicit folder picker, size
     slider, and an allocated-vs-free progress bar. `<form>` → `saveHome()`.
  4. **Transfer quotas** — upload then download blocks (each a toggle + number +
     unit + usage bar), and the "All quotas" button that opens the (kept)
     overview modal.
  5. **Security** — 2FA state + reset, and the `#d-password` field with its
     change-password submit (self-change logs out, see Invariants).
  6. **Actions** — lock/unlock and the destructive delete (with the
     erase-or-keep-folder toggle).
- **Narrow threshold.** `narrow` is measured by a `ResizeObserver` on
  `users-split`, a child of `<main>` — so both the rail (256px + 1px border)
  and `<main>`'s `p-6` (48px) are already subtracted. `content < 760px` flips
  at roughly **1065px of window width**, not 1016px. The element carrying
  `data-testid="users-split"` is **always mounted** inside
  `{#if !narrow || detailOpen}` so the one-shot `ro.observe(splitEl)` always
  has a target. No `sidebarWidth` is passed — forcing `'100%'` collapses the
  measured column to 0 and would lock `narrow` true forever.
- **`canBack` bug fixed (pre-existing, was live).** The old template used
  `disabled={!canBack}` / `{#if canBack}` on a variable declared nowhere, so
  the shipped bundle threw `ReferenceError` in strict mode on opening the
  create pane — "New user" was broken. The notion is **removed**, not declared:
  the detail top bar is always rendered when `detailOpen`, so it commanded
  nothing. `$: canBack = narrow` would have been wrong — the E2E suite calls
  `backToGrid()` at desktop width.
- i18n: two keys added — `users.role_groups_title` and `users.list_count`
  (`%count%`) — plus `users.quota_view` for the rail footer. All four locales
  stay at parity.

#### Deviations from the mockup (deliberate)

| Mockup | HomeVault | Why |
|---|---|---|
| 208px rail | `--hv-window-sidebar-w` (256px) | No 208px stop exists; adding one would be per-app vocabulary |
| Card radius 16px | 12px (`SectionCard`) | The design system reserves 16px for windows |
| `#155DFC` / `#3B82F6` buttons | `--hv-primary` (#1E62AE) | One brand accent |
| No back bar | Back bar always rendered when a pane is open | The E2E suite navigates back at desktop width |
| Editable *Username* | Read-only | Not editable in this flow; avoids a second labelled input |
| "Change password…" under Identity | Moved to **Security** | Keeps every credential control in one card |
| Fixed card heights | Content-driven | The explicit ask — HomeVault has more to show |

#### Known debts and a signed-off test derogation

- **`users-tile` naming debt.** The rail's *rows* keep the `data-testid`
  `users-tile` (37 uses in the E2E spec) although they are no longer grid
  tiles. Renaming means rewriting the spec; deferred on purpose.
- **Dropped `users-new` test id.** The header action snippet is gone, so the
  rail button carries only `users-tile-new`. `getByRole('button', {name: 'New
  user'}).first()` still resolves — it is now the *only* such button.
- **`ghost` class reconciled markup-side.** E2E 4.5 selects
  `[data-testid="users-inp-btn"].ghost`, a selector no element matched. The
  eye-toggle buttons now carry the bare class `ghost`. It is a class *name*,
  not a design value, so it does not breach the token rule.
- **Test derogation (conscious gap) — partially lifted 2026-09-14.** The change
  was first delivered with **no compilation and no Playwright run**, a
  deliberate dated exception to CLAUDE.md's "Every feature ships with its
  tests", recorded rather than skipped silently. The compile half has since
  been run at the user's request:
  - `npm run check` → **7 errors, none in `Users.svelte` or `Quotas.svelte`**.
    The 7 are pre-existing and live in `Restic.svelte` (2), `Syncthing.svelte`
    (2) and `Downloader.svelte` (3). *(An earlier note in this file said 9 —
    that figure was never measured, it was carried over from the plan. The
    real number is 7.)*
  - `npm run build` → **OK** (32 s); the built `static/dist/assets/` chunks
    are emitted, so the app registry entry resolves.
  - `cargo build` → **OK** (9 m 55 s, no warnings).
  - **Two real defects were found by the check and fixed**: `Quotas.svelte`
    never imported `t` from `$lib/stores/i18n` (22 cascading errors), and its
    header passed a `{subtitle}` variable that was never declared. Both are
    fixed; the file is now type-clean.
  - **The E2E half is still unrun.** `npx playwright test
    tests/e2e/users/users.spec.ts --project=firefox` and
    `npx playwright test tests/e2e/desktop/desktop.spec.ts --project=firefox`
    (the latter covers the `--hv-app-quotas-*` tokens) have not been executed,
    and the spec was not modified. Visual pass still owed at 1440 / 1065 /
    1024 / 900 / 400px in all four languages, plus locked and unlocked for the
    `users-state.locked` badge.
- **Pre-existing spec defects — reported, not fixed** (the spec was out of
  scope, and three of the four are the spec's bug, not the markup's):
  `:498`, `:502`, `:1079` select `'[data-testid="users-setting-row"]CH input'`,
  a leftover of the old `.hv-users-search input` (correct form:
  `[data-testid="users-search"] input`); `:398` expects
  `label[for="c-username"] .req`, but the required marker is now a `<span>`;
  `:511` `expect(chip).not.toHaveClass(/on/)` is **structurally wrong** —
  Playwright's non-partial `toHaveClass` tests the regex against the whole
  `classList` string, and `font-medium` / `transition-colors` both contain
  "on". The correct form is `/\bon\b/`, already used by 4.11. Do **not** warp
  the markup to satisfy it.

### Single-view layout (2026-09-08) — SUPERSEDED

> Replaced by the master/detail layout above. Kept for the history of the
> `HomevaultClaudeDesign` mapping; the class names it names no longer exist.

Markup/CSS-only rework to the `HomevaultClaudeDesign/templates/homevault-users`
template (`<script>` untouched — every handler, `bind:`, `$:` and
invariant unchanged; the dark navy template is re-mapped onto the `--hv-shell-*`
paper-white vocabulary and the single `--hv-primary` accent).

- **One view at a time**, driven by `creating` / `selectedUser`:
  1. **Grid** — a search + filter row above a `repeat(auto-fill, minmax(210px,
     1fr))` grid of avatar tiles (`.hv-users-tile`: round colour avatar, name,
     `@username`, role badge + active/locked state) plus a trailing dashed
     "new user" tile (`.hv-users-tile-new`).
  2. **Detail** — a back-arrow top bar (avatar + name + home path + lock button)
     over a `.hv-users-settings` rounded list; each `.hv-users-setting-row` is
     icon + title/hint + inline control (identity, role — its own row with the
     active role's description, password, 2FA, groups, home folder, transfer
     quotas), then a separate red `.danger-block` row for delete. The identity
     and role rows are two `<form>`s both calling `saveProfile()` (`editForm` is
     shared, so a save from either persists name + email + role).
  3. **Create** — the same back-arrow top bar over a `.hv-users-settings` list
     (identity fields, role cards, group chips, home-folder row) + a full-width
     "Create account" / "Cancel" action row.
- The previous persistent two-pane (list + detail) layout is gone: a detail
  action no longer keeps the grid mounted beside it — `closeDetail()` (the
  top-bar back arrow) returns to the grid. `.hv-users-list-pane` /
  `.hv-users-detail-pane` / `.hv-users-row` / `.hv-users-noselect` no longer
  exist; the E2E suite navigates back to the grid explicitly (`backToGrid`).
- Responsive: `@container hv-users` (queries the window, not the viewport) —
  `≤760px` collapses the grid columns, stacks the search/filters, and drops the
  inline setting-row controls below their text; `≤460px` hides the lock-button
  label and makes the create actions full-width. The a11y contract from the
  2026-09-08 pass below (landmarks, `<form>` wrappers, `<fieldset>`/`<legend>`,
  ARIA state, `role="switch"`/`progressbar`, contrast / forced-colors / coarse-
  pointer media queries) is preserved.
- `frontend/src/lib/services/api.ts`: `getUsers`, `createUser`, `updateUser`,
  `deleteUser`, `updateUserGroups`, `getUserHome`, `setUserHome`,
  `deleteUserHome`, `updateUserPassword`, `updateUserRole`, `setUserLocked`,
  `adminDisable2FA`, `getAvailableHomeDisks`.
- `src/groups.rs` `GroupService::is_admin_group` hides the real `homevault_admin`
  system group (which grants passwordless sudo) from the ordinary Groups UI.
- **Group checklist:** `loadGroups()` fetches `GET /groups`, splits
  `homevault_admin` into `adminGroup`, and puts the rest in the editable
  `groups` array. A genuine fetch failure sets `groupsError` so the UI shows
  "Could not load the group list" + a Retry button instead of the neutral
  "no groups" text; the empty state points the admin at the Groups app
  (`users.no_groups_hint`).
- **Admin group chip (2026-09-08):** `homevault_admin` is shown in the Users
  group list as a **read-only chip** (`.hv-users-chip.admin-locked`,
  `disabled`) — checked ⟺ the account's role is admin
  (`editForm.role`/`createForm.role`), never tickable, labelled "Managed by
  the Role setting". It is never added to the editable `groups` array and
  `toggleGroupSel` / `saveGroups` / `createForm.groups` never reference it, so
  membership stays driven solely by the Role selector
  (`database.rs::sync_admin_user_to_group` on promote,
  `GroupService::remove_user_from_group` on demote). This keeps the app role
  and the real sudo group from ever diverging. Normal groups remain open to
  any user (role `user` or `admin`).
- i18n: `[users]` table in `src/i18n/*.toml`. `users.username_example` /
  `fullname_example` / `email_example` back the input placeholders (localized —
  never hard-code "Jean Dupont"). `formatBytes()` in `Users.svelte` formats via
  `Intl.NumberFormat(lang, { style: 'unit' })` — locale units, not a fixed
  `o/Ko/Mo` array.

### Accessibility pass (2026-09-08)

> Layout specifics below (list/detail *panes*, pane-collapse breakpoints) were
> superseded the same day by the single-view template rework above; the ARIA /
> forms / theming contract it established still holds.

Markup/CSS-only rework (plus 3 one-line success-message additions in the
`saveProfile` / `saveGroups` / `saveHome` success branches and the `formatBytes`
rewrite — no other `<script>` logic touched, every handler / `bind:` /
invariant unchanged):

- Semantic structure: `<section>` landmarks for the list and detail panes,
  `<ul>`/`<li>` for the user list, one `<h1>` per rendered detail/create/empty
  state and `<h2>` card titles, `<form>` wrappers (Enter-to-submit) around
  create / profile / password / groups / quota, `<fieldset>`/`<legend>` +
  `role="group"` for role / groups / quota groupings.
- ARIA: search input `aria-label`; errors wired with `aria-invalid` +
  `aria-describedby` + `role="alert"`; success lines `role="status"`; filters,
  role/segment, chips, disks get `aria-pressed`; selected row `aria-current`;
  password eye buttons get a descriptive localized `aria-label` + `aria-pressed`
  (was `aria-label="toggle"`); range sliders `aria-labelledby` + `aria-valuetext`;
  quota bars `role="progressbar"`; decorative icons `aria-hidden`; list scroll
  `aria-busy`.
- Responsive: pane-collapse breakpoints are now `@container hv-users` (queries
  the window, not the viewport — a narrow Users window reflows), overview
  `<table>` gets an `overflow-x` wrapper, the lock-button label hides on a very
  narrow window.
- Forms: `autocomplete="new-password"` on both password fields, `autocomplete="off"`
  on name/email, `required` on create username/password.
- Theming: `@media (prefers-contrast: more)` lifts the low-alpha `--hv-shell-*`
  text/border tokens locally; `@media (forced-colors: active)` gives the custom
  toggle + selected states a border fallback; `@media (pointer: coarse)` expands
  the compact toggle / inline-button hit area to ≥44px via `::after` without
  resizing them.
- Loading states use `<Spinner>`; `transition: all` replaced with explicit
  property lists; logical CSS properties (`border-inline-end`, `text-align: start`,
  `padding-inline`); hard-coded `#5b73e6` / `#111` dropped.

Deferred (needs a dedicated visual-diff pass, higher regression risk): the
half-pixel font sizes (`13.5px` …) and off-scale radii (`13/15/20px`) are still
not mapped to `--hv-text-*` / `--hv-radius-*`, and the `rgba(46,114,201,α)`
primary tints are still inlined rather than tokenized. **Closed 2026-09-14** by
the master/detail rework: the new markup carries no half-pixel size, no
off-scale radius and no inline primary tint — every value is a `var(--hv-*)` or
a Tailwind utility resolving to one. The only literals left in the file are
data and pre-existing shape constants (`AVATAR_COLORS`' palette, the
`switchKnob` shadow, the `w-[46px]` icon tiles, `min-w-[150px]` on disk cards,
`flex-[0_0_90px]` on the unit selects) — none introduced by this change.

## Invariants

- The `homevault_admin` group must never be exposed as an ordinary group
  checkbox — a past bug compared a display name to a full system name and never
  matched, leaking sudo. Guard logic lives in `src/groups.rs`.
- Deleting a user cleans up live terminal sessions (`handle_user_delete`).
- **Deleting a user also revokes the share permissions it holds *directly***
  (`PermissionService::revoke_all_user_permissions`, called by
  `handle_user_delete` before the account row is dropped): the
  `share_permissions` rows are deleted and the real POSIX ACLs removed
  (best-effort, like every revoke — an unresolvable share must not block the
  database cleanup). Without this the rows outlive the account — the table
  keys a grantee by **name only**, with no foreign key to `users` — and
  recreating an account with the same username silently inherits the deleted
  user's access. Group grants are deliberately kept: the account was first
  removed from its groups (`remove_user_from_group`), and the group rows
  belong to the *remaining* members, whose access must survive. Regression
  guard: `tests/users_integration.rs` →
  `test_user_delete_revokes_its_direct_share_permissions_only`.
- If the edited user is the current admin, a self password change invalidates the
  session and forces logout/redirect (shared with [My Account](./account.md)).
- `home_size_bytes` and `home_path` are mutually exclusive on the wire: picking
  a disk clears the explicit path and vice versa. `saveProfile` must keep
  forwarding **both** through `updateUserFull`.
- The E2E suite (`tests/e2e/users/users.spec.ts`) is the contract for this
  screen's `data-testid` hooks and input ids (`c-*`, `d-*`). `Users.svelte` may
  be edited — but the hooks must survive,
  or the spec must be updated in the same change.

## Related

- [Groups](./groups.md)
- [My Account](./account.md)
- [Authentication](./auth.md)

# Users Management

**Purpose:** Admin CRUD for user accounts, groups, home folders, passwords, roles,
lock state, and admin-side 2FA disable.

## Backend

Handlers in `src/handlers/users.rs`; models in `src/handlers/users_models.rs`.

| Endpoint | Auth | Notes |
|----------|------|-------|
| `GET /users` | admin | list |
| `POST /users` | admin | create (random password if empty); sets `must_change_password` (the owner replaces it at first sign-in) |
| `GET /users/{username}` | admin/self | detail (includes `is_host_account`) |
| `PUT /users/{username}` | admin/self | update |
| `PUT /users/{username}/full` | admin | full profile (first/last name) |
| `DELETE /users/{username}` | admin | delete — removes the **Linux account** first (`userdel`, retryable on failure), then the row; kills PTY sessions via `state.pty_manager.kill_session`; revokes the account's direct share permissions (see Invariants) |
| `PUT /users/{username}/password` | admin/self | reset password; targeting someone else re-arms `must_change_password`, a self change clears it; also sets the Linux password (`chpasswd`, best-effort) |
| `GET /users/{username}/sessions` | admin | list a user's active sessions; tokens masked to 8 chars + `…` — an admin audits, never hijacks |
| `DELETE /users/{username}/sessions` | admin | bulk revoke: sign that user out everywhere (their current device included); the user just signs back in |
| `PUT /users/{username}/expiry` | admin | set (`{"expires_at": "<RFC 3339>"}`) or clear (`null`) the account horizon; a past date is refused |
| `PUT /users/{username}/role` | admin | change role — see *Host (Linux) account lifecycle* below |
| `PUT /users/{username}/locked` | admin | lock/unlock — a **permanent** lock (clears `locked_until`) mirrored to the Linux password (`usermod -L/-U`); the automatic lockout from failed logins expires by itself |
| `POST /users/{username}/2fa/disable` | admin | admin disable 2FA |
| `PUT /users/{username}/groups` | admin | set group membership (blank names rejected, unknown group → 400) |
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
| `GET /shares/access/groups` | admin | every group grant (optional `?group=`) |
| `POST /shares/access/groups` | admin | grant/update `{group, kind, resource, permission?}` |
| `DELETE /shares/access/groups` | admin | revoke `?group=&kind=&resource=` |

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

### Group-level grants (2026-10-20, migration 051)

Re-typing the same grant for every member of a household does not scale, and
an account added to a group later inherited nothing. `resource_group_shares`
(`sqlx/migrations/051_resource_group_shares.sql`) holds the same
`(kind, resource, permission)` triple keyed by **group display name** instead
of a member. `group_name` is a HomeVault group (the name the Groups app shows,
i.e. `hv_<name>` without the prefix) and deliberately has **no foreign key**:
groups live in `/etc/group`, `hv_groups` is only a metadata mirror.

Resolution is a **union, computed at read time** from the member's *live* OS
membership (`GroupService::get_user_groups`, off the async runtime via
`spawn_blocking`): `effective_visible_resources()` merges direct grants and
group grants with `ResourceShareRepository::merge_strongest` — strongest level
wins (`full ⊃ write ⊃ read`). Nothing is copied into `resource_shares`, so

- leaving a group revokes the inherited access on the very next request (no
  cache to invalidate, no migration to run),
- revoking a group row never demotes a member who also holds a direct row,
- a corrupt level is ignored rather than granting anything (fail closed).

`GET /shares/access/mine` gained `inherited` (the group grants that apply to
the caller) so the UI can explain *why* a resource is visible. The Users
detail pane's access card renders them under "Inherited from groups" with a
small editor (group select limited to the member's own groups, then kind,
resource, permission). Deleting a group purges its grants
(`revoke_all_for_group`) — a later group reusing the display name must not
inherit access nobody granted it.

The editor is `frontend/src/lib/components/users/MemberAccessCard.svelte`,
embedded in the Users detail view (admins see a read-only note instead).

## Account expiry (2026-10-20, migration 050)

`users.expires_at` (NULL = never, the default for every account) is an
admin-set UTC horizon. `PUT /users/{username}/expiry` is its only writer — the
profile update deliberately does not touch it, so a name/e-mail save can never
shift the horizon as a side effect, and a date already in the past is refused
(that mistake would lock the account out with no warning). Enforcement is
layered like the lockout and the password guard:

1. `AuthService::authenticate` re-checks the horizon on **every** attempt and
   returns `AuthError::AccountExpired` before the password is even verified —
   no credential feedback, no lockout-ladder footprint, and the PAM
   fall-through is covered too;
2. `AuthService::revoke_expired_account_sessions` deletes the sessions of
   expired accounts — called once at boot (`init`) and by a 10-minute loop
   started with the other schedulers in `web/mod.rs`. That loop is the
   deactivation path for a session an account already held; sign-in refusal is
   immediate and independent of it.

The Users detail pane renders it as its own "Account expiry" card
(`users-expiry-edit` → `users-expiry-input` datetime-local → save/remove),
showing the local time of the stored UTC instant and an "Expired" badge once
the horizon is behind us. The filter rail's "active" filter still means
`locked = false`; an expired account is reported by that badge, not by the
lock flag (expiry is not a lock — clearing the horizon restores the account
without touching `locked`/`locked_until`).

## Admin per-user sessions (2026-10-20)

Incident tooling next to the Security card's password form: `GET
/api/users/{username}/sessions` returns the target's live sessions — IP,
created/expires and a token **masked to its first 8 characters + `…`** — and
`DELETE` on the same path signs that user out of every device at once
(`handle_admin_user_sessions` / `handle_admin_revoke_user_sessions`). The
masking is the security decision of this feature: a full token list would let
an admin replay someone else's session silently, so revocation is bulk-only —
an incident needs "log them out everywhere", and the user simply signs back
in. The Users detail pane renders the list (`users-session-row`, masked
reference as `<code>`) with a "Sign out everywhere" button inside the Security
card; the list reloads only when the pane opens or switches to another user,
so a finished action's notice is not wiped by the post-save rail refresh.

## Forced password change (2026-10-20, migration 049)

A password an admin chose or generated — `POST /users` and `PUT /users/{username}/password`
**targeting someone else** — travels out of band and can stay valid forever. Both set
`users.must_change_password = 1` (migration `049_must_change_password.sql`,
`DEFAULT 0`). Any change where the current password was verified (self change, recovery reset) clears it;
setup never sets it. The login response and `/auth/me` expose the flag as
`must_change_password`, so the client routes to a forced-change screen on the login shell
(`Login.svelte` in `passwordChangeMode`, gate route `password-change` in `appGate.ts`) —
and `middleware::password_change_guard` is the real boundary: a flagged session may only
reach `GET /auth/me`, `GET /auth/setup-status`, `GET /api/settings/public`,
`POST /auth/logout` and `PUT /users/{own username}/password`; every other API call answers
403 with code `password_change_required`. Never shrink that allowlist — the change screen
(and a reload mid-flow) needs exactly those reads, and a flagged *admin* session must
still not be able to reset someone else's password while skipping its own obligation.

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
| Card radius 16px | 18px (`SectionCard`, the surface stop) | One surface corner for every container; the mock's 16px stop was retired |
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

## Host (Linux) account lifecycle (2026-10-21)

A HomeVault account may or may not have a matching Linux account. Both cases
are now owned by one module, `src/host_accounts.rs` (`HostAccountService`):

| Case | How the row looks | Who owns the credential |
|------|-------------------|-------------------------|
| HomeVault account | normal Argon2 `password_hash` | HomeVault; the Linux account is provisioned on demand (admin promotion, group membership, SFTP share) |
| Host (PAM) account | `password_hash == "!pam-provisioned"`, `is_host_account: true` | the host's `/etc/shadow`; HomeVault only mirrors it |

Before this pass nothing removed or revoked the host half, which produced four
real holes:

1. **Delete left a working login.** `DELETE /users/{u}` only dropped the row.
   A deleted admin kept a Linux login *and* — because admin status is derived
   from `homevault_admin` membership — a working HomeVault **admin** login that
   the next PAM sign-in silently recreated. The handler now runs `userdel`
   **before** the row delete, so a refusal (account still logged in, `userdel`
   lacks privileges) leaves both sides consistent and retryable.
2. **Demotion left real sudo.** `PUT /users/{u}/role` used to clear only
   `homevault_admin`; `sudo`/`wheel` stayed. `HostAccountService::revoke_admin`
   clears all three (`HOST_ADMIN_GROUPS`) **before** the `role_id` write, and
   `AuthService::update_role` signs the demoted account out — a session keeps
   the `is_admin` flag it was created with, and `get_session` now re-derives it
   from the current role on every call.
3. **Lock left the host password usable.** The DB flag stops a PAM sign-in but
   not SSH or the web terminal. Lock/unlock now mirrors to
   `usermod -L` / `-U` (best-effort, logged).
4. **Password reset did nothing on a host-backed account.** The row's hash is
   the PAM sentinel, so the old host password kept working. `update_password`
   now pushes the new secret through `chpasswd` on **stdin** (never argv, which
   `ps` would expose).

Safety rails, all in `HostAccountService`:

- **Username gate** before any command: `[A-Za-z_][A-Za-z0-9_-]{0,31}`, never a
  leading `-`, so nothing can be parsed as an option or a path fragment.
- **System accounts are refused**: `uid < 1000` or `uid >= 65534` (Debian/
  Ubuntu service accounts, `nobody`) can never be deleted or locked.
- **A PAM-mirrored row whose Linux account still exists cannot be deleted**
  (`400`, message names the `userdel` command). HomeVault did not create that
  account, and dropping only the mirror row would resurrect it at the next
  sign-in. If the Linux account is already gone, the stale row *can* be
  removed, so the page stays cleanable.
- **Time-bounded**: every command runs under a 20 s `HOST_OP_TIMEOUT` via
  `tokio::process` with `kill_on_drop`, off the async runtime.
- `userdel` is deliberately **not** given `-r`: home removal follows the
  explicit `delete_home_folder` choice and the folder HomeVault actually
  recorded (falling back to `/home/homevault/<user>`).

The frontend shows a **Linux account** badge on mirrored rows and disables the
Delete button with an explanation (`users.host_account_badge` /
`users.host_account_hint`, en/fr/de/es).

Live-host verification (needs root, `#[ignore]`d): `tests/login_pam_integration.rs`
→ `grant_admin_provisions_the_account_and_every_admin_group`,
`revoke_admin_clears_every_admin_group`, `set_password_rotates_the_host_credential`,
`set_locked_locks_and_unlocks_the_host_credential`,
`delete_account_removes_the_linux_account_but_keeps_its_home`,
`delete_account_refuses_a_system_account`. The decisions around those commands
(host flags, stale mirror cleanup, home-row cleanup, session revocation on
demotion, group validation) are pinned without touching the host in
`tests/users_integration.rs`.

## Invariants

- The `homevault_admin` group must never be exposed as an ordinary group
  checkbox — a past bug compared a display name to a full system name and never
  matched, leaking sudo. Guard logic lives in `src/groups.rs`.
- Deleting a user cleans up live terminal sessions (`handle_user_delete`) and
  removes its Linux account before the row (see *Host (Linux) account
  lifecycle*). A PAM-mirrored account whose Linux user still exists is refused.
- Every host command goes through `HostAccountService`: validated username,
  system-account guard, 20 s ceiling, `tokio::process` — never a bare
  `std::process::Command` from a handler.
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
- Group-level grants are resolved, never copied: `effective_visible_resources`
  unions `resource_shares` with `resource_group_shares` at read time from the
  member's live OS groups. Anything that lists resources must go through
  `visible_app_ids` / `can_access` / `event_visible_to` so the union rule is
  applied once. Guard: `resource_share_repo` unit tests +
  `resource_shares_integration.rs`.
- The expiry horizon has exactly one writer, `PUT /users/{u}/expiry`; the
  profile update deliberately never touches `expires_at`. Sign-in refusal lives
  in `AuthService::authenticate` *before* the password check (no credential
  oracle, no lockout footprint, PAM included) and an expired account is **not**
  a locked one — never reuse `locked`/`locked_until` for it.
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

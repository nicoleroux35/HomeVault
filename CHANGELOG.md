# Changelog

All notable changes to HomeVault are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the versioning
follows [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Fixed

- **The topbar's network, notification and power popovers are on the theme
  ladder, and clicking inside the notification panel no longer closes it.**
  The three buttons themselves were already one shape (56px circle, glyph at
  half), but they painted a bespoke inset focus outline instead of
  `--hv-focus-ring`, the network popover used one-off literals (84px rows,
  52px tiles, a 48px copy button) with a **22px** IP value — the page-title
  step for a row value — and the notification panel titled itself at 17px (the
  *section* step) with 12px form text. All three now follow the shared scale:
  ~60px rows with 40px tiles and a 44px copy button, 15px row values and panel
  titles, 13px form text, and the shared focus ring on every control.
  Separately, the notification panel's root was the only topbar popover without
  a `stopPropagation` guard, so a click on a severity chip, the search field or
  mark-as-read bubbled to the shell's global click handler and dismissed the
  panel (E2E 16.2 could never pass). The power confirmation sheet got the same
  pass: 18px glyphs, 44px buttons and a focus ring on its close button and
  footer actions, which had **none** (WCAG 2.4.7).

- **E2E `desktop.spec.ts` 16.4 mocks the health call it actually makes.** The
  route glob was `**/api/health/**` while the desktop calls `/api/health` (no
  trailing segment), so the 426 never fired and the reload-banner assertion
  could not pass.

- **The account menu now sits on the theme's type and space ladder.** The
  topbar user popover ran on a one-off scale that belonged to no token step:
  a 22px name (the *page-title* size), 17px menu rows 73px tall, 25px glyphs,
  a 76px avatar, 24px padding and a full 420px width — a settings sheet
  hanging under a 52px pill. It now uses the same steps as every other topbar
  popover: name `--hv-text-md` (15px), role `--hv-text-sm` (12px), rows
  `--hv-text-base` (13px) at **44px** (`--hv-touch-target-min`), 18px glyphs,
  a 40px avatar (rendered at 72px, CSS-scaled so it stays crisp on HiDPI),
  16px header padding and a 320px panel (a menu, not a data panel). Rows are
  inset 8px so their hover wash is a rounded control inside the card, and they
  finally carry the shared `--hv-focus-ring` on `:focus-visible` — keyboard
  focus in this menu was previously **invisible** (WCAG 2.4.7). E2E 4.1–4.4
  (shared card chrome across the three popovers) stay green.

- **The window frame footer's icons no longer read a size larger than the
  header's.** The shared 36px strip shipped with 16px help / resize-grip
  glyphs next to the min/max/close trio's 14px ones, so the two rows of
  window chrome disagreed — and because the footer's assets are full-bleed
  circles while the trio's are inset, its 13.75px of ink outgrew the trio's
  8–12px. Both now clamp to one new token, `--hv-winctrl-glyph` (14px): the
  footer buttons stay 28px, only the glyph box shrinks, and their inline SVGs
  drop the `width`/`height` attributes so the CSS owns the box — the rule the
  header's app icon tile already followed. E2E `14.13` pins the parity.

- **The launcher's search bar no longer carries a blue ring.** The brand focus
  ring was on screen permanently — the dialog's focus trap lands on the query
  field the moment the launcher opens — and it was painted twice, because
  `base.css`'s global `input:focus` rule outranks the field's own class and
  stamped a second blue ring inside the capsule. The bar now takes a 2px slate
  rim drawn *inside* the capsule (`inset`, 7.1:1 on the white surface, no
  second hue) and the field paints no ring of its own. Keyboard focus stays
  visible: the rim appears only while the field holds focus. E2E `5.13` pins
  it (no brand blue, no field shadow, rim inset and ≥3:1, rim gone on Tab).

### Changed

- **Settings lost both icon-tile header tiers — titles and descriptions only.**
  The content header (the active category's title) and every `SettingsCard`
  header (TLS, Antivirus, Anti-Ransomware, Firewall, AI Assistant, UPS, SNMP)
  carried a 36px tinted icon tile beside the copy. The category glyph already
  sits on the active rail item, so the tile only repeated it while costing the
  panel 36px of height per header. Both headers are now two lines — title over
  description — with `px-6 py-2.5` / `py-2.5` instead of `pt-5 pb-4` / `py-4`,
  and `SettingsCard` drops its `icon`, `iconVariant` and `accent` props, which
  only ever coloured that glyph. Measured: content header **87.5px → 57px**,
  card header **83.5px → 57px**. Getting there also uncovered a silent 16px:
  `base.css` declares an unlayered `p { margin-bottom: var(--hv-space-4) }`, so
  the `m-0` both subtitles carried never applied — the new header measured
  73px until it took `mb-0!`. E2E 23.3 now pins the new shape (two children,
  no icon, header ≤ 64px) instead of the 36px tile.

- **Every wallpaper thumbnail is the same size.** The grid let each image pick
  its own height, and the bundled thumbs are not all one shape — `hv-09` and
  `hv-13` ship 640×480 while the other thirteen are 640×427 — so those two
  rendered ~13px taller than their neighbours and the rows came out ragged.
  `.settings-wallpaper-thumb` now takes a fixed `aspect-[8/5]` with
  `object-cover`, so all 15 tiles share one box whatever the asset's ratio;
  `object-cover` crops instead of distorting. `h-auto` stays deliberately: the
  `height="200"` attribute is a presentational hint that would otherwise
  defeat `aspect-ratio`. E2E 4.7 additionally requires every tile to measure
  the same width and height.

- **A fresh install opens with every desktop widget turned off.** All eleven
  monitors in `monitorStore.ts::defaultPositions()` now default to
  `*Visible: false` — the desktop is clean until the operator picks widgets
  from the Widgets app or Settings → Widgets. Positions are still seeded, so
  a widget switched on lands in its intended slot, and an account with a saved
  layout keeps its own flags (the stored value is read first). **Reset to
  Default** in the Widgets app now restores that same shipped default
  (everything off) instead of turning everything on. E2E `widgets.spec.ts`
  3.5 follows, and 2.2 enables the Resource monitor before measuring the
  panel geometry.

- **Adding a web shortcut moved off the desktop and into the launcher.** The
  "add shortcut" control used to sit at the bottom of the desktop's shortcut
  column, so a desktop that only exists to *show* what the user pinned also
  carried a configuration action — and the column was rendered even when the
  set was empty, leaving a lone "Add" tile floating on the wallpaper. The
  desktop now renders the pinned set and nothing else (no empty column), and
  the add tile became the launcher grid's **last cell**, after every app:
  same tile geometry, dashed ring instead of an app's identity gradient,
  `aria-label` from `shortcuts.add`. It is withheld while a search query or a
  category chip narrows the grid (a filtered grid answers "where is X?") and
  once `MAX_SHORTCUTS` is reached. The URL dialog moved to
  `components/desktop/ShortcutAddDialog.svelte`, owned by `Desktop.svelte`,
  so the launcher only fires a callback and closes. `shortcuts.open` /
  `shortcuts.remove` also stop shipping literal `{title}` to screen readers:
  the placeholder syntax is `%title%`, which is what the translator
  interpolates. E2E `shortcuts.spec.ts` re-targets the launcher tile (8
  tests; `1.2` pins "last cell, after every app") and `desktop.spec.ts`
  `5.14` excludes that tile from its A→Z run.

- **The external-agent (MCP) gateway is configured in Settings, not in the AI
  app.** The admin panel that used to slide in from the right of the Chat
  window (enable switch, permission editors, token mint/revoke) is now the
  `McpCard` of Settings → AI Assistant, next to the model card — the gateway
  is a server-wide admin configuration, so it no longer lives inside a
  conversation view. The card is admin-only and self-gating: it probes
  `GET /api/mcp/status` and renders nothing when that probe is rejected, so
  non-admins never see it. Its writes stay immediate (kill-switch, global
  permissions, one-time tokens) rather than joining the Settings draft — a
  deferred Save would leave the gateway open after the operator switched it
  off. `components/chat/McpPanel.svelte` is deleted; the Chat toolbar no
  longer carries the toggle. E2E moved to `tests/e2e/settings/mcp.spec.ts`
  (§15, 7 tests, incl. one that pins the Chat app free of the panel).

- **The desktop power menu now wears the dock's shape language.** The three
  actions behind the topbar Power button (Shut Down / Restart / Log Out) were
  three rounded cards; they are now three **circular gradient tiles with the
  label underneath**, exactly like a dock tile: `--hv-dock-item-size` diameter
  (56px desktop, 48px on the phone — still above the 44px target), glyph at
  half the tile, one hue per action (red / blue / slate), and the dock's own
  hover lift + `brightness(1.05)`. The button around the tile paints nothing
  and keeps the one control corner, so the focus ring and E2E `14.12`'s
  corner contract are unchanged. New E2E `7.5` pins the circle, the
  token-parity with the dock tile, the three distinct gradients, the
  label-under-tile anatomy and the touch target.

- **The launcher lists its apps A→Z, and its veil now hides the desktop.**
  Two pieces of user feedback on the same screen. (a) The tile grid was sorted
  by a hand-kept `CAT_ORDER` and then alphabetically inside each category, so
  the first tile was "System" and there was no alphabetical entry point into
  ~25 apps; the grid is now one A→Z list (`Intl.Collator` on the UI language,
  accent/case-insensitive, numeric), with the chip filter applied on top, and
  the category chips are sorted by their *localised* label so the row reads in
  the same order as the tiles under it. E2E `5.14` pins both, before and after
  a filter. (b) `--hv-launcher-scrim` `rgba(15,23,42,0.72)` → **`0.86`** and
  `--hv-launcher-blur` `2px` → **`6px`**: the desktop's icons and wallpaper
  stayed legible behind the launcher, which read as a translucent panel over
  the desktop instead of an app screen. Contrast only improves — the
  worst-case tile label (tile → veil → white paper) goes 5.5:1 → **8.7:1**.

- **Both desktop pills are 72px tall, and their controls grew with them.**
  `--hv-chrome-dock-h` 64px → **72px** and `--hv-chrome-topbar-h` 62px →
  **72px**, each on its own token (the dock and the topbar had been 84/62,
  then 64/62; above phone width they now match). Everything inside follows the
  reference dock's 0.775 ratio instead of a pixel count: dock tiles
  `--hv-dock-item-size` 50px → **56px** (glyph 25px → **28px**), topbar
  buttons `--hv-chrome-topbar-btn-size` 48px → **56px** (glyph 24px →
  **28px**), identity avatar 32px → **36px** (half the pill), dock count badge
  20px → **22px** (40% of the tile), dock group divider 32px → 36px, topbar
  divider now `calc(btn / 2)` = 28px. The dock's vertical padding stays
  derived — `calc((var(--hv-chrome-dock-h) - var(--hv-dock-item-size)) / 2)`
  = 8px — so the tile still fills the bar exactly; horizontal padding and the
  10px pitch are unchanged (the pitch is floored by the hover lift, now 5.04px
  a side). Phone keeps its compact pair: **56px** topbar with 44px buttons and
  a **52px** dock with 44px tiles, the only width where the dock is shorter
  than the topbar. `snapUtils.ts` moves with it (`topbarHeight: 80`,
  `dockHeight: 72`), and E2E `8.3` now pins 72/72 above 480px.

- **The dock is 64px tall on desktop/tablet (was 84px), 52px on phone.**
  `--hv-chrome-dock-h` drops from the mockup's authored 84px, and the tile
  follows the reference dock's 0.775 ratio down with it: `--hv-dock-item-size`
  64px → **50px**, glyph 32px → **25px** (now `calc(tile / 2)`), count badge
  26px → **20px** (the same 40%-of-tile proportion, matching the phone's own
  value), group divider 40px → 32px (`0.64 × tile`). The bar's vertical padding
  is now **derived** in CSS — `calc((var(--hv-chrome-dock-h) -
  var(--hv-dock-item-size)) / 2)` = 7px — so a future height change can never
  leave dead space around the icons; horizontal padding and pitch stay at 10px
  (the pitch is floored by the hover lift: 4.5px a side, so ~9px minimum).
  Phone goes 56px → 52px = the 44px touch floor (its tile size, unchanged) +
  4px a side, which makes the phone the one width where the dock is shorter
  than the topbar pill. `snapUtils.ts` (`dockHeight: 64`) and the desktop E2E
  lockstep test moved with it.

- **Every app window now ends in a shared frame footer.** `WindowFrame`
  renders one 36px chrome strip under the page: a help button on the far left
  (opens the in-product assistant) and the window's resize grip on the far
  right. The grip replaces the invisible 18px SE corner hotspot and uses the
  `resize-circle-horizontal` glyph rotated 45° clockwise, so it reads as the
  bottom-right diagonal it pulls; it is a real button (pointer drag + arrow
  keys, 16px a press, 64px with Shift) and is omitted while the window is
  maximized. The seven edge strips are unchanged. New i18n keys
  `desktop.window_help` / `desktop.window_resize` in all four locales. See
  `docs/guides/desktop.md` → *Frame footer*.

- **Design tokens now follow `design-tokens.md` for everything except colour.**
  The Apple-style spec at the repo root is adopted as the reference for the
  non-colour craft, translated onto the `--hv-*` vocabulary in
  `frontend/src/lib/styles/theme/tokens.css`: a layout rhythm
  (`--hv-section-gap-*`, `--hv-component-gap-*`, `--hv-card-padding-*`,
  `--hv-content-max-width*`), a 8/12/18 corner ladder
  (`--hv-radius-badge` / `-default` / `-2xl`, plus `--hv-radius-pill` for
  tags), multi-layer contact + ambient elevation (`--hv-shadow-*`), the 400–900
  weight scale, 1.1 title / 1.53 body leading, -0.015em title tracking and the
  734/1068/1440 layout breakpoints. `.hv-card` takes the 32px card padding and
  the resting card shadow; `.hv-badge` takes the 8px badge corner. Its **colour
  palette is deliberately not adopted** — one accent (`#1E62AE`) and the
  blue-grey ink ramp stay the only colour authority — and form buttons keep the
  control corner rather than the reference's pill (one documented deviation).
  See `docs/guides/design-system.md` §2.2 and §3.2.

### Added

- **Scheduled Tasks** (new app, migration `043_scheduled_tasks.sql`): one
  generic cron engine replacing per-feature schedulers for admin-defined
  recurring jobs — single-executable commands (no shell, absolute path,
  30-minute timeout), pool snapshots, S.M.A.R.T. self-tests and host power
  actions. Payloads are validated at creation *and* at run time
  (`TaskAction::from_parts`); every execution is recorded in a run history
  with an output tail. See `docs/guides/scheduled-tasks.md`.
- **Metrics** (new app, migration `044_metrics_history.sql`): a 60 s
  collector (CPU deltas, memory, load, network throughput, max thermal-zone
  temperature) with 8-day retention, bucketed 1 h / 24 h / 7 d SVG charts,
  gaps rendered as gaps. See `docs/guides/metrics.md`.
- **Host power management + disk spindown** (Settings → System, migration
  `045_disk_power.sql`): CPU governor (validated against the kernel list),
  host reboot/shutdown/suspend, and per-disk hdparm APM + idle timeout that
  are re-applied at every startup. See `docs/guides/power.md`.
- **Per-event notification toggles + webhook sink formats**: every engine
  notification carries a stable event id (`backup.jobs`, `app.updates`,
  `health.<check>`); admins mute events individually
  (`GET|PUT /api/notifications/events/config`, absent = enabled) and choose
  the webhook payload shape (`generic` | Slack | Discord). See
  `docs/guides/notifications.md`.
- **Snapshot schedules with GFS retention** (Storage → new "Snapshots" tab,
  migration `046_snapshot_schedules.sql`): cron-driven `{prefix}-<timestamp>`
  snapshots per pool with grandfather-father-son pruning (newest N
  hourly/daily/weekly/monthly/yearly generations); pruning only ever deletes
  snapshots carrying the schedule's own prefix — manual and backup snapshots
  are untouchable. See `docs/guides/snapshots.md`.
- Fixed a latent `api.ts` bug where `ttl: 0` is falsy and silently fell back
  to the 5 s default TTL (the topbar notification list, What's New, VPN peer
  detail and SMART capabilities stayed stale for up to 5 s after a mutation;
  now `{ cache: false }`).

- Persistent notification center: the bell panel now keeps the full history
  with severity filters, an unread-only toggle and text search, and the
  "What's New" release-highlights modal shown once per version per account
  (`GET|POST /api/whats-new[/seen]`, migration `042_whats_new.sql`,
  `src/release_notes.rs`). See `docs/guides/notifications.md`.
- Storage health: a per-virtual-disk scrub-status endpoint + inline UI
  (`GET /api/storage/virtual-disks/{name}/scrub`) and foreign-pool import for
  ZFS/Btrfs pools discovered on attached disks (`GET /api/storage/importable-pools`,
  `POST /api/storage/import-pool`). See `docs/guides/storage.md`.
- Hardened app-proxy auth: proxied apps no longer receive the API session.
  The proxy gate accepts only an app-proxy-scoped `hv_app_session` token minted
  by the server-chosen `GET /app-auth` entry, and both HomeVault cookies are
  stripped before forwarding upstream. See `docs/guides/docker.md`.
- Versioned client/server contract: `GET /api/meta` + the
  `X-HomeVault-Contract` header, with a 426 + reload banner on a mismatch
  (`src/api_contract.rs`, `src/middleware/contract.rs`). See
  `docs/ARCHITECTURE.md`.
- Development tooling: `scripts/dev-instance.sh` (multi-instance `.local`
  dev via `HV_MDNS_HOSTNAME`), a real-VM Playwright suite
  (`playwright.vm.config.ts`, `tests/vm/*.vm.test.ts`), and
  `docs/guides/development.md`.

- External AI-agent server (MCP, Streamable HTTP): `POST /mcp`
  (`initialize`, `tools/list`, `tools/call`) with opaque `hv_mcp_…` bearer
  tokens, one master kill-switch and one global permission set shared by all
  tokens (apps / App Store / files / VMs / knowledge base / reboot — deny by
  default), managed from an admin-only "External agents" panel in the Chat app
  (`GET|PUT /api/mcp/status`, `PUT /api/mcp/permissions`, `POST|DELETE
  /api/mcp/tokens`). The `knowledge.search` tool exposes the internal RAG base
  to agents. See `docs/guides/mcp.md`.
- App catalog phase B (feature parity on the same `homevault-app.toml` shape):
  dependencies with `implements` alternatives + install-review endpoint,
  `[[environment]]` install settings rendered as `{{NAME}}`, `arch` +
  `min_homevault_version` compatibility (listed and enforced), per-app compose
  logs, and a sequential `update-all` queue. Install/uninstall share their core
  with the MCP `apps.*` tools. See `docs/guides/docker.md`.

- GitHub community health files: `LICENSE` (MIT), `SECURITY.md`,
  `CONTRIBUTING.md`, `CODE_OF_CONDUCT.md`, issue/PR templates, Dependabot,
  `CODEOWNERS`, `.editorconfig`, `.gitattributes`.
- SPDX metadata (`license`, `repository`, `homepage`) in `Cargo.toml`,
  `frontend/package.json`, and root `package.json`.

### Added

- **Automatic snapshots of HomeVault's own database** (`src/db_backup.rs`,
  migration `048_app_meta.sql`). The database runs in WAL mode, so copying
  `homevault.db` on its own yields a file with **no tables at all** (measured:
  a 4 KB `.db` next to a 1.8 MB `-wal`) — the obvious manual backup silently
  produced nothing restorable. Snapshots are written with `VACUUM INTO`, which
  produces a single self-contained file: at startup when due, daily, before any
  migration is applied, and on demand. Settings → System → *Database snapshots*
  shows the last run, the schedule and the files on disk.
- **Liveness and readiness probes** — `GET /api/health/live` (public) and
  `GET /api/health/ready` (public, 503 when the database cannot be queried or
  was found damaged at boot). `GET /api/health` previously answered **HTTP 200**
  in both of those cases, so no monitor could detect an outage from it.

### Security

- **CSP no longer allows `'unsafe-inline'` scripts.** `script-src` lists a
  per-response nonce instead, and the single inline boot script in `index.html`
  carries it. With `'unsafe-inline'` plus the session token in `localStorage`,
  any injected script could read the token and exfiltrate a full session.
  `style-src` keeps `'unsafe-inline'` (Svelte scoped styles and `style=""`
  attributes cannot be nonce'd) — documented debt in `guides/security.md`.
- **The installer can roll back and uninstall.** `--rollback` swaps in
  `/opt/homevault/homevault.previous` (kept by every upgrade); `--uninstall`
  removes the app but **keeps** the data, and `--purge` deletes it behind a
  typed `PURGE` confirmation; `--dry-run` prints every action and changes
  nothing (and works without root). A failed release no longer leaves an
  appliance with no way back.
- **HTTPS now actually protects the login.** Enabling TLS left the plain-HTTP
  listener serving the wizard and the login form — admin password in clear — and
  `redirect` defaulted to **off**, so nothing changed in practice. It now
  defaults to **on** (opt out with `TLS_REDIRECT=0`), a legacy `data/tls.json`
  picks up the safe default via `serde(default)`, and the installer's HTTPS
  question defaults to **yes**. The session cookie gains `Secure` when the
  request arrived over TLS, and the wizard + login screen show a
  `role="status"` warning when served over plain HTTP on a non-loopback host.
- **Removed the dead `fail2ban` middleware.** `src/middleware/fail2ban.rs`
  defined an in-memory IP ban list that no route or handler ever referenced — it
  read like an active protection while doing nothing. Brute-force defence is the
  per-account lockout and the per-IP throttle (see below).
- **The login lockout is temporary again.** Five wrong passwords used to set
  `users.locked` and never clear it, so on a single-admin appliance a typo storm
  bricked the only account — the message said "temporarily locked" while the
  only ways back in were the offline recovery code or editing the database. A
  lock now records `locked_until` (migration `047`): an automatic lock lifts
  itself once the timestamp has passed and a successful login clears it too,
  while a deliberate admin lock (no `locked_until`) stays permanent. The
  migration repairs accounts already bricked by the counter.
- **Authentication is throttled per client IP.** The only counter was per
  account, so password spraying — one password tried across many usernames —
  was unlimited. `AuthService::check_ip_rate_limit` now runs before the account
  lookup on both `/auth/login` and `/auth/2fa/verify`: 30 attempts/minute and
  120/hour per IP, returning `429`. The handler honours that status instead of
  collapsing it into a 400.
- **The client IP is the TCP peer, not `X-Forwarded-For`.** Trusting the header
  outright let anyone forge the address recorded in the login history — and,
  now that the throttle is keyed by IP, sidestep it by rotating a header value.
  The headers are honoured only from a loopback peer (a reverse proxy on this
  host) or an address listed in `HOMEVAULT_TRUSTED_PROXIES`.
- **Login accessibility:** the inline error is now a `role="alert"` /
  `aria-live="assertive"` region, the credential fields carry `aria-invalid` +
  `aria-describedby`, and closing the error modal returns focus to the form
  instead of dropping it on `<body>`. The locked-account message points at the
  recovery-code path rather than "contact support".
- **The first-run bootstrap endpoints can no longer be driven from a foreign
  web page.** The CORS layer is permissive (`allow_origin(Any)`), so the
  local-network guard alone left a hole: the operator's browser *is* on the LAN
  and the mDNS name is predictable, so any site they visited could
  `POST /api/auth/setup` and claim the first admin. `middleware::setup_guard`
  now also refuses a foreign `Origin`, `Origin: null` and any `Sec-Fetch-Site`
  other than `same-origin`/`none` (403), and rate-limits the guarded routes to
  20 requests/min per peer (429). `HOMEVAULT_SETUP_ALLOW_REMOTE=1` no longer
  bypasses these two checks.
- **The setup lock is self-healing.** `.setup_lock` records `pid` +
  `created_at`; a dead PID or an age over 15 minutes makes it reclaimable
  (`mark_setup_in_progress` removes and re-creates it under `O_EXCL`), and
  `cleanup_stale_setup_lock()` runs at startup. A crash, an OOM kill or a power
  cut during setup used to dead-end the wizard permanently — `is_first_run`
  stayed true while every submission answered "Setup is already in progress".
- **Password policy raised to 12 characters, with a compromised-password
  check.** `SetupService::validate_password` is now the single policy (the
  wizard, admin-created users, password changes and recovery-code resets all
  delegate to it): 12–128 characters, mixed case + digit, not on the
  obvious/breached list (`Password1234`, `Homevault2026`, `qwerty…`, …) and no
  character repeated more than four times in a row. The wizard's live checklist
  lists all six rules and gates its "Next" button on them.
- **The recovery-code screen is blocking.** A failed generation now offers a
  retry instead of a "continue": the code is the only offline way back into the
  account (there is no email reset), so the wizard no longer lets an admin walk
  away without one.

### Removed

- **Setup Wizard: the storage, RAID, restore *and language* steps are gone** —
  the wizard is down to 4 steps (welcome, account, password, verify). RAID is
  configured from its own app, the appliance is installed by script on an
  existing OS, a backup is restored from the Backup app, and the interface
  language is now **detected from the OS/browser locale** (English when
  unsupported, changeable in Settings) instead of being asked during
  onboarding. Removed with them: `GET /auth/setup/storage-options`, the
  frontend `getSetupStorageOptions` service method, the
  storage/RAID/restore/language step markup in `SetupWizard.svelte`, the
  `setup.storage.*` / `setup.raid.*` / `setup.restore.*` / `setup.language.*`
  i18n keys in all four locales, and the matching integration test.
  `setup_progress.data` now carries nothing.

### Added

- **OS locale detection for the interface language**: `GET /auth/setup-status`
  reports `system_language` (`SetupService::detect_system_language()` reads
  `LC_ALL` → `LC_MESSAGES` → `LANG` → `LANGUAGE`, then `/etc/default/locale`),
  and `languageStore.applySystemLanguage()` uses it when the user made no
  explicit choice and the browser locale is not one HomeVault ships. Settings
  remains the only place a preference is stored.

### Fixed

- **An older HomeVault binary silently ran against a newer database.** Migrations
  are append-only and never reverted, so a downgrade started happily and then
  failed on the first query expecting a column it does not know about. Startup
  now compares `app_meta.schema_version` (migration `048`) with the highest
  migration embedded in the binary and **refuses to start** with a message that
  says what to reinstall — and `src/bin/server.rs` propagates the failure so the
  process exits non-zero (`systemd Restart=on-failure` can see it).
- **A damaged database was only discovered at the first query.** Startup runs
  `PRAGMA quick_check(1)` and surfaces a failure through `/api/health/ready`
  (`503 degraded`) instead of waiting for random queries to blow up.

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

- **One corner language across the shell, the apps and the gateway.** An
audit found the radius scale applied unevenly: the desktop widget/monitor
panels, the notification list rows and the floating path picker carried
literals off the appliance stops (28px power dialog, 20px notification
rows, 14/7/9px picker parts), a dead `--hv-radius-pill` token, an undefined
Tailwind `rounded-4xl` on the badge, and `calc(var(--radius) - 5px)` (1px)
inside the input group. Every corner now reads a stop from
`docs/guides/design-system.md` §3.2 — window `--hv-radius-2xl` (16px),
floating chrome panels `--hv-radius-4xl` (24px), cards/modals/pickers
`--hv-radius-xl`/`--hv-radius-card` (12px), controls/fields/buttons
`--hv-radius-control`/`--hv-radius-md` (8px), pills/dots
`--hv-radius-full`. The login and setup-wizard fields and buttons moved off
`rounded-lg` (10px, the card stop) onto the control stop so they match the
shared `Input`/`Button`. Guards: `desktop.spec.ts` 7.4 (power dialog ==
topbar popover = 24px), `login.spec.ts` 13.7 (setup card 16px / login field
8px). The `--hv-radius-4xl` documentation row was corrected (it never was
the login/setup card stop) and the stale `--hv-radius-sm` (2px) note in the
theme README was fixed.

- **The WebDAV server is now part of HomeVault (Rust), not a generated
  Python script + root systemd unit.** `src/webdav/` is an in-process axum
  listener built on `dav-server` (same hyper 1.x stack as the app proxy),
  managed by `WEBDAV` (`start`/`stop`/`reload`, mirrors `APP_PROXY`). It also
  fixes three real defects in the old implementation: Basic auth now actually
  verifies the HomeVault credentials (the Python `_check_auth` accepted any
  `Authorization: Basic` header), the URL's first segment is a **share name**
  confined to that share's root (the old server ignored `path_prefix` and
  served the process CWD as root), and `PROPFIND` is authenticated. The
  `share` grants of `resource_shares` are enforced per member on every
  operation, and `path_prefix` / `require_auth` / `require_ssl` / `locking` /
  `allowed_methods` are all honored (the default config is closed: port 8082 +
  auth on). An upgraded host disables and removes the legacy
  `homevault-webdav.service` + `homevault-webdav.py` at startup. See
  `docs/guides/shares.md`.

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
- **The login screen and the setup wizard now use the desktop's palette
  instead of a second identity of their own.** Both carried a page-scoped
  indigo/violet "gateway" palette — a second accent, a pastel gradient and a
  slate ink ramp one step lighter than the app. Every `--hv-login-*` /
  `--hv-wizard-*` value is now an alias of an Appliance token: the accent
  *is* `--hv-primary` (#1E62AE), the canvas *is* `--hv-desktop-bg` (declared
  once — `Desktop.svelte` no longer keeps its own copy of that gradient), the
  ink is `--hv-shell-text-*`, and the card borrows the desktop window's
  hairline and elevation. The wizard's emerald family (`#10B981` / `#22C55E` /
  `#16A34A` and its create gradient) failed WCAG 1.4.3 / 1.4.11 in every role
  it carried ink (2.28–3.77:1) and was replaced by the Appliance success ramp
  (`#1E7E34` / `#166428`); the amber recovery box is now white paper inside a
  `--hv-warning` border with an amber code chip, and the login screen uses one
  error red instead of two. `login.spec.ts` 13.6 asserts the aliases *and*
  every contrast floor of the gateway palette. See
  `docs/guides/design-system.md` §6.1/§6.2.

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

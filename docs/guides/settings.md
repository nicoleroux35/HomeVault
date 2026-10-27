# Settings

**Purpose:** Server configuration grouped into categories (general, security,
appearance, widgets, notifications, ai, monitoring, system). Aggregate fetch +
per-category get/put. The screen was re-skinned on 2026-09-14 to the Figma
"Settings Screen" design (`Sans titre`, node `16-6187`) — a 208px two-line
category rail beside a content column with its own header, over one
section/row/card vocabulary shared by the seven embedded sub-cards (TLS,
Antivirus, Anti-Ransomware, Firewall, AI Assistant, UPS, SNMP). Both header
tiers are two lines of text — title over description, no icon tile — since
2026-10-19. A design
change may touch any of it, as long as the tests stay green
(`CLAUDE.md` → Design system governance). The group Disk-Quota card + its
`/api/quotas` backend were removed 2026-09-09 — see
[Disk Quotas](./quotas.md).

## Backend

Handlers in `src/handlers/settings.rs`. Persistence via
`src/settings_persistence.rs` (writes `data/settings.json`).

| Endpoint | Auth | Notes |
|----------|------|-------|
| `GET /settings` | auth | aggregate of all categories |
| `GET /settings/public` | public | safe subset (server name, language, version) |
| `GET/PUT /settings/general` | auth / admin | server name, timezone — the timezone is validated and **applied to the host** (see below) |
| `GET/POST /settings/language` | auth | set UI language — persisted as `general.language` and re-applied at boot before the first page renders (ROADMAP 13.4) |
| `GET /translations` | auth | flattened translation map (mounted at `/api/translations`; see [i18n](../I18N.md)) |
| `GET/PUT /settings/email` | auth / admin | SMTP relay; validated before it is stored and used to generate the `msmtp` configuration (ROADMAP 13.3) |
| `POST /settings/email/test` | admin | sends a real test email via `EmailService::send_test_email` to `NOTIFICATION_SETTINGS.email` (previously a stub that only logged and always reported success) |
| `GET/PUT /settings/notifications` | auth / admin | alert email, threshold, `webhook_url`/`webhook_enabled`; the URL is validated at save time and again at dispatch |
| `GET /settings/appearance` | **any session** | the appliance look: `wallpaper` (one of the 44 bundled — no upload) + `accent_color`. Read by every account's boot hydration (ROADMAP 26.1); carries nothing else — no e-mail, SMTP, update or notification field |
| `PUT /settings/appearance` | admin | same two fields, validated (see below). `theme` / `density` no longer exist (ROADMAP 26.2) |
| `GET/PUT /settings/update` | auth / admin | update-check frequency — **default `Manual`** since 2026-09-07 (G2: no automatic outbound call to GitHub) |
| `GET /settings/version` | auth | current version + last **cached** check. Contacts the update endpoint only when the saved frequency says a scheduled check is due — opening Settings is never an outbound call by itself |
| `POST /settings/version/check` | admin | "Check now": the only path that **forces** an outbound check, whatever the saved frequency |
| `GET /settings/export` | admin | JSON dump, stamped with `schema_version` |
| `POST /settings/import` | admin | JSON import; schema version and every field are validated first |
| `POST /settings/reset` | admin | reset to defaults, including the families stored outside `settings.json`; partial failures come back as `warnings` |
| `GET /settings/backup/db` | admin | snapshots of HomeVault's own database (settings + files on disk + total size) |
| `PUT /settings/backup/db` | admin | update `interval_hours` / `retain`; a lower retention prunes immediately |
| `POST /settings/backup/db/now` | admin | take a `VACUUM INTO` snapshot now |
| `GET/PUT /snmp/settings` | admin | SNMP community/version (`v1`/`v2c` only) |
| `GET /snmp/availability\|info\|interfaces\|storage` | admin | SNMP-discoverable device data; `interfaces` accepts `iface` to narrow the result |
| `GET /ups/availability\|list\|status/{name}\|variables/{name}` | admin | UPS/NUT monitoring (live readings) |
| `GET/POST /ups/config`, `DELETE /ups/config/{name}` | admin | manage the `ups.conf` device sections (serial/network/USB), then reload NUT; each unit that refused the restart is named in `reload_warnings` |

### General: server name and timezone (ROADMAP 13.4)

- **Server name** → sanitized to an RFC 1123 label and applied with
  `hostnamectl set-hostname` (scoped sudoers grant); a rename reports
  `restart_required`, surfaced as an `Alert` on the General card.
- **Timezone** → validated as an IANA name *before* anything is persisted
  (an unknown zone answers `400` naming the value), then applied with
  `timedatectl set-timezone` (scoped sudoers grant, run through
  `spawn_blocking` so the handler never blocks the runtime). A failure to
  apply does **not** silently look like success: the save still returns
  `200` but carries `timezone_notice`, shown as a warning `Alert`.
- The picker offers the **whole IANA database**, not a shortlist: the screen
  renders `GET /settings/timezones` (every region-prefixed name `chrono-tz`
  ships, plus `UTC`, sorted) grouped by region, so a zone the backend accepts
  can never be unpickable in the UI. Legacy aliases without a region (`CET`,
  `GB-Eire`) stay accepted on write but are kept out of the list, where they
  would duplicate every entry under a name nobody looks for; a value that is
  not in the list (an externally-set alias, or a failed fetch) is still shown
  as its own option so the field never silently rewrites the setting. The
  hint under the field says what the zone drives — the server clock and every
  scheduled task without its own `timezone`.
- The grouping is shared, not duplicated: `frontend/src/lib/utils/timezones.ts`
  (`groupTimezones`, `timezoneIsListed`, `SERVER_TIMEZONE`) is what both this
  card and the scheduled-task form render, so the two pickers cannot drift
  apart again (the task form was a free-text `Europe/Paris` placeholder while
  this card offered a 14-zone shortlist — one of them refused nothing and the
  other accepted almost nothing).
- `settings::effective_timezone()` is the value the rest of the app reads —
  notably the scheduled-task engine, which evaluates every task without its
  own `timezone` in it (see [Scheduled Tasks](./scheduled-tasks.md)). It is a
  pure in-memory read; `system_timezone()` (the `/etc/timezone` /
  `/etc/localtime` reader) runs only at startup and in the Dashboard's
  offloaded server-details builder.
- On a fresh install (no persisted settings file) the timezone is **seeded
  from the host** at startup, so the setting never contradicts the real
  system zone.

### General: local-network discovery (ROADMAP 29.1, 2026-10-24)

A **"Discoverable on the local network"** switch (`mdns_enabled`) sits in the
General card, after *Start on boot*. It is the on/off control for the
`homevault.local` advertisement described in [mdns.md](./mdns.md).

- It **ships on** (`GeneralSettings::default().mdns_enabled == Some(true)`):
  an appliance that cannot be found on the LAN on a fresh install is not the
  product. Only an explicit `false` switches it off.
- The save **reaches the host**, not just `settings.json`:
  `handle_general_settings_update` calls `crate::mdns::apply_enabled(port,
  enabled)` on `spawn_blocking` (it runs `sudo rm`/`sudo tee`). Off →
  the Avahi service file and the `host-name` override are removed and the
  daemon renamed back; on → both are written back immediately instead of
  waiting up to 120 s for the reconciler.
- A host that refuses (no `sudo`, no D-Bus) answers `200` with
  `mdns_notice`, shown as a dismissible `Alert` (`data-testid="mdns-error"`,
  `settings.mdns_apply_failed`). A save that could not reach the host must not
  look saved — the same rule as `timezone_notice` and `autostart_error`.
  `mdns_notice` carries the host's **raw** reason (an stderr tail), never a
  server-translated sentence: the card wraps it in the localised
  `settings.mdns_apply_failed` (`%reason%`), so the wording follows the user's
  locale instead of the server's — the `autostart_error` contract, not the
  `timezone_notice` one.
- Both `GET /settings/general` and the aggregate `GET /settings` report the
  **live** flag (`crate::mdns::is_enabled()`), not the stored copy: they agree
  except in that failure window, and the screen must show what the appliance
  actually does.
- The staged draft field is `draft.general.mdns_enabled`; like every other
  General field it is only persisted by the rail's **Save** button.

Before this existed, `crate::mdns::unpublish()` had **no caller**: the only way
to stop advertising was to disable Avahi by hand. The dead function was the
evidence the roadmap tracked.

### General: language (ROADMAP 13.4)

- `handle_set_language` writes `GENERAL_SETTINGS.language` and calls
  `save_all_settings()`, so the choice is part of `settings.json`. Before this
  it only mutated the in-process locale and the choice reverted to `en` on the
  next restart.
- `settings_persistence::apply_loaded_settings` reads `general.language` and
  calls `I18n::set_locale` during `init_persistence()`, i.e. before the HTTP
  server accepts its first request — the first rendered page is already in the
  chosen language.
- Only the four shipped locales (`en`, `fr`, `de`, `es`) are accepted.
  `normalize_language()` trims, lowercases and keeps the primary subtag, so a
  browser tag (`fr-BE`, `es-419`, `de_AT`) or a hand-edited value (`FR`)
  normalises instead of being refused; a tag that is not
  `language[-script][-region]` (`en-US-extra-extra`) and one whose language is
  not shipped both answer `400`. `reset_settings()` puts the
  process back on `en`.
- The General card saves the language through `POST /settings/language` rather
  than the PUT, because the locale has to change immediately. A failure is
  surfaced as a warning `Alert` (`settings.language_save_failed`) instead of
  looking saved. On load, `Settings.svelte` seeds `draft.general.language` from
  the active `$languageStore` when no non-default language has been persisted on
  the server yet, so the selector matches the active UI language detected from
  the browser/OS locale.

### Settings stored outside `settings.json` (ROADMAP 13.5)

`reset_settings()` clears the JSON blob and the in-memory statics;
`reset_domain_settings()` (`handlers/settings.rs`) handles the rest:

| Family | Where it lives | Reset through |
|---|---|---|
| Antivirus (quarantine dir, schedule, archive toggle) | `av_settings` row | `AntivirusRepository::save_settings(AvSettings::default())` |
| Anti-ransomware thresholds | `ransomware_settings` row | `RansomwareRepository::save_settings(default)` |
| Per-disk spindown | `disk_power_settings` rows | `DiskPowerRepository::delete_all()` |
| Database snapshots | `data/db_backup.json` | `DbBackupService::reset_settings()` (the scheduler also re-reads the file every tick) |
| UPS devices | `/etc/nut/ups.conf` | `ups::reset_config()` — shipped section restored, NUT reloaded |

Scan history, quarantined threats and protected paths are evidence/admin data,
not settings, and are deliberately left in place. A domain that fails is
returned in `warnings` (the response is still `success: true`, because most of
the reset did happen) and the page shows `settings.settings_reset_partial`
instead of the plain success toast.

### UPS device configuration and NUT reloads (ROADMAP 13.2)

- Every `upsc` read validates the device name (`validate_ups_name`, a
  `[A-Za-z0-9._-]` label) and the variable (`validate_ups_variable`) before it
  reaches the argv; both refuse before any process starts. Neither value may
  contain whitespace (a trailing newline is refused, not trimmed away) and a
  name may not start with `-`, which `upsc` would read as an option.
- Writes go through the scoped sudoers grant built by `nut_grants()` in
  `src/setup.rs`: `tee /etc/nut/ups.conf`, `systemctl restart
  nut-driver-enumerator.service` and `systemctl restart nut-server.service`.
  Before this the grant contained no `nut-*` entry at all, so `restart_nut()`
  could never succeed and a save reported success while the daemon kept its old
  configuration.
- `save_config` / `delete_config` / `reset_config` return
  `Result<Vec<String>, UpsError>`: the file is written first (an edit is never
  lost to a refused reload), then every unit that failed is collected. The
  handlers expose that as `reload_warnings`, and `UpsCard` renders it with
  `role="status"` — "saved" and "live" are no longer the same claim.
- All UPS calls run through `spawn_blocking` (ROADMAP X.1) with deadlines
  (`NUT_QUERY_TIMEOUT` 10 s, `NUT_RESTART_TIMEOUT` 60 s; ROADMAP X.2).

### SNMP (ROADMAP 13.5)

- The saved `version` now reaches `snmpget`/`snmpwalk` (`-v <version>`); it was
  write-only, with `run_walk` hardcoding `2c`.
- Only `v1` and `v2c` are accepted. `v3` needs a user plus auth/privacy
  protocols this card does not collect, so it is refused at save time with a
  message saying so rather than silently running as v2c. The select no longer
  offers it and the hint under it says why.
- `GET /snmp/interfaces?iface=<name>` filters the parsed table (matched against
  `ifName`, case-insensitively); an empty value keeps every interface. The
  parameter used to be accepted and ignored.
- Queries run on the blocking pool with a 10 s deadline (`SNMP_QUERY_TIMEOUT`).

### Import and export format (ROADMAP 13.5)

- Every export carries `schema_version` (`CURRENT_SETTINGS_SCHEMA`). An import
  from a newer schema is refused with both numbers in the message instead of
  being half-applied.
- `validate_import()` checks every field before the blob replaces the stored
  one, and names the offender (`general.timezone 'Mars/Olympus' is not a known
  IANA timezone`, `email.smtp_port must be 1-65535`,
  `notifications.webhook_url: …`). An absent `schema_version` is a legacy blob
  and is still accepted.

## Frontend

- `frontend/src/routes/Settings.svelte` — a single component with a left
  category rail (no separate layout/sidebar files). Renders its own
  `<WindowHeader>` (`settings` is in `WindowFrame`'s `OWN_HEADER_APPS`).
  Embedded cards live in `frontend/src/lib/components/settings/`.
- **Category rail = 8 groups** (`SETTINGS_CATEGORIES` + `activeCategory`):
  `general`, `security`, `appearance`, `widgets`, `notifications`, `ai`,
  `monitoring`, `system`. Several groups stack more than one
  `{#if activeCategory === …}` block, each with its own section label: `security`
  = HTTPS/TLS + Antivirus + Anti-Ransomware + Firewall; `appearance` = wallpaper
  grid only; `widgets` = active monitors + refresh interval + weather widget;
  `notifications` = e-mail/SMTP + alerts + webhook; `monitoring` = UPS + SNMP;
  `system` = software update + system information + data management + privacy.
  Blocks stay in their original file position — only the guard value changed —
  so a group renders its cards in top-to-bottom file order. Don't re-split
  these back into flat tabs.

### Single settings catalog (page + search + ⌘K)

The category list is **not** declared in `Settings.svelte` any more. It lives
in `frontend/src/lib/stores/settingsCatalog.ts` as `SETTINGS_CATEGORIES`, and
that one array feeds three consumers:

1. `routes/Settings.svelte` — the rail and the content header
   (`const categories = SETTINGS_CATEGORIES`).
2. `routes/Desktop.svelte` — the global ⌘K search index, which folds in
   `settingsSearchItems(translate)` so **every** category is searchable.
3. `requestSettingsCategory(key)` / `pendingSettingsCategory` — a one-shot
   store a search hit uses to open Settings already switched to its category
   (`Settings.svelte` consumes and clears it on mount).

Each entry carries `labelKey`, `subtitleKey`, `icon`, `accent` and a short
`keywords` list of English synonyms (`smtp`, `tls`, `ollama`, `uptime`, …).
Adding a category is therefore **one catalog entry + its i18n keys + the
`{#if activeCategory === …}` block** — the rail, the header and search all pick
it up with no second list to keep in sync. The catalog is a pure module (no
rune context) so it stays unit-testable; `settingsSearchItems` takes the
translate function as an argument instead of importing the i18n store.

### The 2026-09-14 shell

Three pieces, each with one job:

- **Rail item** — `WindowSidebarItem.svelte` with `variant="rich"`, `subtitle`
  and `accent`. The 28px **circular** icon tile (`--hv-settings-icon-tile`) is
  the category colour mixed to
  `--hv-settings-cat-tint` (9%) behind a 16px glyph in the full colour; when the item
  is active the tile goes **solid** with a **white** glyph and the row gains a
  trailing 4x20 pill (`::after` in `WindowSidebar.svelte:281`, coloured by
  `--hv-row-accent`). Both tile states are contrast-governed (≥3:1, WCAG
  1.4.11 — E2E 17.5 measures them live). `variant`/`accent`/`subtitle` are
  opt-in — the ~10 other `WindowSidebarItem` consumers are untouched.
- **Rail typography is the compact scale** (2026-09-15): the two-line row uses
  `--hv-text-sm` for the label and `--hv-text-xs` for the subtitle (both
  compact chrome — never the `--hv-font-size-*` content scale), the rail
  header micro-label is `--hv-text-xs` + `--hv-letter-spacing-wider` +
  `--hv-line-height-normal`, and the shared item/footer paddings + gaps in
  `WindowSidebar(.svelte)` resolve through the Tailwind spacing scale
  (`calc(var(--spacing) * …)`). Same three tokens drive `secLabel`/
  `secLabelFirst` in `vocabulary.ts`, so rail and section labels read alike.
- **Content header** — a non-scrolling sibling of the panel: the category
  title (`[data-testid="settings-header-title"]`) over the same subtitle the
  rail item shows (`[data-testid="settings-header-subtitle"]`), two lines and
  nothing else. The 36px tinted icon tile was **removed 2026-10-19** (the
  active rail item already carries the glyph), and the block went
  `pt-5 pb-4` → `px-6 py-2.5`: **measured 87.5px → 57px**. `PageLayout` has no
  header slot, so Settings renders its own.
- **Card headers are the same two lines** — `SettingsCard` lost its own icon
  tile in the same pass, and with it the `icon` / `iconVariant` / `accent`
  props: a card's identity is its title, and the per-card tint only ever
  coloured the glyph it no longer renders. `py-4` → `py-2.5`: **measured
  83.5px → 57px**.
- **One icon tile for the whole screen (2026-10-19)** — every decorative tile
  is a circle at `--hv-settings-icon-tile` (28px) with a 16px glyph, through
  `settingsIconTile` (`vocabulary.ts`) or the rail's rich CSS. The screen used
  to mix 28px tiles with a 12px corner and 36px tiles with an 18px corner and a
  20px glyph, so the Widgets rows, the Privacy rows and the rail read as three
  components. Card *headers* carry no tile at all any more (see above); the
  only other tile is the window-header icon, which is already a 28px circle.
  E2E 23.12 pins the size, the shape and the 16/20 glyph ladder across all
  eight categories.
- **The subtitle's `mb-0!` is as load-bearing as the `!` on the heading** —
  `styles/theme/base.css` declares an unlayered
  `p { margin-bottom: var(--hv-space-4) }`, so the plain `m-0` the header
  carried never applied and both header tiers silently kept **16px of dead
  space** under the subtitle (the content header measured 73px instead of 57
  until this was caught by measuring the rendered box, not by reading the
  classes — see **Invariants**).
- **Section + card + row** — `components/settings/vocabulary.ts` is the single
  source of the class strings, imported by both `Settings.svelte` and
  `SettingsCard.svelte`. A section label is a **sibling** immediately before its
  card (never inside it), which is what E2E 23.5 pins. `settingsCard` carries
  `shrink-0` — load-bearing, see **Invariants**.
  - `settingsSelect` fills its row; `settingsSelectCompact` (no `w-full`) hugs
    its text — the mock's shape for a control holding a short value, which is
    what the refresh-interval select uses. Both compose one `selectBase`, so
    the geometry is declared once.
  - The heading type utilities carry a trailing **`!`** — see **Invariants**.

`theme/Switch.svelte` is the screen's only toggle: a visually-hidden native
`<input type="checkbox">` inside `<label class="settings-toggle-label">` with a
40x22 track + 16px knob. It is deliberately **not** `theme/Toggle` (that one is
a bits-ui `role="switch"` *button*, with no `input` for `flipSwitch()` /
`isChecked()` to read). `accent` travels to CSS as `--hv-switch-accent`: every
row passes its **category** colour (`activeCat.accent` in `Settings.svelte`,
`SECURITY_ACCENT` in the security sub-cards), and a monitor row passes that
monitor's catalogue colour instead. `ariaLabel` names a bare track;
`ariaLabelledby` points at a visible `<span id=…>` in the same row when the
wording already exists there — no duplicated string.

> The TLS card was the screen's last hold-out (2026-09-14): its two switches
> were bits-ui `Toggle`s, i.e. the *brand* blue on a 32x18.4px track, which is
> neither the security category colour nor the design's geometry. They now use
> `Switch` + `SECURITY_ACCENT`, which also retired the dead `.hv-toggle-switch`
> rule in `components.css` (44x24/20px knob — the class E2E 12.3/12.4/12.5/12.6
> used to target, which is why those three were failing before the re-skin and
> why 12.6, which had never run, failed the first time after it; all four now
> drive `flipSwitch()`). `theme/Toggle` keeps its other call sites (Shares,
> Docker, Restic, Storage) — do not migrate *those* to `Switch`, they are not
> on this screen and have no `flipSwitch()` contract.

- `frontend/src/lib/services/api.ts`: `getAllSettings`, `getPublicSettings`,
  `getTimezones` (the picker's IANA list), `getGeneralSettings`/`updateGeneralSettings`, `getLanguage`/`setLanguage`,
  `getEmailSettings`/..., `getNotificationSettings`/..., `getAppearanceSettings`/...,
  `getUpdateSettings`/..., `checkForUpdates`, `exportSettings`/`importSettings`/
  `resetSettings`, `getSnmpSettings`/..., `getUpsList`/`getUpsStatus`/
  `getUpsAvailability`.
- AI model card on Settings (five hardware-ranked models + the MCP gateway card): see
  [AI Assistant](./chat.md) and [MCP server](./mcp.md). The same `ai`
  category also mounts `McpCard.svelte` (external agents / MCP gateway,
  admin-only, self-gating on `GET /api/mcp/status`) — see
  [External Agents](./mcp.md). Its kill-switch, permissions and tokens are
  immediate actions, not `draft` fields, so the rail-footer Save never
  applies to them.
- Security cards on Settings: `TlsCard.svelte`, `AntivirusCard.svelte`,
  `AntiRansomwareCard.svelte`, `FirewallCard.svelte` — see
  [HTTPS/TLS](./tls.md), [Antivirus](./antivirus.md),
  [Anti-Ransomware](./ransomware.md), [Firewall](./firewall.md).
  `TlsCard` (2026-09-09) configures the built-in HTTPS listener
  (`GET/PUT /api/settings/tls`, `POST /api/settings/tls/regenerate`); its
  editable fields are flushed by the screen-wide Save button, with the
  "restart required" banner surfaced from the store's `tls.saveResult` after a
  global save. `regenerate`/`restart` stay immediate actions.
- `UpsCard.svelte`/`SnmpCard.svelte` (`frontend/src/lib/components/settings/`)
  are wired into `Settings.svelte` and have real frontend UI — this is not an
  orphaned backend-only feature. They are the whole `monitoring` category now.
  - **SNMP** (`src/snmp.rs`): reads the local `snmpd` agent with the numeric
    `-On` `snmpwalk`/`snmpget` output and parses the OID columns directly
    (2026-09-15) — it must never match symbolic MIB names (`IF-MIB::ifDescr`),
    which require MIB files that aren't installed by default. The interface +
    storage sections need `snmpd`'s view to cover `.1.3.6.1.2.1.2` (ifTable)
    and `.1.3.6.1.2.1.25.2` (hrStorageTable); the Debian/Ubuntu default
    `systemonly` view does not, so `install.sh` drops
    `/etc/snmp/snmpd.conf.d/homevault.conf` to extend it. System info needs no
    extra view (it is inside `systemonly`).
  - **UPS** (`src/ups.rs`): calls `upsc -L`/`upsc <name>`, which require a
    running `upsd` with at least one UPS defined in `/etc/nut/ups.conf`. NUT
    ships with `MODE=none` (upsd refuses to start), so `install.sh` sets
    `MODE=standalone` and adds one generic auto-detect section —
    `driver = usbhid-ups`, `port = auto` — which attaches to whichever
    supported USB UPS is present (no per-model vendor/product id hardcoded;
    the driver's `Restart=always` retry loop picks up a UPS plugged in later).
    A configured-but-not-connected UPS is a normal state and is logged at
    `debug!`, not `warn!`.
  - **UPS device configuration** (`src/ups.rs` + `src/handlers/ups.rs`):
    a serial or network UPS cannot be guessed, so the card lets the admin add
    it as a dedicated `[section]` in `/etc/nut/ups.conf`:
    `GET /ups/config` lists the sections; `POST /ups/config` upserts one
    (`{name, driver, port, desc?}`); `DELETE /ups/config/{name}` removes one;
    both writes reload NUT. The driver is an **allow-list**
    (`ALLOWED_UPS_DRIVERS`: `usbhid-ups`, `blazer_ser`, `blazer_usb`,
    `nutdrv_qx`, `apcsmart`, `apcsmart-old`, `snmp-ups`, `bcmxcp`, `bestups`,
    `dummy-ups`) because NUT execs `/lib/nut/<driver>` — never a free-form
    command. Name is `[A-Za-z0-9_-]+` (it becomes `[name]`), the port/address
    rejects whitespace, control chars and `..`, and the whole section is
    round-tripped by the pure `parse_ups_conf`/`upsert_ups_section`/
    `remove_ups_section` functions (unit-tested). Config strings are pure;
    the file I/O goes through `write_file_as_root`. `tests/ups_integration.rs`
    pins the auth contract (no token/invalid → 401, non-admin → 403, admin →
    200); the admin write path is exercised live only, since it touches the
    real `/etc/nut/ups.conf` + systemd.
- ~~`QuotaCard.svelte`~~ — deleted 2026-09-09 with the per-group filesystem
  quota feature; see [Disk Quotas](./quotas.md).

## System → Database snapshots (2026-10-17)

The **System** category carries a "Database snapshots" card, below *Data
management*. It exists because the database runs in WAL mode: copying
`homevault.db` on its own yields a file with **no tables**, so the obvious
manual backup silently produces nothing restorable. The card surfaces:

- the last snapshot time (or "Never"),
- the schedule (`interval_hours`, including "manual only") and retention
  (`retain`, 1–100),
- every snapshot on disk with its size, newest first, and a *Before upgrade*
  badge on a `.pre-migration` restore point (those are outside `retain` and the
  pruning pass never removes them),
- **Snapshot now** and **Save schedule** buttons.

State lives in `Settings.svelte` (`dbBackup`, `dbBackupInterval`,
`dbBackupRetain`, `dbBackupBusy`, `dbBackupError`), loaded in `onMount` through
`loadDbBackup()`; `handleBackupNow()` and `handleSaveDbBackup()` call the two
mutating endpoints. A failure of *this screen's* call renders in a `role="alert"`
row (`data-testid="db-backup-error"`) instead of being swallowed, and a failure
of a *scheduled* attempt — which the screen never sees happen — is served in
`last_error` and rendered by `data-testid="db-backup-last-error"` until the next
success clears it (2026-10-24, ROADMAP 22.2). See
[backup-restic.md](./backup-restic.md) → *HomeVault's own database* for the
mechanics, the process-wide serialisation and the restore procedure.

## Widgets category — real monitors + refresh interval

`widgets` is not a mock: the 11 rows come from `stores/widgetCatalog.ts`
(`WIDGET_DEFS`, the same catalog `routes/Widgets.svelte` renders as tiles) and
each row's `Switch` reads and writes `monitorStore`. A row carries
`settings-widget-row` + `data-widget="<id>"` (the hooks E2E 23.6/23.7/23.10
use) — see **Invariants** for why the class is not called
`settings-monitor-row`.

- **Visibility** — the Settings rows bind to `draft.widgets.visible`, and the
  `saveAll()` widgets saver applies them through the matching
  `monitorStore.set<X>Visible`. The store pushes to `POST /monitor-layout` on
  an **800ms debounce** (`monitorStore.ts::scheduleServerPush`) — that push is
  the save, so it fires only after the user clicks the Settings Save button.
- **Refresh interval** — `refreshMs` lives **inside the same
  `/api/monitor-layout` JSON blob** as the widget positions, not in a column.
  The backend treats `layout_json` as an opaque string
  (`handlers/monitor_layout.rs`), so this needed **no migration and no schema
  change**. Options are `REFRESH_INTERVAL_OPTIONS` = 2 / 5 / 10 / 30 / 60 s,
  defaulting to `DEFAULT_REFRESH_MS` (5000). It is staged in
  `draft.widgets.refreshMs` and applied on Save.
  - `loadFromStorage` whitelists it explicitly — that path does not spread, so
    a new key must be added there or it is silently dropped.
  - `syncFromServer` spreads, and `normalizeRefreshMs` coerces anything
    unusable back to the default, so a blob written by an older frontend keeps
    working.
  - `Desktop.svelte` reads `$monitorStore.refreshMs` for the four resource
    pollers (cpu / ram / temp / network) via `startResourcePollers(ms)`; a
    change re-subscribes them (`subscribeWidget` is reference-counted and keyed
    `` `${widgetId}:${intervalMs}` ``). The long cadences (Docker catalog,
    notifications) stay on their own fixed intervals — they are not "widget
    refresh".

## Appearance — one appliance look (ROADMAP 26, 2026-10-24)

**Decision (option A of ROADMAP 26).** The look — **wallpaper + accent colour** —
is a *single, appliance-wide* value an admin saves from Settings → Appearance.
It is **readable by any signed-in session** and **writable by an admin only**.
Appearance is deliberately *not* per-account: "the look of the appliance" stays
an admin decision, and the data model (one global `APPEARANCE_SETTINGS`) already
said so.

### Who reads what

| Caller | Call | Result |
|---|---|---|
| any session (boot hydration, `App.svelte`) | `GET /settings/appearance` | `200` + `{ wallpaper, accent_color }` |
| admin (Settings window) | `GET /settings` (aggregate) | `200` + every family, `appearance` inside |
| non-admin (Settings window) | `GET /settings` (aggregate) | `403 "Admin privileges required for this operation"` (a permission refusal, not a malformed request — ROADMAP X.7) |
| admin | `PUT /settings/appearance` | `200`, validated |
| non-admin | `PUT /settings/appearance` | `403 "Admin privileges required for this operation"`, nothing mutated |

Before 2026-10-24 the hydration called the admin-only aggregate, so for a
non-admin it failed, the `catch` swallowed it, and the desktop kept that
origin's `localStorage` copy: two origins, two wallpapers, no explanation. It
now calls the appearance endpoint; a failure is **reported once**
(`toast.error(settings.appearance_load_failed)`) and the desktop keeps the last
known value, which is now an explained fallback instead of a silent one.

The read endpoint carries **only** those two fields — no e-mail, SMTP, webhook,
update or notification setting — so making it session-readable adds no leak
(asserted in `tests/settings_core_integration.rs`,
`test_appearance_is_readable_by_a_non_admin_session`).

### Validation (`PUT /settings/appearance`)

Both fields are validated **before** anything is stored, so a refusal never
leaves the blob half-written:

- `wallpaper` — `""` (never picked), `/wallpapers/hv-NN.webp`, `hv-NN`, a bare
  JSON string, or the `WallpaperConfig` JSON the store round-trips
  (`{"type":"image","value":…}`; the legacy `{"type":"default"}` maps to
  `hv-01`). Everything is normalised to the **bare** `/wallpapers/hv-NN.webp`
  URL. `hv-00`, `hv-45`, a custom upload, a data URL, a gradient, a
  non-bundled host — all refused with `400` and a message starting
  `wallpaper:` that names the allowed set (`validate_wallpaper_field`).
- `accent_color` — `#rgb`, `#rrggbb` or `#rrggbbaa` only
  (`validate_accent_color_field`). The value is written into the
  `--hv-primary` custom property in the browser, so anything that could close
  the declaration is refused. `#4a90d9` is still rewritten to HomeVault Blue
  (`normalize_accent`).

The same wallpaper rule runs on load (`apply_loaded_settings`): a stored value
that does not name a bundled photo — a legacy custom upload, a hand-edited
`settings.json` — is dropped to `""` instead of being handed to a desktop that
would silently reset it.

### `theme` and `density` are gone

Both fields were persisted and round-tripped but consumed by **nothing**
(`grep -rn "data-theme\|data-density" frontend/src` → 0 hits; no dark theme
exists in the tree). They were removed from `AppearanceSettings`, from both
appearance handlers, from the aggregate, from the draft store and from the
i18n catalog — a user can no longer save a theme that changes nothing. The
persisted blob stays backward compatible: `PersistedAppearance` still *reads*
the two keys (so an older `data/settings.json` loads), but never writes them,
and the reader ignores them. An old client that still sends them is accepted
(serde ignores unknown fields) and they are not echoed back.

## Save button (no auto-save)

Since 2026-09-15 the Settings window has **one Save button**, in the rail
footer, that flushes every category. There is no auto-save anywhere on the
screen:

- `stores/settingsDraft.svelte.ts` is the single draft of every editable value
  (the main groups + the editable fields of the TLS / Antivirus /
  Anti-Ransomware / SNMP sub-cards + autostart + AI model + widget layout +
  weather prefs). Every category binds to `draft.*`; changing a field only
  marks its group dirty (a JSON snapshot per group compared against the
  baseline from the last load/save).
- `saveAll()` (wired to `[data-testid="settings-save"]` in the sidebar footer)
  flushes every dirty group sequentially through its own PUT/POST and advances
  the baseline only on success.
- **The button is the save status** (2026-09-23): one element, one message.
  Its colour *and* label are the state machine — the button carries the state
  as `data-state`, and the label always changes with the colour, so nothing on
  this screen is colour-alone (WCAG 1.4.1):

  | `data-state` | fill | label (`settings.*`) |
  | --- | --- | --- |
  | `clean` | blue `--hv-primary` | `save` |
  | `dirty` | red `--hv-error` | `save_changes` |
  | `saving` | blue, disabled | `autosave_saving` |
  | `saved` | green `--hv-success` | `autosave_saved` — held 2.5 s, then `clean` |
  | `error` | red `--hv-error` | `save_error` |

  White on the three fills measures 6.15:1 (blue), 5.73:1 (red) and
  5.14:1 (green) ✓ for §1.4.3. The blue is the live `--hv-primary` accent,
  which `App.svelte` re-hydrates from `appearance.accent_color` at boot —
  since 2026-09-23 that default is `#1E62AE` (HomeVault Blue, i.e. the
  token itself: 6.15:1 behind white, 4.99–5.68:1 as text on
  `--hv-surface` and the `--hv-primary-*` tints). The retired default
  `#4a90d9` failed both directions (≤3.34:1) and is rewritten on every
  path in by `handlers::settings::normalize_accent` (shipped default,
  persisted load, import, `PUT /settings/appearance`); the accent has no
  editing UI, so no operator choice is overridden.
  An edit staged during the green flash wins (it clears the flash), and a
  failed group parks the button on `error` — it never claims "Saved". The `[data-testid="settings-save-status"]` line kept from
  the old status pill is now **sr-only** (`role="status"`): screen readers hear
  every state change while the rail footer renders exactly one element. The
  visible pill above the button and the "N unsaved change(s)" caption under it
  were removed the same day. The block sits in the rail footer — never in the
  window header, whose bar carries the icon + title only (2026-09-17).
- The sub-cards no longer keep their own Save buttons — their editable fields
  live in the same draft, so edits survive switching categories (a sub-card
  unmounts when its group leaves the panel, but the draft lives in the store).
  Their *actions* (scan now, resolve threat, add/remove path, regenerate cert,
  firewall enable/disable/rules, test email, check updates) remain immediate.
- The **language** select routes through `api.setLanguage` +
  `languageStore.setLanguage` inside the general saver, only when it changed.
- The **AI model** select stages `draft.ai.model`; Save activates it via
  `chatSetModel`. Installing/pulling a model is still an immediate action.
- The **autostart** toggle stages `draft.autostart.enabled`; Save calls
  `api.setAutostart`.
- The **mDNS** toggle stages `draft.general.mdns_enabled`; Save sends it in
  the General `PUT` (ROADMAP 29.1).
- **Widgets / weather** stage `draft.widgets.*` and `draft.weather.*`; Save
  applies them through `monitorStore` / `weatherLocationStore` /
  `weatherDisplayPrefsStore` (their own persistence paths are the save). The
  reactive `syncWidgetsFromMonitor` / `syncWeatherFromStores` mirror the live
  stores into the draft — but no-op while the group is dirty, so a late server
  sync never clobbers staged edits.

## Autostart card (System)

The **Start on boot** toggle drives `systemctl enable|disable homevault`
through `src/autostart.rs`. It is admin-only, and the sudoers grant is scoped
to exactly `systemctl enable homevault` / `systemctl disable homevault` —
never the bare `systemctl` (see [power.md](power.md)).

**Both host calls are bounded and off the async runtime** (ROADMAP 25.1):
`is_enabled` and `set_enabled` run their `std::process::Command` inside
`spawn_blocking` with a `run_timed` deadline (`AUTOSTART_PROBE_TIMEOUT` 10 s,
`AUTOSTART_UPDATE_TIMEOUT` 30 s). `sudo systemctl enable` can block for
seconds on the sudo PAM stack or on a systemd job ordering; parking a Tokio
worker on it would also park the whole settings save.

**“The host cannot answer” is not “disabled”** (ROADMAP 25.2). The probe is
tri-state:

| Host answer | `is_enabled()` | `GET /settings/autostart` | Card |
|---|---|---|---|
| exit 0 | `Ok(true)` | `200 {enabled:true}` | switch on |
| exit 1, clean stderr (`disabled`, `masked`, `static`, …) | `Ok(false)` | `200 {enabled:false}` | switch off |
| exit 1, non-empty stderr (container, no systemd, unit missing) | `Err(reason)` | `500 {success:false, message:<reason>}` | switch **disabled**, `role="alert"` naming the reason |

Before this, the `unwrap_or(false)` collapsed the third row into “off”, so an
admin on a non-systemd host saw a switch that lied. A failed **read** disables
the switch until a later read succeeds (`autostartUnavailable`), so it cannot
stage a change against an unknown baseline.

**A refusal shows the host's reason and changes nothing.** `set_enabled`
returns the stderr tail (`polkit refused: no authorization`, `sudo: a password
is required`, `Failed to get unit file state…`); the handler relays it as the
`500` body's `message`, and the card renders it as an error `Alert`
(`role="alert"`, dismissible, `data-testid="autostart-error"`) wrapped in the
localised sentence `settings.autostart_error`. The staged flip is reverted to
the last server-accepted value (`baselines['autostart']`) so the switch never
shows a state the host refused; the rail-footer Save button parks on
`data-state="error"`.

## Invariants

- `/settings/public` is the only settings endpoint callable without a session
  (used by the login page to know server name/language/version). It carries
  **no appearance**: `GET /settings/appearance` is session-readable but still
  requires a session (ROADMAP 26.1).
- Appearance is one global value: any session reads it, only an admin writes
  it, and both writers and the load path validate it (see *Appearance* above).
- **The update check is opt-in and enforced server-side** (`CLAUDE.md` →
  Compliance, point 1). `is_check_due` (`handlers/settings.rs`) is the single
  gate: `Manual`, `Disabled` and an absent preference never contact the update
  endpoint, an opted-in schedule contacts it once its interval has elapsed, and
  only `POST /settings/version/check` (admin) forces a check. The mount-time
  `GET /settings/version` serves the cached answer, so opening the screen is not
  a third-party call. A failed attempt still stamps the interval (otherwise a
  blocked GitHub would be retried on every open). Guards:
  `settings_core_integration.rs::test_version_info_never_contacts_the_update_endpoint_by_default`
  / `…_when_disabled` / `test_check_now_is_the_only_forced_outbound_path` /
  `test_a_due_schedule_checks_and_a_fresh_one_does_not` /
  `test_check_now_requires_admin`, unit `handlers::settings::tests::test_no_scheduled_check_without_an_opted_in_frequency`,
  E2E 8.3/8.4. `update_check_attempts()` exposes the attempt counter so "zero
  outbound attempts" is measured, not assumed; `HOMEVAULT_VERSION_CHECK_URL`
  redirects the check (tests, offline/mirrored installs).
- One Save button for the whole screen — see **Save button** above; the
  initial load must never PUT/POST. (The sub-cards' *action* buttons — a UFW
  enable, a cert swap, a ClamAV quarantine delete — are immediate by design
  and unrelated to parameter persistence.)
- Appearance changes update `frontend/src/lib/stores/settingsDraft.svelte.ts`
  (draft) / `wallpaper.ts` live; `--hv-primary` re-hydrates from the saved accent at boot.
- Wallpaper-tile selection state uses the reactive `$: selectedWpUrl` (not a
  helper fn) so `class:is-selected`, `aria-checked`, and the check badge all
  update on click — see `CLAUDE.md` Settings section.
- The wallpaper-picker portal target is `#app`, not `document.body`;
  `/wallpapers/{*path}` must stay registered before the SPA fallback.
- Language changes reload the translation map via `/translations` (mounted at
  `/api/translations`).
- `[data-testid="settings-content"]` is a column flexbox that scrolls
  (`overflow-y:auto`, `min-height:0`, `flex:1`). Every card is a **direct
  child** (E2E 23.4) and must keep `flex-shrink:0` — a scrolling flexbox is
  what makes the automatic `min-height:auto` resolve to zero, so without it the
  column squeezes tall cards and their content is clipped instead of the panel
  scrolling. `vocabulary.ts::settingsCard` carries the `shrink-0`.
- One toggle geometry screen-wide: `theme/Switch.svelte` only, in every
  category **including the sub-cards**. Do not reintroduce a second
  hand-rolled switch, and do not put a `theme/Toggle` back on this screen —
  it is a different size and a different colour (E2E 23.9 fails on a stray
  track; 23.10 on an accent-less one).
- Every `var(--…)` on this screen is a `--hv-settings-*` token or an Appliance
  token; the category accents are the eight `--hv-settings-cat-*` solids, and a
  tint is always derived from its solid via `--hv-settings-cat-tint` so the two
  cannot drift. Every accent clears ≥3:1 in both tile states — 2026-09-23:
  widgets `#A78BFA`→`#7C3AED`, notifications `#F59E0B`→`#B45309`, ai
  `#06B6D4`→`#0E7490`, monitoring `#10B981`→`#047857` (the violet-400 /
  amber-500 / cyan-500 / emerald-500 stops sat at 2.00–2.72:1); guarded by
  E2E 17.5.
- **2026-09-26 AA/standardness pass** (Settings UI audit): the AI
  requirement chips take their text from `--hv-settings-chip-text`
  (`#0E7490` on `--hv-settings-hairline` = 4.89:1; the ramp's
  `--hv-settings-label` only reached 4.34:1 there); `vocabulary.ts` focuses
  inputs/selects/textareas with `focus:border-[var(--hv-primary)]` like
  every other screen (it was `--hv-settings-cat-general`, so mouse-click
  and keyboard focus painted two different blues); and icons on this screen
  take only the scale's **16 or 20** step (`size={15}` was never on any
  grid — see `design-system.md` §6.3).
- **The heading type utilities carry a trailing `!`** (`text-md!`,
  `font-bold!`, `leading-5!`, `text-[var(--hv-settings-title)]!` on the
  content header `<h2>` and every `SettingsCard` title; `text-sm!` +
  `font-semibold!` + `leading-5!` on `SnmpCard`'s `<h3>`s) — this is
  not decoration. `styles/theme/base.css:54-67` declares a bare
  `h1..h6 { font-weight: …; line-height: …; color: … }` plus per-level
  `font-size` **outside any `@layer`**, and an unlayered rule outranks every
  Tailwind v4 utility (Tailwind emits its utilities inside `@layer utilities`)
  *regardless of the utility's class specificity*. Without the flag these
  headings render at the base `h2` size — 28px/600/#1F2733 instead of the
  design's 16px/700/24px/#1D293D — which is precisely what happened until
  2026-09-14, and no E2E assertion covered it (it was caught by measuring the
  computed styles live). Removing a `!` here silently un-styles the heading
  again; the same trap applies to any `<h1>`–`<h6>` given type utilities.
- **The same unlayered-rule trap applies to `p` margins** (`mb-0!` on the
  content header's subtitle and on `.settings-card-subtitle`).
  `base.css` declares `p { margin-bottom: var(--hv-space-4) }` outside any
  `@layer` too, so the `m-0` both headers used to carry never applied: each
  header kept **16px of dead height** under its description. Measured
  2026-10-19 — content header 73px instead of 57px, card header 83.5px instead
  of 57px — and nothing in the markup showed it; only measuring the rendered
  box did. Any `m-0` / `mt-0` you add to a `<p>` on this screen needs the `!`.
- **The form-element defaults are in `@layer base`** — `base.css`'s
  `button { background:none; padding:0 }` and `input/select/textarea`
  (`width:100%`, 1px border, `--hv-input-padding`) were **unlayered**, so they
  beat every layered utility on the screen: the sidebar-footer Save button
  (`settingsBtnPrimary` → `bg-[var(--hv-primary)] px-4 …`) rendered as white
  text on a transparent background (no fill, no padding — visually missing),
  the ghost buttons lost their border/fill, `settingsInput` showed the 1px /
  16px defaults instead of the 1.45px / `px-3` vocabulary, and the compact
  refresh select ignored `w-auto` and stretched to the row's full width.
  `@layer base` (2026-09-15) puts the defaults below the utilities layer, so
  the Settings button/select/input utilities win again. The `h1..h6` rules are
  **still** unlayered — the heading `!` above remains load-bearing. This is
  the same trap the universal reset already documents at the top of
  `base.css`: keep element defaults inside `@layer base`, never unlayered.
  2026-09-26 extended that to the generic `*:focus` / `*:focus-visible`
  pair: unlayered, its `border-radius: var(--hv-radius-sm)` snapped every
  `rounded-lg` input from 10px to 4px the moment it took keyboard focus;
  in `@layer base` the `rounded-*` utilities win again while the element
  focus rules (`button/input/a/[role]`) stay unlayered and authoritative.
- `settingsSelect` (`w-full`) fills its row; `settingsSelectCompact` (`w-auto`)
  hugs its text. The refresh-interval select is the compact one — the mock
  draws it 76px wide, and a `w-full` select rendered 280px. The two compose one
  `selectBase`, so the geometry lives in a single declaration rather than in
  two width classes whose order would decide the winner.
- `refreshMs` is written into the `monitor-layout` blob. If that read path is
  ever refactored from an explicit whitelist to a spread, re-check
  `loadFromStorage` — a spread there would silently drop device-only keys.
- The widgets rows are `settings-widget-row` and **must never** be renamed to
  anything containing `-monitor`. Every E2E spec's `gotoDesktop()` injects
  `[class*="-monitor"]{display:none!important}` to hide the floating desktop
  widgets, and that is a *substring* match: a class of ours containing
  `-monitor` gets hidden too. That is exactly what made the 11 rows 0x0
  (invisible label, 0px track) in E2E 23.7/23.9 on 2026-09-14 while the source
  CSS was perfectly correct — a failure that costs an afternoon to find, so
  the constraint is recorded here and on the injected rule itself.

## Related

- [Themes](./themes.md), [i18n](../I18N.md), [Security](./security.md),
  [AI Assistant](./chat.md), [Notifications](./notifications.md),
  [Antivirus](./antivirus.md), [Anti-Ransomware](./ransomware.md),
  [Firewall](./firewall.md), [Widgets](./widgets.md),
  [Design system](./design-system.md)

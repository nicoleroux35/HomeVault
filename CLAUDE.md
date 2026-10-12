# CLAUDE.md

Operational guardrails for working in this repo. **Reference documentation
lives in `docs/`** and is the canonical source of truth for *how things work*
(architecture, the per-endpoint API contract, one guide per app/feature). This
file carries only the **rules** and the **engineering invariants** — what not to
touch, and what is easy to break again.

When `docs/` and this file disagree on a fact, fix whichever is stale. When a
section here and a guide disagree on a *rule*, this file wins.

---

# Rules

## Keep the repository clean — no scratch/reference files in commits

- Before a broad `git add`, run `git status` and check every new path — don't
  add something you can't explain the purpose of.
- Before committing a new file/dir, confirm it's referenced by the build or is
  genuinely useful docs/tooling (`grep -r` for it). Never commit design
  exports, canvases, debug logs, screenshots, or pasted terminal output just
  because they're in the working tree.
- Session-scoped verification/audit findings go into `docs/` (the relevant
  guide) or, if one touches an engineering invariant, into a report to the
  user — never a standalone root `.md`.
- New AI-tool cache/scratch dirs go in `.gitignore` (`.cursor/`, `.devin/`,
  `.kombai/`, `.playwright-mcp/`, `.opencode/`, `.ds-sync/`, … are already
  there). Ad-hoc screenshots at the repo root are gitignored via `/*.png`;
  delete them, don't commit them.

## Every feature ships with its tests

Every new feature, screen, or significant change **must** include complete
tests before it's considered done. All three categories below apply where
relevant — a backend-only change still needs logic/unit + integration
coverage; a frontend-only change still needs E2E coverage.

### Test file layout

- `tests/<module>_integration.rs` — Rust integration tests (Cargo convention).
- `tests/e2e/<screen-name>/<screen-name>.spec.ts` — Playwright E2E (dir name =
  screen name, lowercase; one `.spec.ts` per screen covering **all** its
  functionality).
- Logic/unit tests live in a `#[cfg(test)] mod tests { … }` block at the bottom
  of the production file they test, not under `tests/`.
- `playwright.config.ts` at the project root, `testDir: './tests/e2e'`.

### 1. Logic/unit tests (`#[cfg(test)]`)

Pure logic only — no DB, HTTP, filesystem (except `tempfile`), no
`sudo`/system commands. Deterministic (fixed timestamps, no randomness).
Cover: validation functions, edge cases, error paths, reserved names,
format/parsing (a number- or locale-formatting function needs a test for
*each* input shape — a mismatched cast order or an unhandled decimal separator
is exactly what this category exists to catch before it reaches a live host).
Naming: `test_<function>_<scenario>`. Run: `cargo test <module>::tests`.

### 2. Integration tests (`tests/*_integration.rs`)

Every DB-touching handler module. Fresh **in-memory SQLite** (`":memory:"`)
per test, all embedded migrations run on it. Build an Axum `Router` with the
handlers under test, drive it with `tower::ServiceExt::oneshot` — no real
server. Test the full lifecycle: JSON body → handler → DB → JSON response.
Cover: auth guards (401/403), validation (400), success (200), idempotence,
error responses. **Not this category's job** (push to E2E): UI/CSS/responsive,
WebSocket/PTY, anything that shells out to a real system command (`sudo`,
`mdadm`, `zpool`, …). Naming: `test_<endpoint>_<scenario>`. Run:
`cargo test --test <module>_integration`.

### 3. End-to-end (Playwright, `tests/e2e/`)

Each `.spec.ts` has sections for: (1) page load, (2) form fields,
(3) validation, (4) all interactions, (5) submission flows, (6) navigation,
(7) responsive (≥ 360 / 768 / 1920 px), (8) accessibility, (9) performance,
(10) **security** (mandatory — no sensitive data in URL/localStorage, XSS not
reflected as markup), (11) keyboard, (12) console & network, (13) edge cases,
(14) memory regression.

Rules: every test standalone (navigates, detects state, adapts — no
`test.skip()` unless genuinely inapplicable); no shared mutable state
(`npx playwright test -g "<name>"` must work in isolation); **real backend
only**, never mocks; memory/CPU logged in `afterEach` with a warning above
10 MB growth; detect all three initial states (login / setup wizard /
connection error). This is also the category for anything a unit/integration
test can't reach: real system commands, WebSocket/PTY, real browser/CSS.

```bash
npx playwright test tests/e2e/login/login.spec.ts            # a screen (Firefox default)
npx playwright test tests/e2e/login/login.spec.ts -g "4.1"   # one test
npx playwright test tests/e2e/login/login.spec.ts --project=chromium
BASE_URL=http://localhost:3000 npx playwright test …          # custom server
npx playwright install firefox                                # if missing
```

When starting a scratch server for tests, **always pass a scratch `PORT=`**
alongside `DATA_DIRECTORY=` or you silently hit a real instance on 8080.

**Seeded E2E admin:** the browser suites log in as `e2eadmin` / `E2eSecret123`
(override with `E2E_ADMIN_USER` / `E2E_ADMIN_PASS`). This account is seeded in
the dev instance; `DELETE /api/users/e2eadmin` removes it. Suites that snapshot
mutable state (`settings.spec.ts`, `widgets.spec.ts`) save it in `beforeAll`
and restore in `afterAll`.

## Keep TESTING.md in sync

`TESTING.md` cross-references every screen in `SCREENS.md` against four test
levels (manual audit, Rust unit, Rust integration, Playwright E2E). Update it
**in the same change** that moves the ground it tracks:

- After writing/running tests: flip the relevant `[ ]` → `[x]`, re-count with
  the `grep -c` commands in the file's "Verification method" section (never
  estimate — a stale count reads as verified when it isn't), update the
  summary counts at the top.
- After a manual/live audit: update that screen's "Manual audit" date and fold
  the finding into "Remaining to test as a priority" if it changes the risk picture.
- A screen in `SCREENS.md` with no `TESTING.md` row (or vice-versa): fix it.
- Applies whether the test work happened this session or was reported as
  already done — `TESTING.md` must reflect the tree as it stands *after* the
  change.

## Keep docs/ in sync — always current, always English

`docs/` is the living developer reference: `INDEX.md` (entry point),
`ARCHITECTURE.md`, `API.md`, `BACKEND.md`, `FRONTEND.md`, `I18N.md`, and one
`guides/<area>.md` per app/feature. It is only useful while it's true — treat
it like `TESTING.md`, not a one-off writeup:

- **Update the relevant doc(s) in the same change** that adds/removes an
  endpoint, a module, a store, a migration, a frozen file, a design token
  rule, an invariant, or a feature. New feature → its guide gets a section and
  `INDEX.md` gets a row; removed feature → its guide row goes.
- **English only.** The product UI defaults to French; every word of `docs/`
  (and every code comment) is English.
- Each guide keeps the shape in `INDEX.md` → "Doc conventions": one-line
  purpose, overview, endpoints/components with `path:line` refs,
  request/response shapes, invariants, links to related docs. Endpoint paths
  are written without the `/api` prefix.
- Counts and file lists in `docs/` are produced by actually grepping the tree,
  not from memory.
- `docs/design/` holds legacy design-reference material — it carries its own
  "LEGACY" banner and is **not** authoritative; `docs/FRONTEND.md` +
  `tokens.css` are.
- This applies whether the work happened this session or was reported as
  already done.

## RAG knowledge base — always indexed, always current

The RAG system (`src/rag.rs`, `src/rag_indexers.rs`, `src/handlers/rag.rs`) feeds
the Chat AI with relevant context. It must stay healthy and documented:

- **When adding a new feature or app catalog entry**, also add a corresponding
  `FeatureDoc` entry in `src/rag_indexers.rs::feature_docs()` and update
  `docs/guides/rag.md`'s "Document Sources" table. This is **enforced**:
  `tests::test_every_guide_has_a_feature_doc` enumerates `docs/guides/*.md` and
  fails when a guide has no entry — a new guide means a new `FEATURE_GUIDES`
  row (id → filename) and a new `FeatureDoc`, or the build breaks. The mapping
  is not 1:1 (`backup` ← `backup-restic.md`, `virtual-machines` ← `vm.md`,
  `setup` ← `setup-wizard.md`, `cloudsync` ← `cloud-sync.md`). A guide about how
  the codebase is built (`design-system.md`) belongs in `DEVELOPER_ONLY_GUIDES`
  instead of the index. Entries describe the product to a user — no file paths,
  no handler names (`test_feature_docs_are_unique_and_substantive` rejects
  `src/`, `.svelte` and backticks).
- **When changing the embedding model** (currently `nomic-embed-text`), update
  `src/rag.rs` (`EMBEDDING_MODEL`), `docs/guides/rag.md`, and the i18n key
  `chat.rag_embedding_model` in all 4 locale files.
- **When changing the text of a shipped feature**, check its `FeatureDoc`
  entry too: the assistant answers from those entries, and they go stale
  silently. (The 2026-10-12 pass also refreshed the Users entry, which still
  described account deletion without the share-permission revoke.)
- **When adding a new document source** (e.g. user manuals, system metrics),
  add a new indexer function in `src/rag_indexers.rs`, register it in
  `index_all()`, add an endpoint if needed, and update the guide.
- **Migration 033** creates the RAG tables. Any schema change needs a new
  migration (latest is 036 — see `sqlx/migrations/`) and the migration count
  must be updated in `docs/BACKEND.md`.
- `docs/guides/rag.md` is **not frozen** — update it in the same change that
  modifies RAG behavior.

## Compliance & security baseline — non-negotiable

HomeVault must satisfy, at all times: **GDPR/RGPD** (Reg. (EU) 2016/679) and
**Privacy by Design & Default** (Art. 25; ISO/IEC 31700); **ISO/IEC 29100**
(the 11 privacy principles); **ISO/IEC 27701** (PIMS — every feature touching
personal data needs a documented purpose, lawful basis, retention period,
controller/processor split — HomeVault is the software, the operator is the
controller, so design so they *can* comply); **ISO/IEC 27001 & 27002** (ISMS
+ Annex A controls); **NIST** (CSF 2.0, SP 800-53 families where they add
rigour, the Privacy Framework, SSDF SP 800-218 for the build); **OWASP** (ASVS
as the verification bar, Top 10 as the never-list, Proactive Controls / Cheat
Sheets as the how-to — every endpoint and form reviewed against them).

**How this binds day-to-day work — check every change:**

1. **Data minimisation first.** No field, log line, metric, telemetry ping, or
   stored value unless a named feature needs it. Transient value → don't
   persist. Prefer local/on-device — any outbound third-party call is a
   privacy decision, must be justified, documented in `docs/`, and off or
   consented by default (cf. the Weather monitor + the connection-popover
   geo-IP lookup, both deliberately client-side and non-essential).
2. **Purpose & retention.** New personal-data storage (accounts, sessions,
   event log, quotas, notifications, recovery codes, shares/ACLs, backup
   metadata) needs a one-line purpose, a lawful basis, and a
   retention/rotation story (auto-expiry, a cleanup scheduler, or an explicit
   admin action). The journal cleanup scheduler + session `expires_at` are the
   pattern.
3. **Secure by default.** Ship the locked-down setting: 2FA available,
   sessions expiring, TLS assumed, firewall on-path, least privilege on every
   handler (`require_auth` reads, `require_admin` mutations — the split must
   never weaken), nothing exposing data before the admin opts in.
4. **Secrets never leak.** No secret, passphrase, recovery code, session token
   or password — plaintext or reversibly encoded — in logs, errors,
   URLs/query strings, exports, client storage, or the event log. Redact at
   the boundary (`SettingsPersistence::export()` smtp_password is the
   pattern). Hash with Argon2; store only what verifies.
5. **Input/output discipline (OWASP).** Validate + normalise every input
   server-side; parameterised SQL only; encode on output; CSRF middleware
   stays on; path traversal, SSRF, injection and auth-bypass checked on every
   new endpoint. The frozen sections' auth-bypass fixes are the cautionary
   tale.
6. **Transparency & data-subject rights.** UI copy says what is collected and
   why (`[settings] privacy_*`). Deleting a user must actually purge their
   personal data — home dir, SFTP jail, sessions, quota rows, PTY sessions:
   the `handle_user_delete` cleanup chain is the reference; extend it, never
   regress it.
7. **Auditability.** Security- and privacy-relevant events (auth,
   role/permission changes, data export, deletion, config changes) recorded in
   the application event log in plain language — the log itself minimised and
   on a retention timer (it is personal data too).
8. **Verification.** Every feature's tests include the negative cases:
   unauthorised → 401/403, validation → 400, no sensitive data in
   responses/URLs/storage, XSS payloads not reflected as markup. The E2E
   "Security" section is mandatory.

When a change can't meet this baseline, **stop and flag it** — don't ship a
gap. Record any deliberate signed-off exception as a dated note in the
relevant guide, never silently.

A 2026-09-07 baseline audit found 11 gaps (G1–G11). G1 (event-log retention
default on/90d), G2 (`UpdateFrequency` default `Manual` — no automatic GitHub
call), G5 (security-headers middleware wired as the outermost layer), G7
(`/api/progress/*` now `require_auth`), G8 (`.github/workflows/ci.yml`), G9
(`PRIVACY.md`), G11 (dead plaintext-HTTP `get_location()`) are **closed**.
G11 was closed on 2026-09-22 by removing `get_location()` entirely — the
server no longer geo-locates itself, `/server/info` only reports IP/port/
display address, and the only remaining location lookup is the browser-side
`ipwho.is` connection popover (documented in `PRIVACY.md`). G6 (built-in TLS) is **closed** as of
2026-09-09: HomeVault can terminate HTTPS itself — an optional second listener
on `HTTPS_PORT` (default 8443) with a self-generated self-signed certificate or
an admin-supplied PEM pair, plus an HTTP→HTTPS redirect toggle. Opt-in
(`TLS_ENABLED` / Settings > Security > HTTPS/TLS); an ACME/Let's Encrypt mode is
a possible later phase. See `docs/guides/tls.md` + the **TLS / HTTPS** section
below. G3 (server-side third-party calls not individually consented), G4
(`login_attempts` pruning only at startup), G10 (no self-service data export)
are **deferred** — details and follow-ups in `PRIVACY.md`.

---

# Deployment model — runs as root (2026-09-04, commit `2d5ba40`)

No dedicated `homevault` service account. The systemd unit `install.sh` writes
has no `User=`/`Group=` (systemd → root); `install.sh` only creates
`/home/homevault` as a home-folder root and the first admin's real Linux
account at setup. Full rationale + consequences: **`docs/ARCHITECTURE.md` →
Deployment model**.

Two things that bind this codebase:

- **`SetupService::configure_sudoers()` is dead code** — no sudoers file is
  generated. Every "the `configure_sudoers` grant must not revert" note that
  used to be in the frozen sections is **historical**; nothing reads that list.
- **The `sudo` wrappers on the individual commands stay.** `mdadm`,
  `zfs`/`zpool`, `cryptsetup`, `setquota`, `useradd`/`groupadd`,
  `hostnamectl`, `journalctl`/`tail`, `ufw`, `smartctl`, `wipefs`, `mount`,
  `setfacl`, `tee`, … all still shell out via `sudo` and succeed against the
  distro-default `root ALL=(ALL:ALL) ALL`. They keep a future non-root mode
  possible — don't strip them. `/etc/*` file writes go through a
  `write_file_as_root` / `sudo tee` helper (bare `fs::write` silently EACCESed
  for the old non-root account and would again).

`homevault-sftp` (the SFTP jail group/service) and `homevault_admin` (the OS
admin group) are unrelated to the dropped account and still exist.

`installer/install.sh` is the **only installer file** — self-contained by
design, since a `curl | sudo bash` install has no checkout on disk. The retired
`installer/homevault.service`, `installer/setup-automount.sh`,
`installer/optional-packages.conf` and packaging `Makefile` were folded into it
(the one file both installs and, via `--package`, builds the release archive).
`src/dependencies.rs`'s tests keep it honest: every inventory `package` must
appear as a whole-word token in `install.sh`, the inline systemd unit / udiskie
config must keep their load-bearing directives, and `install.sh` must not
reference any sibling file. Ollama is installed via its official `curl … | sh`
(not apt), best-effort; **no model is auto-downloaded** — the admin pulls one
from Settings > AI.

---

# TLS / HTTPS (2026-09-09) — `docs/guides/tls.md`

Built-in, **opt-in**, additive. The plain-HTTP listener on `PORT` (8080) is
unchanged. When enabled (`TLS_ENABLED`, or Settings > Security > HTTPS/TLS,
persisted to `{DATA_DIRECTORY}/data/tls.json`), `web::run()` *also* binds an
`axum_server` TLS listener on `HTTPS_PORT` (8443) serving the same router.

- **`src/tls.rs`** owns everything: settings load/save, self-signed generation
  + renewal (`rcgen`, `{DATA_DIRECTORY}/tls/self-signed.{crt,key}`, SANs cover
  localhost / hostname / `homevault.local` / LAN IP, ~13-month validity, renew
  under 30 days), cert introspection (`x509-parser`), path/port validation.
- **Modes:** `self-signed` (default) or `manual` (admin-supplied PEM paths,
  validated as absolute + traversal-free + parseable). No ACME yet.
- **Redirect:** `TLS_REDIRECT` / the toggle adds `middleware::https_redirect`
  to the **HTTP** router only; loopback is exempt (installer health check +
  app-proxy keep working over plain HTTP).
- **HSTS** (`middleware/security_headers.rs`) is now sent **only** when
  `crate::tls::tls_active()` — and without `preload`.
- **Apply model:** listener binds once at startup. `PUT /api/settings/tls`
  persists + returns `requires_restart: true` (mirrors OMV restarting nginx).
- Endpoints (admin-only): `GET|PUT /api/settings/tls`,
  `POST /api/settings/tls/regenerate`. Frontend:
  `frontend/src/lib/components/settings/TlsCard.svelte` in the `security`
  category. `settings_persistence::reset()` also clears `tls.json`.
- A broken/missing certificate is logged and the instance stays **HTTP-only** —
  TLS trouble must never stop the server.

Treat the `src/tls.rs` invariants above (SAN list, renewal window, path
validation, HTTP-only fallback, loopback redirect exemption) as easy to
break.

---

# Design system — one source of truth

Full detail: **`docs/guides/design-system.md`** + `docs/FRONTEND.md` → Design
system. The product has **one** identity — "HomeVault Appliance" (2026-09-12),
which replaced "Fluent Paper" / "Frosted Glass Modernism". Nothing references
another framework's design language (Fluent, Material, …) as an authority.

- `frontend/src/lib/styles/theme/tokens.css` is the **only** place design
  tokens are declared. Every `var(--…)` in `.svelte`/`.css` **must use the
  `--hv-` prefix**. The legacy non-`--hv-` alias block in `tokens.css` is
  inert — no new consumers.
- **Appliance palette** — one brand accent `--hv-primary` (#1E62AE "HomeVault
  Blue") + a cool blue-grey neutral ramp (#1F2733 ink family), semantic hues
  `--hv-success` #1E7E34 / `--hv-warning` #B45309 / `--hv-error` #C22B21 /
  `--hv-info` = primary. Surfaces are **opaque paper** (`--hv-shell-*`:
  glass #FFFFFF / card #F7F8FA / fill #EFF1F4 / hairlines #DCE1E7) — no blur,
  a dark photo wallpaper must never wash out ink text. Radius stops are
  appliance-tight (controls 6–8px, cards 10–12px, windows 16px).
- **No hardcoded values** anywhere: a color, radius, shadow, spacing or
  z-index in a page/component is a `var(--hv-*)` or a Tailwind token
  (`bg-primary`, `text-muted-foreground`, …) that resolves to one. **No
  per-app class prefixes** (`dk-*`, `db-*`, `restic-*`, `cs-*`, …).
- **Every screen is touchscreen AND mouse compatible (WCAG 2.5.5 / Apple HIG
  / Material)**: every interactive target — button, input, select, textarea,
  switch, slider, icon-button, picker row — is ≥ `--hv-touch-target-min`
  (44px) on each axis, sized with that token (or a Tailwind class resolving
  to it), never a smaller literal. The *visible* geometry may stay compact
  (the 40×22 switch track is fine) only while the **hit area** is 44px.
  Nothing may be interactive on hover only — anything hover-revealed must be
  reachable by tap and keyboard focus. A screen is done only when it works
  with a mouse *and* a finger.
- **Two type scales, deliberately split**: `--hv-font-size-*` (content,
  default) and `--hv-text-*` (compact chrome, one step smaller). Don't merge —
  it would resize ~16 files. Typeface is Inter (self-hosted).
- **Component library is canonical for every screen**: the theme primitives
  (`theme/` → `Button`, `Input`, `Select`, `Toggle`, `Checkbox`, `Radio`,
  `Alert`, `Badge`, `Spinner`, `Modal`, `Icon`, `UserAvatar`, `PathField`,
  `FolderPicker` / `PathPickerField` + `FloatingPathPicker`) delegate to the
  **shadcn-svelte / Melt UI** layer (`ui/`); the **page composites**
  (`app/` → `PageLayout`, `StatCard`, `DataTable`, `SectionCard`,
  `EmptyState`, `FormRow`) standardize the page anatomy; `WindowHeader`
  (`desktop/`) is the one in-window header. New screens use these — nothing
  hand-rolled per app.
- **One in-window header — `WindowHeader`**: every app renders exactly one
  `<WindowHeader>`, and its bar is **[APP ICON] [WINDOW TITLE] and nothing
  else** (2026-09-17). No subtitle (removed 2026-09-14); and — this is the part
  that keeps regressing — **no app fills the `center` / `extra` / `actions` /
  `actionIcon` + `actionLabel` props**: a page-level action is a `WindowToolbar`
  row *under* the header (`PageLayout`'s `toolbarLeft` / `toolbarRight`), the
  screen's own in-body toolbar (Explorer), or the rail footer when it belongs
  with one control (Settings' save state lives in its one rail-footer Save
  button — colour + label, plus an sr-only status line; 2026-09-23). The tab
  row lives in `.hv-winhdr-wrap`, outside the bar, and the min/max/close trio is
  a **sibling** of the bar (owned by `WindowFrame`), so both stay. E2E `14.11`
  walks all 24 registry app ids and fails any bar that refills a slot or
  hand-rolls a control into `.hv-winhdr`. The page root is a non-scrolling flex
  column (`height:100%; overflow:hidden`); the header sits flush
  (`flex-shrink:0`), a `.*-body` / `.*-layout` child takes
  `flex:1; min-height:0; overflow-y:auto` and carries the page padding.
- **No manual "Refresh" button (2026-09-08)**: data revalidates automatically —
  `lib/utils/autoRevalidate.ts::onAppVisible(loadFn, { pollMs? })` on
  `visibilitychange` + window `focus`, plus a slow `pollMs` for live screens
  (Docker, Health). IconRefresh stays only for genuine *actions* (restart
  service/container, retry connection, rebuild table, rescan folder,
  regenerate avatar, re-query Ollama).
- **One colour per app — its own gradient**: every app's icon tile uses its
  dedicated `--hv-app-<id>-grad` token on the Launchpad tile, the in-window
  `WindowHeader` and (for pinned apps) the dock. The icon tile is never flat.
  A new app needs its own `--hv-app-<id>-grad` token, its `--hv-app-<id>-c`
  running-dot colour (the gradient's **dark stop**, lowercased — E2E `14.2`
  enforces it) and a dock class if it gets pinned (a tile with no class falls
  back to the accent gradient while its dot keeps its own colour). Never
  re-hardcode a different accent or a flat colour on dock/header/launchpad.
- **Topbar pill + dock** always use `--hv-shell-glass` (never a bespoke
  low-alpha white, and never a `backdrop-filter` — a dark wallpaper must not
  wash out the ink) plus the chrome material pair
  `--hv-chrome-surface-border` / `--hv-chrome-surface-shadow`, which
  `.hv-topbar-panel` shares so a popover can't mismatch the pill it hangs off.
- **Each pill owns its height** — `--hv-chrome-topbar-h` (62px) /
  `--hv-chrome-dock-h` (84px), and the dock is deliberately the taller one.
  Both are `box-sizing: border-box`, so each token is the rendered height and
  the keep-out (`--hv-chrome-*-keepout`) stays exact. Anything anchored to the
  dock — `.hv-activity-panel`, the ≤900px maximized inset — derives from the
  **dock's** band, never the topbar's. `snapUtils.ts DEFAULT_SNAP_CONFIG` is a
  hand-maintained mirror of that geometry; E2E `14.1` fails if they drift.

Screens migrate onto this vocabulary one by one (checklist in
`docs/guides/design-system.md` §8); there are no frozen screens blocking it.

---

# Design system governance — no frozen screens

Every page/component is **editable**. The old "frozen area" registry is gone
(2026-09-12): a screen that was audited and locked is still audited, but a
design change may touch it — when you do, keep its **tests** green (Rust
integration + Playwright E2E) and re-capture a reference screenshot.

That said, the hard-won technical invariants below are **still binding** — they
are correctness/security knowledge that survived real bugs, not style. They
live in compact form here and in full detail in each `docs/guides/<area>.md`;
the guides are the reference and must be updated in the same change as any fix.

---

# Accessibility baseline — WCAG 2.2 AA + EN 301 549 + ISO 9241 (2026-09-15)

HomeVault must be usable entirely with **mouse, touch, pen and keyboard**, at
**WCAG 2.2 AA**, **EN 301 549 §9** and **ISO 9241-410/171**. This is a
correctness bar, not a style preference — review every new/edited screen
against it. Full detail: `docs/guides/design-system.md` → §7.1. The rules:

1. **44px hit areas on every control.** A compact (28–40px) icon button keeps
   its small visible geometry but must carry a transparent 44px hit square via
   the shared `.hv-ic` / `.hv-ic-abs` classes (`components.css`) — never a
   bespoke `::after` on an element that already has one (`.hv-cl-btn`,
   `.hv-switch-track`, `.hv-dock-item` have their own documented pads). Raw
   `button`/`a`/`input`/`select` get a 44px floor under
   `@media (any-pointer: coarse)` in `base.css` — keep it `any-pointer`, never
   revert to `pointer: coarse` (a hybrid touch-laptop's primary pointer is
   fine, but the touch panel must still get 44px). Targets pass WCAG 2.2 2.5.8
   (24px) by default; the project floor is the stricter 44px.
2. **Focus is always visible.** `--hv-focus-ring` = a 2px solid `#1E62AE` ring
   + 4px halo (≈4.4:1 on white). Never write `outline: none` on a
   `:focus-visible` rule without keeping `box-shadow: var(--hv-focus-ring)` —
   a colour change alone is not a focus indicator (2.4.7). Inputs focus with
   border-primary + ring. Any new custom control (calendar, slider, picker)
   must ship the same ring.
3. **Contrast floor.** Any token used for text must be ≥4.5:1 on every surface
   it renders on; status dots / focus rings / control boundaries ≥3:1 (1.4.3 +
   1.4.11). The audited tokens already hold (`--hv-shell-text-3` #5F6B76,
   `--hv-login-muted` #64748B, `--hv-settings-label` #64748B,
   `--hv-shell-border-strong` #87929E, badge `--hv-chrome-badge-bg` #DC2626,
   `--hv-wizard-error`/timer hues). A new page-scoped palette must be
   contrast-checked before it lands. The shipped accent default is
   `#1E62AE` (≥4.5:1 both behind white text and as text on the light
   surfaces/tints); the legacy `#4a90d9` fails both directions and is
   rewritten by `handlers::settings::normalize_accent` — never reintroduce
   it as a default, a `theme-color` or a gradient stop.
4. **Modals trap focus.** Prefer the bits-ui Dialog (`theme/Modal`,
   `theme/Drawer` — trap + Escape + focus restore built in). Any hand-rolled
   overlay (Desktop power/confirm, `HvLaunchpad`) must `use:focusTrap` from
   `lib/utils/focusTrap.ts`. A `role="dialog"` without a trap is a 2.1.1 gap;
   restore focus to the trigger on close (2.4.3).
5. **Nothing is hover-only, right-click-only or double-click-only.** A control
   revealed on `group-hover` must also appear on `group-focus-within` /
   `focus-visible` (the Chat delete button is the reference). Every contextual
   action has a visible button or keyboard path (Journal's copy is
   dblclick **and** Enter/Space).
6. **Accessible names.** Every icon-only button carries `aria-label` from an
   i18n key — never a hardcoded string, never `title` alone (invisible on
   touch). Every input has a `<label for>` or `aria-label` (the 2FA digit
   boxes name themselves per-field). `role="button"` containers must not nest
   `<button>` children. One `<h1>` per screen.
7. **Keyboard reachability of the desktop shell.** Windows open from dock /
   Launchpad / topbar with a native `<button>`; the tab order reaches every
   in-window control; drag/reposition is an enhancement on top of click
   actions, never the only path.

The 2026-09-15 audit pass (see §7.1) fixed: the 10%-alpha focus ring, three
low-contrast text tokens, the notification-badge ratio, the 28–36px icon
buttons, the hover-only Chat delete, the hand-rolled modals' focus, `pointer:
coarse` → `any-pointer: coarse`, the TOTP field names, and the title-only
icon-button names. Re-running the pass must not reintroduce any of them.

---

# Internationalization

Full detail: **`docs/I18N.md`**. The rules that matter when editing:

- `src/i18n/{en,fr,es,de}.toml` are the source of truth, embedded in the
  binary. The four files are at **key parity** (identical key sets, identical
  `%placeholder%` sets per key) — a new key goes into all four.
- **English base/fallback.** Never hardcode a UI string in a component — use
  `t('section.key')`.
- Some keys are looked up at runtime as `` `section.${var}` `` and grep for the
  full name finds nothing — **these must never be deleted**. The list of
  dynamic prefixes is in `docs/I18N.md`; if you add a new
  `` `section.${var}` `` lookup, keep the whole `section`.
- All four files are correctly accented (fr diacritics, de `ä/ö/ü/ß`, es
  `-ción`/`¿?`). Value edits keep keys and `%placeholder%` tokens byte-stable.

---

# Engineering invariants (2026-09-12 — formerly the frozen-area registry)

Every area below was audited end-to-end with a real backend, had real bugs
found + fixed + re-verified live. The file list per area moved to the guides
(`docs/guides/<area>.md` — the canonical reference, updated in the same change
as any fix). These are the **"easy to get wrong again"** notes — correctness /
security knowledge that survived real bugs. A change that touches one of these
areas must keep the relevant tests green (Rust integration + Playwright E2E)
and re-capture a reference screenshot.

## Auth, sessions, recovery codes · `docs/guides/auth.md`, `setup-wizard.md`

- The email/token "forgot password" flow was **deleted** (migration 029) and
  replaced by an offline 32-digit recovery code. It is **never** stored/logged
  in plaintext; the Setup Wizard display is non-copyable (`user-select:none`,
  no copy/download) and shown exactly once; `verify_recovery_code`'s
  rate-limit key stays namespaced `recovery:` so a bad code never trips the
  login lockout; a successful reset must **not** consume/delete the code.
- The recovery-code `<input>` in `Login.svelte` is one-way `value=` +
  `oninput` (caret-restore) — do NOT revert to `bind:value`. A `<Button
  type="submit">` renders a native `<button type="submit">` so native form
  submission fires — don't swap it for a non-form-associated element without
  re-verifying the submit chain.
- `is_admin` checks the DB first (matching `create_session`), group check only
  as a DB-less fallback — the two must not disagree.
- Login / 2FA / setup / logout also `Set-Cookie: hv_session=` — read **only**
  by the per-app proxy; the JSON API must keep reading only `x-session-token`.
- A self password change invalidates every session server-side → the page must
  log out + redirect to `/login` (shared behaviour between `Users.svelte`'s
  `isSelf` branch and `Account.svelte`; a fix to one needs the same fix in the
  other).
- **First-run state comes from the database (2026-10-08).** `is_first_run` is
  `!SetupStateRepository::has_admin()` (a `users` row with `role_id = 2`); the
  `.setup_complete`/`.system_hash` marker files are gone and the singleton
  `setup_state` row (migration 037) holds the fingerprint. `handle_setup` and
  `handle_setup_verify_totp` must gate on `has_admin()`, never a file. The
  `.setup_lock` `O_EXCL` lock stays. Don't reintroduce a file as the authority —
  it can disagree with the accounts it is supposed to describe.
- **A hardware change never re-shows the Setup Wizard.** With an admin present,
  a changed fingerprint sets `needs_recovery: true`; the frontend routes to the
  recovery screen (`Login` `recoveryMode`), whose successful admin sign-in calls
  `POST /auth/setup/rebaseline` (admin-only) to rewrite the fingerprint. Showing
  the Wizard there dead-ends on "Setup already complete".
- **`lib/stores/appGate.ts` is the one frontend route guard** (Umbrel's
  `EnsureUserExists`/`EnsureUserDoesntExist` equivalent). `App.svelte` renders
  setup/recovery/login/desktop/error from `$appGate.route`; `Login.svelte` must
  **not** fetch `/auth/setup-status` or render `SetupWizard` inline, and
  `api.getSetupStatus()` must throw on failure (never fake `is_first_run: true`,
  which would show the Wizard on a network error).
- **The bootstrap endpoints are local-network-only.**
  `middleware::setup_guard` on `POST /auth/setup`, `POST /auth/setup/verify-totp`
  and `GET /auth/check-username` allows only loopback / private / link-local /
  ULA peers, read from `ConnectInfo` — never from `X-Forwarded-For`/`X-Real-IP`
  (forgeable). `/auth/setup-status` and the admin-authenticated
  `/auth/setup/rebaseline` are exempt. `HOMEVAULT_SETUP_ALLOW_REMOTE=1` is the
  documented opt-out.

## Users / Groups / Accounts · `docs/guides/users.md`, `groups.md`, `account.md`

- The `homevault_admin` OS group must never be a **tickable** checkbox — it
  shows as a read-only, role-derived chip (state = `role === 'admin'`, kept out
  of the editable `groups` array; `toggleGroupSel`/`saveGroups`/
  `createForm.groups`/`updateUserGroups` never touch it). Membership is driven
  only by the Role selector (`sync_admin_user_to_group` / `remove_user_from_group`).
- Deleting a user must clean up live PTY sessions (`pty_manager.kill_session`),
  purge the SFTP jail (`SftpJailService::purge_user` — unconditional),
  revoke the share permissions it holds **directly**
  (`PermissionService::revoke_all_user_permissions` — ACL best-effort + the
  `share_permissions` rows; group grants stay, they belong to the remaining
  members) and delete the `transfer_quotas` row — all four so that a
  same-name recreate inherits nothing from the deleted account. The
  `share_permissions` table keys a grantee by **name only** (no foreign key),
  so nothing else enforces this: dropping the revoke call silently re-opens
  the bug. Guard: `users_integration.rs` →
  `test_user_delete_revokes_its_direct_share_permissions_only`.
- Home-directory size limit: the detail-sheet slider saves via `saveHome()`
  only; `saveProfile` must keep forwarding `home_size_bytes` **and
  `home_path`** through `updateUserFull`. `setquota -u` is best-effort and must
  never fail home creation; negative size → 400; size > disk free → 400; the
  slider is offered for every role.
- Custom home directory: `user_homes.rs::validate_custom_home_path` is the
  security gate — MUST stay (rejects relative paths, `.`/`..`, control chars,
  `/`, `FORBIDDEN_HOME_ROOTS` = `/bin /boot /dev /etc /lib* /proc /root /run
  /sbin /sys /usr /var`, anything inside the HomeVault data dir; canonicalizes
  the parent so symlinks can't smuggle the target out).
- `GroupService::list_groups`/`is_admin_group` are shared with Users — editing
  one can silently break the other. Every caller of `handle_group_add_member`
  must validate session + `is_admin` before the admin-group check.

## Terminal / PTY · `docs/guides/terminal.md`

The WS closing must **not** kill the PTY — only `close_tab`, `reap_stale`,
`kill_session`, `cleanup_all` kill (reconnect support). Per-user cap
`MAX_TABS_PER_USER = 8` + the 30-min idle reaper (`SESSION_IDLE_TIMEOUT` via
`PtySession.attached`/`idle_since` + `DetachGuard`) bound orphaned shells.
`terminal` must stay in `WindowFrame.svelte`'s `OWN_HEADER_APPS` (`:324`) — the
app ships its own `<WindowHeader>`, and leaving it out stacks a blank duplicate
header (the bug `quotas` shipped on 2026-09-14, caught by E2E `14.9`). Window
dragging is **generic, not per-app**: `WindowFrame` drags from any
`[data-hv-drag]` region and bails on
`closest('button, a, input, textarea, select, label, [role="button"]')`
(`:62-63`). There is no chromeless terminal branch; the invariant is that the
in-window header keeps its `[data-hv-drag]` region. The tab strip is a bar of
the **window content** (`terminal-tabs`, between `WindowHeader` and
`terminal-infobar`), never inside the header — E2E `2.8` guards the ordering.

## Shares / SFTP / SMB · `docs/guides/shares.md`

- `require_admin` on list/get. `grant_permission` applies the real ACL
  **before** writing the DB row; `revoke_permission`'s DB delete is not blocked
  by an ACL failure. Resolve the real share path via `SharesService::get_share`,
  never a hardcoded `/srv/{name}`.
- SFTP: `ensure_linux_user` clears the `useradd` password lock right after
  `useradd` **only**; `set_authorized_key`'s `0644` must not revert to `0600`
  (OpenSSH's pre-auth worker can't read it); the sshd config
  (`UsePAM no`, `PasswordAuthentication no`, `AllowGroups homevault-sftp`,
  `ChrootDirectory .../%u`, `ForceCommand internal-sftp`) stays;
  `handle_sftp_key_set` keeps its `user_exists()` gate.
- SMB `set_share_enabled` keeps the end-of-string fallback when
  `find("\n[")` returns `None`. `/etc/homevault/*.json` writes stay on
  `write_file_as_root`/`mkdir_as_root`. Every `setfacl` call stays
  `sudo`-wrapped; `acl.rs::remove_acl` also clears the matching `default:` entry.

## Storage / RAID / encryption · `docs/guides/storage.md`, `raid.md`

- **RAID ↔ Storage boundary**: Storage manages every virtual disk (mdadm + ZFS
  + Btrfs) via `storage/virtual-disks[/{name}]` + `/{name}/scrub`; RAID only
  creates. Handlers dispatch by type — mdadm → `DiskService::delete_raid`,
  ZFS/Btrfs → `PoolService`. `pools.rs`'s internal `delete_pool`/`start_scrub`
  still reject `PoolType::Mdadm`; `GET /storage/pools` filters mdadm out (the
  Cache tier's `GET /cache/pools` deliberately still returns them).
- Every disk-write command stays `sudo`-wrapped; `/etc/fstab` is only ever
  written via `write_file_as_root`. `zpool create`'s pool name comes before the
  vdev type. mdadm member/device parses read the detail table's **last token**,
  never `parts.first()`/`starts_with("/dev/")` (the `/dev/md/x:` header line).
- `PoolService::parse_size` handles both suffix styles (`T/G/M/K` and
  `TiB/GiB/…`) + a locale comma; multiply in `f64`, cast last. Same rule in
  `dashboard.rs::parse_size` (`,`→`.`, f64, cast last).
- Btrfs snapshot ops need the pool's real **mount point**, not its bare name;
  Btrfs raid5/6 forces metadata to `raid1`; pool delete is `wipefs -a` on every
  member (type-to-confirm). ZFS `zfs rollback -R` works in place. `CreateSnapshotRequest`
  must not regain a `pool` field.
- `handle_encryption_create` gates `req.device` against
  `DiskService::get_available_devices()` (else an admin can luksFormat the boot
  disk) and cleans up an open LUKS mapper/mount on every failure branch;
  `encryption::erase` runs a trailing `wipefs -a`.
- Rescue (`docs/guides/rescue.md`): `SecurityValidator` must be called
  **synchronously before job creation** on every path/device-taking handler.
  `ddrescue`/`testdisk`/`photorec` stay `sudo`-wrapped; ddrescue status parses
  the **mapfile** (`parse_ddrescue_mapfile`), never a `--status` flag;
  `testdisk_rebuild` uses `/cmd <device> analyze,search` — never re-add `write`
  to `/cmd` (TestDisk 7.1 blocks forever) and keeps the 300 s timeout +
  `stdin(null())` + `kill_on_drop`; `list_block_devices` stays on `lsblk -d -n
  -b -P` key="value" pairs; `mdadm_diagnose` tests failure words first
  (`"clean, degraded"`); `mdadm_repair` maps to `--action=repair`.

## Docker / app catalog · `docs/guides/docker.md`

- `ComposeManager::validate_name` at the top of every project-name-taking
  method (path-traversal → traversal-reachable `remove_dir_all`).
- Catalog ids on the wire are namespaced `<store>:<app>`; the **install key**
  (compose project + data dir) is the bare app id for the `homevault` store,
  `<store>-<app>` otherwise. A new app is a `catalog/<id>/` package, never a
  code entry.
- The registry digest check stays **read-only** (anonymous v2 bearer flow, no
  pull) and treats "unknown" as "no update", never an error.
- The `hv_session` cookie is **proxy-only** — the JSON API reads only the
  `x-session-token` header. `app_proxy/serve.rs`'s upgrade branch sends the
  upstream request in **origin form** (path+query only) and spawns `conn`
  `.with_upgrades()` before `hyper::upgrade::on`.
- `parse_ports`: Docker's `{{.Ports}}` is `IP:HOSTPORT->CONTAINERPORT/proto` —
  **the colon is LEFT of `->`**. The host-port conflict check is re-run
  server-side (`handle_docker_container_create` + `handle_catalog_install`),
  not UI-only.
- **Deliberate:** the app's own published port stays bound to `0.0.0.0` (the
  proxy is an extra entry point, not a lockdown) — don't bind containers to
  `127.0.0.1` without an explicit request.

## Restic / backups · `docs/guides/backup-restic.md`

Reads = `require_auth`, mutations + `?include_password=true` = `require_admin`;
`handle_restic_backup_create` calls `update_backup_job` on both success and
failure. `parse_backup_summary` takes the **last non-`status` JSON line** of
`restic backup --json`; the byte field is `total_bytes_processed`. A failed
scheduled run must still reschedule and mark its job `failed`, never leave it
`running`. `normalize_crontab_dow` maps the crontab DOW field (`0/7=Sun…6=Sat`)
before handing it to the `cron` crate (`1=Sun…7=Sat`).

## Explorer / transfers · `docs/guides/explorer.md`, `transfer-quotas.md`

- Tracked copy/move jobs must register via `PROGRESS_MANAGER.create_job`
  (with `owner`) — never a self-generated UUID (every progress event would be a
  silent no-op). `handle_explorer_copy`/`_move` thread the session username in.
- `JobType` uses serde **adjacent** tagging (`tag = "type"`, `content = "name"`).
  Plain internal tagging cannot serialize the `Custom(String)` newtype variant
  ("cannot serialize tagged newtype variant"), and `Custom` is what every feature
  job uses — Explorer copy/move, Docker install/update, antivirus, RAG, VM,
  rescue, cloud sync, Ollama pull. The fallout was invisible on the taskbar
  (the activity handler maps `JobType` to a slug via `job_type_kind` and never
  serializes the enum) but broke the SSE payload: `handle_progress_sse`'s
  `serde_json::to_string(&event).unwrap_or_default()` sent an **empty** `data:`
  line, so `JSON.parse` threw client-side and `Explorer.svelte`'s `watchJob`
  resolved false for a copy that succeeded. Never revert to `tag` alone; the
  round-trip tests at the bottom of `src/sse/progress.rs` guard the wire shape
  (`{"type":"backup"}` for built-in kinds,
  `{"type":"custom","name":"explorer_copy"}` for a feature slug).
- `ProgressManager::get_progress` must keep returning the job's stored
  `last_event` — never synthesise a placeholder. The activity feed, the SSE
  stream and `GET /api/progress/{job_id}/status` must agree on a job's state
  (`list_activity` and the SSE stream read `last_event` too).
- `restore_trash_item` keeps the rename→(EXDEV)→copy+delete fallback;
  `find_trash_entry` treats a missing trash root as `NotFound` and tries the
  next one; `list_directory_sync` skips a bad entry (`let Ok … else continue`),
  never `?` it. `handle_explorer_eject` keeps the `is_removable_block_device`
  guard. Snapshot mode is read-only: `blockedInSnapshot()` gates every mutation
  except `copySelectedTo`.
- `handle_explorer_download` must go through `api.downloadFile()`, not a bare
  anchor click. `check_quota` compares in `u128` (an `as i64` on a huge value
  goes negative and slips under the limit). Transfer quotas meter **only**
  Explorer transfers — SMB/NFS/FTP/WebDAV are separate daemons.
- **Quotas screen live-audited 2026-10-07** — it was the last screen with no
  live audit and no green E2E run. `tests/e2e/quotas/quotas.spec.ts` is now
  green end-to-end (**38/38**, Firefox, scratch `PORT=` + isolated
  `DATA_DIRECTORY`, no real data; reference count `npx playwright test --list
  --project=firefox` = 38, `grep -cE '^\s*test\('` = 36 — the responsive
  360/768/1920 loop is one line → three tests). The screen
  (`frontend/src/routes/Quotas.svelte`, app id `quotas`) was checked live on the
  same server: launcher open, icon+title-only header, the three
  mutually-exclusive `aria-pressed` filter chips, the seeded row's `used/limit`
  progress bar (`aria-valuenow="50"` on 50 Mo / 100 Mo) **plus** *Unlimited* on
  a disabled direction, and **Manage in Users** emitting `hv-open-app`
  (`detail.id = 'users'`) without leaving `/desktop` or closing Quotas. **No
  dead selector was fixed** — the spec was written after the 2026-09-12
  Appliance migration and already targets `data-testid`; it passed as-is. Still
  open (out of this pass's scope): the screen has **no dedicated Rust
  integration file** — its `GET /transfer-quotas/{type}` endpoint stays covered
  by `transfer_quota.rs` + `explorer_integration.rs`. `TESTING.md` row 6 and the
  summary carry the same date.
- `mkdir`/`delete`/`rename`/`copy`/`move`/`extract`/`permissions`/`preview`
  handlers wrap their sync filesystem/`Command` work in `spawn_blocking`.

## Network · `docs/guides/network.md`

Only *writes* get `ip_sudo` — the read-only `ip … show`/`ping -c 1`/
`ethtool <name>` calls stay bare. `/etc/resolv.conf` and the
`/proc/sys/net/ipv6/…` knobs go through `write_as_root` (`sudo tee`).
`build_resolv_conf` keeps the non-`nameserver` lines (`search`/`options`/
comments). Blocking `ip`/`ethtool`/`ping` calls stay in `spawn_blocking`.

## Security suite (antivirus / ransomware / firewall) · `docs/guides/antivirus.md`, `ransomware.md`, `firewall.md`

- Firewall: **every `sudo ufw …` call goes through `ufw_command()`** (sets
  `LC_ALL=C` **and** `LANGUAGE=""` — `LANGUAGE` overrides `LC_ALL`, and the
  enabled-flag parse looks for the literal `Status: active`). Mandatory-rule
  protection + `{"confirmed":true}`-only enable must not weaken.
- Antivirus: `parse_clamscan_line` parses only path+signature;
  `resolve_move_status` re-resolves each threat's disposition **after**
  `child.wait()` (`clamscan --move` relocates after printing `FOUND`).
  `quarantine_dir()` respects `DATA_DIRECTORY`.
- Anti-Ransomware: `honeypot_debounce` (10 s per-path) must not be removed.
- Folder fields are **pick-only** (no manual entry) via the shared
  `FloatingPathPicker`, which **must** `use:portal` to `<body>` (a desktop
  window's `transform`+`overflow:hidden` traps and clips it).

## Settings · `docs/guides/settings.md`, `wallpapers.md`

- `GET /settings/export` must redact `smtp_password` to `null` — and only in
  `SettingsPersistence::export()` (a path once bypassed the redaction).
- **One Save button (no auto-save)** — every editable value lives in
  `stores/settingsDraft.svelte.ts` (`draft.*`, per-group JSON baseline +
  dirty detection); the rail-footer Save button
  (`[data-testid="settings-save"]`) flushes every dirty group via `saveAll()`.
  That button is **also the screen's only save indicator** (2026-09-23): its
  `data-state` (`clean` / `dirty` / `saving` / `saved` / `error`) picks one
  skin (blue / red / blue / green / red) **and** a matching label — never
  colour alone — while the old visible status pill and the "N unsaved
  change(s)" caption stay deleted; `[data-testid="settings-save-status"]` is
  the sr-only `role="status"` line it renders for screen readers. Keep that
  one-element/one-message rule: don't re-add a second status element next to
  the button.
  Don't reintroduce per-field auto-save, and don't re-add per-card Save
  buttons on the security/monitoring sub-cards — their editable fields are in
  the same draft. The initial load must never PUT/POST; sub-card *action*
  buttons (scan now, resolve threat, add/remove path, regenerate cert, test
  email, check updates) stay immediate. The AI model `<select>` stages
  `draft.ai.model`; Save activates it via `chatSetModel`.
- The category rail is **8 grouped entries** (`categories` array), not the old
  12 flat tabs; don't re-flatten. `UpdateFrequency`'s default is `Manual`.
- **One row vocabulary** (`lib/components/settings/vocabulary.ts`) shared by
  `Settings.svelte` and every card in `components/settings/` — a section label
  is a **sibling** before its card, never inside it. `settingsCard` carries
  `shrink-0`: the panel is a scrolling column flexbox, and without it a tall
  card is squeezed and clips its own content instead of the panel scrolling.
- **One toggle geometry, whole screen incl. the sub-cards** —
  `theme/Switch.svelte` (native checkbox in a label, 40x22/16px knob, filled
  with the row's *category* colour via `accent`). `theme/Toggle` is the
  bits-ui `role="switch"` *button*: different size, brand-blue fill, and no
  `input` for the E2E helpers to read. Don't swap one for the other on
  Settings — the TLS card's last two `Toggle`s were migrated 2026-09-14.
- The Settings palette is page-scoped (`--hv-settings-*`): a category tint is
  always derived from its solid via `--hv-settings-cat-tint`, never a second
  literal. `refreshMs` lives inside the `monitor-layout` blob — no column, no
  migration; but `monitorStore.loadFromStorage` **whitelists** its keys, so a
  new blob field must be added there too.
- Wallpapers: `.settings-wallpaper-thumb` **must keep `h-auto`** and must carry
  **no** `box-shadow` (no outset, no ring — the shadow bled past each tile and
  made rows look overlapping; selection is the `aria-checked` state + the
  `settings-wallpaper-check` badge). Wallpaper-tile selection is driven by the
  reactive `$: selectedWpUrl`. The wallpaper overlay portals to `#app`, not
  `document.body`. `GET /wallpapers/{*path}` must stay registered before the SPA
  fallback.

## Chat / RAG / Cloud Sync / Syncthing / Downloader / VPN · `docs/guides/chat.md`, `rag.md`, `cloud-sync.md`, `syncthing.md`, `downloader.md`, `vpn.md`

- Chat: the session checks in `handle_chat_send`/`handle_chat_models_list`/
  `handle_chat_model_get`/the recommend handler must not be reverted (all four
  were once unauthenticated). `system_prompt` stays per-UI-language. Response
  conventions differ per handler: `send`/`models` answer HTTP 200 with
  `{"success":false}` on failure; `model` GET/PUT + `models/pull` use real
  400/401/403. `handle_chat_models_pull` requests `"stream": true` and consumes
  Ollama NDJSON — don't revert to `stream:false`.
- Cloud Sync: session on every handler; **admin** on every mutation **and** on
  `GET /rclone/config` (rclone's "obscuring" is trivially reversible).
  **The config path comes from `rclone_config_path()` and nowhere else**
  (`RCLONE_CONFIG` override → first existing of rclone's locations →
  `/etc/rclone.conf`); `parse_rclone_config()` uses the same resolver. The four
  read/write handlers used to hardcode `/etc/rclone.conf`, so the editor and
  the remote list could disagree, and saving failed with `Permission denied`
  for any admin whose config lived elsewhere. No handler may reintroduce a
  literal path — guarded by `test_parse_and_the_handlers_resolve_the_same_file`
  (unit) and three `rclone_integration.rs` tests that drive the real endpoints
  with `RCLONE_CONFIG` pointed at a tempdir.
  `MountRequest`'s `#[serde(default)]` on its 3 bool fields stays; the
  `[`-line rejection in `add_remote` stays; `handle_mount_start` uses `--daemon`.
- Syncthing: `require_admin` on all 10 handlers. `handle_syncthing_folders`
  keeps the per-folder `db/status?folder={id}` call; `DbFolderStatus` must not
  regain the 4 fields that response doesn't have; `SyncthingDevice.cert_name`'s
  `#[serde(alias="certName")]` stays; `pause`/`resume`/`scan` keep
  `url::form_urlencoded` on interpolated ids.
- Downloader: the `#[serde(rename(deserialize=...))]` attrs stay (aria2 is
  camelCase; errors are discarded via `if let Ok`). `tellActive`/`tellWaiting`/
  `tellStopped` (merged) stay, not `tellAll`; the 3-step remove fallback
  (`remove` → `forceRemove` → `removeDownloadResult`) stays. `Aria2Task` stays
  `pub`. The activity taskbar reads `aria2.tellActive` via `GET /api/activity`
  (read-only, never spawns aria2).
- VPN: `handle_vpn_capabilities`/`_list`/`_get` keep the `{success,data}`
  envelope, not a bare struct (the frontend checks `response.success &&
  response.data`). PID-numeric check before `kill`; `0600` on written
  key/credential files.

## Dashboard / Health / Widgets · `docs/guides/dashboard.md`, `health.md`, `widgets.md`

- Dashboard: every endpoint requires a session. Blocking subprocess calls
  (`df`, `nvidia-smi`/`rocm-smi`, `smartctl`, `sensors`, `dmidecode`,
  `dpkg-query`) stay in `spawn_blocking` with per-disk lookups via
  `tokio::join!`. `handle_storage_data` reads `parts[5]` for the mount point,
  `parts[4]` for percent, length guard `>= 6`.
- Health: `handle_health_endpoint` always answers HTTP 200 and signals auth
  failure in the body (`success:false`). `HealthStatus`'s 4 variants
  (`healthy`/`warning`/`unhealthy`/`unknown`, serialized PascalCase, mapped
  lowercase) stay in sync with the frontend `CheckStatus` type + i18n keys
  (`'critical'` never appears on the wire).
- Widgets: the Weather monitor calls Open-Meteo **directly from the browser**
  on a 10-min interval (deliberately not proxied). `loadHealthData` tallies
  `status === 'unhealthy'` into the "critical" bucket, never a literal
  `'critical'` string. Monitor visibility stays persisted via
  `/api/monitor-layout`, not component-local. `count_distinct_active_users()`
  keeps filtering `expires_at > now` + deduping by username.

## System Journal · `docs/guides/journal.md`

- The cleanup scheduler's first `tokio::time::interval` tick fires
  **immediately**; with no settings row yet, the `last_cleanup_at.is_none()`
  arm must **persist a baseline without cleaning** (`set_last_cleanup(now)`,
  return `false`) — never call `clear_app_logs()`.
- `set_last_cleanup` must stay an **upsert** (`INSERT … ON CONFLICT(id) DO
  UPDATE`) — a bare `UPDATE … WHERE id=1` is a no-op on a fresh DB.
- `clear_app_logs` must **truncate** the currently-open
  `homevault.log.<UTC-today>`/`homevault.log` in place (same inode, the
  `tracing-appender` fd keeps writing) — never `remove_file` it (orphans the
  appender's fd → the "app" source is permanently empty).
- `JournalCleanupSettings::default()` is on / 90 days (G1). The `app` source
  is a business-event audit trail — always **plain English** regardless of UI
  language, via `events::log_event` on the existing `tracing` pipeline.
- Text-log sources (`/var/log/{syslog,auth.log,kern.log,daemon.log}`) are
  advertised only if `Path::exists()`.

## Frontend reactivity & runes · `docs/FRONTEND.md`

- `Restic.svelte` and `Downloader.svelte` are Svelte-5 **runes mode** — every
  reactive var must be `$state(...)` (a plain `let` reassignment doesn't
  re-render → permanent spinner). `Syncthing.svelte`, `Chat.svelte`,
  `routes/Settings.svelte` and `routes/Desktop.svelte` are legacy-mode.
- **Runes mode is all-or-nothing per file.** A single `$state`/`$derived`/
  `$effect` in a legacy component flips the *whole* file to runes mode, which
  turns every `$:` statement into a hard build error
  (`legacy_reactive_statement_invalid`) and silently de-reactivates every plain
  `let`. So in `Settings.svelte` (~40 mutable locals, a dozen `$:` blocks) or
  `Desktop.svelte`, use a legacy `$:` — never "just one rune". A legacy `$:`
  also runs during init, *before* `onMount`, which is how `Desktop.svelte`
  seeds `resourcePollMs` from `monitorStore` before the first subscribe.

## Static assets, caching & the icon sprite · `docs/ARCHITECTURE.md`, `docs/FRONTEND.md`

- **Cache policy lives in `spa.rs`'s handlers, one policy per route — never a
  blanket `SetResponseHeaderLayer` in `create_app`.** A layer applied after
  the routers are merged also covers the API and `index.html`, so it would pin
  both for a year. The two policies are `CACHE_IMMUTABLE` (content-hashed
  bundles, wallpapers, `static/`) and `CACHE_REVALIDATE` (the two sprites,
  whose URL is fixed but whose content is regenerated every build).
  `index.html`, `sw.js`, `manifest.json` and `favicon.svg` get **no**
  `Cache-Control` on purpose.
- **Every embedded file keeps an `ETag`, and the validator is per content
  coding** (`-br` / `-gz` suffix). Dropping the suffix lets a cache answer a
  `br` request with the identity body. A matching `If-None-Match` must stay a
  `304` with no body.
- **Pre-compressed `.br` / `.gz` siblings are the served bytes, not a cache.**
  `frontend/scripts/precompress-dist.mjs` writes them from npm's `postbuild`,
  and `Content-Encoding` is what makes `CompressionLayer` skip the response.
  They are never addressable on their own — `/foo.js.br` 404s (or falls to the
  SPA fallback), it never returns raw brotli.
- **The icon sprite is generated from the sources, never hand-edited, and it is
  subset.** `npm run build` runs it as `prebuild`. Run `npm run
  icons:generate` after adding an `@tabler`/`@phosphor` icon import that is
  referenced only indirectly, and remember the subset scans a *closure* of
  files (components rendering `<Icon>`, components passing an `icon`-ish prop,
  `$lib/stores` + `$lib/services`, the catalog manifests) — a whole-tree scan
  is not a substitute, it keeps nearly the full library. Do not "fix" a blank
  icon by adding every glyph back: `HV_SPRITE_FULL=1 npm run icons:generate`
  exists for that escape hatch and is a deliberate, temporary step.
- **`include_dir!` emits no `cargo:rerun-if-changed`.** After a frontend-only
  rebuild, `touch src/handlers/spa.rs` before `cargo build --release`, or the
  stale bundle (and any missing `.br` sibling) stays embedded.
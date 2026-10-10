# Frontend Structure

Svelte 5 (runes) + TypeScript + Vite. Source in `frontend/src/`.

```
frontend/src/
  routes/          One .svelte component per app (Dashboard.svelte, Users.svelte, …)
  lib/
    services/api.ts   API client (single `api` singleton, all HTTP calls)
    services/websocket.ts  WebSocket helpers (terminal)
    stores/           Svelte stores / rune modules (auth, i18n, theme, …)
    components/       Shared + layout + theme + desktop + per-feature components
    types/api.ts      Request/response TypeScript types
    styles/           CSS (tokens, theme, app-layout)
```

## Routes (`frontend/src/routes/`)

Each app is a `.svelte` file imported lazily by the desktop shell
(`Desktop.svelte`). The app registry is the `apps` array in
`frontend/src/routes/Desktop.svelte` (id, name, icon, category, component) — this
is the **single source of truth** for both the desktop icon grid and the
launcher (`components/desktop/HvLaunchpad.svelte`), which renders directly from
that array and cannot show an app that isn't really there. Apps opened from the
launcher become draggable windows (`components/desktop/WindowFrame.svelte`).

Routes (27): `Dashboard, Storage, Raid, Explorer, Docker, Network, Restic (Backup),
Users, Quotas, Groups, Shares, CloudSync, Syncthing, Health, Chat (AI Assistant),
Downloader, Rescue, Terminal, Journal, Settings, Account, Login, SetupWizard,
Desktop, VPN, Widgets, VirtualMachines`.

> `Quotas` (2026-09-14) is the read-only transfer-quota overview — not to be
> confused with the filesystem disk quotas on the Users home-folder card. See
> [transfer-quotas.md](./guides/transfer-quotas.md) and
> [quotas.md](./guides/quotas.md) for the collision.

> **Note:** `routes/AppsCenter.svelte`, `routes/Apps.svelte`, and
> `lib/components/theme/AppsCenter.svelte` were removed — they were dead
> duplicate launcher implementations (never imported by anything, with several
> cards pointing at apps that didn't exist). `HvLaunchpad` is the real,
> currently-used launcher and was already correct by construction (it has no
> hardcoded app list). `VPN.svelte` was fully built but not reachable from any
> UI until it was added to the `apps` array — see
> [guides/apps-center.md](./guides/apps-center.md) and
> [guides/vpn.md](./guides/vpn.md).

No routes are frozen (2026-09-12). The old frozen-area registry in `CLAUDE.md`
was replaced by the **engineering invariants** section (correctness/security
knowledge) and a **design-system governance** rule: any design change to an
audited screen must keep its tests green (Rust integration + Playwright E2E)
and re-capture a reference screenshot.

25 of 27 routes have **zero scoped `<style>`** (converted to Tailwind-on-tokens
+ `data-testid` hooks; specs updated). The two exceptions are `Desktop.svelte`
(the shell): its scoped `<style>` is the shared desktop-chrome CSS (dock,
topbar, launcher, windows, monitor widgets) — `hv-*` classes, 100%
token-driven, deliberately not converted (see `docs/guides/design-system.md` §8) —
and `Settings.svelte`, which keeps one small token-driven loader block
(`.set-spinner`; the save-button skin lives in `SettingsSaveButton`).
## Design system

The product has **one** design identity — "HomeVault Appliance" (2026-09-12),
which replaced "Fluent Paper" and "Frosted Glass Modernism". The full
reference is **`docs/guides/design-system.md`**; the summary:

- `frontend/src/lib/styles/theme/tokens.css` is the **single source of truth**
  for design tokens. Every `var(--…)` in `.svelte`/`.css` must use the
  `--hv-` prefix. Values are bespoke HomeVault stops (one brand accent
  `#1E62AE` "HomeVault Blue", cool blue-grey neutrals `#1F2733` ink family,
  semantic `#1E7E34`/`#B45309`/`#C22B21`, opaque `--hv-shell-*` surfaces), so
  the compliance is auditable in the file itself — no external framework is
  authoritative.
- **Opaque paper surfaces** — `--hv-shell-glass` `#FFFFFF` / card `#F7F8FA` /
  fill `#EFF1F4` / hairlines `#DCE1E7`; no backdrop blur anywhere in-window
  (a dark photo wallpaper must never wash out ink text). Depth comes from the
  layered shadow scale.
- **Appliance radius stops** — controls 4–8px, cards 10–12px, large surfaces
  16px, hero cards 20–24px. **Typeface: Inter** (self-hosted), two deliberate
  scales: `--hv-font-size-*` (content) and `--hv-text-*` (compact chrome, one
  step smaller). In markup both go through the one `app.css` `@theme inline`
  bridge — `text-*` = content, `text-c-*` = compact (UI audit W1); the
  `text-[length:var(--hv-…)]` spelling is banned (see
  `docs/guides/design-system.md` §2.1).
- **One component language, three layers**: primitives (`theme/`) → framework
  (`ui/`, shadcn-svelte / Melt UI) → page composites (`app/`) + desktop
  (`desktop/`). New screens use these — nothing hand-rolled per app.

### Icon sprite (`phosphor-` / `tabler-`)

`frontend/src/lib/icons/generate-sprite.mjs` reads `@phosphor-icons/web`
(`selection.json`) and `@tabler/icons-svelte` (Svelte components) and bakes
them into the unified sprite `public/sprite-unified.svg` under the `phosphor-`
/ `tabler-` prefixes. `Icon.svelte` treats `phosphor-*` symbols as filled
glyphs (`fill: currentColor`) since Phosphor's regular set is solid shapes,
unlike Tabler's stroke-based outlines. Usage: `<Icon name="tabler-check" />`
or `<Icon name="check" library="tabler" />`. The library-agnostic
`getIcon(name)` auto-fallback prefers `tabler` then `phosphor`. Regenerate via
`npm run icons:generate`. **The Fluent System Icons set
(`@fluentui/svg-icons`, `fluent-` prefix) was removed 2026-09-11** along with
the Fluent component library — the sprite is now Phosphor + Tabler only.

### shadcn-svelte + Melt UI (2026-09-11, replaces Web Awesome and Fluent)

The shared component library is **shadcn-svelte** (copy-paste components in
`frontend/src/lib/components/ui/`, built on **Melt UI** primitives via
`bits-ui`). It replaces both the `@fluentui/web-components` pilot and the
`@awesome.me/webawesome` wrapper 1:1 — same wrapped theme components, same
external prop APIs, no call site needed updating.

- **Setup:** `components.json` (style `vega`, base `neutral`), the Tailwind v4
  theme bridge in `app.css` (`@theme inline` + `:root`), and the generated
  `lib/components/ui/{button,badge,dialog,input,checkbox,radio-group,switch,
  alert,skeleton,tooltip,select,separator,spinner}`. `lib/utils.ts` exports
  `cn()` (clsx + tailwind-merge) and the `WithElementRef` / `WithoutChildren*`
  type helpers. `tw-animate-css` supplies the `animate-in/out` utilities the
  dialog/select/tooltip use.
- **Primitive set (2026-09-13):** the `ui/` layer was completed with the rest
  of the shadcn-svelte registry: `accordion`, `calendar`, `collapsible`,
  `command`, `context-menu`, `date-picker` (composed from `calendar` +
  `popover` + date-fns), `dropdown-menu`, `input-group`, `pagination`,
  `popover`, `progress`, `scroll-area`, `slider`, `sonner` (toasts),
  `tabs`, `textarea`, `toggle`, `toggle-group`. Toasts are surfaced via the
  `theme/Toast.svelte` wrapper (mounted once in `App.svelte`; `toast()` from
  `svelte-sonner`; hardcoded light theme — HomeVault is light-only).
- **Charts (2026-09-13):** `lib/components/charts/` wraps **LayerChart**
  (Svelte 5 / d3) with `--hv-*` tokens: `DonutChart`, `BarsChart`,
  `TrendLineChart`. The Dashboard's CPU per-core panel uses `BarsChart`.
- **Drawer (2026-09-14):** `theme/Drawer.svelte` wraps the shadcn-svelte /
  bits-ui `Dialog` primitive (Svelte 5 native) positioned as a bottom/top/
  left/right sheet (slide-in/out animations, `--hv-*` tokens, backdrop
  dismiss controlled by `interactOutsideBehavior`). Prefer `Modal` on
  desktop; `Drawer` targets < 768px. Replaces the former Vaul Svelte
  implementation, which was the only dependency pinning a Svelte-4-era
  `bits-ui@0.21`/`@melt-ui/svelte@0.76` stack (npm ERESOLVE peer warnings on
  every install); Vaul's swipe/snap-points drag layer is dropped.
- **Theming is a bridge, not a theme object:** `tokens.css` (`--hv-*`) stays
  the single source of truth. `app.css`'s `:root` maps shadcn's semantic
  variables (`--background`, `--foreground`, `--primary`, `--ring`, …) onto the
  `--hv-*` tokens — #1E62AE brand, #1F2733 ink on #FFFFFF/#F7F8FA paper,
  #DCE1E7 hairlines, #C22B21 danger — and the `@theme inline` block registers
  them as Tailwind v4 color/radius tokens (`bg-primary`, `text-muted-foreground`,
  `ring-ring`, `rounded-md`, …). A `variant="brand"`-style button therefore
  renders HomeVault's blue, not shadcn's default black; `success`/`warning`/
  `danger` render `#1E7E34`/`#B45309`/`#C22B21`.
- **Wrapped components (external prop API unchanged):** `Button` → shadcn
  button (variants `primary`→default, `secondary`, `success`, `danger`,
  `warning`, `ghost`; `loading` renders the shared Spinner), `Badge` → shadcn
  badge (semantic tints from the `--hv-badge-*` tokens; `dot`/`pulse` stay
  hand-built), `Spinner` → shadcn spinner (lucide loader-2), `Modal` → shadcn
  dialog (bits-ui: declarative `open` prop + `onOpenChange` → `onclose`;
  `closeOnBackdrop=false` cancels `onInteractOutside`), `Toggle` → shadcn
  switch (`bind:checked`), `Alert` → shadcn alert (semantic tint variants,
  Tabler leading icon), `Input` → shadcn input, `Checkbox` → shadcn checkbox
  (bits-ui, `checked`/`indeterminate`), `Radio` → shadcn radio-group.
- **No Shadow DOM:** unlike Web Awesome/Fluent, these render real elements, so
  pages can style them with plain scoped CSS (the former `:global(wa-button)`
  hacks were rewritten to `:global(button.hv-btn)` / `:global(div.hv-alert)`).

### Styling policy — Tailwind on tokens (2026-09-12)

One styling method, one vocabulary. Everything reads the `--hv-*` tokens from
`tokens.css` (the single source of truth):

- **New components and screens** use Tailwind utilities (wired through
  `app.css` → `@theme inline` → `--hv-*` bridge) + the shared components. Never
  introduce a third color vocabulary.
- **No hardcoded values**: a color, radius, shadow, spacing or z-index in a
  page/component must be a `var(--hv-*)` (or a Tailwind token like `bg-primary`,
  `text-muted-foreground` that resolves to one). **No per-app class prefixes**
  (`dk-*`, `db-*`, `restic-*`, `cs-*`, …) — any repeated visual becomes a
  shared component or token.
- **Legacy hand-rolled `hv-*` classes** (`components.css`/`utilities.css`) stay
  on the existing screens until each migrates (checklist in
  `docs/guides/design-system.md` §8), but nothing new is written in them.
- A scoped `<style>` block is tolerated only for a genuine one-off page layout
  and must reference tokens for every value; the long-term goal is to remove
  them entirely.

The migration is tracked in `docs/guides/design-system.md` §8 — each screen is
converted in one pass (shared components + Tailwind, scoped CSS removed,
per-app prefix deleted), its E2E suite re-run and a reference screenshot
captured, then `TESTING.md` is updated in the same change.

Most of the app's real UI (native `<button>`/`<input>`/`<select>` styled with
`hv-*` CSS classes) does **not** go through the component library at all and is
out of scope for this migration — **except where a screen is deliberately
converted**, tracked per-screen below.

**Per-screen native-element conversion (beyond the shared component
library):**

- **`Login.svelte` (2026-09-14 re-skin — DSM 7 gateway, see
  `design-system.md` §6.1):** markup/CSS rewritten as pure Tailwind-on-tokens
  with **no scoped `<style>`**; the `<script>` logic (setup-status detection,
  credentials → 2FA flow, modal error handling, recovery-code reset) is
  unchanged. Visuals: full-screen pastel gradient (`--hv-login-bg-gradient`)
  with two blurred orbs, a live 24 h clock (`hour12: false`), and a 380 px
  white card with a gradient logo tile, brand title + version + step label,
  DSM fields, the remember-me `<Toggle>`, the forgot-password link, a primary
  CTA via `<Button>` + `bg-[image:var(--hv-login-btn-grad)]!`, and a footer.
  The 2FA step adds a step-progress indicator, 6 code boxes (44×52 px,
  soft-indigo `--hv-login-accent-soft` filled state), a 30 s TOTP countdown
  ring and a back link. Errors open the shared error `<Modal>` and shake the
  card (`animate-[hv-shake_0.4s_ease]`); a successful login scales + fades the
  card out before navigating to `/desktop`.
  **Still native**: username/password/recovery-code/new-password `<input>`
  fields (password-manager autofill risk), the 2 icon-only password-toggle
  buttons and the 6 TOTP boxes. `base.css` puts its global `input`/`button`
  defaults in `@layer base` (2026-09-15), so the field visuals carry Tailwind
  trailing-`!` modifiers as belt-and-braces and live on the **wrapper** div
  (`textInputClass` / `fieldWrapperClass` constants) — the inner inputs are
  transparent and borderless. **Verified live in a real browser**: form-submit chain, 2FA
  autofocus + auto-advance, remember-me switch, forgot/recovery flow, tab
  order (username → password → toggle → remember → forgot → submit), and
  responsive fit down to 360 px. All `login-*` testids are unchanged, so
  `tests/e2e/login/login.spec.ts` covers the new screen as-is.

- **`SetupWizard.svelte` (2026-09-14 re-skin — DSM 7 gateway, see
  `design-system.md` §6.2):** markup/CSS rewritten as pure Tailwind-on-tokens
  with **no scoped `<style>`**; the `<script>` logic (step flow, validation,
  server-side availability, account creation, one-time recovery code, optional
  2FA enrolment) is unchanged. Visuals: blurred orbs + live 24 h clock, a
  520 px white card with the brand tile, DSM step dots (`setup-stepper-*`,
  `class:done`/`class:current`) + gradient progress bar, welcome step with 3
  feature cards, account step (centered avatar + username/fullname fields),
  password step (strength bars + requirement checklist), and a verification
  step with a summary card (avatar, username, admin badge, availability +
  masked password rows) before the emerald "create account" CTA. After
  creation the mandatory **amber recovery box** (`.hv-recovery-box`, code shown
  once, non-copyable, acknowledged via checkbox) then the optional **2FA box**
  (`.hv-2fa-box`: secret, otpauth link, code input, backup codes) appear in
  the same card.
  **Same field rules as Login**: native `<input>` kept for autofill; the DSM
  field look lives on the wrapper div (`fieldWrapperClass`), the inner input is
  transparent/borderless (`textInputClass`), and raw `input`/`button` visual
  utilities carry the trailing-`!` important modifier as belt-and-braces
  (`base.css`'s form-element defaults are in `@layer base` since 2026-09-15, so
  the utilities win on their own — the `!` is kept for consistency with the
  still-unlayered headings). Footer CTAs stay `<Button variant="secondary|primary">`
  (`hv-btn-secondary` / `hv-btn-primary` hooks preserved) restyled with the
  DSM gradients. **Verified live in a real browser** (mocked first-run API):
  step 0→3 walkthrough, strength/checklist states, availability row, recovery
  + 2FA screens, and responsive fit down to 360 px. `setup.spec.ts` was
  updated to correct broken selectors (stray `]2`/`]B` suffixes and `hv-alert-*`
  used as tag selectors → `.hv-alert-*` classes) and test 19.1 now walks the
  mandatory recovery step before 2FA.

## API client (`frontend/src/lib/services/api.ts`)

A single `ApiClient` class exported as `api`. Responsibilities:

- Injects `X-Session-Token` and `X-CSRF-Token` headers (from localStorage).
- Per-method GET caching with TTL + LRU/size eviction (`saveToCache`/`getFromCache`,
  default TTL 5 s, max 100 entries).
- `invalidateForMutation(endpoint)` on mutations: evicts the endpoint's own
  GET keys — and, after a `/settings/<group>` write, every `/settings` key too,
  because the aggregate `GET:/settings` that seeds the Settings window lives
  under its own key (otherwise a close/reopen right after a save re-seeds from
  pre-save data, E2E 15.2).
- `subscribeToProgress(jobId, …)` opens an `EventSource` to `/progress/{job_id}`.
- Methods grouped: auth, users, groups, docker, dashboard, network, vpn, rclone,
  syncthing, explorer, rescue, shares, restic, downloader, settings, security,
  widgets, terminal, avatars, server, encryption (`getEncryptedVolumes`,
  `createEncryptedVolume`, `unlockEncryptedVolume`, `lockEncryptedVolume`,
  `deleteEncryptedVolume`), notifications (`getNotifications`,
  `getUnreadNotificationCount`, `markNotificationRead`,
  `markAllNotificationsRead`, `deleteNotification`). The Docker app catalog
  (`GET/POST/DELETE /docker/catalog*`) is called directly from `Docker.svelte`'s
  own local `authedFetch` helper rather than through `api.ts`, consistent with
  how the rest of `Docker.svelte` already calls Docker/Compose endpoints — see
  [guides/docker.md](./guides/docker.md).

Import pattern: `import { api } from '$lib/services/api';` then
`await api.getUsers()`.

## Stores (`frontend/src/lib/stores/`)

| File | Responsibility |
|------|----------------|
| `auth.ts` | current user, session token, login/logout |
| `i18n.ts` | active locale, `t(key)` lookup, translation map |
| `settingsDraft.svelte.ts` | Settings draft state (per-group baseline + dirty tracking + `saveAll()`); also holds the re-hydrated theme/accent |
| `activityStore.ts` | activity taskbar state |
| `weatherDisplayPrefs.ts` | weather widget display preferences |
| `notifications.ts` | notification center |
| `windowManager.ts` | open windows, focus/minimize/maximize, sizes |
| `monitorStore.ts` | desktop monitor-tile catalog (Resource/RAID/Security/VPN/Downloader/Syncthing/Calendar/Docker/Weather/Sessions/Health) + per-tile visibility + `refreshMs`, all persisted to the `/api/monitor-layout` blob |
| `widgetCatalog.ts` | `WIDGET_DEFS` — the 11 monitors (icon, accent, i18n keys, visibility key + setter), shared by `Widgets.svelte` and Settings > Widgets |
| `weatherLocation.ts` | the Weather monitor's saved city (localStorage) |
| `datetimeStore.ts`, `search.ts`, `wallpaper.ts`, `dragState.ts`, `explorerShareIntent.ts` | misc UI state |

Widget polling infra (`subscribeWidget` / `createWidgetPolling`) lives in
`components/desktop/widgetData.ts`, not in `stores/`. Toasts go through
`svelte-sonner` (`toast()` + the `Toast` component mounted in `App.svelte`).
Dashboard metrics are fetched directly via `api.ts`. There is no
`stores/theme.ts`, `stores/toast.ts`, `stores/widgetData.ts` or
`stores/dashboard.ts`.

(`desktopWidgets.ts`, `DesktopWidget.svelte`, `widgetTheme.ts`,
`WallpaperPicker.svelte` were removed — see [Widgets](./guides/widgets.md) and
[Wallpapers](./guides/wallpapers.md).)

## Shared components (`frontend/src/lib/components/`)

- `theme/` — design-system primitives + `index.ts` barrel export. The **used
  core** (canonical for new screens): `Spinner`, `Icon`, `Modal`, `Button`,
  `Badge`, `PathField` + `FolderPicker` (modal path picker) /
  `PathPickerField` + `FloatingPathPicker` (non-modal, draggable, portalled to
  `<body>`; locations sidebar + list/grid toggle; `mode="folder"|"file"` — the
  picker for new code), `UserAvatar`,
  `Alert`, `Toggle`, `Select`, `Input` (+ `WindowHeader` from
  `desktop/`), plus `Toast` (mounted once in `App.svelte`; `toast()` from
  `svelte-sonner`) and `Drawer` (bits-ui `Dialog` mobile bottom-sheet). The
  interactive ones (`Button`, `Badge`, `Spinner`, `Modal`,
  `Toggle`, `Alert`, `Input`, `Checkbox`, `Radio`) wrap the shadcn-svelte
  components in `ui/` (Melt/bits-ui) — see "shadcn-svelte + Melt UI" above.
  - **`Toggle` vs `Switch`** — two switches, deliberately:
    `Toggle` is a bits-ui `role="switch"` **button** (label rendered as a
    sibling span) and is the general-purpose one (Shares, Docker, Restic,
    Storage); `Switch` is a visually-hidden native `<input type="checkbox">`
    inside a `<label class="settings-toggle-label">`, for places that need real
    `input` semantics. Pick `Switch` only when something reads the checkbox
    itself. **The Settings screen is entirely on `Switch`** (sub-cards
    included — its E2E suite drives `.settings-toggle-label` +
    `input[type=checkbox]` via `flipSwitch()`, and the design fixes one toggle
    geometry screen-wide); do not put a `Toggle` back there.
- `ui/` — the generated shadcn-svelte components (`components/ui/{button,badge,
  dialog,input,checkbox,radio-group,switch,alert,skeleton,tooltip,select,
  separator,spinner,accordion,calendar,collapsible,command,context-menu,
  dropdown-menu,input-group,pagination,popover,progress,scroll-area,slider,
  sonner,tabs,textarea,toggle,toggle-group}` + the composed `date-picker`),
  copy-paste sources built on Melt UI / bits-ui. Import
  individual modules (`$lib/components/ui/button/index.js`), never a barrel
  that pulls everything in.
- `charts/` — **LayerChart wrappers** (2026-09-13) themed with `--hv-*` tokens:
  `DonutChart`, `BarsChart`, `TrendLineChart`. Used by the Dashboard CPU
  per-core panel; barrel export via `index.ts`.
- `app/` — **page composites** (2026-09-12) that standardize the page anatomy
  and kill per-app CSS reinvention: `PageLayout` (WindowHeader + sidebar/
  toolbar/footer + scrolling body), `StatCard`, `DataTable`, `SectionCard`
  (`title`/`titleSnippet`/`description`/`actions` + the optional `divider` prop
  — header hairline + compact body rhythm — added 2026-09-14 for stacked card
  lists such as the Users detail pane; a card's height is always its content),
  `EmptyState`, `FormRow`. New screens are built from these.
- `desktop/` — `WindowFrame`, `WindowHeader` (the one standardized in-window
  header — **icon tile + title only**: no `subtitle` prop since the 2026-09-14
  standardization, and since 2026-09-17 **no app fills its `center` / `extra` /
  `actions` / `actionLabel` slots** either, so the bar carries the 28px tile and
  the title and nothing else — page-level actions go in a `WindowToolbar` row
  under it. The props stay for the frame-provided header; E2E `14.11` fails any
  app that refills one. The tile is `--hv-winhdr-icon-size` = `--hv-winctrl-size`,
  the same 28px as the min/max/close trio, on one shared centre line; the tile
  glyph is **16px, owned by `.hv-winhdr-icon :global(svg)`** — do not pass
  `size` on an icon snippet, the CSS clamp wins and the prop is dead noise,
  while `stroke` is not clamped and must stay),
  `WindowToolbar`
  (the standard action row under the header — `left` = search/filters/
  breadcrumbs, `right` = primary buttons; reached from `PageLayout`'s
  `toolbarLeft` / `toolbarRight` snippets),
  `WindowSidebar`
  (icon-rail collapse via `collapseable`/`collapsed`/`onToggle`, published as
  the `hvSidebarCollapsed` context) with its standard `WindowSidebarItem`
  (icon + label + count + detail + trailing action, icon-only when collapsed;
  the opt-in `variant="rich"` adds a two-line `subtitle` and a per-item
  `accent` colour tile — see [Settings](./guides/settings.md) — leaving every
  other consumer's markup untouched)
  and `WindowSidebarSection` (uppercase micro-label, hidden when collapsed),
  `WindowToolbar`, `WindowFooter`, `WidgetHeader`, `HvLaunchpad`, monitor-tile
  components. (`Wallpaper*`, `AppHeader`, `MonitorHeader` removed.)
- `settings/` — `vocabulary.ts` (the shared section/card/row/button class
  strings) + `SettingsCard.svelte` (the shell every sub-card is built on) +
  the embedded cards `AntivirusCard`, `AntiRansomwareCard`, `FirewallCard`,
  `UpsCard`, `SnmpCard`, `TlsCard`.
  (The old sidebar-Settings layout files were removed — `Settings.svelte` is a
  single component with a left category rail.)
- `storage/`, `docker/`, `vm/`, `search/` — feature-specific components.
  `search/` (`SearchBar`, `SearchResults`, `SearchItem`) backs the desktop
  command palette and is i18n-driven via the `[search]` locale table.
- `ErrorBoundary.svelte`, `LazyLoad.svelte`, `NotificationCenter.svelte`.

## Data freshness — no manual "Refresh" buttons (2026-09-08)

App windows have **no manual Refresh button**. `lib/utils/autoRevalidate.ts`
→ `onAppVisible(loadFn, { pollMs? })` re-runs a screen's load function when the
window/tab regains attention (`visibilitychange` → visible, `window` `focus`),
with a 1.5s cooldown to coalesce that pair; the `api.ts` 5s response cache
handles longer-term de-duplication. Wire it as an `onMount` return / `$effect`
teardown. Pass `pollMs` (~45s) for genuinely live screens (Docker, Health).

- Removed from: Users, Groups, Shares, Health, Docker, Restic (had no auto
  refresh — now use `onAppVisible`); Storage (extended — also `onAppVisible`);
  Journal, Syncthing, Dashboard, Network (already poll on an interval);
  Explorer (live filesystem watcher + `onVisibilityChange` now also re-lists
  the current directory).
- Still present, and correct — these are **actions**, not a page refresh:
  regenerate avatar (Account), regenerate response (Chat), retry connection
  (Login / Setup Wizard), rebuild partition table / repair RAID (Rescue),
  restart container / service (Docker / Shares), rescan folder (Syncthing),
  sync SFTP access / permissions (Shares), sync app store (Docker), re-query
  Ollama models (Settings → AI).

## i18n usage

Components call the `t` helper from `stores/i18n.ts` with dotted keys
(`t('users.title')`). The translation map is loaded from
`GET /translations` (mounted at `/api/translations`). See [i18n](./I18N.md).

## Build & check

```bash
cd frontend
npm install
npm run dev      # Vite dev server
npm run build    # production build → static/dist
npm run check    # svelte-check type checking
```

## Related docs

- [Architecture](./ARCHITECTURE.md)
- [API Contract](./API.md)
- [Internationalization](./I18N.md)

# HomeVault Design System

The **single source of truth** for the HomeVault visual language. The product
has exactly one design identity — "HomeVault Appliance" (2026-09-12) — which
replaces every earlier skin ("Fluent Paper", "Frosted Glass Modernism", "Clean
Paper & Slate"). Nothing in the codebase may reference another framework's
design language (Fluent, Material, …) as an authority: tokens, components and
pages all read from the `--hv-*` tokens declared in
`frontend/src/lib/styles/theme/tokens.css`, which this document describes.

> Superseded specs: `DESIGN stich.md` at the repo root described the earlier
> "Frosted Glass Modernism" direction (`#2E72C9`, translucent blur surfaces). It
> is **archived, not authoritative** — the product now ships the opaque
> Appliance identity below.

## 1. Identity

HomeVault Appliance is the visual language of a **serious NAS management
console** (reference mood: the approved appliance-console mocks). It is:

- **Opaque and light** — solid paper surfaces, crisp 1px hairlines, layered
  soft shadows. No translucent/blur "glass" anywhere in-window (a dark photo
  wallpaper must never wash out ink text). Depth comes from shadow, never from
  backdrop-filter.
- **Light-only by design** — there is no dark theme and none is planned: no
  `prefers-color-scheme` / `data-theme` theming exists anywhere in
  `frontend/src`, and the stray `dark:` variants once shipped by the shadcn
  primitives under `components/ui/**` were stripped (UI audit W14, 2026-09).
  Ship one light theme and be excellent at it rather than a half-tested
  second palette; never add a `dark:` variant.
- **One brand accent** — a single professional blue ("HomeVault Blue"
  `#1E62AE`) reserved for interactive elements, links, focus rings and
  selection. Every app icon tile keeps its own gradient (see
  [App icon gradients](#6-app-icon-gradients)) but the *chrome* is monochrome
  blue-on-grey.
- **Dense but legible** — this is an information-dense admin product (tables,
  forms, live metrics). Compact type, tight rows, generous touch targets
  (≥44px).
- **Precise** — small radii (controls 6–8px, cards 12–16px, windows 16px),
  restrained motion, no decorative effects.

## 2. Tokens — the single source of truth

`tokens.css` declares the whole vocabulary under the `--hv-` prefix. Token
categories:

| Category | Example tokens | Notes |
|---|---|---|
| Brand ramp | `--hv-primary(-hover/-active)`, `--hv-primary-50…900` | HomeVault Blue, light → dark |
| Neutral ramp | `--hv-neutral-50…900`, `--hv-text-primary/secondary/tertiary` | cool blue-greys |
| Semantic | `--hv-success`, `--hv-warning`, `--hv-error`, `--hv-info` | status hues + light bg/border/text variants |
| Surfaces | `--hv-surface*`, `--hv-shell-glass/card/fill/border*` | two aligned vocabularies (legacy `surface*` + shell) |
| Typography | `--hv-font-size-*` (content), `--hv-text-*` (compact chrome), `--hv-font-weight-*`, `--hv-line-height-*` | two deliberate scales, see below |
| Spacing | `--hv-space-0-5 … 24` | 4px base grid |
| Radius | `--hv-radius-sm … full`, `--hv-radius-card`, `--hv-radius-control` | appliance stops, see below |
| Shadows | `--hv-shadow-sm … 2xl`, `--hv-shell-shadow-card` | layered low-alpha black |
| Elevation/z-index | `--hv-z-*` | 1000-point increments |
| Motion | `--hv-transition*`, `--hv-ease-*`, `--hv-chrome-*` | transform/opacity only |
| Desktop chrome | `--hv-chrome-*`, `--hv-window-*`, `--hv-winhdr-*`, `--hv-dock-*` | shell geometry |
| Login gateway | `--hv-login-*` (bg gradient, orbs, accent, field, timer) + `--hv-wizard-*` (recovery box, create CTA) | scoped gateway palette, login + setup wizard only — see §6.1/§6.2 |
| Settings screen | `--hv-settings-*` (slate ramp, tile/canvas/hairline/border, cat tints + 8 `--hv-settings-cat-*` accents, toggle geometry) | scoped to `routes/Settings.svelte` + `components/settings/` only — see §6.3 |

### 2.1 The two type scales (do not merge)

- `--hv-font-size-*` — content scale (body, headings, form labels).
- `--hv-text-*` — compact chrome scale (table cells, chips, meta rows), one
  step smaller.

Pick by role, never by name similarity. Merging them resizes ~16 files.

**Role tokens** (`--hv-text-stat`, `--hv-text-card-title`, `--hv-text-label`)
cover three recurring sizes that used to drift as raw literals (stat-card
numbers, card/section titles, form-field labels); they map onto the scales
above. `--hv-font-size-5xl` (62px) is the gauge/hero display step.

**Applying a size/family token in markup (UI audit W1, 2026-09-23).** Sizes go
through the **one** Tailwind bridge declared in `app.css` → `@theme inline`:

- `text-*` — **content scale** (`--hv-font-size-*`: `text-micro` 11,
  `text-xs` 11, `text-sm` 12, `text-base` 14, `text-md` 16, `text-lg` 18,
  `text-xl` 20, `text-2xl` 24, `text-3xl` 28, `text-4xl` 30, `text-5xl` 62) —
  body copy, headings, form labels.
- `text-c-*` — **compact chrome scale** (`--hv-text-*`: `text-c-xs` 11,
  `text-c-sm` 12, `text-c-base` 13, `text-c-md` 14, `text-c-lg` 16,
  `text-c-xl` 18, `text-c-2xl` 20, `text-c-3xl` 28) plus the role steps
  `text-c-stat` 20, `text-c-card-title` 16, `text-c-label` 13 — table cells,
  chips, meta rows.

**Never write `text-[length:var(--hv-…)]`** — that spelling (and the whole
`text-[length:` family) is banned; all 520+ occurrences were migrated to
the utilities above. The [D1 style gate](../design/ui-audit-tasklist.md) is
the planned automated enforcement of this rule — it does not exist yet, so
until it lands the ban binds by review, not by CI. A *new* size token therefore needs a `--text-*` /
`--text-c-*` line in `@theme inline` in the same change that adds it. Font
families keep the hint (`font-[family-name:var(--hv-…)]` or the paren
shorthand `font-(family-name:--hv-…)`), because an un-hinted `font-[var(…)]`
is read as a **font-weight**.

The un-hinted `text-[var(--hv-…)]` spelling stays correct for **colour**
tokens only (`--hv-text-primary/secondary/tertiary`, `--hv-shell-text-*`).
That matters because **`--hv-text-*` is overloaded**: it holds colours
(`--hv-text-primary` = `#1f2733`) *and* sizes (`--hv-text-xs` = `11px`), so
the same un-hinted spelling is correct for one and silently wrong for the
other — it compiles to a `color:` declaration (or, when the value isn't a
valid colour, nothing at all). The 2026-09-12 typography audit's claim to have
"fixed every existing occurrence" did not hold for exactly this reason: three
size usages survived in `components/settings/vocabulary.ts` and the Settings
rail header, rendering at the inherited 14px instead of 11px — found on
2026-09-17 by an E2E failure (`settings.spec.ts` 23.5), not by grep. When you
suspect this class of bug, check the **compiled** CSS, not the source:

```bash
grep -o '\.text-\\\[var(--hv-text-xs)\\\]{[^}]*}' static/dist/assets/*.css   # → {color:…}  WRONG for a size
grep -o '\.text-c-xs{[^}]*}' static/dist/assets/*.css                        # → {font-size:var(--hv-text-xs)}  RIGHT
grep -o '\.text-\\\[length[^{]*' static/dist/assets/*.css                    # → must be empty (W1 closed)
```

The legacy `--text-*` aliases that used to sit in tokens.css's unlayered
`:root` block (they beat Tailwind's `@layer theme` by cascade accident) were
removed in the same change — the bridge above is the only declaration site.

## 3. The Appliance palette

| Role | Token | Value |
|---|---|---|
| Brand (base) | `--hv-primary` | `#1E62AE` |
| Brand hover | `--hv-primary-hover` | `#18508D` |
| Brand pressed | `--hv-primary-active` | `#0E2F54` |
| Ink (primary text) | `--hv-text-primary` / `--hv-shell-text-strong` | `#1F2733` |
| Ink (secondary) | `--hv-text-secondary` / `--hv-shell-text-2` | `#454F5B` |
| Ink (muted) | `--hv-text-tertiary` / `--hv-shell-text` | `#69747F` |
| Placeholder | `--hv-shell-text-3` | `#909AA6` |
| Canvas (window surface) | `--hv-window-bg` (frame) / `--hv-shell-glass` | `#FFFFFF` |
| Card surface | `--hv-shell-card` / `--hv-surface-container-low` | `#F7F8FA` |
| Fill (toolbar/chip) | `--hv-shell-fill` / `--hv-surface-container` | `#EFF1F4` |
| Hairline soft | `--hv-shell-border-soft` | `#EDF0F4` |
| Hairline | `--hv-shell-border` / `--hv-border` | `#DCE1E7` |
| Hairline strong (outline) | `--hv-shell-border-strong` / `--hv-border-hover` | `#B9C2CC` |
| Success | `--hv-success` | `#1E7E34` |
| Warning | `--hv-warning` | `#B45309` |
| Error | `--hv-error` | `#C22B21` |
| Info | `--hv-info` | `= --hv-primary` |

### 3.1 Interaction states

The four interaction states are tokens, never ad-hoc fills:

- `--hv-subtle-hover` / `--hv-subtle-pressed` — transparent icon/text buttons.
- `--hv-card-hover` / `--hv-card-pressed` — cards.
- `--hv-stroke-hover` / `--hv-stroke-pressed` — outlined controls.
- Focus ring: brand glow `--hv-focus-ring` — a **2px solid `#1E62AE` ring
  (≈4.4:1 on white) + a 4px halo**, not the old 10%-alpha-only glow
  (≈1.1:1, failed WCAG 2.4.7 / 1.4.11). A component that suppresses its
  outline (`outline: none` on `:focus-visible`) must keep the box-shadow
  ring — never a bare border-colour change with no ring.

### 3.2 Radius stops (appliance)

| Stop | Value | Usage |
|---|---|---|
| `--hv-radius-sm` | 4px | inline badges, tiny chips |
| `--hv-radius-default` | 6px | inputs, buttons, selects |
| `--hv-radius-md` | 8px | small cards, list items |
| `--hv-radius-lg` | 10px | default card corners |
| `--hv-radius-xl` | 12px | large cards, sections |
| `--hv-radius-2xl` | 16px | window corners, large surfaces |
| `--hv-radius-3xl` | 20px | hero surfaces |
| `--hv-radius-4xl` | 24px | login/setup cards |
| `--hv-radius-full` | 9999px | pills, circular icon buttons |
| `--hv-radius-card` | 12px | semantic card corner |
| `--hv-radius-control` | 8px | interactive control corner |

## 4. Component architecture — one library, three layers

Every screen is built from the same three layers. **Nothing is written
per-app.**

| Layer | Location | Contents |
|---|---|---|
| Primitives | `lib/components/theme/` | `Button`, `Input`, `Select`, `Toggle`, `Checkbox`, `Radio`, `Alert`, `Badge`, `Spinner`, `Modal`, `Toast`, `Drawer`, `Icon`, `UserAvatar`, `PathField`/`FolderPicker`/`PathPickerField`/`FloatingPathPicker`. Barrel export via `index.ts`. |
| Framework | `lib/components/ui/` | shadcn-svelte / Melt UI generated components (`button`, `dialog`, `input`, `select`, `switch`, `checkbox`, `radio-group`, `alert`, `badge`, `spinner`, `accordion`, `calendar`, `collapsible`, `command`, `context-menu`, `date-picker`, `dropdown-menu`, `input-group`, `pagination`, `popover`, `progress`, `scroll-area`, `separator`, `skeleton`, `slider`, `sonner`, `tabs`, `textarea`, `toggle`, `toggle-group`, `tooltip`). The theme primitives delegate to these. Import individual modules, never a barrel. |
| Charts | `lib/components/charts/` | LayerChart (Svelte 5 / d3) wrappers themed with `--hv-*` tokens: `DonutChart`, `BarsChart`, `TrendLineChart`. Barrel export via `index.ts`. |
| Page composites | `lib/components/app/` | `PageLayout`, `StatCard`, `DataTable`, `SectionCard`, `EmptyState`, `FormRow` — the shared page anatomy that kills per-app reinvention. |
| Desktop shell | `lib/components/desktop/` | `WindowFrame`, `WindowHeader`, `WindowSidebar`, `WindowToolbar`, `WindowFooter`, `HvLaunchpad`. |

### 4.1 Page anatomy (PageLayout)

Every in-window app renders exactly one `<PageLayout>`:

```
<PageLayout title="…" data-testid="…-page">
  {#snippet icon()}<IconX />{/snippet}
  {#snippet toolbarRight()}          <!-- page-level actions, under the header -->
    <Button variant="primary" onclick={…}>…</Button>
  {/snippet}

  <SectionCard title="…">…</SectionCard>   <!-- body content -->
</PageLayout>
```

It composes `WindowHeader` (flush, flex-shrink:0) + an optional
`WindowSidebar` / `WindowToolbar` / `WindowFooter` + a `flex:1; min-height:0;
overflow-y:auto` body that carries the page padding
(`--hv-window-content-padding`).

The header bar is **[APP ICON] [WINDOW TITLE] and nothing else** (2026-09-17;
the 2026-09-14 pass had already removed the `subtitle` prop). The bar itself —
`.hv-winhdr`, the element carrying `[data-hv-drag]` — holds exactly two things:
the 28px icon tile and the title. The other two pieces of the header's *visual*
composition live outside it: the optional tab row sits in the wrap *below* the
bar, and the min/max/close trio is a **sibling** of the bar (owned by
`WindowFrame`, absolutely positioned over the window's top-right) — not a child
of it. The bar keeps a `--hv-winhdr-btn-clear` (120px) right reserve for the
trio, and that slot must stay empty.

Concretely, inside `.hv-winhdr` there must be **no** `.hv-winhdr-actions` and
**no** `.hv-winhdr-center`, and no interactive element at all
(`button`/`input`/`select`/`textarea`/`a`/`label`). That rules out, on every
screen:

- action buttons — use a `WindowToolbar` row under the header
  (`PageLayout`'s `toolbarLeft` / `toolbarRight` snippets), or the screen's own
  in-body toolbar when it already has one (Explorer's file toolbar);
- status pills / badges / connection indicators — `WindowToolbar` row, or the
  rail footer when they belong to a single control (Settings has no pill at
  all: its one rail-footer `SettingsSaveButton` carries the save state in its
  colour **and** label, with an sr-only `role="status"` line for screen
  readers);
- checkboxes, filters, search fields — the toolbar's left cluster.

The `WindowHeader` props `center`, `extra`, `actions`, `actionIcon` /
`actionLabel` / `onAction` still exist for the frame-provided header and for
future needs, but **no app in the registry uses them**: E2E `14.11` walks every
app id in `OWN_HEADER_APPS` and fails any bar that refills one of those slots or
hand-rolls a control into the bar. A page-level action belongs under the header,
where it can't push the title off the bar's centre line or collide with the
control trio's reserve. The icon tile and the trio are both
`--hv-winctrl-size` (28px) on one shared vertical axis, so the bar renders at
exactly `--hv-winhdr-h` (44px). `tabsLabel` / `sidebarLabel` (accessible
names) are not subtitles and stay.

`SectionCard` — the standard titled card:

- `title` (plain) or `titleSnippet` (icon + heading, e.g.
  `<h2 class="…text-c-card-title…">`), `description`,
  `actions`, `class`, `data-testid`.
- **`divider?: boolean` (default `false`, added 2026-09-14)** — draws a hairline
  (`--hv-shell-border-soft`) under the header and switches the body to the
  compact rhythm (`pt-3` instead of `pt-1`). Use it for stacked card lists
  where the header/body boundary needs to read (the Users detail pane's six
  cards). Leave it off for a lone card, where the hairline is just noise.
- A card is **never given a fixed height** — its height is its content. Stack
  cards in a `flex flex-col gap-3` inside the scrolling body; that is what makes
  a card grow when a screen has more to show than the mockup it came from.
- **Filling a grid cell** — a card that must stretch its body (bento cells with
  a pinned footer row) targets the body element directly, e.g.
  `class="flex flex-col [&>.hv-section-card-body]:flex [&>.hv-section-card-body]:flex-1 [&>.hv-section-card-body]:flex-col"`
  (Dashboard CPU/Memory; Health swaps `flex-1` for `items-center`). Never
  re-add a per-page card wrapper just to get this behaviour.
- **Adoption (UI audit W2, 2026-09-24)** — the per-app card constants
  (`statCard`, `sectionCard`, `emptyState`, `emptyCard`, Dashboard/Restic
  `card`) are deleted: metrics render through `StatCard`, empty states through
  `EmptyState`, panels through `SectionCard`. See §8 for the counts and the
  three deliberate skips (VM's `h2` E2E contract, Dashboard's stat-tile child
  hooks, one-line placeholders).

### 4.1.1 Sidebar rail (WindowSidebar + Item + Section)

The left rail is built from three shared components (`desktop/`):

- `WindowSidebar` — the rail container: fill background + right hairline,
  256px (`--hv-window-sidebar-w`). **Icon-rail collapse** (Aurora-inspired,
  2026-09-13): when `collapseable` a chevron narrows the rail to 64px
  (`--hv-window-sidebar-w-icon`) instead of hiding it. The state is a
  bindable `collapsed` prop (`bind:sidebarCollapsed` on `PageLayout`) and is
  published to the rail's children as the `hvSidebarCollapsed` context (a
  `$state` object — never a bare prop, which `setContext` would freeze).
  Rails whose content can't shrink to icons (e.g. Docker's metrics cards)
  simply omit `collapseable`.
- `WindowSidebarItem` — the one standard nav row: `icon`/`iconName` +
  `label` + optional `count` badge / `detail` text / trailing `action`
  snippet (rendered as a *sibling* of the row button, never nested) and a
  `dropActive` highlight for drag-and-drop destinations. Active state uses
  the shared `is-active` slot. When the rail is collapsed it renders
  icon-only and moves the label into a `title` tooltip.
  - **`variant="rich"` (2026-09-14, opt-in)** — adds a second label line
    (`subtitle`) and a per-item `accent` colour, plus a trailing 4x20 pill on
    the active row. The accent arrives as `--hv-row-accent`; the tile is
    `color-mix(… var(--hv-settings-cat-tint) …, transparent)` at rest and
    **solid with a white glyph** when active, so the accent can't be an
    invisible accent-on-accent. Used only by `Settings`; the other consumers
    are byte-identical without it.
  - **Rail type/pacing is fully tokenised** (2026-09-15): the rail is compact
    chrome, so label/subtitle/header use the compact scale
    (`--hv-text-sm` label, `--hv-text-xs` subtitle + rail header, the latter
    with `--hv-letter-spacing-wider` + `--hv-line-height-normal`), and the
    item/footer paddings + gaps resolve through the Tailwind spacing scale
    (`calc(var(--spacing) * …)` — `px-3`-equivalents, no off-rhythm `*.5`
    stops since UI audit W3). No px literals
    remain in the rich-item / footer geometry of `WindowSidebar.svelte` /
    `WindowSidebarItem.svelte` (the collapse-chevron toggle padding and the
    `--hv-winsidebar-title` margins are out of this scope).
- `WindowSidebarSection` — the standard grouped heading: the uppercase
  `.hv-winsidebar-title` micro-label above its children, hidden on a
  collapsed rail. `label` may be omitted for unlabelled groups (e.g. Trash).

Wired into `Explorer`, `Settings` (208px rich two-line rail — no
`collapseable`: an icon-only rail cannot show the design's subtitles) and
`Docker` (tab nav + metrics section). The **status footer** (`WindowFooter
mode="status"` — the Puter `explorer-footer` pattern: live counts/state
stripped of chrome) is applied to the data-dense screens: `Explorer`
(folder summary + selection), `Syncthing` (folders/devices/connected),
`Storage` (disks/virtual/partitions/removable), `Shares` (per-protocol
counts), `Downloader` (active/completed), `Network` (interfaces/link/IP).
See `docs/guides/explorer.md`.

Every multi-section screen uses the **single WindowHeader tab row**
(`PageLayout` `tabs`/`activeTab`) rather than a hand-rolled strip — `Restic`'s
repo sections (snapshots/jobs/schedules, with counts) moved onto it in the
same 2026-09-13 pass, joining `Storage`, `Syncthing`, `Network`, `Rescue`,
`CloudSync`, `VPN`.

**Master–detail 2-pane** (Aurora Mail pattern, `Mail.tsx:593-800`) — object
screens with a list + a detail pane keep both mounted side by side instead
of swapping views: `VirtualMachines` (machine list left + the former overlay
drawer now an inline 420px right column), `Restic` (repo cards left + repo
detail/snapshots-jobs-schedules right) and `Syncthing` (devices/folders
tabs: compact list left + detail pane right) are the 2026-09-13
references; `Groups` already was one, and `Users` joined on 2026-09-14
(account rail + six `SectionCard` cards, replacing its tile grid + stacked
sheet — see [users.md](./users.md)). The split collapses to one pane via a
**ResizeObserver on the split container** (the windows float, so a viewport
media query would lie) — below 720–760px of *content* width the list rail is
removed from the DOM while a detail is open (back/close button returns;
Escape still closes the VM drawer). Measure the content column, not the
window: on `Users` the rail (256px + 1px) and `<main>`'s `p-6` (48px) are
already subtracted, so its 760px threshold lands at roughly 1065px of window
width. `Downloader` follows the same pattern for its task list (the
detail re-resolves the selected task by gid on every poll, so live
progress/speed stay fresh instead of a stale snapshot).

### 4.1.2 Loading states and the data table (UI audit W11/W12, 2026-09-24)

- **Skeletons, not spinners (W11).** A screen's loading branch renders
  shimmer rows built from the shared `.hv-skeleton-text` /
  `.hv-skeleton-rect` / `.hv-skeleton-circle` utilities (`utilities.css`,
  backed by `--hv-skeleton-bg` + `--hv-skeleton-shimmer`) instead of a
  `Spinner` / `IconLoader`. The wrapper keeps its `data-testid`
  (`journal-loading`, `storage-loading`, `docker-empty`,
  `explorer-state-message` — the E2E contracts), gets `role="status"` +
  one sr-only label reusing an existing `*.loading` i18n key, and the
  bars themselves are `aria-hidden="true"`. Bar widths come from a fixed
  formula (`45 + ((i * 31 + j * 17) % 36) %`) so skeletons shimmer but
  never jump between renders. Adopted on `Journal` (entry list),
  `Storage` (stat grid + list), `Docker` (hub grid, stores list, catalog
  grid) and `Explorer` (sidebar + both file-area states), plus
  `DataTable` below. A spinner that lives *inside the control owning the
  action* (Storage's per-row refresh, Explorer's search/upload feedback)
  stays a spinner. Nothing renders `animate-spin` inside
  `explorer-file-area`: E2E pins that count to 0 after load, and
  skeletons satisfy it by construction.
- **`DataTable` is the one table (`app/DataTable.svelte`, W12).** Its
  anatomy: a two-layer wrapper — `hv-datatable w-full overflow-hidden`
  (the card, carrying the caller's radius/border/background) around a
  `w-full overflow-auto max-h-[75vh]` scroller — so horizontal overflow
  never reaches the window frame (E2E asserts the window itself has no
  scrollbar), a sticky header (`sticky top-0`, `bg-card`, the hairline
  drawn as an inset `box-shadow` on each `<th>` because collapsed
  borders don't stick, first header cell `z-20`), a frozen first body
  column (`sticky left-0 z-[5] bg-card`, row hover mirrored through
  `tr.group` + `group-hover:bg-[var(--hv-subtle-hover)]`), and a pager
  rendered **outside** the scroller. Column features: every column with
  a non-empty `label` is sortable unless `sortable: false` — a real
  `<button>` with `aria-sort` on its `<th>`, one primary caret when
  active, a faint stacked pair when idle,
  `min-h-[var(--hv-touch-target-min)]`, and `sortId = null` initially so
  the caller's own order stays the default (Quotas'
  users-before-groups); numeric columns (raw `number` values)
  right-align automatically unless `align` says otherwise; `loading`
  draws `loadingRows` (default 5) skeleton rows with `aria-busy` on
  `<tbody>` plus an sr-only `role="status"` line — no extra `tbody tr`,
  so row-count assertions stay clean; pagination (100 rows per page)
  engages only past 300 rows, shows `{from}–{to} / {total}` labelled by
  `common.pagination_nav` with `common.previous` / `common.next`, and
  clamps the page when rows shrink. The component no longer imports
  `Spinner`. The seven route-local hand-rolled `<table>`s (CloudSync
  ×2, Rescue, Users, Restic ×3) migrate to it in a follow-up pass gated
  on `npm run check` + `npm run build` + their E2E suites.

### 4.2 Styling policy — Tailwind on tokens, no hand-rolled per-page CSS

- **New components and screens** use Tailwind utilities (wired through
  `app.css` `@theme inline` → `--hv-*` bridge) + the shared components.
- **No hardcoded values**: a color, radius, shadow, spacing or z-index in a
  page/component must be a `var(--hv-*)` (or a Tailwind token like
  `bg-primary`, `text-muted-foreground` that resolves to one).
- **No `var()` fallbacks.** Every `--hv-*` token is defined at `:root`, so
  `var(--hv-text-primary, #191c1e)` never renders its second argument — the
  fallback only rots (one stale fallback in the tree would have shipped 2.54:1
  contrast if it ever did). Write `var(--hv-text-primary)` alone; UI audit W4
  stripped every existing fallback (svelte/ts/css, 2026-09).
- **Responsive means container queries, never viewport queries (UI audit
  W15, 2026-09-24).** The windows float inside the viewport, so an `sm:` /
  `md:` variant measures the browser, not the window. The `@container`
  anchor sits on `PageLayout`'s two `hv-page-body` `<main>`s, on
  `dialog-content.svelte`, on the `SetupWizard` root, on the VM drawer
  `<aside>` and on Dashboard/Account/Settings page bodies; everything
  inside collapses with `@min-[…]` / `@max-[…]` variants against those
  anchors. 19 distinct thresholds, grep-verified: the nine `@min-`
  (grow) rules — `420px` ShareLinkModal two-column, `480px` Settings
  setting rows go inline, `540px` Raid type grid, `560px` Raid footer +
  Network cards, `640px` wizard hint + Users two-column, `700px`
  CloudSync summary, `768px` Users forms, `880px` stat grids 2→4
  columns, `900px` Docker metrics split — and the ten `@max-` (stack)
  rules spanning `320px`–`1200px`, e.g. `360px` form field pairs
  collapsing to one column (Docker, Restic, VPN, VM modals) and
  `768px`/`1200px` Dashboard widget-grid steps. The legacy viewport
  utilities (`hv-grid*`, `hv-sm/md/lg/xl`) are deleted — zero Tailwind
  viewport variants remain in routes. Two things keep plain `@media` on
  purpose: shell chrome that genuinely spans the browser viewport
  (topbar, dock, launchpad, `Desktop` backdrop), and `position: fixed`
  pickers/backdrops that portal to `<body>` and therefore sit outside any
  container — their viewport-relative sizing is a documented exception
  (the shared `FolderPicker` in Restic/Downloader/Explorer/Users, Docker's
  fixed URL-menu backdrop), not a licence to add `@container` inside them.
- **No per-app class prefixes** (`dk-*`, `db-*`, `restic-*`, `cs-*`, …). Any
  repeated visual must become a shared component or token.
- Scoped `<style>` blocks are tolerated only for a genuine one-off page layout
  and must reference tokens for every value. The long-term goal is to remove
  them entirely (see the migration checklist below).
- The legacy hand-rolled `hv-*` classes (`components.css`/`utilities.css`) stay
  until each screen migrates, but nothing new is written in them.

### 4.3 Buttons — one system, three entry points

There is exactly one button system, with three sanctioned entry points:

1. **Text action buttons → `<Button>`** (`components/theme/Button.svelte`).
   It wraps the shadcn primitive and emits the documented
   `.hv-btn hv-btn-primary|secondary|danger|ghost|…` classes. This is the
   standard way to get `hv-btn` — routes never hand-write `hv-btn-*` class
   strings.
2. **`.hv-btn` stays adopted (it is not retired).** `components.css` owns the
   shape/hover/focus/press states behind `<Button>`, `theme/FolderPicker` and
   the window error-recovery action use it directly, and E2E asserts on
   `.hv-btn-ghost` / `.hv-btn-primary`. Treat the raw classes as an internal
   detail of those components, not as an API for screens.
3. **Icon buttons and shell inputs → shared class constants.**
   `components/app/controlClasses.ts` (re-exported from `$lib/components/app`)
   defines the recurring hand-rolled shapes once: `iconBtn` /
   `iconBtnDanger` (36px compact), `tileBtn` (44px tile), `actionBtn` /
   `actionIconBtn` (h-10 toolbar action), `dangerHover`, `primaryBtn` /
   `ghostBtn` (h-11) and `textInput`. Compose with `cn()` for size or state
   overrides instead of re-declaring a per-screen copy (the audit-era
   per-route `iconBtn`/`act`/`vmBtn`/`actionBtn` duplicates are gone).

Whichever entry point you use: an icon-only `<button>` still needs an
accessible name (`aria-label` from an i18n key) and, below 44px, the shared
`.hv-ic` hit square (§7.1) — every icon constant above already carries it.

### 4.4 Icons — one family per layer (UI audit W6)

Every glyph has exactly one sanctioned source, chosen by the layer it
renders in:

- **App code (routes, `app/`, `desktop/`, `settings/`) → the shared
  sprite.** Render it with `theme/Icon.svelte` (`<Icon name="…">`), which
  resolves the name through `getIcon` in `lib/icons/mapping-unified.ts`
  (tabler first, phosphor fallback) and draws it via `<use href="#icon-id">`
  against the sprite sheet. This is the default for new code; existing
  route-local `@tabler/icons-svelte` imports draw the same tabler family
  and stay as they are.
- **`components/ui/**` (shadcn primitives) → `@lucide/svelte`, only there.**
  Lucide never leaves that directory; app code must not import it.
- **Inline `<svg>` → artwork only** — wallpapers, orbs and decorative
  scenes (Dashboard, Desktop, SetupWizard, Login) — never a functional
  glyph on a control.

A text character (`✓`, `✕`, `★`) is never an icon: use the layer's source
above. Whichever source you use, the accessible name lives on the *control*
(§7.1), not on the glyph (`<Icon>` renders `aria-hidden`).

## 5. Desktop shell

- Floating topbar pill + dock, both `--hv-shell-glass` opaque with the chrome
  material pair — `--hv-chrome-surface-border` (slate-300) + 
  `--hv-chrome-surface-shadow`. No backdrop blur, ever: a dark photo wallpaper
  must not wash the ink out of the chrome (E2E `14.3`).
- **Each pill owns its height** and the dock is deliberately the taller one
  (62px vs 84px): `--hv-chrome-topbar-h` / `--hv-chrome-dock-h`. Both are
  `box-sizing: border-box`, so the tokens are the rendered heights — which is
  what makes `--hv-chrome-topbar-keepout` / `--hv-chrome-dock-keepout` exact.
  Everything anchored to the dock (`.hv-activity-panel`, the ≤900px maximized
  inset) derives from the **dock's** band.
- Window corners: `--hv-window-radius` (16px). Maximized windows hide the
  topbar; on ≤900px they inset above the dock.
- **The app window is the "AppWindow" mock (2026-09-14, Figma "Sans titre"
  node `3:76`)** — the exact spec lives in `desktop.md` → *In-window anatomy*.
  In one line: an opaque white frame in a **slate-300** hairline
  (`--hv-window-border` `#CBD5E1`, deliberately darker than the `--hv-shell-*`
  card hairline) under the mock's own two-layer shadow, a constant **44px**
  header bar, and the amber / blue / red control trio **inline in that bar** at
  28px — no longer 48px circles floating over the content.
- **Chrome is Figma-inspired (approved gateway mocks) (2026-09-13, deepened 2026-09-14):** window
  controls are traffic lights (`--hv-winctrl-*`: amber minimize, blue
  maximize, red close) instead of neutral circles, on the mock's own stops
  (`#FFB900` / `#2B7FFF` / `#FF6467` — **not** the semantic
  `--hv-warning`/`--hv-primary`/`--hv-error`); the dock running indicator
  is a full-width bar in the app's solid hue (`--hv-app-<id>-c`,
  `.hv-dock-dot` + `--dock-c`), which must stay the dark stop of that app's
  `--hv-app-<id>-grad` (E2E `14.2`); topbar buttons are flat 44px chips on the
  pill — icon + label
  from 901px up, icon-only below (no tile / border / shadow, hover
  `--hv-subtle-hover`, `--hv-radius-xl`) — and
  the profile avatar is a 32px circle. Dock tiles are 48px /
  `--hv-radius-2xl` with a spring hover lift + `brightness(1.05)` and a
  CSS tooltip on `--hv-tooltip-bg` (suppressed under `@media (hover: none)`).
  Palette stays the HomeVault appliance stop set, with the chrome border/shadow
  as the one dedicated exception (see `desktop.md`).
- **Page-scoped palette `--hv-launcher-*`** (Launchpad, 2026-09-16, Figma
  "Sans titre" node `23:6646`). Like `--hv-settings-*` / `--hv-login-*` /
  `--hv-wizard-*`, it is read by exactly one component
  (`HvLaunchpad.svelte`). It exists because the launcher is the one surface
  that sits on a **dark translucent scrim** rather than opaque paper, so the
  `--hv-shell-text-*` ink set does not apply. Two mock values were contrast-
  corrected on the way in (scrim `0.55` → **`0.72`**, inactive chip text
  85% white → **opaque white**), both documented on the token block; the
  visible 32px chip keeps a 44px transparent `::after` pad and the search
  input is a full `--hv-touch-target-min` inside the mock's 56px bar. Its
  scrim is the one blurred surface in the shell, and it declares the standard
  `backdrop-filter` **alone** — pairing it with `-webkit-backdrop-filter` made
  the minifier keep only the prefixed form, which Firefox ignores (see
  `desktop.md`). Its chip row grows to fill the content column
  (`flex: 1 1 auto`) so the category module is never visibly narrower than the
  tile grid it filters; the ≤640px block undoes it for the phone's sideways
  scroller. The
  tiles are the per-app `--hv-app-<id>-grad`, never the mock's flat white —
  §6 is binding.
- The desktop wallpaper is a light blue-grey gradient (`--hv-desktop-bg`) by
  default; a photo wallpaper may be chosen in Settings → Appearance.
- Every dock/topbar/launcher control is ≥44px; `:hover` lifts flatten under
  `@media (hover: none)`.

### 5.1 Form controls stay readable on touch — no iOS auto-zoom

iOS Safari auto-zooms into any `input`/`select`/`textarea` whose font-size is
**below 16px** on focus. The compact chrome scale (`--hv-text-*`) and the
legacy `--text-sm`/`--text-base` aliases (`12px`/`14px`) are far under that
threshold, so `base.css`'s `@media (pointer: coarse)` block forces form text
to `var(--hv-font-size-md)` (16px) on touch pointers. Desktop/mouse keeps the
compact sizes. This rule must not be removed or lowered — a tap into a login
or settings field must never zoom the page (the reference shells ship 17px;
HomeVault uses the exact 16px threshold). E2E: `login.spec.ts` §12 asserts the
computed font-size stays ≥16px on a coarse pointer.

## 6. App icon gradients

Each app has one dedicated gradient token `--hv-app-<id>-grad` used on the
dock tile, the in-window `WindowHeader` icon and the Launchpad tile — never a
flat color, never a bespoke accent. A new app gets its own token (+ a dock
class if it is pinned, and a `--hv-app-<id>-c` running-dot colour). 
`--hv-app-accent-grad` remains the generic fallback and is deliberately **not**
an app colour — it is tied to `--hv-primary` (#1E62AE) and must stay off the
app ladder.

The set is aligned on a Tailwind **500 → 700** ladder (light → dark stop), so
the whole dock reads as one palette rather than a mix of saturated and pastel
tiles. Two invariants hold across the set:

- `--hv-app-<id>-c` is **always** the second (darker) stop of
  `--hv-app-<id>-grad`, lowercased. The running dot and the tile must agree.
  E2E `14.2` fails if they drift. The single documented exception is
  `--hv-app-launchpad-c`: the Launchpad launcher is not an app, its tile is
  the pale `--hv-app-launchpad-grad` and its dot stays brand indigo.
- A pinned app with no `.hv-dock-icon-<id>` class in `Desktop.svelte` falls
  back to the accent gradient while its dot uses its own colour. Adding a dock
  tile and adding its class are one change.

Retuning a `--hv-app-*-grad` is therefore **not** a chrome tweak: the token is
read by the dock, the Launchpad and every `WindowHeader`, plus a handful of
in-page buttons and chips (`Chat.svelte`, `Terminal.svelte`, `Users.svelte`,
`Account.svelte`). It repaints the product's app identity.

### 6.1 The gateway (login + setup wizard)

The **login and setup-wizard screens** follow the approved login and
wizard mocks and are the one deliberate exception to the
single-accent rule: a full-screen soft pastel gradient
(`--hv-login-bg-gradient`: indigo → violet → pink) with two blurred orbs
(`--hv-login-orb-*`), a large live 24 h clock, and a compact white card
(380 px login / 520 px wizard). The rest of the product stays on the
Appliance palette; every value used by the gateway is a `--hv-login-*` or
`--hv-wizard-*` token declared in `tokens.css`.

- `frontend/src/routes/Login.svelte` is pure **Tailwind-on-tokens** (no scoped
  `<style>`), built from the shared `Button`/`Toggle`/`Modal` (shadcn-svelte /
  Melt UI) components + native inputs kept for autofill. The primary CTA uses
  the theme `Button` with `bg-[image:var(--hv-login-btn-grad)]!`.
- Raw `<input>`/`<button>` elements keep the trailing `!` important modifier
  on their visual utilities as belt-and-braces. `base.css`'s global
  `input`/`button` defaults (`width:100%`, white background, `--hv-primary`
  focus ring, `background:none`) live in `@layer base` since 2026-09-15, so
  Tailwind's layered utilities now win over them on their own — a plain
  `bg-[var(--hv-primary)] px-4` button renders correctly without `!`. The
  `!` is still kept here (and on the headings, whose `h1..h6` rules remain
  unlayered) so both cases read the same. The gateway field background/border/
  focus ring live on the **wrapper** div; the inner input is transparent and
  borderless (`textInputClass` / `fieldWrapperClass` constants in
  `Login.svelte`).
- Gradients must be applied as `bg-[image:var(--hv-login-…)]!` (the `image:`
  type hint) — a bare `bg-[var(--hv-login-btn-grad)]` compiles to
  `background-color` and silently drops the gradient.
- Motion is token/keyframe driven: the card shakes on a failed attempt via the
  existing `hv-shake` keyframes (`animate-[hv-shake_0.4s_ease]`), and a
  successful login scales + fades the card out (`scale-95 opacity-0` +
  `transition-all`) before navigating to `/desktop`.
- E2E contract (`tests/e2e/login/login.spec.ts`): all `login-*` testids, the
  `#username`/`#password`/`#recovery-*` native inputs, the modal-based error
  flow and the 2FA code boxes are unchanged by the redesign.

### 6.2 The setup wizard

`frontend/src/routes/SetupWizard.svelte` uses the same gateway vocabulary as
the login screen (reusing the `--hv-login-*` tokens) plus its own
`--hv-wizard-*` accents: the per-feature icon hues on the welcome step, the
amber recovery-code box (`--hv-wizard-recovery-*`) and the emerald "create
account" CTA (`--hv-wizard-create-grad`). Its text runs on the **gateway slate
ramp** (`--hv-wizard-text-heading/title/label/soft/body`, slate-900→500) — one
step lighter than the Appliance ink — and its fields are **white** with a
hairline shadow (unlike the login's `#F8FAFC` fields). Error borders/text use
`--hv-wizard-error` (#EF4444) and update live like the mock (username invalid
→ red / valid → indigo, confirm mismatch → red / match → green). Same styling
rules as §6.1 — Tailwind-on-tokens, no scoped `<style>`, trailing-`!`
modifiers on raw `input`/`button` utilities (including the `h1` color, which
`base.css`'s unlayered `h1 { color }` rule would otherwise override), gradients
as `bg-[image:var(--hv-…)]!`.

- The card header carries the wizard step dots (`setup-stepper-dot`/`-label`/
  `-line` with `class:done`/`class:current`), a gradient progress bar, and the
  brand tile. The 4 steps map to HomeVault's real flow: welcome → account
  (avatar + username + fullname) → password (strength bars + checklist) →
  verify & create; the mandatory one-time recovery code and the optional 2FA
  enrolment come **after** creation, styled with the same amber box + indigo
  CTA voice.
- The E2E contract (`tests/e2e/setup/setup.spec.ts`) hooks are all preserved:
  `setup-container`/`setup-card`/`setup-footer`/`setup-step-*`, `data-step="0..3"`,
  `hv-btn-primary`/`hv-btn-secondary`, the `#username`/`#fullname`/
  `#password`/`#confirm-password`/`#twofactor-code` inputs, and the `hv-*`
  classes (`.hv-welcome-title`, `.hv-feature`, `.hv-password-strength`,
  `.hv-req-list`, `.hv-summary-item/value`, `.hv-2fa-box`, `.hv-recovery-box`,
  …). The recovery step is exercised inline by test 19.1 before 2FA.

### 6.3 The Settings screen (2026-09-14, Figma `Sans titre` node `16-6187`)

A **page-scoped** palette rather than a new identity: the Settings mock's own
slate ramp, so the screen matches the frame exactly without dragging the
Appliance palette with it. Everything it declares lives under
`--hv-settings-*` in `tokens.css`; no other screen reads those tokens.

- **Greys are the Tailwind v4 slate ramp** (`#F8FAFC` canvas, `#F1F5F9`
  hairline, `#E2E8F0` border/track-off, `#64748B` micro-label, `#314158` row,
  `#1D293D` title, `#0F172B` active title) and the **eight category accents
  are Tailwind v4 hues**, so every value is traceable to the frame instead of
  invented — four still the mock's \*-500 step, four (widgets, notifications,
  ai, monitoring) darkened to their \*-600/\*-700 step on 2026-09-23 so both
  rail-tile states clear 3:1 (§7.1; E2E `settings.spec.ts` 17.5). The one
  colour that is neither: `--hv-settings-chip-text` (`#0E7490`, the ai
  accent) for the requirement chips, because micro text on the hairline
  background needs 4.5:1 — the ramp's `#64748B` reaches only 4.34:1 there
  while reusing the accent gets 4.89:1 (2026-09-26).
- **A tint is always derived from its solid**
  (`color-mix(in srgb, var(--hv-row-accent) var(--hv-settings-cat-tint),
  transparent)` at 9%), never written as a second literal — the tint and the
  accent cannot drift. The same custom property drives the tile, the active
  pill and the row's `Switch`.
- **Card and rail use `1.45px` hairlines** (`--hv-settings-border-w`) — the
  mock's weight, kept as a token because it appears on every outline.
- **One toggle geometry**: `theme/Switch.svelte`, 40x22 track / 16px knob /
  20px travel, `--hv-settings-toggle-off` when off and `--hv-switch-accent`
  (per-row) when on. It is a native checkbox in a label, *not* the bits-ui
  `Toggle` — see `docs/FRONTEND.md` → `Toggle` vs `Switch`. This holds for the
  **whole screen including the sub-cards**: the TLS card's two bits-ui switches
  (brand blue, 32x18.4px) were the last exception, migrated 2026-09-14 along
  with the dead `.hv-toggle-switch` rule they were expected to carry. The
  visible 40x22 track stays compact because the **hit area** is a transparent
  `--hv-touch-target-min` square on the track (2026-09-14).
- **Every interactive control is `--hv-touch-target-min` (44px) tall** — the
  WCAG 2.5.5 / Apple HIG / Material touch floor, mouse *and* finger. Inputs,
  selects, textareas and buttons use the `controlHeight` helper in
  `vocabulary.ts`; icon buttons, quick-model chips, weather results, the alert
  slider (44px input + 24px thumb) and the `PathPickerField` /
  `FloatingPathPicker` rows and buttons all resolve to the same token
  (2026-09-14). No `h-[34px]`-style literals — the token is the only height.
- **Icons take one of the scale's steps — 16 / 20 / 24** and this screen
  uses only the first two: **16** for glyphs inline with copy, rail entries
  and plain icon buttons, **20** for glyphs in a `w-9 h-9` tile (card
  header, active category, privacy rows) or a dedicated icon button. 24 is
  the next step for larger contexts (`w-10` tiles, empty states) and never
  appears here. 2026-09-26: the stray `14`, `15` and `18` values were
  removed from `Settings.svelte` and `components/settings/*` — `size={15}`
  was never on the grid, and five different sizes for one screen's icons
  is not a scale.
- **Layout contract**: 208px rail (`--hv-settings-sidebar-w`), a non-scrolling
  content header, then `[data-testid="settings-content"]` = the scrolling
  column flexbox whose cards are direct children and keep `shrink-0`. Section
  labels are **siblings** before their card. `components/settings/
  vocabulary.ts` is the one place those class strings are written; both
  `Settings.svelte` and `SettingsCard.svelte` import it, which is what keeps a
  card in the route identical to a card in `components/settings/`.
- Behaviour since 2026-09-15: **one Save button in the rail footer**
  (`[data-testid="settings-save"]`), no auto-save anywhere — every category
  stages into `stores/settingsDraft.svelte.ts` and `saveAll()` flushes the
  dirty groups; the sub-cards' editable fields share the same draft (their
  *actions* stay immediate). Since 2026-09-23 that button is also the screen's
  only save indicator: `data-state` (`clean`/`dirty`/`saving`/`saved`/`error`)
  picks one skin per state — `settingsBtnPrimary` / `settingsBtnDanger` /
  `settingsBtnSuccess` — and a matching label, never colour alone. See
  [Settings](./settings.md).

## 7. Rules that bind every change

1. One accent, one typeface (Inter, self-hosted), one radius language.
2. Every `var(--…)` uses the `--hv-` prefix. No third color vocabulary.
3. New UI → shared components + Tailwind-on-tokens. No new `hv-*` classes.
4. No sensitive value (secret/password/token) ever rendered into markup,
   URLs, storage or logs — redaction happens at the API boundary.
5. Every new screen ships its tests incl. the E2E Security section (see
   `CLAUDE.md` → rules).
6. **Type utilities on a heading need a trailing `!`.** `styles/theme/base.css`
   declares bare `h1..h6 { font-weight; line-height; color }` plus per-level
   `font-size` **outside any `@layer`**, so they outrank every Tailwind utility
   (Tailwind v4 emits utilities inside `@layer utilities`) no matter how
   specific the utility class is. `class="text-md"` on an `<h2>` therefore
   loses to `h2 { font-size: var(--hv-font-size-3xl) }` and renders at 28px.
   Write `text-md! font-bold! leading-6!` — the trailing modifier is the
   house escape hatch (`p-0!` / `overflow-hidden!` are the same tool). Only
   headings are affected; a `<div>`/`<span>` styled the same way is not.
7. **Touchscreen and mouse are both first-class (WCAG 2.5.5 / Apple HIG /
   Material).** Every interactive target — button, input, select, textarea,
   switch, slider, icon-button, picker row — is ≥ `--hv-touch-target-min`
   (44px) on each axis. A compact *visible* control (e.g. the 40x22 switch
   track) is acceptable only while its hit area is 44px; nothing is
   interactive on hover only (everything must be reachable by tap and by
   keyboard focus). A screen ships only when it works with a mouse *and* a
   finger. See §6.3 and `CLAUDE.md` → design system.
8. **One icon family per layer, no text glyphs (§4.4).** App code uses the
   shared sprite `<Icon>` (tabler-first), `@lucide/svelte` stays inside
   `components/ui/**`, inline `<svg>` is artwork only, and `✓`/`✕`/`★`
   typed as text is never an icon.

### 7.1 Accessibility invariants — WCAG 2.2 AA / EN 301 549 / ISO 9241

HomeVault ships against WCAG 2.2 AA + EN 301 549 §9 + ISO 9241-410/171.
These are correctness rules, enforced by review and by the E2E
"Security"/"Accessibility" sections — see `CLAUDE.md` → Accessibility baseline.

- **44px hit areas.** Every compact (28–40px) icon button carries a
  transparent 44px hit square via the shared `.hv-ic` (static button) /
  `.hv-ic-abs` (already `position: absolute`) classes in `components.css`.
  Add the class — never hand-roll a second `::after` on an element that
  already has one (the window trio `.hv-cl-btn` keeps its own 34px pad;
  `.hv-dock-item`, `.hv-switch-track` have their own). `base.css` also
  bumps raw `button`/`a`/`input`/`select` to 44px under
  `@media (any-pointer: coarse)` — deliberately `any-pointer`, so a hybrid
  touch-laptop (fine primary pointer) still gets the 44px floor.
- **Focus always visible.** `--hv-focus-ring` is a 2px solid `#1E62AE` ring
  + 4px halo (≈4.4:1 on white), not a translucent glow. Never write
  `outline: none` in a `:focus-visible` rule without also keeping the ring
  (`box-shadow: var(--hv-focus-ring)`). Inputs show border-primary + ring.
  A component whose only focus cue is a colour change fails 2.4.7.
- **Contrast floor.** Any token used for *text* must be ≥4.5:1 on every
  surface it renders on; any status dot / focus / boundary colour ≥3:1.
  The audited text tokens already sit above the floor (`--hv-shell-text-3`
  #5F6B76, `--hv-login-muted` #64748B, `--hv-settings-label` #64748B,
  `--hv-shell-border-strong` #87929E, badge bg #DC2626, wizard/timer hues).
  A new page-scoped palette must be contrast-checked before landing.
  The accent counts both as text and as fill: its shipped default `#1E62AE`
  clears 4.5:1 behind white text *and* as text on `--hv-surface` and the
  `--hv-primary-*` tints — the retired default `#4a90d9` did neither
  (≤3.34:1) and is still rewritten server-side by
  `handlers::settings::normalize_accent` (2026-09-23). Same rule for the
  brand gradient `--hv-app-accent-grad`: its light stop starts at `#3275C4`
  (4.69:1 with white), never lighter. Component palettes obey the same
  floor: the shared avatar palette (`$lib/utils/avatars.ts` — single source
  for Users *and* Groups, both rendering white initials) keeps every stop
  ≥4.5:1 with white, the Storage disk ring/legend swatches stay ≥3:1 on
  white, and the Chat tile's light stop is `#0d9488` (3.74:1 with its white
  glyph). The Settings rail's eight category accents clear 3:1 in **both**
  tile states (full-colour glyph on the 9% tint at rest, white glyph on the
  solid active — §6.3), and the Login/Wizard indigo ladder renders ≥4.5:1
  wherever it carries text (`accent` `#4F46E5`, `soft-border` `#7C3AED`,
  both gradients ≥5.70:1 under white). Runtime guards: `groups.spec.ts`
  16.1 (avatars), `settings.spec.ts` 17.5 (rail tiles), `login.spec.ts`
  13.6 (login tokens) — each reads live values, never hardcoded RGB.
- **Modals trap focus.** Prefer the bits-ui Dialog (`theme/Modal`,
  `theme/Drawer` — trap + Escape + focus restore built in). Hand-rolled
  overlays (Desktop power/confirm, `HvLaunchpad`) must use
  `use:focusTrap` from `lib/utils/focusTrap.ts` (focus in, Tab cycle,
  Escape, focus restore). A `role="dialog"` with no trap is a 2.1.1 gap.
- **Nothing is hover-only.** A control revealed on `group-hover` must also
  appear on `group-focus-within` / `focus-visible` (e.g. the Chat delete
  button). No right-click or double-click dependency — every such action
  has a keyboard/button path.
- **Names on icon buttons.** Every icon-only button carries `aria-label`
  (an `i18n` key — never a hardcoded string, never `title` alone, which
  touch users never see). Forms: every input has a `<label for>` or
  `aria-label` (the 2FA digit boxes name themselves per-field).
- **No nested interactives.** A container with `role="button"` must not
  contain `<button>` children (Explorer rows are named via `aria-label`;
  prefer a separate control when restructuring).
- **Heading structure.** One `<h1>` per screen (Login now marks its brand
  title as the top heading; wizard steps use real `<h1>`s).

## 8. Migration checklist (screens, in priority order)

Each screen migrates in one pass: replace hand-rolled markup/CSS with the
shared components, delete its scoped `<style>` (or reduce it to a one-off
layout), remove its per-app class prefix, then re-run its E2E suite and
capture a reference screenshot.

1. ~~`Health`, `Widgets`, `Groups` (small)~~ **done 2026-09-12** — `PageLayout` + `EmptyState` / `SectionCard` / `FormRow`, Tailwind-on-tokens, zero scoped CSS, `data-testid` hooks, E2E specs updated, `TESTING.md` current.
2. ~~`Account`, `Journal`, `Raid`, `Rescue`~~ **done 2026-09-12** — `Account` (2-col grid via Tailwind container queries, 2FA), `Journal` (toolbar + WindowFooter, priorities → tokens), `Raid` (full wizard), `Rescue` (4 tabs, `PathField` kept). The 5 legacy components (`Card`, `CardHeader`, `CardContent`, `ProgressBar`, `Divider`) — last consumer `Rescue` — were **deleted** from `theme/`.
3. ~~`Shares`, `Dashboard`~~ **done 2026-09-12** — `Shares` (6 protocol tabs + modals, `svcBar`/`modeSwitch`/`toggleField` snippets kept), `Dashboard` (12-col bento grid + Tailwind container queries, SVG gauge/core/mem kept). `Users` and `Storage` remained — their pre-existing TS error and the `tsconfig.node.json`/`vite.config.ts` `tsc` issues (masked by the check) were **fixed**: `npm run check` is at **0 errors**.
4. ~~`Users`, `Storage`~~ **done 2026-09-12** — `Users` (card grid / settings sheet / creation, `FolderPicker`+`Modal` kept, `homevault_admin` security guards + quota slider intact), `Storage` (6 tabs: disks / virtual / partitions / removable / encryption / cache, SMART + ring chart + mover kept). **Batch 3 complete.**
5. ~~`Network`, `VPN`, `Syncthing`, `Downloader`, `CloudSync`~~ **done 2026-09-12** — `Network` (stats + summary + ethernet/wifi tabs + config modals), `VPN` (WireGuard/OpenVPN, QR codes), `Syncthing` (3 tabs, progress), `Downloader` (aria2, `FolderPicker`), `CloudSync` (rclone 3 tabs, sync/config modals). **Batch 4 complete.**
6. ~~`Restic`, `Chat`, `Explorer`, `Docker`~~ **done 2026-09-12** — `Restic` (repos/creation/detail, snapshots/jobs/schedules, retention policies), `Chat` (side history + message thread + suggestions), `Explorer` (full file manager: places sidebar, breadcrumb, list/grid view, trash, snapshots, rename/compress/perms/preview/collision modals), `Docker` (rail nav + metrics, 6 tabs incl. catalog + compose, inline modals). **Batch 5 complete.**
7. ~~`Settings`, `VirtualMachines`, `Login`, `SetupWizard`~~ **done 2026-09-12** — `Settings` (category rail + cards, integrated security/monitoring/TLS cards kept, AI + weather — **re-themed 2026-09-14** on the Figma `16-6187` mock: 208 px two-line rail with one tile per category, content header, section/card/row vocabulary shared by the 6 sub-cards; scoped `--hv-settings-*` palette, see §6.3; auto-save replaced by one Save button 2026-09-23), `VirtualMachines` (machine list + detail drawer + catalog/ISO wizard, console/screen overlays), `Login` (auth + 2FA + code recovery, native inputs kept for autofill), `SetupWizard` (stepper + admin creation + recovery + 2FA). **Batch 6 complete.** — **Login re-skinned 2026-09-14** to the approved gateway (§6.1): pastel gradient + orbs + clock + 380 px card, TOTP progress indicator + countdown ring, shake on error, unlock fade; Tailwind-on-tokens, zero scoped CSS, E2E contract intact. — **SetupWizard re-skinned 2026-09-14** to the same gateway (§6.2): 520 px card + wizard step dots/progress, welcome feature cards, avatar + username/fullname, password strength + checklist, verification summary, amber recovery box, 2FA box; recovery step exercised in `setup.spec.ts` §19.
8. ~~`Terminal`~~ **done 2026-09-12** — `PageLayout` + xterm tabs + infobar in Tailwind-on-tokens, `data-testid="terminal-*"`, zero scoped CSS.
9. **`Desktop.svelte` (the shell) — documented exception.** Its ~2800 lines of scoped CSS are the **shared chrome CSS** (dock, topbar, launcher, windows, monitor widgets): `hv-*` classes (shared vocabulary, not a per-app prefix), **100% token-driven** (`--hv-*`/semantics — the old `rgba(15,108,189,…)` and `#fcd34d` literals were replaced by tokens/`color-mix`). It is not converted to Tailwind: this `<style>` is the equivalent of a shared `desktop.css`, and a blind migration would break drag/resize, snap zones and monitors. **The unfreeze is complete elsewhere: no screen has a per-app prefix or scoped CSS anymore** (besides the small `Settings.svelte` loader block). — Tightened 2026-09-14: the dock/topbar literals that remained (`border-radius: 16px`, the duplicated `0 2px 4px rgba(0,0,0,.14)` shadow on both pills, `background: #292929` on the tooltip, `.hv-dock-icon-chat`'s inline gradient) moved to `--hv-radius-*`, `--hv-chrome-surface-shadow`, `--hv-tooltip-bg` and `--hv-app-launchpad-grad`. A few decorative literals predating this pass remain outside the rules that were touched — they are not a licence to add more.


After each batch: update `docs/FRONTEND.md` + `TESTING.md` in the same change
and delete the dead CSS (`components.css`/`utilities.css` rules with zero
consumers).

**2026-09-17 — header-bar pass, complete.** The last in-window-header debt left
by lots 1–8 is closed: screens that had migrated to `PageLayout` still carried
page-level controls inside the bar (action buttons, the Ollama status pill, a
refresh button, the Journal auto-refresh checkbox). Each now lives in the
`WindowToolbar` row under the header — the 12 screens that render one are
`Chat`, `CloudSync`, `Downloader`, `Groups`, `Journal`, `Network`, `Quotas`,
`Restic`, `Shares`, `Syncthing`, `VirtualMachines` and `VPN` — except
Settings, whose save status lives in the rail footer with its one Save button
(2026-09-23: it *is* the button's colour + label now, plus an sr-only status
line — no visible pill left in the footer).
The `WindowHeader` slot props (`center`/`extra`/`actions`/`actionIcon`/
`actionLabel`) stay for the frame-provided header, but no screen in the registry
fills one; E2E `14.11` walks all 24 app ids and fails any bar that does. The
test does not stop at the slots: it also pins the bar's markup to exactly the
two pieces it may hold — the icon tile and the title block — and the tile to
exactly one glyph, so a stray element that is not a control (an added badge, a
status dot, a redundant wrapper) fails it too.

**2026-09-24 — shared-component adoption pass (W2), complete.** The UI audit's
"swap each hand-rolled card for the shared one" fix closed on every targeted
screen. **34 `StatCard` blocks** (Downloader, Network, CloudSync, Syncthing,
VPN, Restic, Storage, Shares — their local `statCard` / `statIcon` /
`statValue` / `statLabel` constants deleted), **28 `EmptyState` blocks**
(Storage, Restic, CloudSync, Docker, Users, Groups, Syncthing, Network, VPN,
Chat, Shares — local `emptyState` / `emptyCard` constants deleted; icons, the
Restic install commands, CTAs and the Chat suggestions moved into the
`iconSnippet` / `action` snippets so the `data-testid` hooks stay on the
component root), and **22 `SectionCard` blocks** (CloudSync's 4 empties + 5
panels, Restic's 3 tables + settings card, Dashboard's 9 bento cards — local
`sectionCard` / `card` constants deleted; Dashboard keeps `cardHead` /
`cardName` / `cardSub` inside `titleSnippet`, right-hand chips (CPU temp,
memory %, GPU/OS badges) move to `actions`, and CPU/Memory/Health fill their
grid cell through the `[&>.hv-section-card-body]` variants). The retired
`rounded-2xl … p-6` card constants give way to the appliance corner (12 px)
and the standard `px-5`/`pb-5` body — one card shape everywhere, not a
per-page variant.

Deliberate skips, with reasons: **VM's `vm-empty`** stays hand-rolled because
`tests/e2e/vm/vm.spec.ts` pins `[data-testid="vm-empty"] h2` while
`EmptyState` renders `<h3>` — changing the shared heading level would ripple
across every converted screen; **Dashboard's 4 top stat tiles** stay as-is
because `dashboard.spec.ts` pins the child hooks `dashboard-stat-value` /
`dashboard-stat-label`, which `StatCard`'s string `label` / `value` API cannot
host; **one-line placeholders** (Raid's note, Rescue/Account/Explorer/Journal
lines, Syncthing's in-list notes, Docker's inline loaders via `dkEmpty`, the
Users/Groups loading branches) stay hand-rolled — a single `<p>`/loader is not
a card anatomy. All counts above are static-grep verified; `npm run check`,
`npm run build`, the E2E suites and the `TESTING.md` counts remain to be run
for this pass.

**2026-09-24 — responsive / loading / data-table pass (W15, W11, W12),
complete (static verification).** W15 moved the remaining responsive rules
from viewport to container queries: **20 files** now carry them,
`@container`
anchors on both `PageLayout` bodies, `dialog-content.svelte`, the
`SetupWizard` root, the VM drawer and the Dashboard/Account/Settings page
bodies, the collapse thresholds consolidated in §4.2, zero viewport
variants left in routes, the dead viewport utilities (`hv-grid*`,
`hv-sm/md/lg/xl`) deleted, and `theme/README.md` + the theme quick-start
rewritten around `@container`. Accepted deltas: the `position: fixed`
pickers/backdrops (the shared `FolderPicker` — Restic, Downloader,
Explorer, Users — and Docker's URL-menu backdrop) portal to `<body>` and keep viewport-relative
sizing. W11 replaced the route loading branches — Journal's list,
Storage's stats, Docker's hub/stores/catalog, Explorer's sidebar and both
file-area states (8 sites) — with skeleton rows (§4.1.2); the
`Spinner`/`IconLoader` imports left with their last consumers
(`Journal` and `Docker` dropped both, `Storage` and `Explorer` keep theirs
for the in-control spinners), and the dead `dk-spin` references went the
same way. W12 rewrote `DataTable` (§4.1.2): two-layer wrapper, sticky
header + frozen first column, button-based sorting with `aria-sort`,
numeric right-align, skeleton loading rows and a pager dormant below 300
rows. Counts above are static-grep verified; `npm run check`,
`npm run build`, the E2E suites and the `TESTING.md` counts remain to be
run for this pass, the audit task-list boxes stay unticked until they do,
and the seven route-local tables (CloudSync ×2, Rescue, Users, Restic ×3)
still await migration to `DataTable`.

## Related docs

- [Frontend structure](../FRONTEND.md) — routes, stores, API client.
- [Desktop shell & windowing](./desktop.md)
- [Settings (wallpapers)](./settings.md)
- [UI audit tasklist](../design/ui-audit-tasklist.md) — the 2026-09-23 full-app
  design audit backlog (2 Critical / 18 Warnings / 1 Polish, baseline 11/100),
  with the per-finding file:line locations, the fix for each, and the four
  tooling gates that keep the families from returning.
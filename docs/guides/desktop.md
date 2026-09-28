# Desktop Shell & Windowing

**Purpose:** The Svelte SPA "desktop": launchpad, top bar, dock, draggable/resizable
app windows, and desktop widgets.

## Backend

The desktop is purely a frontend concern. Backend touchpoints: the data endpoints
each window calls, plus `GET/POST /api/monitor-layout` (per-account monitor-tile
positions + visibility) and the power menu's `POST /api/server/restart|shutdown`
(admin). `Desktop.svelte` is editable like any other file (the frozen-area
registry was removed on 2026-09-12) — but it is the shell every screen is
hosted in, so a change to it is a change to all of them; keep its E2E suite
green (`tests/e2e/desktop/desktop.spec.ts`) and re-capture a reference
screenshot when its chrome moves.

## Frontend

- Shell: `frontend/src/routes/Desktop.svelte`. The app registry is the `apps`
  array (`frontend/src/routes/Desktop.svelte:700`): each entry has
  `{ id, name, icon, category, desc, component }`. `openApp` spawns a window.
- Window manager store: `frontend/src/lib/stores/windowManager.ts`
  (open/focus/minimize/maximize, default size 1024×720). Two ways to open a
  window: `openWindow(appId, title, icon, componentLoader)` (native Svelte
  apps) and `openWindowUrl(appId, title, icon, url)` (`Window.contentType ===
  'url'`, rendered as a sandboxed `<iframe>` — used by installed Docker
  catalog apps, see [Apps Center](./apps-center.md#dynamic-docker-apps)).
- Window chrome: `frontend/src/lib/components/desktop/WindowFrame.svelte`
  (drag/resize; content is cached so it does not reload during drag/resize).
- Top bar (the floating pill) and dock are markup + CSS **inside**
  `Desktop.svelte` (the old `HvTopBar.svelte`/`HvDock.svelte` files are gone).
  The launcher overlay is `frontend/src/lib/components/desktop/HvLaunchpad.svelte`
  (full-screen scrim + tile grid — see *Launcher* below).
  A shared motion vocabulary (`--hv-chrome-*`) lives in `tokens.css`. Both the
  pill and the dock use `--hv-shell-glass` (never a bespoke low-alpha white —
  it fails contrast over a dark photo wallpaper) plus the dedicated chrome
  border/shadow pair below. Maximizing a window above 900px hides both; below
  900px it insets instead.
- **Each pill owns its height.** `--hv-chrome-topbar-h` (62px) and
  `--hv-chrome-dock-h` (84px) are separate tokens, and the dock is
  deliberately the taller one — it is the primary target and its 48px tiles
  need the room. `.hv-menubar` and `.hv-dock` are both
  `box-sizing: border-box`, so each token is the *rendered* height, which is
  what makes `--hv-chrome-topbar-keepout` / `--hv-chrome-dock-keepout` exact.
  Never point one pill at the other's token again: the heights were once
  locked together, which is what made a taller dock impossible. The dock's
  84px is the mockup's own authored height and does **not** decompose as
  `12 + 48 + 12` — DSM's dock item is a column whose label line is reserved
  even though it only paints on hover. HomeVault ships no dock labels, so 72px
  would have been honest; 84px was kept because the mockup is the reference and
  the surplus reads as breathing room. The rationale lives on the token.
- **Dock tile pitch — `--hv-chrome-dock-gap` (10px).** `.hv-dock-container`'s
  `gap` (not `.hv-dock`'s) is what spaces the tiles, because the dividers are
  flex children of that same row. A hovered tile runs
  `.hv-dock-item:hover { translateY(-10px) scale(1.18) }`, i.e.
  48 × 0.18 / 2 ≈ **4.3px on each side** — so at the old 4px gap two
  neighbours visibly overlapped mid-hover and appeared to touch. 10px keeps
  ~6px of real clearance at the hover peak. The ≤480px block keeps a 6px gap:
  the lift is flattened there (no `:hover` on touch), so tight spacing is
  safe. E2E `14.10` hovers a middle tile, measures the settled clearance to
  both neighbours and requires it to clear the growth the animation actually
  produced — it does not read the token.
- **Anything anchored to the dock derives from the dock's band**, not the
  topbar's: `.hv-activity-panel`'s `bottom` and the `<=900px` maximized-window
  inset both read `--hv-chrome-dock-keepout`. `.hv-activity-panel` was once
  pinned to `--hv-chrome-topbar-h` and would silently overlap a taller dock.
  E2E `14.1` pins the JS mirror; `8.6` and the activity-taskbar test pin the
  geometry.
- **The chrome material is its own token pair** — `--hv-chrome-surface-border`
  (slate-300) and `--hv-chrome-surface-shadow` — consumed by the pill, the
  dock and every panel hanging off the pill (`.hv-topbar-panel`, so a popover
  can't mismatch the pill it is anchored to). This is deliberate rather than a
  retune of `--hv-shell-border`: the shell hairline is consumed by every
  window, card and toolbar, and darkening it would repaint the whole product
  for a change requested on two pills.
- The topbar profile avatar wrapper (`.hv-user-pill-avatar`) forces its inner
  `<img>` to `width/height:100%` + `object-fit:cover` — the `<UserAvatar>`
  renders at `size=62` while the wrapper is `--hv-chrome-avatar-size` (32px),
  so without the override the face is centre-cropped. The 32px avatar is
  smaller than the 44px it replaced on purpose: at pill height the username
  carries the identity and the picture is only a marker.
- **Topbar buttons are icon + label chips (adopted 2026-09-14).** The mockup's
  icon + label layout was declined once, on three grounds that no longer hold:
  it was said to need four new i18n keys per locale (false — every label reuses
  an existing key: `app_network_name`, `notifications`, and the previously
  orphaned `desktop.power`), and to break the 44px touch rhythm
  (false — the mockup itself specifies `height: 44px`, so only the *width*
  grows). The third — that it widens the pill — is real and is what the
  breakpoint handles. (The topbar **Widgets** button was removed 2026-09-15 —
  adding/removing desktop widgets lives in Settings > Widgets, so the pill is
  Network / Notifications / Power.)
  - From **901px** up each button is `width: auto`, `padding: 0 var(--hv-space-3)`,
    `gap: var(--hv-space-1-5)` — icon then label. Below 901px
    `.hv-topbar-btn-label` is `display: none`, so the button collapses back to
    its 44×44 icon-only default. The label stays in the DOM at every width (the
    `.hv-user-pill-name` pattern): one markup branch, no JS width check, and
    `display:none` also keeps it out of the accessibility tree so the button's
    accessible name stays the `aria-label` alone.
  - **901 is not a new breakpoint** — it is the shell's existing "full desktop
    chrome" threshold (`chromeAutoHideEligible`, `viewportWidth > 900`, in
    `Desktop.svelte`; the ≤900px maximized-window inset uses the same split).
  - The two responsive blocks sit **after** the motion section, not next to the
    base rule, and the ≥901 block re-asserts `:hover`/`:active`: the shared
    `translateY(-4px) scale(1.08)` lift would grow a ~137px chip ~11px and bite
    into the 4px inter-chip gap, so the chips drop the scale; and because the
    block is later in source than the motion rule, a hovered + pressed chip
    would otherwise lose its press dip to the equal-specificity tie.
  - Icons stay 22px, radius `--hv-radius-xl`. **No token, no height and no
    keep-out changed** — the pill's 62px is still driven by the 48px user chip
    (`8 + 32 + 8`), so the delta is purely horizontal. E2E `14.1`/`8.3` pin that
    and must stay green untouched.
- **The notification badge is anchored to the button, not to the bell.** It
  measures `16px` on `top: -3px; right: -3px` — the mockup's own value, and what
  a chip can carry (the 26px `.hv-dock-badge` shape belongs to a 48px dock
  tile). Its invariant is vertical: the badge spans `y = button.top-3 … +13`
  while the label's line box is centred in the 44px button (≈ `+14.5 … +29.5`),
  so the badge lives in the band **above** the label and a `99+` count widens
  into empty space instead of covering a glyph. Horizontally `button.right + 3`
  keeps even the badge's 2px `--hv-shell-glass` ring on the pill. Never "tidy"
  an oversized badge with `overflow: hidden` on `.hv-menubar` — that clips the
  ring and the pop-in. Two badges, two rules: `.hv-dock-badge` and
  `.hv-notification-badge` are deliberately **not** shared any more.
- Topbar popovers (network `.hv-net-dropdown`, account `.hv-user-dropdown` in
  `Desktop.svelte`; `.notifications-panel` in `NotificationCenter.svelte`) are
  fully standardized by the shared `.hv-topbar-panel` class
  (`theme/components.css`): it owns placement/stacking **and** the whole
  solid paper-white card look — width clamp (`--hv-topbar-panel-w`, default 420px;
  net overrides to 440), `--hv-radius-4xl`, `--hv-shell-glass` +
  `--hv-chrome-surface-border`/`-shadow` (opaque, no blur), `overflow:hidden`,
  and the 160ms `hvTopbarPanelAppear` scale-in. A panel's own rule only
  carries body content (e.g. `.notifications-panel` keeps just `max-height` +
  flex column). Don't re-add per-panel background/border/radius/animation.
- Dock tiles are 48px with a `--hv-radius-2xl` (16px) radius, one
  `--hv-app-<id>-grad` gradient per app (see
  [design-system.md](./design-system.md) §6); the Launchpad launcher is not an
  app and keeps its own pale `--hv-app-launchpad-grad` with dark ink. Hover
  lifts the tile (`translateY(-10px) scale(1.18)` on the
  `--hv-chrome-ease-spring` curve) and adds `brightness(1.05)`; the tooltip is
  `.hv-dock-item::before` (reading `aria-label`, on `--hv-tooltip-bg`) and the
  click ripple is `::after` — both pseudo-elements are taken, so a third
  effect needs a real element. The tooltip is suppressed under
  `@media (hover: none)`, where a sticky `:hover` would pin a label open for
  good. The dock/topbar divider and the dividers inside `.hv-topbar-panel` all
  read `--hv-chrome-surface-border`.
- Desktop monitor widgets (Resource, RAID, Security, VPN, Downloader,
  Syncthing, Calendar, Docker, Weather, Active Sessions, System Health)
  are floating
  panels rendered directly in `Desktop.svelte` — no separate
  `DesktopWidget.svelte` component (that file, plus `desktopWidgets.ts`
  and `widgetTheme.ts`, was removed on 2026-07-18; see `CLAUDE.md`'s
  Widgets app section). Each is table-driven: `MonitorKind`,
  `MONITOR_WIDTH`/`MONITOR_SELECTOR`/`MONITOR_CLOSE_SELECTOR`,
  `getMonitorPosition`/`updateMonitorPosition` in `Desktop.svelte`, one
  entry in `frontend/src/lib/stores/monitorStore.ts` (position +
  visibility, persisted per-account via `GET/POST /api/monitor-layout`), and one tile in
  `frontend/src/routes/Widgets.svelte`'s `widgetDefs`. The Weather panel
  additionally reads/writes `frontend/src/lib/stores/weatherLocation.ts`
  and calls Open-Meteo directly from the browser (no backend proxy).
- Shortcuts: `frontend/src/lib/stores/desktopShortcuts.ts`.

## Launcher (Launchpad, Figma "Sans titre" node `23:6646`, 2026-09-16)

`frontend/src/lib/components/desktop/HvLaunchpad.svelte` is the app launcher,
mounted by `Desktop.svelte` when the dock's Launchpad tile is activated (Escape,
a scrim click, or picking an app closes it). It renders directly from the shell's
`apps` registry — the whole list, filtered — never a duplicated catalog.

**It is not a card.** The mock is a full-screen dimmed, blurred scrim with a
top-aligned 828px content column. The previous implementation was a centered
opaque 1000×720 modal with a vertical category sidebar and wide description
cards; that anatomy is gone. The stack is now:

1. **Search bar** — 56px white pill (`--hv-launcher-surface`), the mock's
   two-layer drop shadow, magnifier glyph on the left, a clear button
   (`.hv-ic-abs`) on the right while a query is typed.
2. **Category chips** — one horizontally-wrapping row. "All apps" first, then
   each category actually present in the app list, in `CAT_ORDER`. They are
   radio-like *filter* buttons carrying `aria-pressed` inside a `role="group"`
   — not a `tablist`, because they narrow the one grid in place instead of
   swapping panels. Clicking the active chip clears the filter. The chips
   **grow to fill the row** (`flex: 1 1 auto`): a chip is otherwise only as
   wide as its label, so on a host with a handful of categories the row used
   to stop around two thirds of the way across and the module read as narrower
   than the tiles under it. Growth is equal *delta*, so a longer label stays
   the wider chip, and `grow` was chosen over `justify-content: space-between`
   for the wrapped case (a fully populated catalog has up to 13 chips): both
   spread the trailing line, but `grow` spreads it as slightly wider pills
   where `space-between` opens a void between two far-apart ones. The ≤640px
   block pins `flex: none` back, since there the row is a one-line
   `overflow-x: auto` scroller.
3. **Tile grid** — `repeat(auto-fill, minmax(124px, 1fr))`, **6 columns** of
   128px at the column's 828px (`--hv-launcher-content-w`; the mock drew 5).
   Each tile is the app's icon tile over a centered, two-line-clamped label.
   Empty result → `.lp-empty` with a reset button; a typed query also shows
   the `.lp-count` results line.

The three stacked modules are deliberately **one width**: the search bar, the
chip row and the grid are children of the same column, so widening
`--hv-launcher-content-w` widens all three together and they can never
misalign. E2E `5.12` asserts exactly that (search width == chip-row width ==
grid width, and a shared left edge) alongside the 6-column count. It also
asserts the second, separate half of it: the chip row's *contents* reach the
grid's right edge. The row being 828px wide is not enough — a chip is only as
wide as its label, so with a short category list the chips stopped two thirds
across and the module looked narrower than the grid even though the box
matched. The assertion is per rendered line, so it holds with or without a
wrap. The column stays a *maximum* (`min(100%, …)`): on narrower screens
`auto-fill` drops to 5, 4, … columns on its own, and the ≤640px block pins 3.

**The scrolling container is `.lp-panel`, not the grid wrapper** — and that is
a width decision, not a scrolling one. A scrollbar that takes layout width
(Chrome on Windows/Linux, a GTK desktop with overlay scrolling off) takes it
from its own container's content box. While `.lp-content` scrolled, that width
came off the grid alone — the search bar and the chip row sit outside it — so
with the tiles overflowing the grid rendered narrower and further left than the
other two. Scrolling the panel puts all three in one content box, so the gutter
is taken from all three equally. The E2E engine's scrollbar overlays its content
(0px), which is why the drift is invisible in the suite; `5.12` therefore
measures the three widths a second time at 1440×520, where the tiles overflow —
the only regime in which they can come apart. `scrollbar-gutter: stable` keeps
the gutter reserved when a chip filter shortens the list, so the column does not
resize as the chips are used, and `.lp-search` is `position: sticky` so the
query field stays reachable while the tiles scroll under it.

**Tiles keep the per-app gradient, not the mock's white square.** The mock
draws white tiles with dark ink; `CLAUDE.md`'s "one colour per app" rule is
binding and wins — the tile is `--hv-app-<id>-grad` (the same value the dock
tile and the in-window `WindowHeader` use, threaded in as `--lp-ic`) with the
existing white-glyph sheen. The mock supplies the *layout* (56px rounded tile
above the label), not the fill.

**No fabricated counts.** The mock's tiles carry `3` / `1` notification badges;
HomeVault has no such data, so no badge is invented. The only corner marker is
`.lp-card-state`: a green dot rendered **only while a Docker app's container is
up** — presence, not colour, carries the meaning (exactly the dock's running
indicator). A grey "stopped" dot is deliberately absent: the tile ground
composites to ≈`rgb(100,105,117)` over the scrim, where the muted ink measures
≈1.01:1 and even `--hv-success` ≈1.07:1, so a neutral dot would be invisible
(1.4.11). The stopped case is stated in text instead, via `dockerCardDesc()` on
the tile's `aria-describedby`.

**Page-scoped palette `--hv-launcher-*`.** The launcher is the one HomeVault
surface that sits on a *dark translucent* ground, so it cannot borrow the
opaque-paper `--hv-shell-text-*` ink set. Two mock values could not ship as
authored:

- The scrim is `rgba(15,23,42,0.72)`, not the mock's `0.55`. At `0.55` the
  tile fill resolves to ≈`rgb(136,139,150)` over a light wallpaper and the
  white labels land at **3.39:1**, under the WCAG 2.2 AA 4.5:1 text floor.
  `0.72` measures ≈**5.4:1** in the same worst case.
- Inactive chip text is opaque `#FFFFFF`, not the mock's 85%-alpha white,
  which measures ≈**4.28:1** over the 15% chip fill.

**The scrim's `blur(2px)` is written as the standard property only.** The mock
carries `backdropFilter: blur(2px)` on the launcher frame, and it is the one
blurred surface in the shell: the no-`backdrop-filter` rule (`tokens.css`)
protects *ink* surfaces from a photo wallpaper, whereas the scrim does the
opposite — it darkens that wallpaper so the white labels read on it. Do **not**
add the `-webkit-backdrop-filter` companion: the CSS minifier collapses the pair
into a lone `-webkit-backdrop-filter`, and Firefox reports
`CSS.supports('-webkit-backdrop-filter', …) === false`, so the blur silently
resolves to `none` there. Writing the standard property alone makes the
minifier emit **both** forms itself. E2E `5.7` measures the resolved value, so
the regression cannot return unnoticed (it was found exactly this way).

`.lp-cat` keeps the mock's 32px visible pill but carries a transparent 44px
`::after` hit pad (the `.hv-ic` pattern); the search `input` is a full
`--hv-touch-target-min` tall inside a 6px-padded row, so the bar still renders
at the mock's 56px. Because the brand `--hv-focus-ring` disappears into the dark
scrim, chips and tiles use a **white 2px ring + dark halo** instead — the search
bar keeps the brand ring, being a white surface.

**Safety cautions survive as accessible descriptions.** Eight apps carry a
`desktop.app_<id>_info` advisory (Rescue, RAID, Storage, Network, VPN, Backup,
Users, Docker). The mock's tile has room for an icon and a label only, so rather
than orphan those keys they ride on `aria-describedby` → a visually-hidden
`.lp-sr` span. Read them there before "tidying up" an apparently unused key.

Phone (`≤640px`) tightens the inset to `env(safe-area-inset-top)`, scrolls the
chip row horizontally (`flex-wrap: nowrap` + `flex: none` on each chip, undoing
the desktop growth — a grown chip would fill the row and leave nothing to scroll
to) instead of wrapping it, and drops the grid to `minmax(104px, 1fr)` — three
columns at 400px. The vertical scroller stays the panel, so no tile is ever
unreachable (E2E `5.11`).

The E2E contract lives in `desktop.spec.ts` §5 and is unchanged by the
redesign: `.lp-overlay`, `.lp-panel`, `.lp-card[title="…"]`,
`.lp-search-field`, the drag handlers, the Docker external-open menu
(`.lp-ext-*`), `.lp-empty` / `.lp-reset` and `.lp-count` all kept their
selectors.

## In-window anatomy (AppWindow mock, 2026-09-14)

Every app window is the same skeleton — `WindowFrame.svelte` (frame, drag,
resize, the min/max/close trio) + `WindowHeader.svelte` (one header per
window: app-gradient icon tile + title, and — since 2026-09-17 — **nothing
else in the bar**; the tab row lives in the wrap above it) + optional
`WindowToolbar.svelte` (44px action row under the header, where every
page-level action now lives), `WindowSidebar.svelte` (256px left rail, 44px
nav rows) and `WindowFooter.svelte` (`actions` 56px button bar or `status`
32px strip).

### The window itself — Figma "Sans titre" node `3:76`

The mock is the authority for the frame, the bar and the trio. Values below
are its authored numbers, cross-checked pixel-by-pixel against a 2× PNG
export; E2E `14.5`–`14.8` pin them.

- **Frame:** fill `--hv-window-bg` (`#FFFFFF`, **opaque** — a dark wallpaper
  must not show through), a 1px `--hv-window-border` (`#CBD5E1`, slate-300)
  hairline, `--hv-window-radius` (16px), and the mock's own two-layer shadow
  `--hv-window-shadow` (`0 4px 16px rgba(0,0,0,.06), 0 1px 3px rgba(0,0,0,.08)`).
  Note the hairline is **deliberately darker than `--hv-shell-border`**: the
  frame reads as a distinct object over the wallpaper where a card hairline
  would dissolve. Maximized windows are square (edge-to-edge); tablet-inset
  maximized keeps the radius.
- **Header bar:** `--hv-winhdr-h` — a **44px floor** (8px + the 28px
  control row + 8px), `padding: 8px 12px`, `gap: 12px`, on a
  `--hv-winhdr-border` (`#E2E8F0`) underline. Its vertical padding is the
  derived token `--hv-winhdr-pad-y` = `(--hv-winhdr-h − --hv-winctrl-size) / 2`,
  so the bar is *exactly* one control row tall. It stays a `min-height`, not a
  `height`, so a wrapped action cluster at ≤480px grows instead of clipping —
  but **every shipped app now renders at exactly 44px**: the standardized bar
  is icon tile + title + the pinned trio and nothing else (see the anatomy
  note below). E2E `14.6` asserts the literal 44 as a regression guard, so
  anything that creeps back into the bar fails the suite.
  The underline is a `border-bottom` on **`.hv-winhdr-wrap`**, not on
  `.hv-winhdr` itself — the wrap also contains the optional tab row, so the
  mock's single rule closes the whole header block rather than the bar alone.
  A test that reads `borderBottomColor` off `.hv-winhdr` gets the inherited
  `currentColor` (the ink, `#1F2733`) and can never match; this is exactly how
  `14.6` failed the first time it ran.
- **Icon tile:** `--hv-winhdr-icon-size` (**= `var(--hv-winctrl-size)`, 28px**
  — see the standardization section below; it was 24px before 2026-09-14),
  `--hv-winhdr-icon-radius` **8px**, the app's own `--hv-app-<id>-grad` (never
  flat — see the one-colour-per-app rule in `CLAUDE.md`), white glyph
  **clamped to 16px by CSS**. The glyph size is *not* a prop: a route's icon
  snippet must not pass `size=`, because
  `.hv-winhdr-icon :global(svg) { width/height: 16px }` overrides the SVG
  width/height attribute `<Icon>` writes from it (a stylesheet beats a
  presentation attribute). `stroke` is the exception — no rule overrides
  `stroke-width`, so it stays explicit.
- **Title:** 14px / 600 on a 20px line, `--hv-winhdr-title` (`#314158`).
  This is `--hv-text-md`, the compact chrome step — the bar is only 44px, so
  the old 16px `--hv-font-size-*` content step no longer fits it.
- **Control trio (min / max / close):** `--hv-winctrl-size` **28px**,
  `--hv-winctrl-radius` **8px**, `--hv-winctrl-gap` **6px**, flat fills
  `--hv-winctrl-min` `#FFB900` / `--hv-winctrl-max` `#2B7FFF` /
  `--hv-winctrl-close` `#FF6467` (each with a `-hover` step) and the mock's
  own 14×14 white glyphs, at `stroke-width: 1.458` (the SVG's own value — it
  is what makes the glyphs read the mock's weight at 14px). **These are the
  mock's literal stops, not the semantic tokens**: `#FFB900` and `#FF6467`
  are brighter than `--hv-warning`/`--hv-error` by design, so do not
  "harmonise" them back.

### Where the trio lives — and the reserve that protects the title

`WindowFrame.svelte` renders the trio (`data-testid="win-minimize|maximize|close"`)
**absolutely positioned into the header band** at `top: 8px; right: 12px` —
one renderer for every window, so an app can't forget it, and `WindowHeader`
reserves the matching space on its right edge via
`--hv-winhdr-btn-clear` (**120px** desktop / `--hv-winhdr-btn-clear-sm`
**108px** ≤480px). That number is the trio's total footprint, and the two
must move together:

```
3 × 28px (buttons) + 2 × 6px (gaps) = 96px, + 12px inset + 12px gap = 120px
```

The ≤480px value drops the 12px gap (the bar wraps there) → 108px. E2E `14.8`
measures the real boxes at 1440 / 768 / 400px rather than trusting the tokens,
because the trio is absolutely positioned: the header's right padding is the
**only** thing stopping a long title from running underneath it.

The trio is 28px for fidelity, which is under the ≥44px touch rule — so each
button carries a transparent `::after` hit pad out to **34px** (the largest
the 6px gap allows before neighbouring targets overlap). Don't grow the visual
to 44px "for the target": that is the mock's geometry, and the hit pad is the
compromise.

### One header per window — `OWN_HEADER_APPS`

Every app renders its own `<WindowHeader>` (directly or through
`PageLayout`), and `WindowFrame.svelte` keeps a `OWN_HEADER_APPS` `Set` of
those app ids so it skips its own fallback header for them. **A new app must
be added to that set** — an app left out gets both bars stacked, and the
fallback's is blank (its `win.icon` sprite name has no glyph). `quotas` was
missed on 2026-09-14 and shipped that duplicate until E2E `14.9` (one
`.hv-winhdr` per window, across a registry sample) caught it. The frame's
fallback header is now the same shape as every app's — icon + title only — so
it too renders at exactly the mock's 44px.

### Header standardization (2026-09-14)

The bar is **icon tile + title + the pinned min/max/close trio**, and nothing
else. One control row, everything on one shared vertical centre line:

- `WindowHeader`'s `subtitle` prop was **removed** (along with `PageLayout`'s),
  and every route that passed one dropped it. The title is the bar's only text.
- The icon tile is `--hv-winhdr-icon-size`, defined as `var(--hv-winctrl-size)`
  — deliberately the **same 28px** as each of the trio's squares, so the app
  glyph and the window controls read as one control row.
- The tile's glyph is **clamped to 16px by `.hv-winhdr-icon :global(svg)`**,
  which is the single owner of that size. The `size={26}` every route's icon
  snippet used to pass was therefore inert, and passing it *at all* was a
  second, drifting copy of the clamp — all 24 header snippets (23 `PageLayout`
  routes + `Terminal`, which renders `<WindowHeader>` itself) and
  `WindowHeader`'s own `<Icon name={iconName}>` fallback had it removed on
  2026-09-14. `Dashboard.svelte` spells its glyph as a raw inline `<svg>`
  instead of `<Icon>`, so its equivalent `width="26" height="26"` went with
  them (its `viewBox` stays — that is what makes the clamp scale the glyph).
  **Keep `stroke` when touching these snippets.** Sizes on *other* snippets are
  real and must stay: Docker's modals (22), Explorer's menus (15/18), Settings'
  category rail (18), the Desktop topbar/dock (26) and monitor widgets (16),
  Login's error modals (22), and `WindowHeader`'s own action button and tab row
  (15, on `.hv-winhdr-action-btn` / `.hv-winhdr-tab`, which no clamp targets).
- WindowFrame pins the trio with
  `top: calc((--hv-winhdr-h − --hv-winctrl-size) / 2)`, **not** `top: 50%`:
  `.hv-window-controls` is absolute inside `.hv-window` (the full-height fixed
  frame), so `50%` would centre on the *window* rather than on the header bar.
  E2E `14.7` compares the tile's, the trio's and the title's mid-lines against
  the bar's own mid-line.
- **Decorative header payload removed with it**: the Dashboard "Live" badge,
  the Health status pill, the Rescue four-tool availability strip and the
  Syncthing version string. Header-*decorative* content is gone; the **real**
  controls stay — Explorer's upload, Journal's auto-refresh/clear/cleanup,
  Network's diagnostics/scan, Restic's back-to-repos, Syncthing's restart,
  VPN's refresh/new-tunnel, and every app's tab row.
- Anything that was a genuine *reading* rather than decoration moved to the
  page body: the Health overall status is now the `health-overall` strip at the
  top of `Health.svelte`, rendering the same `health-badge` test id it carried
  in the header (i18n key `health.overall_status`). The Rescue availability
  badge moved onto each tool's own card (`SectionCard`'s `actions` slot), one
  per tab.
- A description or a status summary belongs in the page body, not in the bar:
  it is the only way the bar can stay 44px and the title stay centred.

### The rest of the standard

- Tab rows are underline-style with 44px rows and full arrow-key support; the
  primary action button is a brand button (hover = `--hv-primary-hover`) sized
  to `--hv-winctrl-size` (28px) so it sits flush with the trio instead of
  filling the 44px bar edge to edge; disabled buttons use the
  exact disabled pair (`--hv-shell-border-soft` bg + `--hv-disabled-fg`
  text), never an opacity wash.
- Toolbar/sidebar/footer buttons and nav rows: 44px targets, 8px radius,
  hover = `--hv-subtle-hover`, pressed/disabled per the
  state tokens in `tokens.css`.

### Opening another app from inside an app — `hv-open-app`

A screen that needs to hand off to a sibling app must **not** call
`openWindow` itself: `openApp` is local to `Desktop.svelte` and is the sole
owner of the app's icon, its `--hv-app-<id>-grad` colour, `appIconBg()` and
the focus-or-spawn dedup. Duplicating that is exactly the "one colour per app"
breach `CLAUDE.md` forbids.

The convention is a window-level custom event:

```js
window.dispatchEvent(new CustomEvent('hv-open-app', { detail: { id: 'shares' } }));
```

`Desktop.svelte`'s `handleOpenAppRequest` (`onMount`, removed on destroy)
resolves `detail.id` against the `apps` registry and calls the same `openApp`
the launchpad and dock use. Two consumers today: `Explorer` → `shares`, and —
since 2026-09-14 — `Users` → `quotas` (the rail's "Quota view" button) plus
`Quotas` → `users` (its "Manage in Users" button). Add the listener once in
the shell; a screen only ever dispatches.

## Monitor widget standard (Appliance, 2026-09-12)

The 11 floating monitor panels (Resources, RAID, Security, VPN, Downloads,
Syncthing, Calendar, Docker, Weather, Sessions, Health) share one card
anatomy, enforced by the trailing `FLOATING WIDGET PANELS` override block in
`Desktop.svelte` (one rule for all 11 — never style a panel's surface
individually):

- Surface: white card, 1px hairline, **12px** floating-card corner
  (`--hv-radius-card`, the same stop as in-window cards — not a control
  radius), layered shadow. Widths use the shared `--hv-monitor-w-compact`
  (280px) / `--hv-monitor-w` (300px) / `--hv-monitor-w-wide` (320px) token
  set. `WidgetHeader.svelte` owns the 44px drag bar (`--hv-widget-header-h`,
  matching the in-window app header): a 28px accent icon tile
  (`--hv-winhdr-icon-size`) + a 14px semibold title — **no close/action
  button** (2026-09-15: monitor visibility is managed from Settings > Widgets,
  so the bar is icon + title only).
- The maximized-window inset above the dock derives from
  `--hv-chrome-dock-keepout`, which is `--hv-chrome-dock-offset` +
  `--hv-chrome-dock-h` (the dock's own height; it used to borrow the topbar's)
  — never hardcode the dock height again (a stale 56px `--hv-dock-item-size`
  once left a 6px overlap on tablet).
- Body rhythm: 16px panel padding, 12px section gaps, 8px row gaps
  (`--hv-space-4/3/2`); data numerals tabular; empty states in secondary
  ink (never tertiary grey as sole content).
- Interactive rows/buttons are 44px: widget actions, docker per-container
  buttons (44px, 20px glyphs), raid/weather CTA buttons (44px brand
  buttons), calendar nav is the one documented 36px exception. Disabled
  uses the exact disabled pair, never opacity.
- Drag uses Pointer Events with `touch-action: none` on the header bar;
  panels clamp to the live viewport without writing back to the
  `monitorStore` position.

## Invariants

- Window content must survive drag/resize (a past bug re-evaluated an `{#await}`
  block on every layout change; fixed by caching in `onMount`).
- Windows are opened from the launchpad; the Terminal, Settings, and Chat also have
  fixed top-bar action buttons in `frontend/src/routes/Desktop.svelte`.
- **Appliance theme (2026-09-12, replaces Fluent Paper):** bespoke tokens —
  brand accent `--hv-primary` `#1E62AE` (hover `#18508D` / pressed `#0E2F54`),
  opaque `--hv-shell-*` surfaces (no blur, so a dark photo wallpaper never
  washes out ink text), layered shadows, 1px hairlines. Floating chrome radii
  are appliance-tight (pills full, 44px+ controls 16, compact 8–12, window
  controls are the AppWindow mock's traffic lights — `--hv-winctrl-*`, 28px
  at an 8px radius —
  and the topbar buttons are flat chips on the pill, no tile,
  tooltips 4). Full palette + the governance rules live in
  `docs/guides/design-system.md`; the per-token values are in `tokens.css`
  itself. The
  no-photo default wallpaper is a light gradient (`defaultDesktopGradient`
  in `Desktop.svelte`, mirrored in `Login.svelte` — keep the two in sync).
  No hardcoded navy literals in the chrome (all `--hv-shell-*` tokens).
- **Tablet & touch (2026-09-10):** a `481–900px` media query compacts the
  topbar pill (avatar-only identity, slim clock — all targets ≥ 44px), and
  `@media (hover: none)` flattens the sticky `:hover` dock/topbar lifts on
  touch screens. Both blocks must stay **before** the trailing
  `PHONE CHROME OVERRIDES` block (equal-specificity source-order ties).
- **Touch targets & chrome labels (2026-09-11):** every desktop-chrome
  control is a ≥ 44px touch target — 44px topbar buttons (the 62px is the
  *pill's* height, not the target's), 48px dock icons, 44px widget
  close/action buttons on the 48px
  `WidgetHeader` bar, 44px launchpad sidebar rows (phone chips included).
  Documented exceptions: the calendar widget's nav row uses compact **36px**
  buttons (a 300px panel cannot fit five 44px targets beside the month label
  without truncating it; still ≥ WCAG 2.2 AA 24px) — see the comment on
  `.hv-calendar-nav-btn`; and the window min/max/close trio is **28px**
  (the AppWindow mock's own size) with a transparent 34px `::after` hit pad,
  34px being the widest a target can be at the mock's 6px gap without
  overlapping its neighbour. Both are compromises on the rule, not its
  removal — don't "fix" either by resizing the visual.
- **Chrome labelling (2026-09-11):** every chrome button carries both
  `aria-label` and `title` from the same translated key, so hover tooltips,
  screen readers, and the dock's CSS tooltip (`::before` reads `aria-label`)
  always agree. Dock buttons use the translated app names
  (`desktop.app_<id>_name` — the Health entry reads "Santé", never the old
  hardcoded "Security"); topbar buttons use `app_network_name`,
  `notifications` (`desktop.notifications`); window
  controls use `window_minimize` / `window_maximize` / `window_restore` +
  `common.close`. The notifications toggle is a single `<button>` (a
  `div[role=button]` wrapping a `<button>` double-announced and swallowed
  Space).
  - Since the icon + label chips landed (2026-09-14) the topbar buttons have a
    **visible** label too, so the accessible name must satisfy WCAG 2.5.3
    "Label in Name" — it has to contain the label. Network / Notifications
    use the same key for both. Power is the one deliberate pair:
    the visible label is the short `desktop.power` ("Alimentation", "Energie"),
    while `aria-label`/`title` keep the longer `power_options` ("Options
    d'alimentation", "Energieoptionen") — each of the four locales' longer
    string contains its shorter one. E2E `15.5` enforces the containment.
  - E2E suites must NOT match on these translated labels to *locate* an
    element. The hooks are `data-testid="dock-<appId>"`,
    `"win-minimize|maximize|close"`, and — since the chips made both the text
    and the old "power is the last `.hv-topbar-btn`" trick unusable —
    `"topbar-network|notifications|power"` (see
    `tests/e2e/desktop/desktop.spec.ts::dockApp` / `topbarBtn`).

## Activity taskbar

The dock's transient activity button + the panel above the dock are markup +
CSS in `Desktop.svelte`, driven by `frontend/src/lib/stores/activityStore.ts`
(polls `GET /api/activity`). Full spec: [activity.md](./activity.md). The old
8 s `/api/progress/jobs` poll was removed; `activeJobCount` and the topbar
`.hv-menubar-activity` strip now come from the same store.

## Related

- [Apps Center](./apps-center.md), [Themes](./themes.md),
  [Dashboard](./dashboard.md), [Activity taskbar](./activity.md)

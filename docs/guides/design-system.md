# HomeVault Design System

The **single source of truth** for the HomeVault visual language. The product
has exactly one design identity — "HomeVault Appliance" (2026-09-12) — which
replaces every earlier skin ("Fluent Paper", "Frosted Glass Modernism", "Clean
Paper & Slate"). Nothing in the codebase may reference another framework's
design language (Fluent, Material, …) as an authority: tokens, components and
pages all read from the `--hv-*` tokens declared in
`frontend/src/lib/styles/theme/tokens.css`, which this document describes. The
one external reference it followed is an Apple-style token spec, and only for
its **non-colour** rules (spacing, corners, elevation, type metrics,
breakpoints; see §2.2). That spec is not published with the repository: the
adopted values live in `tokens.css` and are documented here. Its colour palette
is not adopted.

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
- **Precise** — three corner families (badges 8px, controls 12px, containers
  18px) plus the capsule/pill for tags, never a decorative 20–28px hero
  curve, restrained motion, no decorative effects.
- **Apple-derived, appliance-owned** — the *non-colour* half of the language
  (spacing rhythm, corner ladder, elevation, weights, leading, tracking,
  layout breakpoints) is ported from an Apple-style reference (§2.2 below). Its colour palette is deliberately **not** adopted: one
  accent (`#1E62AE`) and the cool blue-grey ink ramp stay the only colour
  authority in the product.

## 2. Tokens — the single source of truth

`tokens.css` declares the whole vocabulary under the `--hv-` prefix. Token
categories:

| Category | Example tokens | Notes |
|---|---|---|
| Brand ramp | `--hv-primary(-hover/-active)`, `--hv-primary-50…900` | HomeVault Blue, light → dark |
| Neutral ramp | `--hv-neutral-50…900`, `--hv-text-primary/secondary/tertiary` | cool blue-greys |
| Semantic | `--hv-success`, `--hv-warning`, `--hv-error`, `--hv-info` | status hues + light bg/border/text variants |
| Surfaces | `--hv-surface*`, `--hv-shell-glass/card/fill/border*` | two aligned vocabularies (legacy `surface*` + shell) |
| Typography | `--hv-font-size-*` (content), `--hv-text-*` (compact chrome), `--hv-font-weight-*` (400–900), `--hv-line-height-*`, `--hv-letter-spacing-*` | two deliberate scales, see below; weights/leading/tracking per Apple ref §3/§7/§8 |
| Spacing | `--hv-space-0-5 … 24` (4px grid) + `--hv-section-gap-*`, `--hv-component-gap-*`, `--hv-card-padding-*`, `--hv-content-max-width*` | 4px base grid + the reference's layout rhythm, see §2.2 |
| Radius | `--hv-radius-badge … full`, `--hv-radius-card`, `--hv-radius-control`, `--hv-radius-pill` | 8 / 12 / 18 + pill, see §3.2 |
| Shadows | `--hv-shadow-xs … 2xl`, `--hv-shell-shadow-card` | multi-layer low-alpha black (contact + ambient), see §2.2 |
| Elevation/z-index | `--hv-z-*` | 1000-point increments |
| Motion | `--hv-transition*`, `--hv-ease-*`, `--hv-chrome-*` | transform/opacity only |
| Desktop chrome | `--hv-chrome-*`, `--hv-window-*`, `--hv-winhdr-*`, `--hv-dock-*` | shell geometry |
| Layout breakpoints | `--hv-breakpoint-sm/md/lg` | 734 / 1068 / 1440, see §2.2 |
| Login + wizard gateway | `--hv-login-*` (canvas, orbs, accent, field, timer) + `--hv-wizard-*` (recovery box, create CTA) | **aliases of the Appliance palette** — same accent, ink ramp and canvas as the desktop — see §6.1/§6.2 |
| Settings screen | `--hv-settings-*` (slate ramp, tile/canvas/hairline/border, cat tints + 8 `--hv-settings-cat-*` accents, toggle geometry) | scoped to `routes/Settings.svelte` + `components/settings/` only — see §6.3 |

### 2.1 One text ladder — the native text styles

Every text size in the product resolves to one of these steps. The ladder is
the platform's own desktop text-style set, so a label, a body line, a card
title and a screen title land on the sizes a desktop application uses for
those same functions — not on an ad-hoc web scale.

The **Size** column is the value at the default reader font size (16px); the
token itself is declared in `rem` so it scales — see *The ladder is `rem`*
below before adding or editing one.

| Role token | Size | Weight | Used for |
|---|---|---|---|
| `--hv-type-caption` | 11 | 400/700 | badges, chips, uppercase section eyebrows |
| `--hv-type-footnote` | 11 | 400 | helper text under a field, table meta |
| `--hv-type-subheadline` | 11 | 500/600 | column headers, small labels |
| `--hv-type-callout` | 12 | 400 | secondary prose, hints |
| `--hv-type-body` | 13 | 400 | **default** reading and UI text |
| `--hv-type-headline` | 13 | 600 | emphasised body, button labels |
| `--hv-type-title-3` | 15 | 600/700 | card, panel and window titles |
| `--hv-type-title-2` | 17 | 600 | section titles (`h3`) |
| `--hv-type-title-1` | 22 | 600/700 | page titles (`h2`) |
| `--hv-type-large-title` | 26 | 600 | screen titles (`h1`) |
| `--hv-type-display` | 34 | 600 | empty-state / hero headings |
| `--hv-type-metric` | 20 | 400/700 | stat-card numbers, KPI readouts |

Two documented deviations, both deliberate:

- **11px floor.** The platform's caption step is 10px; nothing readable renders
  below 11px here (WCAG 1.4.4 resizing, and 10px is illegible on the high-DPI
  panels this appliance is administered from).
- **Decorative display numerals** — `--hv-type-gauge` (62px, the dashboard
  gauge) and `--hv-type-clock` / `-clock-sm` / `-clock-wizard` (72 / 48 / 56px,
  the gateway clock). These are visual elements, not body text.

#### The ladder is `rem`, and it scales (2026-10-25, HIG conformance)

The sizes above are the values at the **default** reader font size. Every step
in `tokens.css` is declared in `rem` (`0.6875rem` = 11px, `0.8125rem` = 13px,
…), and `html` is `clamp(100%, 100%, 20px)` — so raising the browser's
configured font size resizes the whole product, up to a supported **1.25×
ceiling**. At 100% the render is pixel-identical to the table, so scaling
costs the design nothing.

This is a requirement, not a nicety:

- **Apple HIG, Layout → Adaptability:** "Be prepared for text-size changes …
  Apps that don't respond to this setting can be difficult or impossible to
  use for people who rely on this feature."
- **WCAG 1.4.4 (Resize text):** a px-only ladder survives *browser zoom* but
  ignores the reader's own font-size preference — the setting a low-vision
  user actually changes. Before 2026-10-25 this screen did exactly that:
  measured identical 11px labels at a 16px **and** a 32px root.

Two rules follow for any new token:

1. **Never declare a type size in `px`.** A px step silently opts out of
   scaling and nothing in the visual design changes at 100%, so it passes a
   visual review and fails the requirement. `rem` or a `calc()` on a `rem`.
2. **A box that holds text is a floor, never a ceiling.** `min-h-11`, not
   `h-11`: at 1.25× the field text is 16.25px, and a fixed 44px box clipped it
   (found and fixed on the login screen the same day). The 44px touch floor
   stays — it is the minimum, so `min-h` satisfies both.

The 20px ceiling is deliberate: past 1.25×, dense desktop chrome (dock, data
tables, window titles) starts to clip at the widths a NAS is administered
from. Both macOS and Windows cap their own text-size setting the same way; a
reader who needs more has browser zoom, which scales the layout too.

Guard: `tests/e2e/design/typography.spec.ts` §6 — every step scales with the
root, nothing clips at the ceiling, the gateway card keeps one deterministic
width, and `html` carries a `clamp()` rather than a px literal. Verified by
mutation: reverting `--hv-type-body` to `13px` turns 6.1 red with
`body at root 20px / Expected: 16.25 / Received: 13`.

**The two vocabularies share this ladder.** `--hv-font-size-*` (content) and
`--hv-text-*` (compact chrome) stay separate *names* because ~380 call sites
read `text-c-*` and the intent (dense chrome vs reading) is still different —
but since 2026-10-18 both resolve to the same steps, so a role renders at one
size no matter which name a call site uses. Merging the names is optional
churn; the values are what bind. `h1`–`h6` map 1:1 onto
`large-title` / `title-1` / `title-2` / `title-3` / `headline` /
`subheadline` in `base.css`, so a bare heading never falls back to a browser
default.

**The guard is executable, not prose.** `tests/e2e/design/typography.spec.ts`
walks the live DOM of the login screen and five apps and fails on any element
rendering its own text at a size outside the ladder, plus a role table (field
label 13, helper 11, card/panel/window title 15, metric 20, uppercase eyebrow
11) and the 11px floor. Verified by mutation: setting `--hv-type-body` to 14px
turns it red and names the offending elements.

**Applying a size/family token in markup (UI audit W1, 2026-09-23).** Sizes go
through the **one** Tailwind bridge declared in `app.css` → `@theme inline`:

- `text-t-*` — **native roles** (`--hv-type-*`), the recommended entry point:
  `text-t-caption`, `text-t-footnote`, `text-t-subheadline`, `text-t-callout`,
  `text-t-body`, `text-t-headline`, `text-t-title-3`, `text-t-title-2`,
  `text-t-title-1`, `text-t-large-title`, `text-t-display`, `text-t-metric`,
  `text-t-gauge`, `text-t-clock` / `-sm` / `-wizard`.
- `text-*` — **content scale** (`--hv-font-size-*`: `text-micro` 11,
  `text-xs` 11, `text-sm` 12, `text-base` 13, `text-md` 15, `text-lg` 17,
  `text-xl` 20, `text-2xl` 22, `text-3xl` 26, `text-4xl` 34, `text-5xl` 62) —
  body copy, headings, form labels.
- `text-c-*` — **compact chrome scale** (`--hv-text-*`: `text-c-xs` 11,
  `text-c-sm` 12, `text-c-base` 13, `text-c-md` 15, `text-c-lg` 17,
  `text-c-xl` 20, `text-c-2xl` 22, `text-c-3xl` 26) plus the role steps
  `text-c-stat` 20, `text-c-card-title` 15, `text-c-label` 13 — table cells,
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

### 2.2 Layout rhythm, elevation, type metrics, breakpoints (Apple reference)

Since 2026-10-19 the *non-colour* rules of the visual language come from an
Apple-style token spec (adopted, but not published with this repository),
translated onto `--hv-*` names in `tokens.css` — that file is the authority,
the section numbers below only trace where each value came from. The Appliance identity is unchanged: opaque paper, one accent,
hairlines, dense-but-legible. What the reference tightened is the *craft*
around it — how much air a page gets, how deep a card sits, how a title is
tracked and led.

**Colours are explicitly out of scope.** The reference's §1 (Apple's
white / `#1D1D1F` / `#F5F5F7` / `#0066CC` set) is **not** adopted: the
Appliance palette (§3) stays the only colour authority. The same goes for the
reference's purple/pink and blue/cyan display gradients — the two gradient
tokens that do exist are sweeps of the HomeVault Blue ramp.

| Reference | HomeVault token(s) | Value | Used for |
|---|---|---|---|
| §2 section gap | `--hv-section-gap` / `-sm` / `-lg` | 100 / 80 / 120px | between major page blocks (settings pages, gateway sections) |
| §2 component gap | `--hv-component-gap-sm/md/lg` | 16 / 32 / 48px | heading → paragraph → control inside one block |
| §2 card padding | `--hv-card-padding` / `-sm` / `-lg` | 32 / 24 / 40px | internal padding of cards and tiles (`.hv-card` takes the 32px default; in-window app bodies keep the 24px compact step) |
| §2 content width | `--hv-content-max-width` / `-lg` | 980 / 1200px | centred reading / gateway column; app windows stay full-bleed |
| §3 title weight | `--hv-font-weight-semibold` / `-bold` | 600 / 700 | headings (600), display/hero headings (700) |
| §3 body weight | `--hv-font-weight-regular` / `-medium` | 400 / 500 | paragraphs, captions/labels |
| §3 CJK steps | `--hv-font-weight-extrabold` / `-black` | 800 / 900 | available for display headings; unused — HomeVault ships en/fr/es/de |
| §5 elevation | `--hv-shadow-xs … 2xl` | contact + ambient pairs | `md` = resting card, `lg` = hover lift / floating window, `2xl` = modal |
| §7 tracking | `--hv-letter-spacing-title` / `-body` | -0.015em / 0 | `h1`–`h3` tracked in, body untracked |
| §8 leading | `--hv-line-height-title` / `-body` | 1.1 / 1.53 | headings (`h1`–`h6`) / prose and `.hv-card-body` |
| §9 breakpoints | `--hv-breakpoint-sm/md/lg` | 734 / 1068 / 1440px | page-level layout bands (phone / tablet / desktop / large desktop) |
| §6 display gradients | `--hv-gradient-text-accent` / `-accent-soft`, `--hv-gradient-bg-light`, `.hv-text-gradient` | accent ramp | one hero/display heading per screen at most; never body copy |

Notes that bind the table:

- **Elevation is two layers, never one hard drop.** Each stop is a tight
  contact shadow plus a wide ambient one, so a card reads crisp on paper
  without a visible "border" of shadow. Depth still comes from shadow alone —
  no `backdrop-filter` anywhere (a dark photo wallpaper must never wash out
  ink).
- **The shadow ladder is retuned in one place.** `--hv-shell-shadow-card`,
  `--hv-window-shadow` and `--hv-modal-shadow` are aliases of `md`, `lg` and
  `2xl`; never give a surface its own shadow value.
- **Section/component gaps are page-level, not window-level.** An in-window
  app body keeps `--hv-window-content-padding` (24px = the compact card
  padding). A NAS console shows tables and forms, not a landing page — the
  reference's 80–120px rhythm belongs to page composition (gateway, settings
  sections, empty states), where it is used.
- **Breakpoints are a contract, not a runtime API.** CSS custom properties
  cannot be read inside `@media`, so the tokens document the bands and the
  media queries spell the literal px. Component-level tweaks may still use
  Tailwind's own utilities (`sm:` 640 / `md:` 768 / `lg:` 1024 / `xl:` 1280).
- **`.hv-text-gradient` degrades safely.** Under `forced-colors: active` or
  `prefers-contrast: high` the clip is dropped and the heading renders as
  plain ink — a clipped gradient is invisible text to those users.

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

**Three families, one value each (2026-10-19).** The corner ladder follows
the reference §4, keeping the appliance's
one-number-per-role discipline: a corner either belongs to a *badge* (an
inline, non-interactive marker: **8px**), to a *control* (something you
click: **12px**), or to a *container* (a surface that holds things:
**18px**) — or it is a capsule/pill. This is enforced on **every button of
every screen**: `desktop.spec.ts` `14.12` walks the whole app registry and
fails any `<button>` / `[role="button"]` whose computed corner is not the
control stop, a circle or a capsule (see the four exceptions below).

The scale used to carry three container stops (16px windows, 24px chrome
panels, 12px cards) and three control stops (6 / 8 / 10px), and the result
was that the same class of object read differently depending on which
component it came from: a window next to a popover looked like two different
products, and a button next to the input it submits did not line up. Both
families are now a single number — every button, input, select, chip and row
takes **12px** (the reference's `--apple-radius-card-sm`), every container
**18px** (`--apple-radius-card`), every non-control badge **8px**
(`--apple-radius-badge`) — so the vendored shadcn `rounded-md`,
`rounded-lg`, `rounded-xl`/`rounded-2xl` and every hand-written
`var(--hv-radius-control)` all resolve to one of them, whatever the button's
size.

| Stop | Value | Usage |
|---|---|---|
| `--hv-radius-badge` | 8px | **THE badge stop** — inline badges, micro chips, key caps (not controls) |
| `--hv-radius-sm` | 8px | alias of the badge stop (keeps `rounded-sm` resolving) |
| `--hv-radius-default` | 12px | **THE control stop** — inputs, buttons, selects, chips, rows |
| `--hv-radius-md` | 12px | alias of the control stop (keeps `rounded-md` resolving) |
| `--hv-radius-lg` | 12px | alias of the control stop (keeps `rounded-lg` resolving) |
| `--hv-radius-xl` | 18px | **the surface stop** (alias) |
| `--hv-radius-2xl` | 18px | **THE surface stop** — windows, widgets, modals, drawers, popovers, cards |
| `--hv-radius-3xl` | 18px | compat alias of `--hv-radius-2xl` |
| `--hv-radius-4xl` | 18px | compat alias of `--hv-radius-2xl` (chrome panels) |
| `--hv-radius-pill` | 980px | **the tag stop** — filter/severity/category chips (`--apple-radius-button`) |
| `--hv-radius-full` | 9999px | true capsules and circular icon buttons |
| `--hv-radius-card` | 18px | semantic card corner (aliases the surface stop) |
| `--hv-radius-control` | 12px | interactive control corner (aliases `--hv-radius-default`) |

Retune the surface stop on `--hv-radius-2xl`, the control stop on
`--hv-radius-default` and the badge stop on `--hv-radius-badge` — the aliases
exist only so the ~40 container, ~76 control and ~20 badge call sites, plus
the Tailwind `rounded-*` bridge in `app.css`, keep resolving. Never give a
container, a control or a badge its own number: that is exactly how the
three-stop drift happened on both sides.

**Buttons — the four exceptions `14.12` allows** (a button that is none of
these takes 12px):

| Case | Corner | Why |
|---|---|---|
| Circular controls (dock tiles, power-sheet tiles, topbar buttons, window trio, avatars) | `50%` | a *shape*, not a corner |
| Capsule chips / pills (launcher categories, filter + severity chips, the user pill) | `9999px` | same — `.hv-pill` / `rounded-pill` names the tag stop |
| Table / list rows with hairline separators (Explorer file list, Journal log) | `0` | the separators define the row; rounding it would notch every hairline |
| Children of a segmented control | `0` | the group clips them (`overflow: hidden`) |

Everything else — including buttons that happen to be large (a selectable
card, a launcher tile, a settings nav row) — takes the control corner. A
`rounded-xl` / `rounded-2xl` utility on a `<button>` is a bug: it silently
puts a surface corner on a control, which is how Storage's disk cards, the
RAID wizard's option cards and Settings' category rows ended up at a surface
corner while the buttons next to them were at the control corner.

**One deliberate deviation from the reference:** it gives
*all* buttons and tags its 980px pill (`--apple-radius-button`). HomeVault
adopts that for **tags** (`--hv-radius-pill`, `.hv-pill`, the chip classes)
but keeps **form buttons on the control corner**. A 44px pill sitting next to
the 12px field it submits breaks the row it lives in, and this is a dense
admin console, not a marketing page. If that ever changes, it is one token
(`--hv-radius-control`) plus `14.12` — not a per-component decision.

**Corner audit (2026-10-19).** Every visible corner reads one of the stops
above — no literal `border-radius` outside the scale. The only literals left
are the 2px micro-radius on the 8–10px legend swatches (`Raid`, `Storage`)
and the tooltip arrow: a data-encoding dot, not a UI corner. The element →
stop mapping the shell and the gateway share:

| Element | Stop |
|---|---|
| App window, tablet-maximized window | `--hv-window-radius` (18px, aliased by `--hv-window-border-radius`) |
| Maximized window on desktop / edge-to-edge phone | `0` (intentional) |
| Launchpad icon tiles | `50%` (circle, like the dock tile) |
| Power action sheet (Shut Down / Restart / Log Out) — circular tiles with the label underneath, `--hv-dock-item-size` diameter | `50%` (the tile) / `--hv-radius-control` (the button) |
| Topbar pill **and** dock pill — capsules, ends are exact semicircles (the full radius clamps to half the pill height); topbar buttons + dock icon tiles — perfect uniform circles | `--hv-radius-full` / `50%` |
| Topbar popovers, power dialog, activity panel | `--hv-radius-4xl` (18px — the surface stop) |
| Desktop widget/monitor panels, in-window cards, modals, drawers, folder pickers | `--hv-radius-card`/`--hv-radius-2xl` (18px) |
| Inputs, buttons, selects, list rows, chips (non-pill) | `--hv-radius-control`/`--hv-radius-md` (12px) |
| Status badges, key caps, non-control micro surfaces (`.hv-badge`) | `--hv-radius-badge`/`--hv-radius-sm` (8px) |
| Filter / severity / category chips, user pill | `--hv-radius-pill` / `--hv-radius-full` |
| Window header icon tile — **perfect circle**, like the dock tiles / topbar buttons | `--hv-winhdr-icon-radius` (`50%`) |
| Window control trio (min / max / close) — **perfect circles**, like the dock tiles / topbar buttons | `--hv-winctrl-radius` (`50%`) |
| Pills, dots, status chips, avatars, spinners | `--hv-radius-full` / `50%` |

Login and SetupWizard are Tailwind-on-tokens: card `rounded-2xl` (18px — the
surface stop), fields/buttons `rounded-md` (12px — the control stop, so the
field and the button under it line up), inner cards `rounded-xl` (18px). They
use no radius outside this map.

## 4. Component architecture — one library, three layers

Every screen is built from the same three layers. **Nothing is written
per-app.**

| Layer | Location | Contents |
|---|---|---|
| Primitives | `lib/components/theme/` | `Button`, `Input`, `Select`, `Toggle`, `Checkbox`, `Radio`, `Alert`, `Badge`, `Spinner`, `Modal`, `Toast`, `Drawer`, `Icon`, `UserAvatar`, `PathField`/`FolderPicker`/`PathPickerField`/`FloatingPathPicker`. Barrel export via `index.ts`. |
| Framework | `lib/components/ui/` | shadcn-svelte generated components — **exactly 11 on disk**: `alert`, `badge`, `button`, `checkbox`, `dialog`, `input`, `radio-group`, `select`, `sonner`, `spinner`, `switch`. The theme primitives delegate to these. Import individual modules, never a barrel. See *What is actually vendored* below before assuming another component exists. |
| Charts | `lib/components/charts/` | LayerChart (Svelte 5 / d3) wrappers themed with `--hv-*` tokens: `DonutChart`, `BarsChart`, `TrendLineChart`. Barrel export via `index.ts`. |
| Page composites | `lib/components/app/` | `PageLayout`, `StatCard`, `DataTable`, `SectionCard`, `EmptyState`, `FormRow` — the shared page anatomy that kills per-app reinvention. |
| Desktop shell | `lib/components/desktop/` | `WindowFrame`, `WindowHeader`, `WindowSidebar`, `WindowToolbar`, `WindowFooter`, `HvLaunchpad`. |

#### What is actually vendored — the list is 11, not 30

The table above used to advertise ~30 generated components (`accordion`,
`calendar`, `collapsible`, `command`, `context-menu`, `date-picker`,
`dropdown-menu`, `input-group`, `pagination`, `popover`, `progress`,
`scroll-area`, `separator`, `skeleton`, `slider`, `tabs`, `textarea`, `toggle`,
`toggle-group`, `tooltip` …). **None of the 19 extra ones exists in the tree,
and none is imported anywhere** — `grep -rl 'components/ui/<name>' frontend/src`
returns 0 for every one of them. The doc described an intention, not the code:
the same class of false invariant as ROADMAP X.4. The directory is the truth:

```
$ ls frontend/src/lib/components/ui/
alert  badge  button  checkbox  dialog  input
radio-group  select  sonner  spinner  switch
```

**Do not generate the missing ones speculatively.** Every pattern they would
cover is already solved in the tree, and a second implementation would be dead
weight to maintain — and a per-app skin, which this section forbids:

| Would-be shadcn component | What the tree actually uses |
|---|---|
| `progress` | a plain `role="progressbar"` bar — `routes/Quotas.svelte`, `routes/Users.svelte`, `routes/Desktop.svelte`, `components/backup/BackupProgressIsland.svelte` |
| `skeleton` | the tokenised `.hv-skeleton` / `.hv-skeleton-text` / `-rect` / `-circle` utilities (`base.css` + `utilities.css`, `--hv-skeleton-bg` / `--hv-skeleton-shimmer`) — used by `components/app/DataTable.svelte` (its `loadingRows`), `routes/Docker.svelte`, `routes/Journal.svelte`, `routes/Explorer.svelte` and `routes/Storage.svelte` |
| `tooltip` | the native `title` attribute and the `aria-label`-only icon buttons of §7.1 (no custom tooltip layer exists) |
| `textarea` | the native `<textarea>` styled by `base.css` (7 screens: Chat, CloudSync, Restic, Docker, Settings, Explorer, Shares) |
| `separator` | the hairline tokens `--hv-shell-border-soft` / `border-t` inside `DataTable` and `SectionCard` |
| `accordion`, `collapsible` | the native `<details>` — `routes/VirtualMachines.svelte` (the resize form). It is the only one: elsewhere, secondary content is a `Modal` rather than an inline disclosure |
| `calendar`, `date-picker` | **nothing needs one yet** — no `<input type="date">`/`datetime-local` and no date-picker exists anywhere in the tree. Ship dates as text or via the server's own scheduling form until a screen genuinely requires calendar entry |
| `tabs`, `popover`, `dropdown-menu` | the desktop shell's own `WindowSidebar` / `WindowHeader`, plus the two `position: fixed` card popovers of the Docker screen |
| `slider` | numeric `<input type="number">` and the range fields of the Settings cards |
| `command`, `pagination`, `scroll-area`, `context-menu`, `input-group`, `toggle`, `toggle-group` | unused concepts — the launcher has its own search, `DataTable` paginates server-side, scrolling is native, and `Toggle` is a `theme/` **primitive**, not a shadcn component |

**The rule for the future.** Generate a shadcn component *the day a screen
needs it* — `npx shadcn-svelte@latest add <name>` (`components.json` is wired:
style `vega`, `tailwind.css: src/app.css`, base colour `neutral`) — and add it
to the list above in the same change. A component that exists without a caller
is a bug in this document, not an asset.

**Icon libraries — the boundary is enforced.** `@tabler/icons-svelte` is the
product's set; `@lucide/svelte` is allowed **only** inside `components/ui/**`
(the generated primitives import it, see §4.6/§721 of the icon rules). Verified
by grep: zero `@lucide/svelte` imports outside `ui/`. `@phosphor-icons/web`
feeds the generated sprite (`npm run icons:generate`).

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

Below all of that, **`WindowFrame` adds one 36px frame footer to every
window** (2026-10-18): help on the far left, the SE resize grip on the far
right, both 28px `.hv-ic` icon buttons on the shared focus ring, both with a
**14px glyph** (`--hv-winctrl-glyph` — the min/max/close trio's step, so the
header row and the footer strip are one icon size; the strip shipped at 16px
and read a size larger than the bar). It is chrome
— apps never render it and never pass content into it; an app's own
`WindowFooter` stays inside the page, above it. See
[desktop.md](./desktop.md#frame-footer--help--resize-grip-2026-10-18).

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
exactly `--hv-winhdr-h` (44px). Both are **perfect circles**
(`--hv-winhdr-icon-radius` and `--hv-winctrl-radius` are `50%`, the
dock/topbar shape language) — the whole window chrome speaks one shape, and a
labelled button inside the chrome still takes `--hv-radius-control` (8px).
`tabsLabel` / `sidebarLabel` (accessible
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
- **Each pill owns its height** — and since the 2026-10-18 height pass both
  are **72px** on desktop/tablet (**52px** dock on phone, below the topbar's
  56px): `--hv-chrome-topbar-h` / `--hv-chrome-dock-h`. Both are
  `box-sizing: border-box`, so the tokens are the rendered heights — which is
  what makes `--hv-chrome-topbar-keepout` / `--hv-chrome-dock-keepout` exact.
  Everything anchored to the dock (`.hv-activity-panel`, the ≤900px maximized
  inset) derives from the **dock's** band.
- Window corners: `--hv-window-radius` (18px, the surface stop). Maximized
  windows hide the
  topbar; on ≤900px they inset above the dock.
- **The app window is the "AppWindow" mock (2026-09-14, Figma "Sans titre"
  node `3:76`)** — the exact spec lives in `desktop.md` → *In-window anatomy*.
  In one line: an opaque white frame in a **slate-300** hairline
  (`--hv-window-border` `#CBD5E1`, deliberately darker than the `--hv-shell-*`
  card hairline) under the mock's own two-layer shadow, a constant **44px**
  header bar, and the amber / blue / red control trio **inline in that bar** at
  28px — no longer 48px circles floating over the content (they are 28px
  **circles** again since 2026-10-18: `--hv-winctrl-radius: 50%`, like the dock
  tiles and topbar buttons).
- **Chrome is Figma-inspired (approved gateway mocks) (2026-09-13, deepened 2026-09-14):** window
  controls are traffic lights (`--hv-winctrl-*`: amber minimize, blue
  maximize, red close) instead of neutral circles, on the mock's own stops
  (`#FFB900` / `#2B7FFF` / `#FF6467` — **not** the semantic
  `--hv-warning`/`--hv-primary`/`--hv-error`); the dock running indicator
  is a full-width bar in the app's solid hue (`--hv-app-<id>-c`,
  `.hv-dock-dot` + `--dock-c`), which must stay the dark stop of that app's
  `--hv-app-<id>-grad` (E2E `14.2`); topbar buttons are flat **circles**
  (`--hv-chrome-topbar-btn-size`: 56px, 0.775 × the pill)
  on the pill — icon-only at every width, no tile / border / shadow, hover
  `--hv-subtle-hover`, `border-radius: 50%` — and
  the profile avatar is a 36px circle (32px on the phone). Dock tiles are
  56px **perfect circles** (`border-radius: 50%`, uniform across apps; 44px on
  phone) — the
  diameter is 0.775 × the 72px dock bar, the proportion measured on the
  approved reference dock — inside
  the dock's **capsule**, and the topbar's three buttons are **perfect
  circles** too (56px — 0.775 × the pill, the dock tile's own ratio; 44px on
  the phone; icon-only, name on `aria-label` + the shared CSS tooltip)
  inside the topbar's **capsule** — both pills use `--hv-radius-full`, so their
  ends are exact semicircles. Spring hover lift + `brightness(1.05)` and a
  CSS tooltip on `--hv-tooltip-bg` (suppressed under `@media (hover: none)`).
  Palette stays the HomeVault appliance stop set, with the chrome border/shadow
  as the one dedicated exception (see `desktop.md`).
- **Power action sheet = the dock's tiles (2026-10-19).** The sheet opened by
  the topbar Power button (Shut Down / Restart / Log Out) is not a row of
  cards: each action is a **circular gradient tile** the size of a dock tile
  (`--hv-dock-item-size`, so 56px desktop / 48px phone), same circle, same
  glyph/tile ratio (half the tile), same card shadow and the same hover lift +
  `brightness(1.05)` as `.hv-dock-icon`, with the label underneath. One hue
  per action — red `--hv-error`, blue `--hv-primary`, slate
  `--hv-neutral-700`→`900` — so the three never collapse into one flat tile;
  every glyph is white on its own gradient (≥ 4.9:1, 3:1 required). The button
  around the tile paints nothing, keeps the one control corner and carries the
  shared focus ring, so the shape language stays the dock's while E2E `14.12`
  still sees a control corner. E2E `desktop.spec.ts` `7.5` pins it.
- **Page-scoped palette `--hv-launcher-*`** (Launchpad, 2026-09-16, Figma
  "Sans titre" node `23:6646`). Like `--hv-settings-*` / `--hv-login-*` /
  `--hv-wizard-*`, it is read by exactly one component
  (`HvLaunchpad.svelte`). It exists because the launcher is the one surface
  that sits on a **dark translucent scrim** rather than opaque paper, so the
  `--hv-shell-text-*` ink set does not apply. Two mock values were contrast-
  corrected on the way in (scrim `0.55` → **`0.72`** → **`0.86`** since
  2026-10-22, inactive chip text 85% white → **opaque white**), both documented
  on the token block; the
  visible 32px chip keeps a 44px transparent `::after` pad and the search
  input is a full `--hv-touch-target-min` inside the mock's 56px bar. Its
  scrim is the one blurred surface in the shell (`blur(6px)`, up from the
  mock's 2px for the same reason as the alpha bump: the desktop stayed
  recognisable behind it), and it declares the standard
  `backdrop-filter` **alone** — pairing it with `-webkit-backdrop-filter` made
  the minifier keep only the prefixed form, which Firefox ignores (see
  `desktop.md`). Its chip row grows to fill the content column
  (`flex: 1 1 auto`) so the category module is never visibly narrower than the
  tile grid it filters; the ≤640px block undoes it for the phone's sideways
  scroller. The
  tiles are the per-app `--hv-app-<id>-grad`, never the mock's flat white —
  §6 is binding.
- The desktop wallpaper is a photo (or the light blue-grey canvas gradient
  `--hv-desktop-bg`) — the login screen and the setup wizard paint that same
  canvas token, so the three can never drift apart.
- Every dock/topbar/launcher control is ≥44px; `:hover` lifts flatten under
  `@media (hover: none)`.

### 5.1 Form controls stay readable on touch — no iOS auto-zoom

iOS Safari auto-zooms into any `input`/`select`/`textarea` whose font-size is
**below 16px** on focus. The native body step is 13px — far under that
threshold — so `base.css`'s `@media (any-pointer: coarse)` block forces form
text to a **literal 16px** on touch pointers. Desktop/mouse keeps the 13px
body step. This rule must not be removed or lowered — a tap into a login or
settings field must never zoom the page (the reference shells ship 17px;
HomeVault uses the exact 16px threshold).

That 16px is deliberately a literal and **not** `--hv-font-size-md`: the token
used to be 16px but is now the native title-3 step (15px), so pointing the
rule at it would silently reintroduce the auto-zoom bug (2026-10-18).
E2E: `login.spec.ts` §12 asserts the computed font-size stays ≥16px on a
coarse pointer.

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

The **login and setup-wizard screens** are the product's front door: a
full-screen canvas gradient (`--hv-login-bg-gradient`) with two blurred orbs
(`--hv-login-orb-*`), a large live 24 h clock, and a compact white card
(380 px login / 520 px wizard).

Since **2026-10-17** they no longer run a palette of their own. They used to
carry a page-scoped indigo/violet/pink "gateway" identity from an earlier
mock; every value now **derives from the Appliance palette** the desktop
shell, the dock/topbar chrome and every in-window app already use:

- one accent — `--hv-login-accent` **is** `--hv-primary` (#1E62AE), and
  `--hv-login-accent-soft-border` is the same hue (no second accent, no
  ladder);
- one canvas — `--hv-login-bg-gradient` **is** `--hv-desktop-bg`, the
  desktop's own gradient, declared once in `tokens.css` and read by
  `Desktop.svelte` too;
- one ink ramp — `--hv-login-clock/muted/faint` alias `--hv-shell-text-*` /
  `--hv-shell-border`, and the card borrows the desktop window's hairline
  (`--hv-window-border`) and elevation (`--hv-shadow-xl`);
- the semantic hues alias their Appliance tokens (`--hv-login-error` =
  `--hv-error`, `--hv-login-timer-warn` = `--hv-warning`, …).

The `--hv-login-*` names stay because both screens read them; a component may
still use an Appliance token directly (the recovery modal's fields do). The
contrast floors hold everywhere: accent 6.15:1 on white and 4.99:1 on
`--hv-login-accent-soft`, both gradient stops ≥4.69:1 under their white glyph
/ label. Guard: `login.spec.ts` **13.6**, which also asserts the aliases
(`--hv-login-accent` === `--hv-primary`, gateway canvas === desktop canvas).

- `frontend/src/routes/Login.svelte` is pure **Tailwind-on-tokens** (no scoped
  `<style>`), built from the shared `Button`/`Toggle`/`Modal` (shadcn-svelte /
  Melt UI) components + native inputs kept for autofill. The primary CTA uses
  the theme `Button` with `bg-[image:var(--hv-login-btn-grad)]!` — and so does
  every other primary button on the screen (recovery, retry, modal footer), so
  one screen has one button treatment.
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
amber recovery box (`--hv-wizard-recovery-*`) and the green "create account"
CTA (`--hv-wizard-create-grad`). Its text runs on the **Appliance ink ramp**
(`--hv-wizard-text-heading/title/label/soft/body` → `--hv-shell-text-*`) —
since 2026-10-17 the wizard no longer drifts a step lighter than the rest of
the product — and its fields are **white** with a hairline shadow (unlike the
login's `#F7F8FA` fields). Error borders/text use `--hv-wizard-error`
(= `--hv-error`, #C22B21) and update live (username invalid → red / valid →
brand blue, confirm mismatch → red / match → green).

**2026-10-17 — the emerald family was replaced by the Appliance success ramp.**
The wizard's own `#10B981` / `#22C55E` / `#16A34A` / `#10B981→#059669` stops
failed WCAG 1.4.3 / 1.4.11 in every role they carried ink: 2.54:1 as the
welcome feature glyph, 2.28:1 as the strength label and behind the white
check circles, and 2.54 → 3.77:1 under the white "create account" label.
`--hv-wizard-feature-emerald` / `-success` / `-success-strong` /
`-create-grad` now alias `--hv-success` (#1E7E34, 5.14:1) and
`--hv-success-text` (#166428, 7.27:1). The amber recovery box became white
paper with a 2px `--hv-warning` border, its copy on `--hv-warning-text` /
`--hv-warning`, and only the code chip keeps a tint (`--hv-warning-light`,
code text 6.32:1 on it) — the box's own tint used to leave the 12 px
description at 4.39:1. Same styling rules as §6.1 — Tailwind-on-tokens, no
scoped `<style>`, trailing-`!` modifiers on raw `input`/`button` utilities
(including the `h1` color, which `base.css`'s unlayered `h1 { color }` rule
would otherwise override), gradients as `bg-[image:var(--hv-…)]!`.

- **Page layout (2026-10-16).** The root is a one-column grid
  (`grid-rows-[minmax(auto,1fr)_auto_minmax(3rem,1fr)]`): the live clock owns
  row 1 — its `auto` minimum *is* the clock's rendered height, so no
  hard-coded band value can drift from it — and the 520 px card owns row 2.
  The two `1fr` rows split the leftover space evenly, so the card block is
  centred on both axes at every resolution. When the card is taller than the
  leftover space the split collapses, the card starts right under the clock and
  the page scrolls from the card's top: never clipped, never under the clock.
  Covered by `setup.spec.ts` §10 (centring + no-overflow assertions).
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
  uses only the first two: **16** for glyphs inline with copy, rail entries,
  icon tiles and plain icon buttons, **20** for a dedicated icon button
  (the AI refresh control). 24 is
  the next step for larger contexts (`w-10` tiles, empty states) and never
  appears here. 2026-09-26: the stray `14`, `15` and `18` values were
  removed from `Settings.svelte` and `components/settings/*` — `size={15}`
  was never on the grid, and five different sizes for one screen's icons
  is not a scale. **2026-10-19: the same three values came back through
  `components/theme/PathPickerField.svelte` (`14`) and the MCP card (`15`,
  `18`), which this screen embeds — they are 16 now, and E2E 23.12 walks all
  8 categories and fails on any glyph that is not 16 or 20.**
- **One icon tile on the whole screen (2026-10-19).** Every decorative tile
  — the rail category tile, the 11 Widgets rows, the 3 Privacy rows — is a
  **circle at `--hv-settings-icon-tile` (28px)** holding a 16px glyph, built
  from `settingsIconTile` in `vocabulary.ts` (the rail's comes from
  `WindowSidebarItem`'s rich CSS off the same token). It used to be three
  components: 28px tiles with a 12px corner, and 36px tiles with an 18px
  corner and a 20px glyph. The circle is the same shape language as the dock
  tiles / topbar buttons / window-header icon, and the single size is what
  makes a screen built from one vocabulary read as one screen — E2E 23.12
  asserts every tile is the token's square box, that all tiles share one
  size, and that the radius resolves to at least half the box.
- **Layout contract**: 208px rail (`--hv-settings-sidebar-w`), a non-scrolling
  two-line content header (title over subtitle, no icon tile since
  2026-10-19), then `[data-testid="settings-content"]` = the scrolling
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
   **The trap is not heading-only:** `base.css` also declares an unlayered
   `p { margin-bottom: var(--hv-space-4) }`, so `class="m-0"` on a `<p>`
   loses too and the element keeps a 16px bottom margin. That is invisible in
   the markup and in `svelte-check`; it was found on 2026-10-19 by measuring
   the Settings headers (73px where the classes said 57px). Write `mb-0!`
   (or `m-0!`) on any `<p>` whose box must be tight.
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
9. **Non-colour craft follows the Apple reference (§2.2); colour does not.**
   Spacing rhythm, the 8/12/18 corner ladder, elevation, weights, leading,
   tracking and the 734/1068/1440 breakpoints come from that spec. Its
   palette, its other-hue display gradients and its pill form buttons are
   deliberately not adopted — see §2.2 and §3.2 for the reasoning. A new
   visual value is declared in `tokens.css` as an `--hv-*` token first, never
   written as a literal in a component.

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
  Two surfaces are sanctioned exceptions, both because the brand ring cannot
  be *read* there: the launcher's chips and tiles (white 2px ring + dark halo
  on the dark scrim) and, since 2026-10-19, the launcher's **search bar**
  (`inset 0 0 0 2px rgba(15,23,42,0.72)` — the bar is auto-focused on open, so
  the brand ring was a permanent blue halo; the rim measures 7.1:1 on the
  capsule and adds no second hue). Any other "the ring looks bad here"
  request is answered by picking a *different visible* indicator, never by
  deleting the last one. See [desktop.md](./desktop.md#launcher-launchpad-figma-sans-titre-node-236646-2026-09-16).
- **Contrast floor.** Any token used for *text* must be ≥4.5:1 on every
  surface it renders on; any status dot / focus / boundary colour ≥3:1.
  The audited text tokens already sit above the floor (`--hv-shell-text-3`
  #5F6B76, `--hv-login-muted` = `--hv-shell-text-3`, `--hv-settings-label`
  #64748B, `--hv-shell-border-strong` #87929E, badge bg #DC2626, wizard/timer
  hues).
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
  solid active — §6.3), and the gateway ladder renders ≥4.5:1 wherever it
  carries text (`accent` `#1E62AE`, 4.99:1 on `accent-soft`, both gradient
  stops ≥4.69:1 under white) — the wizard's own emerald family did not and was
  replaced by the Appliance success ramp on 2026-10-17 (§6.2). Runtime guards:
  `groups.spec.ts`
  16.1 (avatars), `settings.spec.ts` 17.5 (rail tiles), `login.spec.ts`
  13.6 (gateway tokens — accent ladder, wizard ink ramp, success/warning
  families, recovery box, all three gradients) — each reads live values,
  never hardcoded RGB.
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

### 7.2 Simplicity invariants — the 7 rules (2026-10-20)

HomeVault is an appliance for a home user. **Simple is a correctness bar, like
accessibility**: a screen that works but needs a manual, offers ten options for
a one-option task, or hides its main action behind jargon is not done. Review
every new or edited screen against the seven rules below; they are binding, and
the summary lives in `CLAUDE.md` → "Simplicity rules".

1. **Immediately understandable.** A first-time user gets what the screen is
   for and what to do without a manual:
   - a plain-language `<h1>` (and one per screen — §7.1), plus one sentence of
     context in the subtitle when the title alone is ambiguous;
   - explicit text labels — an ambiguous icon-only button is never the primary
     action (icon-only stays fine for the compact secondary controls, with an
     `aria-label`, §7.1);
   - no internal jargon in user-facing copy (`RAID array degraded` → "One disk
     has failed — your data is still safe"; protocol names only where the user
     must choose between them);
   - an empty state that says what to do next, not just "No data";
   - test: header + primary button must explain the screen in one sentence.
2. **Only what the task needs.** Drop buttons, fields, options and status rows
   the current task does not use. Simplicity does **not** mean removing
   features — it means not imposing them on everyone: move the rest behind an
   "Advanced" disclosure (`i18n` `common.advanced` / `*.w_advanced`), a details
   panel, or the item's own window. A modal that scrolls is a smell: the
   secondary fields belong in an Advanced section.
3. **Fewer steps, less effort.** Prefill sensible values (the path the user
   came from, the current share, a recommended retention), remember the last
   choice for repeat actions, pick a relevant default, and never ask for
   something the server already knows. No confirmation step that only repeats
   what the user just typed; no wizard step whose answer is always the same.
4. **Progressive complexity.** Essentials first; advanced settings stay
   reachable (collapsed section, "Advanced" toggle, secondary route, the CLI /
   API) but never sit in the beginner's default path. The default path of every
   wizard completes with zero advanced input.
5. **Predictable and consistent.** The same label means the same behaviour
   everywhere (`Save` applies, `Cancel` discards, `Delete` removes) and the
   same button variant sits in the same place on every screen (`Save` in the
   rail footer, `Cancel`/confirm order in modal footers — §4.3). Important
   actions are where they are expected, and every action returns immediate
   visible feedback (pending state, toast, updated row) — never a silent click.
6. **Reassuring and reversible.** Prefer **Undo** / revert-last-action /
   recoverable errors (trash + restore, soft delete, cancelling a running job)
   over confirmation dialogs. Confirm **only** when the action is genuinely
   destructive or hard to reverse, and the dialog names the consequence and the
   affected item ("Delete share `media`? 3 accounts lose access."). Destructive
   styling (`--hv-danger`) is reserved for those actions.
7. **Adapted to the user.** A beginner finishes the common tasks on the default
   path; an experienced user reaches shortcuts and power features (keyboard
   shortcuts — `docs/guides/shortcuts.md`, multi-select, bulk actions, CLI/API
   parity) without those cluttering the beginner path. Never require a beginner
   to understand an expert concept to complete a common task.

Applying these rules must never break §7.1: a control hidden behind "Advanced"
is still labelled, focusable and 44px, and collapsing a section does not remove
it from the keyboard order.

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
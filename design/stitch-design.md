---
name: HomeVault
colors:
  surface: '#f7f9fb'
  surface-dim: '#d8dadc'
  surface-bright: '#f7f9fb'
  surface-container-lowest: '#ffffff'
  surface-container-low: '#f2f4f6'
  surface-container: '#eceef0'
  surface-container-high: '#e6e8ea'
  surface-container-highest: '#e0e3e5'
  on-surface: '#191c1e'
  on-surface-variant: '#434655'
  inverse-surface: '#2d3133'
  inverse-on-surface: '#eff1f3'
  outline: '#737686'
  outline-variant: '#c3c6d7'
  surface-tint: '#0053db'
  surface-variant: '#e0e3e5'
  primary: '#2E72C9'
  on-primary: '#ffffff'
  primary-container: '#2563eb'
  on-primary-container: '#eeefff'
  inverse-primary: '#b4c5ff'
  secondary: '#505f76'
  on-secondary: '#ffffff'
  secondary-container: '#d0e1fb'
  on-secondary-container: '#54647a'
  tertiary: '#943700'
  on-tertiary: '#ffffff'
  tertiary-container: '#bc4800'
  on-tertiary-container: '#ffede6'
  error: '#C4453C'
  on-error: '#ffffff'
  error-container: '#f9dcd9'
  on-error-container: '#7c1f19'
  primary-fixed: '#dbe1ff'
  primary-fixed-dim: '#b4c5ff'
  on-primary-fixed: '#00174b'
  on-primary-fixed-variant: '#003ea8'
  secondary-fixed: '#d3e4fe'
  secondary-fixed-dim: '#b7c8e1'
  on-secondary-fixed: '#0b1c30'
  on-secondary-fixed-variant: '#38485d'
  tertiary-fixed: '#ffdbcd'
  tertiary-fixed-dim: '#ffb596'
  on-tertiary-fixed: '#360f00'
  on-tertiary-fixed-variant: '#7d2d00'
  background: '#f7f9fb'
  on-background: '#191c1e'
  success: '#2E9C6B'
  success-container: '#d1fae5'
  on-success-container: '#065f46'
  warning: '#C98A2E'
  warning-container: '#fef3c7'
  on-warning-container: '#92400e'
  info: '#2E72C9'
  info-container: '#dbeafe'
  on-info-container: '#1e40af'
  surface-window: '#ffffff'
typography:
  headline-xl:
    fontFamily: Inter
    fontSize: 30px
    fontWeight: '700'
    lineHeight: 36px
    letterSpacing: -0.02em
  headline-lg:
    fontFamily: Inter
    fontSize: 24px
    fontWeight: '700'
    lineHeight: 30px
    letterSpacing: -0.01em
  headline-md:
    fontFamily: Inter
    fontSize: 20px
    fontWeight: '700'
    lineHeight: 28px
    letterSpacing: -0.01em
  headline-sm:
    fontFamily: Inter
    fontSize: 18px
    fontWeight: '600'
    lineHeight: 24px
    letterSpacing: 0
  body-lg:
    fontFamily: Inter
    fontSize: 16px
    fontWeight: '400'
    lineHeight: 24px
  body-md:
    fontFamily: Inter
    fontSize: 14px
    fontWeight: '400'
    lineHeight: 20px
  body-sm:
    fontFamily: Inter
    fontSize: 13px
    fontWeight: '400'
    lineHeight: 18px
  label-lg:
    fontFamily: Inter
    fontSize: 14px
    fontWeight: '600'
    lineHeight: 20px
  label-md:
    fontFamily: Inter
    fontSize: 13px
    fontWeight: '600'
    lineHeight: 18px
    letterSpacing: 0.01em
  label-caps:
    fontFamily: Inter
    fontSize: 12px
    fontWeight: '700'
    lineHeight: 16px
    letterSpacing: 0.025em
  button:
    fontFamily: Inter
    fontSize: 14px
    fontWeight: '600'
    lineHeight: 20px
  mono:
    fontFamily: 'JetBrains Mono'
    fontSize: 13px
    fontWeight: '400'
    lineHeight: 18px
rounded:
  sm: 2px
  DEFAULT: 4px
  md: 6px
  lg: 8px
  xl: 12px
  2xl: 16px
  3xl: 22px
  4xl: 28px
  full: 9999px
spacing:
  base: 4px
  xs: 8px
  sm: 12px
  md: 16px
  lg: 24px
  xl: 32px
  2xl: 48px
  3xl: 64px
  gutter: 24px
  margin-mobile: 20px
  margin-desktop: auto
---

## Brand & Style

HomeVault is a self-hosted homelab management platform. The design system is built on **Frosted Glass Modernism** — a clean, layered aesthetic where translucent surfaces, the wallpaper bleeding through, and a single accent blue create a sense of depth and calm authority. The UI is intentionally sparse: generous whitespace, a strict typographic hierarchy, and subtle tonal shifts over heavy shadows. The emotional response should be one of **secure efficiency** — this is a tool that manages real infrastructure.

The design language splits into two tiers: the **Desktop Shell** (floating dock, topbar pill, window chrome — all frosted glass over the wallpaper) and the **In-Window App Shells** (each app's content area, also frosted glass, with dark navy text). Both share the same material vocabulary (`--hv-shell-*` tokens).

## Colors

The palette is anchored by a single professional blue — `#1E62AE` (HomeVault Blue, `--hv-primary`) — used everywhere as the accent. No per-app accent colors; every dock tile, window header icon, and Launchpad tile shares this one gradient (`--hv-app-accent-grad: linear-gradient(145deg, #3275C4, #1E62AE)` — both stops stay ≥4.5:1 with white so tiles can carry white type).

### Surface System (Material Design 3)

| Token | Hex | Usage |
|---|---|---|
| `surface` | `#f7f9fb` | Global background canvas |
| `surface-dim` | `#d8dadc` | Dimmed/inactive surfaces |
| `surface-container-lowest` | `#ffffff` | Pure white (modals, popups) |
| `surface-container-low` | `#f2f4f6` | Subtle containers |
| `surface-container` | `#eceef0` | Default containers |
| `surface-container-high` | `#e6e8ea` | Elevated containers |
| `surface-container-highest` | `#e0e3e5` | Highest elevation (toolbar bg) |

### Glass Shell Surfaces (Desktop + In-Window)

| Token | Value | Usage |
|---|---|---|
| `--hv-shell-glass` | `rgba(255,255,255,0.72)` | Window/panel surface |
| `--hv-shell-card` | `rgba(255,255,255,0.55)` | Card/tile on window |
| `--hv-shell-fill` | `rgba(255,255,255,0.5)` | Toolbar/chip fill |
| `--hv-shell-blur` | `30px` | Backdrop blur radius |
| `--hv-shell-border` | `rgba(27,32,80,0.12)` | Standard hairline |
| `--hv-shell-border-soft` | `rgba(27,32,80,0.07)` | Faint divider |
| `--hv-shell-border-strong` | `rgba(27,32,80,0.18)` | Button/input outline |
| `--hv-shell-text-strong` | `#1b2050` | Primary text on glass |
| `--hv-shell-text-2` | `rgba(27,32,80,0.74)` | Secondary text |
| `--hv-shell-text` | `rgba(27,32,80,0.6)` | Muted body text |
| `--hv-shell-text-3` | `rgba(27,32,80,0.45)` | Placeholder text |

### Text Colors

| Token | Hex | Usage |
|---|---|---|
| `on-surface` | `#191c1e` | Primary text (navy-black) |
| `on-surface-variant` | `#434655` | Secondary text |
| `text-tertiary` | `#6b7280` | Muted/placeholder text |
| `on-background` | `#191c1e` | Text on background |

### Semantic Colors

| Color | Primary | Container | On-Container |
|---|---|---|---|
| **Primary** | `#2E72C9` | `#2563eb` | `#eeefff` |
| **Secondary** | `#505f76` | `#d0e1fb` | `#54647a` |
| **Tertiary** | `#943700` | `#bc4800` | `#ffede6` |
| **Error** | `#C4453C` | `#f9dcd9` | `#7c1f19` |
| **Success** | `#2E9C6B` | `#d1fae5` | `#065f46` |
| **Warning** | `#C98A2E` | `#fef3c7` | `#92400e` |
| **Info** | `#2E72C9` | `#dbeafe` | `#1e40af` |

### Outlines

| Token | Hex | Usage |
|---|---|---|
| `outline` | `#737686` | Borders, dividers |
| `outline-variant` | `#c3c6d7` | Subtle borders |
| `border` | `#e5e7eb` | Default card/section border |
| `border-hover` | `#d1d5db` | Hovered border |

### Primary Scale (Extended)

| Token | Hex | Usage |
|---|---|---|
| `primary-50` | `#eff6ff` | Lightest tint |
| `primary-100` | `#dbeafe` | Light tint |
| `primary-200` | `#bfdbfe` | Subtle tint |
| `primary-300` | `#93c5fd` | Medium tint |
| `primary-400` | `#60a5fa` | Accent tint |
| `primary-500` | `#2E72C9` | Base (same as `--hv-primary`) |
| `primary-600` | `#1e5bb8` | Hover state |
| `primary-700` | `#003590` | Active state |
| `primary-800` | `#1e4485` | Dark variant |
| `primary-900` | `#1e3a8a` | Darkest variant |
| `primary-login` | `#004ac6` | Login-specific blue |

### Neutral Scale (Full Range)

| Token | Hex | Usage |
|---|---|---|
| `neutral-50` | `#f9fafb` | Lightest |
| `neutral-100` | `#f3f4f6` | Light |
| `neutral-200` | `#e5e7eb` | Border default |
| `neutral-300` | `#d1d5db` | Border hover |
| `neutral-400` | `#9ca3af` | Disabled text |
| `neutral-500` | `#6b7280` | Muted text |
| `neutral-600` | `#4b5563` | Secondary text |
| `neutral-700` | `#374151` | Strong text |
| `neutral-800` | `#1f2937` | Near-black |
| `neutral-900` | `#111827` | Black |

## Typography

The system uses **Inter** as the primary sans-serif and **JetBrains Mono** for monospace contexts (terminal, code blocks). Two type scales exist by design — never merge them:

### Content Scale (`--hv-font-size-*`)
For body copy, card/section headings, form labels, and anything the user reads.

| Token | Size | Weight | Line Height | Usage |
|---|---|---|---|---|
| `micro` | 10px | 400 | 14px | Overline badges |
| `xs` | 11px | 400 | 14px | Helper text |
| `sm` | 12px | 400 | 16px | Secondary labels |
| `base` | 14px | 400 | 20px | Default body |
| `md` | 16px | 400 | 24px | Primary body |
| `lg` | 18px | 500 | 24px | Card titles |
| `xl` | 20px | 600 | 28px | Section headings |
| `2xl` | 24px | 700 | 30px | Page headings |
| `3xl` | 28px | 700 | 34px | Hero text |
| `4xl` | 30px | 700 | 36px | Display text |

### Compact Chrome Scale (`--hv-text-*`)
Dense UI elements — table cells, chips, meta rows, dock labels.

| Token | Size | Weight | Line Height | Usage |
|---|---|---|---|---|
| `xs` | 11px | 400 | 14px | Tiny labels |
| `sm` | 12px | 400 | 16px | Compact labels |
| `base` | 13px | 400 | 18px | Table cells |
| `md` | 14px | 400 | 20px | Chips, meta |
| `lg` | 16px | 500 | 22px | Dense headings |

### Font Weights

| Name | Value | Usage |
|---|---|---|
| Regular | 400 | Body text |
| Medium | 500 | Emphasis, secondary headings |
| Semibold | 600 | Labels, buttons |
| Bold | 700 | Headings, page titles |

## Layout & Spacing

Built on a **strict 4px base grid**. Every dimension is a multiple of 4px.

### Spacing Scale

| Token | Value | Usage |
|---|---|---|
| `0.5` | 2px | Micro gaps |
| `1` | 4px | Inline spacing |
| `1.5` | 6px | Tight gaps |
| `2` | 8px | Compact spacing |
| `2.5` | 10px | Small gaps |
| `3` | 12px | Standard compact |
| `4` | 16px | Default spacing |
| `5` | 20px | Mobile safe margin |
| `6` | 24px | Section gutters |
| `8` | 32px | Large gaps |
| `10` | 40px | Major sections |
| `12` | 48px | XL sections |
| `14` | 56px | 2XL sections |
| `16` | 64px | Page margins |
| `20` | 80px | 3XL sections |
| `24` | 96px | Maximum spacing |

### Chrome Inset

| Token | Value | Usage |
|---|---|---|
| `chrome-inset` | 24px (12px on mobile) | Space from screen edge to dock/topbar |
| `chrome-topbar-h` | 90px (72px on mobile) | Topbar pill height |
| `chrome-panel-gap` | 8px | Gap between topbar and opened panel |

## Elevation & Depth

HomeVault uses **Tonal Layering + Soft Blurs** over heavy shadows. The glass morphism approach means depth is conveyed through blur, opacity, and tonal shifts rather than drop shadows.

### Shadow Scale

| Token | Value | Usage |
|---|---|---|
| `shadow-sm` | `0 1px 2px rgba(25,28,30,0.04)` | Subtle lift |
| `shadow-xs` | `0 1px 2px rgba(0,0,0,0.05)` | Minimal lift |
| `shadow-base` | `0 2px 4px rgba(0,0,0,0.1)` | Base elevation |
| `shadow-md` | `0 4px 12px rgba(0,0,0,0.06)` | Cards, panels |
| `shadow-lg` | `0 8px 24px rgba(25,28,30,0.08)` | Floating elements |
| `shadow-xl` | `0 12px 40px rgba(25,28,30,0.12)` | Modals |
| `shadow-2xl` | `0 25px 50px -12px rgba(0,0,0,0.25)` | Maximum elevation |
| `shadow-inner` | `inset 0 2px 4px 0 rgba(0,0,0,0.06)` | Inset shadow |
| `shadow-glass` | `0 8px 32px rgba(0,0,0,0.12)` | Glass panel shadow |
| `shadow-glass-hover` | `0 12px 48px rgba(0,0,0,0.16)` | Hovered glass |
| `shadow-widget` | `0 4px 24px rgba(0,0,0,0.08)` | Widget shadow |
| `shadow-dock-app` | `0 2px 12px rgba(0,0,0,0.15)` | Dock app hover |
| `shadow-focus` | `0 0 0 3px rgba(46,114,201,0.1)` | Focus ring |
| `shadow-focus-thin` | `0 0 0 2px rgba(46,114,201,0.1)` | Thin focus ring |

### Glass Surfaces

| Token | Value | Usage |
|---|---|---|
| `glass-blur` | 24px | Default backdrop blur |
| `glass-blur-strong` | 40px | Launcher overlay |
| `glass-panel-bg` | `rgba(255,255,255,0.75)` | Panel surface |
| `glass-panel-bg-hover` | `rgba(255,255,255,0.85)` | Hovered panel |
| `glass-widget-bg` | `rgba(255,255,255,0.8)` | Widget surface |
| `glass-border` | `rgba(255,255,255,0.3)` | Glass edge border |

### Window Header

| Token | Value | Usage |
|---|---|---|
| `winhdr-bg` | `rgba(255,255,255,0.72)` | Window titlebar |
| `winhdr-border` | `rgba(27,32,80,0.1)` | Titlebar bottom edge |
| `winhdr-blur` | 30px | Glass blur |

## Shapes

The shape language is **Refined Rounded** — soft enough to feel modern, sharp enough to feel precise.

| Token | Value | Usage |
|---|---|---|
| `sm` | 2px | Inline badges, pills |
| `DEFAULT` | 4px | Inputs, buttons |
| `md` | 6px | Small cards |
| `lg` | 8px | Window border, default card |
| `xl` | 12px | Modal corners |
| `2xl` | 16px | Large cards |
| `3xl` | 22px | App icon tiles, dock items |
| `4xl` | 28px | Large app icons |
| `card` | 22px | Window card radius |
| `control` | 14px | Interactive control radius |
| `full` | 9999px | Circular/pill shapes |

## Components

### Buttons
- **Primary:** Background `#2E72C9`, text white. On hover: darken to `#1e5bb8`. On active: `#003590`.
- **Secondary:** Background `#f3f4f6`, text `#191c1e`. On hover: `#e5e7eb`. Border: `1px solid #e5e7eb`.
- **Danger:** Background `#ef4444`, text white. On hover: `#dc2626`.
- **Disabled:** Background `#9ca3af`, text white, pointer-events none.
- **Interactive State:** Scale 0.955 on press (subtle tactile feedback).

### Input Fields
- **Default:** Background `#f7f9fb`, 1px border `#e5e7eb`, height 48px.
- **Focus:** 2px border `#2E72C9`, `box-shadow: 0 0 0 3px rgba(46,114,201,0.1)`. Transition: 200ms ease.
- **Icons:** 20px icons in `#6b7280` (muted slate) within the input.

### Cards
- Always white background (`#ffffff`).
- 1px border of `#e5e7eb` for a cleaner, more "engineered" look.
- Standard internal padding: 24px.
- On hover: subtle border shift to `#d1d5db`.

### Information Banners
- **Info:** Background `#eff6ff`, border `#bfdbfe`, text `#1e40af`, icon `#2563eb`.
- **Success:** Background `#d1fae5`, border `#2E9C6B`, text `#1c5e40`.
- **Warning:** Background `#fef3c7`, border `#C98A2E`, text `#7a521b`.
- **Error:** Background `#fee2e2`, border `#C4453C`, text `#8f2f28`.

## Transitions & Animation

| Token | Value | Usage |
|---|---|---|
| `transition-fast` | 150ms | Press, color changes |
| `transition-base` | 200ms | Standard transitions |
| `transition-slow` | 300ms | Panel open/close |
| `transition-slower` | 500ms | Page transitions |
| `ease-default` | `cubic-bezier(0.4, 0, 0.2, 1)` | Standard easing |
| `ease-in` | `cubic-bezier(0.4, 0, 1, 1)` | Acceleration |
| `ease-out` | `cubic-bezier(0, 0, 0.2, 1)` | Deceleration |
| `ease-in-out` | `cubic-bezier(0.4, 0, 0.2, 1)` | Full cycle |
| `spring-easing` | `cubic-bezier(0.34, 1.56, 0.64, 1)` | Bouncy entrance |

### Desktop Chrome Animation

| Token | Value | Usage |
|---|---|---|
| `chrome-dur-fast` | 150ms | Press/color changes |
| `chrome-dur-med` | 260ms | Hover lift/scale |
| `chrome-dur-slow` | 420ms | One-shot entrance |
| `chrome-ease-spring` | `cubic-bezier(0.34, 1.56, 0.64, 1)` | Spring entrance |
| `chrome-ease-out` | `cubic-bezier(0.16, 1, 0.3, 1)` | Deceleration |
| `chrome-press` | 0.955 | `:active` scale |
| `chrome-idle-opacity` | 0.34 | Dock/topbar after inactivity |

## Desktop Shell

The desktop uses a **floating chrome** model — the dock sits at the bottom, the topbar is a floating pill at the top-right, and windows float freely over the wallpaper.

### Dock
- Item size: 56px
- Background: frosted glass (`--hv-shell-glass`)
- Running indicator: dot below active app icon
- Power button background: `#334155`

### Topbar Pill
- Height: 90px (72px on mobile)
- Background: frosted glass (`--hv-shell-glass`)
- Border: `--hv-shell-border`
- Inset from edge: 24px (12px on mobile)

### Desktop Wallpaper
```
linear-gradient(135deg, #1e3a8a 0%, #2E72C9 50%, #93c5fd 100%)
```

### Window Dimensions
| Token | Value | Usage |
|---|---|---|
| `window-default-width` | 1024px | Default window width |
| `window-default-height` | 720px | Default window height |
| `window-max-width` | 90vw | Maximum window width |
| `window-max-height` | 85vh | Maximum window height |
| `window-border-radius` | 8px | Window corner radius |

### Title Bar
| Token | Value | Usage |
|---|---|---|
| `title-bar-height` | 48px | Title bar height |
| `title-bar-bg` | `#f7f9fb` | Title bar background |
| `title-bar-border` | `1px solid #e5e7eb` | Title bar bottom edge |
| `title-bar-icon-size` | 20px | App icon in title bar |
| `title-bar-title-size` | 14px | Title text size |
| `title-bar-title-weight` | 600 | Title text weight |
| `title-bar-button-size` | 32px | Min/max/close button |
| `title-bar-button-radius` | 4px | Min/max/close radius |

### Sidebar
| Token | Value | Usage |
|---|---|---|
| `sidebar-width` | 256px | Expanded sidebar |
| `sidebar-collapsed-width` | 64px | Collapsed sidebar |

### Component Dimensions
| Token | Value | Usage |
|---|---|---|
| `input-height` | 48px | Input field height |
| `input-padding` | `0 16px` | Input horizontal padding |
| `input-border-width` | 1px | Input border width |
| `button-height` | 48px | Button height |
| `button-padding` | `0 24px` | Button horizontal padding |
| `button-icon-size` | 20px | Icon inside button |
| `modal-max-width` | 420px | Modal max width |
| `modal-header-padding` | `20px 24px` | Modal header padding |
| `modal-close-size` | 32px | Modal close button |
| `dock-item-size` | 56px | Dock icon tile |
| `dock-power-bg` | `#334155` | Dock power button bg |
| `launcher-header-height` | 80px | Launcher header |
| `launcher-app-icon-size` | 80px | Launcher app icon |
| `launcher-app-icon-svg-size` | 40px | Launcher icon SVG |

### App Icon Gradient
One shared accent across all apps:
```
linear-gradient(145deg, #3275C4, #1E62AE)
```
Every dock tile, window header icon, and Launchpad tile uses this gradient. No per-app accent colors.

## Z-Index Scale

| Token | Value | Usage |
|---|---|---|
| `z-base` | 0 | Base layer |
| `z-monitor` | 40 | Monitor widgets |
| `z-dock` | 50 | Dock |
| `z-monitor-drag` | 100 | Monitor drag overlay |
| `z-dropdown-menu` | 100 | Dropdown menus |
| `z-power-overlay` | 200 | Power overlay |
| `z-dropdown` | 1000 | Dropdowns |
| `z-sticky` | 1100 | Sticky elements |
| `z-fixed` | 1200 | Fixed positioning |
| `z-modal-backdrop` | 1300 | Modal overlay |
| `z-modal` | 1400 | Modal content |
| `z-popover` | 1500 | Popovers |
| `z-tooltip` | 1600 | Tooltips |
| `z-notification` | 1700 | Notifications |
| `z-topbar` | 1800 | Topbar pill |
| `z-launcher` | 1900 | App launcher |

## Responsive Breakpoints

| Token | Value | Usage |
|---|---|---|
| `sm` | 640px | Mobile landscape |
| `md` | 768px | Tablet |
| `lg` | 1024px | Small desktop |
| `xl` | 1280px | Desktop |
| `2xl` | 1536px | Large desktop |

### Touch Targets
Minimum touch target: **44px** (WCAG AA compliance).

## Interactive States

### Focus Rings
| Token | Value | Usage |
|---|---|---|
| `focus-ring` | `0 0 0 3px rgba(46,114,201,0.1)` | Default focus ring |
| `focus-ring-error` | `0 0 0 3px rgba(186,26,26,0.1)` | Error focus ring |
| `focus-ring-success` | `0 0 0 3px rgba(16,185,129,0.1)` | Success focus ring |
| `primary-selected-bg` | `rgba(46,114,201,0.15)` | Selected item bg |
| `primary-selected-bg-hover` | `rgba(46,114,201,0.2)` | Hovered selected bg |

### Overlay States
| Token | Value | Usage |
|---|---|---|
| `overlay-normal` | `rgba(0,0,0,0.05)` | Default overlay |
| `overlay-hover` | `rgba(0,0,0,0.08)` | Hovered overlay |
| `overlay-focus` | `rgba(0,0,0,0.08)` | Focused overlay |
| `overlay-active` | `rgba(0,0,0,0.12)` | Pressed/active overlay |
| `overlay-selected` | `rgba(0,0,0,0.06)` | Selected state overlay |
| `overlay-disabled` | `rgba(0,0,0,0.38)` | Disabled overlay |
| `state-hover` | `rgba(0,0,0,0.04)` | Light bg hover |
| `state-pressed` | `rgba(0,0,0,0.08)` | Light bg pressed |
| `state-dragged` | `rgba(0,0,0,0.16)` | Dragged state |

### Card Effects
| Token | Value | Usage |
|---|---|---|
| `card-hover-shadow` | `0 8px 24px rgba(25,28,30,0.08)` | Hovered card shadow |
| `card-active-shadow` | `0 4px 12px rgba(0,0,0,0.06)` | Pressed card shadow |
| `card-border` | `1px solid #e5e7eb` | Default card border |
| `card-hover-border` | `1px solid #d1d5db` | Hovered card border |

## Scrollbars

| Token | Value | Usage |
|---|---|---|
| `size` | 8px | Fine pointer (mouse) |
| `size-touch` | 16px | Coarse pointer (touch) |
| `thumb-min` | 48px | Minimum thumb height |
| `track` | transparent | Overlay look |
| `thumb` | `rgba(67,70,85,0.32)` | Idle thumb color |
| `thumb-hover` | `rgba(67,70,85,0.5)` | Hovered thumb |
| `thumb-active` | `rgba(46,114,201,0.55)` | Grabbed thumb (primary tint) |

## Loading & Skeleton States

- Skeleton background: `#e5e7eb`
- Skeleton shimmer: `linear-gradient(90deg, transparent, rgba(255,255,255,0.5), transparent)`
- Loading spinner color: `#2E72C9` (primary)

### Modal & Overlay
| Token | Value | Usage |
|---|---|---|
| `modal-overlay` | `rgba(0,0,0,0.5)` | Modal backdrop |
| `modal-shadow` | `0 20px 25px -5px rgba(0,0,0,0.1)` | Modal shadow |
| `modal-backdrop` | `rgba(0,0,0,0.6)` | Modal backdrop (darker) |
| `tooltip-bg` | `rgba(0,0,0,0.9)` | Tooltip background |
| `tooltip-text` | `#ffffff` | Tooltip text |
| `launcher-bg` | `rgba(0,0,0,0.75)` | Launcher overlay bg |
| `launcher-backdrop-blur` | `blur(40px)` | Launcher blur |

### Terminal Colors
| Token | Hex | Usage |
|---|---|---|
| `terminal-bg` | `#1e1e1e` | Terminal background |
| `terminal-text` | `#d4d4d4` | Terminal text |
| `terminal-prompt` | `#4ec9b0` | Prompt color |
| `terminal-path` | `#569cd6` | Path display |
| `terminal-command` | `#dcdcaa` | Command text |
| `terminal-output` | `#858585` | Output text |
| `terminal-error` | `#f48771` | Error output |
| `terminal-success` | `#4ec9b0` | Success output |
| `terminal-cursor` | `#d4d4d4` | Cursor color |

### Login Specific
| Token | Value | Usage |
|---|---|---|
| `login-error-bg` | `#fee2e2` | Login error bg |
| `login-error-text` | `#991b1b` | Login error text |
| `login-success-bg` | `#d1fae5` | Login success bg |
| `login-success-text` | `#065f46` | Login success text |
| `login-info-bg` | `#eff6ff` | Login info bg |
| `login-info-border` | `#bfdbfe` | Login info border |
| `login-info-text` | `#1e40af` | Login info text |
| `login-info-icon` | `#2563eb` | Login info icon |

## File Type Icons

| Type | Background | Stroke |
|---|---|---|
| Folder | `#e0f2fe` | `#2E72C9` |
| PDF | `#fef3c7` | `#f59e0b` |
| Excel | `#dcfce7` | `#10b981` |
| Image | `#fce7f3` | `#ec4899` |
| Video | `#dbeafe` | `#3b82f6` |
| Music | `#e9d5ff` | `#a855f7` |

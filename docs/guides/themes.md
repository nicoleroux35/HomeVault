# Themes

**Purpose:** what "theming" actually means in HomeVault today. This guide used
to describe a `ThemeToggle.svelte`, a theme registry and a dark/light switch —
**none of which exist in the tree**. It was rewritten on 2026-10-24 (ROADMAP
26.2) so it describes the shipped code.

## What a "theme" is here

HomeVault is a **light-only appliance with one accent colour**. There is no
dark theme, no `prefers-color-scheme` branch and no `[data-theme]` block
anywhere in `frontend/src`. The three things a user can actually change about
the look are:

| Element | Where it is stored | Who writes it | Who reads it |
|---|---|---|---|
| **Wallpaper** (one of the 44 bundled photos) | `appearance.wallpaper` in `data/settings.json` (+ a per-origin `localStorage` copy `hv-wallpaper`) | admin — `PUT /settings/appearance` | every account's boot hydration (`App.svelte`) |
| **Accent colour** (`--hv-primary`) | `appearance.accent_color` | admin — same endpoint (no editing UI; the shipped value is `#1E62AE`) | `App.svelte` sets `--hv-primary` on `<html>` at boot |
| **Nothing else** | — | — | — |

`theme` and `density` no longer exist: they were persisted no-ops (saved and
round-tripped, consumed by nothing) and were deleted from the model, the
handlers, the draft store and the API in ROADMAP 26.2. Old `settings.json`
files that still carry them keep loading — the reader ignores them.

## Backend

- `PUT /api/settings/appearance` — **admin only**, validated (bundled wallpaper,
  hex accent). A refusal is a `400` naming the field.
- `GET /api/settings/appearance` — **any signed-in session**, returns
  `{ wallpaper, accent_color }` and nothing else.
- Persistence and the exact validation/normalisation rules:
  [Settings → Appearance](./settings.md#appearance--one-appliance-look-roadmap-26-2026-10-24).
- Wallpaper assets and the picker: [Wallpapers](./wallpapers.md).

## Frontend

- **Design tokens (the real "theme"):**
  `frontend/src/lib/styles/theme/tokens.css` — the single source of truth for
  colour, spacing, radius, elevation, type. Components read `--hv-*`
  variables; they must not hardcode colours, so an accent change propagates
  everywhere. `base.css`, `components.css`, `utilities.css` and
  `app-layout.css` build on it.
- **Accent at runtime:** `--hv-primary` (and its ramp) are overridden on
  `document.documentElement` from the server value — `App.svelte`
  (`hydrateAppearanceFromServer`) at boot, `Settings.svelte` after a load.
- **Wallpaper at runtime:** `frontend/src/lib/stores/wallpaper.ts`
  (`WallpaperConfig` = `{type:'image'; value:'/wallpapers/hv-NN.webp'}` only;
  `normalizeWallpaper()` coerces anything else to a bundled photo). Painted by
  `Desktop.svelte`.
- **UI primitives:** `frontend/src/lib/components/theme/` (barrel:
  `index.ts`) — Button, Input, Select, Switch, Modal, Drawer, Toast, … There is
  **no theme component** in that folder: no toggle, no provider, no registry.
- `frontend/src/lib/stores/settingsDraft.svelte.ts` holds the staged
  `appearance: { wallpaper, accent_color }` inside the one Settings draft,
  flushed by the rail-footer Save button.

## Invariants

- One global look: an admin saves it once, every account sees it. A non-admin
  can read it and cannot write it — the Settings → Appearance grid is rendered
  **read-only** for them, with the line "Managed by the administrator"
  (`settings.wallpaper_managed_by_admin`), rather than a control that appears
  to save and silently does nothing.
- A failed hydration is **visible** (a toast, once per boot), never swallowed.
- Colour comes from tokens. A component that hardcodes a hex value breaks the
  accent override and is a bug.
- Any future palette (a dark theme, a second skin) must be contrast-checked
  before it ships: text ≥ 4.5:1 on its surface, focus ring / boundaries ≥ 3:1
  (WCAG 2.2 AA). `#1E62AE` is the only palette blue that passes in both
  directions (white on it 6.15:1; it as text on the light surfaces/tints
  4.99–5.68:1) — see the `DEFAULT_ACCENT` doc comment in
  `src/handlers/settings.rs`.
- The wallpaper selected state is `aria-checked` + the check badge, never a
  ring or an outset shadow (see [Wallpapers](./wallpapers.md)).

## Related

- [Settings](./settings.md), [Wallpapers](./wallpapers.md),
  [Design system](./design-system.md), [Desktop](./desktop.md)

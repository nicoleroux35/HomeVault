# Themes

**Purpose:** Visual theming (theme name, accent color, density, wallpaper) for the
desktop UI.

## Backend

- Appearance is stored via `PUT /settings/appearance` (see
  [Settings](./settings.md)) and persisted in `data/settings.json`.
- Wallpapers: see [Wallpapers](./wallpapers.md).

## Frontend

- Theme state: `frontend/src/lib/stores/settingsDraft.svelte.ts` (active theme, accent, density — staged in the draft, flushed by the rail-footer Save button).
- Design tokens: `frontend/src/lib/styles/theme/tokens.css` (CSS custom properties
  consumed throughout `theme/*.svelte` primitives).
- Theme registry/toggle: `frontend/src/lib/components/theme/ThemeToggle.svelte`,
  `frontend/src/lib/components/theme/index.ts` (barrel of UI primitives).
- Window/desktop CSS: `frontend/src/lib/styles/app-layout.css`
  (window chrome, sizing tokens).
- Design reference (LEGACY, historical only — current tokens live in
  `frontend/src/lib/styles/theme/tokens.css`): `docs/design/legacy-design-reference.md`.

## Invariants

- Theme is applied through CSS variables; components must read tokens rather than
  hardcode colors so dark/light/accent switching works.
- Appearance settings are user-facing but written by an admin-level
  `PUT /settings/appearance` (per the API auth model).

## Related

- [Settings](./settings.md), [Wallpapers](./wallpapers.md), [Desktop](./desktop.md)

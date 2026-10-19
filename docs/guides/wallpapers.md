# Wallpapers

**Purpose:** Pick the desktop wallpaper from the 44 images bundled into the
binary. **There is no upload feature** — it was removed 2026-09-04 (explicit
request: only a choice of pre-bundled wallpapers).

## Backend

There is no wallpaper API. The images are static assets:

- `frontend/public/wallpapers/hv-01.webp … hv-44.webp` (+ `hv-NN-thumb.webp`).
  `vite build` copies `frontend/public/*` into `static/dist/`, and
  `include_dir!` (`spa::STATIC_DIR`) bakes them into the binary.
- Served by `spa::serve_wallpapers_file` on `GET /wallpapers/{*path}` — this
  route **must stay registered before** the `serve_spa` fallback or images
  resolve to `index.html`.

Removed with the old feature: `src/handlers/wallpapers.rs`,
`wallpaper_models.rs`, the four `/api/wallpapers*` routes, the
`/uploads/wallpapers` mount, `config::paths::{wallpapers_dir,
wallpaper_thumbnails_dir}`, the `user_wallpapers` table (migration `030`), and
`WallpaperPicker.svelte`.

## Frontend

- `frontend/src/lib/stores/wallpaper.ts` — `WallpaperConfig` is
  `{type:'default'} | {type:'image'; value:string}` only; `normalizeWallpaper()`
  coerces any legacy value to `default`; `BUNDLED_WALLPAPERS` lists the 44. The
  store only accepts a `/wallpapers/`-prefixed URL.
- Picker: the **Appearance** section of `Settings.svelte` is a `settingsRowStacked`
  row inside the one appearance card (`Settings.svelte:940-961`) holding a
  3-column thumbnail grid, dropping to 2 columns once the card is narrower than
  380px. That is a **container query** (`@container` + `@max-[380px]:grid-cols-2`),
  not a viewport breakpoint — the grid follows the width the window actually
  gives it, which is the right thing in a resizable desktop window. 12px gap
  (`gap-3`) at every width.
- `.settings-wallpaper-thumb` is a **fixed box**: `block w-full h-auto
  aspect-[8/5]` + `object-cover`. The ratio is enforced in CSS, not inherited
  from the file, because the bundled thumbs are **not all the same shape** —
  `hv-09` and `hv-13` ship 640x480 while the other thirteen are 640x427
  (≈3:2). With the old intrinsic-height behaviour those two rendered taller
  than their neighbours and the grid rows came out ragged (2026-10-19).
  `object-cover` crops instead of distorting, `block` removes the inline
  baseline gap under the image, and `8/5` matches the `width="320"
  height="200"` attributes so there is no layout jump before the image
  decodes. `h-auto` stays: the `height="200"` attribute is a presentational
  hint (`height:200px`), and with *both* width and height definite
  `aspect-ratio` would be ignored. Any future asset of any shape lands in the
  same box — E2E 4.7 asserts all 44 tiles share one width and height.
  The thumb carries **no**
  `box-shadow` at all — an outset shadow bled past each tile and made the rows
  look overlapping (2026-09-09). Selection is shown by the `aria-checked` /
  `class:is-selected` state on the tile plus the `settings-wallpaper-check`
  badge, **not** by a ring. E2E 4.7 still fails on any non-`inset` shadow on a
  thumb, so keep it shadowless. The Density `<select>` and accent-colour
  `<input type=color>` were removed from the UI (the fields are still
  carried at their loaded values; `--hv-primary` still hydrates at boot).
  Since 2026-09-15 the wallpaper pick stages into the `appearance` draft and
  only the sidebar-footer Save button PUTs `/api/settings/appearance`.
- Wallpaper round-trips as `JSON.stringify($wallpaperStore)` through the generic
  settings save.
- Login / SetupWizard paint their own duplicated `defaultDesktopGradient` and do
  **not** read the wallpaper store — a photo wallpaper does not show on the login
  screen (out of scope for both screens).

Adding a wallpaper: drop `hv-NN.webp` + `hv-NN-thumb.webp` into
`frontend/public/wallpapers/` and bump the count in `BUNDLED_WALLPAPERS`.

## Related

- [Themes](./themes.md), [Desktop](./desktop.md), [Settings](./settings.md)

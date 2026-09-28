# HomeVault wallpapers

Bundled desktop wallpapers. Copied verbatim into `static/dist/wallpapers/`
by `vite build` and embedded into the release binary (`include_dir!` in
`src/handlers/spa.rs`), so the binary is the only artifact needed to ship them.

## Naming

- `hv-NN.webp` — full image, long edge capped at 2560 px, WebP q82.
- `hv-NN-thumb.webp` — 640 px preview used by the Settings → Appearance grid.

Numbering is contiguous from `hv-01`. The frontend list lives in
`frontend/src/lib/stores/wallpaper.ts` (`BUNDLED_WALLPAPERS`) — add the new
`hv-NN` pair there when adding an image.

## Rendering

The desktop paints the active image with `background-size: cover` +
`background-position: center`, so every image fills the viewport at any
aspect ratio without letterboxing.

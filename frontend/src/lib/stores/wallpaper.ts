/* =============================================================================
 * File          : wallpaper.ts
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 28/09/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Desktop wallpaper choice, limited to the bundled photos served from
 *   /wallpapers/ (custom uploads and gradients are gone). Every stored or
 *   received value — including legacy ones (bare colours, gradients, the
 *   old default) — is normalized to a valid bundled image, so stale values
 *   can never produce a broken desktop. Persisted in localStorage.
 *
 * Dependencies:
 *   - svelte (>= 5)
 *
 * Usage:
 *   wallpaperStore + BUNDLED_WALLPAPERS in Desktop.svelte and Settings →
 *   Appearance.
 * =============================================================================
 */

import { writable } from 'svelte/store';

/**
 * The desktop wallpaper: one of the bundled photo wallpapers served from
 * `/wallpapers/`. Custom uploads / arbitrary gradients are no longer
 * supported — the choice is limited to what ships inside the binary.
 *
 * `{ type: 'default' }` (the built-in faceted gradient) is legacy only —
 * explicit request to remove it as a selectable/fallback option. It's kept
 * in the type purely so `normalizeWallpaper()` still recognises old stored
 * values; nothing sets it any more, and `DEFAULT_WALLPAPER` below now
 * points at a bundled photo instead, so any such stray value is normalized
 * away on next read. Desktop.svelte's gradient/facet-layer rendering path
 * stays as an inert fallback rather than being ripped out.
 */
export type WallpaperConfig =
  | { type: 'default' }
  | { type: 'image'; value: string };

/** One bundled wallpaper: full image + grid thumbnail, both under /wallpapers/. */
export interface BundledWallpaper {
  id: string;
  /** Full-resolution image URL (used as the desktop background). */
  url: string;
  /** Small preview URL (used by the Settings → Appearance grid). */
  thumb: string;
}

/** The wallpapers embedded in the binary (see frontend/public/wallpapers/). */
export const BUNDLED_WALLPAPERS: BundledWallpaper[] = Array.from(
  { length: 15 },
  (_, i) => {
    const id = `hv-${String(i + 1).padStart(2, '0')}`;
    return { id, url: `/wallpapers/${id}.webp`, thumb: `/wallpapers/${id}-thumb.webp` };
  }
);

// The blue gradient is no longer offered — every fresh install (and every
// account migrating off an old `{type:'default'}` value, since
// normalizeWallpaper() below falls through to this for anything that isn't
// a recognised bundled image) now defaults to the first bundled photo.
const DEFAULT_WALLPAPER: WallpaperConfig = {
  type: 'image',
  value: BUNDLED_WALLPAPERS[0].url,
};

const STORAGE_KEY = 'hv-wallpaper';

/**
 * Coerce any stored/received value into a valid WallpaperConfig. Legacy
 * values (bare colour strings, gradient objects, custom data-URL images,
 * the old `{type:'default'}` gradient, or a now-removed preset) all fall
 * back to the default bundled photo.
 */
export function normalizeWallpaper(raw: unknown): WallpaperConfig {
  if (raw && typeof raw === 'object') {
    const r = raw as Record<string, unknown>;
    if (
      r.type === 'image' &&
      typeof r.value === 'string' &&
      r.value.startsWith('/wallpapers/')
    ) {
      return { type: 'image', value: r.value };
    }
  }
  return DEFAULT_WALLPAPER;
}

function readInitial(): WallpaperConfig {
  try {
    const saved = localStorage.getItem(STORAGE_KEY);
    return saved ? normalizeWallpaper(JSON.parse(saved)) : DEFAULT_WALLPAPER;
  } catch {
    return DEFAULT_WALLPAPER;
  }
}

function createWallpaperStore() {
  const { subscribe, set } = writable<WallpaperConfig>(readInitial());

  return {
    subscribe,

    /** Persist the wallpaper choice and notify subscribers. */
    setWallpaper(wallpaper: WallpaperConfig) {
      const next = normalizeWallpaper(wallpaper);
      try {
        localStorage.setItem(STORAGE_KEY, JSON.stringify(next));
      } catch {
        // Private-mode / storage-disabled: keep it in memory only.
      }
      set(next);
    },

    /** Accept a raw value (e.g. the server-persisted JSON string) and apply it. */
    applyRaw(raw: unknown) {
      this.setWallpaper(normalizeWallpaper(raw));
    },

    /** Back to the default bundled wallpaper (the old gradient is gone). */
    reset() {
      this.setWallpaper(DEFAULT_WALLPAPER);
    },
  };
}

export const wallpaperStore = createWallpaperStore();

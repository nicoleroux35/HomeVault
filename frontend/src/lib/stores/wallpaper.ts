/* =============================================================================
 * File          : wallpaper.ts
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 28/09/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Desktop wallpaper choice: the v2 built-in gradient (the default) or one
 *   of the bundled photos served from /wallpapers/. Custom uploads are gone;
 *   every stored or received value is normalized to one of those two shapes,
 *   so a stale value can never produce a broken desktop. Persisted in
 *   localStorage.
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
 * `{ type: 'default' }` is the v2 wallpaper — the brand gradient + halos +
 * dot grid declared in `tokens.css` (`--hv-desktop-bg`,
 * `--hv-desktop-blob-*`, `--hv-desktop-dots`). It is the shipped default and a
 * first-class tile in Settings → Appearance; a bundled photo remains a
 * per-appliance choice.
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
  { length: 44 },
  (_, i) => {
    const id = `hv-${String(i + 1).padStart(2, '0')}`;
    return { id, url: `/wallpapers/${id}.webp`, thumb: `/wallpapers/${id}-thumb.webp` };
  }
);

// v2 ships the built-in gradient again: it is the desktop language of the
// refresh, and a fresh install must look like the approved screen.
const DEFAULT_WALLPAPER: WallpaperConfig = { type: 'default' };

const STORAGE_KEY = 'hv-wallpaper';

/**
 * Coerce any stored/received value into a valid WallpaperConfig. Legacy
 * values (bare colour strings, gradient objects, custom data-URL images,
 * the old `{type:'default'}` gradient, or a now-removed preset) all fall
 * back to the default bundled photo.
 */
export function normalizeWallpaper(raw: unknown): WallpaperConfig {
  // The settings API persists the wallpaper as a bare path string
  // (`/wallpapers/hv-07.webp`) or an empty string for the built-in gradient;
  // older exports may carry the JSON-encoded object. Accept every shape.
  if (typeof raw === 'string') {
    const trimmed = raw.trim();
    if (!trimmed) return DEFAULT_WALLPAPER;
    if (trimmed.startsWith('/wallpapers/')) {
      return { type: 'image', value: trimmed };
    }
    try {
      return normalizeWallpaper(JSON.parse(trimmed));
    } catch {
      return DEFAULT_WALLPAPER;
    }
  }
  if (raw && typeof raw === 'object') {
    const r = raw as Record<string, unknown>;
    if (r.type === 'default') return DEFAULT_WALLPAPER;
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

/** Serialize the live choice for the settings API: `""` means the gradient. */
export function serializeWallpaper(wallpaper: WallpaperConfig): string {
  return wallpaper.type === 'image' ? JSON.stringify(wallpaper) : '';
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

    /** Back to the v2 built-in gradient. */
    reset() {
      this.setWallpaper(DEFAULT_WALLPAPER);
    },
  };
}

export const wallpaperStore = createWallpaperStore();

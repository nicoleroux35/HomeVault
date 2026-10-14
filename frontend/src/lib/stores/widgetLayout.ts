/* =============================================================================
 * File          : widgetLayout.ts
 * Author        : Nicolas Le Roux
 * Created       : 13/10/2026
 * Last modified : 13/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Per-account widget layout for the /edit-widgets page: which (up to 3)
 *   widgets are featured and the editor display order, namespaced per user
 *   in localStorage. Visibility itself stays owned by monitorStore. Stored
 *   orders are sanitized against the catalog, and widgets added in later
 *   releases are appended so they always show up.
 *
 * Dependencies:
 *   - svelte (>= 5), stores/widgetCatalog, stores/auth
 *
 * Usage:
 *   widgetLayoutStore in EditWidgets.svelte and the desktop widget column.
 * =============================================================================
 */

import { writable, get } from 'svelte/store';
import { authStore } from './auth';
import { WIDGET_DEFS } from './widgetCatalog';

/**
 * Per-account widget layout for the `/edit-widgets` page.
 *
 * Two things are edited there:
 *
 *   - `featured` — up to `MAX_FEATURED` (3) widget ids pinned to the top of
 *     the desktop's widget column. This is the "max 3" constraint.
 *   - `order` — the display order of every widget in the editor, so a drag
 *     reorder survives a reload.
 *
 * Visibility itself stays owned by `monitorStore` (the toggle lives there);
 * this store only records *which* active widgets are featured and in what
 * order they are listed. Persisted per-account in localStorage, namespaced
 * exactly like `monitorStore` and `shortcuts`, so two accounts on one browser
 * never share a layout.
 */

const STORAGE_KEY_PREFIX = 'hv-widget-layout:';

/** The maximum number of featured (pinned) widgets. */
export const MAX_FEATURED = 3;

export interface WidgetLayout {
  featured: string[];
  order: string[];
}

/** The catalog order, used as the default and to fill in missing ids. */
function catalogOrder(): string[] {
  return WIDGET_DEFS.map((d) => d.id);
}

function isKnown(id: string): boolean {
  return WIDGET_DEFS.some((d) => d.id === id);
}

function sanitize(raw: unknown): WidgetLayout {
  const fallback: WidgetLayout = { featured: [], order: catalogOrder() };
  if (!raw || typeof raw !== 'object') return fallback;
  const obj = raw as Partial<WidgetLayout>;
  const featured = Array.isArray(obj.featured)
    ? obj.featured.filter((id) => typeof id === 'string' && isKnown(id)).slice(0, MAX_FEATURED)
    : [];
  const known = Array.isArray(obj.order)
    ? obj.order.filter((id) => typeof id === 'string' && isKnown(id))
    : [];
  // Append any catalog entry the stored order predates, so a widget added in a
  // later release still shows up in the editor.
  const order = [...known, ...catalogOrder().filter((id) => !known.includes(id))];
  return { featured, order };
}

function storageKeyFor(username: string): string {
  return STORAGE_KEY_PREFIX + username;
}

function loadFromStorage(username: string): WidgetLayout {
  try {
    const raw = localStorage.getItem(storageKeyFor(username));
    if (!raw) return { featured: [], order: catalogOrder() };
    return sanitize(JSON.parse(raw));
  } catch {
    return { featured: [], order: catalogOrder() };
  }
}

function saveToStorage(username: string, layout: WidgetLayout) {
  try {
    localStorage.setItem(storageKeyFor(username), JSON.stringify(layout));
  } catch {
    /* storage unavailable — layout stays in memory for this session */
  }
}

function createWidgetLayoutStore() {
  const { subscribe, set, update } = writable<WidgetLayout>({
    featured: [],
    order: catalogOrder(),
  });

  let currentUsername: string | null = null;
  let hydrated = false;
  let current: WidgetLayout = { featured: [], order: catalogOrder() };

  subscribe((layout) => {
    current = layout;
    if (hydrated && currentUsername) saveToStorage(currentUsername, layout);
  });

  authStore.subscribe((state) => {
    const username = state.user?.username ?? null;
    if (username === currentUsername) return;
    hydrated = false;
    currentUsername = username;
    set(username ? loadFromStorage(username) : { featured: [], order: catalogOrder() });
    hydrated = true;
  });

  return {
    subscribe,
    /**
     * Toggle a widget's featured state. Returns `false` when the cap is
     * reached (the caller shows the "max 3" hint).
     */
    toggleFeatured(id: string): boolean {
      if (!isKnown(id)) return false;
      if (current.featured.includes(id)) {
        update((l) => ({ ...l, featured: l.featured.filter((f) => f !== id) }));
        return true;
      }
      if (current.featured.length >= MAX_FEATURED) return false;
      update((l) => ({ ...l, featured: [...l.featured, id] }));
      return true;
    },
    /** Replace the whole order (drag & drop reorder). */
    setOrder(order: string[]) {
      update((l) => ({ ...l, order: sanitize({ ...l, order }).order }));
    },
    /** Move `id` to sit immediately before `beforeId` (or to the end). */
    moveBefore(id: string, beforeId: string | null) {
      update((l) => {
        const order = l.order.filter((x) => x !== id);
        const at = beforeId ? order.indexOf(beforeId) : -1;
        if (at === -1) order.push(id);
        else order.splice(at, 0, id);
        return { ...l, order };
      });
    },
    /** The current layout, outside a reactive context. */
    all(): WidgetLayout {
      return get({ subscribe });
    },
    set,
  };
}

export const widgetLayoutStore = createWidgetLayoutStore();

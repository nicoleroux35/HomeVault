/* =============================================================================
 * File          : shortcuts.ts
 * Author        : Nicolas Le Roux
 * Created       : 13/10/2026
 * Last modified : 13/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Pinned web-shortcut tiles on the desktop: a small per-account,
 *   namespaced-in-localStorage list capped at 12, carrying the metadata
 *   fetched once server-side (SSRF-guarded) so tiles render without
 *   re-fetching. Entries are sanitized on load and deduplicated by URL.
 *
 * Dependencies:
 *   - svelte (>= 5)
 *
 * Usage:
 *   shortcutsStore in DesktopShortcuts.svelte.
 * =============================================================================
 */

import { writable, get } from 'svelte/store';
import { authStore } from './auth';

/**
 * Pinned web shortcuts — a small, per-account set of URLs shown as icons on
 * the desktop, next to the app dock.
 *
 * Each shortcut stores the metadata fetched once when it was added (title,
 * description, favicon URL, origin) so the desktop renders a real icon without
 * re-fetching on every mount. The metadata fetch itself is server-side
 * (`POST /api/shortcuts/fetch-metadata`) and SSRF-guarded — see
 * `src/handlers/shortcuts.rs`.
 *
 * Storage is per-account in localStorage (namespaced like `monitorStore`), so
 * two accounts sharing a browser never see each other's shortcuts. This is a
 * deliberate MVP: shortcuts are a desktop preference, and localStorage already
 * carries the widget layout for the same reason.
 */

const STORAGE_KEY_PREFIX = 'hv-shortcuts:';

export interface Shortcut {
  /** Stable id (also the DOM key). */
  id: string;
  /** The URL the shortcut opens. */
  url: string;
  /** Display title (page title, or the host when the page had none). */
  title: string;
  description: string;
  /** Absolute favicon URL, when the page declared one. */
  iconUrl: string | null;
  /** `scheme://host` of the page. */
  siteName: string;
}

/** The maximum number of pinned shortcuts — beyond this the desktop row wraps
 *  into the widgets and stops being scannable. */
export const MAX_SHORTCUTS = 12;

function storageKeyFor(username: string): string {
  return STORAGE_KEY_PREFIX + username;
}

function sanitize(list: unknown): Shortcut[] {
  if (!Array.isArray(list)) return [];
  const out: Shortcut[] = [];
  for (const raw of list) {
    if (!raw || typeof raw !== 'object') continue;
    const s = raw as Partial<Shortcut>;
    if (typeof s.url !== 'string' || !s.url) continue;
    out.push({
      id: typeof s.id === 'string' && s.id ? s.id : `sc_${out.length}`,
      url: s.url,
      title: typeof s.title === 'string' && s.title ? s.title : s.url,
      description: typeof s.description === 'string' ? s.description : '',
      iconUrl: typeof s.iconUrl === 'string' && s.iconUrl ? s.iconUrl : null,
      siteName: typeof s.siteName === 'string' ? s.siteName : '',
    });
  }
  return out.slice(0, MAX_SHORTCUTS);
}

function loadFromStorage(username: string): Shortcut[] {
  try {
    const raw = localStorage.getItem(storageKeyFor(username));
    if (!raw) return [];
    return sanitize(JSON.parse(raw));
  } catch {
    return [];
  }
}

function saveToStorage(username: string, list: Shortcut[]) {
  try {
    localStorage.setItem(storageKeyFor(username), JSON.stringify(list));
  } catch {
    /* storage unavailable — shortcuts stay in memory for this session */
  }
}

function createShortcutsStore() {
  const { subscribe, set, update } = writable<Shortcut[]>([]);

  let currentUsername: string | null = null;
  let hydrated = false;
  let current: Shortcut[] = [];

  subscribe((list) => {
    current = list;
    if (hydrated && currentUsername) saveToStorage(currentUsername, list);
  });

  authStore.subscribe((state) => {
    const username = state.user?.username ?? null;
    if (username === currentUsername) return;
    hydrated = false;
    currentUsername = username;
    set(username ? loadFromStorage(username) : []);
    hydrated = true;
  });

  return {
    subscribe,
    /** Add a shortcut (deduplicated by URL). Returns `false` at the cap. */
    add(shortcut: Omit<Shortcut, 'id'>): boolean {
      if (current.length >= MAX_SHORTCUTS) return false;
      if (current.some((s) => s.url === shortcut.url)) return false;
      const id = `sc_${Date.now().toString(36)}${Math.floor(Math.random() * 1e4)}`;
      update((list) => [...list, { ...shortcut, id }]);
      return true;
    },
    remove(id: string) {
      update((list) => list.filter((s) => s.id !== id));
    },
    /** Replace the whole list (used by import/restore). */
    set,
    /** The current list, outside a reactive context. */
    all(): Shortcut[] {
      return get({ subscribe });
    },
  };
}

export const shortcutsStore = createShortcutsStore();

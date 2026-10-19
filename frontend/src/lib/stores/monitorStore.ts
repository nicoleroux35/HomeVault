/* =============================================================================
 * File          : monitorStore.ts
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 19/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Per-account layout of the floating desktop monitor panels: positions,
 *   per-panel visibility, and the shared poll cadence, stored as one blob
 *   namespaced per user in localStorage (legacy unnamespaced key migrated
 *   once) and mirrored server-side so the layout follows the account across
 *   browser origins. Hand-edited or foreign refresh values are coerced back
 *   to a sane cadence so they can never starve or hammer the pollers.
 *
 * Dependencies:
 *   - svelte (>= 5)
 *
 * Usage:
 *   monitorStore in Desktop.svelte (placement/pollers), Settings → Widgets
 *   (visibility), widgetCatalog (visibility keys).
 * =============================================================================
 */

import { writable } from 'svelte/store'
import { authStore } from './auth'
import { api } from '$lib/services/api'

// Positions/visibility are desktop preferences for one specific HomeVault
// account, not a single machine-wide layout — without per-user namespacing,
// two accounts sharing the same browser would silently overwrite each
// other's widget layout. `LEGACY_STORAGE_KEY` is the old, unnamespaced key
// this replaces; kept only as a one-time migration source below.
const LEGACY_STORAGE_KEY = 'hv-monitor-positions'
const STORAGE_KEY_PREFIX = 'hv-monitor-positions:'

function storageKeyFor(username: string): string {
  return STORAGE_KEY_PREFIX + username
}

export interface MonitorPositions {
  resourceMonitor: { x: number; y: number }
  raidMonitor: { x: number; y: number }
  securityMonitor: { x: number; y: number }
  vpnMonitor: { x: number; y: number }
  downloaderMonitor: { x: number; y: number }
  syncthingMonitor: { x: number; y: number }
  calendarMonitor: { x: number; y: number }
  dockerMonitor: { x: number; y: number }
  weatherMonitor: { x: number; y: number }
  sessionsMonitor: { x: number; y: number }
  healthMonitor: { x: number; y: number }
  resourceMonitorVisible: boolean
  raidMonitorVisible: boolean
  securityMonitorVisible: boolean
  vpnMonitorVisible: boolean
  downloaderMonitorVisible: boolean
  syncthingMonitorVisible: boolean
  calendarMonitorVisible: boolean
  dockerMonitorVisible: boolean
  weatherMonitorVisible: boolean
  sessionsMonitorVisible: boolean
  healthMonitorVisible: boolean
  /** Poll cadence, in milliseconds, shared by every resource-backed monitor.
   *  Lives in the same layout blob as the positions so it follows the account
   *  across origins — see `setRefreshMs` and `Desktop.svelte`'s pollers. */
  refreshMs: number
}

/** The boolean `*Visible` fields of `MonitorPositions` — the key a widget
 *  catalog entry names to read its own visibility off the store. */
export type MonitorVisibilityKey = {
  [K in keyof MonitorPositions]: MonitorPositions[K] extends boolean ? K : never
}[keyof MonitorPositions]

/** The default poll cadence, matching the mock's "5 s" refresh option. */
export const DEFAULT_REFRESH_MS = 5000

/** The refresh cadences offered by Settings > Widgets, in milliseconds. */
export const REFRESH_INTERVAL_OPTIONS = [2000, 5000, 10000, 30000, 60000] as const

/** Coerce whatever a stored blob carries for `refreshMs` back to a usable
 *  cadence — a hand-edited localStorage entry or a foreign value must not be
 *  able to starve (or hammer) every poller on the desktop. */
function normalizeRefreshMs(value: unknown): number {
  return typeof value === 'number' && Number.isFinite(value) && value >= 1000
    ? value
    : DEFAULT_REFRESH_MS
}

function defaultPositions(): MonitorPositions {
  const rightEdge = Math.max(20, (typeof window !== 'undefined' ? window.innerWidth : 1920) - 300)
  // Positions are always supplied so a widget switched on lands where the
  // layout expects it; **visibility defaults to off for every monitor** — a
  // fresh install opens on a clean desktop, and the operator picks the
  // widgets they want from the Widgets app / Settings → Widgets. An existing
  // account is unaffected: `loadFromStorage` reads its own saved
  // `*Visible` flags first (`parsed.xVisible ?? d.xVisible`).
  //
  // The topbar is a floating pill anchored top-right
  // (top: 8px, height: 72px -> occupies y in [8, 80] since the 2026-10-18
  // height pass; it was 24/90 when this layout was drawn). Right-column
  // widgets sit under it, so the right column's first slot is y: 132 — a
  // 52px clearance below the pill's bottom edge, which is why this value did
  // not have to move with the pill.
  return {
    resourceMonitor: { x: rightEdge, y: 132 },
    raidMonitor: { x: 20, y: 84 },
    securityMonitor: { x: 20, y: 500 },
    vpnMonitor: { x: rightEdge, y: 470 },
    downloaderMonitor: { x: rightEdge, y: 640 },
    syncthingMonitor: { x: 340, y: 500 },
    calendarMonitor: { x: 340, y: 84 },
    dockerMonitor: { x: 660, y: 84 },
    weatherMonitor: { x: 660, y: 420 },
    sessionsMonitor: { x: 340, y: 660 },
    healthMonitor: { x: 660, y: 660 },
    resourceMonitorVisible: false,
    raidMonitorVisible: false,
    securityMonitorVisible: false,
    vpnMonitorVisible: false,
    downloaderMonitorVisible: false,
    syncthingMonitorVisible: false,
    calendarMonitorVisible: false,
    dockerMonitorVisible: false,
    weatherMonitorVisible: false,
    sessionsMonitorVisible: false,
    healthMonitorVisible: false,
    refreshMs: DEFAULT_REFRESH_MS,
  }
}

function loadFromStorage(username: string): MonitorPositions {
  try {
    // One-time migration: an account that already had a layout saved under
    // the old, unnamespaced key (from before per-user storage existed) gets
    // it copied forward instead of silently reset to defaults. Only the
    // first account to load after this change benefits — by design, since
    // the legacy key never held more than one browser-wide layout anyway.
    let saved = localStorage.getItem(storageKeyFor(username))
    if (!saved) {
      const legacy = localStorage.getItem(LEGACY_STORAGE_KEY)
      if (legacy) {
        saved = legacy
        localStorage.setItem(storageKeyFor(username), legacy)
        localStorage.removeItem(LEGACY_STORAGE_KEY)
      }
    }
    if (saved) {
      const parsed = JSON.parse(saved)
      const d = defaultPositions()
      return {
        resourceMonitor: parsed.resourceMonitor ?? d.resourceMonitor,
        raidMonitor: parsed.raidMonitor ?? d.raidMonitor,
        securityMonitor: parsed.securityMonitor ?? d.securityMonitor,
        vpnMonitor: parsed.vpnMonitor ?? d.vpnMonitor,
        downloaderMonitor: parsed.downloaderMonitor ?? d.downloaderMonitor,
        syncthingMonitor: parsed.syncthingMonitor ?? d.syncthingMonitor,
        calendarMonitor: parsed.calendarMonitor ?? d.calendarMonitor,
        dockerMonitor: parsed.dockerMonitor ?? d.dockerMonitor,
        weatherMonitor: parsed.weatherMonitor ?? d.weatherMonitor,
        sessionsMonitor: parsed.sessionsMonitor ?? d.sessionsMonitor,
        healthMonitor: parsed.healthMonitor ?? d.healthMonitor,
        resourceMonitorVisible: parsed.resourceMonitorVisible ?? d.resourceMonitorVisible,
        raidMonitorVisible: parsed.raidMonitorVisible ?? d.raidMonitorVisible,
        securityMonitorVisible: parsed.securityMonitorVisible ?? d.securityMonitorVisible,
        vpnMonitorVisible: parsed.vpnMonitorVisible ?? d.vpnMonitorVisible,
        downloaderMonitorVisible: parsed.downloaderMonitorVisible ?? d.downloaderMonitorVisible,
        syncthingMonitorVisible: parsed.syncthingMonitorVisible ?? d.syncthingMonitorVisible,
        calendarMonitorVisible: parsed.calendarMonitorVisible ?? d.calendarMonitorVisible,
        dockerMonitorVisible: parsed.dockerMonitorVisible ?? d.dockerMonitorVisible,
        weatherMonitorVisible: parsed.weatherMonitorVisible ?? d.weatherMonitorVisible,
        sessionsMonitorVisible: parsed.sessionsMonitorVisible ?? d.sessionsMonitorVisible,
        healthMonitorVisible: parsed.healthMonitorVisible ?? d.healthMonitorVisible,
        // Added after the first layouts shipped — a blob saved before this
        // key existed falls back to the default rather than reading undefined.
        refreshMs: normalizeRefreshMs(parsed.refreshMs),
      }
    }
  } catch {
  }
  return defaultPositions()
}

function saveToStorage(username: string, positions: MonitorPositions) {
  try {
    localStorage.setItem(storageKeyFor(username), JSON.stringify(positions))
  } catch {
  }
}

function createMonitorStore() {
  // Nothing is persisted yet until we know which account is logged in — the
  // authStore subscription below fires synchronously with the real username
  // (or null while logged out) as soon as it's known, before Desktop.svelte
  // ever mounts and reads a position, so this starting value is never
  // actually shown to a real, authenticated user.
  const { subscribe, set, update } = writable<MonitorPositions>(defaultPositions())

  let currentUsername: string | null = null
  let hydrated = false
  let currentPositions: MonitorPositions = defaultPositions()
  let pushTimer: ReturnType<typeof setTimeout> | undefined

  // localStorage is per-origin (host:port), so the same account accessed
  // from two different addresses (e.g. a LAN IP and a public IP forwarded to
  // the same server) would otherwise each keep their own independent layout.
  // Debounced (position updates fire continuously while dragging a widget)
  // so a drag doesn't hammer the API with one request per pointer-move.
  function scheduleServerPush(username: string, positions: MonitorPositions) {
    if (pushTimer) clearTimeout(pushTimer)
    pushTimer = setTimeout(() => {
      if (currentUsername !== username) return
      api.setMonitorLayout(JSON.stringify(positions)).catch(() => {
        // Non-fatal: this origin's localStorage still has the latest value.
      })
    }, 800)
  }

  async function syncFromServer(username: string) {
    try {
      const res = await api.getMonitorLayout()
      if (currentUsername !== username) return // account changed while awaiting
      if (res.success && res.data?.layout_json) {
        const parsed = JSON.parse(res.data.layout_json)
        const d = defaultPositions()
        set({ ...d, ...parsed, refreshMs: normalizeRefreshMs(parsed.refreshMs) })
      } else if (res.success) {
        // No layout saved server-side yet for this account - push whatever
        // this origin already has (fresh defaults or a pre-existing
        // localStorage layout) so other origins converge on it too.
        scheduleServerPush(username, currentPositions)
      }
    } catch {
      // Non-fatal: keep whatever localStorage/default already provided.
    }
  }

  subscribe((p) => {
    currentPositions = p
    if (hydrated && currentUsername) {
      saveToStorage(currentUsername, p)
      scheduleServerPush(currentUsername, p)
    }
  })

  // Reload the whole store whenever the logged-in account changes — covers
  // both a fresh page load (authStore.checkAuth()) and a live login/logout,
  // and keeps two different accounts on the same browser from sharing one
  // widget layout.
  authStore.subscribe((state) => {
    const username = state.user?.username ?? null
    if (username === currentUsername) return
    hydrated = false
    currentUsername = username
    set(username ? loadFromStorage(username) : defaultPositions())
    hydrated = true
    if (username) syncFromServer(username)
  })

  return {
    subscribe,
    updateResourcePosition(x: number, y: number) {
      update((p) => ({ ...p, resourceMonitor: { x, y } }))
    },
    updateRaidPosition(x: number, y: number) {
      update((p) => ({ ...p, raidMonitor: { x, y } }))
    },
    updateSecurityPosition(x: number, y: number) {
      update((p) => ({ ...p, securityMonitor: { x, y } }))
    },
    updateVpnPosition(x: number, y: number) {
      update((p) => ({ ...p, vpnMonitor: { x, y } }))
    },
    updateDownloaderPosition(x: number, y: number) {
      update((p) => ({ ...p, downloaderMonitor: { x, y } }))
    },
    updateSyncthingPosition(x: number, y: number) {
      update((p) => ({ ...p, syncthingMonitor: { x, y } }))
    },
    setResourceVisible(visible: boolean) {
      update((p) => ({ ...p, resourceMonitorVisible: visible }))
    },
    setRaidVisible(visible: boolean) {
      update((p) => ({ ...p, raidMonitorVisible: visible }))
    },
    setSecurityVisible(visible: boolean) {
      update((p) => ({ ...p, securityMonitorVisible: visible }))
    },
    setVpnVisible(visible: boolean) {
      update((p) => ({ ...p, vpnMonitorVisible: visible }))
    },
    setDownloaderVisible(visible: boolean) {
      update((p) => ({ ...p, downloaderMonitorVisible: visible }))
    },
    setSyncthingVisible(visible: boolean) {
      update((p) => ({ ...p, syncthingMonitorVisible: visible }))
    },
    updateCalendarPosition(x: number, y: number) {
      update((p) => ({ ...p, calendarMonitor: { x, y } }))
    },
    setCalendarVisible(visible: boolean) {
      update((p) => ({ ...p, calendarMonitorVisible: visible }))
    },
    updateDockerPosition(x: number, y: number) {
      update((p) => ({ ...p, dockerMonitor: { x, y } }))
    },
    setDockerVisible(visible: boolean) {
      update((p) => ({ ...p, dockerMonitorVisible: visible }))
    },
    updateWeatherPosition(x: number, y: number) {
      update((p) => ({ ...p, weatherMonitor: { x, y } }))
    },
    setWeatherVisible(visible: boolean) {
      update((p) => ({ ...p, weatherMonitorVisible: visible }))
    },
    updateSessionsPosition(x: number, y: number) {
      update((p) => ({ ...p, sessionsMonitor: { x, y } }))
    },
    setSessionsVisible(visible: boolean) {
      update((p) => ({ ...p, sessionsMonitorVisible: visible }))
    },
    updateHealthPosition(x: number, y: number) {
      update((p) => ({ ...p, healthMonitor: { x, y } }))
    },
    setHealthVisible(visible: boolean) {
      update((p) => ({ ...p, healthMonitorVisible: visible }))
    },
    setRefreshMs(ms: number) {
      update((p) => ({ ...p, refreshMs: ms }))
    },
    reset() {
      set(defaultPositions())
    },
  }
}

export const monitorStore = createMonitorStore()

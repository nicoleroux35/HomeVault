/* =============================================================================
 * File          : weatherLocation.ts
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 28/09/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   The city chosen for the Weather widget, resolved through the geocoding
 *   service and synced per account: localStorage is per-origin, so the
 *   value is mirrored to the server (debounced push, pull on login, and a
 *   push of the local value when the account has none yet) so every browser
 *   origin converges on the same city. Server failures are non-fatal.
 *
 * Dependencies:
 *   - svelte (>= 5)
 *
 * Usage:
 *   weatherLocationStore in WeatherWidget / Settings.
 * =============================================================================
 */

import { writable } from 'svelte/store';
import { authStore } from './auth';
import { api } from '$lib/services/api';

/** A location chosen for the desktop weather widget, resolved via Open-Meteo's geocoding API. */
export interface WeatherLocation {
  name: string;
  lat: number;
  lon: number;
}

const STORAGE_KEY = 'hv-weather-location';

function createWeatherLocationStore() {
  let initial: WeatherLocation | null = null;
  try {
    const saved = localStorage.getItem(STORAGE_KEY);
    if (saved) initial = JSON.parse(saved);
  } catch {
  }

  const { subscribe, set } = writable<WeatherLocation | null>(initial);

  // localStorage is per-origin (host:port), so the same account accessed
  // from two different addresses (e.g. a LAN IP and a public IP forwarded to
  // the same server) would otherwise each keep their own chosen city.
  // Mirrors monitorStore.ts's server-sync approach.
  let currentUsername: string | null = null
  let pushTimer: ReturnType<typeof setTimeout> | undefined

  function scheduleServerPush(username: string, location: WeatherLocation | null) {
    if (pushTimer) clearTimeout(pushTimer)
    pushTimer = setTimeout(() => {
      if (currentUsername !== username) return
      api.setWeatherLocation(JSON.stringify(location)).catch(() => {
        // Non-fatal: this origin's localStorage still has the latest value.
      })
    }, 500)
  }

  async function syncFromServer(username: string) {
    try {
      const res = await api.getWeatherLocation();
      if (currentUsername !== username) return // account changed while awaiting
      if (res.success && res.data?.location_json) {
        const parsed = JSON.parse(res.data.location_json);
        setAndPersist(parsed, false);
      } else if (res.success && initial) {
        // No location saved server-side yet for this account - push whatever
        // this origin already has locally so other origins converge on it.
        scheduleServerPush(username, initial);
      }
    } catch {
      // Non-fatal: keep whatever localStorage already provided.
    }
  }

  authStore.subscribe((state) => {
    const username = state.user?.username ?? null
    if (username === currentUsername) return
    currentUsername = username
    if (username) syncFromServer(username)
  })

  function setAndPersist(location: WeatherLocation | null, pushToServer: boolean) {
    set(location);
    try {
      if (location) localStorage.setItem(STORAGE_KEY, JSON.stringify(location));
      else localStorage.removeItem(STORAGE_KEY);
    } catch {
    }
    if (pushToServer && currentUsername) scheduleServerPush(currentUsername, location);
  }

  return {
    subscribe,
    set(location: WeatherLocation | null) {
      setAndPersist(location, true);
    },
  };
}

export const weatherLocationStore = createWeatherLocationStore();

/* =============================================================================
 * File          : weatherDisplayPrefs.ts
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 28/09/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Optional fields the user can toggle on the Weather widget (feels-like,
 *   wind gusts, precipitation, cloud cover, pressure), on top of the two
 *   always-shown ones. A pure display preference persisted in localStorage
 *   only — unlike the chosen city, it is not synced server-side. Defaults
 *   match the widget's original display so existing users see no change.
 *
 * Dependencies:
 *   - svelte (>= 5)
 *
 * Usage:
 *   weatherDisplayPrefsStore in WeatherWidget / Settings → Appearance.
 * =============================================================================
 */

import { writable } from 'svelte/store';

/**
 * Optional Open-Meteo `current=` fields the user can toggle on/off for the
 * desktop Weather widget, on top of the two always-shown ones (temperature,
 * condition). Local to this browser only (localStorage) — unlike
 * weatherLocationStore, this is a display preference, not chosen data, so
 * it doesn't need the same per-account server sync.
 */
export type WeatherField = 'feels_like' | 'wind_gusts' | 'precipitation' | 'cloud_cover' | 'pressure';

export const WEATHER_FIELDS: WeatherField[] = ['feels_like', 'wind_gusts', 'precipitation', 'cloud_cover', 'pressure'];

const STORAGE_KEY = 'hv-weather-display-fields';

// Matches the widget's original, pre-customization display: humidity and
// wind speed are always shown (not part of this optional set) and none of
// the new optional fields are enabled by default, so existing users see no
// change until they opt in from Settings.
const DEFAULT_FIELDS: WeatherField[] = [];

function createWeatherDisplayPrefsStore() {
  let initial: WeatherField[] = DEFAULT_FIELDS;
  try {
    const saved = localStorage.getItem(STORAGE_KEY);
    if (saved) {
      const parsed = JSON.parse(saved);
      if (Array.isArray(parsed)) {
        initial = parsed.filter((f): f is WeatherField => WEATHER_FIELDS.includes(f));
      }
    }
  } catch {
  }

  const { subscribe, set } = writable<WeatherField[]>(initial);

  function persist(fields: WeatherField[]) {
    set(fields);
    try {
      localStorage.setItem(STORAGE_KEY, JSON.stringify(fields));
    } catch {
    }
  }

  return {
    subscribe,
    toggle(field: WeatherField, enabled: boolean) {
      let current: WeatherField[] = [];
      subscribe((v) => (current = v))();
      const next = enabled
        ? current.includes(field) ? current : [...current, field]
        : current.filter((f) => f !== field);
      persist(next);
    },
    setFields(fields: WeatherField[]) {
      const next = fields.filter((f): f is WeatherField => WEATHER_FIELDS.includes(f));
      persist(next);
    },
  };
}

export const weatherDisplayPrefsStore = createWeatherDisplayPrefsStore();

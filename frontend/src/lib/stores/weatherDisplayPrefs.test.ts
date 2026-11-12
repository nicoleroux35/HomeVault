/**
 * weatherDisplayPrefs — the optional weather-widget fields (local only).
 *
 * The store reads localStorage at import time, so each case re-imports the
 * module after seeding storage; that is exactly the boot path a real browser
 * takes.
 */
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { get } from 'svelte/store';

type PrefsModule = typeof import('$lib/stores/weatherDisplayPrefs');

async function load(): Promise<PrefsModule> {
  vi.resetModules();
  return import('$lib/stores/weatherDisplayPrefs');
}

describe('weatherDisplayPrefsStore', () => {
  beforeEach(() => localStorage.clear());

  it('defaults to no optional fields', async () => {
    const { weatherDisplayPrefsStore } = await load();
    expect(get(weatherDisplayPrefsStore)).toEqual([]);
  });

  it('restores a persisted list and filters unknown values', async () => {
    localStorage.setItem('hv-weather-display-fields', JSON.stringify(['wind_gusts', 'bogus', 'pressure']));
    const { weatherDisplayPrefsStore } = await load();
    expect(get(weatherDisplayPrefsStore)).toEqual(['wind_gusts', 'pressure']);
  });

  it('toggles a field on and off and persists the result', async () => {
    const { weatherDisplayPrefsStore } = await load();

    weatherDisplayPrefsStore.toggle('feels_like', true);
    expect(get(weatherDisplayPrefsStore)).toEqual(['feels_like']);

    // Toggling the same field twice must not duplicate it.
    weatherDisplayPrefsStore.toggle('feels_like', true);
    expect(get(weatherDisplayPrefsStore)).toEqual(['feels_like']);

    weatherDisplayPrefsStore.toggle('feels_like', false);
    expect(get(weatherDisplayPrefsStore)).toEqual([]);
    expect(JSON.parse(localStorage.getItem('hv-weather-display-fields')!)).toEqual([]);
  });

  it('replaces the set through setFields and drops unknown fields', async () => {
    const { weatherDisplayPrefsStore } = await load();

    weatherDisplayPrefsStore.setFields(['cloud_cover', 'nope' as never, 'precipitation']);
    expect(get(weatherDisplayPrefsStore)).toEqual(['cloud_cover', 'precipitation']);
  });

  it('ignores corrupt stored JSON instead of throwing', async () => {
    localStorage.setItem('hv-weather-display-fields', '{not json');
    const { weatherDisplayPrefsStore } = await load();
    expect(get(weatherDisplayPrefsStore)).toEqual([]);
  });
});

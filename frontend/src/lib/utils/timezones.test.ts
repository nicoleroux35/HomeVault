/**
 * timezones — the shared IANA picker grouping used by Settings and the
 * scheduled-task form.
 */
import { describe, expect, it } from 'vitest';
import { groupTimezones, timezoneIsListed, SERVER_TIMEZONE } from '$lib/utils/timezones';

const NAMES = ['UTC', 'Africa/Cairo', 'America/New_York', 'America/Toronto', 'Europe/Paris', 'Europe/Berlin'];

describe('groupTimezones', () => {
  it('groups by the region before the slash and sorts the regions', () => {
    expect(groupTimezones(NAMES)).toEqual([
      { region: 'Africa', zones: ['Africa/Cairo'] },
      { region: 'America', zones: ['America/New_York', 'America/Toronto'] },
      { region: 'Europe', zones: ['Europe/Paris', 'Europe/Berlin'] },
    ]);
  });

  it('drops region-less entries such as UTC, which both pickers render separately', () => {
    expect(groupTimezones(NAMES).flatMap((g) => g.zones)).not.toContain('UTC');
  });

  it('returns no group for an empty list', () => {
    expect(groupTimezones([])).toEqual([]);
  });
});

describe('timezoneIsListed', () => {
  const groups = groupTimezones(NAMES);

  it('always accepts the server-timezone sentinel and UTC', () => {
    expect(timezoneIsListed(groups, SERVER_TIMEZONE)).toBe(true);
    expect(timezoneIsListed(groups, 'UTC')).toBe(true);
  });

  it('accepts a listed zone and refuses a legacy alias', () => {
    expect(timezoneIsListed(groups, 'Europe/Paris')).toBe(true);
    expect(timezoneIsListed(groups, 'CET')).toBe(false);
    expect(timezoneIsListed(groups, 'GB-Eire')).toBe(false);
  });
});

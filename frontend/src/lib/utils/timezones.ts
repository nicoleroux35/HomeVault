/* =============================================================================
 * File          : timezones.ts
 * Author        : Nicolas Le Roux
 * Created       : 23/10/2026
 * Last modified : 23/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   One IANA timezone list for every picker in the app. The names come from
 *   `GET /api/settings/timezones` (the zones the scheduler can actually
 *   evaluate) and are grouped by region here, so the Settings General card
 *   and the scheduled-task form render the same zones in the same order —
 *   they used to disagree, the task form being a free-text field with a
 *   `Europe/Paris` placeholder while Settings offered a shortlist.
 *
 * Dependencies:
 *   - none
 *
 * Usage:
 *   `const groups = groupTimezones(res.data.timezones)` then render one
 *   `<optgroup>` per `region`; `timezoneIsListed(groups, value)` decides
 *   whether the current value needs an extra option of its own.
 * =============================================================================
 */

/** One region's zones, in the server's (sorted) order. */
export interface TimezoneGroup {
  region: string;
  zones: string[];
}

/** The value that means "follow the appliance timezone" — the empty string,
 *  which the API reads as `NULL` on the task row and as the General setting
 *  for everything else. */
export const SERVER_TIMEZONE = '';

/**
 * Splits the flat, sorted name list into one group per region.
 *
 * `UTC` carries no region and is therefore not part of any group: both
 * pickers render it as their own first option so it is always one click
 * away, above ~500 regional entries.
 */
export function groupTimezones(names: string[]): TimezoneGroup[] {
  const groups = new Map<string, string[]>();
  for (const name of names) {
    const slash = name.indexOf('/');
    if (slash === -1) continue;
    const region = name.slice(0, slash);
    const zones = groups.get(region);
    if (zones) zones.push(name);
    else groups.set(region, [name]);
  }
  return [...groups.entries()]
    .sort(([a], [b]) => a.localeCompare(b))
    .map(([region, zones]) => ({ region, zones }));
}

/**
 * Whether the picker already offers `value`.
 *
 * A value it does not hold — a legacy alias (`CET`, `GB-Eire`) set through
 * the API, or any value while the list is still loading — must still be
 * rendered as its own option: a `<select>` whose value matches no `<option>`
 * silently displays the first one, and the next save would then persist that
 * different zone.
 */
export function timezoneIsListed(groups: TimezoneGroup[], value: string): boolean {
  if (!value) return true; // "server timezone" is always offered
  if (value === 'UTC') return true;
  return groups.some((g) => g.zones.includes(value));
}

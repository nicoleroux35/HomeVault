/**
 * appColor — the deterministic per-app colour used by the Dashboard's live
 * usage bands and chips.
 *
 * Stability is the contract: the same container name must produce the same
 * colour across polls and reloads, and the fixed saturation/lightness is what
 * keeps the chip above the 3:1 non-text contrast bar.
 */
import { describe, expect, it } from 'vitest';
import { appColor } from '$lib/utils/appColor';

describe('appColor', () => {
  it('is deterministic across calls', () => {
    expect(appColor('plex')).toBe(appColor('plex'));
    expect(appColor('home-assistant')).toBe(appColor('home-assistant'));
  });

  it('stays inside the documented HSL shape', () => {
    for (const name of ['a', 'plex', 'nextcloud', 'immich', 'paperless-ngx', 'ümlaut']) {
      expect(appColor(name)).toMatch(/^hsl\(\d{1,3} 62% 52%\)$/);
    }
  });

  it('gives different names different hues (no accidental collisions in the catalogue)', () => {
    const names = [
      'plex',
      'nextcloud',
      'immich',
      'home-assistant',
      'paperless-ngx',
      'syncthing',
      'baikal',
      'jellyfin',
    ];
    expect(new Set(names.map(appColor)).size).toBe(names.length);
  });

  it('handles the empty name without throwing', () => {
    expect(appColor('')).toMatch(/^hsl\(\d{1,3} 62% 52%\)$/);
  });
});

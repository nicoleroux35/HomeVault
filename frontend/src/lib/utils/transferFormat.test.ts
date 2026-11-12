/**
 * transferFormat — the presentation layer for transfer rows.
 *
 * The server sends raw numbers precisely so this module owns formatting; a
 * regression here silently turns "45.3 MB / 128.7 MB" into "45 MB" or an ETA
 * into "NaN min". The cases below pin the boundaries (unknown, zero, each
 * unit step) and the four supported languages.
 */
import { describe, expect, it } from 'vitest';
import {
  formatBytes,
  formatEta,
  formatSpeed,
  progressPercent,
  transferDetailLine,
} from '$lib/utils/transferFormat';

describe('formatBytes', () => {
  it('renders an explicit zero without a unit-less value', () => {
    expect(formatBytes(0)).toBe('0 B');
  });

  it('returns an empty string for unknown or non-finite input', () => {
    expect(formatBytes(null)).toBe('');
    expect(formatBytes(undefined)).toBe('');
    expect(formatBytes(Number.NaN)).toBe('');
    expect(formatBytes(Number.POSITIVE_INFINITY)).toBe('');
    expect(formatBytes(-1)).toBe('');
  });

  it('uses no decimal below 1 MB and one decimal from MB up', () => {
    expect(formatBytes(1)).toBe('1 B');
    expect(formatBytes(999)).toBe('999 B');
    expect(formatBytes(1024)).toBe('1 KB');
    expect(formatBytes(1024 * 1024 - 1)).toBe('1024 KB');
    expect(formatBytes(1024 * 1024)).toBe('1.0 MB');
    expect(formatBytes(45.3 * 1024 * 1024)).toBe('45.3 MB');
  });

  it('steps through GB and TB', () => {
    expect(formatBytes(1.4 * 1024 ** 3)).toBe('1.4 GB');
    expect(formatBytes(2 * 1024 ** 4)).toBe('2.0 TB');
  });
});

describe('formatSpeed', () => {
  it('appends /s and drops unknown or zero throughput', () => {
    expect(formatSpeed(12 * 1024 * 1024)).toBe('12.0 MB/s');
    expect(formatSpeed(640 * 1024)).toBe('640 KB/s');
    expect(formatSpeed(0)).toBe('');
    expect(formatSpeed(null)).toBe('');
    expect(formatSpeed(Number.NaN)).toBe('');
  });
});

describe('formatEta', () => {
  it('returns an empty string for unknown or negative values', () => {
    expect(formatEta(null)).toBe('');
    expect(formatEta(undefined)).toBe('');
    expect(formatEta(-1)).toBe('');
    expect(formatEta(Number.NaN)).toBe('');
  });

  it('coarsens by magnitude', () => {
    expect(formatEta(0)).toBe('< 1 s');
    expect(formatEta(8)).toBe('8 s');
    expect(formatEta(120)).toBe('2 min');
    expect(formatEta(3 * 3600 + 20 * 60)).toBe('3 h 20 min');
    expect(formatEta(3600)).toBe('1 h');
    expect(formatEta(2 * 86_400)).toBe('2 d');
  });

  it('localises the unit words and falls back to English', () => {
    expect(formatEta(120, 'fr')).toBe('2 min');
    expect(formatEta(3 * 3600, 'de')).toBe('3 Std');
    expect(formatEta(2 * 86_400, 'fr')).toBe('2 j');
    expect(formatEta(2 * 86_400, 'it')).toBe('2 d');
  });
});

describe('transferDetailLine', () => {
  it('joins the processed/total pair, speed and localised ETA', () => {
    expect(
      transferDetailLine(
        { bytes_processed: 45.3 * 1024 ** 2, bytes_total: 128.7 * 1024 ** 2, speed_bps: 12 * 1024 ** 2, eta_seconds: 120 },
        { etaLabel: 'left' },
      ),
    ).toBe('45.3 MB / 128.7 MB · 12.0 MB/s · 2 min left');
  });

  it('falls back to the server message when no counter exists', () => {
    expect(transferDetailLine({ method: 'rsync' }, { fallback: 'Preparing…' })).toBe('Preparing…');
    expect(transferDetailLine({}, {})).toBe('');
  });

  it('keeps a total without a processed count', () => {
    expect(transferDetailLine({ bytes_total: 1024 })).toBe('1 KB');
  });
});

describe('progressPercent', () => {
  it('clamps to 0..100', () => {
    expect(progressPercent(-4)).toBe(0);
    expect(progressPercent(42)).toBe(42);
    expect(progressPercent(140)).toBe(100);
  });

  it('returns null when no estimate exists', () => {
    expect(progressPercent(null)).toBeNull();
    expect(progressPercent(undefined)).toBeNull();
    expect(progressPercent(Number.NaN)).toBeNull();
  });
});

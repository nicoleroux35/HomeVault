/* =============================================================================
 * File          : transferFormat.ts
 * Author        : Nicolas Le Roux
 * Created       : 31/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Human formatting for transfer rows (HV-UX-02): byte counts, throughput,
 *   time remaining and the "45.3 MB / 128.7 MB · 12.0 MB/s · 2 min left"
 *   line the transfer tray and the Explorer's progress bar both render.
 *
 * Dependencies:
 *   - none
 *
 * Usage:
 *   `import { formatBytes, formatSpeed, formatEta, transferDetailLine } from
 *   '$lib/utils/transferFormat'`.
 * =============================================================================
 */

/**
 * HomeVault transfer formatting.
 *
 * The server reports raw numbers (`bytes_processed`, `bytes_total`,
 * `speed_bps`, `eta_seconds`) precisely so this file owns the presentation: a
 * byte count formatted on the server could not follow the user's locale, and a
 * client parsing "45.3 MB / 128.7 MB" would break the first time the server
 * reworded it.
 *
 * Every function is total (no throw) and returns `''`/`null` for "unknown", so
 * a caller can concatenate without guards.
 */

const KB = 1024;
const MB = KB * 1024;
const GB = MB * 1024;
const TB = GB * 1024;

/**
 * Compact byte size: `0 B`, `912 KB`, `45.3 MB`, `1.4 GB`, `2.0 TB`.
 *
 * One decimal from MB up (a 128.7 MB file must not read as "128 MB"), none
 * below (a byte figure with a decimal is noise). Negative or non-finite input
 * is treated as 0 rather than rendered as `NaN B`.
 */
export function formatBytes(bytes: number | null | undefined): string {
  if (bytes === null || bytes === undefined || !Number.isFinite(bytes) || bytes <= 0) {
    return bytes === 0 ? '0 B' : '';
  }
  if (bytes >= TB) return `${(bytes / TB).toFixed(1)} TB`;
  if (bytes >= GB) return `${(bytes / GB).toFixed(1)} GB`;
  if (bytes >= MB) return `${(bytes / MB).toFixed(1)} MB`;
  if (bytes >= KB) return `${Math.round(bytes / KB)} KB`;
  return `${Math.round(bytes)} B`;
}

/** Throughput: `12.0 MB/s`, `640 KB/s`; `''` when unknown or zero. */
export function formatSpeed(bytesPerSecond: number | null | undefined): string {
  if (
    bytesPerSecond === null ||
    bytesPerSecond === undefined ||
    !Number.isFinite(bytesPerSecond) ||
    bytesPerSecond <= 0
  ) {
    return '';
  }
  return `${formatBytes(bytesPerSecond)}/s`;
}

/**
 * Time remaining, coarse on purpose: `8 s`, `2 min`, `3 h 20 min`, `4 j`.
 *
 * A transfer row does not need second precision an hour out, and an ETA that
 * reads "1 h 59 min 58 s" looks more certain than it is. `language` selects
 * the unit words (`en`/`fr`/`de`/`es`).
 */
export function formatEta(
  seconds: number | null | undefined,
  language: string = 'en',
): string {
  if (
    seconds === null ||
    seconds === undefined ||
    !Number.isFinite(seconds) ||
    seconds < 0
  ) {
    return '';
  }
  const units = ETA_UNITS[language] ?? ETA_UNITS.en;
  const s = Math.round(seconds);
  if (s <= 0) return units.now;
  if (s < 60) return `${s} ${units.second}`;
  if (s < 3600) return `${Math.round(s / 60)} ${units.minute}`;
  if (s < 86_400) {
    const hours = Math.floor(s / 3600);
    const minutes = Math.round((s % 3600) / 60);
    return minutes > 0 ? `${hours} ${units.hour} ${minutes} ${units.minute}` : `${hours} ${units.hour}`;
  }
  return `${Math.round(s / 86_400)} ${units.day}`;
}

const ETA_UNITS: Record<
  string,
  { now: string; second: string; minute: string; hour: string; day: string }
> = {
  en: { now: '< 1 s', second: 's', minute: 'min', hour: 'h', day: 'd' },
  fr: { now: '< 1 s', second: 's', minute: 'min', hour: 'h', day: 'j' },
  de: { now: '< 1 s', second: 's', minute: 'Min', hour: 'Std', day: 'T' },
  es: { now: '< 1 s', second: 's', minute: 'min', hour: 'h', day: 'd' },
};

/** The parts of a transfer row this helper needs. */
export interface TransferDetailInput {
  bytes_processed?: number | null;
  bytes_total?: number | null;
  speed_bps?: number | null;
  eta_seconds?: number | null;
  method?: string | null;
}

/**
 * The detail line under a progress bar: `45.3 MB / 128.7 MB · 12.0 MB/s ·
 * 2 min left`.
 *
 * Omitted rather than blanked: a transfer with no byte counter renders the
 * server's own message (`fallback`), and one with a byte counter but no speed
 * yet renders just the pair. `etaLabel` is the localised word for "left"
 * (`2 min left` / `2 min restantes`); the caller passes the translated string
 * so this module stays free of i18n lookups.
 */
export function transferDetailLine(
  detail: TransferDetailInput,
  options: { language?: string; etaLabel?: string; fallback?: string | null } = {},
): string {
  const { language = 'en', etaLabel = '', fallback = null } = options;
  const processed = formatBytes(detail.bytes_processed);
  const total = formatBytes(detail.bytes_total);
  const parts: string[] = [];
  if (processed && total) {
    parts.push(`${processed} / ${total}`);
  } else if (total) {
    parts.push(total);
  }
  const speed = formatSpeed(detail.speed_bps);
  if (speed) parts.push(speed);
  const eta = formatEta(detail.eta_seconds, language);
  if (eta) parts.push(etaLabel ? `${eta} ${etaLabel}` : eta);
  if (parts.length === 0) return fallback ?? '';
  return parts.join(' · ');
}

/**
 * Percentage for a bar width, clamped to `0..100`.
 *
 * `null` when the producer has no estimate — the caller renders an
 * indeterminate bar instead of a 0 % one that never moves.
 */
export function progressPercent(progress: number | null | undefined): number | null {
  if (progress === null || progress === undefined || !Number.isFinite(progress)) return null;
  return Math.min(100, Math.max(0, progress));
}

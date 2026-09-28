import { writable } from 'svelte/store';

/** Date-display strategy: system-default, European (DD/MM), American (MM/DD), or Asian (YYYY/MM). */
export type DateFormat = 'auto' | 'european' | 'american' | 'asian';

/** Time-display strategy: system-default, 24-hour, or 12-hour with AM/PM. */
export type TimeFormat = 'auto' | '24h' | '12h';

export interface DateTimeConfig {
  dateFormat: DateFormat;
  timeFormat: TimeFormat;
}

const defaultConfig: DateTimeConfig = {
  dateFormat: 'auto',
  timeFormat: 'auto',
};

function createDateTimeStore() {
  // Restore saved preference or start with the system-following defaults
  const savedConfig = localStorage.getItem('hv-datetime-format');
  const initialConfig: DateTimeConfig = savedConfig
    ? JSON.parse(savedConfig)
    : defaultConfig;

  const { subscribe, set, update } = writable<DateTimeConfig>(initialConfig);

  return {
    subscribe,

    /** Persist the date format choice and update subscribers. */
    setDateFormat(dateFormat: DateFormat) {
      update(cfg => {
        const newConfig = { ...cfg, dateFormat };
        localStorage.setItem('hv-datetime-format', JSON.stringify(newConfig));
        return newConfig;
      });
    },

    /** Persist the time format choice and update subscribers. */
    setTimeFormat(timeFormat: TimeFormat) {
      update(cfg => {
        const newConfig = { ...cfg, timeFormat };
        localStorage.setItem('hv-datetime-format', JSON.stringify(newConfig));
        return newConfig;
      });
    },

    /** Replace the full config at once. */
    setConfig(config: DateTimeConfig) {
      localStorage.setItem('hv-datetime-format', JSON.stringify(config));
      set(config);
    },

    /** Revert to the system-default format. */
    reset() {
      localStorage.setItem('hv-datetime-format', JSON.stringify(defaultConfig));
      set(defaultConfig);
    },
  };
}

export const datetimeStore = createDateTimeStore();

/** Map a DateFormat to an Intl locale identifier. Returns `undefined` for `'auto'` so the system locale is used. */
export function getDateLocale(dateFormat: DateFormat): string | undefined {
  switch (dateFormat) {
    case 'european':
      return 'fr-FR';
    case 'american':
      return 'en-US';
    case 'asian':
      return 'ja-JP';
    case 'auto':
    default:
      return undefined;
  }
}

/** Map a TimeFormat to the `hour12` Intl option. Returns `undefined` for `'auto'` so the system default is used. */
export function getHour12Setting(timeFormat: TimeFormat): boolean | undefined {
  switch (timeFormat) {
    case '24h':
      return false;
    case '12h':
      return true;
    case 'auto':
    default:
      return undefined;
  }
}

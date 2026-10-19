/* =============================================================================
 * File          : settingsCatalog.ts
 * Author        : Nicolas Le Roux
 * Created       : 13/10/2026
 * Last modified : 19/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Single source of truth for the Settings screen's 8 rail groups, shared
 *   by the rail, the content header and the global search index — so a
 *   category can never exist in one and be missing (or stale) in the
 *   others. Adding a category means one entry here plus its i18n keys.
 *
 * Dependencies:
 *   - @tabler/icons-svelte
 *   - svelte (>= 5)
 *
 * Usage:
 *   routes/Settings.svelte renders from it; Desktop.svelte folds
 *   settingsSearchItems() into the search index.
 * =============================================================================
 */

import {
  IconActivity,
  IconBrain,
  IconInfoCircle,
  IconLayoutGrid,
  IconPalette,
  IconSettings,
  IconShieldCheckered
} from '@tabler/icons-svelte';
import { writable } from 'svelte/store';
import NotificationBellIcon from '$lib/components/shared/NotificationBellIcon.svelte';

/**
 * The Settings screen catalog — the single source of truth for every settings
 * category.
 *
 * Before this module existed, the rail's category list lived inline in
 * `routes/Settings.svelte` (an array of 8 entries) and the global ⌘K search
 * index built its own two hardcoded "Settings"-ish entries in
 * `routes/Desktop.svelte`. The two could drift: a category added to the rail
 * was invisible to search, and a search entry could point at a category that
 * no longer existed.
 *
 * Every consumer now reads this list:
 *   - `routes/Settings.svelte` renders the rail and the content header from it.
 *   - `routes/Desktop.svelte` folds `settingsSearchItems()` into the ⌘K index.
 *
 * Adding a category therefore means adding ONE entry here (plus its i18n keys)
 * and the `{#if activeCategory === …}` block that renders its cards. Nothing
 * else needs touching — see `docs/guides/settings.md`.
 */

/** One settings category, as rendered in the rail / content header / search. */
export interface SettingsCategory {
  /** Stable id, also the `activeCategory` value that gates the panel blocks. */
  key: string;
  /** Rail item label AND the content-header title. */
  labelKey: string;
  /** Rail item second line AND the content-header subtitle. */
  subtitleKey: string;
  /** Tabler glyph (or a Svelte component) for the rail tile / header tile. */
  icon: any;
  /** Category accent token driving the rail tile, header tile and row accents. */
  accent: string;
  /**
   * Extra free-text search terms (English, lowercase) folded into the ⌘K
   * index alongside the translated label + subtitle. Use this for synonyms a
   * user might type that are not in the label ("smtp", "tls", "ollama",
   * "uptime", …). Keep it short — the translated keys already carry the
   * primary terms.
   */
  keywords: string[];
}

/**
 * The 8 rail groups. Order is the rail order. `labelKey` reuses each group's
 * existing card-title translation; `subtitleKey` is shared by the rail item
 * and the content header so they can never disagree.
 *
 * Keys are declared literally (never built as `settings.nav_subtitle_${key}`)
 * so a grep for the full key name still finds it — see docs/I18N.md.
 */
export const SETTINGS_CATEGORIES: SettingsCategory[] = [
  {
    key: 'general',
    labelKey: 'settings.general_title',
    subtitleKey: 'settings.nav_subtitle_general',
    icon: IconSettings,
    accent: 'var(--hv-settings-cat-general)',
    keywords: ['general', 'language', 'timezone', 'time zone', 'server name', 'hostname', 'instance', 'locale']
  },
  {
    key: 'security',
    labelKey: 'settings.security',
    subtitleKey: 'settings.nav_subtitle_security',
    icon: IconShieldCheckered,
    accent: 'var(--hv-settings-cat-security)',
    keywords: ['security', 'https', 'tls', 'ssl', 'certificate', 'antivirus', 'clamav', 'ransomware', 'firewall', 'ufw']
  },
  {
    key: 'appearance',
    labelKey: 'settings.appearance_title',
    subtitleKey: 'settings.nav_subtitle_appearance',
    icon: IconPalette,
    accent: 'var(--hv-settings-cat-appearance)',
    keywords: ['appearance', 'theme', 'dark', 'light', 'wallpaper', 'accent', 'color', 'density', 'look']
  },
  {
    key: 'widgets',
    labelKey: 'settings.widgets_title',
    subtitleKey: 'settings.nav_subtitle_widgets',
    icon: IconLayoutGrid,
    accent: 'var(--hv-settings-cat-widgets)',
    keywords: ['widgets', 'monitors', 'desktop', 'refresh', 'poll', 'weather', 'layout']
  },
  {
    key: 'notifications',
    labelKey: 'settings.notifications_title',
    subtitleKey: 'settings.nav_subtitle_notifications',
    icon: NotificationBellIcon,
    accent: 'var(--hv-settings-cat-notifications)',
    keywords: ['notifications', 'alerts', 'email', 'smtp', 'webhook', 'mail']
  },
  {
    key: 'ai',
    labelKey: 'settings.ai_title',
    subtitleKey: 'settings.nav_subtitle_ai',
    icon: IconBrain,
    accent: 'var(--hv-settings-cat-ai)',
    keywords: ['ai', 'assistant', 'ollama', 'model', 'llm', 'chat', 'mcp', 'agent', 'external agents', 'token']
  },
  {
    key: 'monitoring',
    labelKey: 'settings.monitoring_title',
    subtitleKey: 'settings.nav_subtitle_monitoring',
    icon: IconActivity,
    accent: 'var(--hv-settings-cat-monitoring)',
    keywords: ['monitoring', 'ups', 'snmp', 'power', 'battery', 'metrics']
  },
  {
    key: 'system',
    labelKey: 'settings.system_title',
    subtitleKey: 'settings.nav_subtitle_system',
    icon: IconInfoCircle,
    accent: 'var(--hv-settings-cat-system)',
    keywords: ['system', 'update', 'version', 'software', 'information', 'data', 'export', 'import', 'reset', 'privacy']
  }
];

/**
 * A one-shot request to open Settings on a given category, set by the global
 * search index (and any future deep-link) and consumed by `Settings.svelte`
 * on mount. `null` means "no preference" — the screen keeps its default
 * category. Reset to `null` by the consumer once applied so reopening
 * Settings from the dock does not re-jump to a stale category.
 */
export const pendingSettingsCategory = writable<string | null>(null);

/** Ask the Settings screen to activate `key` the next time it mounts. */
export function requestSettingsCategory(key: string): void {
  pendingSettingsCategory.set(key);
}

/** Look up a category by its stable key (never returns undefined for a valid key). */
export function findSettingsCategory(key: string): SettingsCategory | undefined {
  return SETTINGS_CATEGORIES.find((c) => c.key === key);
}

/** A search hit produced from the catalog, before it is turned into a
 *  `SearchableItem` (kept dependency-free so this module stays a pure catalog
 *  and the search store owns the item shape). */
export interface SettingsSearchHit {
  key: string;
  labelKey: string;
  subtitleKey: string;
  icon: any;
  accent: string;
  keywords: string[];
}

/**
 * The catalog projected for the global search index. `translate` is injected
 * (rather than importing the i18n store here) so this stays a pure module
 * that unit tests can exercise without a Svelte rune context.
 */
export function settingsSearchItems(
  translate: (key: string) => string
): (SettingsSearchHit & { title: string; description: string; searchKeywords: string[] })[] {
  return SETTINGS_CATEGORIES.map((cat) => {
    const title = translate(cat.labelKey);
    const description = translate(cat.subtitleKey);
    return {
      key: cat.key,
      labelKey: cat.labelKey,
      subtitleKey: cat.subtitleKey,
      icon: cat.icon,
      accent: cat.accent,
      keywords: cat.keywords,
      title,
      description,
      // Translated label + subtitle words plus the English synonyms, all
      // lowercased so the search store's `includes` matching is consistent.
      searchKeywords: [
        title.toLowerCase(),
        description.toLowerCase(),
        cat.key,
        ...cat.keywords
      ]
    };
  });
}

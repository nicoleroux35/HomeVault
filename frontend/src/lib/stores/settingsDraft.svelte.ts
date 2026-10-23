/* =============================================================================
 * File          : settingsDraft.svelte.ts
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 28/09/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   The Settings screen's single draft of every editable value, persisted
 *   ONLY when the user clicks Save (no auto-save anywhere). Edits survive
 *   category switches because the draft lives here while sub-cards unmount;
 *   dirty detection compares a JSON snapshot per group against the last
 *   state known to be on the server, and saveAll() advances a baseline only
 *   on success. Svelte 5 runes module.
 *
 * Dependencies:
 *   - svelte (>= 5, runes)
 *
 * Usage:
 *   Bound by routes/Settings.svelte and the embedded TLS/Antivirus/
 *   Ransomware/SNMP sub-cards.
 * =============================================================================
 */

import { api } from '$lib/services/api';
import { languageStore } from './i18n';
import { get } from 'svelte/store';
import { monitorStore, type MonitorPositions } from './monitorStore';
import { WIDGET_DEFS } from './widgetCatalog';
import { weatherLocationStore, type WeatherLocation } from './weatherLocation';
import { weatherDisplayPrefsStore, type WeatherField } from './weatherDisplayPrefs';

/**
 * The Settings screen's single draft of every editable value, persisted ONLY
 * when the user clicks "Save" in the sidebar footer (no auto-save anywhere).
 *
 * Every category on the screen binds its fields to `draft` — the main groups
 * in `routes/Settings.svelte` and the embedded sub-cards (TLS, Antivirus,
 * Anti-Ransomware, SNMP) — so edits survive switching categories (a sub-card
 * unmounts when its group leaves the panel, but its draft lives here). Dirty
 * detection is a JSON snapshot per group compared against `baselines[group]`,
 * the last state known to be on the server. `saveAll()` flushes every dirty
 * group through its own saver and advances the baseline only on success.
 *
 * This module uses runes ($state / $derived). Consuming it from a legacy
 * component is fine — it is the runes *declaration* that flips a whole file,
 * and this is a standalone .svelte.ts module, not a legacy component.
 */

export type SettingsGroup =
  | 'general'
  | 'email'
  | 'notifications'
  | 'update'
  | 'appearance'
  | 'autostart'
  | 'ai'
  | 'widgets'
  | 'weather'
  | 'tls'
  | 'antivirus'
  | 'ransomware'
  | 'snmp';

const ALL_GROUPS: SettingsGroup[] = [
  'general',
  'email',
  'notifications',
  'update',
  'appearance',
  'autostart',
  'ai',
  'widgets',
  'weather',
  'tls',
  'antivirus',
  'ransomware',
  'snmp',
];

export interface TlsDraft {
  enabled: boolean;
  https_port: number | null;
  http_port: number;
  mode: 'self-signed' | 'manual';
  cert_path: string;
  key_path: string;
  redirect: boolean;
}

export interface WidgetsDraft {
  /** Visibility keyed by the `MonitorPositions` boolean field name. */
  visible: Record<string, boolean>;
  refreshMs: number;
}

export interface WeatherDraft {
  location: WeatherLocation | null;
  displayFields: WeatherField[];
}

export interface DraftValues {
  general: { server_name: string; timezone: string; language: string };
  email: {
    smtp_server: string;
    smtp_port: number;
    smtp_username: string;
    smtp_password: string;
    from_address: string;
    use_tls: boolean;
  };
  notifications: {
    email: string;
    enable_alerts: boolean;
    alert_threshold: number;
    smtp_enabled: boolean;
    webhook_url: string;
    webhook_enabled: boolean;
    webhook_format: string;
  };
  update: { check_frequency: string };
  appearance: { theme: string; wallpaper: string; accent_color: string; density: string };
  autostart: { enabled: boolean };
  ai: { model: string };
  widgets: WidgetsDraft;
  weather: WeatherDraft;
  tls: TlsDraft;
  antivirus: {
    quarantine_dir: string;
    scheduled_scan_enabled: boolean;
    scheduled_scan_cron: string;
    scheduled_scan_path: string;
    scan_archives: boolean;
    last_scan_at: string | null;
  };
  ransomware: {
    enabled: boolean;
    honeypots_enabled: boolean;
    rate_threshold_events: number;
    rate_threshold_seconds: number;
    auto_response: 'alert_only' | 'lock_account' | 'disable_share';
  };
  snmp: { community: string; version: string };
}

export const draft = $state<DraftValues>({
  general: { server_name: 'HomeVault', timezone: 'UTC', language: 'en' },
  email: {
    smtp_server: '',
    smtp_port: 587,
    smtp_username: '',
    smtp_password: '',
    from_address: '',
    use_tls: true,
  },
  notifications: {
    email: '',
    enable_alerts: true,
    alert_threshold: 80,
    smtp_enabled: false,
    webhook_url: '',
    webhook_enabled: false,
    webhook_format: 'generic',
  },
  update: { check_frequency: 'weekly' },
  appearance: { theme: 'auto', wallpaper: '', accent_color: '#1E62AE', density: 'comfortable' },
  autostart: { enabled: false },
  ai: { model: '' },
  widgets: { visible: {}, refreshMs: 5000 },
  weather: { location: null, displayFields: [] },
  tls: { enabled: false, https_port: 8443, http_port: 8080, mode: 'self-signed', cert_path: '', key_path: '', redirect: false },
  antivirus: {
    quarantine_dir: '/var/lib/homevault/quarantine',
    scheduled_scan_enabled: false,
    scheduled_scan_cron: '0 3 * * 0',
    scheduled_scan_path: '',
    scan_archives: true,
    last_scan_at: null,
  },
  ransomware: {
    enabled: false,
    honeypots_enabled: true,
    rate_threshold_events: 50,
    rate_threshold_seconds: 10,
    auto_response: 'alert_only',
  },
  snmp: { community: 'public', version: '2c' },
});

/** Last-persisted state per group (JSON snapshot), the dirty baseline. */
const baselines = $state<Record<string, string>>({});

/** Groups whose server values have already been read into the draft. */
const loadedGroups = $state<Record<string, boolean>>({});

/** The last server language, so a general save only re-requests it on change. */
let savedLanguage = $state('en');

export function setSavedLanguage(lang: string) {
  savedLanguage = lang;
}

/** Mark an object group as loaded and baseline it against its current draft. */
export function seedGroup(group: SettingsGroup) {
  loadedGroups[group] = true;
  baselines[group] = snap(group);
}

/** Seed a scalar group (autostart / ai) from the server and baseline it. */
export function seedAutostart(enabled: boolean) {
  draft.autostart = { enabled };
  seedGroup('autostart');
}

export function seedAiModel(model: string) {
  draft.ai = { model };
  seedGroup('ai');
}

/** Notice shown after a server-name save (hostname restart recommendation). */
export const notices = $state<{ hostname: string | null; timezone: string | null }>({
  hostname: null,
  timezone: null,
});

export function clearHostnameNotice() {
  notices.hostname = null;
}

export function clearTimezoneNotice() {
  notices.timezone = null;
}

/** TLS save result surfaced to the TlsCard (restart banner + cert info). */
export const tls = $state<{ saveResult: { requires_restart: boolean; cert_info: any } | null }>({
  saveResult: null,
});

export function setTlsSaveResult(result: { requires_restart: boolean; cert_info: any } | null) {
  tls.saveResult = result;
}

let _saving = $state(false);
let _error = $state(false);

/** Post-save callbacks per group (e.g. SNMP re-reads its live data). */
const onSavedCallbacks = $state<Record<string, (() => void)[]>>({});

export function onGroupSaved(group: SettingsGroup, cb: () => void) {
  (onSavedCallbacks[group] ||= []).push(cb);
}

/** The group's full draft JSON — the dirty-comparison snapshot. */
function snap(group: SettingsGroup): string {
  return JSON.stringify(draft[group]);
}

function isDirty(group: SettingsGroup): boolean {
  return baselines[group] !== undefined && snap(group) !== baselines[group];
}

const dirtyGroups = $derived.by(() => {
  const out: SettingsGroup[] = [];
  for (const g of ALL_GROUPS) {
    if (isDirty(g)) out.push(g);
  }
  return out;
});

const dirtyCount = $derived(dirtyGroups.length);

/**
 * The dirty count / save status. These are getters, not exported derived state
 * (Svelte forbids exporting `$derived` from a module). Read them from a runes
 * component (`$derived(getSaveStatus())`) where reads inside the call are
 * tracked — see `SettingsSaveButton.svelte`, whose colour + label state
 * machine is built from both.
 */
export function getDirtyCount(): number {
  return dirtyCount;
}

export function getSaveStatus(): 'idle' | 'saving' | 'saved' | 'error' {
  return _saving ? 'saving' : _error ? 'error' : dirtyCount > 0 ? 'idle' : 'saved';
}

/**
 * Record the server values of a group into the draft and baseline it, once.
 * Later mounts of the same category skip the refetch so unsaved edits survive.
 */
export function registerGroup(group: SettingsGroup, values: any) {
  if (loadedGroups[group]) return;
  loadedGroups[group] = true;
  Object.assign(draft[group], values);
  baselines[group] = snap(group);
}

/** Forget the server-loaded state of groups so the next mount refetches them. */
export function invalidateGroups(...groups: SettingsGroup[]) {
  for (const g of groups) {
    delete loadedGroups[g];
    delete baselines[g];
  }
}

/**
 * Copy the live monitor layout into the widgets draft — but only while the
 * user has NOT edited this group (otherwise a late server sync would clobber
 * staged changes). Called on load and on monitorStore changes.
 */
export function syncWidgetsFromMonitor(positions: MonitorPositions) {
  if (isDirty('widgets')) return;
  const visible: Record<string, boolean> = {};
  for (const def of WIDGET_DEFS) visible[def.visibleKey] = positions[def.visibleKey];
  draft.widgets.visible = visible;
  draft.widgets.refreshMs = positions.refreshMs;
  baselines['widgets'] = snap('widgets');
}

/**
 * Same protection for the weather draft (location + display fields), which
 * also syncs from a live store.
 */
export function syncWeatherFromStores(location: WeatherLocation | null, fields: WeatherField[]) {
  if (isDirty('weather')) return;
  draft.weather.location = location;
  draft.weather.displayFields = fields;
  baselines['weather'] = snap('weather');
}

async function saveGroup(group: SettingsGroup): Promise<void> {
  switch (group) {
    case 'general': {
      const res = await api.updateGeneralSettings({
        server_name: draft.general.server_name,
        timezone: draft.general.timezone,
      });
      if (!res.success) throw new Error(group);
      notices.hostname = res.data?.restart_required ? res.data.hostname_notice : null;
      // ROADMAP 13.4: the timezone is now applied to the host. A refusal
      // (no sudo, unknown zone on the host) is reported instead of looking
      // like a save that worked.
      notices.timezone = res.data?.timezone_notice ?? null;
      if (draft.general.language !== savedLanguage) {
        const lr = await api.setLanguage(draft.general.language);
        if (lr.success) {
          languageStore.setLanguage(draft.general.language as any);
          savedLanguage = draft.general.language;
        }
      }
      break;
    }
    case 'email': {
      const data: Record<string, any> = {
        smtp_server: draft.email.smtp_server,
        smtp_port: draft.email.smtp_port,
        smtp_username: draft.email.smtp_username,
        from_address: draft.email.from_address,
        use_tls: draft.email.use_tls,
      };
      if (draft.email.smtp_password) data.smtp_password = draft.email.smtp_password;
      const res = await api.updateEmailSettings(data);
      if (!res.success) throw new Error(group);
      break;
    }
    case 'notifications': {
      const res = await api.updateNotificationSettings({
        email: draft.notifications.email,
        enable_alerts: draft.notifications.enable_alerts,
        alert_threshold: draft.notifications.alert_threshold,
        smtp_enabled: draft.notifications.smtp_enabled,
        webhook_url: draft.notifications.webhook_url,
        webhook_enabled: draft.notifications.webhook_enabled,
        webhook_format: draft.notifications.webhook_format,
      });
      if (!res.success) throw new Error(group);
      break;
    }
    case 'update': {
      const res = await api.updateUpdateSettings({ check_frequency: draft.update.check_frequency });
      if (!res.success) throw new Error(group);
      break;
    }
    case 'appearance': {
      const res = await api.updateAppearanceSettings({
        theme: draft.appearance.theme,
        wallpaper: draft.appearance.wallpaper,
        accent_color: draft.appearance.accent_color,
        density: draft.appearance.density,
      });
      if (!res.success) throw new Error(group);
      break;
    }
    case 'autostart': {
      const res = await api.setAutostart(draft.autostart.enabled);
      if (!res.success) throw new Error(group);
      break;
    }
    case 'ai': {
      if (!draft.ai.model) break;
      const res = await api.chatSetModel(draft.ai.model);
      if (!res.success) throw new Error(group);
      break;
    }
    case 'widgets': {
      // Apply the staged layout through monitorStore — its own persistence
      // path (localStorage + POST /monitor-layout) is the save.
      const current = get(monitorStore);
      for (const def of WIDGET_DEFS) {
        const visible = draft.widgets.visible[def.visibleKey];
        if (visible !== undefined && visible !== current[def.visibleKey]) {
          def.setVisible(visible);
        }
      }
      monitorStore.setRefreshMs(draft.widgets.refreshMs);
      break;
    }
    case 'weather': {
      if (draft.weather.location) weatherLocationStore.set(draft.weather.location);
      weatherDisplayPrefsStore.setFields(draft.weather.displayFields);
      break;
    }
    case 'tls': {
      const res = await api.setTlsSettings({
        enabled: draft.tls.enabled,
        https_port: draft.tls.https_port || null,
        mode: draft.tls.mode,
        cert_path: draft.tls.mode === 'manual' ? draft.tls.cert_path || null : null,
        key_path: draft.tls.mode === 'manual' ? draft.tls.key_path || null : null,
        redirect: draft.tls.redirect,
      });
      if (!res.success) throw new Error(group);
      const d = (res.data as any)?.settings;
      setTlsSaveResult({
        requires_restart: !!((res.data as any)?.requires_restart ?? d?.requires_restart),
        cert_info: d?.cert_info ?? null,
      });
      break;
    }
    case 'antivirus': {
      const res = await api.updateAntivirusSettings(draft.antivirus);
      if (!res.success) throw new Error(group);
      break;
    }
    case 'ransomware': {
      const res = await api.updateRansomwareSettings(draft.ransomware);
      if (!res.success) throw new Error(group);
      break;
    }
    case 'snmp': {
      const res = await api.updateSnmpSettings({ community: draft.snmp.community, version: draft.snmp.version });
      if (!res.success) throw new Error(group);
      break;
    }
  }
}

/**
 * Persist every dirty group, sequentially, advancing baselines only on
 * success. Returns the number of groups that failed.
 */
export async function saveAll(): Promise<number> {
  if (_saving) return 0;
  const dirty = dirtyGroups;
  if (dirty.length === 0) {
    _error = false;
    return 0;
  }
  _saving = true;
  _error = false;
  notices.hostname = null;
  notices.timezone = null;
  setTlsSaveResult(null);
  let failed = 0;
  for (const group of dirty) {
    try {
      await saveGroup(group);
      baselines[group] = snap(group);
      (onSavedCallbacks[group] || []).forEach((cb) => cb());
    } catch {
      failed++;
    }
  }
  _saving = false;
  _error = failed > 0;
  return failed;
}
<!-- ========================================================================
     File          : Settings.svelte
     Author        : Nicolas Le Roux
     Created       : 28/09/2026
     Last modified : 19/10/2026 by Nicolas Le Roux
     Version       : 1.0.0

     Description:
     The Settings app: the 8 rail groups (general, security, network,
     appearance, widgets, notifications, AI, monitoring/system) rendered
     from the settings catalog, with the embedded TLS/Antivirus/
     Anti-Ransomware/SNMP/MCP sub-cards, wallpaper picker and the shared draft
     + explicit Save All flow.

     Dependencies:
       - svelte (>= 5), stores/settingsDraft.svelte + settingsCatalog,
         components/settings barrel, theme primitives

     Usage:
     Opened as a desktop window app.
     ======================================================================== -->
<script lang="ts">
  import { onMount } from 'svelte';
  import { get } from 'svelte/store';
  import { api } from '$lib/services/api';
  import type { DbBackupStatus } from '$lib/types/api';
  import { notificationsStore } from '$lib/stores/notifications';
  import { isAdmin } from '$lib/stores/auth';
  import { wallpaperStore, BUNDLED_WALLPAPERS } from '$lib/stores/wallpaper';
  import { monitorStore, REFRESH_INTERVAL_OPTIONS } from '$lib/stores/monitorStore';
  import { WIDGET_DEFS } from '$lib/stores/widgetCatalog';
  import { languageStore, t, translate } from '$lib/stores/i18n';
  import { SETTINGS_CATEGORIES, pendingSettingsCategory } from '$lib/stores/settingsCatalog';
  import { weatherLocationStore, type WeatherLocation } from '$lib/stores/weatherLocation';
  import { weatherDisplayPrefsStore, WEATHER_FIELDS, type WeatherField } from '$lib/stores/weatherDisplayPrefs';
  import {
    draft,
    invalidateGroups,
    syncWidgetsFromMonitor,
    syncWeatherFromStores,
    onGroupSaved,
    seedGroup,
    seedAutostart,
    seedAiModel,
    setSavedLanguage,
    notices,
    clearHostnameNotice,
    clearTimezoneNotice,
    clearLanguageNotice,
    clearMdnsNotice,
    setAutostartNotice,
    clearAutostartNotice,
    type SettingsGroup
  } from '$lib/stores/settingsDraft.svelte';
  import SettingsSaveButton from '$lib/components/settings/SettingsSaveButton.svelte';
  import { Alert, Switch } from '$lib/components/theme';
  import { PageLayout, ConfirmDialog } from '$lib/components/app';
  import { createConfirmState } from '$lib/utils/confirmDialog.svelte';
  import { groupTimezones, timezoneIsListed, type TimezoneGroup } from '$lib/utils/timezones';
  import { cn } from '$lib/utils.js';
  import WindowHeader from '$lib/components/desktop/WindowHeader.svelte';
  import WindowSidebarItem from '$lib/components/desktop/WindowSidebarItem.svelte';
  import {
    secLabel,
    secLabelFirst,
    settingsCard,
    settingsCardFooter,
    settingsRow,
    settingsRowHint,
    settingsRowLabel,
    settingsRowLast,
    settingsRowStacked,
    settingsLabel,
    settingsHint,
    settingsInput,
    settingsSelect,
    settingsSelectCompact,
    settingsTextarea,
    settingsBtn,
    settingsBtnGhost,
    settingsBtnPrimary,
    settingsBtnDanger,
    settingsIconTile
  } from '$lib/components/settings/vocabulary';
  import AntivirusCard from '$lib/components/settings/AntivirusCard.svelte';
  import AntiRansomwareCard from '$lib/components/settings/AntiRansomwareCard.svelte';
  import FirewallCard from '$lib/components/settings/FirewallCard.svelte';
  import TlsCard from '$lib/components/settings/TlsCard.svelte';
  import UpsCard from '$lib/components/settings/UpsCard.svelte';
  import SnmpCard from '$lib/components/settings/SnmpCard.svelte';
  import McpCard from '$lib/components/settings/McpCard.svelte';
  import KnowledgeBaseCard from '$lib/components/settings/KnowledgeBaseCard.svelte';
  import { IconAlertTriangle, IconCookie, IconShield, IconCheck, IconSettings, IconWifiOff, IconRefresh, IconMapPin, IconPencil, IconLock } from '@tabler/icons-svelte';

  // Sidebar categories — 8 grouped entries instead of a 12-item flat list.
  // The list itself is the single source of truth in
  // `stores/settingsCatalog.ts`, shared with the global ⌘K search index so a
  // category can never be visible in the rail but missing from search (or
  // vice-versa). Each `key` gates one or more of the `{#if activeCategory ===
  // …}` blocks in the markup; several former standalone categories stack
  // inside one group (each renders its own section label + card, so the group
  // still reads as labelled sub-sections — same pattern Security/Monitoring
  // already use):
  //
  //   general   → General
  //   security  → HTTPS/TLS + Antivirus + Anti-Ransomware + Firewall
  //   appearance→ Wallpaper
  //   widgets   → Active monitors + Refresh + Weather widget
  //   notifications → E-mail / SMTP + Alerts + Webhook
  //   ai        → AI Assistant + External agents (MCP gateway)
  //   monitoring→ UPS + SNMP
  //   system    → Software update + System information + Data management +
  //               Privacy
  const categories = SETTINGS_CATEGORIES;
  let activeCategory = 'general';
  /* The active category's record — drives the content header (icon, title,
     subtitle, accent). Falls back to the first entry so the header is never
     empty, even for a moment during hydration.

     Deliberately a legacy `$:` statement, not `$derived`: this component is
     legacy-mode (like Syncthing/Chat), and a single rune here would switch the
     whole file to runes mode, invalidating the ~40 plain `let`s and the other
     `$:` blocks it is built from. */
  let activeCat = categories[0];
  $: activeCat = categories.find((c) => c.key === activeCategory) ?? categories[0];

  // A ⌘K search hit can ask for a specific category (see
  // `stores/settingsCatalog.ts` → `requestSettingsCategory`). Consume the
  // one-shot request and immediately clear it, so reopening Settings from the
  // dock later does not re-jump to a stale category.
  $: if ($pendingSettingsCategory) {
    activeCategory = $pendingSettingsCategory;
    pendingSettingsCategory.set(null);
  }
  const dlg = createConfirmState();

  // Settings state — every editable value lives in `draft` (the shared
  // settingsDraft store) and is persisted ONLY by the sidebar-footer Save
  // button. There is no auto-save: changing a field just marks its group
  // dirty. See `stores/settingsDraft.svelte.ts`.

  // Version info
  let versionInfo = {
    current_version: '1.0.0',
    latest_version: null as string | null,
    up_to_date: true,
    release_date: null as string | null,
    error: null as string | null
  };

  // Server info
  let serverInfo = {
    ip: '',
    port: 0,
    display_address: ''
  };

  // Local UI state
  let loading = true;
  let testingEmail = false;
  let checkingUpdate = false;
  // ROADMAP 25.2: the host could not answer the start-on-boot probe, so the
  // switch must not pretend to know the current state. The alert says why.
  let autostartUnavailable = false;

  // Database snapshots (HomeVault's own SQLite database). The database runs in
  // WAL mode, so the `.db` file alone is NOT restorable — the backend writes
  // `VACUUM INTO` snapshots into `data/backups/`.
  let dbBackup: DbBackupStatus | null = null;
  let dbBackupLoading = true;
  let dbBackupBusy = false;
  let dbBackupError = '';
  let dbBackupInterval = 24;
  let dbBackupRetain = 7;

  // Data management (export/import/reset)
  let exportingSettings = false;
  let importingSettings = false;
  let resettingSettings = false;
  let importText = '';
  let importFileInput: HTMLInputElement;
  let importingFromFile = false;

  // Languages
  const languages = [
    { value: 'en', label: 'English' },
    { value: 'fr', label: 'Français' },
    { value: 'es', label: 'Español' },
    { value: 'de', label: 'Deutsch' }
  ];

  // Timezones — every zone the server can evaluate, grouped by region for
  // the picker. The list is shared with the scheduled-task form
  // (`$lib/utils/timezones`), so both screens offer the same zones: this
  // card used to hardcode 14 of them while the backend accepted any IANA
  // name, and the task form was a free-text field on top of that.
  let timezoneGroups: TimezoneGroup[] = [];

  async function loadTimezones() {
    const res = await api.getTimezones();
    const names = res.data?.timezones;
    if (res.success && Array.isArray(names) && names.length) {
      timezoneGroups = groupTimezones(names);
    }
  }

  // AI / Ollama
  let aiModels: { name: string; modified_at: string; size: number }[] = [];
  let currentModelDisplay = '';
  let ollamaRunning = false;
  let aiLoading = true;
  let aiPulling = false;
  let newModelName = '';
  let aiPullMessage = '';
  let aiPullError = '';
  let recommendations: {
    name: string; description: string; min_ram_gb: number;
    min_vram_gb: number | null; requires_gpu: boolean;
    speed: string; size_gb: number; fits: boolean;
  }[] = [];
  let systemSpecs: { total_ram_gb: number; has_gpu: boolean; gpu_name: string | null; gpu_vram_gb: number | null } | null = null;
  let recommending = false;

  // Real names from the public Ollama library — a typo here fails at pull
  // time, in front of the operator, so this list stays short and boring.
  const quickModels = [
    'qwen3:0.6b',
    'gemma3:1b',
    'qwen3:1.7b',
    'llama3.2:3b',
    'qwen3:4b',
    'gemma3:4b',
  ];

  async function loadAiModels() {
    aiLoading = true;
    aiPullMessage = '';
    aiPullError = '';
    try {
      const response = await api.chatListModels();
      if (response.success && response.data) {
        aiModels = response.data.models || [];
        ollamaRunning = response.data.ollama_running;
        currentModelDisplay = response.data.current;
        // Seed the AI group draft from the server's active model, but only
        // on first load — a later refresh must not clobber an unsaved pick.
        if (!draft.ai.model) {
          seedAiModel(response.data.current || '');
        }
      } else {
        aiModels = [];
        ollamaRunning = false;
      }
    } catch {
      aiModels = [];
      ollamaRunning = false;
    }
    aiLoading = false;
  }

  async function loadRecommendations() {
    recommending = true;
    try {
      const res = await api.chatRecommendModels();
      if (res.success && res.data) {
        recommendations = res.data.recommendations || [];
        systemSpecs = res.data.system;
      }
    } catch {
      recommendations = [];
      systemSpecs = null;
    }
    recommending = false;
  }

  /** Surfaces a settings save/action result via the notification center (not a toast — see notificationsStore). */
  function notifySettings(message: string, type: 'success' | 'error' = 'success') {
    notificationsStore.add({ title: translate('desktop.app_settings_name'), message, type });
  }

  async function handleInstallAndLoad(modelName: string) {
    newModelName = modelName;
    aiPulling = true;
    aiPullMessage = translate('settings.ai_installing_model', { model: modelName });
    aiPullError = '';
    try {
      const pullRes = await api.chatPullModel(modelName);
      if (pullRes.success) {
        aiPullMessage = translate('settings.ai_model_installed_activating', { model: modelName });
        await loadAiModels();
        // Stage the freshly installed model as the pick; the Save button
        // activates it server-side.
        draft.ai.model = modelName;
        aiPullMessage = translate('settings.ai_model_installed', { model: modelName });
      } else {
        aiPullError = pullRes.error || translate('settings.ai_install_failed');
        aiPullMessage = '';
      }
    } catch (err: any) {
      aiPullError = err?.message || translate('settings.ai_comm_error');
      aiPullMessage = '';
    }
    aiPulling = false;
  }

  async function handlePullModel() {
    const model = newModelName.trim();
    if (!model) return;
    aiPulling = true;
    aiPullMessage = translate('settings.ai_installing_model_wait', { model });
    aiPullError = '';
    try {
      const response = await api.chatPullModel(model);
      if (response.success) {
        aiPullMessage = translate('settings.ai_model_installed', { model });
        newModelName = '';
        await loadAiModels();
        draft.ai.model = model;
      } else {
        aiPullError = response.error || translate('settings.ai_install_failed');
        aiPullMessage = '';
      }
    } catch (err: any) {
      aiPullError = err?.message || translate('settings.ai_server_comm_error');
      aiPullMessage = '';
    }
    aiPulling = false;
  }

  // Update frequencies
  const updateFrequencies = [
    { value: 'manual', label: 'Manual' },
    { value: 'daily', label: 'Once per day' },
    { value: 'weekly', label: 'Once per week' },
    { value: 'monthly', label: 'Once per month' },
    { value: 'disabled', label: 'Disabled' }
  ];

  // Load settings, version info, and server info in parallel on mount
  onMount(async () => {
    await Promise.all([
      loadSettings(),
      loadVersionInfo(),
      loadServerInfo(),
      loadAiModels(),
      loadRecommendations(),
      loadAutostart(),
      loadNotificationEvents(),
      loadPower(),
      loadDbBackup(),
      loadTimezones()
    ]);
    // After a save flushes the AI group, keep the display model in step.
    onGroupSaved('ai', () => {
      currentModelDisplay = draft.ai.model;
    });
  });

  async function loadAutostart() {
    const res = await api.getAutostart();
    if (res.success && res.data) {
      seedAutostart(!!(res.data as any).enabled);
      // A host that answered is a host that can be toggled again.
      autostartUnavailable = false;
      clearAutostartNotice();
    } else {
      // ROADMAP 25.2: "the host could not answer" is not "disabled" — show
      // the reason and disable the switch instead of displaying a false off.
      autostartUnavailable = true;
      setAutostartNotice(res.message || translate('settings.autostart_update_failed'));
    }
  }

  async function loadSettings() {
    loading = true;
    try {
      const response = await api.getAllSettings();
      if (response.success && response.data) {
        const data = response.data;
        // Seed every main group into the draft store and baseline it. Nothing
        // is persisted here — the sidebar-footer Save button flushes only the
        // groups the user actually changed.
        draft.general = {
          server_name: data.general?.server_name || 'HomeVault',
          timezone: data.general?.timezone || 'UTC',
          // When the browser/OS or `localStorage['hv-language']` resolved a
          // non-English UI locale while the server still holds the unconfigured
          // `'en'` default, seed the selector from `$languageStore` so it
          // reflects the active interface language instead of "English".
          language:
            typeof localStorage !== 'undefined' && localStorage.getItem('hv-language')
              ? $languageStore
              : data.general?.language && data.general.language !== 'en'
                ? data.general.language
                : $languageStore || data.general?.language || 'en',
          // The live flag, not the stored one: the server reports what the
          // appliance actually does (ROADMAP 29.1).
          mdns_enabled: data.general?.mdns_enabled !== false
        };
        draft.email = {
          smtp_server: data.email?.smtp_server || '',
          smtp_port: data.email?.smtp_port || 587,
          smtp_username: data.email?.smtp_username || '',
          smtp_password: '',
          from_address: data.email?.from_address || '',
          use_tls: data.email?.use_tls ?? true
        };
        draft.update = {
          check_frequency: data.update?.check_frequency || 'weekly'
        };
        draft.notifications = {
          email: data.notifications?.email || '',
          enable_alerts: data.notifications?.enable_alerts ?? true,
          alert_threshold: data.notifications?.alert_threshold || 80,
          smtp_enabled: data.notifications?.smtp_enabled ?? false,
          webhook_url: data.notifications?.webhook_url || '',
          webhook_enabled: data.notifications?.webhook_enabled ?? false,
          webhook_format: data.notifications?.webhook_format || 'generic'
        };
        draft.appearance = {
          wallpaper: '',
          accent_color: data.appearance?.accent_color || '#1E62AE'
        };

        // Apply the persisted accent color and wallpaper live — the desktop
        // updates immediately, but the server only gets the new value on Save.
        document.documentElement.style.setProperty('--hv-primary', draft.appearance.accent_color);
        if (data.appearance?.wallpaper) {
          try {
            // applyRaw keeps only a bundled /wallpapers/ image; anything else
            // resolves to the default photo. The server validates the value
            // on save and on load (ROADMAP 26.1), so this catch is a safety
            // net rather than the only check.
            wallpaperStore.applyRaw(JSON.parse(data.appearance.wallpaper));
          } catch {
            // A value that is not JSON at all (hand-edited export).
          }
        }
        // The wallpaper draft is the live store's JSON — the same value the
        // appearance saver sends — so the baseline matches what is on screen.
        draft.appearance.wallpaper = JSON.stringify($wallpaperStore);

        // Baseline every seeded group so a fresh load never reads as dirty.
        setSavedLanguage(draft.general.language);
        for (const g of ['general', 'email', 'update', 'notifications', 'appearance'] as SettingsGroup[]) {
          seedGroup(g);
        }

        // Widget layout + weather preferences live in their own stores and are
        // mirrored into the draft (unless already edited). Fire both after the
        // main groups so their baselines are set from the live state.
        syncWidgetsFromMonitor(get(monitorStore));
        syncWeatherFromStores(get(weatherLocationStore), get(weatherDisplayPrefsStore));
        // Enable the live wallpaper→draft mirror once the baseline is set.
        wallpaperDirtySync = true;
      }
    } catch (error) {
      console.error('Failed to load settings:', error);
      notifySettings(translate('settings.load_settings_failed'), 'error');
    }
    loading = false;
  }

  async function loadVersionInfo() {
    try {
      const response = await api.checkForUpdates();
      if (response.success && response.data) {
        versionInfo = response.data;
      }
    } catch (error) {
      console.error('Failed to load version info:', error);
    }
  }

  async function loadServerInfo() {
    try {
      const response = await api.getServerInfo();
      if (response.success && response.data) {
        serverInfo = response.data;
      }
    } catch (error) {
      console.error('Failed to load server info:', error);
    }
  }

  // Wallpaper choice is applied live (the desktop behind the window updates
  // immediately); the server only persists it when the user clicks Save (the
  // `appearance` group). The default-gradient tile was removed (explicit
  // request) — every wallpaper offered here is now a bundled photo.
  //
  // `selectedWpUrl` is a reactive `$:` (not a helper fn) so the tile's
  // `class:is-selected`, `aria-checked` AND the check-badge `{#if}` all
  // re-evaluate when the store changes — calling a function that reads
  // `$wallpaperStore` only re-ran the `class:` directive.
  $: selectedWpUrl = $wallpaperStore.type === 'image' ? $wallpaperStore.value : '';

  // Mirror the live wallpaper into the `appearance` draft so picking a tile
  // marks that group dirty — but only after the initial load has baselined it.
  let wallpaperDirtySync = false;
  $: if (wallpaperDirtySync) draft.appearance.wallpaper = JSON.stringify($wallpaperStore);
  // ROADMAP 26.1 (option A): the appliance look is one global value an admin
  // saves. A non-admin reads it (the boot hydration paints it) but cannot
  // write it — the grid is shown read-only with an explanatory line instead
  // of a control that appears to save and does not.
  $: appearanceReadOnly = !$isAdmin;

  // Keep the widgets + weather drafts in step with their live stores — the
  // sync helpers no-op while the group is dirty, so a late server sync can
  // never clobber edits the user staged for Save.
  $: syncWidgetsFromMonitor($monitorStore);
  $: syncWeatherFromStores($weatherLocationStore, $weatherDisplayPrefsStore);

  // --- Per-event notification toggles (own endpoint, immediate save) -----
  interface NotificationEventRow {
    id: string;
    group: string;
    label_key: string;
    enabled: boolean;
  }
  let notificationEvents: NotificationEventRow[] = [];
  let powerLoaded = false;
  let powerGovernors: string[] = [];
  let powerGovernor = '';
  let powerGovernorSupported = false;
  let powerSpindownSupported = false;
  let powerSpindown: { device: string; apm_level: number | null; spindown_minutes: number | null }[] = [];
  let newSpindown = { device: '', apm_level: 127, spindown_minutes: 20 };
  let powerBusy = false;

  async function loadNotificationEvents() {
    try {
      const res = await api.getNotificationEventsConfig();
      if (res.success) notificationEvents = res.data?.events ?? [];
    } catch {
      notificationEvents = [];
    }
  }

  async function toggleNotificationEvent(id: string, enabled: boolean) {
    // Optimistic update; the row's Switch shows the new state immediately
    // and a failed PUT rolls it back (same UX as the SNMP/UPS cards).
    const row = notificationEvents.find((e) => e.id === id);
    if (row) row.enabled = enabled;
    const res = await api.updateNotificationEventsConfig({ events: { [id]: enabled } });
    if (!res.success) {
      if (row) row.enabled = !enabled;
      notifySettings(res.message || translate('settings.save_failed'), 'error');
    }
  }

  // ── Database snapshots ──────────────────────────────────────────────
  async function loadDbBackup() {
    dbBackupLoading = true;
    try {
      const res = await api.getDbBackupStatus();
      if (res.success && res.data) {
        dbBackup = res.data;
        dbBackupInterval = res.data.settings.interval_hours;
        dbBackupRetain = res.data.settings.retain;
      }
    } catch {
      /* the card degrades to its empty state */
    } finally {
      dbBackupLoading = false;
    }
  }

  async function handleBackupNow() {
    dbBackupBusy = true;
    dbBackupError = '';
    try {
      const res = await api.backupDatabaseNow();
      if (res.success && res.data?.status) {
        dbBackup = res.data.status;
      } else {
        dbBackupError = res.message || translate('settings.db_backup_failed');
      }
    } catch {
      dbBackupError = translate('settings.db_backup_failed');
    } finally {
      dbBackupBusy = false;
    }
  }

  async function handleSaveDbBackup() {
    dbBackupBusy = true;
    dbBackupError = '';
    try {
      const res = await api.saveDbBackupSettings(dbBackupInterval, dbBackupRetain);
      if (res.success && res.data) {
        dbBackup = res.data;
      } else {
        dbBackupError = res.message || translate('settings.db_backup_failed');
      }
    } catch {
      dbBackupError = translate('settings.db_backup_failed');
    } finally {
      dbBackupBusy = false;
    }
  }

  /// Human-readable size for a snapshot row.
  function formatBytes(bytes: number): string {
    if (!bytes || bytes <= 0) return '0 B';
    const units = ['B', 'KB', 'MB', 'GB'];
    const i = Math.min(units.length - 1, Math.floor(Math.log(bytes) / Math.log(1024)));
    return `${(bytes / Math.pow(1024, i)).toFixed(i === 0 ? 0 : 1)} ${units[i]}`;
  }

  /// Short local timestamp for a snapshot row.
  function formatBackupDate(iso: string): string {
    if (!iso) return '—';
    const d = new Date(iso);
    return Number.isNaN(d.getTime()) ? iso : d.toLocaleString();
  }

  async function loadPower() {
    if (powerLoaded) return;
    try {
      const [statusRes, spindownRes] = await Promise.all([api.getPowerStatus(), api.getPowerSpindown()]);
      if (statusRes.success && statusRes.data) {
        powerGovernors = statusRes.data.available_governors ?? [];
        powerGovernor = statusRes.data.governor ?? '';
        powerGovernorSupported = statusRes.data.governor_supported;
        powerSpindownSupported = statusRes.data.spindown_supported;
      }
      if (spindownRes.success) powerSpindown = spindownRes.data?.settings ?? [];
    } catch {
      // Host without cpufreq/hdparm: cards render with unsupported states.
    } finally {
      powerLoaded = true;
    }
  }

  async function saveGovernor() {
    powerBusy = true;
    try {
      const res = await api.setPowerGovernor(powerGovernor);
      notifySettings(res.success ? translate('power.saved') : (res.message || translate('settings.save_failed')), res.success ? 'success' : 'error');
      if (res.success) powerGovernor = (res.data?.governor as string) || powerGovernor;
    } catch {
      notifySettings(translate('settings.save_failed'), 'error');
    } finally {
      powerBusy = false;
    }
  }

  async function hostPower(action: 'reboot' | 'shutdown' | 'suspend') {
    const message = action === 'reboot' ? translate('power.confirm_reboot')
      : action === 'shutdown' ? translate('power.confirm_shutdown')
      : translate('power.confirm_suspend');
    if (!(await dlg.ask(message))) return;
    powerBusy = true;
    try {
      const res = await api.hostPowerAction(action);
      if (!res.success) notifySettings(res.message || translate('settings.save_failed'), 'error');
    } finally {
      powerBusy = false;
    }
  }

  async function addSpindown() {
    if (!newSpindown.device.trim()) return;
    powerBusy = true;
    try {
      const res = await api.setPowerSpindown({
        device: newSpindown.device.trim(),
        apm_level: newSpindown.apm_level,
        spindown_minutes: newSpindown.spindown_minutes,
      });
      if (res.success) {
        powerSpindown = [...powerSpindown.filter((x) => x.device !== newSpindown.device.trim()), res.data!.setting].sort((a, b) => a.device.localeCompare(b.device));
        notifySettings(res.data?.applied ? translate('power.applied_ok') : translate('power.applied_failed'));
        newSpindown = { device: '', apm_level: 127, spindown_minutes: 20 };
      } else {
        notifySettings(res.message || translate('settings.save_failed'), 'error');
      }
    } finally {
      powerBusy = false;
    }
  }

  async function removeSpindown(device: string) {
    powerBusy = true;
    try {
      const res = await api.setPowerSpindown({ device });
      if (res.success) powerSpindown = powerSpindown.filter((x) => x.device !== device);
    } finally {
      powerBusy = false;
    }
  }

  async function applyAllSpindown() {
    powerBusy = true;
    try {
      await api.applyPowerSpindown();
      notifySettings(translate('power.applied_ok'));
    } finally {
      powerBusy = false;
    }
  }

  async function handleTestEmail() {
    testingEmail = true;
    try {
      const response = await api.testEmailSettings();
      if (response.success) {
        notifySettings(translate('settings.test_sent'));
      } else {
        notifySettings(response.error || translate('settings.test_failed'), 'error');
      }
    } catch (error) {
      notifySettings(translate('settings.test_failed'), 'error');
    }
    testingEmail = false;
  }

  async function handleCheckUpdate() {
    checkingUpdate = true;
    try {
      // "Check now" is the explicit opt-in: it is the only action that makes
      // the server contact the update endpoint (POST), whereas the mount-time
      // load above only reads the cached answer (GET).
      const response = await api.checkForUpdatesNow();
      if (response.success && response.data) {
        versionInfo = response.data;
      } else {
        notifySettings(response.error || translate('settings.check_failed'), 'error');
      }
    } catch (error) {
      notifySettings(translate('settings.check_failed'), 'error');
    }
    checkingUpdate = false;
  }

  async function handleExportSettings() {
    exportingSettings = true;
    try {
      const response = await api.exportSettings();
      if (response.success && response.data) {
        const json = JSON.stringify(response.data, null, 2);
        const blob = new Blob([json], { type: 'application/json' });
        const url = URL.createObjectURL(blob);
        const a = document.createElement('a');
        a.href = url;
        a.download = `homevault-settings-${new Date().toISOString().slice(0, 10)}.json`;
        document.body.appendChild(a);
        a.click();
        document.body.removeChild(a);
        URL.revokeObjectURL(url);
        notifySettings(translate('settings.settings_exported'));
      } else {
        notifySettings(response.error || translate('settings.export_failed'), 'error');
      }
    } catch (error) {
      notifySettings(translate('settings.export_failed'), 'error');
    }
    exportingSettings = false;
  }

  async function handleImportSettings() {
    let data: any = null;
    if (importingFromFile) {
      if (!importFileInput?.files?.length) {
        notifySettings(translate('settings.reset_confirm_msg'), 'error');
        return;
      }
      try {
        const text = await importFileInput.files[0].text();
        data = JSON.parse(text);
      } catch {
        notifySettings(translate('settings.invalid_json'), 'error');
        return;
      }
    } else {
      if (!importText.trim()) {
        notifySettings(translate('settings.paste_json_import'), 'error');
        return;
      }
      try {
        data = JSON.parse(importText);
      } catch {
        notifySettings(translate('settings.invalid_json'), 'error');
        return;
      }
    }

    importingSettings = true;
    try {
      const response = await api.importSettings(data);
      if (response.success) {
        importText = '';
        if (importFileInput) importFileInput.value = '';
        importingFromFile = false;
        // Re-read everything: main groups are re-seeded by loadSettings; the
        // independently-loaded ones (autostart, AI) are invalidated + refetched.
        invalidateGroups('ai', 'autostart', 'widgets', 'weather');
        await loadSettings();
        await loadAutostart();
        await loadAiModels();
        notifySettings(translate('settings.settings_imported'));
      } else {
        notifySettings(response.error || translate('settings.import_failed'), 'error');
      }
    } catch (error) {
      notifySettings(translate('settings.import_failed'), 'error');
    }
    importingSettings = false;
  }

  async function handleResetSettings() {
    if (!(await dlg.ask(translate('settings.reset_confirm')))) return;
    resettingSettings = true;
    try {
      const response = await api.resetSettings();
      if (response.success) {
        invalidateGroups('ai', 'autostart', 'widgets', 'weather');
        await loadSettings();
        await loadAutostart();
        await loadAiModels();
        // ROADMAP 13.5: the reset now also clears the families stored outside
        // settings.json; a domain that could not be reset is named instead of
        // being hidden behind a plain success toast.
        const warnings = ((response.data as any)?.warnings as string[] | undefined) ?? [];
        if (warnings.length > 0) {
          notifySettings(
            translate('settings.settings_reset_partial', { reason: warnings.join('; ') }),
            'error',
          );
        } else {
          notifySettings(translate('settings.settings_reset'));
        }
      } else {
        notifySettings(response.error || translate('settings.reset_failed'), 'error');
      }
    } catch (error) {
      notifySettings(translate('settings.reset_failed'), 'error');
    }
    resettingSettings = false;
  }

  // Check if update available
  function isUpdateAvailable(): boolean {
    return versionInfo.latest_version !== null && !versionInfo.up_to_date;
  }

  // Weather widget location — the desktop Weather monitor only ever reads
  // weatherLocationStore now; this is the one place that writes to it,
  // via the same free, no-key, CORS-enabled Open-Meteo geocoding endpoint
  // the widget itself used to call directly before this moved here.
  let weatherSearchQuery = '';
  let weatherSearchResults: WeatherLocation[] = [];
  let weatherSearching = false;
  let weatherEditingLocation = false;

  async function searchWeatherLocation() {
    const query = weatherSearchQuery.trim();
    if (!query) return;
    weatherSearching = true;
    weatherSearchResults = [];
    try {
      const url = `https://geocoding-api.open-meteo.com/v1/search?name=${encodeURIComponent(query)}&count=5&language=${$languageStore}&format=json`;
      const res = await fetch(url);
      if (!res.ok) throw new Error(`HTTP ${res.status}`);
      const json = await res.json();
      weatherSearchResults = (json.results ?? []).map((r: any) => ({
        name: [r.name, r.admin1, r.country].filter(Boolean).join(', '),
        lat: r.latitude,
        lon: r.longitude,
      }));
    } catch {
      notifySettings(translate('desktop.weather_search_failed'), 'error');
    }
    weatherSearching = false;
  }

  function chooseWeatherLocation(location: WeatherLocation) {
    // Stage the pick into the draft; the desktop widget only follows once the
    // user clicks Save (the `weather` saver pushes to the location store).
    draft.weather.location = location;
    weatherEditingLocation = false;
    weatherSearchQuery = '';
    weatherSearchResults = [];
  }

  function openWeatherLocationEditor() {
    weatherEditingLocation = true;
    weatherSearchQuery = '';
    weatherSearchResults = [];
  }

  // Optional Open-Meteo fields the desktop Weather widget can show, on top
  // of the always-shown temperature/condition/humidity/wind. Reuses the
  // same `desktop.weather_<field>` labels the widget itself displays.
  function handleWeatherFieldToggle(field: WeatherField, checked: boolean) {
    const fields = [...draft.weather.displayFields];
    if (checked && !fields.includes(field)) fields.push(field);
    if (!checked) {
      const i = fields.indexOf(field);
      if (i >= 0) fields.splice(i, 1);
    }
    draft.weather.displayFields = fields;
  }

  // ---------------------------------------------------------------------
  // Class vocabulary — the shared "section label + card + row" language of
  // the Settings design. It lives in `components/settings/vocabulary.ts` so
  // the six embedded sub-cards (built on `SettingsCard`) share the same one.
  // ---------------------------------------------------------------------
</script>

<PageLayout
  title={$t('desktop.app_settings_name')}
  data-testid="settings-page"
  sidebarLabel={$t('settings.subtitle')}
  sidebarTestId="settings-sidebar"
  sidebarWidth="var(--hv-settings-sidebar-w)"
  sidebarClass="[--winsidebar-pad:8px] bg-[var(--hv-settings-tile)]! border-r-[1.45px]! border-r-[var(--hv-settings-border)]!"
  bodyClass="p-0! overflow-hidden! flex flex-col bg-[var(--hv-settings-canvas)]"
>
  {#snippet icon()}
    <IconSettings stroke={1.7} />
  {/snippet}

  {#snippet sidebar()}
    <!-- Rail header — the micro-label above the category list. Rendered as the
         first child of the rail body and pulled full-bleed with negative
         margins, so the 8px body padding doesn't inset its hairline.
         The size token needs its `length:` type hint. `--hv-text-*` is
         overloaded — it holds colours (--hv-text-primary) *and* sizes
         (--hv-text-xs) — so Tailwind cannot tell which property an unhinted
         `text-[var(…)]` means and emits a `color:` rule: right for the
         colours, silently wrong for the sizes, which then render at the
         inherited size instead of their own.
         Keep the token spelled out only in this prose form: Tailwind scans
         comments for class candidates too, so writing the full unhinted
         utility here would emit the very dead rule this note explains. -->
    <div class="-mx-2 -mt-2 mb-2 px-4 py-4 border-b-[1.45px] border-[var(--hv-settings-hairline)] text-c-xs font-bold uppercase tracking-[var(--hv-letter-spacing-wider)] leading-[var(--hv-line-height-normal)] text-[var(--hv-settings-label)]">
      {$t('desktop.app_settings_name')}
    </div>
    <nav class="flex flex-col gap-0.5" aria-label={$t('settings.subtitle')}>
      {#each categories as cat (cat.key)}
        <WindowSidebarItem
          variant="rich"
          label={$t(cat.labelKey)}
          subtitle={$t(cat.subtitleKey)}
          accent={cat.accent}
          testId="settings-nav-item"
          class={cn(activeCategory === cat.key && 'set-nav-active')}
          active={activeCategory === cat.key}
          onclick={() => (activeCategory = cat.key)}
        >
          {#snippet icon()}<svelte:component this={cat.icon} size={16} />{/snippet}
        </WindowSidebarItem>
      {/each}
    </nav>
  {/snippet}

  {#snippet sidebarFooter()}
    <!-- One control in the rail footer: the Save button is also the screen's
         save indicator (colour + label state machine, plus the sr-only status
         line it renders) — the visible status pill above it and the unsaved
         caption under it were removed 2026-09-23. The header bar stays
         icon + title + window controls only. -->
    <SettingsSaveButton />
  {/snippet}

  <!-- Content header — the active category's identity: title over the same
       subtitle the rail item shows, on two lines, and nothing else. The 36px
       tinted icon tile was removed 2026-10-19: the active rail item already
       carries the category glyph, and the tile cost the panel 36px of height.
       Sits flush above the scrolling panel and does not scroll with it. -->
  <div class="flex-none px-6 py-2.5 bg-[var(--hv-settings-tile)] border-b-[1.45px] border-[var(--hv-settings-border)]">
    <!-- The type utilities are `!` on purpose: `styles/theme/base.css`
         declares a bare `h2 { font-size: … }` OUTSIDE any `@layer`, and an
         unlayered rule outranks every layered Tailwind utility no matter how
         specific the utility's class is. Without the flag this heading
         renders at the 28px base size instead of the design's 16px/700
         (`text-md`, the 16px content-scale step — `text-base` is 14px). -->
    <h2 data-testid="settings-header-title" class="m-0 text-md! font-bold! leading-5! text-[var(--hv-settings-title)]!">{$t(activeCat.labelKey)}</h2>
    <!-- `mb-0!` is load-bearing for the same reason as the `!` above, one
         rule further down: `base.css` declares an unlayered
         `p { margin-bottom: var(--hv-space-4) }`, so a plain `m-0` loses and
         the header silently keeps 16px of dead height under the subtitle
         (measured 73px instead of 57). -->
    <p data-testid="settings-header-subtitle" class="m-0! text-xs leading-4 text-[var(--hv-settings-label)]">{$t(activeCat.subtitleKey)}</p>
  </div>

  {#if loading}
    <div class="flex-1 min-h-0 flex items-center justify-center gap-3 text-[var(--hv-settings-label)]">
      <div data-testid="settings-spinner" class="set-spinner"></div>
      <span>{$t('settings.loading_settings')}</span>
    </div>
  {:else}
    <!-- The scroll container. Every section label and every card is a DIRECT
         child of this element (E2E 2.5): the labels are siblings of their
         card, spaced with padding, and the cards carry `shrink-0` so a tall
         card is never squeezed into clipping its own content. -->
    <div data-testid="settings-content" class="flex-1 min-h-0 overflow-y-auto flex flex-col px-6 pb-4">
      {#if activeCategory === 'general'}
        <div data-testid="settings-card-title" class={secLabelFirst}></div>
        <div data-testid="settings-card" class={settingsCard}>
          {#if notices.hostname}
            <div class="py-3 border-b-[1.45px] border-[var(--hv-settings-hairline)]">
              <Alert variant="warning" dismissible ondismiss={clearHostnameNotice}>{notices.hostname}</Alert>
            </div>
          {/if}
          {#if notices.timezone}
            <div class="py-3 border-b-[1.45px] border-[var(--hv-settings-hairline)]">
              <Alert variant="warning" dismissible ondismiss={clearTimezoneNotice}>{notices.timezone}</Alert>
            </div>
          {/if}
          {#if notices.language}
            <div class="py-3 border-b-[1.45px] border-[var(--hv-settings-hairline)]">
              <Alert variant="warning" dismissible ondismiss={clearLanguageNotice}>{$t('settings.language_save_failed', { reason: notices.language })}</Alert>
            </div>
          {/if}
          <div class={settingsRow}>
            <label class={settingsRowLabel} for="serverName">{$t('settings.server_name')}</label>
            <input id="serverName" class={cn(settingsInput, 'max-w-[280px]')} type="text" bind:value={draft.general.server_name} placeholder="HomeVault" />
          </div>
          <div class={settingsRow}>
            <label class={settingsRowLabel} for="language">{$t('settings.language_label')}</label>
            <select id="language" class={cn(settingsSelect, 'max-w-[280px]')} bind:value={draft.general.language}>
              {#each languages as lang}<option value={lang.value}>{lang.label}</option>{/each}
            </select>
          </div>
          <div class={settingsRow}>
            <label class={settingsRowLabel} for="timezone">{$t('settings.timezone')}</label>
            <div class="flex min-w-0 flex-col gap-1 @min-[480px]:items-end">
              <select
                id="timezone"
                class={cn(settingsSelect, 'max-w-[280px]')}
                bind:value={draft.general.timezone}
                aria-describedby="timezoneHint"
              >
                {#if !timezoneIsListed(timezoneGroups, draft.general.timezone)}
                  <option value={draft.general.timezone}>{draft.general.timezone}</option>
                {/if}
                <option value="UTC">UTC</option>
                {#each timezoneGroups as group}
                  <optgroup label={group.region}>
                    {#each group.zones as zone}<option value={zone}>{zone}</option>{/each}
                  </optgroup>
                {/each}
              </select>
              <p id="timezoneHint" class={cn(settingsRowHint, 'max-w-[280px]')}>{$t('settings.timezone_hint')}</p>
            </div>
          </div>
          <div class={settingsRow} data-testid="settings-autostart-row">
            <div class="min-w-0">
              <span class={settingsRowLabel}>{$t('settings.autostart_label')}</span>
              <p class={settingsRowHint}>{$t('settings.autostart_description')}</p>
            </div>
            <Switch
              bind:checked={draft.autostart.enabled}
              accent={activeCat.accent}
              disabled={autostartUnavailable}
              ariaLabel={$t('settings.autostart_label')}
              data-testid="settings-autostart"
            />
          </div>
          {#if notices.autostart}
            <div class="py-3 border-t-[1.45px] border-[var(--hv-settings-hairline)]" data-testid="autostart-error">
              <Alert variant="error" dismissible ondismiss={clearAutostartNotice}>
                {$t(
                  autostartUnavailable ? 'settings.autostart_read_failed' : 'settings.autostart_error',
                  { reason: notices.autostart }
                )}
              </Alert>
            </div>
          {/if}
          <div class={settingsRow} data-testid="settings-mdns-row">
            <div class="min-w-0">
              <span class={settingsRowLabel}>{$t('settings.mdns_label')}</span>
              <p class={settingsRowHint}>{$t('settings.mdns_description')}</p>
            </div>
            <Switch
              bind:checked={draft.general.mdns_enabled}
              accent={activeCat.accent}
              ariaLabel={$t('settings.mdns_label')}
              data-testid="settings-mdns"
            />
          </div>
          {#if notices.mdns}
            <div class="py-3 border-t-[1.45px] border-[var(--hv-settings-hairline)]" data-testid="mdns-error">
              <Alert variant="error" dismissible ondismiss={clearMdnsNotice}>
                {$t('settings.mdns_apply_failed', { reason: notices.mdns })}
              </Alert>
            </div>
          {/if}
        </div>
      {/if}
      {#if activeCategory === 'security'}
        <!-- The four security sub-cards carry their own header (title +
             subtitle, two lines) via the shared SettingsCard shell, so they
             need no section label above them — their `settings-card-title` IS
             the card heading (E2E 12.1 reads those). They stay *direct*
             children of the panel so E2E 2.5's clip check still covers them. -->
        <TlsCard class="mt-6" />
        <AntivirusCard class="mt-3" />
        <AntiRansomwareCard class="mt-3" />
        <FirewallCard class="mt-3" />
      {/if}
      {#if activeCategory === 'appearance'}
        <div data-testid="settings-card-title" class={secLabelFirst}></div>
        <div data-testid="settings-card" class={settingsCard}>
          <div class={settingsRowStacked}>
            <span class={settingsLabel} id="wallpaperLabel">{$t('settings.wallpaper')}</span>
            <p class={settingsHint}>{$t('settings.wallpaper_hint')}</p>
            <!-- ROADMAP 26.1 (option A): the look is one global value written
                 by an admin PUT. A non-admin gets the grid read-only plus the
                 reason, never a control that appears to save and does not. -->
            {#if appearanceReadOnly}
              <p class="settings-hint text-xs leading-4 text-[var(--hv-settings-label)]" data-testid="appearance-read-only">
                <IconLock size={14} stroke={2} class="inline-block align-[-2px] mr-1" />
                {$t('settings.wallpaper_managed_by_admin')}
              </p>
            {/if}
            <!-- Container query, not a viewport breakpoint: the grid follows
                 the width the window actually gives it. 3 columns normally,
                 2 once the card is narrower than 380px. -->
            <div class="@container">
              <div class="settings-wallpaper-grid grid grid-cols-3 @max-[380px]:grid-cols-2 gap-3 mt-1" role="radiogroup" aria-labelledby="wallpaperLabel" aria-readonly={appearanceReadOnly} aria-disabled={appearanceReadOnly} data-readonly={appearanceReadOnly}>
                {#each BUNDLED_WALLPAPERS as w, i (w.id)}
                  <button type="button" class="settings-wallpaper-tile relative block w-full p-0 border-0 bg-transparent cursor-pointer" class:is-selected={w.url === selectedWpUrl} role="radio" aria-checked={w.url === selectedWpUrl} aria-disabled={appearanceReadOnly} disabled={appearanceReadOnly} aria-label={`${$t('settings.wallpaper')} ${i + 1}`} onclick={() => wallpaperStore.setWallpaper({ type: 'image', value: w.url })}>
                    <!-- `aspect-[8/5]` is the equaliser: the bundled thumbs are
                         not all the same shape (3:2, 4:3 and 16:9 all occur,
                         they follow the source photo), so a height derived
                         from the file let the taller ones render above their
                         neighbours. A fixed ratio + `cover` gives all 44 tiles
                         one identical box. `h-auto` is required, not cosmetic:
                         the `height="200"` attribute is a presentational
                         hint mapped to `height:200px`, and with both width
                         and height definite `aspect-ratio` is ignored.
                         `block` kills the inline baseline gap under the
                         image, which otherwise pads the tile by ~4px. -->
                    <img class="settings-wallpaper-thumb block w-full h-auto aspect-[8/5] rounded-lg object-cover border-[1.45px] border-[var(--hv-settings-border)]" src={w.thumb} alt="" decoding="async" width="320" height="200" />
                    {#if w.url === selectedWpUrl}<span class="settings-wallpaper-check absolute top-1.5 right-1.5 grid place-items-center w-6 h-6 rounded-full bg-[var(--hv-primary)] text-[var(--hv-on-primary)]"><IconCheck size={16} stroke={3} /></span>{/if}
                  </button>
                {/each}
              </div>
            </div>
          </div>
        </div>
      {/if}
      {#if activeCategory === 'notifications'}
        <div data-testid="settings-card-title" class={secLabelFirst}></div>
        <div data-testid="settings-card" class={settingsCard}>
          <div class={settingsRow}>
            <label class={settingsRowLabel} for="smtpServer">{$t('settings.smtp_server')}</label>
            <input id="smtpServer" class={cn(settingsInput, 'max-w-[280px]')} type="text" bind:value={draft.email.smtp_server} placeholder="smtp.example.com" />
          </div>
          <div class={settingsRow}>
            <label class={settingsRowLabel} for="smtpPort">{$t('settings.smtp_port')}</label>
            <input id="smtpPort" class={cn(settingsInput, 'max-w-[280px]')} type="number" bind:value={draft.email.smtp_port} placeholder="587" />
          </div>
          <div class={settingsRow}>
            <label class={settingsRowLabel} for="smtpUsername">{$t('settings.username_label')}</label>
            <input id="smtpUsername" class={cn(settingsInput, 'max-w-[280px]')} type="text" bind:value={draft.email.smtp_username} placeholder="user@example.com" />
          </div>
          <div class={settingsRow}>
            <label class={settingsRowLabel} for="smtpPassword">{$t('settings.password_label')}</label>
            <input id="smtpPassword" class={cn(settingsInput, 'max-w-[280px]')} type="password" bind:value={draft.email.smtp_password} placeholder={$t('settings.smtp_password_placeholder')} />
          </div>
          <div class={settingsRow}>
            <label class={settingsRowLabel} for="fromAddress">{$t('settings.from_address')}</label>
            <input id="fromAddress" class={cn(settingsInput, 'max-w-[280px]')} type="email" bind:value={draft.email.from_address} placeholder="noreply@example.com" />
          </div>
          <div class={settingsRowLast}>
            <span class={settingsRowLabel}>{$t('settings.use_tls')}</span>
            <Switch
              bind:checked={draft.email.use_tls}
              accent={activeCat.accent}
              ariaLabel={$t('settings.use_tls')}
            />
          </div>
          <div data-testid="settings-card-footer" class={settingsCardFooter}>
            <button class={cn(settingsBtn, settingsBtnGhost)} onclick={handleTestEmail} disabled={testingEmail}>{testingEmail ? $t('settings.sending') : $t('settings.test_email')}</button>
          </div>
        </div>
      {/if}
      {#if activeCategory === 'notifications'}
        <div data-testid="settings-card-title" class={secLabel}></div>
        <div data-testid="settings-card" class={settingsCard}>
          <div class={settingsRow}>
            <div class="min-w-0">
              <label class={settingsRowLabel} for="notifyEmail">{$t('settings.notification_email')}</label>
              <p class={settingsRowHint}>{$t('settings.receive_system_alerts')}</p>
            </div>
            <input id="notifyEmail" class={cn(settingsInput, 'max-w-[280px]')} type="email" bind:value={draft.notifications.email} placeholder="admin@example.com" />
          </div>
          <div class={settingsRowStacked}>
            <label class={settingsRowLabel} for="alertThreshold">{$t('settings.alert_threshold')}: {draft.notifications.alert_threshold}%</label>
            <input type="range" id="alertThreshold" min="0" max="100" step="5" bind:value={draft.notifications.alert_threshold} class="settings-alert-range w-full h-[var(--hv-touch-target-min)] accent-[var(--hv-primary)]" />
          </div>
          <div class={settingsRow}>
            <span class={settingsRowLabel}>{$t('settings.enable_alert_notifications')}</span>
            <Switch
              bind:checked={draft.notifications.enable_alerts}
              accent={activeCat.accent}
              ariaLabel={$t('settings.enable_alert_notifications')}
            />
          </div>
          <div class={settingsRow}>
            <span class={settingsRowLabel}>{$t('settings.use_smtp_for_notifications')}</span>
            <Switch
              bind:checked={draft.notifications.smtp_enabled}
              accent={activeCat.accent}
              ariaLabel={$t('settings.use_smtp_for_notifications')}
            />
          </div>
          <div class={settingsRowStacked}>
            <label class={settingsRowLabel} for="webhookUrl">{$t('settings.webhook_url_label')}</label>
            <input id="webhookUrl" class={settingsInput} type="url" bind:value={draft.notifications.webhook_url} placeholder="https://example.com/webhook" />
            <p class={settingsRowHint}>{$t('settings.webhook_hint')}</p>
          </div>
          <div class={settingsRowLast}>
            <span class={settingsRowLabel}>{$t('settings.enable_webhook_notifications')}</span>
            <Switch
              bind:checked={draft.notifications.webhook_enabled}
              accent={activeCat.accent}
              ariaLabel={$t('settings.enable_webhook_notifications')}
            />
          </div>
          <div class={settingsRowStacked}>
            <label class={settingsRowLabel} for="webhookFormat">{$t('notifications.events.webhook_format')}</label>
            <select id="webhookFormat" class={cn(settingsSelect, 'max-w-[280px]')} bind:value={draft.notifications.webhook_format}>
              <option value="generic">{$t('notifications.events.format_generic')}</option>
              <option value="slack">{$t('notifications.events.format_slack')}</option>
              <option value="discord">{$t('notifications.events.format_discord')}</option>
            </select>
            <p class={settingsRowHint}>{$t('notifications.events.webhook_format_help')}</p>
          </div>
        </div>
        <!-- Per-event toggles: a separate endpoint (notification_events),
             saved immediately on toggle — deliberately outside the draft
             flow because it is not part of the aggregate PUT. -->
        <div data-testid="settings-card-title" class={secLabel}></div>
        <div data-testid="settings-card" class={settingsCard}>
          <div class={settingsRowStacked}>
            <span class={settingsRowLabel}>{$t('notifications.events.title')}</span>
            <p class={settingsRowHint}>{$t('notifications.events.desc')}</p>
          </div>
          {#if notificationEvents.length === 0}
            <div class={settingsRowLast}><p class="m-0 text-sm text-[var(--hv-settings-label)]">{$t('settings.loading_settings')}</p></div>
          {:else}
            {#each notificationEvents as ev (ev.id)}
              <div class={ev.id === notificationEvents[notificationEvents.length - 1].id ? settingsRowLast : settingsRow} data-testid="settings-event-row">
                <span class={settingsRowLabel}>{$t(ev.label_key)}</span>
                <Switch
                  checked={ev.enabled}
                  accent={activeCat.accent}
                  ariaLabel={$t(ev.label_key)}
                  data-testid={`settings-event-${ev.id}`}
                  onchange={(v: boolean) => toggleNotificationEvent(ev.id, v)}
                />
              </div>
            {/each}
          {/if}
        </div>
      {/if}
      {#if activeCategory === 'system'}
        <div data-testid="settings-card-title" class={secLabelFirst}></div>
        <div data-testid="settings-card" class={settingsCard}>
          <div class={settingsRow}>
            <label class={settingsRowLabel} for="updateFrequency">{$t('settings.check_frequency')}</label>
            <select id="updateFrequency" class={cn(settingsSelect, 'max-w-[280px]')} bind:value={draft.update.check_frequency}>
              {#each updateFrequencies as freq}<option value={freq.value}>{$t(`settings.update_${freq.value}`)}</option>{/each}
            </select>
          </div>
          <div class="settings-version-box flex flex-col gap-0.5 py-3">
            <div class="settings-version-row flex justify-between text-sm py-1"><span class="text-[var(--hv-settings-label)]">{$t('settings.current_version')}</span><span class="font-semibold text-[var(--hv-settings-title)]">v{versionInfo.current_version}</span></div>
            {#if versionInfo.latest_version}
              <div class="settings-version-row flex justify-between text-sm py-1"><span class="text-[var(--hv-settings-label)]">{$t('settings.latest_version')}</span><span class="font-semibold text-[var(--hv-settings-title)]">v{versionInfo.latest_version}{#if !versionInfo.up_to_date}<span class="ml-1 px-1.5 py-0.5 rounded bg-[var(--hv-warning-bg)] text-[var(--hv-warning)] text-micro font-bold uppercase">{$t('settings.update_available_badge')}</span>{/if}</span></div>
            {/if}
            {#if versionInfo.release_date}<div class="settings-version-row flex justify-between text-sm py-1"><span class="text-[var(--hv-settings-label)]">{$t('settings.release_date2')}</span><span class="font-semibold text-[var(--hv-settings-title)]">{versionInfo.release_date}</span></div>{/if}
            {#if !versionInfo.latest_version}<p data-testid="version-no-check-hint" class="m-0 mt-1 text-xs text-[var(--hv-settings-label)]">{$t('settings.update_auto_hint')}</p>{/if}
            {#if versionInfo.error}<div class="settings-version-row flex items-center gap-1.5 text-xs text-[var(--hv-error)]"><IconAlertTriangle size={16} /> <span>{versionInfo.error}</span></div>{/if}
          </div>
          <div data-testid="settings-card-footer" class={settingsCardFooter}>
            <button class={cn(settingsBtn, settingsBtnGhost)} onclick={handleCheckUpdate} disabled={checkingUpdate}>{checkingUpdate ? $t('settings.checking') : $t('settings.check_for_updates')}</button>
          </div>
        </div>
      {/if}
      {#if activeCategory === 'ai'}
        <div data-testid="settings-card-title" class={secLabelFirst}></div>
        <div data-testid="settings-card" class={settingsCard}>
          {#if aiLoading}
            <div class="py-3 text-sm text-[var(--hv-settings-label)]">{$t('settings.ai_loading')}</div>
          {:else}
            <div class={settingsRowStacked}>
              <label class={settingsRowLabel} for="aiModel">{$t('settings.ai_active_model')}</label>
              <div class="settings-ai-select-row flex items-center gap-2">
                <select id="aiModel" class={cn(settingsSelect, 'max-w-xs')} bind:value={draft.ai.model} disabled={aiModels.length === 0}>
                  {#each aiModels as model}<option value={model.name}>{model.name}</option>{/each}
                </select>
              </div>
              <p class={settingsRowHint}>{$t('settings.ai_current_model', { model: currentModelDisplay })}</p>
            </div>

            <div class={settingsRowStacked}>
              <div class="flex items-center gap-2 flex-wrap">
                <span class={settingsRowLabel}>{$t('settings.ai_recommended')}</span>
                {#if recommending}<span class="text-xs text-[var(--hv-settings-label)]">{$t('settings.ai_analyzing')}</span>{:else if systemSpecs}<span class="text-xs text-[var(--hv-settings-label)]">{translate('settings.ai_specs', { ram: String(systemSpecs.total_ram_gb) })}{#if systemSpecs.has_gpu} · {systemSpecs.gpu_vram_gb ? translate('settings.ai_gpu', { gpu: systemSpecs.gpu_name ?? 'GPU', vram: String(systemSpecs.gpu_vram_gb) }) : (systemSpecs.gpu_name ?? 'GPU')}{/if}</span>{/if}
              </div>
              {#if recommendations.length > 0}
                <div class="grid gap-2.5">
                  {#each recommendations as rec, i}
                    <div data-testid="settings-ai-recommendation" data-fits={rec.fits} class:settings-ai-rec-card-top={i === 0 && rec.fits} class={cn('p-3 rounded-xl bg-[var(--hv-settings-canvas)] border-[1.45px] border-[var(--hv-settings-border)]', i === 0 && rec.fits && 'border-[var(--hv-primary)]/40')}>
                      {#if i === 0 && rec.fits}<div class="inline-block px-2 py-0.5 mb-2 rounded bg-[var(--hv-primary)] text-[var(--hv-on-primary)] text-micro font-bold uppercase">{$t('settings.ai_best_choice')}</div>{/if}
                      <div class="flex items-center justify-between gap-2">
                        <span class="text-sm font-bold text-[var(--hv-settings-title-strong)]">{rec.name}</span>
                        <span class="text-xs text-[var(--hv-settings-label)]">{rec.speed}</span>
                      </div>
                      <p class="m-0 mt-1 text-xs text-[var(--hv-settings-row)]">{rec.description}</p>
                      <div class="flex gap-1.5 mt-2 flex-wrap">
                        <span class="px-1.5 py-0.5 rounded bg-[var(--hv-settings-hairline)] text-micro font-bold uppercase text-[var(--hv-settings-chip-text)]">RAM &ge; {rec.min_ram_gb} GB</span>
                        {#if rec.min_vram_gb}<span class="px-1.5 py-0.5 rounded bg-[var(--hv-settings-hairline)] text-micro font-bold uppercase text-[var(--hv-settings-chip-text)]">VRAM &ge; {rec.min_vram_gb} GB</span>{/if}
                        <span class="px-1.5 py-0.5 rounded bg-[var(--hv-settings-hairline)] text-micro font-bold uppercase text-[var(--hv-settings-chip-text)]">{rec.size_gb.toFixed(1)} GB</span>
                        {#if !rec.fits}<span class="px-1.5 py-0.5 rounded bg-[var(--hv-warning-bg)] text-micro font-bold uppercase text-[var(--hv-warning)]">{$t('settings.ai_needs_ram', { ram: String(rec.min_ram_gb) })}</span>{/if}
                      </div>
                      <div class="mt-2">
                        {#if aiModels.some(m => m.name === rec.name)}
                          <span class="inline-flex items-center gap-1 text-xs font-semibold text-[var(--hv-success)]"><IconCheck size={16} stroke={2.5} /> {$t('settings.ai_installed')}</span>
                        {:else}
                          <button class={cn(settingsBtn, settingsBtnGhost)} onclick={() => handleInstallAndLoad(rec.name)} disabled={aiPulling}>{aiPulling && newModelName === rec.name ? $t('settings.ai_installing') : $t('settings.ai_install_activate')}</button>
                        {/if}
                      </div>
                    </div>
                  {/each}
                </div>
              {:else if !recommending}
                <p class={settingsRowHint}>{$t('settings.ai_no_recommendations')}</p>
              {/if}
            </div>

            <div class={settingsRowStacked}>
              <label class={settingsRowLabel} for="newModelName">{$t('settings.ai_install_specific')}</label>
              <div class="flex gap-2">
                <input type="text" id="newModelName" class={cn(settingsInput, 'max-w-xs')} bind:value={newModelName} placeholder="gemma3:4b, qwen3.5:2b..." disabled={aiPulling} />
                <button class={cn(settingsBtn, settingsBtnGhost)} onclick={handlePullModel} disabled={aiPulling || !newModelName.trim()}>{$t('settings.ai_install')}</button>
              </div>
              <div class="flex gap-1.5 flex-wrap">
                {#each quickModels as m}<button class="settings-ai-quick-btn inline-flex items-center justify-center min-h-[var(--hv-touch-target-min)] px-2.5 py-1 rounded-lg border-[1.45px] border-[var(--hv-settings-border)] bg-[var(--hv-settings-canvas)] text-c-xs font-medium text-[var(--hv-settings-label)] cursor-pointer hover:text-[var(--hv-settings-title)]" onclick={() => newModelName = m} disabled={aiPulling}>{m}</button>{/each}
              </div>
              {#if aiPullMessage}<p class={settingsRowHint}>{aiPullMessage}</p>{/if}
              {#if aiPullError}<p class="m-0 text-xs text-[var(--hv-error)]">{aiPullError}</p>{/if}
            </div>

            <div class="settings-ai-status flex items-center gap-2 py-3 text-sm text-[var(--hv-settings-label)]">
              <span class="settings-ai-status-dot {cn('w-2 h-2 rounded-full', ollamaRunning ? 'bg-[var(--hv-success)]' : 'bg-[var(--hv-error)]')}"></span>
              <span>{ollamaRunning ? $t('settings.ai_ollama_running') : $t('settings.ai_ollama_down')}</span>
              <button class="settings-ai-refresh-btn ml-auto grid place-items-center w-[var(--hv-touch-target-min)] h-[var(--hv-touch-target-min)] rounded-full border-none bg-transparent text-[var(--hv-settings-label)] cursor-pointer hover:text-[var(--hv-settings-title)]" onclick={() => { loadAiModels(); loadRecommendations(); }} aria-label={$t('settings.ai_refresh')} title={$t('settings.ai_refresh')}><IconRefresh size={20} /></button>
            </div>
          {/if}
        </div>
        <!-- Knowledge base (RAG). The chat's grounding in the shipped docs:
             shows what is indexed and lets an admin install the embedding
             model + reindex. Right under the model card, since the two are the
             same feature (the assistant needs both a chat model and an
             embedding model). -->
        <KnowledgeBaseCard class="mt-3" />
        <!-- External agents (MCP gateway). Lives here, not in the Chat app:
             it is a server-wide admin configuration (kill-switch, global
             permissions, revocable tokens) sitting next to the model choice.
             The card probes `/api/mcp/status` and renders nothing for a
             non-admin session. -->
        <McpCard class="mt-3" />
      {/if}
      {#if activeCategory === 'monitoring'}
        <UpsCard class="mt-6" />
        <SnmpCard class="mt-3" />
      {/if}
      {#if activeCategory === 'widgets'}
        <div data-testid="settings-card-title" class={secLabelFirst}></div>
        <div data-testid="settings-card" class={settingsCard}>
          {#each WIDGET_DEFS as def (def.id)}
            <div class={cn(settingsRow, 'settings-widget-row')} data-widget={def.id}>
              <div class="flex items-center gap-3 min-w-0">
                <span
                  class={settingsIconTile}
                  style="background: color-mix(in srgb, {def.color} var(--hv-settings-cat-tint), transparent); color: {def.color}"
                >
                  <svelte:component this={def.icon} size={16} />
                </span>
                <div class="min-w-0">
                  <span class={settingsRowLabel}>{$t(def.nameKey)}</span>
                  <p class={settingsRowHint}>{$t(def.descKey)}</p>
                </div>
              </div>
              <Switch
                checked={draft.widgets.visible[def.visibleKey]}
                onchange={(v) => (draft.widgets.visible[def.visibleKey] = v)}
                accent={def.color}
                ariaLabel={$t(def.nameKey)}
              />
            </div>
          {/each}
        </div>

        <div data-testid="settings-card-title" class={secLabel}></div>
        <div data-testid="settings-card" class={settingsCard}>
          <div class={cn(settingsRow, 'last:border-b-0')}>
            <div class="min-w-0">
              <label class={settingsRowLabel} for="refreshInterval">{$t('settings.refresh_interval_label')}</label>
              <p class={settingsRowHint}>{$t('settings.refresh_section_subtitle')}</p>
            </div>
            <select
              id="refreshInterval"
              class={cn(settingsSelectCompact, 'settings-refresh-select')}
              value={String(draft.widgets.refreshMs)}
              onchange={(e) => (draft.widgets.refreshMs = Number((e.currentTarget as HTMLSelectElement).value))}
            >
              {#each REFRESH_INTERVAL_OPTIONS as ms}
                <option value={String(ms)}>{$t(`settings.refresh_interval_${ms / 1000}s`)}</option>
              {/each}
            </select>
          </div>
        </div>

        <div data-testid="settings-card-title" class={secLabel}></div>
        <div data-testid="settings-card" class={settingsCard}>
          {#if draft.weather.location && !weatherEditingLocation}
            <div class={cn(settingsRow, 'settings-weather-current')}>
              <div class="flex items-center gap-3 min-w-0">
                <IconMapPin size={16} class="flex-none text-[var(--hv-settings-label)]" />
                <div class="min-w-0">
                  <span class={settingsRowHint}>{$t('settings.weather_current_location')}</span>
                  <span class="block text-sm font-semibold text-[var(--hv-settings-title)]">{draft.weather.location.name}</span>
                </div>
              </div>
              <button class={cn(settingsBtn, settingsBtnGhost)} onclick={openWeatherLocationEditor}><IconPencil size={16} /> {$t('desktop.weather_change_location')}</button>
            </div>
          {:else}
            <div class={cn(settingsRow, 'settings-weather-search-row')}>
              <input class={cn(settingsInput, 'flex-1')} type="text" placeholder={$t('desktop.weather_search_placeholder')} bind:value={weatherSearchQuery} onkeydown={(e) => { if (e.key === 'Enter') searchWeatherLocation(); }} />
              <button class={cn(settingsBtn, settingsBtnPrimary, 'flex-none')} onclick={searchWeatherLocation} disabled={weatherSearching}>{weatherSearching ? $t('desktop.weather_searching') : $t('desktop.weather_search_button')}</button>
            </div>
            {#if weatherSearchResults.length > 0}
              <div class="flex flex-col gap-1 py-3 border-b-[1.45px] border-[var(--hv-settings-hairline)]">
                {#each weatherSearchResults as result (result.name + result.lat)}
                  <button class="settings-weather-result-btn flex items-center gap-1.5 px-3 min-h-[var(--hv-touch-target-min)] rounded-lg text-left text-sm text-[var(--hv-settings-title)] cursor-pointer hover:bg-[var(--hv-settings-hairline)]" onclick={() => chooseWeatherLocation(result)}><IconMapPin size={16} /> {result.name}</button>
                {/each}
              </div>
            {:else if !draft.weather.location && !weatherSearching}
              <div class="py-3 border-b-[1.45px] border-[var(--hv-settings-hairline)]">
                <p class={cn(settingsHint, 'm-0')}>{$t('desktop.weather_no_location')}</p>
              </div>
            {/if}
          {/if}
          {#if draft.weather.location && weatherEditingLocation}
            <div class={cn(settingsRowLast, 'border-b-[1.45px] border-[var(--hv-settings-hairline)]')}>
              <button class={cn(settingsBtn, settingsBtnGhost)} onclick={() => { weatherEditingLocation = false; weatherSearchQuery = ''; weatherSearchResults = []; }}>{$t('settings.weather_cancel')}</button>
            </div>
          {/if}
          <div class={cn(settingsRowStacked, 'settings-weather-fields')}>
            <div class="min-w-0">
              <span class={settingsRowLabel}>{$t('settings.weather_display_title')}</span>
              <p class={settingsRowHint}>{$t('settings.weather_display_subtitle')}</p>
            </div>
            <div class="flex flex-wrap gap-x-4 gap-y-1">
              {#each WEATHER_FIELDS as field}
                <Switch checked={draft.weather.displayFields.includes(field)} onchange={(v) => handleWeatherFieldToggle(field, v)} label={$t(`desktop.weather_${field}`)} accent={activeCat.accent} />
              {/each}
            </div>
          </div>
        </div>
      {/if}
      {#if activeCategory === 'system'}
        <div data-testid="settings-card-title" class={secLabel}></div>
        <div data-testid="settings-card" class={settingsCard}>
          <div class="settings-info-row flex items-center justify-between gap-4 py-3 border-b-[1.45px] border-[var(--hv-settings-hairline)]">
            <span class={settingsRowLabel}>{$t('settings.server_address')}</span>
            <span class="text-sm font-semibold text-[var(--hv-settings-title-strong)]">{serverInfo.display_address || `${serverInfo.ip}:${serverInfo.port}`}</span>
          </div>
          <div class="settings-info-row flex items-center justify-between gap-4 py-3 border-b-[1.45px] border-[var(--hv-settings-hairline)]">
            <span class={settingsRowLabel}>{$t('settings.platform')}</span>
            <span class="text-sm font-semibold text-[var(--hv-settings-title-strong)]">Linux x64</span>
          </div>
          <div class="settings-info-row flex items-center justify-between gap-4 py-3">
            <span class={settingsRowLabel}>{$t('settings.version_label')}</span>
            <span class="text-sm font-semibold text-[var(--hv-settings-title-strong)]">HomeVault v{versionInfo.current_version}</span>
          </div>
        </div>
      {/if}
      {#if activeCategory === 'system'}
        <div data-testid="settings-card-title" class={secLabel}></div>
        <div data-testid="settings-card" class={settingsCard}>
          <div class={settingsRowStacked}>
            <span class={settingsRowLabel}>{$t('settings.backup_restore')}</span>
            <!-- Order is load-bearing: E2E 14.1/14.2 address these by index
                 (export = first, paste/file toggle = second). -->
            <div class="settings-data-actions flex gap-2 flex-wrap">
              <button class={cn(settingsBtn, settingsBtnGhost)} onclick={handleExportSettings} disabled={exportingSettings}>{exportingSettings ? $t('settings.exporting') : $t('settings.export_settings')}</button>
              <button class={cn(settingsBtn, settingsBtnGhost)} onclick={() => (importingFromFile = !importingFromFile)}>{importingFromFile ? $t('settings.paste_json_instead') : $t('settings.import_from_file')}</button>
              <button class={cn(settingsBtn, settingsBtnDanger)} onclick={handleResetSettings} disabled={resettingSettings}>{resettingSettings ? $t('settings.resetting') : $t('settings.reset_to_defaults')}</button>
            </div>
          </div>
          {#if importingFromFile}
            <div class={cn(settingsRowStacked, 'settings-field-full')}>
              <label class={settingsRowLabel} for="importFile">{$t('settings.settings_file')}</label>
              <input id="importFile" type="file" accept="application/json,.json" bind:this={importFileInput} class={cn(settingsInput, 'h-auto py-1.5')} />
            </div>
          {:else}
            <div class={cn(settingsRowStacked, 'settings-field-full')}>
              <label class={settingsRowLabel} for="importText">{$t('settings.paste_settings_json')}</label>
              <textarea id="importText" class={settingsTextarea} bind:value={importText} placeholder={$t('settings.paste_settings_placeholder')}></textarea>
            </div>
          {/if}
          <div class={cn(settingsRowLast, 'settings-field-full')}>
            <button class={cn(settingsBtn, settingsBtnPrimary)} onclick={handleImportSettings} disabled={importingSettings}>{importingSettings ? $t('settings.importing') : $t('settings.import_settings')}</button>
          </div>
        </div>
      {/if}
      {#if activeCategory === 'system'}
        <!-- Database snapshots. The database runs in WAL mode, so copying
             `homevault.db` alone yields a file with no tables; these are
             `VACUUM INTO` snapshots, self-contained and restorable. -->
        <div data-testid="settings-card-title" class={secLabel}></div>
        <div data-testid="settings-card" class={settingsCard}>
          <div class={settingsRowStacked}>
            <span class={settingsRowLabel}>{$t('settings.db_backup_title')}</span>
            <p class={settingsRowHint}>{$t('settings.db_backup_hint')}</p>
          </div>

          {#if dbBackupLoading}
            <div class="py-3 text-sm text-[var(--hv-settings-label)]">{$t('settings.db_backup_loading')}</div>
          {:else}
            <div class={settingsRow}>
              <span class={settingsRowLabel}>{$t('settings.db_backup_last')}</span>
              <span class="text-sm font-semibold text-[var(--hv-settings-title)]" data-testid="db-backup-last">
                {dbBackup?.last_backup_at ? formatBackupDate(dbBackup.last_backup_at) : $t('settings.db_backup_never')}
              </span>
            </div>

            <!-- A failed attempt must not be indistinguishable from a missing
                 one: the reason the server recorded is shown until the next
                 success clears it (ROADMAP 22.2). -->
            {#if dbBackup?.last_error}
              <div class={cn(settingsRow, 'settings-field-full')}>
                <p class="m-0 flex items-center gap-1.5 text-xs text-[var(--hv-error)]" role="alert" data-testid="db-backup-last-error">
                  <IconAlertTriangle size={16} />
                  <span>{$t('settings.db_backup_last_error', { error: dbBackup.last_error })}</span>
                </p>
              </div>
            {/if}

            <div class={settingsRow}>
              <label class={settingsRowLabel} for="dbBackupInterval">{$t('settings.db_backup_interval')}</label>
              <select id="dbBackupInterval" class={cn(settingsSelect, 'max-w-[200px]')} bind:value={dbBackupInterval} disabled={dbBackupBusy}>
                {#each [0, 6, 12, 24, 48, 168] as hours}
                  <option value={hours}>{hours === 0 ? $t('settings.db_backup_interval_off') : $t('settings.db_backup_interval_hours', { hours: String(hours) })}</option>
                {/each}
              </select>
            </div>

            <div class={settingsRow}>
              <label class={settingsRowLabel} for="dbBackupRetain">{$t('settings.db_backup_retain')}</label>
              <input id="dbBackupRetain" type="number" min="1" max="100" class={cn(settingsInput, 'max-w-[120px]')} bind:value={dbBackupRetain} disabled={dbBackupBusy} />
            </div>

            <div class={cn(settingsRowStacked, 'settings-field-full')}>
              <span class={settingsRowLabel}>{$t('settings.db_backup_snapshots', { count: String(dbBackup?.backups?.length ?? 0), size: formatBytes(dbBackup?.total_size_bytes ?? 0) })}</span>
              {#if dbBackup && dbBackup.backups.length > 0}
                <ul class="m-0 mt-1 flex list-none flex-col gap-1 p-0" data-testid="db-backup-list">
                  {#each dbBackup.backups as entry (entry.name)}
                    <li class="flex items-center justify-between gap-3 rounded-md border border-[var(--hv-settings-hairline)] px-3 py-1.5 text-xs">
                      <span class="min-w-0 flex-1 truncate font-[family-name:var(--hv-font-mono)] text-[var(--hv-settings-title)]">{entry.name}</span>
                      {#if entry.is_pre_migration}
                        <span class="shrink-0 rounded-full border border-[var(--hv-settings-hairline)] px-2 py-0.5 text-[var(--hv-settings-label)]">
                          {$t('settings.db_backup_pre_migration')}
                        </span>
                      {/if}
                      <span class="shrink-0 tabular-nums text-[var(--hv-settings-label)]">{formatBytes(entry.size_bytes)}</span>
                    </li>
                  {/each}
                </ul>
              {:else}
                <p class={settingsRowHint}>{$t('settings.db_backup_empty')}</p>
              {/if}
            </div>

            {#if dbBackupError}
              <div class={cn(settingsRow, 'settings-field-full')}>
                <p class="m-0 flex items-center gap-1.5 text-xs text-[var(--hv-error)]" role="alert" data-testid="db-backup-error">
                  <IconAlertTriangle size={16} /> <span>{dbBackupError}</span>
                </p>
              </div>
            {/if}

            <div class={cn(settingsCardFooter, 'flex gap-2 flex-wrap')}>
              <button class={cn(settingsBtn, settingsBtnPrimary)} onclick={handleBackupNow} disabled={dbBackupBusy} data-testid="db-backup-now">
                {dbBackupBusy ? $t('settings.db_backup_working') : $t('settings.db_backup_now')}
              </button>
              <button class={cn(settingsBtn, settingsBtnGhost)} onclick={handleSaveDbBackup} disabled={dbBackupBusy} data-testid="db-backup-save">
                {$t('settings.db_backup_save')}
              </button>
            </div>
          {/if}
        </div>
      {/if}

      {#if activeCategory === 'system'}
        <!-- Host power management (admin actions, confirmed) -->
        <div data-testid="settings-card-title" class={secLabelFirst}></div>
        <div data-testid="settings-card" class={settingsCard}>
          <div class={settingsRowStacked}>
            <span class={settingsRowLabel}>{$t('power.governor_title')}</span>
            <p class={settingsRowHint}>{$t('power.governor_desc')}</p>
          </div>
          {#if powerGovernorSupported}
            <div class={settingsRow} data-testid="power-governor-row">
              <select
                class={cn(settingsSelect, 'max-w-[280px]')}
                bind:value={powerGovernor}
                aria-label={$t('power.governor_title')}
                data-testid="power-governor-select"
                disabled={powerBusy}
              >
                {#each powerGovernors as gov (gov)}<option value={gov}>{gov}</option>{/each}
              </select>
              <button class={cn(settingsBtn, settingsBtnGhost)} onclick={saveGovernor} disabled={powerBusy}>{$t('common.save')}</button>
            </div>
          {:else}
            <div class={settingsRowLast}><p class="m-0 text-sm text-[var(--hv-settings-label)]">{$t('power.governor_unsupported')}</p></div>
          {/if}
          <div class={settingsRowStacked}>
            <span class={settingsRowLabel}>{$t('power.actions_title')}</span>
            <p class={settingsRowHint}>{$t('power.actions_desc')}</p>
            <div class="flex flex-wrap gap-2 mt-2" data-testid="power-actions">
              <button class={cn(settingsBtn, settingsBtnGhost)} data-testid="power-reboot" onclick={() => hostPower('reboot')} disabled={powerBusy}>{$t('power.reboot')}</button>
              <button class={cn(settingsBtn, settingsBtnDanger)} data-testid="power-shutdown" onclick={() => hostPower('shutdown')} disabled={powerBusy}>{$t('power.shutdown')}</button>
              <button class={cn(settingsBtn, settingsBtnGhost)} data-testid="power-suspend" onclick={() => hostPower('suspend')} disabled={powerBusy}>{$t('power.suspend')}</button>
            </div>
          </div>
        </div>
        <!-- Disk spindown (hdparm -B / -S), persisted per disk -->
        <div data-testid="settings-card-title" class={secLabel}></div>
        <div data-testid="settings-card" class={settingsCard}>
          <div class={settingsRowStacked}>
            <span class={settingsRowLabel}>{$t('power.spindown_title')}</span>
            <p class={settingsRowHint}>{$t('power.spindown_desc')}</p>
          </div>
          {#if !powerSpindownSupported}
            <div class={settingsRowLast}><p class="m-0 text-sm text-[var(--hv-settings-label)]">{$t('power.governor_unsupported')}</p></div>
          {:else}
            {#if powerSpindown.length > 0}
              {#each powerSpindown as sd (sd.device)}
                <div class={settingsRow} data-testid="power-spindown-row">
                  <span class={settingsRowLabel} class:font-mono={true}>{sd.device}</span>
                  <div class="flex items-center gap-2">
                    <span class="text-xs text-[var(--hv-settings-label)]">
                      APM {sd.apm_level ?? '—'} · {sd.spindown_minutes === 0 ? $t('power.never') : sd.spindown_minutes ? `${sd.spindown_minutes} min` : '—'}
                    </span>
                    <button class={cn(settingsBtn, settingsBtnGhost)} data-testid="power-spindown-remove-{sd.device}" onclick={() => removeSpindown(sd.device)} disabled={powerBusy}>{$t('power.remove')}</button>
                  </div>
                </div>
              {/each}
            {/if}
            <div class={settingsRowLast} data-testid="power-spindown-add">
              <div class="flex flex-wrap items-end gap-2 w-full">
                <label class="flex flex-col gap-1 text-c-label text-[var(--hv-settings-label)]">
                  <span>{$t('power.device')}</span>
                  <input class={cn(settingsInput, 'w-32')} type="text" bind:value={newSpindown.device} placeholder="sda" aria-label={$t('power.device')} />
                </label>
                <label class="flex flex-col gap-1 text-c-label text-[var(--hv-settings-label)]">
                  <span>{$t('power.apm')}</span>
                  <input class={cn(settingsInput, 'w-24')} type="number" min="1" max="255" bind:value={newSpindown.apm_level} aria-label={$t('power.apm')} />
                </label>
                <label class="flex flex-col gap-1 text-c-label text-[var(--hv-settings-label)]">
                  <span>{$t('power.minutes')}</span>
                  <input class={cn(settingsInput, 'w-24')} type="number" min="0" max="330" bind:value={newSpindown.spindown_minutes} aria-label={$t('power.minutes')} />
                </label>
                <button class={cn(settingsBtn, settingsBtnPrimary)} onclick={addSpindown} disabled={powerBusy}>{$t('power.add')}</button>
                <button class={cn(settingsBtn, settingsBtnGhost)} onclick={applyAllSpindown} disabled={powerBusy}>{$t('power.apply')}</button>
              </div>
            </div>
          {/if}
        </div>
        <!-- Privacy (existing) -->
        <div data-testid="settings-card-title" class={secLabel}></div>
        <div data-testid="settings-card" class={settingsCard}>
          <div class="settings-privacy-feature flex items-start gap-3 py-3 border-b-[1.45px] border-[var(--hv-settings-hairline)]">
            <span class={cn(settingsIconTile, 'bg-[var(--hv-primary-light)] text-[var(--hv-primary)]')}><IconShield size={16} /></span>
            <div><strong class="block text-sm text-[var(--hv-settings-title-strong)]">{$t('settings.privacy_local')}</strong><p class="m-0 text-xs text-[var(--hv-settings-label)]">{$t('settings.privacy_local_desc')}</p></div>
          </div>
          <div class="settings-privacy-feature flex items-start gap-3 py-3 border-b-[1.45px] border-[var(--hv-settings-hairline)]">
            <span class={cn(settingsIconTile, 'bg-[var(--hv-warning-bg)] text-[var(--hv-warning)]')}><IconCookie size={16} /></span>
            <div><strong class="block text-sm text-[var(--hv-settings-title-strong)]">{$t('settings.privacy_cookies')}</strong><p class="m-0 text-xs text-[var(--hv-settings-label)]">{$t('settings.privacy_cookies_desc')}</p></div>
          </div>
          <div class="settings-privacy-feature flex items-start gap-3 py-3">
            <span class={cn(settingsIconTile, 'bg-[var(--hv-success-bg)] text-[var(--hv-success)]')}><IconWifiOff size={16} /></span>
            <div><strong class="block text-sm text-[var(--hv-settings-title-strong)]">{$t('settings.privacy_offline')}</strong><p class="m-0 text-xs text-[var(--hv-settings-label)]">{$t('settings.privacy_offline_desc')}</p></div>
          </div>
        </div>
      {/if}
    </div>
  {/if}
  <ConfirmDialog
    {dlg}
    onconfirm={() => dlg.settle(true)}
    oncancel={() => dlg.settle(false)}
  />
</PageLayout>

<style>
  /* The panel loader. The save pip now lives in the SettingsSaveButton
     component (colour + label state machine), with its own CSS. */
  .set-spinner {
    display: inline-block;
    flex: none;
    width: 20px;
    height: 20px;
    border-radius: var(--hv-radius-full);
    border: 2px solid var(--hv-settings-border);
    border-top-color: var(--hv-primary);
    animation: set-spin 700ms linear infinite;
  }

  @keyframes set-spin {
    to {
      transform: rotate(360deg);
    }
  }

  /* Alert-threshold slider: the 44px input is the touch target; the track is
     a slim hairline and the thumb is ≥24px (WCAG 2.5.8). `accent-color` can't
     size a native thumb, so the track/thumb are drawn explicitly. */
  .settings-alert-range {
    appearance: none;
    background: transparent;
  }
  .settings-alert-range::-webkit-slider-runnable-track {
    height: 4px;
    border-radius: var(--hv-radius-full);
    background: var(--hv-settings-hairline);
  }
  .settings-alert-range::-webkit-slider-thumb {
    appearance: none;
    width: 24px;
    height: 24px;
    margin-top: calc(var(--hv-space-2-5) * -1);
    border: none;
    border-radius: var(--hv-radius-full);
    background: var(--hv-primary);
  }
  .settings-alert-range::-moz-range-track {
    height: 4px;
    border-radius: var(--hv-radius-full);
    background: var(--hv-settings-hairline);
  }
  .settings-alert-range::-moz-range-thumb {
    width: 24px;
    height: 24px;
    border: none;
    border-radius: var(--hv-radius-full);
    background: var(--hv-primary);
  }

  @media (prefers-reduced-motion: reduce) {
    .set-spinner {
      animation-duration: 2s;
    }
  }
</style>
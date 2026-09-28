<script lang="ts">
  import { authStore } from '$lib/stores/auth';
  import { api } from '$lib/services/api';
  import { windowManager } from '$lib/stores/windowManager';
  import { wallpaperStore } from '$lib/stores/wallpaper';
  import WindowFrame from '$lib/components/desktop/WindowFrame.svelte';
  import { onMount } from 'svelte';
  import { get } from 'svelte/store';
  import SearchBar from '$lib/components/search/SearchBar.svelte';
  import { openSearch, type SearchableItem } from '$lib/stores/search';
  import NotificationCenter from '$lib/components/NotificationCenter.svelte';
  import NotificationBellIcon from '$lib/components/shared/NotificationBellIcon.svelte';
  import WifiIcon from '$lib/components/shared/WifiIcon.svelte';
  import EthernetIcon from '$lib/components/shared/EthernetIcon.svelte';
  import ProfileIcon from '$lib/components/shared/ProfileIcon.svelte';
  import UserAvatar from '$lib/components/theme/UserAvatar.svelte';
  import { websocket } from '$lib/services/websocket';
  import { notificationsStore } from '$lib/stores/notifications';
  import LazyLoad from '$lib/components/LazyLoad.svelte';
  import { IconServer, IconSettings as SettingsIcon, IconMoon, IconUser, IconActivity, IconCpu, IconThermometer, IconArrowUp, IconArrowDown, IconDatabase, IconPackage, IconTerminal as TerminalIcon, IconPower, IconShieldCheckered, IconLock, IconHome, IconLayoutGrid, IconMenu, IconSearch, IconPlus, IconDownload, IconUpload, IconTrash, IconPencil, IconCheck, IconAlertCircle, IconInfoCircle, IconAlertTriangle, IconBug, IconShield, IconRefresh, IconCalendar, IconChevronLeft, IconChevronRight, IconChevronsLeft, IconChevronsRight, IconPlayerPlay, IconSquare, IconContainer as ContainerIcon, IconSun, IconSun as Sun, IconCloud, IconCloudRain, IconSnowflake, IconCloudStorm, IconMapPin, IconUsers, IconHeartbeat, IconLogout, IconRobot, IconGlobe, IconSignal3g, IconCopy, IconCloudFog } from '@tabler/icons-svelte';
  import { subscribeWidget, type WidgetLoadResult } from '$lib/components/desktop/widgetData';
  import { launchpadIcons } from '$lib/assets/launchpadIcons';
  import HvLaunchpad from '$lib/components/desktop/HvLaunchpad.svelte';
  import WidgetHeader from '$lib/components/desktop/WidgetHeader.svelte';
  import { monitorStore, DEFAULT_REFRESH_MS, type MonitorPositions } from '$lib/stores/monitorStore';
  import { weatherLocationStore } from '$lib/stores/weatherLocation';
  import { weatherDisplayPrefsStore } from '$lib/stores/weatherDisplayPrefs';
  import Icon from '$lib/components/theme/Icon.svelte';
  import { applySnap } from '$lib/utils/snapUtils';
  import { focusTrap } from '$lib/utils/focusTrap';
  import { t, translate, languageStore } from '$lib/stores/i18n';
  import { activity } from '$lib/stores/activityStore';

  // Lazy load app components
  const Dashboard = () => import('./Dashboard.svelte');
  const Account = () => import('./Account.svelte');
  const Terminal = () => import('./Terminal.svelte');
  const Settings = () => import('./Settings.svelte');
  const Users = () => import('./Users.svelte');
  const Raid = () => import('./Raid.svelte');
  const Docker = () => import('./Docker.svelte');
  const Network = () => import('./Network.svelte');
  const Restic = () => import('./Restic.svelte');
  const Shares = () => import('./Shares.svelte');
  const Explorer = () => import('./Explorer.svelte');
  const Groups = () => import('./Groups.svelte');
  const Quotas = () => import('./Quotas.svelte');
  const Health = () => import('./Health.svelte');
  const CloudSync = () => import('./CloudSync.svelte');
  const Syncthing = () => import('./Syncthing.svelte');
  const Chat = () => import('./Chat.svelte');
  const Downloader = () => import('./Downloader.svelte');
  const Rescue = () => import('./Rescue.svelte');
  const Storage = () => import('./Storage.svelte');
  const VPN = () => import('./VPN.svelte');
  const Widgets = () => import('./Widgets.svelte');
  const Journal = () => import('./Journal.svelte');
  const VirtualMachines = () => import('./VirtualMachines.svelte');
  let currentTime = new Date();
  let now = Date.now();
  let showUserMenu = false;
  let showAppsCenter = false;
  let showActivityPanel = false;
  let dockWidth = 0;
  let showNotifications = false;
  let connectionType = '';
  let showConnectionInfo = false;
  let connLocalIp = '';
  let connPublicIp = '';
  let connIpLoading = false;
  let connIpLoaded = false;
  let showPowerModal = false;
  let logoutAction: 'logout' | 'restart' | 'shutdown' | null = null;
  let systemUpdateAvailable = false;

  // ── Desktop chrome effects (dock + topbar) ──────────────────────────────
  // All effect logic below is deliberately cheap: discrete-event listeners
  // (never a permanent mousemove), one short poll gated on tab visibility,
  // rAF-throttled work only while the pointer is actually over the dock, and
  // transform/opacity-only CSS. Everything degrades cleanly when the OS
  // asks for reduced motion.
  const prefersReducedMotion =
    typeof window !== 'undefined' &&
    typeof window.matchMedia === 'function' &&
    window.matchMedia('(prefers-reduced-motion: reduce)').matches;

  // Idle dim — after this long with no discrete input, the dock + topbar
  // fade back so they stop drawing the eye (and stop burning static pixels
  // on OLED panels). Any click / key / wheel restores them.
  const IDLE_DIM_MS = 90_000;
  // Held in a plain object, not a reactive `let`, so the pointermove handler
  // that touches it every frame doesn't schedule a component update each time.
  const activityState = { at: Date.now() };
  let chromeIdle = false;

  // Background-activity feed — the shared `activityStore` polls `/api/activity`
  // (PROGRESS_MANAGER jobs + aria2 downloads) while this component is mounted.
  // Drives both the thin indeterminate strip on the topbar pill and the dock
  // activity button + taskbar panel above the dock.
  $: activityTasks = $activity.tasks;
  $: activityCount = $activity.count;
  $: activeJobCount = $activity.count;
  // Resolve each row's display name: dynamic label (torrent / file) if the
  // server gave one, else the localised `kind`, else the raw slug. Re-runs on
  // task change AND on language change (both `$t` and `activityTasks` are deps).
  $: localizedActivityTasks = activityTasks.map((task) => {
    const key = `desktop.activity_kind_${task.kind}`;
    const v = $t(key);
    return { ...task, displayName: task.label || (v === key ? '' : v) || task.kind };
  });
  // Close the taskbar panel automatically once nothing is running, or when the
  // dock itself slides away (maximized window / phone chrome hidden).
  $: if (showActivityPanel && (activityCount === 0 || dockHidden)) showActivityPanel = false;

  function toggleActivityPanel() {
    showActivityPanel = !showActivityPanel;
    if (showActivityPanel) {
      showAppsCenter = false;
      showUserMenu = false;
      showConnectionInfo = false;
      showNotifications = false;
    }
  }

  // Gates the "reacted to a change" one-shots (bell swing, attention bounce)
  // so the first burst of data arriving after login doesn't set them off.
  let chromeReady = false;

  // Notification bell — a single one-shot swing whenever the unread count
  // climbs, replacing the old always-running badge pulse.
  let bellSwing = false;
  let prevUnread = 0;
  $: onUnreadCountChange($notificationsStore.unreadCount);
  function onUnreadCountChange(n: number) {
    if (chromeReady && n > prevUnread && !prefersReducedMotion) {
      bellSwing = true;
      setTimeout(() => (bellSwing = false), 620);
    }
    prevUnread = n;
  }

  // NOTE: the dock badge counts, the attention nudge and the auto-hide
  // logic live further down (after dockerData / healthData / viewportWidth
  // are declared) — see "Dock chrome — badges / attention / auto-hide".

  // How many non-minimized windows an app has open — >1 widens the running
  // dot into a short bar.
  function openWindowCount(appId: string): number {
    return $windowManager.filter((w) => w.appId === appId && !w.isMinimized).length;
  }

  // Whether an app has ANY window open, minimized or not — drives the dock
  // running dot (`.hv-dock-dot`).
  function hasOpenWindow(appId: string): boolean {
    return $windowManager.some((w) => w.appId === appId);
  }

  // Bound for the auto-hide peek `:hover` check below. The dock hover
  // grow/lift is pure CSS (`.hv-dock-item:hover`) — one icon at a time,
  // neighbours untouched.
  let dockEl: HTMLElement | null = null;
  let menubarEl: HTMLElement | null = null;

  function markActivity() {
    activityState.at = Date.now();
    if (chromeIdle) chromeIdle = false;
  }

  // Monitor drag state
  type MonitorKind = 'resource' | 'raid' | 'security' | 'vpn' | 'downloader' | 'syncthing' | 'calendar' | 'docker' | 'weather' | 'sessions' | 'health';
  let draggingMonitor: MonitorKind | null = null;
  let monitorDragStartX = 0;
  let monitorDragStartY = 0;
  let monitorWidgetStartX = 0;
  let monitorWidgetStartY = 0;
  let monitorCurrentX = 0;
  let monitorCurrentY = 0;
  let monitorSnapEdge: 'top' | 'bottom' | 'left' | 'right' | 'none' = 'none';

  // Tracked so the resource/RAID monitor widgets below can be re-clamped
  // into view on resize — a position saved while dragging on a wide
  // desktop screen otherwise renders fully outside a phone/tablet
  // viewport on next load, with no scrollbar or drag handle reachable to
  // bring it back. Only affects where the widgets are *displayed*, never
  // written back to monitorStore, so the saved position is unchanged and
  // the widget returns to its original spot once the window is wide again.
  let viewportWidth = typeof window !== 'undefined' ? window.innerWidth : 1920;
  let viewportHeight = typeof window !== 'undefined' ? window.innerHeight : 1080;
  $: resourceMonitorClamped = (() => {
    void viewportWidth; void viewportHeight;
    return applySnap($monitorStore.resourceMonitor.x, $monitorStore.resourceMonitor.y, 280, 400);
  })();
  $: raidMonitorClamped = (() => {
    void viewportWidth; void viewportHeight;
    return applySnap($monitorStore.raidMonitor.x, $monitorStore.raidMonitor.y, 320, 400);
  })();
  $: securityMonitorClamped = (() => {
    void viewportWidth; void viewportHeight;
    return applySnap($monitorStore.securityMonitor.x, $monitorStore.securityMonitor.y, 300, 400);
  })();
  // These three panels are much shorter than Resource/RAID/Security (a
  // header plus 1-2 compact rows), so they get their own, smaller height
  // estimate for applySnap's viewport-clamping math — passing the same
  // ~400px used for the taller panels made maxY overly conservative and
  // clamped both this panel and a sibling default position down to the
  // exact same spot on a typical-height viewport, rendering them stacked
  // on top of each other instead of at their own distinct default y.
  $: vpnMonitorClamped = (() => {
    void viewportWidth; void viewportHeight;
    return applySnap($monitorStore.vpnMonitor.x, $monitorStore.vpnMonitor.y, 280, 180);
  })();
  $: downloaderMonitorClamped = (() => {
    void viewportWidth; void viewportHeight;
    return applySnap($monitorStore.downloaderMonitor.x, $monitorStore.downloaderMonitor.y, 280, 190);
  })();
  $: syncthingMonitorClamped = (() => {
    void viewportWidth; void viewportHeight;
    return applySnap($monitorStore.syncthingMonitor.x, $monitorStore.syncthingMonitor.y, 280, 150);
  })();
  $: calendarMonitorClamped = (() => {
    void viewportWidth; void viewportHeight;
    return applySnap($monitorStore.calendarMonitor.x, $monitorStore.calendarMonitor.y, 300, 320);
  })();
  $: dockerMonitorClamped = (() => {
    void viewportWidth; void viewportHeight;
    return applySnap($monitorStore.dockerMonitor.x, $monitorStore.dockerMonitor.y, 300, 320);
  })();
  $: weatherMonitorClamped = (() => {
    void viewportWidth; void viewportHeight;
    return applySnap($monitorStore.weatherMonitor.x, $monitorStore.weatherMonitor.y, 280, 220);
  })();
  $: sessionsMonitorClamped = (() => {
    void viewportWidth; void viewportHeight;
    return applySnap($monitorStore.sessionsMonitor.x, $monitorStore.sessionsMonitor.y, 280, 150);
  })();
  $: healthMonitorClamped = (() => {
    void viewportWidth; void viewportHeight;
    return applySnap($monitorStore.healthMonitor.x, $monitorStore.healthMonitor.y, 280, 190);
  })();

  // Resource monitor data
  type CpuData = { usage: number };
  type RamData = { usage_percent: number };
  type TempData = { temp: number };
  type NetworkData = { rx_mbps: number; tx_mbps: number };

  let cpuData: WidgetLoadResult<CpuData> = { status: 'idle' };
  let ramData: WidgetLoadResult<RamData> = { status: 'idle' };
  let tempData: WidgetLoadResult<TempData> = { status: 'idle' };
  let networkData: WidgetLoadResult<NetworkData> = { status: 'idle' };

  // RAID monitor data
  type RaidArray = {
    name: string;
    level: string;
    status: 'healthy' | 'degraded' | 'failed';
    devices: number;
    size: string;
    used: string;
    free: string;
    used_percent: number;
    free_percent: number;
  };
  type RaidData = {
    arrays: RaidArray[];
    total_capacity: string;
    total_used: string;
    total_free: string;
  };

  let raidData: WidgetLoadResult<RaidData> = { status: 'idle' };
  // Translate raw backend RaidArray[] to the frontend RaidData shape the template expects.
  // Backend returns: { name: "/dev/md0", level: "raid5", state: "clean", devices: ["/dev/sda1", ...], size: bytes, used: bytes }
  // Frontend expects: { arrays: [{ name, level: "5", status, devices, size, used, free, used_percent, free_percent }], total_capacity, total_used, total_free }
  function translateRaidData(raw: any): RaidData {
    if (!Array.isArray(raw)) {
      if (raw && Array.isArray(raw.arrays)) {
        raw = raw.arrays;
      } else {
        return { arrays: [], total_capacity: '0 B', total_used: '0 B', total_free: '0 B' };
      }
    }
    const arrays: RaidArray[] = raw.map((r: any) => {
      const size = r.size || r.size_bytes || 0;
      const used = r.used || r.used_bytes || 0;
      const free = Math.max(0, size - used);
      const usedPercent = size > 0 ? Math.round((used / size) * 100) : 0;
      const freePercent = size > 0 ? Math.round((free / size) * 100) : 0;
      let status: 'healthy' | 'degraded' | 'failed' = 'healthy';
      const st = (r.state || '').toLowerCase();
      if (st === 'clean' || st === 'active' || st === '') {
        status = 'healthy';
      } else if (st === 'degraded' || st === 'recovering') {
        status = 'degraded';
      } else {
        status = 'failed';
      }
      return {
        name: (r.name || '').replace('/dev/', ''),
        level: (r.level || '').replace('raid', ''),
        status,
        devices: Array.isArray(r.devices) ? r.devices.length : (r.devices || 0),
        size: formatBytes(size),
        used: formatBytes(used),
        free: formatBytes(free),
        used_percent: usedPercent,
        free_percent: freePercent,
      };
    });
    const totalCapacity = arrays.reduce((sum, a) => sum + parseBytes(a.size), 0);
    const totalUsed = arrays.reduce((sum, a) => sum + parseBytes(a.used), 0);
    const totalFree = Math.max(0, totalCapacity - totalUsed);
    return {
      arrays,
      total_capacity: formatBytes(totalCapacity),
      total_used: formatBytes(totalUsed),
      total_free: formatBytes(totalFree),
    };
  }

  function formatBytes(bytes: number): string {
    if (bytes === 0) return '0 B';
    const units = ['B', 'KB', 'MB', 'GB', 'TB', 'PB'];
    const i = Math.floor(Math.log(bytes) / Math.log(1024));
    const val = bytes / Math.pow(1024, i);
    return val.toFixed(i > 0 ? 1 : 0) + ' ' + units[i];
  }

  function parseBytes(s: string): number {
    const m = s.match(/^([\d.]+)\s*(B|KB|MB|GB|TB|PB)$/);
    if (!m) return 0;
    const val = parseFloat(m[1]);
    const units: Record<string, number> = { B: 0, KB: 1, MB: 2, GB: 3, TB: 4, PB: 5 };
    return val * Math.pow(1024, units[m[2]]);
  }

  // Security monitor data — pulls from the same read-only endpoints already
  // used by Settings' Antivirus/Firewall/Anti-Ransomware cards
  // (src/handlers/antivirus.rs, firewall.rs, ransomware.rs), no new backend
  // surface. Polled at a slower cadence than CPU/RAM since these values
  // rarely change between ticks.
  type SecurityData = {
    avLastScan: { finishedAt: string | null; startedAt: string } | null;
    avThreatCount: number;
    fwAvailable: boolean;
    fwEnabled: boolean;
    fwRuleCount: number;
    ransomProtectedCount: number;
    ransomLastEvent: { eventType: string; createdAt: string } | null;
  };

  let securityData: WidgetLoadResult<SecurityData> = { status: 'idle' };

  async function loadSecurityData() {
    const [scansRes, threatsRes, fwRes, ransomRes, eventsRes] = await Promise.all([
      api.getAntivirusScanHistory(),
      api.getAntivirusThreats(),
      api.getFirewallStatus(),
      api.getRansomwareStatus(),
      api.getRansomwareEvents(),
    ]);

    if (!scansRes.success && !fwRes.success && !ransomRes.success) {
      securityData = { status: 'error', error: 'Failed to load security data' };
      return;
    }

    const scans = scansRes.success && Array.isArray(scansRes.data) ? scansRes.data : [];
    const threats = threatsRes.success && Array.isArray(threatsRes.data) ? threatsRes.data : [];
    const fw = fwRes.success && fwRes.data ? fwRes.data : { available: false, enabled: false, rules: [] };
    const ransom = ransomRes.success && ransomRes.data ? ransomRes.data : { protected_path_count: 0 };
    const events = eventsRes.success && Array.isArray(eventsRes.data) ? eventsRes.data : [];

    securityData = {
      status: 'ready',
      data: {
        avLastScan: scans.length > 0
          ? { finishedAt: scans[0].finished_at ?? null, startedAt: scans[0].started_at }
          : null,
        avThreatCount: threats.length,
        fwAvailable: !!fw.available,
        fwEnabled: !!fw.enabled,
        fwRuleCount: Array.isArray(fw.rules) ? fw.rules.length : 0,
        ransomProtectedCount: ransom.protected_path_count ?? 0,
        ransomLastEvent: events.length > 0
          ? { eventType: events[0].event_type, createdAt: events[0].created_at }
          : null,
      },
    };
  }

  // Compact date formatting for security-monitor timestamps, following the
  // same LOCALE_MAP pattern as formatDate/formatTime below.
  function formatShortDate(iso: string): string {
    const d = new Date(iso);
    if (isNaN(d.getTime())) return iso;
    return d.toLocaleDateString(LOCALE_MAP[$languageStore] || 'en-US', {
      month: 'short',
      day: 'numeric',
      hour: '2-digit',
      minute: '2-digit',
    });
  }

  // VPN monitor data — read-only, reuses the same /vpn/connections endpoint
  // the VPN app itself calls, no new backend surface.
  type VpnData = { wireguardActive: number; openvpnActive: number; wireguardAvailable: boolean; openvpnAvailable: boolean };
  let vpnData: WidgetLoadResult<VpnData> = { status: 'idle' };

  async function loadVpnData() {
    const [capsRes, connsRes] = await Promise.all([api.getVpnCapabilities(), api.listVpnConnections()]);
    if (!capsRes.success && !connsRes.success) {
      vpnData = { status: 'error', error: 'Failed to load VPN data' };
      return;
    }
    const caps = capsRes.success && capsRes.data ? capsRes.data.capabilities ?? capsRes.data : {};
    const conns: any[] = connsRes.success && connsRes.data ? (connsRes.data.connections ?? connsRes.data) : [];
    vpnData = {
      status: 'ready',
      data: {
        wireguardActive: conns.filter(c => c.vpn_type === 'wireguard' && c.status === 'connected').length,
        openvpnActive: conns.filter(c => c.vpn_type === 'openvpn' && c.status === 'connected').length,
        wireguardAvailable: !!caps.wireguard,
        openvpnAvailable: !!caps.openvpn,
      },
    };
  }

  // Downloader (aria2) monitor data — deliberately checks /downloader/status
  // (a passive read) first and only calls global-stat/tasks when aria2 is
  // already running: those two endpoints silently spawn the aria2c process
  // if it isn't running yet (ensure_aria2_running in src/handlers/downloader.rs),
  // and a background desktop widget must never have the side effect of
  // starting a service on its own just by being visible.
  type DownloaderData = { available: boolean; running: boolean; activeCount: number; downloadSpeed: number; uploadSpeed: number };
  let downloaderData: WidgetLoadResult<DownloaderData> = { status: 'idle' };

  function formatSpeed(bytesPerSec: number): string {
    if (bytesPerSec < 1024) return `${bytesPerSec} B/s`;
    if (bytesPerSec < 1024 * 1024) return `${(bytesPerSec / 1024).toFixed(1)} KB/s`;
    return `${(bytesPerSec / (1024 * 1024)).toFixed(1)} MB/s`;
  }

  async function loadDownloaderData() {
    const statusRes = await api.getDownloaderStatus();
    if (!statusRes.success || !statusRes.data) {
      downloaderData = { status: 'error', error: 'Failed to load downloader status' };
      return;
    }
    const { available, running } = statusRes.data;
    if (!available || !running) {
      downloaderData = { status: 'ready', data: { available, running, activeCount: 0, downloadSpeed: 0, uploadSpeed: 0 } };
      return;
    }
    const statRes = await api.getDownloaderGlobalStat();
    if (statRes.success && statRes.data) {
      downloaderData = {
        status: 'ready',
        data: {
          available,
          running,
          activeCount: parseInt(statRes.data.num_active, 10) || 0,
          downloadSpeed: parseInt(statRes.data.download_speed, 10) || 0,
          uploadSpeed: parseInt(statRes.data.upload_speed, 10) || 0,
        },
      };
    } else {
      downloaderData = { status: 'ready', data: { available, running, activeCount: 0, downloadSpeed: 0, uploadSpeed: 0 } };
    }
  }

  // Syncthing monitor data — read-only status/folder endpoints, no new
  // backend surface. Syncthing itself runs as an independent system
  // service, so unlike aria2 there's no start-on-read side effect here.
  type SyncthingData = { available: boolean; folderCount: number; pendingFiles: number };
  let syncthingData: WidgetLoadResult<SyncthingData> = { status: 'idle' };

  async function loadSyncthingData() {
    const statusRes = await api.getSyncthingStatus();
    if (!statusRes.success || !statusRes.data) {
      syncthingData = { status: 'error', error: 'Failed to load Syncthing status' };
      return;
    }
    if (!statusRes.data.available) {
      syncthingData = { status: 'ready', data: { available: false, folderCount: 0, pendingFiles: 0 } };
      return;
    }
    const foldersRes = await api.getSyncthingFolders();
    const folders: any[] = foldersRes.success && Array.isArray(foldersRes.data) ? foldersRes.data : [];
    syncthingData = {
      status: 'ready',
      data: {
        available: true,
        folderCount: folders.length,
        pendingFiles: folders.reduce((sum, f) => sum + (f.need_files ?? 0), 0),
      },
    };
  }

  // Docker monitor — reuses the same /docker/available and /docker/containers
  // endpoints the Docker app itself calls (via api.ts's existing, already-
  // admin-guarded getDockerContainers/startContainer/stopContainer/
  // restartContainer methods), no new backend surface. Docker.svelte itself
  // uses its own local authedFetch instead of these api.ts methods by design
  // (see CLAUDE.md) — that choice is unrelated to this widget, which is a
  // different consumer of the same, already-existing api.ts methods.
  type DockerMonitorContainer = { name: string; image: string; state: string };
  type DockerData = { available: boolean; containers: DockerMonitorContainer[] };
  let dockerData: WidgetLoadResult<DockerData> = { status: 'idle' };
  let dockerActionBusy: Record<string, boolean> = {};

  async function loadDockerData() {
    const availRes = await api.isDockerAvailable();
    if (!availRes.success || !availRes.data?.available) {
      dockerData = { status: 'ready', data: { available: false, containers: [] } };
      return;
    }
    const res = await api.getDockerContainers();
    if (!res.success || !res.data) {
      dockerData = { status: 'error', error: 'Failed to load Docker data' };
      return;
    }
    dockerData = { status: 'ready', data: { available: true, containers: res.data } };
  }

  async function dockerContainerAction(name: string, action: 'start' | 'stop' | 'restart') {
    dockerActionBusy = { ...dockerActionBusy, [name]: true };
    const call = action === 'start' ? api.startContainer(name) : action === 'stop' ? api.stopContainer(name) : api.restartContainer(name);
    const res = await call;
    if (!res.success) {
    }
    await loadDockerData();
    dockerActionBusy = { ...dockerActionBusy, [name]: false };
  }

  // Weather monitor — calls Open-Meteo directly from the browser (free,
  // no API key, CORS-enabled), not through the HomeVault backend, since
  // there's nothing privileged or host-specific about a weather lookup.
  // The chosen location lives in its own small localStorage-backed store
  // (weatherLocationStore), not monitorStore, which only ever holds
  // position/visibility for every monitor — mixing a third concern
  // (arbitrary user-entered location data) into that store made it a much
  // stranger thing to reset/reason about via Widgets.svelte's
  // "Reset to Default", which only ever touches visibility.
  // The location itself is no longer chosen from inside this floating
  // panel — that search UI now lives in Settings (a "where does this
  // widget's data come from" configuration belongs with the rest of the
  // app's settings, not buried in a desktop panel); this component only
  // ever *reads* weatherLocationStore now, same as it reads any other
  // already-configured setting.
  type WeatherData = {
    temperature: number; weatherCode: number; windSpeed: number; humidity: number;
    feelsLike: number; windGusts: number; precipitation: number; cloudCover: number; pressure: number;
  };
  let weatherData: WidgetLoadResult<WeatherData> = { status: 'idle' };

  // WMO weather interpretation codes (https://open-meteo.com/en/docs), bucketed
  // into the handful of icons/labels a "simple" widget needs — not a 1:1
  // mapping of every one of the ~28 real codes.
  function weatherCodeInfo(code: number): { icon: typeof Sun; labelKey: string } {
    if (code === 0) return { icon: Sun, labelKey: 'desktop.weather_clear' };
    if (code <= 3) return { icon: IconCloud, labelKey: 'desktop.weather_cloudy' };
    if (code === 45 || code === 48) return { icon: IconCloudFog, labelKey: 'desktop.weather_fog' };
    if (code >= 51 && code <= 67) return { icon: IconCloudRain, labelKey: 'desktop.weather_rain' };
    if (code >= 71 && code <= 86) return { icon: IconSnowflake, labelKey: 'desktop.weather_snow' };
    if (code >= 95) return { icon: IconCloudStorm, labelKey: 'desktop.weather_storm' };
    return { icon: IconCloud, labelKey: 'desktop.weather_cloudy' };
  }

  // Re-fetch whenever the configured location changes — including a change
  // made from the Settings window while this desktop is already mounted,
  // since choosing a location no longer happens inside this panel.
  $: if ($weatherLocationStore) loadWeatherData();
  $: if (!$weatherLocationStore) weatherData = { status: 'idle' };

  async function loadWeatherData() {
    const location = get(weatherLocationStore);
    if (!location) {
      weatherData = { status: 'idle' };
      return;
    }
    weatherData = { status: 'loading' };
    try {
      const fields = 'temperature_2m,relative_humidity_2m,weather_code,wind_speed_10m,apparent_temperature,wind_gusts_10m,precipitation,cloud_cover,pressure_msl';
      const url = `https://api.open-meteo.com/v1/forecast?latitude=${location.lat}&longitude=${location.lon}&current=${fields}&timezone=auto`;
      const res = await fetch(url);
      if (!res.ok) throw new Error(`HTTP ${res.status}`);
      const json = await res.json();
      weatherData = {
        status: 'ready',
        data: {
          temperature: Math.round(json.current.temperature_2m),
          weatherCode: json.current.weather_code,
          windSpeed: Math.round(json.current.wind_speed_10m),
          humidity: Math.round(json.current.relative_humidity_2m),
          feelsLike: Math.round(json.current.apparent_temperature),
          windGusts: Math.round(json.current.wind_gusts_10m),
          precipitation: json.current.precipitation,
          cloudCover: Math.round(json.current.cloud_cover),
          pressure: Math.round(json.current.pressure_msl),
        },
      };
    } catch {
      weatherData = { status: 'error', error: translate('desktop.weather_error') };
    }
  }

  // Active Sessions monitor — admin-only backend count (distinct users with
  // a currently valid session, not raw session rows, so one user open in
  // three tabs still counts as one). Non-admins get a 403 from the
  // endpoint, same as every other admin-gated monitor (Security); shown as
  // an error state rather than hiding the panel, consistent with those.
  type SessionsData = { count: number };
  let sessionsData: WidgetLoadResult<SessionsData> = { status: 'idle' };

  async function loadSessionsData() {
    const res = await api.getActiveSessionsCount();
    if (!res.success || !res.data) {
      sessionsData = { status: 'error', error: translate('desktop.sessions_error') };
      return;
    }
    sessionsData = { status: 'ready', data: { count: res.data.count } };
  }

  // System Health monitor — reuses the Health app's own existing, already-
  // audited `/api/health` endpoint (`api.healthCheck()`) verbatim; no new
  // backend surface. Just tallies the 8 checks' `status` field into
  // counts so an admin can see at a glance whether anything needs
  // attention without opening the full Health page.
  type HealthCheckStatus = 'healthy' | 'warning' | 'unhealthy' | 'unknown';
  type HealthSummary = { healthy: number; warning: number; critical: number; unknown: number; overall: HealthCheckStatus };
  let healthData: WidgetLoadResult<HealthSummary> = { status: 'idle' };

  async function loadHealthData() {
    const res = await api.healthCheck();
    if (!res.success || !res.data) {
      healthData = { status: 'error', error: translate('desktop.health_error') };
      return;
    }
    const checks: { status: HealthCheckStatus }[] = res.data.checks || [];
    const summary: HealthSummary = { healthy: 0, warning: 0, critical: 0, unknown: 0, overall: res.data.overall_status || 'unknown' };
    for (const check of checks) {
      if (check.status === 'healthy') summary.healthy++;
      else if (check.status === 'warning') summary.warning++;
      else if (check.status === 'unhealthy') summary.critical++;
      else summary.unknown++;
    }
    healthData = { status: 'ready', data: summary };
  }

  // ── Dock chrome — badges / attention / auto-hide ───────────────────────
  // Placed here (not up with the other chrome-effect state) so it can read
  // dockerData / healthData / viewportWidth, all declared above.

  // Dock badges — counts surfaced straight from data the desktop already
  // polls, shown only when non-zero.
  $: dockerRunningCount =
    dockerData.status === 'ready' && dockerData.data.available
      ? dockerData.data.containers.filter((c) => c.state === 'Running').length
      : 0;
  $: healthIssueCount =
    healthData.status === 'ready'
      ? healthData.data.warning + healthData.data.critical
      : 0;

  // Dock attention nudge — a one-shot bounce of a pinned icon when its
  // underlying state degrades (health checks start failing).
  let dockAttention: Record<string, boolean> = {};
  let prevHealthIssueCount = 0;
  $: onHealthIssueCountChange(healthIssueCount);
  function onHealthIssueCountChange(n: number) {
    if (chromeReady && n > prevHealthIssueCount && !prefersReducedMotion) {
      triggerDockAttention('health');
    }
    prevHealthIssueCount = n;
  }
  function triggerDockAttention(appId: string) {
    dockAttention = { ...dockAttention, [appId]: true };
    setTimeout(() => {
      dockAttention = { ...dockAttention, [appId]: false };
    }, 600);
  }

  // Fullscreen mode — while a window is maximized it takes the whole
  // screen (WindowFrame sets inset:0), so BOTH the floating dock and the
  // topbar pill slide out of the way. Each peeks back when the pointer
  // reaches its screen edge (bottom → dock, top → topbar). The pointer
  // listener is attached only while a window is maximized, so there is no
  // always-on handler otherwise. Only on pointer-first, roomy viewports:
  // on phones/tablets an app almost always opens maximized and an
  // edge-reveal is fiddly with touch, so there the chrome just stays put.
  let dockPeek = false;
  let topbarPeek = false;
  $: anyTopbarPanelOpen = showUserMenu || showConnectionInfo || showNotifications;
  $: chromeAutoHideEligible =
    viewportWidth > 900 && $windowManager.some((w) => w.isMaximized && !w.isMinimized);
  // On a phone an open app is truly fullscreen (WindowFrame gives it the
  // whole viewport), so the topbar pill and dock just get in the way — hide
  // them outright while a window is up (closing it returns to the desktop
  // where they show again). No edge-peek here: it's fiddly with touch, and
  // the phone convention is "you're on the home screen or in an app".
  $: phoneChromeHidden =
    viewportWidth <= 600 && $windowManager.some((w) => w.isMaximized && !w.isMinimized);
  $: dockHidden = (chromeAutoHideEligible && !dockPeek) || phoneChromeHidden;
  $: menubarHidden =
    ((chromeAutoHideEligible && !topbarPeek) || phoneChromeHidden) && !anyTopbarPanelOpen;
  let chromePeekListening = false;
  let dockPeekRehideTimer: ReturnType<typeof setTimeout> | null = null;
  let topbarPeekRehideTimer: ReturnType<typeof setTimeout> | null = null;
  function onChromePeekMove(e: PointerEvent) {
    // Dock — bottom edge
    if (e.clientY >= window.innerHeight - 4) {
      if (dockPeekRehideTimer) { clearTimeout(dockPeekRehideTimer); dockPeekRehideTimer = null; }
      if (!dockPeek) dockPeek = true;
    } else if (dockPeek && !dockPeekRehideTimer) {
      dockPeekRehideTimer = setTimeout(() => {
        dockPeekRehideTimer = null;
        if (dockEl && !dockEl.matches(':hover')) dockPeek = false;
      }, 600);
    }
    // Topbar — top edge
    if (e.clientY <= 4) {
      if (topbarPeekRehideTimer) { clearTimeout(topbarPeekRehideTimer); topbarPeekRehideTimer = null; }
      if (!topbarPeek) topbarPeek = true;
    } else if (topbarPeek && !topbarPeekRehideTimer) {
      topbarPeekRehideTimer = setTimeout(() => {
        topbarPeekRehideTimer = null;
        if (!anyTopbarPanelOpen && menubarEl && !menubarEl.matches(':hover')) topbarPeek = false;
      }, 600);
    }
  }
  $: syncChromePeekListener(chromeAutoHideEligible);
  function syncChromePeekListener(active: boolean) {
    if (typeof window === 'undefined') return;
    if (active && !chromePeekListening) {
      window.addEventListener('pointermove', onChromePeekMove, { passive: true });
      chromePeekListening = true;
    } else if (!active && chromePeekListening) {
      window.removeEventListener('pointermove', onChromePeekMove);
      chromePeekListening = false;
      dockPeek = false;
      topbarPeek = false;
    }
  }

  // Best-effort IP geolocation of the public IP, shown as an extra row in
  // the connection popover. Fetched browser-side from a keyless CORS
  // endpoint (same pattern as the Weather monitor's Open-Meteo call); if
  // it's blocked / offline / returns nothing usable, the row stays hidden.
  let serverLocation: { label: string; flag: string } | null = null;

  async function loadServerLocation(ip: string) {
    if (!ip) return;
    const ctrl = new AbortController();
    const timer = setTimeout(() => ctrl.abort(), 6000);
    try {
      const res = await fetch(`https://ipwho.is/${encodeURIComponent(ip)}`, { signal: ctrl.signal });
      if (!res.ok) return;
      const d = await res.json();
      if (!d || d.success === false) return;
      const parts = [d.city, d.region, d.country].filter(
        (p: unknown): p is string => typeof p === 'string' && p.length > 0,
      );
      if (parts.length === 0) return;
      serverLocation = { label: parts.join(', '), flag: (d.flag && d.flag.emoji) || '' };
    } catch {
      // unreachable / blocked / timeout — leave serverLocation null (row hidden)
    } finally {
      clearTimeout(timer);
    }
  }

  // Topbar connection-status popover — the local LAN IP (from the primary,
  // non-loopback interface via /network/summary), the internet-facing IP
  // (via /network/public-ip) and, when available, the server's rough
  // location. Loaded lazily on first open rather than polled.
  async function loadConnectionInfo() {
    connIpLoading = true;
    const [summary, pub] = await Promise.all([
      api.getNetworkSummary(),
      api.getPublicIp(),
    ]);
    if (summary.success && summary.data) {
      connLocalIp = summary.data.local_ip || '';
    }
    if (pub.success && pub.data) {
      connPublicIp = pub.data.ip || '';
      if (connPublicIp) loadServerLocation(connPublicIp);
    }
    connIpLoading = false;
    connIpLoaded = true;
  }

  // The three topbar-button panels (network / account / notifications) are
  // mutually exclusive: opening one closes the others. Pressing a button
  // whose panel is already open just closes it.
  type TopbarPanel = 'net' | 'user' | 'notifications';
  function toggleTopbarPanel(panel: TopbarPanel, e?: Event) {
    e?.stopPropagation();
    const open =
      (panel === 'net' && !showConnectionInfo) ||
      (panel === 'user' && !showUserMenu) ||
      (panel === 'notifications' && !showNotifications);
    showConnectionInfo = panel === 'net' && open;
    showUserMenu = panel === 'user' && open;
    showNotifications = panel === 'notifications' && open;
    showAppsCenter = false;
    showActivityPanel = false;
    if (panel === 'net' && open && !connIpLoaded) {
      loadConnectionInfo();
    }
  }

  async function copyToClipboard(text: string) {
    try { await navigator.clipboard.writeText(text); } catch { /* noop */ }
  }

  // Calendar monitor — purely local, no backend surface. Builds the day grid
  // for the current month; today's cell is flagged so the template can paint
  // it in a different color. Weekday order follows the active UI language.
  type CalendarCell = { key: string; label: string; inMonth: boolean; isToday: boolean };
  let calendarNow = new Date();

  $: calendarDays = (() => {
    const year = calendarNow.getFullYear();
    const month = calendarNow.getMonth();
    const today = new Date();
    const isSameDay = today.getFullYear() === year && today.getMonth() === month;
    const first = new Date(year, month, 1);
    const startOffset = first.getDay(); // 0 = Sunday
    const daysInMonth = new Date(year, month + 1, 0).getDate();
    const cells: CalendarCell[] = [];
    for (let i = 0; i < startOffset; i++) {
      cells.push({ key: `pad-${i}`, label: '', inMonth: false, isToday: false });
    }
    for (let d = 1; d <= daysInMonth; d++) {
      cells.push({
        key: `day-${d}`,
        label: String(d),
        inMonth: true,
        isToday: isSameDay && d === today.getDate(),
      });
    }
    while (cells.length % 7 !== 0) {
      cells.push({ key: `tail-${cells.length}`, label: '', inMonth: false, isToday: false });
    }
    return cells;
  })();

  // Month/year navigation — reassigns calendarNow so the calendarDays/
  // calendarMonthLabel reactive statements above and below recompute.
  function calendarGoToPrevMonth() { calendarNow = new Date(calendarNow.getFullYear(), calendarNow.getMonth() - 1, 1); }
  function calendarGoToNextMonth() { calendarNow = new Date(calendarNow.getFullYear(), calendarNow.getMonth() + 1, 1); }
  function calendarGoToPrevYear() { calendarNow = new Date(calendarNow.getFullYear() - 1, calendarNow.getMonth(), 1); }
  function calendarGoToNextYear() { calendarNow = new Date(calendarNow.getFullYear() + 1, calendarNow.getMonth(), 1); }
  function calendarGoToToday() { calendarNow = new Date(); }

  // Monitor drag handlers — table-driven so adding a new draggable monitor
  // widget only means adding one entry per table, not another if/else
  // branch in every function below.
  const MONITOR_WIDTH: Record<MonitorKind, number> = {
    resource: 280, raid: 320, security: 300, vpn: 280, downloader: 280, syncthing: 280, calendar: 300, docker: 300, weather: 280, sessions: 280, health: 280,
  };
  const MONITOR_SELECTOR: Record<MonitorKind, string> = {
    resource: '.hv-resource-monitor', raid: '.hv-raid-monitor', security: '.hv-security-monitor',
    vpn: '.hv-vpn-monitor', downloader: '.hv-downloader-monitor', syncthing: '.hv-syncthing-monitor',
    calendar: '.hv-calendar-monitor', docker: '.hv-docker-monitor', weather: '.hv-weather-monitor', sessions: '.hv-sessions-monitor',
    health: '.hv-health-monitor',
  };
  /* Per-monitor accent for the header icon tile — same colours as the
     `WIDGET_DEFS` catalogue so a monitor's tile matches its Settings row. */
  const MONITOR_COLOR: Record<MonitorKind, string> = {
    resource: '#1E62AE', raid: '#6366f1', security: '#C22B21', vpn: '#059669',
    downloader: '#22c55e', syncthing: '#0ea5e9', calendar: '#d97706', docker: '#2496ed',
    weather: '#0284c7', sessions: '#7c3aed', health: '#e11d48',
  };

  function getMonitorPosition(pos: MonitorPositions, type: MonitorKind): { x: number; y: number } {
    switch (type) {
      case 'resource': return pos.resourceMonitor;
      case 'raid': return pos.raidMonitor;
      case 'security': return pos.securityMonitor;
      case 'vpn': return pos.vpnMonitor;
      case 'downloader': return pos.downloaderMonitor;
      case 'syncthing': return pos.syncthingMonitor;
      case 'calendar': return pos.calendarMonitor;
      case 'docker': return pos.dockerMonitor;
      case 'weather': return pos.weatherMonitor;
      case 'sessions': return pos.sessionsMonitor;
      case 'health': return pos.healthMonitor;
    }
  }

  function updateMonitorPosition(type: MonitorKind, x: number, y: number) {
    switch (type) {
      case 'resource': return monitorStore.updateResourcePosition(x, y);
      case 'raid': return monitorStore.updateRaidPosition(x, y);
      case 'security': return monitorStore.updateSecurityPosition(x, y);
      case 'vpn': return monitorStore.updateVpnPosition(x, y);
      case 'downloader': return monitorStore.updateDownloaderPosition(x, y);
      case 'syncthing': return monitorStore.updateSyncthingPosition(x, y);
      case 'calendar': return monitorStore.updateCalendarPosition(x, y);
      case 'docker': return monitorStore.updateDockerPosition(x, y);
      case 'weather': return monitorStore.updateWeatherPosition(x, y);
      case 'sessions': return monitorStore.updateSessionsPosition(x, y);
      case 'health': return monitorStore.updateHealthPosition(x, y);
    }
  }

  function handleMonitorDragStart(e: PointerEvent, type: MonitorKind) {
    const target = e.target as HTMLElement;
    // Drag from the header only (matches app windows), never from a control.
    if (!target.closest('.hv-widget-header')) return;
    if (target.closest('button, a, input, [role="button"]')) return;

    e.preventDefault();
    e.stopPropagation();
    draggingMonitor = type;
    monitorDragStartX = e.clientX;
    monitorDragStartY = e.clientY;

    const pos = getMonitorPosition(get(monitorStore), type);
    monitorWidgetStartX = pos.x;
    monitorWidgetStartY = pos.y;
    monitorCurrentX = monitorWidgetStartX;
    monitorCurrentY = monitorWidgetStartY;

    // Pointer Events (not mouse) so widget dragging works with touch and pen.
    document.addEventListener('pointermove', handleMonitorDragMove, { passive: false });
    document.addEventListener('pointerup', handleMonitorDragEnd);
    document.addEventListener('pointercancel', handleMonitorDragEnd);
    document.body.style.cursor = 'grabbing';
    document.body.style.userSelect = 'none';
  }

  function handleMonitorDragMove(e: PointerEvent) {
    if (!draggingMonitor) return;
    e.preventDefault();

    const deltaX = e.clientX - monitorDragStartX;
    const deltaY = e.clientY - monitorDragStartY;

    let newX = monitorWidgetStartX + deltaX;
    let newY = monitorWidgetStartY + deltaY;

    const width = MONITOR_WIDTH[draggingMonitor];
    const el = document.querySelector(MONITOR_SELECTOR[draggingMonitor]) as HTMLElement | null;
    const elHeight = el ? el.offsetHeight : 400;

    const result = applySnap(newX, newY, width, elHeight);
    monitorSnapEdge = result.snapEdge;
    monitorCurrentX = result.x;
    monitorCurrentY = result.y;
  }

  function handleMonitorDragEnd() {
    if (!draggingMonitor) return;

    const type = draggingMonitor;
    draggingMonitor = null;
    monitorSnapEdge = 'none';

    document.body.style.cursor = '';
    document.body.style.userSelect = '';
    document.removeEventListener('pointermove', handleMonitorDragMove);
    document.removeEventListener('pointerup', handleMonitorDragEnd);
    document.removeEventListener('pointercancel', handleMonitorDragEnd);

    updateMonitorPosition(type, monitorCurrentX, monitorCurrentY);
  }

  // Applications configuration. Structural fields (id/icon/color/category/
  // component) never change; `name`/`desc` below are the English fallback
  // only — the reactive `apps` derived below is what's actually rendered
  // (translated, id-keyed lookup via `desktop.app_<id>_name`/`_desc`,
  // matching the same pattern already used for the Docker catalog's
  // `catalog_desc_<id>` and the Widgets store's `widget_<id>_name`).
  const appDefs = [
    { id: 'dashboard', name: 'Dashboard', icon: 'dashboard', color: 'var(--hv-app-dashboard-grad)', category: 'System', desc: 'System overview & live stats', component: Dashboard, customIcon: launchpadIcons.dashboard },
    { id: 'storage', name: 'Storage', icon: 'storage', color: 'var(--hv-app-storage-grad)', category: 'Storage', desc: 'Disks, volumes & pools', component: Storage, customIcon: launchpadIcons.storage },
    { id: 'raid', name: 'RAID', icon: 'database', color: 'var(--hv-app-raid-grad)', category: 'Storage', desc: 'Create virtual disks', component: Raid, customIcon: launchpadIcons.raid },
    { id: 'explorer', name: 'Explorer', icon: 'folder_open', color: 'var(--hv-app-explorer-grad)', category: 'Storage', desc: 'Browse shared files', component: Explorer, customIcon: launchpadIcons.explorer },
    { id: 'docker', name: 'Docker', icon: 'inventory_2', color: 'var(--hv-app-docker-grad)', category: 'System', desc: 'Containers & images', component: Docker, customIcon: launchpadIcons.docker },
    { id: 'network', name: 'Network', icon: 'wifi', color: 'var(--hv-app-network-grad)', category: 'Network', desc: 'Interfaces & routing', component: Network, customIcon: launchpadIcons.network },
    { id: 'vpn', name: 'VPN', icon: 'lock', color: 'var(--hv-app-vpn-grad)', category: 'Network', desc: 'WireGuard & OpenVPN access', component: VPN, customIcon: launchpadIcons.vpn },
    { id: 'backup', name: 'Backup', icon: 'backup', color: 'var(--hv-app-backup-grad)', category: 'Storage', desc: 'Snapshots & restores', component: Restic, customIcon: launchpadIcons.backup },
    { id: 'users', name: 'Users', icon: 'people', color: 'var(--hv-app-users-grad)', category: 'System', desc: 'Accounts & permissions', component: Users, customIcon: launchpadIcons.users },
    { id: 'groups', name: 'Groups', icon: 'group', color: 'var(--hv-app-groups-grad)', category: 'System', desc: 'User groups & roles', component: Groups, customIcon: launchpadIcons.groups },
    { id: 'quotas', name: 'Quotas', icon: 'gauge', color: 'var(--hv-app-quotas-grad)', category: 'System', desc: 'Transfer quota usage', component: Quotas, customIcon: launchpadIcons.quotas },
    { id: 'shares', name: 'Shares', icon: 'folder_shared', color: 'var(--hv-app-shares-grad)', category: 'Storage', desc: 'Network file shares', component: Shares, customIcon: launchpadIcons.shares },
    { id: 'rclone', name: 'Cloud Sync', icon: 'cloud_sync', color: 'var(--hv-app-rclone-grad)', category: 'Network', desc: 'Remote cloud storage', component: CloudSync, customIcon: launchpadIcons.rclone },
    { id: 'syncthing', name: 'Syncthing', icon: 'folder_sync', color: 'var(--hv-app-syncthing-grad)', category: 'Network', desc: 'Continuous file sync', component: Syncthing, customIcon: launchpadIcons.syncthing },
    { id: 'health', name: 'Health', icon: 'monitor_heart', color: 'var(--hv-app-health-grad)', category: 'Monitoring', desc: 'System health & alerts', component: Health, customIcon: launchpadIcons.health },
    { id: 'chat', name: 'AI Assistant', icon: 'chat', color: 'var(--hv-app-chat-grad)', category: 'Productivity', desc: 'Ask the AI assistant', component: Chat, customIcon: launchpadIcons.assistant },
    { id: 'downloader', name: 'Downloader', icon: 'download', color: 'var(--hv-app-downloader-grad)', category: 'Productivity', desc: 'Manage downloads', component: Downloader, customIcon: launchpadIcons.downloader },
    { id: 'rescue', name: 'Rescue', icon: 'healing', color: 'var(--hv-app-rescue-grad)', category: 'System', desc: 'Recovery & repair', component: Rescue, customIcon: launchpadIcons.rescue },
    { id: 'widgets', name: 'Widgets', icon: 'layout-grid', color: 'var(--hv-app-widgets-grad)', category: 'System', desc: 'Add or remove desktop widgets', component: Widgets, customIcon: launchpadIcons.widgets },
    { id: 'journal', name: 'System Journal', icon: 'article', color: 'var(--hv-app-journal-grad)', category: 'Monitoring', desc: 'Centralized system logs & events', component: Journal, customIcon: launchpadIcons.journal },
    { id: 'virtual-machines', name: 'Virtual Machines', icon: 'server', color: 'var(--hv-app-virtual-machines-grad)', category: 'System', desc: 'Manage VMs and system containers', component: VirtualMachines, customIcon: launchpadIcons.virtualMachines },
  ];

  // Looks up `desktop.app_<id>_<field>` and falls back to the English
  // default (from appDefs) if the key is missing — same fallback style as
  // Docker.svelte's catalogDescFor(). `$t` is dereferenced here (rather
  // than calling the plain `translate()` helper) specifically so this
  // reactive block re-runs on every language change.
  function appField(id: string, field: 'name' | 'desc', fallback: string, tFn: (k: string) => string): string {
    const key = `desktop.app_${id}_${field}`;
    const translated = tFn(key);
    return translated === key ? fallback : translated;
  }

  // Every app uses its own gradient (`--hv-app-<id>-grad` in tokens.css) on
  // every surface — Launchpad tile + in-window WindowHeader tile via the
  // app's `color`, dock tile via the per-app `.hv-dock-icon-*` classes (apps
  // without one fall back to the `--hv-app-accent-grad` base rule). The icon
  // tile is deliberately never flat. `--hv-app-accent-grad` is the
  // base/fallback (dashboard aliases it).
  const APP_ACCENT_GRAD = 'var(--hv-app-accent-grad)';
  function appIconBg(_id: string, _color: string): string {
    return _color || APP_ACCENT_GRAD;
  }

  $: apps = appDefs.map(a => ({
    ...a,
    name: appField(a.id, 'name', a.name, $t),
    desc: appField(a.id, 'desc', a.desc, $t),
    iconBg: appIconBg(a.id, a.color),
  }));

  // Installed Docker catalog apps, surfaced as Launchpad tiles (fetched
  // below). They all live under one "Docker Apps" category and never open
  // as an in-app window — clicking a tile opens the app's own web UI in a
  // new browser tab or window (the user picks), same as the Docker page's
  // container "Open" menu. A tile disappears when the app is uninstalled and
  // greys out (not clickable) when the app is installed but stopped.
  let dockerApps: Array<{ id: string; app_id: string; name: string; icon: string; category: string; installed: boolean; running: boolean; installed_port?: number; proxy_port?: number }> = [];

  async function loadDockerCatalog() {
    const res = await api.getDockerCatalog();
    if (res.success && res.data) {
      dockerApps = res.data;
    }
  }

  $: installedDockerApps = dockerApps
    .filter(a => a.installed)
    .map(a => {
      // Home Assistant runs with network_mode: host, so it has no
      // compose-published port to report — its UI is always on 8123.
      // Prefer the per-app reverse proxy port when it's up.
      const port = a.app_id === 'home-assistant' ? 8123 : (a.proxy_port ?? a.installed_port);
      return {
        id: `docker-${a.id}`,
        name: a.name,
        icon: 'container',
        customIcon: launchpadIcons.dockerApp,
        color: 'var(--hv-primary)',
        iconBg: 'var(--hv-primary)',
        category: 'Docker Apps',
        isDockerApp: true,
        running: a.running,
        // undefined when the app publishes no port → the tile renders
        // disabled with a "no web UI" label instead of being clickable.
        url: port ? `http://${location.hostname}:${port}` : undefined,
      };
    });

  $: allApps = [...apps, ...installedDockerApps];

  // Dashboard app (always visible in dock)
  $: dashboardApp = { id: 'dashboard', name: appField('dashboard', 'name', 'Dashboard', $t), icon: 'dashboard', color: 'var(--hv-primary)', component: Dashboard };

  // Dock apps (system tools)
  $: terminalApp = { id: 'terminal', name: appField('terminal', 'name', 'Terminal', $t), icon: 'terminal', color: 'var(--hv-primary)', component: Terminal };
  $: settingsApp = { id: 'settings', name: appField('settings', 'name', 'Settings', $t), icon: 'settings', color: 'var(--hv-primary)', component: Settings };

  // Minimized window bouncing back to its dock slot
  let bouncingWindowId: string | null = null;

  // Every app that has its own permanently-pinned dock icon must NOT also
  // get a second, dynamic icon here when minimized — its pinned icon shows
  // the running dot directly instead (see `.hv-dock-dot` in the markup).
  const PINNED_DOCK_APP_IDS = ['dashboard', 'terminal', 'docker', 'health', 'chat', 'settings'];

  // Derived: minimized windows for the dock (one per app, most recent first)
  $: minimizedWindows = $windowManager
    .filter(w => w.isMinimized && !PINNED_DOCK_APP_IDS.includes(w.appId))
    .slice()
    .reverse();

  // Get color for a given appId
  function getAppColor(appId: string): string {
    const app = apps.find(a => a.id === appId);
    if (app) return app.color;
    if (appId === 'terminal') return terminalApp.color;
    if (appId === 'settings') return settingsApp.color;
    return 'var(--hv-neutral-500)';
  }

  // Restore a minimized window from the dock with a small bounce animation
  function handleRestoreFromDock(windowId: string) {
    bouncingWindowId = windowId;
    windowManager.restoreWindow(windowId);
    setTimeout(() => {
      if (bouncingWindowId === windowId) {
        bouncingWindowId = null;
      }
    }, 500);
  }

  // Click dock item: restore if minimized, focus if open, open if not present
  // If this app already has a window, bring it to the front (un-minimising
  // if needed) and return true. Callers open a *new* window only when this
  // returns false — so a launcher / topbar / dock button pressed twice
  // never stacks duplicate windows of the same app.
  function focusExistingWindow(appId: string): boolean {
    const win = $windowManager.find(w => w.appId === appId);
    if (!win) return false;
    if (win.isMinimized) windowManager.restoreWindow(win.id);
    else windowManager.focusWindow(win.id);
    return true;
  }

  function handleDockAppClick(appId: string) {
    if (focusExistingWindow(appId)) return;
    const app = [...apps, terminalApp as any, settingsApp as any].find(a => a.id === appId);
    if (app?.component) {
      windowManager.openWindow(appId, app.name, app.icon, app.component, appIconBg(appId, app.color));
    }
  }

  // Set a pending action (logout / restart / shutdown) — the confirmation modal will execute it
  async function requestLogout() {
    showPowerModal = false;
    logoutAction = 'logout';
  }

  async function handleRestart() {
    showPowerModal = false;
    logoutAction = 'restart';
  }

  async function handleShutdown() {
    showPowerModal = false;
    logoutAction = 'shutdown';
  }

  // Execute the user's chosen action after they confirm via the modal dialog
  async function confirmLogoutAction() {
    const action = logoutAction;
    logoutAction = null;

    if (action === 'logout') {
      await handleLogout();
    } else if (action === 'restart') {
      const response = await api.restartServer();
      if (response.success) {
      } else {
      }
    } else if (action === 'shutdown') {
      const response = await api.shutdownServer();
      if (response.success) {
      } else {
      }
    }
  }

  function cancelLogoutAction() {
    logoutAction = null;
  }

  // Resource monitor polling — shared with every individually-placed
  // cpu/ram/network/cpu-temp Widget.svelte instance via subscribeWidget()
  // (frontend/src/lib/components/desktop/widgetData.ts) so N widgets on
  // the desktop still only produce one HTTP request per tick per metric,
  // not N+1. subscribeWidget() also does the immediate first load, so no
  // separate initial-load call is needed here.
  //
  // The cadence is Settings > Widgets > "Refresh interval"
  // (`monitorStore.refreshMs`, persisted in the `/api/monitor-layout` blob).
  // subscribeWidget() is reference-counted and keyed by `id:intervalMs`, so
  // re-subscribing on a change tears the old poller down and starts a new
  // one — no leaked interval, no duplicate request.
  let unsubResource: (() => void)[] = [];

  function startResourcePollers(ms: number) {
    unsubResource.forEach((unsub) => unsub());
    unsubResource = [
      subscribeWidget<CpuData>('cpu-usage', ms, (r) => (cpuData = r)),
      subscribeWidget<RamData>('ram-usage', ms, (r) => (ramData = r)),
      subscribeWidget<TempData>('cpu-temp', ms, (r) => (tempData = r)),
      subscribeWidget<NetworkData>('network-traffic', ms, (r) => (networkData = r)),
    ];
  }

  // `mounted` keeps this from subscribing before onMount, which already does
  // the first subscribe itself; on every later change it is the one that
  // restarts the pollers. Reading `$monitorStore.refreshMs` here (rather than
  // inside onMount) is what makes it reactive.
  let resourcePollMs = DEFAULT_REFRESH_MS;
  let resourcePollMounted = false;
  $: if ($monitorStore.refreshMs !== resourcePollMs) {
    resourcePollMs = $monitorStore.refreshMs;
    if (resourcePollMounted) startResourcePollers(resourcePollMs);
  }

  onMount(() => {
    const timeInterval = setInterval(() => {
      now = Date.now();
      currentTime = new Date();
    }, 1000);

    // Connect WebSocket
    websocket.connect();

    resourcePollMounted = true;
    startResourcePollers($monitorStore.refreshMs);

    // RAID monitor polling
    const raidInterval = setInterval(async () => {
      const result = await api.getRaidArrays();
      if (result.success && result.data) {
        raidData = { status: 'ready', data: translateRaidData(result.data) };
      } else {
        raidData = { status: 'error', error: result.error || 'Failed to load RAID data' };
      }
    }, 5000);

    // Security monitor polling — antivirus/firewall/ransomware change rarely
    // compared to CPU/RAM, so a longer interval is enough.
    loadSecurityData();
    const securityInterval = setInterval(loadSecurityData, 15000);

    // VPN / Downloader / Syncthing monitor polling
    loadVpnData();
    const vpnInterval = setInterval(loadVpnData, 15000);
    loadDownloaderData();
    const downloaderInterval = setInterval(loadDownloaderData, 5000);
    loadSyncthingData();
    const syncthingInterval = setInterval(loadSyncthingData, 15000);
    loadDockerData();
    const dockerInterval = setInterval(loadDockerData, 10000);
    // Refresh the catalog (installed / running state) on a slower cadence —
    // it shells out to `docker compose ps` per app. Also refreshed on demand
    // when the Launchpad opens (toggleAppsCenter).
    const dockerCatalogInterval = setInterval(loadDockerCatalog, 30000);

    // Weather monitor polling — a real forecast doesn't change meaningfully
    // faster than this, and Open-Meteo is a third-party service being
    // called directly from the browser, not our own backend, so this stays
    // deliberately much less chatty than the local-backend monitors above.
    // The initial load itself happens via the `$: if ($weatherLocationStore)`
    // reactive statement near loadWeatherData's own definition, not here —
    // that same statement is also what makes this panel refresh right away
    // when the location is changed from Settings while this desktop is
    // already open, instead of waiting for the next 10-minute tick.
    const weatherInterval = setInterval(loadWeatherData, 10 * 60 * 1000);

    // Active Sessions monitor polling — same cadence as the other
    // local-backend monitors (Security/VPN/Syncthing) above.
    loadSessionsData();
    const sessionsInterval = setInterval(loadSessionsData, 15000);

    // Health monitor polling — same cadence as Security (checks change
    // slowly relative to CPU/RAM).
    loadHealthData();
    const healthInterval = setInterval(loadHealthData, 15000);

    // Notifications polling
    async function loadNotifications() {
      const result = await api.getNotifications();
      if (result.success && result.data) {
        notificationsStore.setFromServer(result.data);
      }
    }
    loadNotifications();
    const notificationsInterval = setInterval(loadNotifications, 30000);

    // Connection type polling (wired vs. wireless icon in the topbar)
    async function loadConnectionType() {
      const result = await api.getNetworkSummary();
      if (result.success && result.data) {
        connectionType = result.data.connection_type || '';
      }
    }
    loadConnectionType();
    const connectionTypeInterval = setInterval(loadConnectionType, 30000);

    loadDockerCatalog();

    // Initial RAID load (cpu/ram/temp/network are already covered by the
    // immediate load built into subscribeWidget() above).
    (async () => {
      const raidResult = await api.getRaidArrays();
      if (raidResult.success && raidResult.data) {
        raidData = { status: 'ready', data: translateRaidData(raidResult.data) };
      } else {
        raidData = { status: 'error', error: raidResult.error || 'Failed to load RAID data' };
      }
    })();

    // Global keyboard shortcuts
    function handleGlobalKeydown(e: KeyboardEvent) {
      if ((e.ctrlKey || e.metaKey) && e.key === 'k') {
        e.preventDefault();
        openSearch();
      }
      if (e.key === 'Escape' && showActivityPanel) {
        showActivityPanel = false;
      }
    }

    window.addEventListener('keydown', handleGlobalKeydown);

    // Let an in-window app ask the desktop to open another app (e.g. the
    // File Explorer's "Share this folder" → opens the Shares app).
    function handleOpenAppRequest(e: Event) {
      const id = (e as CustomEvent).detail?.id;
      if (!id) return;
      const app = apps.find((a) => a.id === id);
      if (app) openApp(app);
    }
    window.addEventListener('hv-open-app', handleOpenAppRequest as EventListener);

    // Background-activity: the `activity` store owns the poll now (see
    // `activityStore.ts`) — it feeds `activeJobCount` + the taskbar reactively.

    // Idle-dim bookkeeping. markActivity is a single timestamp write (plus a
    // one-off flag flip when coming back from dimmed), so even pointermove is
    // cheap here; a slow timer does the actual dim decision.
    window.addEventListener('pointermove', markActivity, { passive: true });
    window.addEventListener('pointerdown', markActivity, { passive: true });
    window.addEventListener('keydown', markActivity, { passive: true });
    window.addEventListener('wheel', markActivity, { passive: true });
    const idleInterval = setInterval(() => {
      const menusOpen =
        showUserMenu || showNotifications || showConnectionInfo || showPowerModal ||
        showAppsCenter || showActivityPanel;
      if (menusOpen) {
        activityState.at = Date.now();
        return;
      }
      const shouldDim = Date.now() - activityState.at >= IDLE_DIM_MS;
      if (shouldDim !== chromeIdle) chromeIdle = shouldDim;
    }, 5000);

    // Let the first render + initial data settle before the reactive
    // one-shots (bell swing, attention bounce) are allowed to fire.
    const readyTimer = setTimeout(() => (chromeReady = true), 1500);

    return () => {
      clearInterval(timeInterval);
      resourcePollMounted = false;
      unsubResource.forEach((unsub) => unsub());
      unsubResource = [];
      clearInterval(raidInterval);
      clearInterval(securityInterval);
      clearInterval(vpnInterval);
      clearInterval(downloaderInterval);
      clearInterval(syncthingInterval);
      clearInterval(dockerInterval);
      clearInterval(dockerCatalogInterval);
      clearInterval(weatherInterval);
      clearInterval(sessionsInterval);
      clearInterval(healthInterval);
      clearInterval(notificationsInterval);
      clearInterval(connectionTypeInterval);
      clearInterval(idleInterval);
      clearTimeout(readyTimer);
      window.removeEventListener('keydown', handleGlobalKeydown);
      window.removeEventListener('hv-open-app', handleOpenAppRequest as EventListener);
      window.removeEventListener('pointermove', markActivity);
      window.removeEventListener('pointerdown', markActivity);
      window.removeEventListener('keydown', markActivity);
      window.removeEventListener('wheel', markActivity);
      if (chromePeekListening) window.removeEventListener('pointermove', onChromePeekMove);
      if (dockPeekRehideTimer) clearTimeout(dockPeekRehideTimer);
      if (topbarPeekRehideTimer) clearTimeout(topbarPeekRehideTimer);
      websocket.disconnect();
    };
  });

  function openApp(app: { id: string; name: string; icon: string; color: string; component?: any; isDockerApp?: boolean; url?: string }) {
    showAppsCenter = false;
    showActivityPanel = false;
    showUserMenu = showConnectionInfo = showNotifications = false;
    // Docker catalog apps never open as an in-app window — the Launchpad
    // routes their click straight to openExternalUrl (new tab / new window).
    if (app.isDockerApp) return;
    // Already open? Just focus it — don't stack another window.
    if (focusExistingWindow(app.id)) return;
    if (app.component) {
      windowManager.openWindow(app.id, app.name, app.icon, app.component, appIconBg(app.id, app.color));
    }
  }

  /** Open a URL (a Docker catalog app's web UI) in a new browser tab or a
   *  real new window — the popup feature string is what makes the browser
   *  spawn a window rather than a tab. Mirrors Docker.svelte's
   *  openContainerUrl. */
  function openExternalUrl(url: string, mode: 'tab' | 'window') {
    if (mode === 'window') {
      const w = 1280;
      const h = 800;
      const left = Math.max(0, Math.round((window.screen.width - w) / 2));
      const top = Math.max(0, Math.round((window.screen.height - h) / 2));
      window.open(url, '_blank', `popup,noopener,noreferrer,width=${w},height=${h},left=${left},top=${top}`);
    } else {
      window.open(url, '_blank', 'noopener,noreferrer');
    }
  }

  function openAccount() {
    showUserMenu = false;
    if (focusExistingWindow('account')) return;
    windowManager.openWindow('account', translate('account.heading'), 'user', Account, 'var(--hv-primary)');
  }

  async function handleLogout() {
    await authStore.logout();
    window.history.pushState({}, '', '/login');
    window.dispatchEvent(new PopStateEvent('popstate'));
  }

  // Matches the LOCALE_MAP pattern already used in Chat.svelte/Dashboard.svelte/Restic.svelte.
  const LOCALE_MAP: Record<string, string> = { en: 'en-US', fr: 'fr-FR', de: 'de-DE', es: 'es-ES' };

  $: calendarMonthLabel = calendarNow.toLocaleDateString(LOCALE_MAP[$languageStore] || 'en-US', {
    month: 'long',
    year: 'numeric',
  });

  function formatTime(date: Date) {
    return date.toLocaleTimeString(LOCALE_MAP[$languageStore] || 'en-US', {
      hour: '2-digit',
      minute: '2-digit',
      second: '2-digit',
      hour12: false
    });
  }

  function formatDate(date: Date) {
    return date.toLocaleDateString(LOCALE_MAP[$languageStore] || 'en-US', {
      weekday: 'short',
      month: 'short',
      day: 'numeric',
      year: 'numeric'
    });
  }

  function handleCloseWindow(id: string) {
    const win = $windowManager.find(w => w.id === id);
    windowManager.closeWindow(id);
    // Installed/uninstalled apps only need to be picked up once the Docker
    // app itself is closed — avoids adding a new polling interval.
    if (win?.appId === 'docker') {
      loadDockerCatalog();
    }
  }

  function handleMinimizeWindow(id: string) {
    windowManager.minimizeWindow(id);
  }

  function handleMaximizeWindow(id: string) {
    windowManager.maximizeWindow(id);
  }

  function handleFocusWindow(id: string) {
    windowManager.focusWindow(id);
  }

  function handleRestoreWindow(id: string) {
    windowManager.restoreWindow(id);
  }

  function handleUpdatePosition(id: string, x: number, y: number) {
    windowManager.updateWindowPosition(id, x, y);
  }

  function handleUpdateSize(id: string, width: number, height: number) {
    windowManager.updateWindowSize(id, width, height);
  }

  let searchIndex: SearchableItem[] = [];

  // Build a flat search index from apps and quick actions so the global SearchBar can find everything
  $: searchIndex = (apps.map(app => ({
    id: `app-${app.id}`,
    type: 'app' as const,
    title: app.name,
    description: $t('desktop.search_open_app_desc', { name: app.name }),
    icon: app.icon,
    color: app.color,
    keywords: [app.name.toLowerCase(), app.id],
    action: () => openApp(app)
  })) as SearchableItem[]).concat([
    {
      id: 'action-terminal',
      type: 'action' as const,
      title: $t('desktop.search_terminal_title'),
      description: $t('desktop.search_terminal_desc'),
      icon: 'terminal',
      color: 'var(--hv-primary)',
      keywords: $t('desktop.search_terminal_keywords').split(',').map(k => k.trim()),
      action: () => openApp(terminalApp)
    },
    {
      id: 'action-settings',
      type: 'action' as const,
      title: $t('desktop.search_settings_title'),
      description: $t('desktop.search_settings_desc'),
      icon: 'settings',
      color: 'var(--hv-primary)',
      keywords: $t('desktop.search_settings_keywords').split(',').map(k => k.trim()),
      action: () => openApp(settingsApp)
    },
    {
      id: 'action-wallpaper',
      type: 'setting' as const,
      title: $t('desktop.search_wallpaper_title'),
      description: $t('desktop.search_wallpaper_desc'),
      icon: 'wallpaper',
      color: 'var(--hv-primary)',
      keywords: $t('desktop.search_wallpaper_keywords').split(',').map(k => k.trim()),
      action: () => openApp(settingsApp)
    },
    {
      id: 'action-chat',
      type: 'action' as const,
      title: $t('desktop.search_chat_title'),
      description: $t('desktop.search_chat_desc'),
      icon: 'chat',
      color: 'var(--hv-primary)',
      keywords: $t('desktop.search_chat_keywords').split(',').map(k => k.trim()),
      action: () => openApp(apps.find(a => a.id === 'chat')!)
    }
  ]);

  function toggleAppsCenter() {
    showAppsCenter = !showAppsCenter;
    if (showAppsCenter) {
      showActivityPanel = false;
      showUserMenu = showConnectionInfo = showNotifications = false;
      // Pick up any Docker app installed / removed / started / stopped
      // since the desktop loaded, so its Launchpad tile is right.
      loadDockerCatalog();
    }
  }

  // paper wallpaper — soft slate-paper gradient + faint light facets (the
  // "Clean Paper & Slate" default; mirrored in Login.svelte's .hv-login-page)
  const defaultDesktopGradient = `linear-gradient(135deg, #f4f6fb 0%, #eaeef7 32%, #eef2f9 58%, #e2e8f4 82%, #dae2f1 100%)`;

  // Derive the desktop background from the wallpaper store. A bundled photo
  // is shown with two `cover` layers: a blurred, zoomed copy fills every
  // corner with no flat-colour bands, and a sharp copy on top covers the
  // viewport edge-to-edge (no bands). The built-in faceted gradient (+ its
  // clip-path layers) is legacy-only now (see wallpaper.ts) but stays as
  // a fallback.
  $: usePhotoWallpaper = $wallpaperStore.type === 'image';
  // Narrowed here so the template can read the URL without a type error —
  // `usePhotoWallpaper` alone doesn't narrow the store's union for TS.
  $: photoWallpaperUrl = $wallpaperStore.type === 'image' ? $wallpaperStore.value : '';
  $: wallpaperStyle = usePhotoWallpaper ? '' : `background: ${defaultDesktopGradient};`;
</script>

<svelte:window
  bind:innerWidth={viewportWidth}
  bind:innerHeight={viewportHeight}
  onclick={() => { showUserMenu = false; showAppsCenter = false; showNotifications = false; showConnectionInfo = false; showActivityPanel = false; }}
/>

<!-- Global Search Bar -->
<SearchBar {searchIndex} />

<div class="hv-desktop-container" style={wallpaperStyle}>
  {#if usePhotoWallpaper}
    <!-- Blurred, zoomed `cover` copy: fills every corner of the viewport,
         whatever its aspect ratio, so there's never a flat-colour band. -->
    <div class="hv-wallpaper-photo-blur" style="background-image: url('{photoWallpaperUrl}');"></div>
    <!-- The same photo again, `contain`-sized on top: always shows the
         whole image uncropped, centred over the blurred fill. -->
    <div class="hv-wallpaper-photo-sharp" style="background-image: url('{photoWallpaperUrl}');"></div>
  {:else}
    <!-- faceted wallpaper layers — only over the built-in gradient, never over a photo -->
    <div class="hv-wallpaper-layer hv-wallpaper-layer-1"></div>
    <div class="hv-wallpaper-layer hv-wallpaper-layer-2"></div>
    <div class="hv-wallpaper-layer hv-wallpaper-layer-3"></div>
  {/if}
  <!-- Top Bar (floating pill) -->
  <header
    class="hv-menubar"
    class:hv-chrome--idle={chromeIdle}
    class:hv-menubar--hidden={menubarHidden}
    bind:this={menubarEl}
  >
    {#if activeJobCount > 0}
      <div
        class="hv-menubar-activity"
        role="progressbar"
        aria-busy="true"
        aria-valuemin="0"
        aria-valuemax="100"
        aria-label={$t('desktop.background_activity')}
      >
        <span class="hv-menubar-activity-bar"></span>
      </div>
    {/if}
    <div class="hv-menubar-inner">
      <!-- User pill -->
      <button class="hv-user-pill" onclick={(e)=>toggleTopbarPanel('user', e)}>
        <div class="hv-user-pill-avatar">
          <UserAvatar username={$authStore.user?.username ?? ''} seed={$authStore.user?.avatar_seed ?? null} size={62} />
        </div>
        <span class="hv-user-pill-name">{$authStore.user?.username || $t('desktop.user_role')}</span>
      </button>

      <span class="hv-menubar-divider"></span>

      <!-- Clock -->
      <div class="hv-menubar-clock">
        <span class="hv-menubar-time">
          {#each formatTime(currentTime) as char, i (char + i)}
            {#if /\d/.test(char)}
              <span class="hv-clock-digit"><span class="hv-clock-digit-inner">{char}</span></span>
            {:else}
              <span class="hv-clock-sep">{char}</span>
            {/if}
          {/each}
        </span>
        <span class="hv-menubar-date">{formatDate(currentTime)}</span>
      </div>

      <span class="hv-menubar-divider"></span>

      <!-- Connection Status -->
      <div class="hv-connection-wrap">
        <button
          class="hv-topbar-btn"
          data-testid="topbar-network"
          onclick={(e) => toggleTopbarPanel('net', e)}
          aria-label={$t('desktop.app_network_name')}
          title={$t('desktop.app_network_name')}
        >
          {#if connectionType === 'WiFi'}
            <WifiIcon size={26} />
          {:else}
            <EthernetIcon size={26} />
          {/if}
          <span class="hv-topbar-btn-label">{$t('desktop.app_network_name')}</span>
        </button>

        {#if showConnectionInfo}
          <!-- svelte-ignore a11y_no_static_element_interactions a11y_click_events_have_key_events -->
          <div class="hv-net-dropdown hv-topbar-panel" onclick={(e) => e.stopPropagation()}>
            <div class="hv-net-body">
              <!-- Local IP — periwinkle accent -->
              <div class="hv-net-ip-row hv-net-ip-local">
                <div class="hv-net-ip-icon-tile">
                   <IconSignal3g size={22} />
                </div>
                <div class="hv-net-ip-info">
                  <span class="hv-net-ip-label">{$t('network.conn_local_ip')}</span>
                  <span class="hv-net-ip-value">{connIpLoading ? '…' : (connLocalIp || $t('desktop.ip_unavailable'))}</span>
                </div>
                <button class="hv-net-copy" onclick={() => copyToClipboard(connLocalIp)} aria-label={$t('common.copy')}>
                  <IconCopy size={18} />
                </button>
              </div>

              <!-- Public IP — neutral -->
              <div class="hv-net-ip-row hv-net-ip-public">
                <div class="hv-net-ip-icon-tile hv-net-ip-icon-public">
                  <IconGlobe size={22} />
                </div>
                <div class="hv-net-ip-info">
                  <span class="hv-net-ip-label">{$t('network.conn_public_ip')}</span>
                  <span class="hv-net-ip-value">{connIpLoading ? '…' : (connPublicIp || $t('desktop.ip_unavailable'))}</span>
                </div>
                <button class="hv-net-copy" onclick={() => copyToClipboard(connPublicIp)} aria-label={$t('common.copy')}>
                  <IconCopy size={18} />
                </button>
              </div>

              <!-- Server location — only shown when the geo lookup succeeded -->
              {#if serverLocation}
                <div class="hv-net-ip-row hv-net-loc-row">
                  <div class="hv-net-ip-icon-tile hv-net-ip-icon-loc">
                    <IconMapPin size={22} />
                  </div>
                  <div class="hv-net-ip-info">
                    <span class="hv-net-ip-label">{$t('desktop.net_location')}</span>
                    <span class="hv-net-ip-value hv-net-loc-value">
                      {#if serverLocation.flag}<span class="hv-net-loc-flag">{serverLocation.flag}</span>{/if}{serverLocation.label}
                    </span>
                  </div>
                </div>
              {/if}
            </div>
          </div>
        {/if}
      </div>

      <!-- Notifications — a single button (the panel anchors to the
           menubar pill, not to a wrapper). The count badge stays a child of
           the BUTTON, not of the bell span: the mockup pins it to the button's
           top-right corner, and the button is the chip whose corner that is
           once the label appears at 901px — see rule 12 for why the badge can
           never land on the label. -->
      <div class="hv-notifications-trigger">
        <button class="hv-topbar-btn hv-notif-btn" data-testid="topbar-notifications" class:hv-bell-swing={bellSwing} onclick={(e)=>toggleTopbarPanel('notifications', e)} aria-label={$t('desktop.notifications')} title={$t('desktop.notifications')}>
          <span class="hv-notif-bell-icon">
            <NotificationBellIcon size={26} stroke={2} />
          </span>
          <span class="hv-topbar-btn-label">{$t('desktop.notifications')}</span>
          {#if $notificationsStore.unreadCount > 0}
            <span class="hv-notification-badge">{$notificationsStore.unreadCount > 99 ? '99+' : $notificationsStore.unreadCount}</span>
          {/if}
        </button>
        <NotificationCenter isOpen={showNotifications} />
      </div>

      <!-- Power — stays LAST: the label reuses the orphan `desktop.power`
           key, while aria-label/title keep the longer `power_options` (the
           accessible name must contain the visible label — WCAG 2.5.3). -->
      <button class="hv-topbar-btn" data-testid="topbar-power" onclick={() => (showPowerModal = true)} aria-label={$t('desktop.power_options')} title={$t('desktop.power_options')}>
        <IconPower size={26} stroke={2} />
        <span class="hv-topbar-btn-label">{$t('desktop.power')}</span>
      </button>

      <!-- User Menu Dropdown -->
      {#if showUserMenu}
        <!-- svelte-ignore a11y_no_static_element_interactions a11y_click_events_have_key_events -->
        <div class="hv-user-dropdown hv-topbar-panel" onclick={(e) => e.stopPropagation()}>
          <div class="hv-user-info">
            <div class="hv-user-avatar">
              <UserAvatar username={$authStore.user?.username ?? ''} seed={$authStore.user?.avatar_seed ?? null} size={76} />
            </div>
            <div class="hv-user-details">
              <div class="hv-user-details-name">{($authStore.user?.fullname || $authStore.user?.username) ?? ''}</div>
              <div class="hv-user-details-role">{$authStore.user?.is_admin ? $t('desktop.administrator') : $t('desktop.user_role')}</div>
            </div>
          </div>
          <div class="hv-user-menu-divider"></div>
          <button class="hv-user-menu-item" onclick={openAccount}>
            <IconUser size={26} stroke={1.7} />
            <span>{$t('account.heading')}</span>
          </button>
          <div class="hv-user-menu-divider"></div>
          <button class="hv-user-menu-item hv-logout" onclick={handleLogout}>
            <IconLogout size={26} stroke={1.8} />
            <span>{$t('desktop.logout')}</span>
          </button>
        </div>
      {/if}
    </div>
  </header>

  <!-- Resource Monitor (draggable) -->
  {#if $monitorStore.resourceMonitorVisible}
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div
      class="hv-resource-monitor"
      class:dragging={draggingMonitor === 'resource'}
      style="left: {draggingMonitor === 'resource' ? monitorCurrentX : resourceMonitorClamped.x}px; top: {draggingMonitor === 'resource' ? monitorCurrentY : resourceMonitorClamped.y}px;"
      onpointerdown={(e) => handleMonitorDragStart(e, 'resource')}
    >
      <WidgetHeader
        color={MONITOR_COLOR.resource}
        label={$t('desktop.resources')}
      >
        {#snippet icon()}<IconActivity size={16} stroke={2} />{/snippet}
      </WidgetHeader>

      <div class="hv-gauge-row">
        <div class="hv-gauge">
          <svg width="72" height="72" viewBox="0 0 72 72">
            <circle class="hv-gauge-track" cx="36" cy="36" r="30" />
            <circle
              class="hv-gauge-fill hv-gauge-fill-cpu"
              cx="36" cy="36" r="30"
              stroke-dasharray="188.5"
              stroke-dashoffset={188.5 - (cpuData.status === 'ready' ? cpuData.data.usage : 0) / 100 * 188.5}
            />
            <text x="36" y="43" class="hv-gauge-value hv-gauge-value-cpu">
              {#if cpuData.status === 'ready'}
                {cpuData.data.usage.toFixed(0)}%
              {:else if cpuData.status === 'loading'}
                ...
              {:else}
                --
              {/if}
            </text>
          </svg>
          <div class="hv-gauge-label">
            <IconCpu size={12} color="var(--hv-info)" stroke={2} />
            <span>{$t('desktop.cpu')}</span>
          </div>
        </div>

        <div class="hv-gauge">
          <svg width="72" height="72" viewBox="0 0 72 72">
            <circle class="hv-gauge-track" cx="36" cy="36" r="30" />
            <circle
              class="hv-gauge-fill hv-gauge-fill-ram"
              cx="36" cy="36" r="30"
              stroke-dasharray="188.5"
              stroke-dashoffset={188.5 - (ramData.status === 'ready' ? ramData.data.usage_percent : 0) / 100 * 188.5}
            />
            <text x="36" y="43" class="hv-gauge-value hv-gauge-value-ram">
              {#if ramData.status === 'ready'}
                {ramData.data.usage_percent.toFixed(0)}%
              {:else if ramData.status === 'loading'}
                ...
              {:else}
                --
              {/if}
            </text>
          </svg>
          <div class="hv-gauge-label">
            <IconServer size={12} color="var(--hv-primary)" stroke={2} />
            <span>{$t('desktop.ram')}</span>
          </div>
        </div>

        <div class="hv-gauge">
          <svg width="72" height="72" viewBox="0 0 72 72">
            <circle class="hv-gauge-track" cx="36" cy="36" r="30" />
            <circle
              class="hv-gauge-fill hv-gauge-fill-temp"
              class:high={tempData.status === 'ready' && tempData.data.temp >= 80}
              cx="36" cy="36" r="30"
              stroke-dasharray="188.5"
              stroke-dashoffset={188.5 - (tempData.status === 'ready' ? Math.min(tempData.data.temp / 100 * 100, 100) : 0) / 100 * 188.5}
            />
            <text x="36" y="43" class="hv-gauge-value hv-gauge-value-temp">
              {#if tempData.status === 'ready'}
                {tempData.data.temp.toFixed(0)}°
              {:else if tempData.status === 'loading'}
                ...
              {:else}
                --
              {/if}
            </text>
          </svg>
          <div class="hv-gauge-label">
            <IconThermometer size={12} color="var(--hv-warning)" stroke={2} />
            <span>{$t('desktop.temp')}</span>
          </div>
        </div>
      </div>

      <div class="hv-resource-network">
        <div class="hv-network-item">
          <div class="hv-network-label">
            <IconArrowUp size={12} stroke={2} />
            <span>{$t('desktop.upload')}</span>
          </div>
          <div class="hv-network-value">
            {#if networkData.status === 'ready'}
              {networkData.data.tx_mbps.toFixed(2)} Mbps
            {:else}
              --
            {/if}
          </div>
        </div>
        <div class="hv-network-item">
          <div class="hv-network-label">
            <IconArrowDown size={12} stroke={2} />
            <span>{$t('desktop.download')}</span>
          </div>
          <div class="hv-network-value">
            {#if networkData.status === 'ready'}
              {networkData.data.rx_mbps.toFixed(2)} Mbps
            {:else}
              --
            {/if}
          </div>
        </div>
      </div>

      <div class="hv-system-update" class:available={systemUpdateAvailable}>
        <svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
          <path d="M21 12a9 9 0 0 0-9-9 9.75 9.75 0 0 0-6.74 2.74L3 8"/>
          <path d="M3 3v5h5"/>
          <path d="M3 12a9 9 0 0 0 9 9 9.75 9.75 0 0 0 6.74-2.74L21 16"/>
          <path d="M16 16h5v5"/>
        </svg>
        <span class="hv-system-update-text">
          {#if systemUpdateAvailable}
            {$t('desktop.system_update_available')}
          {:else}
            {$t('desktop.system_up_to_date')}
          {/if}
        </span>
      </div>
    </div>
  {/if}

  <!-- RAID Monitor (draggable) -->
  {#if $monitorStore.raidMonitorVisible}
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div
      class="hv-raid-monitor"
      class:dragging={draggingMonitor === 'raid'}
      style="left: {draggingMonitor === 'raid' ? monitorCurrentX : raidMonitorClamped.x}px; top: {draggingMonitor === 'raid' ? monitorCurrentY : raidMonitorClamped.y}px;"
      onpointerdown={(e) => handleMonitorDragStart(e, 'raid')}
    >
      <WidgetHeader
        color={MONITOR_COLOR.raid}
        label={$t('desktop.raid_arrays')}
      >
        {#snippet icon()}<IconDatabase size={16} stroke={2} />{/snippet}
      </WidgetHeader>

      {#if raidData.status === 'ready' && raidData.data.arrays && raidData.data.arrays.length > 0}
        <!-- RAID Arrays -->
        <div class="hv-raid-list">
          {#each raidData.data.arrays as array}
            <div class="hv-raid-item">
              <div class="hv-raid-header">
                <div class="hv-raid-name">
                  <IconServer size={18} stroke={2} />
                  <span class="hv-raid-name-text">{array.name}</span>
                </div>
                <span class="hv-raid-type">{$t('desktop.raid_level', { level: array.level })}</span>
              </div>
              <div class="hv-raid-info">
                <div class="hv-raid-status hv-raid-status-{array.status}">
                  <span class="hv-raid-status-dot"></span>
                  <span>{$t(`desktop.status_${array.status}`)}</span>
                </div>
                <div class="hv-raid-info-value">{array.size}</div>
              </div>
              <div class="hv-raid-progress">
                <div class="hv-raid-progress-label">
                  <span class="hv-raid-progress-used">{array.used} {$t('desktop.used')}</span>
                  <span class="hv-raid-progress-free">{array.free} {$t('desktop.free')}</span>
                </div>
                <div class="hv-raid-progress-bar">
                  <div class="hv-raid-progress-fill" style="width: {array.used_percent}%"></div>
                </div>
              </div>
            </div>
          {/each}
        </div>

        <!-- Total Storage -->
        <div class="hv-raid-total">
          <div class="hv-raid-total-label">{$t('desktop.total_storage')}</div>
          <div class="hv-raid-total-values">
            <div class="hv-raid-total-used">{raidData.data.total_used}</div>
            <div class="hv-raid-total-capacity">{$t('desktop.of_capacity', { capacity: raidData.data.total_capacity })}</div>
          </div>
        </div>
      {:else if raidData.status === 'loading'}
        <div class="hv-raid-empty">{$t('desktop.loading_raid')}</div>
      {:else if raidData.status === 'error'}
        <div class="hv-raid-empty">{$t('desktop.error_loading_raid')}</div>
      {:else}
        <div class="hv-raid-empty">
          <div class="hv-raid-empty-text">{$t('desktop.no_raid_created')}</div>
          <button class="hv-raid-create-btn" onclick={() => openApp(apps.find(a => a.id === 'raid')!)}>
            <IconPlus size={14} stroke={2} />
            {$t('desktop.create_raid')}
          </button>
        </div>
      {/if}
    </div>
  {/if}

  <!-- Security Monitor (draggable) -->
  {#if $monitorStore.securityMonitorVisible}
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div
      class="hv-security-monitor"
      class:dragging={draggingMonitor === 'security'}
      style="left: {draggingMonitor === 'security' ? monitorCurrentX : securityMonitorClamped.x}px; top: {draggingMonitor === 'security' ? monitorCurrentY : securityMonitorClamped.y}px;"
      onpointerdown={(e) => handleMonitorDragStart(e, 'security')}
    >
      <WidgetHeader
        color={MONITOR_COLOR.security}
        label={$t('desktop.security_monitor')}
      >
        {#snippet icon()}<IconShieldCheckered size={16} stroke={2} />{/snippet}
      </WidgetHeader>

      {#if securityData.status === 'loading' || securityData.status === 'idle'}
        <div class="hv-security-empty">{$t('desktop.loading_security')}</div>
      {:else if securityData.status === 'error'}
        <div class="hv-security-empty">{$t('desktop.error_loading_security')}</div>
      {:else if securityData.status === 'ready'}
        <div class="hv-security-list">
          <!-- Antivirus -->
          <div class="hv-security-item">
            <div class="hv-security-item-header">
              <div class="hv-security-item-name">
                <IconBug size={15} stroke={2} />
                <span>{$t('desktop.antivirus')}</span>
              </div>
              <div class="hv-security-status" class:hv-security-status-ok={securityData.data.avThreatCount === 0} class:hv-security-status-warn={securityData.data.avThreatCount > 0}>
                <span class="hv-security-status-dot"></span>
                <span>
                  {#if securityData.data.avThreatCount > 0}
                    {$t('desktop.av_threats_found', { count: String(securityData.data.avThreatCount) })}
                  {:else}
                    {$t('desktop.av_no_threats')}
                  {/if}
                </span>
              </div>
            </div>
            <div class="hv-security-item-detail">
              {#if securityData.data.avLastScan}
                {$t('desktop.av_last_scan', { date: formatShortDate(securityData.data.avLastScan.finishedAt ?? securityData.data.avLastScan.startedAt) })}
              {:else}
                {$t('desktop.av_no_scans')}
              {/if}
            </div>
          </div>

          <!-- Firewall -->
          <div class="hv-security-item">
            <div class="hv-security-item-header">
              <div class="hv-security-item-name">
                <IconShield size={15} stroke={2} />
                <span>{$t('desktop.firewall')}</span>
              </div>
              <div
                class="hv-security-status"
                class:hv-security-status-ok={securityData.data.fwAvailable && securityData.data.fwEnabled}
                class:hv-security-status-warn={securityData.data.fwAvailable && !securityData.data.fwEnabled}
                class:hv-security-status-off={!securityData.data.fwAvailable}
              >
                <span class="hv-security-status-dot"></span>
                <span>
                  {#if !securityData.data.fwAvailable}
                    {$t('desktop.fw_unavailable')}
                  {:else if securityData.data.fwEnabled}
                    {$t('desktop.fw_active')}
                  {:else}
                    {$t('desktop.fw_inactive')}
                  {/if}
                </span>
              </div>
            </div>
            {#if securityData.data.fwAvailable}
              <div class="hv-security-item-detail">
                {$t('desktop.fw_rules', { count: String(securityData.data.fwRuleCount) })}
              </div>
            {/if}
          </div>

          <!-- Anti-Ransomware -->
          <div class="hv-security-item">
            <div class="hv-security-item-header">
              <div class="hv-security-item-name">
                <IconLock size={15} stroke={2} />
                <span>{$t('desktop.anti_ransomware')}</span>
              </div>
              <div class="hv-security-status" class:hv-security-status-ok={!securityData.data.ransomLastEvent} class:hv-security-status-warn={!!securityData.data.ransomLastEvent}>
                <span class="hv-security-status-dot"></span>
                <span>
                  {#if securityData.data.ransomLastEvent}
                    {$t('desktop.ransom_last_event', { type: securityData.data.ransomLastEvent.eventType })}
                  {:else}
                    {$t('desktop.ransom_no_events')}
                  {/if}
                </span>
              </div>
            </div>
            <div class="hv-security-item-detail">
              {$t('desktop.ransom_protected_paths', { count: String(securityData.data.ransomProtectedCount) })}
            </div>
          </div>
        </div>
      {/if}
    </div>
  {/if}

  <!-- VPN Monitor (draggable) -->
  {#if $monitorStore.vpnMonitorVisible}
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div
      class="hv-vpn-monitor"
      class:dragging={draggingMonitor === 'vpn'}
      style="left: {draggingMonitor === 'vpn' ? monitorCurrentX : vpnMonitorClamped.x}px; top: {draggingMonitor === 'vpn' ? monitorCurrentY : vpnMonitorClamped.y}px;"
      onpointerdown={(e) => handleMonitorDragStart(e, 'vpn')}
    >
      <WidgetHeader
        color={MONITOR_COLOR.vpn}
        label={$t('desktop.vpn_monitor')}
      >
        {#snippet icon()}<IconLock size={16} stroke={2} />{/snippet}
      </WidgetHeader>

      {#if vpnData.status === 'loading' || vpnData.status === 'idle'}
        <div class="hv-vpn-empty">{$t('desktop.loading_vpn')}</div>
      {:else if vpnData.status === 'error'}
        <div class="hv-vpn-empty">{$t('desktop.error_loading_vpn')}</div>
      {:else if vpnData.status === 'ready'}
        <div class="hv-vpn-list">
          <div class="hv-vpn-row">
            <span class="hv-vpn-row-label">WireGuard</span>
            <span class="hv-vpn-row-value">
              {#if vpnData.data.wireguardAvailable}
                {$t('desktop.vpn_active_count', { count: String(vpnData.data.wireguardActive) })}
              {:else}
                {$t('desktop.vpn_not_installed')}
              {/if}
            </span>
          </div>
          <div class="hv-vpn-row">
            <span class="hv-vpn-row-label">OpenVPN</span>
            <span class="hv-vpn-row-value">
              {#if vpnData.data.openvpnAvailable}
                {$t('desktop.vpn_active_count', { count: String(vpnData.data.openvpnActive) })}
              {:else}
                {$t('desktop.vpn_not_installed')}
              {/if}
            </span>
          </div>
        </div>
      {/if}
    </div>
  {/if}

  <!-- Downloader Monitor (draggable) -->
  {#if $monitorStore.downloaderMonitorVisible}
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div
      class="hv-downloader-monitor"
      class:dragging={draggingMonitor === 'downloader'}
      style="left: {draggingMonitor === 'downloader' ? monitorCurrentX : downloaderMonitorClamped.x}px; top: {draggingMonitor === 'downloader' ? monitorCurrentY : downloaderMonitorClamped.y}px;"
      onpointerdown={(e) => handleMonitorDragStart(e, 'downloader')}
    >
      <WidgetHeader
        color={MONITOR_COLOR.downloader}
        label={$t('desktop.downloader_monitor')}
      >
        {#snippet icon()}<IconDownload size={16} stroke={2} />{/snippet}
      </WidgetHeader>

      {#if downloaderData.status === 'loading' || downloaderData.status === 'idle'}
        <div class="hv-downloader-empty">{$t('desktop.loading_downloader')}</div>
      {:else if downloaderData.status === 'error'}
        <div class="hv-downloader-empty">{$t('desktop.error_loading_downloader')}</div>
      {:else if downloaderData.status === 'ready'}
        {#if !downloaderData.data.available}
          <div class="hv-downloader-empty">{$t('desktop.downloader_not_installed')}</div>
        {:else if !downloaderData.data.running}
          <div class="hv-downloader-empty">{$t('desktop.downloader_not_running')}</div>
        {:else}
          <div class="hv-downloader-active">{$t('desktop.downloader_active_count', { count: String(downloaderData.data.activeCount) })}</div>
          <div class="hv-downloader-speeds">
            <div class="hv-downloader-speed-item">
              <div class="hv-downloader-speed-label"><IconArrowDown size={12} stroke={2} /><span>{$t('desktop.download')}</span></div>
              <div class="hv-downloader-speed-value">{formatSpeed(downloaderData.data.downloadSpeed)}</div>
            </div>
            <div class="hv-downloader-speed-item">
              <div class="hv-downloader-speed-label"><IconArrowUp size={12} stroke={2} /><span>{$t('desktop.upload')}</span></div>
              <div class="hv-downloader-speed-value">{formatSpeed(downloaderData.data.uploadSpeed)}</div>
            </div>
          </div>
        {/if}
      {/if}
    </div>
  {/if}

  <!-- Syncthing Monitor (draggable) -->
  {#if $monitorStore.syncthingMonitorVisible}
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div
      class="hv-syncthing-monitor"
      class:dragging={draggingMonitor === 'syncthing'}
      style="left: {draggingMonitor === 'syncthing' ? monitorCurrentX : syncthingMonitorClamped.x}px; top: {draggingMonitor === 'syncthing' ? monitorCurrentY : syncthingMonitorClamped.y}px;"
      onpointerdown={(e) => handleMonitorDragStart(e, 'syncthing')}
    >
      <WidgetHeader
        color={MONITOR_COLOR.syncthing}
        label={$t('desktop.syncthing_monitor')}
      >
        {#snippet icon()}<IconRefresh size={16} stroke={2} />{/snippet}
      </WidgetHeader>

      {#if syncthingData.status === 'loading' || syncthingData.status === 'idle'}
        <div class="hv-syncthing-empty">{$t('desktop.loading_syncthing')}</div>
      {:else if syncthingData.status === 'error'}
        <div class="hv-syncthing-empty">{$t('desktop.error_loading_syncthing')}</div>
      {:else if syncthingData.status === 'ready'}
        {#if !syncthingData.data.available}
          <div class="hv-syncthing-empty">{$t('desktop.syncthing_not_installed')}</div>
        {:else}
          <div class="hv-syncthing-status" class:hv-syncthing-status-syncing={syncthingData.data.pendingFiles > 0}>
            <span class="hv-syncthing-status-dot"></span>
            <span>{syncthingData.data.pendingFiles > 0 ? $t('desktop.syncthing_syncing') : $t('desktop.syncthing_up_to_date')}</span>
          </div>
          <div class="hv-syncthing-detail">
            {$t('desktop.syncthing_summary', { folders: String(syncthingData.data.folderCount), files: String(syncthingData.data.pendingFiles) })}
          </div>
        {/if}
      {/if}
    </div>
  {/if}

  {#if $monitorStore.calendarMonitorVisible}
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div
      class="hv-calendar-monitor"
      class:dragging={draggingMonitor === 'calendar'}
      style="left: {draggingMonitor === 'calendar' ? monitorCurrentX : calendarMonitorClamped.x}px; top: {draggingMonitor === 'calendar' ? monitorCurrentY : calendarMonitorClamped.y}px;"
      onpointerdown={(e) => handleMonitorDragStart(e, 'calendar')}
    >
      <WidgetHeader
        color={MONITOR_COLOR.calendar}
        label={$t('desktop.calendar_monitor')}
      >
        {#snippet icon()}<IconCalendar size={16} stroke={2} />{/snippet}
      </WidgetHeader>

      <div class="hv-calendar-weekdays">
        {#each ['sun', 'mon', 'tue', 'wed', 'thu', 'fri', 'sat'] as wd}
          <div class="hv-calendar-weekday">{$t(`desktop.calendar_weekday_${wd}`)}</div>
        {/each}
      </div>
      <div class="hv-calendar-grid">
        {#each calendarDays as day (day.key)}
          <div
            class="hv-calendar-cell"
            class:hv-calendar-cell-empty={!day.inMonth}
            class:hv-calendar-cell-today={day.isToday}
          >
            {day.label}
          </div>
        {/each}
      </div>

      <div class="hv-calendar-nav">
        <button class="hv-calendar-nav-btn hv-ic" onclick={calendarGoToPrevYear} aria-label={$t('desktop.calendar_prev_year')} title={$t('desktop.calendar_prev_year')}>
          <IconChevronsLeft size={18} stroke={2} />
        </button>
        <button class="hv-calendar-nav-btn hv-ic" onclick={calendarGoToPrevMonth} aria-label={$t('desktop.calendar_prev_month')} title={$t('desktop.calendar_prev_month')}>
          <IconChevronLeft size={18} stroke={2} />
        </button>
        <button class="hv-calendar-nav-btn hv-calendar-nav-label hv-ic" onclick={calendarGoToToday} aria-label={$t('desktop.calendar_today')} title={$t('desktop.calendar_today')}>
          {calendarMonthLabel}
        </button>
        <button class="hv-calendar-nav-btn hv-ic" onclick={calendarGoToNextMonth} aria-label={$t('desktop.calendar_next_month')} title={$t('desktop.calendar_next_month')}>
          <IconChevronRight size={18} stroke={2} />
        </button>
        <button class="hv-calendar-nav-btn hv-ic" onclick={calendarGoToNextYear} aria-label={$t('desktop.calendar_next_year')} title={$t('desktop.calendar_next_year')}>
          <IconChevronsRight size={18} stroke={2} />
        </button>
      </div>
    </div>
  {/if}

  <!-- Docker Monitor (draggable) -->
  {#if $monitorStore.dockerMonitorVisible}
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div
      class="hv-docker-monitor"
      class:dragging={draggingMonitor === 'docker'}
      style="left: {draggingMonitor === 'docker' ? monitorCurrentX : dockerMonitorClamped.x}px; top: {draggingMonitor === 'docker' ? monitorCurrentY : dockerMonitorClamped.y}px;"
      onpointerdown={(e) => handleMonitorDragStart(e, 'docker')}
    >
      <WidgetHeader
        color={MONITOR_COLOR.docker}
        label={$t('desktop.docker_monitor')}
      >
        {#snippet icon()}<ContainerIcon size={16} stroke={2} />{/snippet}
      </WidgetHeader>

      {#if dockerData.status === 'loading' || dockerData.status === 'idle'}
        <div class="hv-docker-empty">{$t('desktop.loading_docker')}</div>
      {:else if dockerData.status === 'error'}
        <div class="hv-docker-empty">{$t('desktop.error_loading_docker')}</div>
      {:else if dockerData.status === 'ready'}
        {#if !dockerData.data.available}
          <div class="hv-docker-empty">{$t('desktop.docker_not_installed')}</div>
        {:else if dockerData.data.containers.length === 0}
          <div class="hv-docker-empty">{$t('desktop.docker_no_containers')}</div>
        {:else}
          <div class="hv-docker-list">
            {#each dockerData.data.containers as c (c.name)}
              <div class="hv-docker-item">
                <div class="hv-docker-item-info">
                  <span
                    class="hv-docker-item-dot"
                    class:hv-docker-dot-on={c.state === 'Running'}
                    class:hv-docker-dot-warn={c.state === 'Restarting' || c.state === 'Paused'}
                  ></span>
                  <span class="hv-docker-item-name" title={c.name}>{c.name}</span>
                </div>
                <div class="hv-docker-item-actions">
                  <button
                    class="hv-docker-action-btn hv-ic"
                    disabled={dockerActionBusy[c.name] || c.state === 'Running'}
                    onclick={() => dockerContainerAction(c.name, 'start')}
                    aria-label={$t('desktop.docker_start', { name: c.name })}
                    title={$t('desktop.docker_start', { name: c.name })}
                  >
                    <IconPlayerPlay size={20} stroke={2} />
                  </button>
                  <button
                    class="hv-docker-action-btn hv-ic"
                    disabled={dockerActionBusy[c.name] || c.state !== 'Running'}
                    onclick={() => dockerContainerAction(c.name, 'restart')}
                    aria-label={$t('desktop.docker_restart', { name: c.name })}
                    title={$t('desktop.docker_restart', { name: c.name })}
                  >
                    <IconRefresh size={20} stroke={2} />
                  </button>
                  <button
                    class="hv-docker-action-btn hv-ic"
                    disabled={dockerActionBusy[c.name] || c.state !== 'Running'}
                    onclick={() => dockerContainerAction(c.name, 'stop')}
                    aria-label={$t('desktop.docker_stop', { name: c.name })}
                    title={$t('desktop.docker_stop', { name: c.name })}
                  >
                    <IconSquare size={20} stroke={2} />
                  </button>
                </div>
              </div>
            {/each}
          </div>
        {/if}
      {/if}
    </div>
  {/if}

  {#if $monitorStore.weatherMonitorVisible}
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div
      class="hv-weather-monitor"
      class:dragging={draggingMonitor === 'weather'}
      style="left: {draggingMonitor === 'weather' ? monitorCurrentX : weatherMonitorClamped.x}px; top: {draggingMonitor === 'weather' ? monitorCurrentY : weatherMonitorClamped.y}px;"
      onpointerdown={(e) => handleMonitorDragStart(e, 'weather')}
    >
      <WidgetHeader
        color={MONITOR_COLOR.weather}
        label={$t('desktop.weather_monitor')}
      >
        {#snippet icon()}<IconSun size={16} stroke={2} />{/snippet}
      </WidgetHeader>

      {#if !$weatherLocationStore}
        <div class="hv-weather-empty hv-weather-empty-configure">
          <span>{$t('desktop.weather_no_location')}</span>
          <button class="hv-weather-open-settings-btn" onclick={() => openApp(settingsApp)}>
            {$t('desktop.weather_open_settings')}
          </button>
        </div>
      {:else if weatherData.status === 'loading' || weatherData.status === 'idle'}
        <div class="hv-weather-empty">{$t('desktop.weather_loading')}</div>
      {:else if weatherData.status === 'error'}
        <div class="hv-weather-empty">{weatherData.error}</div>
      {:else if weatherData.status === 'ready'}
        {@const info = weatherCodeInfo(weatherData.data.weatherCode)}
        <div class="hv-weather-body">
          <div class="hv-weather-location">
            <IconMapPin size={12} stroke={2} />
            <span>{$weatherLocationStore?.name}</span>
          </div>
          <div class="hv-weather-main">
            <svelte:component this={info.icon} size={40} stroke={1.75} />
            <span class="hv-weather-temp">{weatherData.data.temperature}°C</span>
          </div>
          <div class="hv-weather-condition">{$t(info.labelKey)}</div>
          <div class="hv-weather-details">
            <span>{$t('desktop.weather_humidity')}: {weatherData.data.humidity}%</span>
            <span>{$t('desktop.weather_wind')}: {weatherData.data.windSpeed} km/h</span>
            {#if $weatherDisplayPrefsStore.includes('feels_like')}
              <span>{$t('desktop.weather_feels_like')}: {weatherData.data.feelsLike}°C</span>
            {/if}
            {#if $weatherDisplayPrefsStore.includes('wind_gusts')}
              <span>{$t('desktop.weather_wind_gusts')}: {weatherData.data.windGusts} km/h</span>
            {/if}
            {#if $weatherDisplayPrefsStore.includes('precipitation')}
              <span>{$t('desktop.weather_precipitation')}: {weatherData.data.precipitation} mm</span>
            {/if}
            {#if $weatherDisplayPrefsStore.includes('cloud_cover')}
              <span>{$t('desktop.weather_cloud_cover')}: {weatherData.data.cloudCover}%</span>
            {/if}
            {#if $weatherDisplayPrefsStore.includes('pressure')}
              <span>{$t('desktop.weather_pressure')}: {weatherData.data.pressure} hPa</span>
            {/if}
          </div>
        </div>
      {/if}
    </div>
  {/if}

  {#if $monitorStore.sessionsMonitorVisible}
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div
      class="hv-sessions-monitor"
      class:dragging={draggingMonitor === 'sessions'}
      style="left: {draggingMonitor === 'sessions' ? monitorCurrentX : sessionsMonitorClamped.x}px; top: {draggingMonitor === 'sessions' ? monitorCurrentY : sessionsMonitorClamped.y}px;"
      onpointerdown={(e) => handleMonitorDragStart(e, 'sessions')}
    >
      <WidgetHeader
        color={MONITOR_COLOR.sessions}
        label={$t('desktop.sessions_monitor')}
      >
        {#snippet icon()}<IconUsers size={16} stroke={2} />{/snippet}
      </WidgetHeader>

      {#if sessionsData.status === 'loading' || sessionsData.status === 'idle'}
        <div class="hv-sessions-empty">{$t('desktop.sessions_loading')}</div>
      {:else if sessionsData.status === 'error'}
        <div class="hv-sessions-empty">{sessionsData.error}</div>
      {:else if sessionsData.status === 'ready'}
        <div class="hv-sessions-body">
          <span class="hv-sessions-count">{sessionsData.data.count}</span>
          <span class="hv-sessions-label">{$t('desktop.sessions_connected_label')}</span>
        </div>
      {/if}
    </div>
  {/if}

  {#if $monitorStore.healthMonitorVisible}
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div
      class="hv-health-monitor"
      class:dragging={draggingMonitor === 'health'}
      style="left: {draggingMonitor === 'health' ? monitorCurrentX : healthMonitorClamped.x}px; top: {draggingMonitor === 'health' ? monitorCurrentY : healthMonitorClamped.y}px;"
      onpointerdown={(e) => handleMonitorDragStart(e, 'health')}
    >
      <WidgetHeader
        color={MONITOR_COLOR.health}
        label={$t('desktop.health_monitor')}
      >
        {#snippet icon()}<IconHeartbeat size={16} stroke={2} />{/snippet}
      </WidgetHeader>

      {#if healthData.status === 'loading' || healthData.status === 'idle'}
        <div class="hv-health-empty">{$t('desktop.health_loading')}</div>
      {:else if healthData.status === 'error'}
        <div class="hv-health-empty">{healthData.error}</div>
      {:else if healthData.status === 'ready'}
        <div class="hv-health-overall" class:hv-health-overall-ok={healthData.data.overall === 'healthy'} class:hv-health-overall-warn={healthData.data.overall === 'warning'} class:hv-health-overall-crit={healthData.data.overall === 'unhealthy'}>
          <span class="hv-health-overall-dot"></span>
          <span>
            {#if healthData.data.overall === 'healthy'}{$t('health.healthy')}
            {:else if healthData.data.overall === 'warning'}{$t('health.warning')}
            {:else if healthData.data.overall === 'unhealthy'}{$t('health.critical')}
            {:else}{$t('health.unknown')}{/if}
          </span>
        </div>
        <div class="hv-health-stats">
          <div class="hv-health-stat hv-health-stat-ok">
            <span class="hv-health-stat-value">{healthData.data.healthy}</span>
            <span class="hv-health-stat-label">{$t('health.healthy')}</span>
          </div>
          <div class="hv-health-stat hv-health-stat-warn">
            <span class="hv-health-stat-value">{healthData.data.warning}</span>
            <span class="hv-health-stat-label">{$t('health.warning')}</span>
          </div>
          <div class="hv-health-stat hv-health-stat-crit">
            <span class="hv-health-stat-value">{healthData.data.critical}</span>
            <span class="hv-health-stat-label">{$t('health.critical')}</span>
          </div>
        </div>
      {/if}
    </div>
  {/if}

  <!-- Desktop Area with Windows -->
  <main class="hv-desktop">
    {#each $windowManager as window (window.id)}
      <WindowFrame
        win={window}
        onClose={() => handleCloseWindow(window.id)}
        onMinimize={() => handleMinimizeWindow(window.id)}
        onMaximize={() => handleMaximizeWindow(window.id)}
        onFocus={() => handleFocusWindow(window.id)}
        onUpdatePosition={(x, y) => handleUpdatePosition(window.id, x, y)}
        onUpdateSize={(w, h) => handleUpdateSize(window.id, w, h)}
      >
        {#if window.componentLoader}
          <LazyLoad componentLoader={window.componentLoader} windowTitle={window.title} />
        {:else if window.component}
          <svelte:component this={window.component} />
        {:else}
          <!-- Never reached: every window opened by the app catalogue or the
               top bar carries a component by construction. -->
          <div class="hv-window-placeholder">
            <IconPackage size={64} stroke={2} />
            <h2>{window.title}</h2>
          </div>
        {/if}
      </WindowFrame>
    {/each}
  </main>

  <!-- Power Options Modal -->
  {#if showPowerModal}
    <!-- svelte-ignore a11y_click_events_have_key_events -->
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div
      class="hv-power-overlay"
      onclick={() => (showPowerModal = false)}
      role="dialog"
      tabindex="-1"
      aria-modal="true"
      aria-label={$t('desktop.power_options')}
      use:focusTrap={{ escape: () => (showPowerModal = false) }}
    >
      <!-- svelte-ignore a11y_click_events_have_key_events -->
      <!-- svelte-ignore a11y_no_static_element_interactions -->
      <div
        class="hv-power-dialog"
        onclick={(e) => { e.stopPropagation(); }}
      >
        <!-- No header on the power dialog: it is a pure action sheet and the
             role="dialog" overlay already carries an accessible name. -->

        <div class="hv-power-content">
          <div class="hv-power-options">
            <!-- Shut Down -->
            <button class="hv-power-option hv-power-option-shutdown" onclick={handleShutdown}>
              <svg xmlns="http://www.w3.org/2000/svg" width="26" height="26" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">
                <path d="M12 3v10"/>
                <path d="M8.5 5.5a6 6 0 1 0 7 0"/>
              </svg>
              <span class="hv-power-label">{$t('desktop.shut_down')}</span>
            </button>

            <!-- Restart -->
            <button class="hv-power-option" onclick={handleRestart}>
              <svg xmlns="http://www.w3.org/2000/svg" width="26" height="26" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">
                <path d="M20 12a8 8 0 1 1-3-6.2"/>
                <path d="M20 4v4h-4"/>
              </svg>
              <span class="hv-power-label">{$t('desktop.restart')}</span>
            </button>

            <!-- Log Out -->
            <button class="hv-power-option" onclick={requestLogout}>
              <svg xmlns="http://www.w3.org/2000/svg" width="26" height="26" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">
                <path d="M15 5H7a2 2 0 0 0-2 2v10a2 2 0 0 0 2 2h8"/>
                <path d="M14 12h7"/>
                <path d="M18 9l3 3-3 3"/>
              </svg>
              <span class="hv-power-label">{$t('desktop.log_out')}</span>
            </button>
          </div>
        </div>
      </div>
    </div>
  {/if}

  <!-- Action Confirmation Modal (shown when user picks an action) -->
  {#if logoutAction}
    <!-- svelte-ignore a11y_click_events_have_key_events -->
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div
      class="hv-power-overlay"
      onclick={cancelLogoutAction}
      role="dialog"
      tabindex="-1"
      aria-modal="true"
      aria-labelledby="confirm-modal-title"
      use:focusTrap={{ escape: cancelLogoutAction }}
    >
      <!-- svelte-ignore a11y_click_events_have_key_events -->
      <!-- svelte-ignore a11y_no_static_element_interactions -->
      <div
        class="hv-power-dialog"
        onclick={(e) => { e.stopPropagation(); }}
      >
        <div class="hv-power-header">
          <div class="hv-power-title">
            <svg xmlns="http://www.w3.org/2000/svg" width="19" height="19" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round">
              <path d="M12 3v10"/><path d="M8.5 5.5a6 6 0 1 0 7 0"/>
            </svg>
            <span id="confirm-modal-title">
              {#if logoutAction === 'logout'}
                {$t('desktop.confirm_logout_title')}
              {:else if logoutAction === 'restart'}
                {$t('desktop.confirm_restart_title')}
              {:else}
                {$t('desktop.confirm_shutdown_title')}
              {/if}
            </span>
          </div>
          <button class="hv-power-close" onclick={cancelLogoutAction} aria-label={$t('common.close')}>
            <svg xmlns="http://www.w3.org/2000/svg" width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
              <path d="M6 6l12 12M18 6L6 18"/>
            </svg>
          </button>
        </div>
        <div class="hv-power-content">
          <p class="hv-confirm-text">
            {#if logoutAction === 'logout'}
              {$t('desktop.confirm_logout_text')}
            {:else if logoutAction === 'restart'}
              {$t('desktop.confirm_restart_text')}
            {:else}
              {$t('desktop.confirm_shutdown_text')}
            {/if}
          </p>
        </div>
        <div class="hv-power-footer">
          <button class="hv-power-btn hv-power-btn-secondary" onclick={cancelLogoutAction}>{$t('common.cancel')}</button>
          <button
            class="hv-power-btn"
            class:hv-power-btn-danger={logoutAction === 'shutdown'}
            class:hv-power-btn-warning={logoutAction === 'restart'}
            onclick={confirmLogoutAction}
          >
            {#if logoutAction === 'logout'}
              {$t('desktop.logout')}
            {:else if logoutAction === 'restart'}
              {$t('desktop.restart')}
            {:else}
              {$t('desktop.shut_down')}
            {/if}
          </button>
        </div>
      </div>
    </div>
  {/if}

  <!-- Dock (solid Fluent paper) -->
  <footer
    class="hv-dock"
    class:hv-chrome--idle={chromeIdle}
    class:hv-dock--hidden={dockHidden}
    bind:this={dockEl}
    bind:clientWidth={dockWidth}
  >
    <div class="hv-dock-container">
      <!-- Dashboard (default blue gradient) -->
      <button
        class="hv-dock-item"
        style="--dock-i: 0; --dock-c: var(--hv-app-dashboard-c)"
        class:hv-dock-item-active={$windowManager.some(w => w.appId === 'dashboard' && !w.isMinimized)}
        class:hv-dock-item-minimized={$windowManager.some(w => w.appId === 'dashboard' && w.isMinimized)}
        class:hv-dock-item-multi={openWindowCount('dashboard') > 1}
        onclick={() => handleDockAppClick('dashboard')}
        data-testid="dock-dashboard"
        aria-label={$t('desktop.app_dashboard_name')}
      >
        <div class="hv-dock-icon hv-dock-icon-dashboard">
          <IconHome size={28} stroke={1.8} />
        </div>
        {#if $windowManager && hasOpenWindow('dashboard')}<span class="hv-dock-dot"></span>{/if}
      </button>

      <!-- Terminal (green gradient) -->
      <button
        class="hv-dock-item"
        style="--dock-i: 1; --dock-c: var(--hv-app-terminal-c)"
        class:hv-dock-item-active={$windowManager.some(w => w.appId === 'terminal' && !w.isMinimized)}
        class:hv-dock-item-minimized={$windowManager.some(w => w.appId === 'terminal' && w.isMinimized)}
        class:hv-dock-item-multi={openWindowCount('terminal') > 1}
        onclick={() => handleDockAppClick('terminal')}
        data-testid="dock-terminal"
        aria-label={$t('desktop.terminal')}
      >
        <div class="hv-dock-icon hv-dock-icon-terminal">
          <TerminalIcon size={28} stroke={1.8} />
        </div>
        {#if $windowManager && hasOpenWindow('terminal')}<span class="hv-dock-dot"></span>{/if}
      </button>

      <!-- Containers (purple gradient) -->
      <button
        class="hv-dock-item"
        style="--dock-i: 2; --dock-c: var(--hv-app-docker-c)"
        class:hv-dock-item-active={$windowManager.some(w => w.appId === 'docker' && !w.isMinimized)}
        class:hv-dock-item-minimized={$windowManager.some(w => w.appId === 'docker' && w.isMinimized)}
        class:hv-dock-item-multi={openWindowCount('docker') > 1}
        onclick={() => handleDockAppClick('docker')}
        data-testid="dock-docker"
        aria-label={$t('desktop.app_docker_name')}
      >
        <div class="hv-dock-icon hv-dock-icon-containers">
          <ContainerIcon size={28} stroke={1.8} />
        </div>
        {#if dockerRunningCount > 0}
          <span class="hv-dock-badge" title={$t('desktop.dock_running', { count: String(dockerRunningCount) })}>
            {dockerRunningCount > 99 ? '99+' : dockerRunningCount}
          </span>
        {/if}
        {#if $windowManager && hasOpenWindow('docker')}<span class="hv-dock-dot"></span>{/if}
      </button>

      <!-- Health (red gradient) -->
      <button
        class="hv-dock-item"
        style="--dock-i: 3; --dock-c: var(--hv-app-health-c)"
        class:hv-dock-item-active={$windowManager.some(w => w.appId === 'health' && !w.isMinimized)}
        class:hv-dock-item-minimized={$windowManager.some(w => w.appId === 'health' && w.isMinimized)}
        class:hv-dock-item-multi={openWindowCount('health') > 1}
        class:hv-dock-attention={dockAttention['health']}
        onclick={() => handleDockAppClick('health')}
        data-testid="dock-health"
        aria-label={$t('desktop.app_health_name')}
      >
        <div class="hv-dock-icon hv-dock-icon-security">
          <IconShieldCheckered size={28} stroke={1.8} />
        </div>
        {#if healthIssueCount > 0}
          <span class="hv-dock-badge hv-dock-badge--alert" title={$t('desktop.dock_issues', { count: String(healthIssueCount) })}>
            {healthIssueCount > 99 ? '99+' : healthIssueCount}
          </span>
        {/if}
        {#if $windowManager && hasOpenWindow('health')}<span class="hv-dock-dot"></span>{/if}
      </button>

      <!-- AI Assistant (light blue gradient) -->
      <button
        class="hv-dock-item"
        style="--dock-i: 4; --dock-c: var(--hv-app-chat-c)"
        class:hv-dock-item-active={$windowManager.some(w => w.appId === 'chat' && !w.isMinimized)}
        class:hv-dock-item-minimized={$windowManager.some(w => w.appId === 'chat' && w.isMinimized)}
        class:hv-dock-item-multi={openWindowCount('chat') > 1}
        onclick={() => handleDockAppClick('chat')}
        data-testid="dock-chat"
        aria-label={$t('desktop.ai_assistant')}
      >
        <div class="hv-dock-icon hv-dock-icon-assistant">
          <IconRobot size={28} stroke={1.8} />
        </div>
        {#if $windowManager && hasOpenWindow('chat')}<span class="hv-dock-dot"></span>{/if}
      </button>

      <!-- Applications / Launchpad (lavender gradient) -->
      <button
        class="hv-dock-item"
        style="--dock-i: 5; --dock-c: var(--hv-app-launchpad-c)"
        class:hv-dock-item-active={showAppsCenter}
        onclick={(e)=>{e.stopPropagation();toggleAppsCenter();}}
        data-testid="dock-launchpad"
        aria-label={$t('desktop.applications')}
      >
        <div class="hv-dock-icon hv-dock-icon-launchpad">
           <IconLayoutGrid size={28} stroke={1.8} />
        </div>
        {#if showAppsCenter}<span class="hv-dock-dot"></span>{/if}
      </button>

      <span class="hv-dock-divider"></span>

      <!-- Settings (white) -->
      <button
        class="hv-dock-item"
        style="--dock-i: 6; --dock-c: var(--hv-app-settings-c)"
        class:hv-dock-item-active={$windowManager.some(w => w.appId === 'settings' && !w.isMinimized)}
        class:hv-dock-item-minimized={$windowManager.some(w => w.appId === 'settings' && w.isMinimized)}
        class:hv-dock-item-multi={openWindowCount('settings') > 1}
        onclick={() => handleDockAppClick('settings')}
        data-testid="dock-settings"
        aria-label={$t('desktop.settings_tooltip')}
      >
        <div class="hv-dock-icon hv-dock-icon-settings">
          <SettingsIcon size={28} stroke={1.8} />
        </div>
        {#if $windowManager && hasOpenWindow('settings')}<span class="hv-dock-dot"></span>{/if}
      </button>

      <!-- Background activity — transient, shown only while tasks are running.
           Click toggles the taskbar panel above the dock. -->
      {#if activityCount > 0}
        <span class="hv-dock-divider"></span>
        <button
          class="hv-dock-item hv-dock-item--enter"
          class:hv-dock-item-active={showActivityPanel}
          onclick={(e) => { e.stopPropagation(); toggleActivityPanel(); }}
          data-testid="dock-activity"
          aria-label={$t('desktop.activity_title')}
          aria-expanded={showActivityPanel}
          aria-haspopup="dialog"
        >
          <div class="hv-dock-icon hv-dock-icon-activity">
            <IconActivity size={28} stroke={1.8} />
          </div>
          <span
            class="hv-dock-badge"
            title={$t('desktop.activity_badge', { count: String(activityCount) })}
          >
            {activityCount > 99 ? '99+' : activityCount}
          </span>
          {#if showActivityPanel}<span class="hv-dock-dot"></span>{/if}
        </button>
      {/if}

      <!-- Minimized windows -->
      {#if minimizedWindows.length > 0}
        <div class="hv-dock-divider"></div>
        {#each minimizedWindows as win (win.id)}
          <button
            class="hv-dock-item hv-dock-item-minimized hv-dock-item--enter"
            class:bouncing={bouncingWindowId === win.id}
            onclick={() => handleRestoreFromDock(win.id)}
            aria-label={win.title}
            title={win.title}
          >
            <div class="hv-dock-icon">
              <Icon name={win.icon} size={26} />
            </div>
            <span class="hv-dock-dot"></span>
          </button>
        {/each}
        <div class="hv-dock-divider"></div>
      {/if}
    </div>
  </footer>

  <!-- Background-activity taskbar — sits flush above the dock, tracks its
       width, one row per running task (name / progress bar / %). Display-only. -->
  {#if showActivityPanel && activityCount > 0}
    <!-- svelte-ignore a11y_click_events_have_key_events: the click only
         stops propagation (keeps the panel open on inside clicks); all
         panel controls are natively keyboard-accessible. -->
    <div
      class="hv-activity-panel"
      class:hv-chrome--idle={chromeIdle}
      style="width: {dockWidth}px"
      role="dialog"
      tabindex="-1"
      aria-label={$t('desktop.activity_title')}
      onclick={(e) => e.stopPropagation()}
    >
      <div class="hv-activity-head">{$t('desktop.activity_title')}</div>
      {#each localizedActivityTasks as task (task.id)}
        <div class="hv-activity-row" title={task.detail ?? ''}>
          <span class="hv-activity-name">{task.displayName}</span>
          <span class="hv-activity-pct">{task.progress == null ? '' : `${Math.round(task.progress)}%`}</span>
          <div
            class="hv-activity-track"
            role="progressbar"
            aria-valuemin="0"
            aria-valuemax="100"
            aria-valuenow={task.progress == null ? undefined : Math.round(task.progress)}
            aria-valuetext={task.progress == null ? $t('desktop.activity_indeterminate') : undefined}
          >
            {#if task.progress == null}
              <span class="hv-activity-fill hv-activity-fill--indet"></span>
            {:else}
              <span
                class="hv-activity-fill"
                style="width: {Math.min(100, Math.max(0, task.progress))}%"
              ></span>
            {/if}
          </div>
        </div>
      {/each}
    </div>
  {/if}

  <!-- Application Launcher -->
  {#if showAppsCenter}
    <HvLaunchpad
      apps={allApps}
      onAppClick={(appId) => openApp(allApps.find(a => a.id === appId)!)}
      onOpenExternal={openExternalUrl}
      onClose={() => showAppsCenter = false}
    />
  {/if}
</div>

<style>
  /* Desktop Container.
     Font comes from the self-hosted Inter face (src/lib/styles/fonts.css) via
     --hv-font-family — no external font CDN, the box may have no internet
     (same rule as Login.svelte; the design template's 'Inter Tight' is a
     Google-Fonts-only face and is deliberately not used here). */
  .hv-desktop-container {
    position: fixed;
    inset: 0;
    display: flex;
    flex-direction: column;
    overflow: hidden;
    background-color: #e8edf6;
    font-family: var(--hv-font-family);
    color: var(--hv-shell-text-strong);
    -webkit-font-smoothing: antialiased;
  }

  /* Photo wallpaper: blurred cover fill behind + sharp cover fill on top.
     Both use background-size: cover so the image fills every corner of the
     viewport with no bands, regardless of aspect ratio. */
  .hv-wallpaper-photo-blur {
    position: absolute;
    inset: -40px; /* covers the scale(1.15) overshoot without a visible gap */
    background-size: cover;
    background-position: center;
    background-repeat: no-repeat;
    filter: blur(60px) brightness(0.72) saturate(1.1);
    transform: scale(1.15);
    pointer-events: none;
  }
  .hv-wallpaper-photo-sharp {
    position: absolute;
    inset: 0;
    background-size: cover;
    background-position: center;
    background-repeat: no-repeat;
    pointer-events: none;
  }

  /* paper wallpaper facets — barely-there slate washes + one light bloom,
     soft enough that the solid white chrome stays the visual anchor */
  .hv-wallpaper-layer {
    position: absolute;
    inset: 0;
    pointer-events: none;
  }

  .hv-wallpaper-layer-1 {
    background: linear-gradient(160deg, rgba(70, 70, 70, 0.12) 0%, rgba(60, 60, 60, 0.06) 60%, var(--hv-primary-light) 100%);
    clip-path: polygon(0% 30%, 41% 30%, 66% 100%, 0% 100%);
  }

  .hv-wallpaper-layer-2 {
    background: linear-gradient(200deg, rgba(255, 255, 255, 0.5), rgba(70, 70, 70, 0.08));
    clip-path: polygon(84% 0%, 100% 0%, 100% 100%, 71% 100%);
  }

  .hv-wallpaper-layer-3 {
    background: radial-gradient(60% 40% at 42% 24%, rgba(255, 255, 255, 0.65), rgba(255, 255, 255, 0) 70%);
  }

  /* Menu Bar — floating pill. Solid paper (`--hv-shell-glass`: opaque white,
     no blur) — a bundled wallpaper can be dark, and the solid pill keeps its
     slate-ink text legible at both extremes. Its surface material comes from
     `--hv-chrome-surface-*`, shared with the dock AND with the popovers that
     hang off this pill, so the pill and its own panels never mismatch. */
  .hv-menubar {
    position: fixed;
    top: var(--hv-chrome-topbar-offset-y);
    right: var(--hv-chrome-topbar-offset-x);
    z-index: var(--hv-z-topbar);
    display: flex;
    align-items: center;
    /* box-sizing makes the rendered height exactly --hv-chrome-topbar-h, which
       is what makes --hv-chrome-topbar-keepout exact. Do not remove it. */
    box-sizing: border-box;
    height: var(--hv-chrome-topbar-h);
    /* Vertical padding is a no-op at a fixed height + align-items:center —
       only the horizontal value actually does anything. */
    padding: 0 var(--hv-space-2);
    border-radius: var(--hv-radius-2xl);
    background: var(--hv-shell-glass);
    border: 1px solid var(--hv-chrome-surface-border);
    box-shadow: var(--hv-chrome-surface-shadow);
    color: var(--hv-shell-text-strong);
  }

  .hv-menubar-inner {
    display: flex;
    align-items: center;
    gap: var(--hv-space-1);
    height: 100%;
  }

  /* User pill — avatar + name. The avatar is small (32px): at pill height the
     name carries the identity, the picture is only a marker. */
  .hv-user-pill {
    display: flex;
    align-items: center;
    gap: var(--hv-space-2);
    padding: var(--hv-space-2);
    border: none;
    border-radius: var(--hv-radius-full);
    background: transparent;
    color: inherit;
    cursor: pointer;
    font: inherit;
  }

  .hv-user-pill:hover {
    background: var(--hv-subtle-hover);
  }

  .hv-user-pill-avatar {
    width: var(--hv-chrome-avatar-size);
    height: var(--hv-chrome-avatar-size);
    border-radius: 50%;
    overflow: hidden;
    display: grid;
    place-items: center;
    flex-shrink: 0;
  }

  .hv-user-pill-avatar :global(.hv-user-avatar) {
    /* Fill the wrapper whatever the <img> width/height attrs say (the
       component renders at size=62) — without this the image is
       centre-cropped and the face is clipped. */
    width: 100%;
    height: 100%;
    display: block;
    object-fit: cover;
    border-radius: 50%;
  }

  .hv-user-pill-name {
    font-size: var(--hv-text-sm);
    font-weight: var(--hv-font-weight-semibold);
    color: var(--hv-shell-text-strong);
    white-space: nowrap;
  }

  .hv-menubar-divider {
    width: 1px;
    height: var(--hv-space-6);
    /* Chrome hairline — the pill uses --hv-chrome-surface-border so its divider
       matches its own outline, not the (lighter) in-window hairline. */
    background: var(--hv-chrome-surface-border);
    margin: 0 var(--hv-space-0-5);
    flex-shrink: 0;
  }

  /* Clock section */
  .hv-menubar-clock {
    display: flex;
    flex-direction: column;
    align-items: center;
    line-height: var(--hv-line-height-tight);
    min-width: var(--hv-chrome-clock-min-w);
  }

  .hv-menubar-time {
    /* Mono, like the mockup: the pill is the one place where digits change
       every second, and a proportional face makes the block twitch. */
    font-family: var(--hv-font-mono);
    font-size: var(--hv-text-md);
    font-weight: var(--hv-font-weight-semibold);
    font-variant-numeric: tabular-nums;
    letter-spacing: var(--hv-letter-spacing-tight);
    color: var(--hv-shell-text-strong);
    /* Keep HH:MM:SS on one line — the per-digit spans would otherwise wrap
       once the pill is squeezed on a narrow phone. */
    display: flex;
    align-items: baseline;
    white-space: nowrap;
  }

  .hv-menubar-date {
    font-size: var(--hv-text-sm);
    color: var(--hv-shell-text);
    white-space: nowrap;
  }

  /* Topbar buttons (network, widgets, notifications, power) — 44px tall, flat,
     on the pill: no tile, no border, no shadow, just a subtle hover wash so
     the pill reads as one surface. From 901px up each one is an icon + label
     chip; the width/padding/gap below are the ICON-ONLY defaults, and the
     ≥901px block further down widens them (the label is hidden below 901px).
     Height stays 44px either way, so the shell's touch rhythm is unchanged. */
  .hv-topbar-btn {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    gap: 0;
    padding: 0;
    width: 44px;
    height: 44px;
    border: none;
    border-radius: var(--hv-radius-xl);
    background: transparent;
    color: var(--hv-shell-text-strong);
    cursor: pointer;
    flex-shrink: 0;
    transition: background var(--hv-chrome-dur-fast);
    position: relative;
  }

  .hv-topbar-btn:hover {
    background: var(--hv-subtle-hover);
  }

  .hv-topbar-btn:focus-visible {
    outline: 2px solid var(--hv-primary);
    outline-offset: -2px;
  }

  .hv-topbar-btn :global(svg) {
    width: 22px;
    height: 22px;
    flex-shrink: 0;
  }

  /* The chip's label. It stays in the DOM at every width — the ≤900px rule
     further down hides it with `display:none`, the same pattern
     `.hv-user-pill-name` uses, so there is no second markup branch and no JS
     width check. `display:none` also keeps it out of the accessibility tree,
     so the button's accessible name stays the `aria-label` alone instead of
     doubling up. `nowrap` matters: „Benachrichtigungen" would otherwise wrap
     inside the chip and push it past the 44px height. Ink matches
     `.hv-user-pill-name` and the icon it sits beside (inherited currentColor)
     — an icon + label control at two tones reads as a bug. */
  .hv-topbar-btn-label {
    font-size: var(--hv-text-sm);
    font-weight: var(--hv-font-weight-medium);
    color: var(--hv-shell-text-strong);
    line-height: var(--hv-line-height-tight);
    white-space: nowrap;
  }

  .hv-notif-btn {
    position: relative;
  }

  /* No position:relative here — the net dropdown anchors to the whole
     `.hv-menubar` pill via `.hv-topbar-panel` (components.css), not to this
     wrapper. */

  /* ── Network dropdown. All the solid-white card chrome (background, border,
     shadow, radius, entrance animation, width clamp) comes from the
     shared `.hv-topbar-panel` class (theme/components.css) — this only widens
     the default and owns the body. ── */
  .hv-net-dropdown {
    --hv-topbar-panel-w: 440px;
  }

  /* Body — the two IP rows are all this popover shows */
  .hv-net-body {
    display: flex;
    flex-direction: column;
    gap: 12px;
    padding: 20px;
  }

  .hv-net-ip-row {
    display: flex;
    align-items: center;
    gap: var(--hv-space-3);
    height: 84px;
    padding: 0 var(--hv-space-3) 0 0;
    border-radius: var(--hv-radius-3xl);
    border: 1px solid transparent;
  }

  .hv-net-ip-local {
    background: var(--hv-primary-selected-bg);
    border-color: color-mix(in srgb, var(--hv-primary) 24%, transparent);
  }

  .hv-net-ip-public {
    background: var(--hv-shell-card);
    border-color: var(--hv-shell-border);
  }

  .hv-net-ip-icon-tile {
    display: grid;
    place-items: center;
    width: 52px;
    height: 52px;
    margin-left: var(--hv-space-3);
    border-radius: 16px;
    background: var(--hv-primary-selected-bg);
    color: var(--hv-primary);
    flex-shrink: 0;
  }

  .hv-net-ip-icon-public {
    background: var(--hv-shell-fill);
    color: var(--hv-shell-text-2);
  }

  /* Server-location row — only rendered when the IP geo lookup succeeded. */
  .hv-net-loc-row {
    background: var(--hv-success-bg);
    border-color: color-mix(in srgb, var(--hv-success) 25%, transparent);
    height: auto;
    min-height: 84px;
    padding-top: 12px;
    padding-bottom: 12px;
  }

  .hv-net-ip-icon-loc {
    background: var(--hv-success-bg);
    color: var(--hv-success-text);
  }

  .hv-net-loc-value {
    font-size: var(--hv-text-lg);
    font-family: var(--hv-font-family);
    white-space: normal;
    overflow: visible;
    text-overflow: clip;
    line-height: var(--hv-line-height-normal);
    user-select: text;
  }

  .hv-net-loc-flag {
    margin-right: 6px;
  }

  .hv-net-ip-info {
    flex: 1;
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-width: 0;
  }

  .hv-net-ip-label {
    font-size: var(--hv-text-base);
    color: var(--hv-shell-text-2);
  }

  .hv-net-ip-value {
    font-size: var(--hv-text-2xl);
    font-weight: var(--hv-font-weight-medium);
    font-family: var(--hv-font-mono, 'JetBrains Mono', monospace);
    color: var(--hv-shell-text-strong);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    user-select: all;
  }

  .hv-net-copy {
    display: grid;
    place-items: center;
    width: 48px;
    height: 48px;
    border: none;
    border-radius: 16px;
    background: var(--hv-shell-fill);
    color: var(--hv-shell-text-2);
    cursor: pointer;
    flex-shrink: 0;
    transition: background 150ms;
  }

  .hv-net-copy:hover { background: var(--hv-subtle-hover); color: var(--hv-shell-text-strong); }

  /* Below 480px the floating pill topbar overflows — shrink to essentials */
  /* NOTE: the phone chrome overrides (topbar / dock / widget-hide) live at
     the very END of this <style> block — they need to come after every base
     rule for `.hv-dock-icon`, `.hv-menubar-*` etc. or they lose the
     equal-specificity source-order tie. See "PHONE CHROME OVERRIDES" below. */

  /* Resource Monitor (draggable) */
  .hv-resource-monitor {
    position: fixed;
    width: var(--hv-monitor-w-compact);
    background: var(--hv-shell-glass);
    border: 1px solid var(--hv-shell-border);
    padding: var(--hv-space-4);
    box-shadow: var(--hv-shell-shadow-card);
	z-index: var(--hv-z-monitor);
  }

  .hv-gauge-row {
    display: flex;
    justify-content: space-between;
    gap: var(--hv-space-2);
    margin-bottom: var(--hv-space-4);
  }

  .hv-gauge {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: var(--hv-space-1-5);
  }

  .hv-gauge-track {
    fill: none;
    stroke: var(--hv-border);
    stroke-width: 6;
  }

  .hv-gauge-fill {
    fill: none;
    stroke-width: 6;
    stroke-linecap: round;
    transform: rotate(-90deg);
    transform-origin: 36px 36px;
    transition: stroke-dashoffset 300ms ease, stroke 300ms;
  }

  .hv-gauge-fill-cpu {
    stroke: var(--hv-primary);
  }

  .hv-gauge-fill-ram {
    stroke: #7c8cf0;
  }

  .hv-gauge-fill-temp {
    stroke: var(--hv-warning);
  }

  .hv-gauge-fill-temp.high {
    stroke: var(--hv-error);
  }

  .hv-gauge-value {
    font-size: var(--hv-text-card-title);
    font-weight: var(--hv-font-weight-semibold);
    fill: var(--hv-text-primary);
    text-anchor: middle;
    font-variant-numeric: tabular-nums;
  }

  .hv-gauge-label {
    display: flex;
    align-items: center;
    gap: var(--hv-space-1);
    font-size: var(--hv-font-size-micro);
    font-weight: var(--hv-font-weight-medium);
    color: var(--hv-neutral-500);
    text-transform: uppercase;
    letter-spacing: 0.05em;
  }

  .hv-gauge-label :global(svg) {
    width: 12px;
    height: 12px;
  }

  .hv-resource-network {
    display: flex;
    justify-content: space-between;
    gap: var(--hv-space-3);
    margin-bottom: var(--hv-space-4);
  }

  .hv-network-item {
    flex: 1;
    padding: var(--hv-space-2);
    background: var(--hv-surface);
    border-radius: var(--hv-radius-md);
    display: flex;
    flex-direction: column;
  }

  .hv-network-label {
    font-size: var(--hv-font-size-micro);
    font-weight: var(--hv-font-weight-semibold);
    color: var(--hv-neutral-400);
    text-transform: uppercase;
    letter-spacing: 0.05em;
    margin-bottom: var(--hv-space-1);
    display: flex;
    align-items: center;
    gap: var(--hv-space-1);
  }

  .hv-network-label :global(svg) {
    width: 12px;
    height: 12px;
  }

  .hv-network-value {
    font-size: var(--hv-font-size-sm);
    font-weight: var(--hv-font-weight-semibold);
    color: var(--hv-text-primary);
    font-variant-numeric: tabular-nums;
  }

  /* System Update Indicator */
  .hv-system-update {
    padding: var(--hv-space-2) 12px;
    background: var(--hv-info-light);
    border: 1px solid var(--hv-primary-200);
    border-radius: var(--hv-radius-md);
    display: flex;
    align-items: center;
    gap: var(--hv-space-2);
  }

  .hv-system-update svg {
    width: 16px;
    height: 16px;
    stroke: var(--hv-info);
    flex-shrink: 0;
  }

  .hv-system-update-text {
    flex: 1;
    font-size: var(--hv-font-size-xs);
    color: var(--hv-info-text-alt);
  }

  .hv-system-update.available {
    background: var(--hv-badge-warning-bg);
    border-color: var(--hv-warning);
  }

  .hv-system-update.available svg {
    stroke: var(--hv-warning);
  }

  .hv-system-update.available .hv-system-update-text {
    color: var(--hv-badge-warning-text);
  }

  /* RAID Monitor (draggable) */
  .hv-raid-monitor {
    position: fixed;
    width: var(--hv-monitor-w-wide);
    background: var(--hv-shell-glass);
    border: 1px solid var(--hv-shell-border);
    padding: var(--hv-space-4);
    box-shadow: var(--hv-shell-shadow-card);
	z-index: var(--hv-z-monitor);
  }

  .hv-raid-list {
    max-height: 400px;
    overflow-y: auto;
  }

  /* Raid item card — matches reference .raid-item */
  .hv-raid-item {
    background: var(--hv-surface);
    border: 1px solid var(--hv-border);
    border-radius: var(--hv-radius-lg);
    padding: var(--hv-space-3);
    margin-bottom: var(--hv-space-3);
  }

  .hv-raid-item:last-child {
    margin-bottom: 0;
  }

  /* Raid header — matches reference .raid-header */
  .hv-raid-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin-bottom: var(--hv-space-2-5);
  }

  /* Raid name group — matches reference .raid-name */
  .hv-raid-name {
    display: flex;
    align-items: center;
    gap: var(--hv-space-2);
  }

  .hv-raid-name :global(svg) {
    width: 18px;
    height: 18px;
    stroke: var(--hv-primary);
  }

  .hv-raid-name-text {
    font-size: var(--hv-font-size-sm);
    font-weight: var(--hv-font-weight-semibold);
    color: var(--hv-text-primary);
  }

  /* Raid type badge — matches reference .raid-type */
  .hv-raid-type {
    font-size: var(--hv-font-size-xs);
    font-weight: var(--hv-font-weight-semibold);
    padding: var(--hv-space-0-5) 8px;
    border-radius: var(--hv-radius-default);
    background: var(--hv-info-light);
    color: var(--hv-info-text-alt);
  }

  /* Raid info row — matches reference .raid-info */
  .hv-raid-info {
    display: flex;
    justify-content: space-between;
    margin-bottom: var(--hv-space-2);
    font-size: var(--hv-font-size-sm);
  }

  .hv-raid-info-value {
    font-weight: var(--hv-font-weight-semibold);
    color: var(--hv-text-primary);
    font-variant-numeric: tabular-nums;
  }

  /* Raid status — matches reference .raid-status */
  .hv-raid-status {
    display: flex;
    align-items: center;
    gap: var(--hv-space-1-5);
    font-size: var(--hv-font-size-xs);
    font-weight: var(--hv-font-weight-medium);
  }

  .hv-raid-status-dot {
    width: 6px;
    height: 6px;
    border-radius: var(--hv-radius-full);
  }

  .hv-raid-status-healthy {
    color: var(--hv-badge-success-text);
  }

  .hv-raid-status-healthy .hv-raid-status-dot {
    background: var(--hv-success);
  }

  .hv-raid-status-degraded {
    color: var(--hv-badge-warning-text);
  }

  .hv-raid-status-degraded .hv-raid-status-dot {
    background: var(--hv-warning);
  }

  .hv-raid-status-failed {
    color: var(--hv-badge-error-text);
  }

  .hv-raid-status-failed .hv-raid-status-dot {
    background: var(--hv-error);
  }

  /* Raid progress — matches reference .raid-progress */
  .hv-raid-progress {
    margin-top: var(--hv-space-2);
  }

  .hv-raid-progress-label {
    display: flex;
    justify-content: space-between;
    font-size: var(--hv-font-size-xs);
    margin-bottom: var(--hv-space-1-5);
  }

  .hv-raid-progress-used {
    color: var(--hv-neutral-500);
  }

  .hv-raid-progress-free {
    color: var(--hv-success);
    font-weight: var(--hv-font-weight-semibold);
  }

  .hv-raid-progress-bar {
    width: 100%;
    height: 8px;
    background: var(--hv-border);
    border-radius: var(--hv-radius-default);
    overflow: hidden;
  }

  .hv-raid-progress-fill {
    height: 100%;
    background: linear-gradient(90deg, var(--hv-primary), var(--hv-primary-hover));
    border-radius: var(--hv-radius-default);
    transition: width 300ms ease;
  }

  /* Total Storage — matches reference .raid-total */
  .hv-raid-total {
    margin-top: var(--hv-space-3);
    padding: var(--hv-space-3);
    background: linear-gradient(135deg, var(--hv-primary) 0%, var(--hv-primary-hover) 100%);
    border-radius: var(--hv-radius-lg);
    color: white;
  }

  .hv-raid-total-label {
    font-size: var(--hv-font-size-xs);
    opacity: 0.9;
    margin-bottom: var(--hv-space-1-5);
  }

  .hv-raid-total-values {
    display: flex;
    justify-content: space-between;
    align-items: baseline;
  }

  .hv-raid-total-used {
    font-size: var(--hv-font-size-xl);
    font-weight: var(--hv-font-weight-bold);
    font-variant-numeric: tabular-nums;
  }

  .hv-raid-total-capacity {
    font-size: var(--hv-font-size-sm);
    opacity: 0.9;
  }

  .hv-raid-empty {
    padding: var(--hv-space-8) 16px;
    text-align: center;
    font-size: var(--hv-font-size-sm);
    color: var(--hv-neutral-500);
  }

  .hv-raid-empty-text {
    margin-bottom: 8px;
  }

  .hv-raid-create-btn {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    /* 44px touch target; 13px label fits the 300px panel. */
    min-height: 44px;
    padding: 0 16px;
    font-size: var(--hv-text-base);
    font-weight: var(--hv-font-medium);
    color: var(--hv-on-primary);
    background: var(--hv-primary);
    border: none;
    border-radius: var(--hv-radius-lg);
    cursor: pointer;
    transition: background 150ms ease;
  }

  .hv-raid-create-btn:hover {
    background: var(--hv-primary-hover);
  }

  .hv-raid-create-btn :global(svg) {
    width: 16px;
    height: 16px;
  }

  /* Security Monitor (draggable) */
  .hv-security-monitor {
    position: fixed;
    width: var(--hv-monitor-w);
    background: var(--hv-shell-glass);
    border: 1px solid var(--hv-shell-border);
    padding: var(--hv-space-4);
    box-shadow: var(--hv-shell-shadow-card);
	z-index: var(--hv-z-monitor);
  }

  .hv-security-list {
    max-height: 400px;
    overflow-y: auto;
  }

  .hv-security-item {
    background: var(--hv-surface);
    border: 1px solid var(--hv-border);
    border-radius: var(--hv-radius-lg);
    padding: var(--hv-space-3);
    margin-bottom: var(--hv-space-3);
  }

  .hv-security-item:last-child {
    margin-bottom: 0;
  }

  .hv-security-item-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--hv-space-2);
  }

  .hv-security-item-name {
    display: flex;
    align-items: center;
    gap: var(--hv-space-2);
    font-size: var(--hv-font-size-sm);
    font-weight: var(--hv-font-weight-semibold);
    color: var(--hv-text-primary);
  }

  .hv-security-item-name :global(svg) {
    stroke: var(--hv-primary);
    flex-shrink: 0;
  }

  .hv-security-status {
    display: flex;
    align-items: center;
    gap: var(--hv-space-1-5);
    font-size: var(--hv-font-size-xs);
    font-weight: var(--hv-font-weight-medium);
    white-space: nowrap;
  }

  .hv-security-status-dot {
    width: 6px;
    height: 6px;
    border-radius: var(--hv-radius-full);
    flex-shrink: 0;
  }

  .hv-security-status-ok {
    color: var(--hv-badge-success-text);
  }

  .hv-security-status-ok .hv-security-status-dot {
    background: var(--hv-success);
  }

  .hv-security-status-warn {
    color: var(--hv-badge-warning-text);
  }

  .hv-security-status-warn .hv-security-status-dot {
    background: var(--hv-warning);
  }

  .hv-security-status-off {
    color: var(--hv-neutral-500);
  }

  .hv-security-status-off .hv-security-status-dot {
    background: var(--hv-neutral-500);
  }

  .hv-security-item-detail {
    margin-top: var(--hv-space-2);
    font-size: var(--hv-font-size-xs);
    color: var(--hv-neutral-500);
  }

  .hv-security-empty {
    padding: var(--hv-space-8) 16px;
    text-align: center;
    font-size: var(--hv-font-size-sm);
    color: var(--hv-neutral-500);
  }

  /* Shared compact-panel look for the VPN / Downloader / Syncthing monitors */
  .hv-vpn-monitor,
  .hv-downloader-monitor,
  .hv-syncthing-monitor {
    position: fixed;
    width: var(--hv-monitor-w-compact);
    background: var(--hv-shell-glass);
    border: 1px solid var(--hv-shell-border);
    padding: var(--hv-space-4);
    box-shadow: var(--hv-shell-shadow-card);
	z-index: var(--hv-z-monitor);
  }

  /* Calendar monitor */
  .hv-calendar-monitor {
    position: fixed;
    width: var(--hv-monitor-w);
    background: var(--hv-shell-glass);
    border: 1px solid var(--hv-shell-border);
    padding: var(--hv-space-4);
    box-shadow: var(--hv-shell-shadow-card);
	z-index: var(--hv-z-monitor);
  }

  .hv-calendar-weekdays {
    display: grid;
    grid-template-columns: repeat(7, 1fr);
    gap: var(--hv-space-1);
    margin-bottom: var(--hv-space-2);
  }

  .hv-calendar-weekday {
    text-align: center;
    font-size: var(--hv-font-size-xs);
    font-weight: var(--hv-font-weight-semibold);
    /* Secondary ink — the old neutral-400 was ~1.6:1 on white, unreadable. */
    color: var(--hv-shell-text-2);
  }

  .hv-calendar-grid {
    display: grid;
    grid-template-columns: repeat(7, 1fr);
    gap: var(--hv-space-1);
  }

  .hv-calendar-cell {
    display: flex;
    align-items: center;
    justify-content: center;
    aspect-ratio: 1 / 1;
    font-size: var(--hv-font-size-sm);
    color: var(--hv-text-primary);
    border-radius: var(--hv-radius-default);
  }

  .hv-calendar-cell-empty {
    color: transparent;
  }

  .hv-calendar-cell-today {
    background: var(--hv-primary);
    color: var(--hv-on-primary);
    font-weight: var(--hv-font-weight-bold);
  }

  .hv-calendar-nav {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--hv-space-1);
    margin-top: var(--hv-space-3);
    padding-top: var(--hv-space-3);
    border-top: 1px solid var(--hv-border);
  }

  /* Compact 36px chrome: the 300px calendar can't fit five 44px targets
     beside the month label without truncating it, so the nav row uses the
     documented compact size (still ≥ WCAG 2.2 AA 24px). */
  .hv-calendar-nav-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    background: transparent;
    border: none;
    border-radius: var(--hv-radius-default);
    color: var(--hv-shell-text-3);
    cursor: pointer;
    transition: background 150ms, color 150ms;
    padding: 0;
    width: 36px;
    height: 36px;
    flex-shrink: 0;
  }

  .hv-calendar-nav-btn:hover {
    background: var(--hv-subtle-hover);
    color: var(--hv-text-primary);
  }

  .hv-calendar-nav-label {
    flex: 1;
    width: auto;
    height: 36px;
    font-size: var(--hv-font-size-xs);
    font-weight: var(--hv-font-weight-semibold);
    color: var(--hv-text-primary);
    text-transform: capitalize;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  /* Docker monitor */
  .hv-docker-monitor {
    position: fixed;
    width: var(--hv-monitor-w);
    background: var(--hv-shell-glass);
    border: 1px solid var(--hv-shell-border);
    padding: var(--hv-space-4);
    box-shadow: var(--hv-shell-shadow-card);
	z-index: var(--hv-z-monitor);
  }

  .hv-docker-list {
    max-height: 320px;
    overflow-y: auto;
    display: flex;
    flex-direction: column;
    gap: var(--hv-space-2);
  }

  .hv-docker-item {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--hv-space-2);
    background: var(--hv-surface);
    border: 1px solid var(--hv-border);
    border-radius: var(--hv-radius-lg);
    padding: var(--hv-space-2) var(--hv-space-3);
  }

  .hv-docker-item-info {
    display: flex;
    align-items: center;
    gap: var(--hv-space-2);
    min-width: 0;
  }

  .hv-docker-item-dot {
    width: 7px;
    height: 7px;
    border-radius: var(--hv-radius-full);
    flex-shrink: 0;
    background: var(--hv-neutral-500);
  }

  .hv-docker-dot-on {
    background: var(--hv-success);
  }

  .hv-docker-dot-warn {
    background: var(--hv-warning);
    /* Restarting/paused is a transient, in-flux state — pulse to draw the eye,
       unlike the steady "on"/running dot above which stays static. */
    animation: hv-pulse 1.6s ease-in-out infinite;
  }

  .hv-docker-item-name {
    font-size: var(--hv-font-size-sm);
    font-weight: var(--hv-font-weight-medium);
    color: var(--hv-text-primary);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .hv-docker-item-actions {
    display: flex;
    align-items: center;
    gap: var(--hv-space-1);
    flex-shrink: 0;
  }

  .hv-docker-action-btn {
    /* 44px touch targets — three fit beside the container name in the
       300px panel (3 × 44 + gaps + flexible info column). */
    width: 44px;
    height: 44px;
    display: flex;
    align-items: center;
    justify-content: center;
    background: transparent;
    border: none;
    border-radius: var(--hv-radius-default);
    color: var(--hv-shell-text-3);
    cursor: pointer;
    transition: background 150ms, color 150ms;
    padding: 0;
    flex-shrink: 0;
  }

  .hv-docker-action-btn:hover:not(:disabled) {
    background: var(--hv-subtle-hover);
    color: var(--hv-text-primary);
  }

  .hv-docker-action-btn:disabled {
    background: var(--hv-shell-border-soft);
    color: var(--hv-disabled-fg);
    opacity: 1;
    cursor: default;
  }

  .hv-weather-monitor {
    position: fixed;
    width: var(--hv-monitor-w-compact);
    background: var(--hv-shell-glass);
    border: 1px solid var(--hv-shell-border);
    padding: var(--hv-space-4);
    box-shadow: var(--hv-shell-shadow-card);
	z-index: var(--hv-z-monitor);
  }

  .hv-weather-empty {
    text-align: center;
    padding: var(--hv-space-4) 0;
    /* Secondary ink — the old tertiary grey was ~3.4:1, unreadable as the
       panel's only content. */
    color: var(--hv-shell-text-2);
    font-size: var(--hv-font-size-sm);
  }

  .hv-weather-empty-configure {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: var(--hv-space-3);
  }

  .hv-weather-open-settings-btn {
    min-height: 44px;
    padding: 0 20px;
    border: none;
    border-radius: var(--hv-radius-default);
    background: var(--hv-primary);
    color: var(--hv-on-primary);
    font-size: var(--hv-font-size-sm);
    cursor: pointer;
  }

  .hv-weather-open-settings-btn:hover {
    background: var(--hv-primary-hover);
  }

  .hv-weather-body {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: var(--hv-space-1);
  }

  .hv-weather-location {
    display: flex;
    align-items: center;
    gap: var(--hv-space-1);
    color: var(--hv-neutral-500);
    font-size: var(--hv-font-size-xs);
  }

  .hv-weather-main {
    display: flex;
    align-items: center;
    gap: var(--hv-space-3);
    margin: var(--hv-space-1) 0;
  }

  .hv-weather-main :global(svg) {
    stroke: var(--hv-primary);
  }

  .hv-weather-temp {
    font-size: var(--hv-font-size-4xl);
    font-weight: var(--hv-font-weight-semibold);
    color: var(--hv-text-primary);
  }

  .hv-weather-condition {
    font-size: var(--hv-font-size-sm);
    color: var(--hv-text-primary);
  }

  .hv-weather-details {
    display: flex;
    flex-wrap: wrap;
    justify-content: center;
    gap: var(--hv-space-1) var(--hv-space-3);
    margin-top: var(--hv-space-2);
    color: var(--hv-neutral-500);
    font-size: var(--hv-font-size-xs);
  }

  .hv-sessions-monitor {
    position: fixed;
    width: var(--hv-monitor-w-compact);
    background: var(--hv-shell-glass);
    border: 1px solid var(--hv-shell-border);
    padding: var(--hv-space-4);
    box-shadow: var(--hv-shell-shadow-card);
	z-index: var(--hv-z-monitor);
  }

  .hv-sessions-empty {
    text-align: center;
    padding: var(--hv-space-4) 0;
    color: var(--hv-neutral-500);
    font-size: var(--hv-font-size-sm);
  }

  .hv-sessions-body {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: var(--hv-space-1);
    padding: var(--hv-space-2) 0;
  }

  .hv-sessions-count {
    font-size: var(--hv-font-size-4xl);
    font-weight: var(--hv-font-weight-semibold);
    color: var(--hv-text-primary);
    line-height: 1;
  }

  .hv-sessions-label {
    font-size: var(--hv-font-size-sm);
    color: var(--hv-neutral-500);
  }

  .hv-health-monitor {
    position: fixed;
    width: var(--hv-monitor-w-compact);
    background: var(--hv-shell-glass);
    border: 1px solid var(--hv-shell-border);
    padding: var(--hv-space-4);
    box-shadow: var(--hv-shell-shadow-card);
	z-index: var(--hv-z-monitor);
  }

  .hv-health-empty {
    text-align: center;
    padding: var(--hv-space-4) 0;
    color: var(--hv-neutral-500);
    font-size: var(--hv-font-size-sm);
  }

  .hv-health-overall {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: var(--hv-space-2);
    padding-bottom: var(--hv-space-3);
    margin-bottom: var(--hv-space-3);
    border-bottom: 1px solid var(--hv-border);
    font-size: var(--hv-font-size-sm);
    font-weight: var(--hv-font-weight-medium);
  }

  .hv-health-overall-dot {
    width: 8px;
    height: 8px;
    border-radius: var(--hv-radius-full);
    background: var(--hv-neutral-500);
  }

  .hv-health-overall-ok { color: var(--hv-badge-success-text); }
  .hv-health-overall-ok .hv-health-overall-dot { background: var(--hv-success); }
  .hv-health-overall-warn { color: var(--hv-badge-warning-text); }
  .hv-health-overall-warn .hv-health-overall-dot { background: var(--hv-warning); }
  .hv-health-overall-crit { color: var(--hv-error); }
  .hv-health-overall-crit .hv-health-overall-dot { background: var(--hv-error); }

  .hv-health-stats {
    display: grid;
    grid-template-columns: repeat(3, 1fr);
    gap: var(--hv-space-2);
  }

  .hv-health-stat {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 2px;
  }

  .hv-health-stat-value {
    font-size: var(--hv-font-size-2xl);
    font-weight: var(--hv-font-weight-semibold);
  }

  .hv-health-stat-label {
    font-size: var(--hv-font-size-xs);
    color: var(--hv-neutral-500);
  }

  .hv-health-stat-ok .hv-health-stat-value { color: var(--hv-success); }
  .hv-health-stat-warn .hv-health-stat-value { color: var(--hv-warning); }
  .hv-health-stat-crit .hv-health-stat-value { color: var(--hv-error); }

  .hv-vpn-empty,
  .hv-downloader-empty,
  .hv-syncthing-empty {
    padding: var(--hv-space-6) 16px;
    text-align: center;
    font-size: var(--hv-font-size-sm);
    color: var(--hv-neutral-500);
  }

  /* VPN Monitor rows */
  .hv-vpn-list {
    display: flex;
    flex-direction: column;
    gap: var(--hv-space-2);
  }

  .hv-vpn-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: var(--hv-space-2) var(--hv-space-3);
    background: var(--hv-surface);
    border: 1px solid var(--hv-border);
    border-radius: var(--hv-radius-lg);
    font-size: var(--hv-font-size-sm);
  }

  .hv-vpn-row-label {
    font-weight: var(--hv-font-weight-semibold);
    color: var(--hv-text-primary);
  }

  .hv-vpn-row-value {
    color: var(--hv-neutral-500);
    font-variant-numeric: tabular-nums;
  }

  /* Downloader Monitor */
  .hv-downloader-active {
    font-size: var(--hv-font-size-sm);
    font-weight: var(--hv-font-weight-semibold);
    color: var(--hv-text-primary);
    margin-bottom: var(--hv-space-3);
  }

  .hv-downloader-speeds {
    display: flex;
    gap: var(--hv-space-3);
  }

  .hv-downloader-speed-item {
    flex: 1;
    background: var(--hv-surface);
    border: 1px solid var(--hv-border);
    border-radius: var(--hv-radius-lg);
    padding: var(--hv-space-2) var(--hv-space-3);
  }

  .hv-downloader-speed-label {
    display: flex;
    align-items: center;
    gap: var(--hv-space-1-5);
    font-size: var(--hv-font-size-xs);
    color: var(--hv-neutral-500);
    margin-bottom: var(--hv-space-1);
  }

  .hv-downloader-speed-value {
    font-size: var(--hv-font-size-sm);
    font-weight: var(--hv-font-weight-semibold);
    color: var(--hv-text-primary);
    font-variant-numeric: tabular-nums;
  }

  /* Syncthing Monitor */
  .hv-syncthing-status {
    display: flex;
    align-items: center;
    gap: var(--hv-space-1-5);
    font-size: var(--hv-font-size-sm);
    font-weight: var(--hv-font-weight-medium);
    color: var(--hv-badge-success-text);
    margin-bottom: var(--hv-space-2);
  }

  .hv-syncthing-status-dot {
    width: 6px;
    height: 6px;
    border-radius: var(--hv-radius-full);
    background: var(--hv-success);
  }

  .hv-syncthing-status-syncing {
    color: var(--hv-badge-warning-text);
  }

  .hv-syncthing-status-syncing .hv-syncthing-status-dot {
    background: var(--hv-warning);
    /* Actively transferring files right now — pulse to distinguish from the
       steady "up to date" dot above, which stays static. */
    animation: hv-pulse 1.6s ease-in-out infinite;
  }

  .hv-syncthing-detail {
    font-size: var(--hv-font-size-xs);
    color: var(--hv-neutral-500);
  }

  /* Dragging state for monitors */
  .hv-resource-monitor.dragging,
  .hv-raid-monitor.dragging,
  .hv-security-monitor.dragging,
  .hv-vpn-monitor.dragging,
  .hv-downloader-monitor.dragging,
  .hv-syncthing-monitor.dragging,
  .hv-calendar-monitor.dragging,
  .hv-docker-monitor.dragging,
  .hv-weather-monitor.dragging,
  .hv-sessions-monitor.dragging,
  .hv-health-monitor.dragging {
	z-index: var(--hv-z-monitor-drag);
	    box-shadow: var(--hv-shell-shadow-card);
	    opacity: 0.95;
    transition: box-shadow 150ms ease, opacity 150ms ease;
  }

  .hv-resource-monitor,
  .hv-raid-monitor,
  .hv-security-monitor,
  .hv-vpn-monitor,
  .hv-downloader-monitor,
  .hv-syncthing-monitor,
  .hv-calendar-monitor,
  .hv-docker-monitor,
  .hv-weather-monitor,
  .hv-sessions-monitor,
  .hv-health-monitor {
    transition: box-shadow 200ms ease, opacity 200ms ease;
    cursor: grab;
    /* Each panel is behind its own {#if $monitorStore.xVisible} block, so this
       plays once whenever a panel's node is freshly mounted (toggled on from
       the Widgets app) — transform/opacity only, doesn't replay on drag. */
    animation: hv-scale-in 160ms var(--hv-ease-out);
  }

  .hv-resource-monitor.dragging,
  .hv-raid-monitor.dragging,
  .hv-security-monitor.dragging,
  .hv-vpn-monitor.dragging,
  .hv-downloader-monitor.dragging,
  .hv-syncthing-monitor.dragging,
  .hv-calendar-monitor.dragging,
  .hv-docker-monitor.dragging,
  .hv-weather-monitor.dragging,
  .hv-sessions-monitor.dragging,
  .hv-health-monitor.dragging {
    cursor: grabbing;
  }

  /* No position:relative on the notification trigger — the panel anchors to
     `.hv-menubar` via `.hv-topbar-panel` (components.css), not to a wrapper. */

  /* User Dropdown — light account menu. All the card chrome
     (background, border, blur, shadow, radius, entrance animation, width
     clamp) comes from the shared `.hv-topbar-panel` class
     (theme/components.css). Full-bleed rows are clipped by the panel's own
     rounded corners (`overflow: hidden` there), so no inner padding here. */

  .hv-user-info {
    display: flex;
    align-items: center;
    gap: var(--hv-space-4);
    padding: 24px;
    background: var(--hv-shell-fill);
  }

  .hv-user-avatar {
    width: 76px;
    height: 76px;
    border-radius: var(--hv-radius-full);
    background: var(--hv-app-accent-grad);
    display: flex;
    align-items: center;
    justify-content: center;
    color: var(--hv-on-primary);
    font-size: var(--hv-font-size-3xl);
    font-weight: var(--hv-font-weight-semibold);
    flex-shrink: 0;
    box-shadow: 0 8px 22px color-mix(in srgb, var(--hv-primary) 28%, transparent);
  }

  .hv-user-details {
    flex: 1;
    min-width: 0;
  }

  .hv-user-details-name {
    font-size: var(--hv-text-2xl);
    font-weight: var(--hv-font-weight-semibold);
    letter-spacing: -0.01em;
    color: var(--hv-shell-text-strong);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .hv-user-details-role {
    font-size: var(--hv-text-md);
    color: var(--hv-shell-text-2);
    margin-top: 5px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .hv-user-menu-divider {
    height: 1px;
    background: var(--hv-shell-border-soft);
  }

  .hv-user-menu-item {
    display: flex;
    align-items: center;
    gap: var(--hv-space-4);
    width: 100%;
    height: 76px;
    padding: 0 24px;
    background: transparent;
    border: none;
    color: var(--hv-shell-text-strong);
    font-size: var(--hv-font-size-lg);
    font-weight: var(--hv-font-weight-medium);
    text-align: left;
    cursor: pointer;
    transition: background 150ms;
  }

  .hv-user-menu-item:hover {
    background: var(--hv-shell-border-soft);
  }

  .hv-user-menu-item :global(svg) {
    width: 26px;
    height: 26px;
    color: var(--hv-shell-text-2);
    flex-shrink: 0;
  }

  .hv-logout {
    color: var(--hv-error);
  }

  .hv-logout:hover {
    background: var(--hv-error-bg);
  }

  .hv-logout :global(svg) {
    color: var(--hv-error);
  }

  /* Desktop Area */
  .hv-desktop {
    flex: 1;
    position: relative;
    overflow: hidden;
  }

  /* Window Placeholder */
  .hv-window-placeholder {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    height: 100%;
    color: var(--hv-text-secondary);
  }

  .hv-window-placeholder :global(svg) {
    width: 64px;
    height: 64px;
    margin-bottom: var(--hv-space-4);
  }

  .hv-window-placeholder h2 {
    margin: 0 0 8px 0;
    font-size: var(--hv-font-size-2xl);
    color: var(--hv-text-tertiary);
  }

  /* Dock — solid white pill, taller than the topbar pill on purpose: it is
     the primary target and its tiles need the room. Same
     `--hv-chrome-surface-*` material as the topbar pill — see the topbar
     comment above for why the surface stays opaque whatever the wallpaper.
     Its height token is its OWN (--hv-chrome-dock-h): the two pills were
     once locked to one height and must never be again. */
  .hv-dock {
    position: fixed;
    bottom: var(--hv-chrome-dock-offset);
    left: 50%;
    transform: translateX(-50%);
    display: flex;
    align-items: center;
    gap: var(--hv-space-1);
    padding: var(--hv-space-3);
    /* box-sizing so the padding lives inside the fixed height, which is what
       makes --hv-chrome-dock-keepout exact. Do not remove it. */
    box-sizing: border-box;
    height: var(--hv-chrome-dock-h);
    border-radius: var(--hv-radius-2xl);
    background: var(--hv-shell-glass);
    border: 1px solid var(--hv-chrome-surface-border);
    box-shadow: var(--hv-chrome-surface-shadow);
    z-index: var(--hv-z-dock);
    max-width: calc(100vw - 2 * var(--hv-chrome-inset));
  }

  .hv-dock-container {
    display: flex;
    align-items: center;
    /* ITS gap — not .hv-dock's — is what spaces the tiles, so it is the one
       that has to clear the hover-lift growth (see --hv-chrome-dock-gap). */
    gap: var(--hv-chrome-dock-gap);
    padding: 0;
    background: transparent;
    border: none;
    border-radius: 0;
    box-shadow: none;
    max-width: none;
    overflow: visible;
  }

  .hv-dock-item {
    position: relative;
    padding: 0;
    background: transparent;
    border: none;
    border-radius: 0;
    cursor: pointer;
    transition: transform var(--hv-chrome-dur-fast) var(--hv-chrome-ease-spring);
    transform-origin: bottom center;
  }

  .hv-dock-item:hover {
    transform: translateY(-10px) scale(1.18);
  }

  /* Every app's dock icon uses its own dedicated gradient (tokens.css).
     The base rule is the fallback; the per-app classes below override it.
     Radius is --hv-radius-2xl (16px) at 48px — the same ratio as the pills. */
  .hv-dock-icon {
    width: 48px;
    height: 48px;
    border-radius: var(--hv-radius-2xl);
    display: grid;
    place-items: center;
    background: var(--hv-app-accent-grad);
    color: var(--hv-on-primary);
    box-shadow: var(--hv-shadow-md);
    transition:
      box-shadow var(--hv-chrome-dur-fast) var(--hv-chrome-ease-out),
      filter var(--hv-chrome-dur-fast) var(--hv-chrome-ease-out);
  }

  /* Hover: the tile lifts out of the dock and brightens slightly. The
     brightness bump is what keeps the lift readable on pale tiles
     (Launchpad), where an extra shadow alone reads as nothing. */
  .hv-dock-item:hover .hv-dock-icon {
    box-shadow: var(--hv-shadow-xl);
    filter: brightness(1.05);
  }

  .hv-dock-icon :global(svg) {
    width: 24px;
    height: 24px;
    stroke: currentColor;
    transition:
      stroke var(--hv-chrome-dur-fast) var(--hv-chrome-ease-out),
      fill var(--hv-chrome-dur-fast) var(--hv-chrome-ease-out);
  }

  /* Per-app dock icon gradients — each app gets its own colour.
     Any pinned app whose id is not listed here MUST get a class added
     here in the same change, or its tile falls back to the accent gradient
     while its running dot uses its own colour. */
  .hv-dock-icon-dashboard {
    background: var(--hv-app-dashboard-grad);
  }
  .hv-dock-icon-terminal {
    background: var(--hv-app-terminal-grad);
  }
  .hv-dock-icon-containers {
    background: var(--hv-app-docker-grad);
  }
  .hv-dock-icon-security {
    background: var(--hv-app-health-grad);
  }
  .hv-dock-icon-assistant {
    background: var(--hv-app-chat-grad);
  }
  .hv-dock-icon-settings {
    background: var(--hv-app-settings-grad);
  }

  /* Launchpad launcher — not an app, so it keeps its own pale look rather
     than an app gradient. Dark ink on light, unlike every app tile. */
  .hv-dock-icon-launchpad {
    background: var(--hv-app-launchpad-grad);
    color: var(--hv-shell-text-strong);
  }

  /* Running indicator — see `.hv-dock-dot` in the DESKTOP CHROME EFFECTS
     section below (a real element, not ::after, which is the click ripple). */

  .hv-dock-item-minimized.bouncing .hv-dock-icon {
    animation: hvDockBounce 500ms cubic-bezier(0.28, 0.84, 0.42, 1);
  }

  @keyframes hvDockBounce {
    0%, 100% { transform: translateY(0); }
    30% { transform: translateY(-18px); }
    50% { transform: translateY(0); }
    70% { transform: translateY(-8px); }
  }

  .hv-dock-divider {
    width: 1px;
    height: 40px;
    /* Chrome hairline — see `.hv-menubar-divider`'s comment. */
    background: var(--hv-chrome-surface-border);
    margin: 0 var(--hv-space-1-5);
    flex-shrink: 0;
  }

  /* ═══════════════════════════════════════════════════════════════════════
     DESKTOP CHROME EFFECTS (dock + topbar)
     ---------------------------------------------------------------------
     Transform / opacity / colour only — no animated blur or box-shadow, so
     this stays cheap on GPU-less servers and software-composited
     clients. Every infinite loop is either gated behind a state or
     paused when the chrome dims; the global prefers-reduced-motion rule in
     base.css already flattens the rest, and the block at the very end of
     this section flattens the JS-driven pieces too.
     ═══════════════════════════════════════════════════════════════════════ */

  /* 1. Dock icon hover — only the icon under the pointer lifts + grows
     (see `.hv-dock-item:hover` in the base dock styles above); neighbours
     are deliberately left alone. */

  /* 2. Icon tooltip label — reads aria-label, waits --hv-chrome-tooltip-delay
     before showing so it doesn't strobe as the pointer sweeps across the
     dock, hides at once on leave. Dark inverted fill, 6px radius: a label,
     not a card. (Suppressed entirely on touch — see the @media (hover: none)
     block at the end of this stylesheet; there is no hover to trigger it.) */
  .hv-dock-item::before {
    content: attr(aria-label);
    position: absolute;
    bottom: calc(100% + var(--hv-space-2));
    left: 50%;
    transform: translateX(-50%) translateY(var(--hv-space-1));
    padding: var(--hv-space-0-5) var(--hv-space-2);
    border-radius: var(--hv-radius-default);
    background: var(--hv-tooltip-bg);
    color: var(--hv-text-inverse);
    font-size: var(--hv-text-xs);
    font-weight: var(--hv-font-weight-medium);
    white-space: nowrap;
    pointer-events: none;
    opacity: 0;
    transition: opacity var(--hv-chrome-dur-fast) ease, transform var(--hv-chrome-dur-fast) ease;
    z-index: 10;
  }

  .hv-dock-item:hover::before,
  .hv-dock-item:focus::before,
  .hv-dock-item:active::before {
    opacity: 1;
    transform: translateX(-50%) translateY(0);
    transition-delay: var(--hv-chrome-tooltip-delay);
  }

  /* 3. Click ripple — expanding circle on :active */
  .hv-dock-item::after {
    content: '';
    position: absolute;
    inset: -4px;
    border-radius: 50%;
    background: rgba(255, 255, 255, 0.18);
    transform: scale(0);
    opacity: 0;
    pointer-events: none;
  }

  .hv-dock-item:active::after {
    animation: hvDockRipple 400ms ease-out forwards;
  }

  @keyframes hvDockRipple {
    0%   { transform: scale(0);   opacity: 0.35; }
    100% { transform: scale(1.6); opacity: 0; }
  }

  /* 3b. Press feedback — the icon itself dips on pointer-down. */
  .hv-dock-item:active .hv-dock-icon {
    transform: scale(var(--hv-chrome-press));
  }

  /* 4. Running indicator — a short bar in the app's colour under any dock
     button whose app has a window open, minimized or not (`.hv-dock-dot`, a
     real element so it never collides with the ::after ripple or the
     ::before tooltip). Figma geometry: h-1 (4px) × the icon width — full
     width for the foreground app, 40% when minimized, hidden when closed.
     The foreground app's bar breathes. Pulse pauses while the dock is
     dimmed. */
  .hv-dock-dot {
    position: absolute;
    bottom: -8px;
    left: 50%;
    /* Proportional to the tile, converted to a clean 20px on the standard
       48px dock tile via calc (20/48): the dot keeps its ratio on the
       smaller phone tiles (≈44–46px) instead of being a fixed px (2026-09-15).
       Constant in every state — no full-width active/multi bar. */
    width: calc(100% * 20 / 48);
    height: 4px;
    border-radius: 999px;
    /* The app's own solid hue (--hv-dock-c set inline per dock button) —
       Figma-style running indicator, not the old ink dot. */
    background: var(--dock-c, var(--hv-primary));
    transform: translateX(-50%);
    pointer-events: none;
    transition: width var(--hv-chrome-dur-med) var(--hv-chrome-ease-out);
    animation: hvDockDotIn 200ms ease backwards;
  }

  .hv-dock-item-active .hv-dock-dot {
    animation: hvDockDotPulse 2.4s ease-in-out infinite;
  }

  .hv-dock.hv-chrome--idle .hv-dock-item-active .hv-dock-dot {
    animation-play-state: paused;
  }

  @keyframes hvDockDotIn {
    from { opacity: 0; }
    to   { opacity: 1; }
  }

  @keyframes hvDockDotPulse {
    0%, 100% { opacity: 0.4; }
    50%      { opacity: 1; }
  }

  /* 5. Dock entrance — icons rise in sequence once, on first mount. */
  .hv-dock-item {
    animation: hvDockItemIn var(--hv-chrome-dur-slow) var(--hv-chrome-ease-spring) backwards;
    animation-delay: calc(var(--dock-i, 0) * 45ms + 60ms);
  }

  .hv-dock-item--enter {
    animation: hvDockItemIn var(--hv-chrome-dur-med) var(--hv-chrome-ease-spring) backwards;
    animation-delay: 0ms;
  }

  @keyframes hvDockItemIn {
    from { opacity: 0; transform: translateY(14px) scale(0.9); }
    to   { opacity: 1; transform: translateY(0) scale(1); }
  }

  /* 6. Attention nudge — one bounce when an app's underlying state degrades
     (e.g. health checks start failing). Reuses the restore-bounce keyframe. */
  .hv-dock-attention .hv-dock-icon {
    animation: hvDockBounce 560ms cubic-bezier(0.28, 0.84, 0.42, 1);
  }

  /* 7. Dock count badge — the round count pill pinned to a 48px dock tile
     (running-container count, health issues; `--alert` is the white/red
     variant). The notification badge used to share this rule; it was split
     off (rule 12) because the two anchors are not the same size — a 48px tile
     can carry a 26px badge, a 44px chip with a label cannot. Phone override
     near the end shrinks + re-tucks it so the dock's `overflow-y:hidden`
     scroll row can't clip it. */
  .hv-dock-badge {
    position: absolute;
    top: -6px;
    right: -6px;
    min-width: 26px;
    height: 26px;
    padding: 0 var(--hv-space-1-5);
    box-sizing: border-box;
    display: grid;
    place-items: center;
    border-radius: var(--hv-radius-full);
    background: var(--hv-success);
    color: var(--hv-chrome-badge-fg);
    font-size: var(--hv-text-base);
    font-weight: var(--hv-font-weight-bold);
    line-height: 1;
    font-variant-numeric: tabular-nums;
    /* Solid-white ring, same paper as the pills the badge sits on — that ring
       is what detaches the badge from a saturated icon tile. */
    box-shadow: 0 0 0 var(--hv-space-0-5) var(--hv-shell-glass);
    animation: hvChromePop var(--hv-chrome-dur-med) var(--hv-chrome-ease-spring) backwards;
  }

  /* The security icon is itself red, so a solid-red badge would vanish on
     it — a white pill with red text + ring reads on any icon colour. */
  .hv-dock-badge--alert {
    background: var(--hv-shell-glass);
    color: var(--hv-error);
    box-shadow: 0 0 0 var(--hv-space-0-5) color-mix(in srgb, var(--hv-error) 90%, transparent);
  }

  /* 8. Dock auto-hide — slides out of the way while a window is maximized,
     peeks back when the pointer touches the bottom edge. */
  .hv-dock {
    transition:
      transform var(--hv-chrome-dur-med) var(--hv-chrome-ease-out),
      opacity var(--hv-chrome-dur-med) ease;
  }

  .hv-dock--hidden {
    transform: translateX(-50%) translateY(150%);
    opacity: 0;
    pointer-events: none;
  }

  /* 9. Topbar icon hover — the 4 pill buttons lift + grow like the dock
     icons (each on its own, neighbours untouched). Scaling a child of the
     solid pill is cheap — only scaling the surface itself would
     re-rasterise, which is why the pill no longer lifts. */
  .hv-menubar {
    transition:
      transform var(--hv-chrome-dur-med) var(--hv-chrome-ease-out),
      opacity var(--hv-chrome-dur-med) ease;
  }

  /* Fullscreen mode — a maximized window owns the whole screen, so the
     pill slides up out of the way (peeks back on top-edge hover, see
     onChromePeekMove). Mirrors `.hv-dock--hidden`. */
  .hv-menubar--hidden {
    /* Clears its own top offset plus a little slack, so the pill is fully
       off-screen and not merely lined up with the top edge. */
    transform: translateY(calc(-100% - var(--hv-chrome-topbar-offset-y) - var(--hv-space-3)));
    opacity: 0;
    pointer-events: none;
  }

  .hv-topbar-btn,
  .hv-user-pill {
    transition:
      background var(--hv-chrome-dur-fast),
      transform var(--hv-chrome-dur-med) var(--hv-chrome-ease-out);
  }

  .hv-topbar-btn:hover {
    transform: translateY(-4px) scale(1.08);
  }

  /* 9b. Press feedback on the topbar controls — same dip as the dock. */
  .hv-topbar-btn:active,
  .hv-user-pill:active {
    transform: scale(var(--hv-chrome-press));
  }

  /* ============================================================
     TOPBAR CHIPS — icon + label from 901px up
     901 is not a new breakpoint: it is already the shell's "full desktop
     chrome" threshold (`chromeAutoHideEligible`, viewportWidth > 900, in the
     script — the ≤900px maximized-window inset uses the same split). Below
     it the pill cannot afford the labels. This block MUST stay after the
     base `.hv-topbar-btn` rules and after the motion section above: the
     `:hover` override below only wins the equal-specificity tie by coming
     last in the source.
     ============================================================ */
  @media (max-width: 900px) {
    .hv-topbar-btn-label {
      display: none;
    }
  }

  @media (min-width: 901px) {
    .hv-topbar-btn {
      width: auto;
      padding: 0 var(--hv-space-3);
      gap: var(--hv-space-1-5);
    }

    /* A ~137px chip at the shared `scale(1.08)` would grow ~11px and bite
       into the 4px gap between chips — it reads as a wobble, not a lift, so
       the chips keep the translate and drop the scale. `:active` has to be
       restated here: this block is later in the source than rule 9b, so
       without it a hovered + pressed chip would keep the hover transform and
       lose its press dip. */
    .hv-topbar-btn:hover {
      transform: translateY(-4px);
    }
    .hv-topbar-btn:active {
      transform: scale(var(--hv-chrome-press));
    }
  }

  /* 10. Idle dim — dock + topbar fade back after inactivity; any click /
      key / wheel restores them (see markActivity in the script). */
  .hv-menubar.hv-chrome--idle,
  .hv-dock.hv-chrome--idle {
    opacity: var(--hv-chrome-idle-opacity);
  }

  .hv-dock.hv-chrome--idle.hv-dock--hidden {
    opacity: 0;
  }

  /* 11. Clock digit slide — individual digit wrapper. */
  .hv-clock-digit {
    display: inline-block;
    overflow: hidden;
    line-height: 1;
  }

  .hv-clock-digit-inner {
    display: inline-block;
    animation: hvClockSlide 280ms var(--hv-ease-out);
  }

  .hv-clock-sep {
    display: inline-block;
    line-height: 1;
    vertical-align: middle;
  }

  @keyframes hvClockSlide {
    from { transform: translateY(-100%); opacity: 0; }
    to   { transform: translateY(0);     opacity: 1; }
  }

  /* 12. Notification badge — its own shape, no longer rule 7's. The mockup
      pins it to the top-right corner of the Notifications BUTTON (not to the
      22px bell inside it), at ~16px, poking just past the corner. 16px is also
      what the geometry needs: at 901px up the button is an icon + label chip,
      and the badge has to read as a corner marker on the chip rather than a
      blob over its text.

      Why `top: -3px` is enough to keep it off the label, without any horizontal
      gymnastics: the badge spans y = button.top-3 … +13, while the label's line
      box is vertically centred in the 44px button (≈ y = +14.5 … +29.5). The
      badge sits wholly in the band ABOVE the label — it can widen all it wants
      when the count reaches "99+" and still never touch a glyph.
      Horizontally: button.right+3 is inside the pill's 6px padding (+1px
      border), so even the badge's 2px `--hv-shell-glass` ring stays on the
      pill. Do NOT "tidy" an oversized badge with `overflow: hidden` on
      `.hv-menubar` — that would clip the ring and the pop-in.

      The bell still swings once when the count climbs (.hv-bell-swing); the
      count is capped at "99+" in the markup. */
  .hv-notification-badge {
    position: absolute;
    top: -3px;
    right: -3px;
    min-width: 16px;
    height: 16px;
    padding: 0 var(--hv-space-1);
    box-sizing: border-box;
    display: grid;
    place-items: center;
    border-radius: var(--hv-radius-full);
    background: var(--hv-chrome-badge-bg);
    color: var(--hv-chrome-badge-fg);
    font-size: var(--hv-text-xs);
    font-weight: var(--hv-font-weight-bold);
    line-height: 1;
    font-variant-numeric: tabular-nums;
    box-shadow: 0 0 0 var(--hv-space-0-5) var(--hv-shell-glass);
    pointer-events: none;
    animation: hvChromePop var(--hv-chrome-dur-med) var(--hv-chrome-ease-spring) backwards;
  }

  .hv-notif-bell-icon {
    display: inline-flex;
    transform-origin: 50% 15%;
  }

  .hv-bell-swing .hv-notif-bell-icon {
    animation: hvBellSwing 620ms ease-in-out;
  }

  @keyframes hvBellSwing {
    0%, 100% { transform: rotate(0); }
    15% { transform: rotate(16deg); }
    35% { transform: rotate(-13deg); }
    55% { transform: rotate(8deg); }
    75% { transform: rotate(-4deg); }
  }

  @keyframes hvChromePop {
    from { opacity: 0; transform: scale(0.4); }
    to   { opacity: 1; transform: scale(1); }
  }

  /* 13. Topbar background-activity strip — one indeterminate bar on the
      pill whenever a PROGRESS_MANAGER job is running. */
  .hv-menubar-activity {
    position: absolute;
    left: 16px;
    right: 16px;
    bottom: 7px;
    height: 3px;
    border-radius: 999px;
    background: var(--hv-shell-border);
    overflow: hidden;
  }

  .hv-menubar-activity-bar {
    display: block;
    position: absolute;
    top: 0;
    bottom: 0;
    width: 38%;
    border-radius: 999px;
    /* Brand wash via color-mix so the strip follows --hv-primary. */
    background: linear-gradient(90deg, color-mix(in srgb, var(--hv-primary) 0%, transparent), color-mix(in srgb, var(--hv-primary) 95%, transparent), color-mix(in srgb, var(--hv-primary) 0%, transparent));
    animation: hvActivitySlide 1.4s linear infinite;
  }

  @keyframes hvActivitySlide {
    from { transform: translateX(-120%); }
    to   { transform: translateX(330%); }
  }

  /* 13b. Background-activity taskbar — a solid card flush above the dock,
     width bound to the dock (`style="width: {dockWidth}px"`). One row per
     running task: name + % on top, a progress track below. Display-only.
     Floats on the DOCK's band, not the topbar's: it is anchored to the dock
     and must clear it by the dock's height + gap, whatever the pill heights
     become. Radius is the large chrome-surface stop (--hv-radius-4xl). */
  .hv-activity-panel {
    position: fixed;
    left: 50%;
    transform: translateX(-50%);
    bottom: calc(var(--hv-chrome-dock-offset) + var(--hv-chrome-dock-h) + var(--hv-space-3));
    z-index: var(--hv-z-dock);
    box-sizing: border-box;
    min-width: 280px;
    max-width: calc(100vw - 2 * var(--hv-chrome-inset));
    display: flex;
    flex-direction: column;
    gap: var(--hv-space-2-5);
    padding: var(--hv-space-3) var(--hv-space-4);
    border-radius: var(--hv-radius-4xl);
    background: var(--hv-shell-glass);
    border: 1px solid var(--hv-chrome-surface-border);
    box-shadow: var(--hv-chrome-surface-shadow);
    animation: hvChromePop var(--hv-chrome-dur-med) var(--hv-chrome-ease-spring) backwards;
  }
  .hv-activity-panel.hv-chrome--idle { opacity: var(--hv-chrome-idle-opacity); }

  .hv-activity-head {
    font-size: var(--hv-text-xs);
    font-weight: var(--hv-font-weight-bold);
    letter-spacing: 0.06em;
    text-transform: uppercase;
    /* Secondary ink — tertiary grey (3.4:1) is unreadable at 11px. */
    color: var(--hv-shell-text-2);
  }

  .hv-activity-row {
    display: grid;
    grid-template-columns: 1fr auto;
    grid-template-areas:
      "name pct"
      "track track";
    column-gap: var(--hv-space-2-5);
    row-gap: 6px;
    align-items: center;
  }

  .hv-activity-name {
    grid-area: name;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-size: var(--hv-text-sm);
    color: var(--hv-shell-text-strong);
  }

  .hv-activity-pct {
    grid-area: pct;
    font-size: var(--hv-text-xs, 12px);
    font-variant-numeric: tabular-nums;
    color: var(--hv-shell-text-2);
  }

  .hv-activity-track {
    grid-area: track;
    position: relative;
    height: 4px;
    border-radius: 999px;
    background: var(--hv-shell-border);
    overflow: hidden;
  }

  .hv-activity-fill {
    display: block;
    position: absolute;
    top: 0;
    bottom: 0;
    left: 0;
    border-radius: 999px;
    background: var(--hv-primary);
    transition: width 0.3s ease;
  }

  .hv-activity-fill--indet {
    width: 38%;
    left: 0;
    background: linear-gradient(90deg, color-mix(in srgb, var(--hv-primary) 0%, transparent), color-mix(in srgb, var(--hv-primary) 95%, transparent), color-mix(in srgb, var(--hv-primary) 0%, transparent));
    animation: hvActivitySlide 1.4s linear infinite;
    transition: none;
  }

  /* 14. (removed 2026-09-11 — the glass fallback block is moot now that
      the chrome is solid Fluent paper; no backdrop-filter remains.) */

  /* 15. Flatten the JS-driven / one-shot pieces under reduced motion (the
      global rule in base.css covers plain CSS animation; these need an
      explicit steady-state). */
  @media (prefers-reduced-motion: reduce) {
    .hv-menubar-activity-bar {
      position: static;
      width: 100%;
      animation: none;
      opacity: 0.6;
    }
    .hv-activity-fill--indet {
      width: 100%;
      animation: none;
      opacity: 0.6;
    }
    .hv-activity-panel { animation: none; }
    /* The global rule zeroes animation duration but not delay, so a
       backwards-filled entrance would still hide the icons briefly. */
    .hv-dock-item,
    .hv-dock-item--enter {
      animation: none;
    }
    .hv-dock-attention .hv-dock-icon { animation: none; }
  }


  /* ── Power Options Modal — light solid card ── */

  /* Overlay */
  .hv-power-overlay {
    position: fixed;
    inset: 0;
    background: var(--hv-modal-backdrop);
    display: flex;
    align-items: center;
    justify-content: center;
	    z-index: var(--hv-z-power-overlay);
  }

  /* Dialog — light solid card */
  .hv-power-dialog {
    width: min(380px, calc(100vw - 2 * var(--hv-chrome-inset) - 16px));
    padding: var(--hv-space-4);
    background: var(--hv-shell-glass);
    border: 1px solid var(--hv-shell-border);
    border-radius: 28px;
    box-shadow: var(--hv-shell-shadow-card);
    animation: hvPowerAppear 200ms ease-out;
    overflow: hidden;
    color: var(--hv-shell-text-strong);
  }

  @keyframes hvPowerAppear {
    from {
      opacity: 0;
      transform: scale(0.95) translateY(-20px);
    }
    to {
      opacity: 1;
      transform: scale(1) translateY(0);
    }
  }

  /* Header — flush to the card edges, rounded top corners */
  .hv-power-header {
    display: flex;
    align-items: center;
    gap: 12px;
    margin: calc(var(--hv-space-4) * -1) calc(var(--hv-space-4) * -1) var(--hv-space-3);
    padding: var(--hv-space-2) var(--hv-space-3) var(--hv-space-2) var(--hv-space-4);
    background: var(--hv-shell-fill);
    border-bottom: 1px solid var(--hv-shell-border-soft);
    border-radius: 28px 28px 0 0;
    font-size: var(--hv-text-md);
    font-weight: var(--hv-font-weight-semibold);
  }

  .hv-power-title {
    flex: 1;
    display: flex;
    align-items: center;
    gap: var(--hv-space-2);
    font-size: var(--hv-text-md);
    font-weight: var(--hv-font-weight-semibold);
    color: var(--hv-shell-text-strong);
  }

  .hv-power-title svg {
    width: 19px;
    height: 19px;
    stroke: currentColor;
  }

  .hv-power-close {
    width: 44px;
    height: 44px;
    flex: none;
    border: none;
    background: var(--hv-shell-fill);
    border-radius: 50%;
    cursor: pointer;
    display: grid;
    place-items: center;
    transition:
      color var(--hv-transition-base) var(--hv-ease-default),
      background-color var(--hv-transition-base) var(--hv-ease-default),
      border-color var(--hv-transition-base) var(--hv-ease-default),
      box-shadow var(--hv-transition-base) var(--hv-ease-default);
    color: var(--hv-shell-text-2);
  }

  .hv-power-close:hover {
    background: var(--hv-shell-border);
    color: var(--hv-shell-text-strong);
  }

  .hv-power-close svg {
    width: 20px;
    height: 20px;
  }

  /* Content — the card's own 18px padding frames it, no extra inset */
  .hv-power-content {
    padding: 0;
  }

  /* Power Options — 3 compact rounded-rectangle buttons */
  .hv-power-options {
    display: grid;
    grid-template-columns: repeat(3, 1fr);
    gap: var(--hv-space-2-5);
  }

  .hv-power-option {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: var(--hv-space-2);
    padding: 16px 8px;
    border: 1px solid var(--hv-shell-border);
    border-radius: var(--hv-radius-3xl);
    background: var(--hv-shell-card);
    color: var(--hv-shell-text-strong);
    cursor: pointer;
    transition:
      color var(--hv-transition-base) var(--hv-ease-default),
      background-color var(--hv-transition-base) var(--hv-ease-default),
      border-color var(--hv-transition-base) var(--hv-ease-default),
      box-shadow var(--hv-transition-base) var(--hv-ease-default);
    width: 100%;
    text-align: center;
    font: inherit;
  }

  .hv-power-option:hover {
    background: var(--hv-shell-border-soft);
  }

  .hv-power-option svg {
    width: 26px;
    height: 26px;
    stroke: currentColor;
    flex-shrink: 0;
  }

  .hv-power-option-shutdown {
    background: var(--hv-error);
    color: var(--hv-on-error);
    border-color: transparent;
  }

  .hv-power-option-shutdown:hover {
    background: var(--hv-error-hover);
  }

  .hv-power-label {
    font-size: var(--hv-text-sm);
    font-weight: var(--hv-font-weight-semibold);
    color: inherit;
    margin: 0;
  }

  /* Footer — flush to the card edges (confirmation dialog) */
  .hv-power-footer {
    display: flex;
    justify-content: flex-end;
    gap: var(--hv-space-2);
    margin: var(--hv-space-3) calc(var(--hv-space-4) * -1) calc(var(--hv-space-4) * -1);
    padding: var(--hv-space-3) var(--hv-space-4);
    border-top: 1px solid var(--hv-shell-border-soft);
  }

  .hv-power-btn {
    padding: var(--hv-space-2) 20px;
    border: none;
    border-radius: var(--hv-radius-md);
    font-size: var(--hv-font-size-base);
    font-weight: var(--hv-font-weight-medium);
    cursor: pointer;
    transition:
      opacity var(--hv-transition-base) var(--hv-ease-default),
      background-color var(--hv-transition-base) var(--hv-ease-default);
    background: var(--hv-primary, var(--hv-primary));
    color: white;
  }

  .hv-power-btn:hover {
    opacity: 0.9;
  }

  .hv-power-btn-secondary {
    background: var(--hv-shell-fill);
    border: 1px solid var(--hv-shell-border-strong);
    color: var(--hv-shell-text-strong);
  }

  .hv-power-btn-secondary:hover {
    background: var(--hv-shell-border);
  }

  .hv-power-btn-danger {
    background: var(--hv-error);
  }

  .hv-power-btn-danger:hover {
    background: var(--hv-error-hover);
  }

  .hv-power-btn-warning {
    background: var(--hv-warning);
  }

  .hv-power-btn-warning:hover {
    background: var(--hv-warning-hover);
  }

  /* Confirm text */
  .hv-confirm-text {
    font-size: var(--hv-font-size-base);
    color: var(--hv-shell-text-2);
    margin: 0;
    padding: 2px 0 6px;
    line-height: var(--hv-line-height-normal);
  }

  @media (max-width: 480px) {
    .hv-power-options {
      grid-template-columns: 1fr;
    }
  }

  /* ============================================================
     FLOATING WIDGET PANELS — solid white cards
     The 11 draggable monitor panels (Resources, RAID, Security,
     VPN, Downloads, Syncthing, Calendar, Containers, Weather,
     Sessions, Health) now share the same light solid-white
     material as the app shells / topbar / dock (--hv-shell-*):
     opaque white paper with ink text, no blur. Appended last so it
     overrides the per-panel light rules above at equal
     specificity — and any panel's own light-era colour comes
     through from the base rules underneath.
     ============================================================ */
  .hv-resource-monitor,
  .hv-raid-monitor,
  .hv-security-monitor,
  .hv-vpn-monitor,
  .hv-downloader-monitor,
  .hv-syncthing-monitor,
  .hv-calendar-monitor,
  .hv-docker-monitor,
  .hv-weather-monitor,
  .hv-sessions-monitor,
  .hv-health-monitor {
    background: var(--hv-shell-glass);
    border: 1px solid var(--hv-shell-border);
    /* Floating widget-card surface — same card corner as the in-window cards
       (--hv-radius-card, 12px), not a control radius. */
    border-radius: var(--hv-radius-card);
    box-shadow: var(--hv-shell-shadow-card);
    color: var(--hv-shell-text-strong);
    overflow: hidden;
  }

  .hv-calendar-nav-btn,
  .hv-docker-action-btn {
    color: var(--hv-shell-text-2);
  }

  .hv-calendar-nav-btn:hover,
  .hv-docker-action-btn:hover:not(:disabled) {
    background: var(--hv-subtle-hover);
    color: var(--hv-shell-text-strong);
  }

  /* Primary body text -> ink */
  .hv-network-value,
  .hv-raid-name-text,
  .hv-raid-info-value,
  .hv-security-item-name,
  .hv-calendar-cell,
  .hv-calendar-nav-label,
  .hv-docker-item-name,
  .hv-weather-temp,
  .hv-weather-condition,
  .hv-sessions-count,
  .hv-vpn-row-label,
  .hv-downloader-active,
  .hv-downloader-speed-value {
    color: var(--hv-shell-text-strong);
  }

  .hv-gauge-value {
    fill: var(--hv-shell-text-strong);
  }

  /* Secondary / muted text -> ink at low alpha */
  .hv-gauge-label,
  .hv-network-label,
  .hv-raid-progress-used,
  .hv-raid-empty,
  .hv-security-status-off,
  .hv-security-item-detail,
  .hv-security-empty,
  .hv-calendar-weekday,
  .hv-docker-empty,
  .hv-weather-empty,
  .hv-weather-location,
  .hv-weather-details,
  .hv-sessions-empty,
  .hv-sessions-label,
  .hv-health-empty,
  .hv-health-stat-label,
  .hv-vpn-empty,
  .hv-downloader-empty,
  .hv-syncthing-empty,
  .hv-vpn-row-value,
  .hv-downloader-speed-label,
  .hv-syncthing-detail {
    color: var(--hv-shell-text-2);
  }

  /* Inner cards / rows — raised white-on-white fill */
  .hv-network-item,
  .hv-raid-item,
  .hv-security-item,
  .hv-docker-item,
  .hv-vpn-row,
  .hv-downloader-speed-item {
    background: var(--hv-shell-card);
    border-color: var(--hv-shell-border);
  }

  /* Tracks / dividers */
  .hv-raid-progress-bar {
    background: var(--hv-shell-border-soft);
  }

  .hv-gauge-track {
    stroke: var(--hv-shell-border-strong);
  }

  .hv-raid-info,
  .hv-calendar-nav,
  .hv-health-overall {
    border-color: var(--hv-shell-border-soft);
  }

  /* Status text — deep semantic tones readable on the white paper */
  .hv-raid-status-healthy,
  .hv-security-status-ok,
  .hv-syncthing-status,
  .hv-health-overall-ok {
    color: var(--hv-success-text);
  }

  .hv-raid-status-degraded,
  .hv-security-status-warn,
  .hv-syncthing-status-syncing,
  .hv-health-overall-warn {
    color: var(--hv-warning-text);
  }

  .hv-raid-status-failed,
  .hv-health-overall-crit {
    color: var(--hv-error-text);
  }

  .hv-security-status-off .hv-security-status-dot,
  .hv-health-overall-dot {
    background: var(--hv-shell-text-3);
  }

  /* Badges / callouts */
  .hv-raid-type {
    background: var(--hv-primary-selected-bg);
    color: var(--hv-primary);
  }

  .hv-system-update {
    background: var(--hv-primary-selected-bg);
    border-color: var(--hv-primary-light);
  }

  .hv-system-update-text {
    color: var(--hv-primary);
  }

  .hv-system-update.available {
    background: var(--hv-warning-bg);
    border-color: var(--hv-warning-border);
  }

  .hv-system-update.available .hv-system-update-text {
    color: var(--hv-warning-text);
  }

  .hv-calendar-cell-today {
    background: var(--hv-primary);
    color: var(--hv-on-primary);
  }

  /* A touch wider than the compact 280px stop so a translated title + count
     fit the bleeding header on one line (e.g. FR "Santé système" + count) */
  .hv-health-monitor,
  .hv-sessions-monitor,
  .hv-weather-monitor {
    width: var(--hv-monitor-w);
  }

  /* Gauge readouts tinted to match their ring */
  .hv-gauge-value-cpu {
    fill: var(--hv-primary);
  }
  .hv-gauge-value-ram {
    fill: #7c8cf0;
  }
  .hv-gauge-value-temp {
    fill: var(--hv-warning);
  }

  /* Status indicators as tinted pills (light surfaces) */
  .hv-raid-status,
  .hv-security-status,
  .hv-syncthing-status {
    padding: var(--hv-space-0-75) var(--hv-space-2-5);
    border-radius: 999px;
    width: fit-content;
    max-width: 100%;
  }

  .hv-raid-status-healthy,
  .hv-security-status-ok,
  .hv-syncthing-status:not(.hv-syncthing-status-syncing) {
    background: var(--hv-success-bg);
  }

  .hv-raid-status-degraded,
  .hv-security-status-warn,
  .hv-syncthing-status-syncing {
    background: var(--hv-warning-bg);
  }

  .hv-raid-status-failed {
    background: var(--hv-error-bg);
  }

  .hv-security-status-off {
    background: var(--hv-shell-fill);
  }

  /* Health overall as a full-width banner */
  .hv-health-overall {
    padding: var(--hv-space-3) var(--hv-space-3);
    border-radius: var(--hv-radius-xl);
    border-bottom: none;
    background: var(--hv-shell-card);
  }

  /* ============================================================
     TABLET CHROME (481–900px) — touch-first, fits ~768px wide
     The full desktop pill overflows a tablet viewport (avatar + name +
     clock + 4 buttons ≈ 820px against ~720px available): drop the user
     name, slim the clock, and step the buttons down to 52px. All targets
     stay ≥ 44px (WCAG AA). Hover lift is preserved — a mouse-equipped
     tablet still gets it.
     Must stay BEFORE the phone block below (source-order ties).
     ============================================================ */
  @media (min-width: 481px) and (max-width: 900px) {
    /* Avatar-only identity — the full name + dropdown stay one tap away. */
    .hv-user-pill-name {
      display: none;
    }

    .hv-user-pill {
      padding: var(--hv-space-1);
    }

    .hv-menubar-clock {
      min-width: 0;
    }

    /* One step down from the 14px desktop clock. */
    .hv-menubar-time {
      font-size: var(--hv-text-sm);
    }

    .hv-menubar-date {
      font-size: var(--hv-text-xs);
    }

    .hv-topbar-btn :global(svg) {
      width: 20px;
      height: 20px;
    }

    /* The dock keeps its 48px tiles and its own height token; only the
       gutter tightens, so the same row fits a 768px viewport. */
    .hv-dock {
      padding: var(--hv-space-2-5) var(--hv-space-3);
    }
  }

  /* ============================================================
     TOUCH CHROME — any pointer without hover (phones, touch tablets)
     :hover states stick after a tap on touch screens (lifted dock icon
     frozen mid-air, tooltip pinned open): flatten them to the tap state
     only. The :active dip + ripple + entrance choreography stay.
     Must stay BEFORE the phone block below (source-order ties).
     ============================================================ */
  @media (hover: none) {
    .hv-dock-item:hover {
      transform: none;
    }

    .hv-dock-item:hover .hv-dock-icon {
      box-shadow: var(--hv-shadow-md);
      filter: none;
    }

    /* No hover, no tooltip — and a label pinned open by a sticky :hover
       would sit on top of the dock for good. */
    .hv-dock-item::before {
      display: none;
    }

    .hv-topbar-btn:hover {
      transform: none;
    }
  }

  /* ============================================================
     PHONE CHROME OVERRIDES — kept last on purpose
     Every base rule for .hv-dock-icon / .hv-menubar-* / the
     monitor panels appears earlier in this file, so these
     responsive overrides only win the equal-specificity tie
     when they come after all of them.
     ============================================================ */

  /* Floating draggable desktop widgets are a pointer / large-screen feature.
     On a phone they can only pile up on top of each other and the content
     underneath — hide them (the Widgets app still toggles their state for
     when the window is wide again). */
  @media (max-width: 640px) {
    .hv-resource-monitor,
    .hv-raid-monitor,
    .hv-security-monitor,
    .hv-vpn-monitor,
    .hv-downloader-monitor,
    .hv-syncthing-monitor,
    .hv-calendar-monitor,
    .hv-docker-monitor,
    .hv-weather-monitor,
    .hv-sessions-monitor,
    .hv-health-monitor {
      display: none;
    }
  }

  @media (max-width: 480px) {
    /* The pill spans the full width and space-betweens its buttons so
       nothing spills off either edge. */
    .hv-menubar {
      left: var(--hv-chrome-inset);
      padding: 0 var(--hv-space-2-5);
      border-radius: var(--hv-radius-2xl);
    }

    .hv-menubar-inner {
      width: 100%;
      justify-content: space-between;
      gap: var(--hv-space-1-5);
    }

    /* The OS status bar already shows the time on a phone. */
    .hv-menubar-clock,
    .hv-menubar-divider {
      display: none;
    }

    .hv-user-pill-name {
      display: none;
    }

    .hv-user-pill {
      padding: var(--hv-space-1);
    }

    .hv-topbar-btn {
      width: 44px;
      height: 44px;
      border-radius: var(--hv-radius-xl);
    }

    .hv-topbar-btn :global(svg) {
      width: 22px;
      height: 22px;
    }

    .hv-dock {
      /* Height comes from --hv-chrome-dock-h (56px on phone); the 44px icons
         are centred in it by align-items, so the vertical padding drops to
         zero — the desktop value would make the content taller than the box.
         Radius matches the phone topbar pill so the two pills stay identical. */
      padding: 0 var(--hv-space-2-5);
      border-radius: var(--hv-radius-2xl);
    }

    /* Decorative hairlines — drop them so the 7 real items fit the width. */
    .hv-dock-divider {
      display: none;
    }

    /* `.hv-dock-container` is the flex row — ITS gap, not `.hv-dock`'s, spaces
       the items. Tighten it back below the desktop value and shrink the items
       so the 7 defaults (5 pinned + Applications + Settings) fit ~360px; any
       extra (minimized-window buttons) scroll the row inside the pill. Safe to
       go tight here: this block already flattens the hover lift, so no tile
       ever grows into its neighbour on touch. */
    .hv-dock-container {
      gap: var(--hv-space-1-5);
      overflow-x: auto;
      overflow-y: hidden;
      scrollbar-width: none;
      max-width: 100%;
    }

    .hv-dock-container::-webkit-scrollbar {
      display: none;
    }

    .hv-dock-item {
      width: 44px;
    }

    .hv-dock-icon {
      width: 44px;
      height: 44px;
      border-radius: var(--hv-radius-xl);
    }

    .hv-dock-icon :global(svg) {
      width: 22px;
      height: 22px;
    }

    /* Pull the running-indicator bar inside the icon so `overflow-y: hidden`
       on the scroll row above doesn't clip it. */
    .hv-dock-dot {
      bottom: 5px;
      height: 3px;
    }

    /* Same reason for the dock count badge: on the 44/46px phone dock tiles
       the 26px desktop badge overflows the top edge and the dock's
       `overflow-y:hidden` clips it — shrink it and tuck it flush into the
       corner. The notification badge is deliberately NOT in this selector: it
       is already 16px at `top: -3px` (see rule 12) and the phone topbar is
       still 56px tall, so its button keeps ~3px of headroom without help. */
    .hv-dock-badge {
      top: 0;
      right: 0;
      min-width: 20px;
      height: 20px;
      padding: 0 var(--hv-space-1);
      font-size: var(--hv-text-xs);
    }

    /* No hover-lift on touch — and it would be clipped by the scroll row. */
    .hv-dock-item:hover,
    .hv-dock-item:hover .hv-dock-icon {
      transform: none;
    }
  }
</style>

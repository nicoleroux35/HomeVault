import {
  IconActivity,
  IconCalendar,
  IconContainer as ContainerIcon,
  IconDatabase,
  IconDownload,
  IconHeartbeat,
  IconLock,
  IconRefresh,
  IconShieldCheckered,
  IconSun,
  IconUsers
} from '@tabler/icons-svelte';
import { monitorStore, type MonitorVisibilityKey } from './monitorStore';

/**
 * The catalog of desktop monitor widgets HomeVault offers.
 *
 * One entry per monitor — this is the single source of truth shared by the
 * Widgets app (`routes/Widgets.svelte`, which renders the catalog grid) and
 * Settings > Widgets (`routes/Settings.svelte`, which renders the same list
 * as toggle rows). Adding a monitor means adding one entry here and its
 * `widgets.<id>_monitor_{name,desc}` i18n keys — never another copy-pasted
 * tile/row block.
 *
 * The entry deliberately holds no `visible` flag: visibility is live state
 * owned by `monitorStore` (persisted to `/api/monitor-layout`, not to the
 * component), so each consumer reads it through `isWidgetVisible()` and
 * writes it through the entry's `setVisible`.
 */
export interface WidgetDef {
  /** Stable id, also the `widgets.<id>_monitor_*` i18n key prefix. */
  id: string;
  /** Tabler glyph rendered on the tile / row. */
  icon: any;
  /** Accent used for the icon tile and, in Settings, the row's toggle. */
  color: string;
  nameKey: string;
  descKey: string;
  /** The `MonitorPositions` boolean this widget's visibility lives in. */
  visibleKey: MonitorVisibilityKey;
  /** Persist a new visibility — routes through `monitorStore`. */
  setVisible: (visible: boolean) => void;
}

export const WIDGET_DEFS: WidgetDef[] = [
  {
    id: 'resource',
    icon: IconActivity,
    color: '#1E62AE',
    nameKey: 'widgets.resource_monitor_name',
    descKey: 'widgets.resource_monitor_desc',
    visibleKey: 'resourceMonitorVisible',
    setVisible: monitorStore.setResourceVisible
  },
  {
    id: 'raid',
    icon: IconDatabase,
    color: '#6366f1',
    nameKey: 'widgets.raid_monitor_name',
    descKey: 'widgets.raid_monitor_desc',
    visibleKey: 'raidMonitorVisible',
    setVisible: monitorStore.setRaidVisible
  },
  {
    id: 'security',
    icon: IconShieldCheckered,
    color: '#C22B21',
    nameKey: 'widgets.security_monitor_name',
    descKey: 'widgets.security_monitor_desc',
    visibleKey: 'securityMonitorVisible',
    setVisible: monitorStore.setSecurityVisible
  },
  {
    id: 'vpn',
    icon: IconLock,
    color: '#059669',
    nameKey: 'widgets.vpn_monitor_name',
    descKey: 'widgets.vpn_monitor_desc',
    visibleKey: 'vpnMonitorVisible',
    setVisible: monitorStore.setVpnVisible
  },
  {
    id: 'downloader',
    icon: IconDownload,
    color: '#22c55e',
    nameKey: 'widgets.downloader_monitor_name',
    descKey: 'widgets.downloader_monitor_desc',
    visibleKey: 'downloaderMonitorVisible',
    setVisible: monitorStore.setDownloaderVisible
  },
  {
    id: 'syncthing',
    icon: IconRefresh,
    color: '#0ea5e9',
    nameKey: 'widgets.syncthing_monitor_name',
    descKey: 'widgets.syncthing_monitor_desc',
    visibleKey: 'syncthingMonitorVisible',
    setVisible: monitorStore.setSyncthingVisible
  },
  {
    id: 'calendar',
    icon: IconCalendar,
    color: '#d97706',
    nameKey: 'widgets.calendar_monitor_name',
    descKey: 'widgets.calendar_monitor_desc',
    visibleKey: 'calendarMonitorVisible',
    setVisible: monitorStore.setCalendarVisible
  },
  {
    id: 'docker',
    icon: ContainerIcon,
    color: '#2496ed',
    nameKey: 'widgets.docker_monitor_name',
    descKey: 'widgets.docker_monitor_desc',
    visibleKey: 'dockerMonitorVisible',
    setVisible: monitorStore.setDockerVisible
  },
  {
    id: 'weather',
    icon: IconSun,
    color: '#0284c7',
    nameKey: 'widgets.weather_monitor_name',
    descKey: 'widgets.weather_monitor_desc',
    visibleKey: 'weatherMonitorVisible',
    setVisible: monitorStore.setWeatherVisible
  },
  {
    id: 'sessions',
    icon: IconUsers,
    color: '#7c3aed',
    nameKey: 'widgets.sessions_monitor_name',
    descKey: 'widgets.sessions_monitor_desc',
    visibleKey: 'sessionsMonitorVisible',
    setVisible: monitorStore.setSessionsVisible
  },
  {
    id: 'health',
    icon: IconHeartbeat,
    color: '#e11d48',
    nameKey: 'widgets.health_monitor_name',
    descKey: 'widgets.health_monitor_desc',
    visibleKey: 'healthMonitorVisible',
    setVisible: monitorStore.setHealthVisible
  }
];

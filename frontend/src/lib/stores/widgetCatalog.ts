/* =============================================================================
 * File          : widgetCatalog.ts
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 28/09/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Single source of truth for the desktop monitor widgets, shared by the
 *   Widgets app and Settings → Widgets (one entry + its i18n keys — never a
 *   copy-pasted tile/row). Deliberately holds no visibility flag: live
 *   visibility belongs to monitorStore, read through isWidgetVisible() and
 *   written through each entry's setVisible.
 *
 * Dependencies:
 *   - $lib/icons/lucide, svelte (>= 5), stores/monitorStore
 *
 * Usage:
 *   WIDGET_DEFS in routes/Widgets.svelte, Settings.svelte, widgetLayout.
 * =============================================================================
 */

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
} from '$lib/icons/lucide';
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
    color: '#2E72C9',
    nameKey: 'widgets.resource_monitor_name',
    descKey: 'widgets.resource_monitor_desc',
    visibleKey: 'resourceMonitorVisible',
    setVisible: monitorStore.setResourceVisible
  },
  {
    id: 'raid',
    icon: IconDatabase,
    color: '#6A45D6',
    nameKey: 'widgets.raid_monitor_name',
    descKey: 'widgets.raid_monitor_desc',
    visibleKey: 'raidMonitorVisible',
    setVisible: monitorStore.setRaidVisible
  },
  {
    id: 'security',
    icon: IconShieldCheckered,
    color: '#C0392B',
    nameKey: 'widgets.security_monitor_name',
    descKey: 'widgets.security_monitor_desc',
    visibleKey: 'securityMonitorVisible',
    setVisible: monitorStore.setSecurityVisible
  },
  {
    id: 'vpn',
    icon: IconLock,
    color: '#167A4D',
    nameKey: 'widgets.vpn_monitor_name',
    descKey: 'widgets.vpn_monitor_desc',
    visibleKey: 'vpnMonitorVisible',
    setVisible: monitorStore.setVpnVisible
  },
  {
    id: 'downloader',
    icon: IconDownload,
    color: '#2560AC',
    nameKey: 'widgets.downloader_monitor_name',
    descKey: 'widgets.downloader_monitor_desc',
    visibleKey: 'downloaderMonitorVisible',
    setVisible: monitorStore.setDownloaderVisible
  },
  {
    id: 'syncthing',
    icon: IconRefresh,
    color: '#96601A',
    nameKey: 'widgets.syncthing_monitor_name',
    descKey: 'widgets.syncthing_monitor_desc',
    visibleKey: 'syncthingMonitorVisible',
    setVisible: monitorStore.setSyncthingVisible
  },
  {
    id: 'calendar',
    icon: IconCalendar,
    color: '#2E72C9',
    nameKey: 'widgets.calendar_monitor_name',
    descKey: 'widgets.calendar_monitor_desc',
    visibleKey: 'calendarMonitorVisible',
    setVisible: monitorStore.setCalendarVisible
  },
  {
    id: 'docker',
    icon: ContainerIcon,
    color: '#6A45D6',
    nameKey: 'widgets.docker_monitor_name',
    descKey: 'widgets.docker_monitor_desc',
    visibleKey: 'dockerMonitorVisible',
    setVisible: monitorStore.setDockerVisible
  },
  {
    id: 'weather',
    icon: IconSun,
    color: '#0E7F92',
    nameKey: 'widgets.weather_monitor_name',
    descKey: 'widgets.weather_monitor_desc',
    visibleKey: 'weatherMonitorVisible',
    setVisible: monitorStore.setWeatherVisible
  },
  {
    id: 'sessions',
    icon: IconUsers,
    color: '#6A45D6',
    nameKey: 'widgets.sessions_monitor_name',
    descKey: 'widgets.sessions_monitor_desc',
    visibleKey: 'sessionsMonitorVisible',
    setVisible: monitorStore.setSessionsVisible
  },
  {
    id: 'health',
    icon: IconHeartbeat,
    color: '#167A4D',
    nameKey: 'widgets.health_monitor_name',
    descKey: 'widgets.health_monitor_desc',
    visibleKey: 'healthMonitorVisible',
    setVisible: monitorStore.setHealthVisible
  }
];

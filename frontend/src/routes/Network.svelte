<!-- ========================================================================
     File          : Network.svelte
     Author        : Nicolas Le Roux
     Created       : 28/09/2026
     Last modified : 28/09/2026 by Nicolas Le Roux
     Version       : 1.0.0

     Description:
     The Network app: interface inventory and details, configuration editor
     (IP/gateway/DNS), Wi-Fi scan and association, diagnostics, public-IP
     and service status.

     Dependencies:
       - svelte (>= 5), theme + app/ composites, $lib/services/api

     Usage:
     Opened as a desktop window app.
     ======================================================================== -->
<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { api } from '$lib/services/api';
  import { t, translate } from '$lib/stores/i18n';
  import { Spinner, Badge, Modal, Button } from '$lib/components/theme';
  import { PageLayout, StatCard, EmptyState, ConfirmDialog } from '$lib/components/app';
  import { createConfirmState } from '$lib/utils/confirmDialog.svelte';
  import { notifyError, notifySuccess } from '$lib/utils/notify';
  import { cn } from '$lib/utils.js';
  import { activateOnEnterOrSpace } from '$lib/utils/activateOnEnterOrSpace';
  import WindowHeader from '$lib/components/desktop/WindowHeader.svelte';
  import { IconPlug, IconDownload, IconLoader, IconUpload, IconWifi, IconWifiOff, IconX, IconGlobe, IconActivity, IconRadio, IconGauge, IconLink, IconSitemap, IconTrash, IconPlus, IconRoute } from '@tabler/icons-svelte';
  import { notificationsStore } from '$lib/stores/notifications';

  // ---------------------------------------------------------------------------
  // Types (mirroring Rust backend)
  // ---------------------------------------------------------------------------

  type InterfaceType = 'ethernet' | 'wifi' | 'bridge' | 'vlan' | 'bond' | 'tunnel' | 'loopback' | 'unknown';
  type InterfaceStatus = 'up' | 'down' | 'unknown';

  interface NetworkInterface {
    name: string;
    iface_type: InterfaceType;
    mac: string | null;
    status: InterfaceStatus;
    ipv4: string | null;
    ipv4_netmask: string | null;
    ipv6: string | null;
    mtu: number | null;
    speed_mbps: number | null;
    duplex: string | null;
    rx_bytes: number;
    tx_bytes: number;
    driver: string | null;
  }

  interface InterfaceConfig {
    method: 'dhcp' | 'static';
    ip: string | null;
    netmask: string | null;
    gateway: string | null;
    dns_servers: string[];
  }

  /** A bond device as `GET /api/network/bonds` answers it. */
  interface BondDevice {
    name: string;
    mode: '802.3ad' | 'active-backup';
    members: string[];
    up: boolean;
    ipv4: string | null;
  }

  /** A saved static route as `GET /api/network/routes` answers it. */
  interface StaticRoute {
    destination: string;
    via: string;
    metric: number | null;
    interface: string;
    active: boolean;
  }

  /** A VLAN device as `GET /api/network/vlans` answers it. */
  interface VlanDevice {
    name: string;
    parent: string;
    vid: number;
    up: boolean;
    ipv4: string | null;
  }

  interface WifiNetwork {
    ssid: string;
    bssid: string;
    signal_dbm: number;
    signal_percent: number;
    frequency_mhz: number;
    channel: number;
    security: string[];
    connected: boolean;
  }

  interface WifiStatus {
    connected: {
      ssid: string;
      bssid: string | null;
      signal_dbm: number | null;
      ip: string | null;
    } | null;
    interface: string | null;
  }

  // ---------------------------------------------------------------------------
  // State
  // ---------------------------------------------------------------------------

  let interfaces: NetworkInterface[] = $state([]);
  let selectedInterface: NetworkInterface | null = $state(null);
  let interfaceConfig: InterfaceConfig | null = $state(null);

  let wifiNetworks: WifiNetwork[] = $state([]);
  let wifiStatus: WifiStatus | null = $state(null);
  let scanningWifi = $state(false);
  let connectingWifi = $state(false);

  let loading = $state(true);
  let saving = $state(false);
  let activeTab: 'ethernet' | 'wifi' = $state('ethernet');

  // Connection summary / diagnostics
  let connectionSummary: {
    local_ip: string | null;
    gateway: string | null;
    dns_servers: string[];
    public_ip: string | null;
    primary_interface: string | null;
    connection_type: string;
  } | null = $state(null);
  let publicIp: string | null = $state(null);
  let diagnostics: {
    gateway_reachable: boolean;
    dns_working: boolean;
    internet_reachable: boolean;
    latency_ms: number | null;
    message: string;
  } | null = $state(null);
  let runningDiagnostics = $state(false);

  // Auto-refresh interval
  let refreshInterval: ReturnType<typeof setInterval>;

  // Modal state
  let showConfigModal = $state(false);
  let showWifiConnectModal = $state(false);
  let wifiModalMode: 'connect' | 'disconnect' = $state('connect');
  let selectedWifi: WifiNetwork | null = $state(null);
  let wifiPassword = $state('');
  let wifiSecurity = $state('wpa2');
  let wifiScanned = $state(false);

  // Config form
  let configMethod: 'dhcp' | 'static' = $state('dhcp');
  let configIp = $state('');
  let configNetmask = $state('255.255.255.0');
  let configGateway = $state('');
  let configDns = $state('');

  // IPv6 config form
  let configIpv6Method: 'auto' | 'dhcp' | 'static' | 'disabled' = $state('auto');
  let configIpv6 = $state('');
  let configIpv6Prefix = $state('64');
  let configIpv6Gateway = $state('');
  let configIpv6Dns = $state('');

  // ---------------------------------------------------------------------------
  // Lifecycle
  // ---------------------------------------------------------------------------

  onMount(async () => {
    await loadAll();
    // Refresh every 10 seconds (background, no full-page spinner)
    refreshInterval = setInterval(() => {
      refreshData();
    }, 10000);
  });

  onDestroy(() => {
    if (refreshInterval) clearInterval(refreshInterval);
  });

  // ---------------------------------------------------------------------------
  // Data Loading
  // ---------------------------------------------------------------------------

  /** Surfaces a failed network action via the notification center — every
      caller previously left its `else` branch empty, so a bad interface
      name, a missing tool, or any other backend error left the spinner
      stopping with zero feedback (2026-09-05, same bug class already fixed
      in Rescue.svelte, found while writing this page's E2E suite). */
  // --- Link aggregation and VLAN devices (B5 / B6) ------------------------
  /** Deleting a bond or a VLAN takes links down: confirm, then say what happened. */
  const dlg = createConfirmState();
  let bonds = $state<BondDevice[]>([]);
  let bondCandidates = $state<string[]>([]);
  let vlans = $state<VlanDevice[]>([]);
  let vlanParents = $state<string[]>([]);
  let maxVid = $state(4094);

  let bondName = $state('');
  let bondMode = $state<'802.3ad' | 'active-backup'>('802.3ad');
  let bondMembers = $state<string[]>([]);
  let bondError = $state('');
  let savingBond = $state(false);

  let vlanParent = $state('');
  let vlanId = $state('10');
  let vlanError = $state('');
  let savingVlan = $state(false);

  let routes = $state<StaticRoute[]>([]);
  let routesAvailable = $state(true);
  let routeDestination = $state('');
  let routeVia = $state('');
  let routeMetric = $state('');
  let routeInterface = $state('');
  let routeError = $state('');
  let savingRoute = $state(false);

  $effect(() => {
    if (!routeInterface && interfaces.length > 0) {
      routeInterface = interfaces[0].name;
    }
  });

  async function loadBonds() {
    const response = await api.getNetworkBonds();
    if (response.success && response.data) {
      bonds = response.data.bonds || [];
      bondCandidates = response.data.candidates || [];
      // A member that is gone from the candidate list (already enslaved
      // elsewhere, or unplugged) must not stay selected.
      bondMembers = bondMembers.filter((m) => bondCandidates.includes(m));
    }
  }

  async function loadVlans() {
    const response = await api.getNetworkVlans();
    if (response.success && response.data) {
      vlans = response.data.vlans || [];
      vlanParents = response.data.parents || [];
      maxVid = response.data.max_vid ?? 4094;
      if (!vlanParents.includes(vlanParent)) vlanParent = vlanParents[0] || '';
    }
  }

  function toggleBondMember(name: string) {
    bondMembers = bondMembers.includes(name)
      ? bondMembers.filter((m) => m !== name)
      : [...bondMembers, name];
  }

  async function createBond() {
    bondError = '';
    if (bondMembers.length === 0) {
      bondError = $t('network.bond_need_member');
      return;
    }
    savingBond = true;
    const response = await api.createNetworkBond({
      ...(bondName.trim() ? { name: bondName.trim() } : {}),
      mode: bondMode,
      members: bondMembers,
    });
    savingBond = false;
    if (!response.success) {
      bondError = response.message || $t('network.bond_create_failed');
      return;
    }
    bondName = '';
    bondMembers = [];
    await Promise.all([loadBonds(), loadInterfaces(false)]);
    notifySuccess(response.message || $t('network.bond_created'));
  }

  async function deleteBond(name: string) {
    if (!(await dlg.ask(translate('network.bond_delete_confirm', { name }), { danger: true }))) return;
    const response = await api.deleteNetworkBond(name);
    if (!response.success) {
      notifyNetworkError(response.message || $t('network.bond_delete_failed'));
      return;
    }
    await Promise.all([loadBonds(), loadInterfaces(false)]);
    notifySuccess($t('network.bond_deleted'));
  }

  async function createVlan() {
    vlanError = '';
    const vid = Number(vlanId);
    if (!vlanParent) {
      vlanError = $t('network.vlan_need_parent');
      return;
    }
    if (!Number.isInteger(vid) || vid < 1 || vid > maxVid) {
      vlanError = translate('network.vlan_bad_id', { max: String(maxVid) });
      return;
    }
    savingVlan = true;
    const response = await api.createNetworkVlan({ parent: vlanParent, vid });
    savingVlan = false;
    if (!response.success) {
      vlanError = response.message || $t('network.vlan_create_failed');
      return;
    }
    await Promise.all([loadVlans(), loadInterfaces(false)]);
    notifySuccess(response.message || $t('network.vlan_created'));
  }

  async function deleteVlan(name: string) {
    if (!(await dlg.ask(translate('network.vlan_delete_confirm', { name }), { danger: true }))) return;
    const response = await api.deleteNetworkVlan(name);
    if (!response.success) {
      notifyNetworkError(response.message || $t('network.vlan_delete_failed'));
      return;
    }
    await Promise.all([loadVlans(), loadInterfaces(false)]);
    notifySuccess($t('network.vlan_deleted'));
  }

  async function loadRoutes() {
    const response = await api.getNetworkRoutes();
    if (response.success && response.data) {
      routes = response.data.routes || [];
      routesAvailable = response.data.available !== false;
    }
  }

  async function createRoute() {
    routeError = '';
    const metricText = routeMetric.trim();
    const metric = metricText ? Number(metricText) : undefined;
    if (metricText && (!Number.isInteger(metric) || metric! < 0 || metric! > 65535)) {
      routeError = $t('network.route_bad_metric');
      return;
    }
    savingRoute = true;
    const response = await api.addNetworkRoute({
      destination: routeDestination.trim(),
      via: routeVia.trim(),
      ...(metric === undefined ? {} : { metric }),
      interface: routeInterface,
    });
    savingRoute = false;
    if (!response.success) {
      routeError = response.message || $t('network.route_create_failed');
      return;
    }
    routeDestination = '';
    routeVia = '';
    routeMetric = '';
    await loadRoutes();
    notifySuccess($t('network.route_created'));
  }

  async function deleteRoute(route: StaticRoute) {
    if (!(await dlg.ask(translate('network.route_delete_confirm', { destination: route.destination }), { danger: true }))) return;
    const response = await api.deleteNetworkRoute({
      destination: route.destination,
      interface: route.interface,
    });
    if (!response.success) {
      notifyNetworkError(response.message || $t('network.route_delete_failed'));
      return;
    }
    await loadRoutes();
    notifySuccess($t('network.route_deleted'));
  }

  function notifyNetworkError(message: string) {
    notificationsStore.add({ title: $t('network.title'), message, type: 'error' });
  }

  async function loadAll() {
    await Promise.all([
      loadInterfaces(true),
      loadWifiStatus(),
      loadConnectionSummary(),
      loadBonds(),
      loadVlans(),
      loadRoutes(),
    ]);
  }

  async function refreshData() {
    await Promise.all([
      loadInterfaces(false),
      loadWifiStatus(),
      loadConnectionSummary(),
      loadBonds(),
      loadVlans(),
      loadRoutes(),
    ]);
  }

  async function loadConnectionSummary() {
    const summary = await api.getNetworkSummary();
    if (summary.success && summary.data) {
      connectionSummary = summary.data;
    }
    const pub = await api.getPublicIp();
    if (pub.success && pub.data) {
      publicIp = pub.data.ip || null;
    }
  }

  async function loadInterfaces(initial = false) {
    if (initial) loading = true;
    const response = await api.getNetworkInterfaces();
    if (response.success && response.data) {
      interfaces = response.data.interfaces || [];
    } else if (initial) {
      // Only surface this on the initial load — the 10s background poll
      // failing once (e.g. a transient hiccup) shouldn't spam a
      // notification every cycle.
      notifyNetworkError(response.message || translate('network.load_failed'));
    }
    if (initial) loading = false;
  }

  async function loadInterfaceConfig(name: string) {
    const response = await api.getNetworkInterfaceConfig(name);
    if (response.success && response.data) {
      interfaceConfig = response.data;
      configMethod = response.data.method;
      configIp = response.data.ip || '';
      configNetmask = response.data.netmask || '255.255.255.0';
      configGateway = response.data.gateway || '';
      configDns = (response.data.dns_servers || []).join(', ');

      // IPv6 config
      configIpv6Method = response.data.ipv6_method || 'auto';
      configIpv6 = response.data.ipv6 || '';
      configIpv6Prefix = response.data.ipv6_prefix?.toString() || '64';
      configIpv6Gateway = response.data.ipv6_gateway || '';
      configIpv6Dns = (response.data.ipv6_dns_servers || []).join(', ');
    }
  }

  async function loadWifiStatus() {
    const response = await api.getWifiStatus();
    if (response.success && response.data) {
      wifiStatus = response.data;
    }
  }

  async function scanWifi() {
    scanningWifi = true;
    const response = await api.scanWifiNetworks();
    if (response.success && response.data) {
      wifiNetworks = response.data.networks || [];
      wifiScanned = true;
    } else {
      notifyNetworkError(response.message || translate('network.scan_failed'));
    }
    scanningWifi = false;
  }

  async function switchToWifiTab() {
    activeTab = 'wifi';
    await loadWifiStatus();
    if (!wifiScanned) {
      await scanWifi();
    }
  }

  // ---------------------------------------------------------------------------
  // Interface Actions
  // ---------------------------------------------------------------------------

  async function toggleInterfaceState(iface: NetworkInterface) {
    const newState = iface.status !== 'up';
    const response = await api.setNetworkInterfaceState(iface.name, newState);
    if (response.success) {
      await loadInterfaces();
    } else {
      notifyNetworkError(response.message || translate('network.state_change_failed'));
    }
  }

  async function openConfigModal(iface: NetworkInterface) {
    selectedInterface = iface;
    // Load first, then show: opening the modal before the GET resolves let a
    // late answer overwrite the radio the user had already changed (the
    // modal flashed DHCP back over a just-selected Static).
    await loadInterfaceConfig(iface.name);
    showConfigModal = true;
  }

  async function saveInterfaceConfig() {
    if (!selectedInterface) return;

    saving = true;
    const dnsServers = configDns
      .split(',')
      .map(d => d.trim())
      .filter(d => d.length > 0);

    const ipv6DnsServers = configIpv6Dns
      .split(',')
      .map(d => d.trim())
      .filter(d => d.length > 0);

    const response = await api.updateNetworkInterfaceConfig(selectedInterface.name, {
      method: configMethod,
      ip: configIp || undefined,
      netmask: configNetmask || undefined,
      gateway: configGateway || undefined,
      dns_servers: dnsServers.length > 0 ? dnsServers : undefined,
      ipv6_method: configIpv6Method,
      ipv6: configIpv6 || undefined,
      ipv6_prefix: configIpv6Prefix ? parseInt(configIpv6Prefix) : undefined,
      ipv6_gateway: configIpv6Gateway || undefined,
      ipv6_dns_servers: ipv6DnsServers.length > 0 ? ipv6DnsServers : undefined,
    });

    if (response.success) {
      showConfigModal = false;
      await loadInterfaces();
      // Netplan/ifupdown persistence is best-effort: the live state is always
      // applied, but a reboot then re-applies the server's persisted config.
      // The backend answers "(temporary — persistence unavailable)" when it
      // could not write it (no netplan, missing sudo) — surface that, exactly
      // like notifyNetworkError surfaces the failure branch.
      if (response.message?.includes('temporary')) {
        notificationsStore.add({
          title: $t('network.title'),
          message: translate('network.temp_config'),
          type: 'warning',
        });
      }
    } else {
      // A netplan reconcile failure rolls the whole change back — including
      // the live state — so refresh what is actually on the wire before the
      // error reaches the operator, instead of showing the pre-rollback
      // card contents.
      await loadInterfaces();
      notifyNetworkError(response.message || translate('network.apply_failed'));
    }
    saving = false;
  }

  // ---------------------------------------------------------------------------
  // WiFi Actions
  // ---------------------------------------------------------------------------

  function openWifiConnectModal(network: WifiNetwork) {
    selectedWifi = network;
    wifiModalMode = network.connected ? 'disconnect' : 'connect';
    wifiPassword = '';
    wifiSecurity = 'wpa2';
    showWifiConnectModal = true;
  }

  async function handleWifiAction() {
    if (!selectedWifi) return;

    connectingWifi = true;

    if (wifiModalMode === 'disconnect') {
      const response = await api.disconnectWifi(selectedWifi.ssid);
      if (response.success) {
        showWifiConnectModal = false;
        await Promise.all([loadWifiStatus(), scanWifi()]);
      } else {
        notifyNetworkError(response.message || translate('network.wifi_disconnect_failed'));
      }
    } else {
      const response = await api.connectWifi(
        selectedWifi.ssid,
        wifiPassword,
        wifiSecurity
      );
      if (response.success) {
        showWifiConnectModal = false;
        await Promise.all([loadWifiStatus(), scanWifi()]);
      } else {
        notifyNetworkError(response.message || translate('network.wifi_connect_failed'));
      }
    }

    connectingWifi = false;
  }

  async function runDiagnostics() {
    runningDiagnostics = true;
    const response = await api.getNetworkDiagnostics();
    if (response.success && response.data) {
      diagnostics = {
        gateway_reachable: response.data.gateway_reachable,
        dns_working: response.data.dns_working,
        internet_reachable: response.data.internet_reachable,
        latency_ms: response.data.latency_ms ?? null,
        message: response.data.message,
      };
    } else {
      notifyNetworkError(response.message || translate('network.diagnostics_failed'));
    }
    runningDiagnostics = false;
  }

  // ---------------------------------------------------------------------------
  // Helpers
  // ---------------------------------------------------------------------------

  function formatBytes(bytes: number): string {
    if (bytes === 0) return '0 B';
    const k = 1024;
    const sizes = ['B', 'KB', 'MB', 'GB', 'TB'];
    const i = Math.floor(Math.log(bytes) / Math.log(k));
    return parseFloat((bytes / Math.pow(k, i)).toFixed(1)) + ' ' + sizes[i];
  }

  // Group ethernet, bridge, bond and VLAN interfaces under the "Ethernet" tab
  // (a bond and a VLAN are how a wired setup grows, so they belong there).
  const ethernetInterfaces = $derived(
    interfaces.filter(
      (i) =>
        i.iface_type === 'ethernet' ||
        i.iface_type === 'bridge' ||
        i.iface_type === 'bond' ||
        i.iface_type === 'vlan'
    )
  );

  // Only show the WiFi tab if at least one wireless interface is detected
  const hasWifi = $derived(interfaces.some((i) => i.iface_type === 'wifi'));

  // Stats
  const totalInterfaces = $derived(interfaces.length);
  const activeInterfaces = $derived(interfaces.filter((i) => i.status === 'up').length);
  const wifiCount = $derived(interfaces.filter((i) => i.iface_type === 'wifi').length);
  const totalSpeed = $derived(interfaces.reduce((sum, i) => sum + (i.speed_mbps || 0), 0));
// Shared shapes (appliance tokens only — the per-app CSS vocabulary is gone).
  const detailRow = 'flex items-center justify-between gap-2 text-sm';
  const detailLabel = 'text-[var(--hv-shell-text-2)]';
  const detailValue = 'font-semibold text-[var(--hv-shell-text-strong)]';
  const detailMono = cn(detailValue, 'font-[family-name:var(--hv-font-mono)]');
  const badge = 'inline-flex items-center px-2 py-0.5 rounded-full text-c-xs font-bold uppercase tracking-wide';
</script>

<PageLayout
  title={$t('network.title')}
  data-testid="network-page"
  tabs={[
    { id: 'ethernet', label: $t('network.ethernet'), icon: IconPlug },
    ...(hasWifi ? [{ id: 'wifi', label: $t('network.wifi'), icon: IconWifi }] : [])
  ]}
  bind:activeTab
  onTabChange={(id) => { if (id === 'wifi') switchToWifiTab(); }}
  footerMode="status"
>
  {#snippet icon()}
    <IconGlobe stroke={1.8} />
  {/snippet}
  <!-- Page-level actions: a WindowToolbar row under the header, never inside
       it (the header bar is icon + title + window controls only). -->
  {#snippet toolbarRight()}
    <Button variant="ghost" data-testid="network-btn" onclick={runDiagnostics} disabled={runningDiagnostics}>
      {#if runningDiagnostics}
        <IconLoader size={16} class="spin" />
      {:else}
        <IconActivity size={16} />
      {/if}
      <span>{runningDiagnostics ? $t('network.testing') : $t('network.diagnostics')}</span>
    </Button>
    {#if activeTab === 'wifi'}
      <Button variant="primary" onclick={scanWifi} disabled={scanningWifi}>
        {#if scanningWifi}
          <IconLoader size={16} class="spin" />
        {:else}
          <IconRadio size={16} />
        {/if}
        <span>{scanningWifi ? $t('network.scanning') : $t('network.scan')}</span>
      </Button>
    {/if}
  {/snippet}

  {#if loading}
    <div data-testid="network-loading" class="flex items-center justify-center gap-3 py-16 text-[var(--hv-shell-text)]">
      <Spinner />
      <span>{$t('network.loading')}</span>
    </div>
  {:else}
    <div class="grid grid-cols-2 @min-[880px]:grid-cols-4 gap-3 mb-5">
      <StatCard data-testid="network-stat-card" label={$t('network.stat_interfaces')} value={`${totalInterfaces}`} tone={'info'}>
        {#snippet iconSnippet()}<IconPlug size={20} />{/snippet}
      </StatCard>
      <StatCard data-testid="network-stat-card" label={$t('network.stat_active')} value={`${activeInterfaces}`} tone={'success'}>
        {#snippet iconSnippet()}<IconActivity size={20} />{/snippet}
      </StatCard>
      <StatCard data-testid="network-stat-card" label={$t('network.stat_wifi')} value={`${wifiCount}`} tone={'info'}>
        {#snippet iconSnippet()}<IconWifi size={20} />{/snippet}
      </StatCard>
      <StatCard data-testid="network-stat-card" label={$t('network.stat_speed')} value={`${totalSpeed || '—'}`} tone={'warning'}>
        {#snippet iconSnippet()}<IconGauge size={20} />{/snippet}
      </StatCard>
    </div>

    {#if connectionSummary}
      <div data-testid="network-summary-card" class="px-5 py-4 rounded-2xl bg-[var(--hv-shell-card)] border border-[var(--hv-shell-border)] shadow-[var(--hv-shell-shadow-card)] mb-4">
        <div class="grid grid-cols-1 @min-[560px]:grid-cols-2 gap-2">
          <div class={detailRow}><span class={detailLabel}>{$t('network.conn_connection')}</span><span class={detailValue}>{connectionSummary.connection_type || '—'}</span></div>
          <div class={detailRow}><span class={detailLabel}>{$t('network.conn_interface')}</span><span class={detailValue}>{connectionSummary.primary_interface || '—'}</span></div>
          <div class={detailRow}><span class={detailLabel}>{$t('network.conn_local_ip')}</span><span class={detailMono}>{connectionSummary.local_ip || '—'}</span></div>
          <div class={detailRow}><span class={detailLabel}>{$t('network.conn_gateway')}</span><span class={detailMono}>{connectionSummary.gateway || '—'}</span></div>
          <div class={detailRow}><span class={detailLabel}>{$t('network.conn_dns')}</span><span class={detailMono}>{connectionSummary.dns_servers.length > 0 ? connectionSummary.dns_servers.join(', ') : '—'}</span></div>
          <div class={detailRow}><span class={detailLabel}>{$t('network.conn_public_ip')}</span><span class={detailMono}>{publicIp || '—'}</span></div>
        </div>
        {#if diagnostics}
          <div data-testid="network-diagnostics" class={cn('mt-3 pt-3 border-t border-[var(--hv-shell-border-soft)]', !diagnostics.internet_reachable && 'text-[var(--hv-error)]')}>
            <div class="flex items-center gap-2 text-xs"><span class={cn('w-2 h-2 rounded-full', diagnostics.gateway_reachable ? 'bg-[var(--hv-success)]' : 'bg-[var(--hv-error)]')}></span> {diagnostics.gateway_reachable ? $t('network.gateway_reachable') : $t('network.gateway_unreachable')}</div>
            <div class="flex items-center gap-2 text-xs mt-1"><span class={cn('w-2 h-2 rounded-full', diagnostics.dns_working ? 'bg-[var(--hv-success)]' : 'bg-[var(--hv-error)]')}></span> {diagnostics.dns_working ? $t('network.dns_working') : $t('network.dns_unreachable')}</div>
            <div class="flex items-center gap-2 text-xs mt-1"><span class={cn('w-2 h-2 rounded-full', diagnostics.internet_reachable ? 'bg-[var(--hv-success)]' : 'bg-[var(--hv-error)]')}></span> {diagnostics.internet_reachable ? $t('network.internet_reachable') : $t('network.internet_unreachable')}{#if diagnostics.latency_ms != null} <span class="text-[var(--hv-shell-text-2)]">{translate('network.latency_ms', { ms: String(diagnostics.latency_ms) })}</span>{/if}</div>
            <div class="text-xs text-[var(--hv-shell-text-2)] mt-1">{diagnostics.message}</div>
          </div>
        {/if}
      </div>
    {/if}

    {#if activeTab === 'ethernet'}
      {#if ethernetInterfaces.length === 0}
        <div class="flex flex-col items-center gap-3 py-12 text-center text-[var(--hv-shell-text)]">
          <IconPlug size={40} class="text-[var(--hv-shell-border-strong)]" />
          <div class="text-lg font-semibold text-[var(--hv-shell-text-2)]">{$t('network.no_ethernet')}</div>
          <div class="text-sm text-[var(--hv-shell-text)]">{$t('network.no_wired')}</div>
        </div>
      {:else}
        <div class="grid gap-4 [grid-template-columns:repeat(auto-fill,minmax(300px,1fr))]">
          {#each ethernetInterfaces as iface}
            <div data-testid="network-card" class="rounded-2xl bg-[var(--hv-shell-card)] border border-[var(--hv-shell-border)] shadow-[var(--hv-shell-shadow-card)] overflow-hidden">
              <div class="flex items-center justify-between gap-3 px-4 pt-4">
                <div class="flex items-center gap-3 min-w-0">
                  <span class={cn('grid place-items-center w-11 h-11 rounded-xl flex-none', iface.status === 'up' ? 'bg-[var(--hv-success-bg)] text-[var(--hv-success)]' : 'bg-[var(--hv-shell-border-soft)] text-[var(--hv-shell-text-2)]')}><IconPlug size={20} /></span>
                  <div class="min-w-0">
                    <div class="text-sm font-bold text-[var(--hv-shell-text-strong)] truncate">{iface.name}</div>
                    <div class="text-xs text-[var(--hv-shell-text-2)] truncate">{iface.iface_type.charAt(0).toUpperCase() + iface.iface_type.slice(1)}{#if iface.driver} · {iface.driver}{/if}</div>
                  </div>
                </div>
                <label class="relative inline-block w-11 h-6 cursor-pointer">
                  <input type="checkbox" class="sr-only" checked={iface.status === 'up'} onchange={() => toggleInterfaceState(iface)} />
                  <span class={cn('block w-11 h-6 rounded-full transition-colors', iface.status === 'up' ? 'bg-[var(--hv-primary)]' : 'bg-[var(--hv-shell-border)]')}></span>
                  <span class={cn('absolute top-1 left-1 w-4 h-4 rounded-full bg-white transition-transform', iface.status === 'up' && 'translate-x-5')}></span>
                </label>
              </div>
              <div class="px-4 py-3">
                <div class="flex items-center gap-2">
                  <span class={cn(badge, iface.status === 'up' ? 'bg-[var(--hv-success-bg)] text-[var(--hv-success)]' : iface.status === 'down' ? 'bg-[var(--hv-error-bg)] text-[var(--hv-error)]' : 'bg-[var(--hv-warning-bg)] text-[var(--hv-warning)]')}>{iface.status === 'up' ? $t('network.connected') : iface.status === 'down' ? $t('network.disconnected') : $t('network.unknown')}</span>
                  {#if iface.speed_mbps}<span class="text-xs text-[var(--hv-shell-text-2)]">{iface.speed_mbps} Mbps</span>{/if}
                </div>
                <div class="flex flex-col gap-1.5 mt-2">
                  <div class={detailRow}><span class={detailLabel}>{$t('network.ipv4')}</span><span class={detailValue}>{iface.ipv4 || '—'}</span></div>
                  {#if iface.ipv4_netmask}<div class={detailRow}><span class={detailLabel}>{$t('network.netmask')}</span><span class={detailValue}>{iface.ipv4_netmask}</span></div>{/if}
                  {#if iface.mac}<div class={detailRow}><span class={detailLabel}>{$t('network.mac')}</span><span class={detailMono}>{iface.mac}</span></div>{/if}
                  {#if iface.mtu}<div class={detailRow}><span class={detailLabel}>{$t('network.mtu')}</span><span class={detailValue}>{iface.mtu}</span></div>{/if}
                </div>
                <div class="flex gap-3 mt-2 text-xs text-[var(--hv-shell-text-2)]">
                  <span class="inline-flex items-center gap-1"><IconDownload size={14} /> {translate('network.rx', { bytes: formatBytes(iface.rx_bytes) })}</span>
                  <span class="inline-flex items-center gap-1"><IconUpload size={14} /> {translate('network.tx', { bytes: formatBytes(iface.tx_bytes) })}</span>
                </div>
              </div>
              <div class="px-4 pb-4">
                <Button variant="ghost" size="sm" data-testid="network-btn" onclick={() => openConfigModal(iface)}>{$t('network.configure')}</Button>
              </div>
            </div>
          {/each}
        </div>
      {/if}


      <!-- Link aggregation and VLAN devices (B5 + B6). Creating one is a real
           change to the host's networking; configuring its address afterwards
           goes through the same card above, never a second editor. -->
      <section class="mt-6" aria-labelledby="network-virtual-heading">
        <h2 id="network-virtual-heading" class="text-base font-bold text-[var(--hv-shell-text-strong)] mb-1">{$t('network.virtual_devices')}</h2>
        <p class="text-sm text-[var(--hv-shell-text-2)] mb-3">{$t('network.virtual_devices_hint')}</p>

        <div class="grid gap-4 [grid-template-columns:repeat(auto-fill,minmax(320px,1fr))]">
          <!-- Bond -->
          <div data-testid="network-bond-card" class="rounded-2xl bg-[var(--hv-shell-card)] border border-[var(--hv-shell-border)] shadow-[var(--hv-shell-shadow-card)] p-4">
            <div class="flex items-center gap-3 mb-3">
              <span class="grid place-items-center w-11 h-11 rounded-xl flex-none bg-[var(--hv-success-bg)] text-[var(--hv-primary)]"><IconLink size={20} /></span>
              <div class="min-w-0">
                <div class="text-sm font-bold text-[var(--hv-shell-text-strong)]">{$t('network.bonds')}</div>
                <div class="text-xs text-[var(--hv-shell-text-2)]">{$t('network.bonds_hint')}</div>
              </div>
            </div>

            {#if bonds.length > 0}
              <ul class="flex flex-col gap-2 mb-4">
                {#each bonds as bond (bond.name)}
                  <li data-testid="network-bond-row" class="flex items-center justify-between gap-2 text-sm">
                    <span class="min-w-0">
                      <span class="font-semibold text-[var(--hv-shell-text-strong)]">{bond.name}</span>
                      <span class="text-[var(--hv-shell-text-2)]"> · {bond.mode} · {bond.members.join(', ') || '—'}{#if bond.ipv4} · {bond.ipv4}{/if}</span>
                    </span>
                    <button type="button" class="hv-ic is-danger" data-testid="network-bond-delete" aria-label={translate('network.bond_delete', { name: bond.name })} onclick={() => deleteBond(bond.name)}><IconTrash size={15} /></button>
                  </li>
                {/each}
              </ul>
            {:else}
              <p class="text-sm text-[var(--hv-shell-text-2)] mb-4">{$t('network.no_bond')}</p>
            {/if}

            {#if bondCandidates.length === 0}
              <p class="text-xs text-[var(--hv-shell-text-2)]">{$t('network.bond_no_members')}</p>
            {:else}
              <div class="flex flex-col gap-3">
                <div>
                  <label class="block text-xs font-semibold text-[var(--hv-shell-text-2)] mb-1" for="bond-name">{$t('network.bond_name')}</label>
                  <input id="bond-name" data-testid="network-bond-name" class="w-full h-9 px-3 rounded-lg border border-[var(--hv-shell-border)] bg-[var(--hv-shell-glass)] text-c-base" bind:value={bondName} placeholder={$t('network.bond_name_auto')} maxlength="15" />
                </div>
                <fieldset>
                  <legend class="text-xs font-semibold text-[var(--hv-shell-text-2)] mb-1">{$t('network.bond_mode')}</legend>
                  <label class="flex items-start gap-2 text-sm mb-1">
                    <input type="radio" name="bond-mode" value="802.3ad" checked={bondMode === '802.3ad'} onchange={() => (bondMode = '802.3ad')} />
                    <span><span class="font-semibold text-[var(--hv-shell-text-strong)]">802.3ad</span> <span class="text-[var(--hv-shell-text-2)]">· {$t('network.bond_mode_lacp')}</span></span>
                  </label>
                  <label class="flex items-start gap-2 text-sm">
                    <input type="radio" name="bond-mode" value="active-backup" checked={bondMode === 'active-backup'} onchange={() => (bondMode = 'active-backup')} />
                    <span><span class="font-semibold text-[var(--hv-shell-text-strong)]">active-backup</span> <span class="text-[var(--hv-shell-text-2)]">· {$t('network.bond_mode_backup')}</span></span>
                  </label>
                </fieldset>
                <fieldset>
                  <legend class="text-xs font-semibold text-[var(--hv-shell-text-2)] mb-1">{$t('network.bond_members')}</legend>
                  {#each bondCandidates as candidate (candidate)}
                    <label class="flex items-center gap-2 text-sm">
                      <input type="checkbox" checked={bondMembers.includes(candidate)} onchange={() => toggleBondMember(candidate)} />
                      <span class="font-[family-name:var(--hv-font-mono)]">{candidate}</span>
                    </label>
                  {/each}
                </fieldset>
                {#if bondError}
                  <p class="text-xs text-[var(--hv-error)]" role="alert" data-testid="network-bond-error">{bondError}</p>
                {/if}
                <Button variant="primary" size="sm" onclick={createBond} disabled={savingBond} data-testid="network-bond-create">
                  <IconPlus size={15} />
                  <span>{savingBond ? $t('network.bond_creating') : $t('network.bond_create')}</span>
                </Button>
              </div>
            {/if}
          </div>

          <!-- VLAN -->
          <div data-testid="network-vlan-card" class="rounded-2xl bg-[var(--hv-shell-card)] border border-[var(--hv-shell-border)] shadow-[var(--hv-shell-shadow-card)] p-4">
            <div class="flex items-center gap-3 mb-3">
              <span class="grid place-items-center w-11 h-11 rounded-xl flex-none bg-[var(--hv-success-bg)] text-[var(--hv-primary)]"><IconSitemap size={20} /></span>
              <div class="min-w-0">
                <div class="text-sm font-bold text-[var(--hv-shell-text-strong)]">{$t('network.vlans')}</div>
                <div class="text-xs text-[var(--hv-shell-text-2)]">{$t('network.vlans_hint')}</div>
              </div>
            </div>

            {#if vlans.length > 0}
              <ul class="flex flex-col gap-2 mb-4">
                {#each vlans as vlan (vlan.name)}
                  <li data-testid="network-vlan-row" class="flex items-center justify-between gap-2 text-sm">
                    <span class="min-w-0">
                      <span class="font-semibold text-[var(--hv-shell-text-strong)]">{vlan.name}</span>
                      <span class="text-[var(--hv-shell-text-2)]"> · {$t('network.vlan_on')} {vlan.parent} · {vlan.vid}{#if vlan.ipv4} · {vlan.ipv4}{/if}</span>
                    </span>
                    <button type="button" class="hv-ic is-danger" data-testid="network-vlan-delete" aria-label={translate('network.vlan_delete', { name: vlan.name })} onclick={() => deleteVlan(vlan.name)}><IconTrash size={15} /></button>
                  </li>
                {/each}
              </ul>
            {:else}
              <p class="text-sm text-[var(--hv-shell-text-2)] mb-4">{$t('network.no_vlan')}</p>
            {/if}

            {#if vlanParents.length === 0}
              <p class="text-xs text-[var(--hv-shell-text-2)]">{$t('network.vlan_no_parent')}</p>
            {:else}
              <div class="flex flex-col gap-3">
                <div>
                  <label class="block text-xs font-semibold text-[var(--hv-shell-text-2)] mb-1" for="vlan-parent">{$t('network.vlan_parent')}</label>
                  <select id="vlan-parent" data-testid="network-vlan-parent" class="w-full h-9 px-3 rounded-lg border border-[var(--hv-shell-border)] bg-[var(--hv-shell-glass)] text-c-base" bind:value={vlanParent}>
                    {#each vlanParents as parent (parent)}<option value={parent}>{parent}</option>{/each}
                  </select>
                </div>
                <div>
                  <label class="block text-xs font-semibold text-[var(--hv-shell-text-2)] mb-1" for="vlan-id">{$t('network.vlan_id')}</label>
                  <input id="vlan-id" data-testid="network-vlan-id" type="number" min="1" max={maxVid} class="w-full h-9 px-3 rounded-lg border border-[var(--hv-shell-border)] bg-[var(--hv-shell-glass)] text-c-base" bind:value={vlanId} />
                  <p class="text-xs text-[var(--hv-shell-text-2)] mt-1">{translate('network.vlan_id_hint', { max: String(maxVid) })}</p>
                </div>
                {#if vlanError}
                  <p class="text-xs text-[var(--hv-error)]" role="alert" data-testid="network-vlan-error">{vlanError}</p>
                {/if}
                <Button variant="primary" size="sm" onclick={createVlan} disabled={savingVlan} data-testid="network-vlan-create">
                  <IconPlus size={15} />
                  <span>{savingVlan ? $t('network.vlan_creating') : $t('network.vlan_create')}</span>
                </Button>
              </div>
            {/if}
          </div>

          <!-- Static routes -->
          <div data-testid="network-routes-card" class="rounded-2xl bg-[var(--hv-shell-card)] border border-[var(--hv-shell-border)] shadow-[var(--hv-shell-shadow-card)] p-4">
            <div class="flex items-center gap-3 mb-3">
              <span class="grid place-items-center w-11 h-11 rounded-xl flex-none bg-[var(--hv-success-bg)] text-[var(--hv-primary)]"><IconRoute size={20} /></span>
              <div class="min-w-0">
                <div class="text-sm font-bold text-[var(--hv-shell-text-strong)]">{$t('network.routes')}</div>
                <div class="text-xs text-[var(--hv-shell-text-2)]">{$t('network.routes_hint')}</div>
              </div>
            </div>

            {#if !routesAvailable}
              <p class="text-xs text-[var(--hv-error)]" role="alert" data-testid="network-routes-unavailable">{$t('network.routes_unavailable')}</p>
            {:else}
              {#if routes.length > 0}
                <ul class="flex flex-col gap-2 mb-4">
                  {#each routes as route (route.destination)}
                    <li data-testid="network-route-row" class="flex items-center justify-between gap-2 text-sm">
                      <span class="min-w-0">
                        <span class="font-semibold font-[family-name:var(--hv-font-mono)] text-[var(--hv-shell-text-strong)]">{route.destination}</span>
                        <span class="text-[var(--hv-shell-text-2)]"> · {$t('network.route_via')} {route.via} · {route.interface}{#if route.metric != null} · {$t('network.route_metric')} {route.metric}{/if}{#if !route.active} · {$t('network.route_inactive')}{/if}</span>
                      </span>
                      <button type="button" class="hv-ic is-danger" data-testid="network-route-delete" aria-label={translate('network.route_delete', { destination: route.destination })} onclick={() => deleteRoute(route)}><IconTrash size={15} /></button>
                    </li>
                  {/each}
                </ul>
              {:else}
                <p class="text-sm text-[var(--hv-shell-text-2)] mb-4">{$t('network.no_route')}</p>
              {/if}

              <div class="flex flex-col gap-3">
                <div>
                  <label class="block text-xs font-semibold text-[var(--hv-shell-text-2)] mb-1" for="route-destination">{$t('network.route_destination')}</label>
                  <input id="route-destination" data-testid="network-route-destination" class="w-full h-9 px-3 rounded-lg border border-[var(--hv-shell-border)] bg-[var(--hv-shell-glass)] text-c-base" bind:value={routeDestination} placeholder="192.168.20.0/24" />
                </div>
                <div>
                  <label class="block text-xs font-semibold text-[var(--hv-shell-text-2)] mb-1" for="route-via">{$t('network.route_via')}</label>
                  <input id="route-via" data-testid="network-route-via" class="w-full h-9 px-3 rounded-lg border border-[var(--hv-shell-border)] bg-[var(--hv-shell-glass)] text-c-base" bind:value={routeVia} placeholder="192.168.1.1" />
                </div>
                <div>
                  <label class="block text-xs font-semibold text-[var(--hv-shell-text-2)] mb-1" for="route-interface">{$t('network.route_interface')}</label>
                  <select id="route-interface" data-testid="network-route-interface" class="w-full h-9 px-3 rounded-lg border border-[var(--hv-shell-border)] bg-[var(--hv-shell-glass)] text-c-base" bind:value={routeInterface}>
                    {#each interfaces as iface (iface.name)}<option value={iface.name}>{iface.name}</option>{/each}
                  </select>
                </div>
                <div>
                  <label class="block text-xs font-semibold text-[var(--hv-shell-text-2)] mb-1" for="route-metric">{$t('network.route_metric')}</label>
                  <input id="route-metric" data-testid="network-route-metric" type="number" min="0" max="65535" class="w-full h-9 px-3 rounded-lg border border-[var(--hv-shell-border)] bg-[var(--hv-shell-glass)] text-c-base" bind:value={routeMetric} placeholder={$t('network.route_metric_optional')} />
                </div>
                {#if routeError}
                  <p class="text-xs text-[var(--hv-error)]" role="alert" data-testid="network-route-error">{routeError}</p>
                {/if}
                <Button variant="primary" size="sm" onclick={createRoute} disabled={savingRoute || !routeDestination.trim() || !routeVia.trim() || !routeInterface} data-testid="network-route-create">
                  <IconPlus size={15} />
                  <span>{savingRoute ? $t('network.route_creating') : $t('network.route_create')}</span>
                </Button>
              </div>
            {/if}
          </div>
        </div>
      </section>

    {:else if activeTab === 'wifi'}
      {#if wifiStatus?.connected}
        <div data-testid="network-card" class="rounded-2xl bg-[var(--hv-shell-card)] border border-[var(--hv-shell-border)] shadow-[var(--hv-shell-shadow-card)] overflow-hidden mb-4">
          <div class="flex items-center justify-between gap-3 px-4 pt-4">
            <div class="flex items-center gap-3 min-w-0">
              <span class="grid place-items-center w-11 h-11 rounded-xl flex-none bg-[var(--hv-success-bg)] text-[var(--hv-success)]"><IconWifi size={20} /></span>
              <div class="min-w-0">
                <div class="text-sm font-bold text-[var(--hv-shell-text-strong)]">{$t('network.connected')}</div>
                <div class="text-xs text-[var(--hv-shell-text-2)]">{$t('network.current_wifi')}</div>
              </div>
            </div>
            <span class={cn(badge, 'bg-[var(--hv-success-bg)] text-[var(--hv-success)]')}>{$t('network.connected')}</span>
          </div>
          <div class="px-4 py-3 flex flex-col gap-1.5">
            <div class="flex items-center gap-1.5 text-sm font-semibold text-[var(--hv-shell-text-strong)]"><IconWifi size={18} /> <span>{wifiStatus.connected.ssid}</span></div>
            {#if wifiStatus.connected.ip}<div class={detailRow}><span class={detailLabel}>{$t('network.ip_address')}</span><span class={detailValue}>{wifiStatus.connected.ip}</span></div>{/if}
            {#if wifiStatus.connected.bssid}<div class={detailRow}><span class={detailLabel}>{$t('network.bssid')}</span><span class={detailMono}>{wifiStatus.connected.bssid}</span></div>{/if}
            {#if wifiStatus.connected.signal_dbm}<div class={detailRow}><span class={detailLabel}>{$t('network.signal')}</span><span class={detailValue}>{wifiStatus.connected.signal_dbm} dBm</span></div>{/if}
          </div>
        </div>
      {/if}

      <div data-testid="network-card" class="rounded-2xl bg-[var(--hv-shell-card)] border border-[var(--hv-shell-border)] shadow-[var(--hv-shell-shadow-card)] overflow-hidden">
        <div class="flex items-center gap-3 px-4 pt-4">
          <span class="grid place-items-center w-11 h-11 rounded-xl flex-none bg-[var(--hv-primary-light)] text-[var(--hv-primary)]"><IconRadio size={20} /></span>
          <div class="flex-1 min-w-0">
            <div class="text-sm font-bold text-[var(--hv-shell-text-strong)]">{$t('network.available_networks')}</div>
            <div class="text-xs text-[var(--hv-shell-text-2)]">{$t('network.wifi_in_range')}</div>
          </div>
          {#if wifiNetworks.length > 0}<span class="text-xs text-[var(--hv-shell-text-2)]">{translate('network.networks_count', { count: String(wifiNetworks.length) })}</span>{/if}
        </div>
        <div class="px-4 py-3">
          {#if wifiNetworks.length === 0}
            <EmptyState compact data-testid="network-empty-mini" title={$t('network.no_networks2')}>
              {#snippet iconSnippet()}<IconWifiOff size={20} stroke={1.8} />{/snippet}
            </EmptyState>
          {:else}
            <div data-testid="network-wifi-list" class="flex flex-col gap-1.5">
              {#each wifiNetworks as network}
                <div
                  data-testid="network-wifi-item"
                  class:connected={network.connected}
                  onclick={() => openWifiConnectModal(network)}
                  onkeydown={(e) => activateOnEnterOrSpace(e, () => openWifiConnectModal(network))}
                  role="button"
                  tabindex="0"
                  class={cn('flex items-center gap-3 px-3 py-2.5 rounded-xl border border-[var(--hv-shell-border)] bg-[var(--hv-shell-border-soft)] cursor-pointer transition-colors hover:bg-[var(--hv-subtle-hover)]', network.connected && 'border-[var(--hv-primary)]/50 bg-[var(--hv-primary-light)]')}
                >
                  <span class={cn('grid place-items-center w-9 h-9 rounded-lg flex-none', network.connected ? 'bg-[var(--hv-primary)] text-white' : 'bg-[var(--hv-shell-fill)] text-[var(--hv-shell-text-2)]')}><IconWifi size={18} /></span>
                  <span class="flex-1 min-w-0">
                    <span class="block text-sm font-semibold text-[var(--hv-shell-text-strong)] truncate">
                      {network.ssid}
                      {#if network.connected}<span class={cn(badge, 'ml-1 bg-[var(--hv-success-bg)] text-[var(--hv-success)]')}>{$t('network.connected')}</span>{/if}
                    </span>
                    <span class="block text-xs text-[var(--hv-shell-text-2)]">{network.signal_percent}% · {network.channel > 14 ? '5 GHz' : '2.4 GHz'}</span>
                  </span>
                  <span class="flex-none">
                    {#if network.security.includes('WPA3')}
                      <span class={cn(badge, 'bg-[var(--hv-success-bg)] text-[var(--hv-success)]')}>{$t('network.security_wpa3')}</span>
                    {:else if network.security.includes('WPA2')}
                      <span class={cn(badge, 'bg-[var(--hv-primary-light)] text-[var(--hv-primary)]')}>{$t('network.security_wpa2')}</span>
                    {:else if network.security.includes('WPA')}
                      <span class={cn(badge, 'bg-[var(--hv-warning-bg)] text-[var(--hv-warning)]')}>{$t('network.security_wpa')}</span>
                    {:else if network.security.includes('WEP')}
                      <span class={cn(badge, 'bg-[var(--hv-warning-bg)] text-[var(--hv-warning)]')}>{$t('network.security_wep')}</span>
                    {:else}
                      <span class={cn(badge, 'bg-[var(--hv-error-bg)] text-[var(--hv-error)]')}>{$t('network.security_open')}</span>
                    {/if}
                  </span>
                </div>
              {/each}
            </div>
          {/if}
        </div>
      </div>
    {/if}
  {/if}

  {#snippet footer()}
    <span data-testid="network-footer" class="flex items-center gap-2 min-w-0 flex-wrap">
      <span>{interfaces.length} {$t('network.stat_interfaces')}</span>
      <span class="text-[var(--hv-shell-text-3)]">&middot;</span>
      <span>{activeInterfaces} {$t('network.stat_active')}</span>
      {#if selectedInterface?.ipv4}
        <span class="text-[var(--hv-shell-text-3)]">&middot;</span>
        <span class="font-[family-name:var(--hv-font-mono)]">{selectedInterface.ipv4}</span>
      {/if}
    </span>
  {/snippet}
</PageLayout>

<!-- Deletion confirmation for the virtual devices: a bond or a VLAN takes
     links down, so the consequence is named before anything is removed. -->
<ConfirmDialog {dlg} onconfirm={() => dlg.settle(true)} oncancel={() => dlg.settle(false)} />

<!-- Interface Configuration Modal -->
<Modal open={showConfigModal && !!selectedInterface} title={translate('network.modal_configure', { name: selectedInterface?.name || '' })} size="md" onclose={() => showConfigModal = false}>
  <div data-testid="network-form-field" class="flex flex-col gap-1.5 mb-3">
    <span class="text-xs font-bold uppercase tracking-wide text-[var(--hv-shell-text-3)]">{$t('network.ipv4_config')}</span>
    <div class="flex gap-4">
      <label class="flex items-center gap-2 text-c-label text-[var(--hv-shell-text-strong)] cursor-pointer"><input type="radio" name="ip-method" value="dhcp" class="accent-[var(--hv-primary)]" bind:group={configMethod} /> <span>{$t('network.dhcp_auto')}</span></label>
      <label class="flex items-center gap-2 text-c-label text-[var(--hv-shell-text-strong)] cursor-pointer"><input type="radio" name="ip-method" value="static" class="accent-[var(--hv-primary)]" bind:group={configMethod} /> <span>{$t('network.static_ip')}</span></label>
    </div>
  </div>

  {#if configMethod === 'static'}
    <div class="grid grid-cols-2 gap-3 mb-3">
      <div data-testid="network-form-field" class="flex flex-col gap-1.5"><label for="cfg-ip" class="text-c-label font-semibold text-[var(--hv-shell-text-strong)]">{$t('network.ip_address')}</label><input id="cfg-ip" class="w-full h-11 px-3 border border-[var(--hv-shell-border-strong)] rounded-lg bg-[var(--hv-shell-border-soft)] text-sm outline-none focus:border-[var(--hv-primary)]" type="text" bind:value={configIp} placeholder="192.168.1.100" /></div>
      <div data-testid="network-form-field" class="flex flex-col gap-1.5"><label for="cfg-netmask" class="text-c-label font-semibold text-[var(--hv-shell-text-strong)]">{$t('network.subnet_mask')}</label><input id="cfg-netmask" class="w-full h-11 px-3 border border-[var(--hv-shell-border-strong)] rounded-lg bg-[var(--hv-shell-border-soft)] text-sm outline-none focus:border-[var(--hv-primary)]" type="text" bind:value={configNetmask} placeholder="255.255.255.0" /></div>
      <div data-testid="network-form-field" class="flex flex-col gap-1.5"><label for="cfg-gw" class="text-c-label font-semibold text-[var(--hv-shell-text-strong)]">{$t('network.gateway_label')}</label><input id="cfg-gw" class="w-full h-11 px-3 border border-[var(--hv-shell-border-strong)] rounded-lg bg-[var(--hv-shell-border-soft)] text-sm outline-none focus:border-[var(--hv-primary)]" type="text" bind:value={configGateway} placeholder="192.168.1.1" /></div>
      <div data-testid="network-form-field" class="flex flex-col gap-1.5"><label for="cfg-dns" class="text-c-label font-semibold text-[var(--hv-shell-text-strong)]">{$t('network.dns_server')}</label><input id="cfg-dns" class="w-full h-11 px-3 border border-[var(--hv-shell-border-strong)] rounded-lg bg-[var(--hv-shell-border-soft)] text-sm outline-none focus:border-[var(--hv-primary)]" type="text" bind:value={configDns} placeholder="8.8.8.8, 1.1.1.1" /><span class="text-c-xs text-[var(--hv-shell-text-2)]">{$t('network.comma_separated')}</span></div>
    </div>
  {/if}

  <div data-testid="network-form-field" class="flex flex-col gap-1.5 mb-3">
    <span class="text-xs font-bold uppercase tracking-wide text-[var(--hv-shell-text-3)]">{$t('network.ipv6_config')}</span>
    <div class="flex flex-wrap gap-4">
      <label class="flex items-center gap-2 text-c-label text-[var(--hv-shell-text-strong)] cursor-pointer"><input type="radio" name="ipv6-method" value="auto" class="accent-[var(--hv-primary)]" bind:group={configIpv6Method} /> <span>{$t('network.auto_slaac')}</span></label>
      <label class="flex items-center gap-2 text-c-label text-[var(--hv-shell-text-strong)] cursor-pointer"><input type="radio" name="ipv6-method" value="dhcp" class="accent-[var(--hv-primary)]" bind:group={configIpv6Method} /> <span>{$t('network.dhcpv6')}</span></label>
      <label class="flex items-center gap-2 text-c-label text-[var(--hv-shell-text-strong)] cursor-pointer"><input type="radio" name="ipv6-method" value="static" class="accent-[var(--hv-primary)]" bind:group={configIpv6Method} /> <span>{$t('network.static')}</span></label>
      <label class="flex items-center gap-2 text-c-label text-[var(--hv-shell-text-strong)] cursor-pointer"><input type="radio" name="ipv6-method" value="disabled" class="accent-[var(--hv-primary)]" bind:group={configIpv6Method} /> <span>{$t('network.disabled')}</span></label>
    </div>
  </div>

  {#if configIpv6Method === 'static'}
    <div class="grid grid-cols-2 gap-3 mb-3">
      <div data-testid="network-form-field" class="flex flex-col gap-1.5"><label for="cfg-ipv6" class="text-c-label font-semibold text-[var(--hv-shell-text-strong)]">{$t('network.ipv6_address')}</label><input id="cfg-ipv6" class="w-full h-11 px-3 border border-[var(--hv-shell-border-strong)] rounded-lg bg-[var(--hv-shell-border-soft)] text-sm outline-none focus:border-[var(--hv-primary)]" type="text" bind:value={configIpv6} placeholder="2001:db8::1" /></div>
      <div data-testid="network-form-field" class="flex flex-col gap-1.5"><label for="cfg-ipv6-prefix" class="text-c-label font-semibold text-[var(--hv-shell-text-strong)]">{$t('network.prefix_length')}</label><input id="cfg-ipv6-prefix" class="w-full h-11 px-3 border border-[var(--hv-shell-border-strong)] rounded-lg bg-[var(--hv-shell-border-soft)] text-sm outline-none focus:border-[var(--hv-primary)]" type="number" bind:value={configIpv6Prefix} placeholder="64" /><span class="text-c-xs text-[var(--hv-shell-text-2)]">{$t('network.typical_64')}</span></div>
      <div data-testid="network-form-field" class="flex flex-col gap-1.5"><label for="cfg-ipv6-gw" class="text-c-label font-semibold text-[var(--hv-shell-text-strong)]">{$t('network.ipv6_gateway')}</label><input id="cfg-ipv6-gw" class="w-full h-11 px-3 border border-[var(--hv-shell-border-strong)] rounded-lg bg-[var(--hv-shell-border-soft)] text-sm outline-none focus:border-[var(--hv-primary)]" type="text" bind:value={configIpv6Gateway} placeholder="fe80::1" /></div>
      <div data-testid="network-form-field" class="flex flex-col gap-1.5"><label for="cfg-ipv6-dns" class="text-c-label font-semibold text-[var(--hv-shell-text-strong)]">{$t('network.ipv6_dns')}</label><input id="cfg-ipv6-dns" class="w-full h-11 px-3 border border-[var(--hv-shell-border-strong)] rounded-lg bg-[var(--hv-shell-border-soft)] text-sm outline-none focus:border-[var(--hv-primary)]" type="text" bind:value={configIpv6Dns} placeholder="2001:4860:4860::8888" /><span class="text-c-xs text-[var(--hv-shell-text-2)]">{$t('network.comma_separated')}</span></div>
    </div>
  {/if}

  {#snippet footer()}
    <div class="flex gap-2.5 justify-end">
      <Button variant="ghost" onclick={() => showConfigModal = false} disabled={saving}>{$t('common.cancel')}</Button>
      <Button variant="primary" onclick={saveInterfaceConfig} disabled={saving}>{saving ? $t('network.applying') : $t('network.apply')}</Button>
    </div>
  {/snippet}
</Modal>

<!-- WiFi Connect Modal -->
<Modal open={showWifiConnectModal && !!selectedWifi} title={wifiModalMode === 'disconnect' ? translate('network.modal_disconnect', { ssid: selectedWifi?.ssid || '' }) : translate('network.modal_connect', { ssid: selectedWifi?.ssid || '' })} size="sm" onclose={() => showWifiConnectModal = false}>
  {#if wifiModalMode === 'connect'}
    <div class="flex flex-col gap-1.5 mb-3">
      <label for="wifi-security" class="text-c-label font-semibold text-[var(--hv-shell-text-strong)]">{$t('network.security')}</label>
      <select id="wifi-security" class="w-full h-11 px-3 border border-[var(--hv-shell-border-strong)] rounded-lg bg-[var(--hv-shell-border-soft)] text-sm outline-none focus:border-[var(--hv-primary)]" bind:value={wifiSecurity}>
        <option value="wpa3">{$t('network.wpa3_personal')}</option>
        <option value="wpa2">{$t('network.wpa2_personal')}</option>
        <option value="wpa">{$t('network.wpa_personal')}</option>
        <option value="wep">{$t('network.wep')}</option>
        <option value="none">{$t('network.open')}</option>
      </select>
    </div>
    {#if wifiSecurity !== 'none'}
      <div class="flex flex-col gap-1.5 mb-3">
        <label for="wifi-password" class="text-c-label font-semibold text-[var(--hv-shell-text-strong)]">{$t('network.password')}</label>
        <input id="wifi-password" type="password" class="w-full h-11 px-3 border border-[var(--hv-shell-border-strong)] rounded-lg bg-[var(--hv-shell-border-soft)] text-sm outline-none focus:border-[var(--hv-primary)]" bind:value={wifiPassword} placeholder={$t('network.password_hint')} />
      </div>
    {/if}
  {:else}
    <p style="margin: 0; font-size: var(--hv-text-md); color: var(--hv-text-primary);">{translate('network.disconnect_confirm', { ssid: selectedWifi?.ssid || '' })}</p>
  {/if}
  {#snippet footer()}
    <div class="flex gap-2.5 justify-end">
      <Button variant="ghost" onclick={() => showWifiConnectModal = false} disabled={connectingWifi}>{$t('common.cancel')}</Button>
      {#if wifiModalMode === 'disconnect'}
        <Button variant="danger" onclick={handleWifiAction} disabled={connectingWifi}>{connectingWifi ? $t('network.disconnecting') : $t('network.disconnect')}</Button>
      {:else}
        <Button variant="primary" onclick={handleWifiAction} disabled={connectingWifi || (wifiSecurity !== 'none' && !wifiPassword)}>{connectingWifi ? $t('network.connecting') : $t('network.connect')}</Button>
      {/if}
    </div>
  {/snippet}
</Modal>
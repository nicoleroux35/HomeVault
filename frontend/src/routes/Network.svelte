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
  import { PageLayout, StatCard, EmptyState } from '$lib/components/app';
  import { cn } from '$lib/utils.js';
  import { activateOnEnterOrSpace } from '$lib/utils/activateOnEnterOrSpace';
  import WindowHeader from '$lib/components/desktop/WindowHeader.svelte';
  import { IconPlug, IconDownload, IconLoader, IconUpload, IconWifi, IconWifiOff, IconX, IconGlobe, IconActivity, IconRadio, IconGauge } from '@tabler/icons-svelte';
  import { notificationsStore } from '$lib/stores/notifications';

  // ---------------------------------------------------------------------------
  // Types (mirroring Rust backend)
  // ---------------------------------------------------------------------------

  type InterfaceType = 'ethernet' | 'wifi' | 'bridge' | 'vlan' | 'tunnel' | 'loopback' | 'unknown';
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

  let interfaces: NetworkInterface[] = [];
  let selectedInterface: NetworkInterface | null = null;
  let interfaceConfig: InterfaceConfig | null = null;

  let wifiNetworks: WifiNetwork[] = [];
  let wifiStatus: WifiStatus | null = null;
  let scanningWifi = false;
  let connectingWifi = false;

  let loading = true;
  let saving = false;
  let activeTab: 'ethernet' | 'wifi' = 'ethernet';

  // Connection summary / diagnostics
  let connectionSummary: {
    local_ip: string | null;
    gateway: string | null;
    dns_servers: string[];
    public_ip: string | null;
    primary_interface: string | null;
    connection_type: string;
  } | null = null;
  let publicIp: string | null = null;
  let diagnostics: {
    gateway_reachable: boolean;
    dns_working: boolean;
    internet_reachable: boolean;
    latency_ms: number | null;
    message: string;
  } | null = null;
  let runningDiagnostics = false;

  // Auto-refresh interval
  let refreshInterval: ReturnType<typeof setInterval>;

  // Modal state
  let showConfigModal = false;
  let showWifiConnectModal = false;
  let wifiModalMode: 'connect' | 'disconnect' = 'connect';
  let selectedWifi: WifiNetwork | null = null;
  let wifiPassword = '';
  let wifiSecurity = 'wpa2';
  let wifiScanned = false;

  // Config form
  let configMethod: 'dhcp' | 'static' = 'dhcp';
  let configIp = '';
  let configNetmask = '255.255.255.0';
  let configGateway = '';
  let configDns = '';

  // IPv6 config form
  let configIpv6Method: 'auto' | 'dhcp' | 'static' | 'disabled' = 'auto';
  let configIpv6 = '';
  let configIpv6Prefix = '64';
  let configIpv6Gateway = '';
  let configIpv6Dns = '';

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
  function notifyNetworkError(message: string) {
    notificationsStore.add({ title: $t('network.title'), message, type: 'error' });
  }

  async function loadAll() {
    await Promise.all([
      loadInterfaces(true),
      loadWifiStatus(),
      loadConnectionSummary(),
    ]);
  }

  async function refreshData() {
    await Promise.all([
      loadInterfaces(false),
      loadWifiStatus(),
      loadConnectionSummary(),
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

  function openConfigModal(iface: NetworkInterface) {
    selectedInterface = iface;
    loadInterfaceConfig(iface.name);
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

  // Group ethernet, bridge, and VLAN interfaces under the "Ethernet" tab
  $: ethernetInterfaces = interfaces.filter(
    i => i.iface_type === 'ethernet' || i.iface_type === 'bridge' || i.iface_type === 'vlan'
  );

  // Only show the WiFi tab if at least one wireless interface is detected
  $: hasWifi = interfaces.some(i => i.iface_type === 'wifi');

  // Stats
  $: totalInterfaces = interfaces.length;
  $: activeInterfaces = interfaces.filter(i => i.status === 'up').length;
  $: wifiCount = interfaces.filter(i => i.iface_type === 'wifi').length;
  $: totalSpeed = interfaces.reduce((sum, i) => sum + (i.speed_mbps || 0), 0);
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
    <Button variant="ghost" onclick={runDiagnostics} disabled={runningDiagnostics}>
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
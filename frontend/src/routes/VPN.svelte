<!-- ========================================================================
     File          : VPN.svelte
     Author        : Nicolas Le Roux
     Created       : 28/09/2026
     Last modified : 28/09/2026 by Nicolas Le Roux
     Version       : 1.0.0

     Description:
     The VPN app: WireGuard server status and peer management (create with
     config/QR download, revoke), OpenVPN profile creation and control.

     Dependencies:
       - svelte (>= 5), theme + app/ composites, $lib/services/api

     Usage:
     Opened as a desktop window app (admin only).
     ======================================================================== -->
<script lang="ts">
  import { onMount } from 'svelte';
  import QRCode from 'qrcode';
  import { api } from '$lib/services/api';
  import { t, translate } from '$lib/stores/i18n';
  import { notificationsStore } from '$lib/stores/notifications';
  import { Button, Spinner, Badge, Modal } from '$lib/components/theme';
  import { PageLayout, ConfirmDialog, StatCard, EmptyState } from '$lib/components/app';
  import { createConfirmState } from '$lib/utils/confirmDialog.svelte';
  import { cn } from '$lib/utils.js';
  import WindowHeader from '$lib/components/desktop/WindowHeader.svelte';
  import { IconShield, IconPlus, IconPower, IconDeviceMobile, IconTrash, IconUpload, IconDownload, IconLoader, IconCheck, IconCopy } from '$lib/icons/lucide';

  // ---------------------------------------------------------------------------
  // Types
  // ---------------------------------------------------------------------------

  type VpnType = 'wireguard' | 'openvpn';
  type VpnStatus = 'connected' | 'disconnected' | 'connecting' | 'error';

  interface VpnConnection {
    id: string;
    name: string;
    vpn_type: VpnType;
    status: VpnStatus;
    interface: string | null;
    local_ip: string | null;
    remote_ip: string | null;
    public_key: string | null;
    endpoint: string | null;
    allowed_ips: string[];
    created_at: string | null;
  }

  interface VpnCapabilities {
    wireguard: boolean;
    openvpn: boolean;
  }

  interface WireGuardServerStatus {
    public_key: string;
    listen_port: number;
    endpoint_host: string | null;
    /** `manual` | `ddns` | `public_ip` | `unset`. */
    endpoint_source: string;
    endpoint_warning: string | null;
    running: boolean;
  }

  interface WireGuardPeer {
    id: string;
    name: string;
    peer_ip: string;
    created_at: string;
    connected: boolean;
    latest_handshake: number | null;
  }

  interface WireGuardClient {
    id: string;
    name: string;
    interface: string;
    endpoint: string | null;
    addresses: string[];
    dns: string[];
    allowed_ips: string[];
    peer_count: number;
    kill_switch: boolean;
    autostart: boolean;
    created_at: string;
    interface_up: boolean;
    connected: boolean;
    latest_handshake: number | null;
    transfer_rx: number;
    transfer_tx: number;
  }

  // ---------------------------------------------------------------------------
  // State
  // ---------------------------------------------------------------------------

  let connections: VpnConnection[] = [];
  let capabilities: VpnCapabilities | null = null;
  let loading = true;
  let activeTab: 'wireguard' | 'wireguard-client' | 'openvpn' = 'wireguard';

  // WireGuard: one always-on server + a list of provisioned peers (devices).
  let wgServer: WireGuardServerStatus | null = null;
  let wgPeers: WireGuardPeer[] = [];

  // WireGuard client tunnels: imported provider configurations (outbound).
  let wgClients: WireGuardClient[] = [];
  let showWgClientModal = false;
  let wgClientName = '';
  let wgClientConfig = '';
  let wgClientKillSwitch = false;
  let wgClientAutostart = true;
  let wgClientFile: File | null = null;
  let importingWgClient = false;
  let wgClientBusyId: string | null = null;

  // Name of the OpenVPN connection whose start request is in flight — the
  // backend verifies the tunnel connects before answering (~1–10 s), so the
  // card's start button becomes a disabled spinner meanwhile.
  let connectingName: string | null = null;

  // Public endpoint shown in peer configs. The backend resolves it from the
  // manual override, then the DDNS hostname, then the detected public IP.
  interface WgEndpoint {
    endpoint_host: string | null;
    endpoint_source: string;
    endpoint_warning: string | null;
  }
  let wgEndpoint: WgEndpoint | null = null;
  let showEndpointModal = false;
  let endpointDraft = '';
  let endpointSaving = false;
  let endpointError = '';

  // Modals
  let showWireGuardModal = false;
  let showOpenVPNModal = false;
  let showPeerResultModal = false;
  let showPeerConfigModal = false;
  let showVpnCredsModal = false;
  let vpnCredsTarget = '';
  let vpnCredsUsername = '';
  let vpnCredsPassword = '';
  const dlg = createConfirmState();

  // "Add device" form — a name is the only input; everything else is
  // generated/detected automatically server-side.
  let wgPeerName = '';

  // The one-time reveal after creating a peer: its raw client config text
  // (for "Download .conf") and the same text rendered as a QR code data URL
  // (for scanning with the WireGuard app) — never fetchable again after
  // this modal closes, since the server never stores the private key.
  let newPeerName = '';
  let newPeerConfig = '';
  let newPeerQrDataUrl = '';
  let qrCopied = false;

  // Re-display an existing peer's config/QR code (fetched from backend).
  let configPeerName = '';
  let configPeerConfig = '';
  let configPeerQrDataUrl = '';
  let configQrCopied = false;
  let configLoading = false;

  // OpenVPN form
  let ovpnName = '';
  let ovpnConfigContent = '';
  let ovpnUsername = '';
  let ovpnPassword = '';
  let ovpnFile: File | null = null;

  // Loading states
  let creatingWg = false;
  let creatingOvpn = false;
  let revokingPeerId: string | null = null;

  // ---------------------------------------------------------------------------
  // Computed
  // ---------------------------------------------------------------------------

  $: openvpnConnections = connections.filter(c => c.vpn_type === 'openvpn');
  $: totalConnections = wgPeers.length + wgClients.length + openvpnConnections.length;
  $: connectedCount = wgPeers.filter(p => p.connected).length + wgClients.filter(c => c.connected).length + openvpnConnections.filter(c => c.status === 'connected').length;
  $: wireguardCount = wgPeers.length + wgClients.length;
  $: openvpnCount = openvpnConnections.length;
  $: wireguardClientCount = wgClients.length;

  // ---------------------------------------------------------------------------
  // Lifecycle
  // ---------------------------------------------------------------------------

  onMount(async () => {
    await loadAll();
  });

  // ---------------------------------------------------------------------------
  // Data Loading
  // ---------------------------------------------------------------------------

  /** Surfaces a VPN action's result via the notification center (not a toast). */
  function notifyVpn(message: string, type: 'success' | 'error' = 'success') {
    notificationsStore.add({ title: 'VPN', message, type });
  }

  async function loadAll() {
    loading = true;
    await loadCapabilities();
    await Promise.all([
      loadConnections(),
      capabilities?.wireguard ? loadWireGuardServer() : Promise.resolve(),
      capabilities?.wireguard ? loadWireGuardEndpoint() : Promise.resolve(),
      capabilities?.wireguard ? loadWireGuardPeers() : Promise.resolve(),
      capabilities?.wireguard ? loadWireGuardClients() : Promise.resolve(),
    ]);
    loading = false;
  }

  async function loadCapabilities() {
    const response = await api.getVpnCapabilities();
    if (response.success && response.data) {
      capabilities = response.data.capabilities;

      // Set default tab based on capabilities
      if (capabilities?.wireguard) {
        activeTab = 'wireguard';
      } else if (capabilities?.openvpn) {
        activeTab = 'openvpn';
      }
    }
  }

  async function loadConnections() {
    const response = await api.listVpnConnections();
    if (response.success && response.data) {
      connections = response.data.connections || [];
    }
  }

  // ---------------------------------------------------------------------------
  // WireGuard Actions
  // ---------------------------------------------------------------------------

  async function loadWireGuardServer() {
    const response = await api.getWireGuardServerStatus();
    if (response.success && response.data?.server) {
      wgServer = response.data.server;
    }
  }

  async function loadWireGuardEndpoint() {
    const response = await api.get('/vpn/wireguard/endpoint');
    if (response.success && response.data) wgEndpoint = response.data;
  }

  function openEndpointModal() {
    endpointDraft = wgEndpoint?.endpoint_host ?? '';
    endpointError = '';
    showEndpointModal = true;
  }

  async function saveEndpoint(host: string) {
    endpointSaving = true;
    endpointError = '';
    const response = await api.put('/vpn/wireguard/endpoint', { host });
    endpointSaving = false;
    if (response.success && response.data) {
      wgEndpoint = response.data;
      showEndpointModal = false;
      await loadWireGuardServer();
      notifyVpn(translate('vpn.endpoint_saved'));
    } else {
      endpointError = response.message || response.error || translate('vpn.endpoint_failed');
    }
  }

  async function loadWireGuardPeers() {
    const response = await api.listWireGuardPeers();
    if (response.success && response.data) {
      wgPeers = response.data.peers || [];
    }
  }

  async function loadWireGuardClients() {
    const response = await api.listWireGuardClients();
    if (response.success && response.data) {
      wgClients = response.data.clients || [];
    }
  }

  // ---------------------------------------------------------------------------
  // WireGuard client tunnels (imported provider configurations)
  // ---------------------------------------------------------------------------

  function handleWgClientFileUpload(event: Event) {
    const target = event.target as HTMLInputElement;
    const file = target.files?.[0];
    if (!file) return;
    wgClientFile = file;
    if (!wgClientName.trim()) {
      wgClientName = file.name.replace(/\.conf$/i, '');
    }
    const reader = new FileReader();
    reader.onload = (e) => {
      wgClientConfig = (e.target?.result as string) || '';
    };
    reader.readAsText(file);
  }

  async function importWireGuardClient() {
    const name = wgClientName.trim();
    const config = wgClientConfig.trim();
    if (!name || !config) return;

    importingWgClient = true;
    const response = await api.importWireGuardClient({
      name,
      config_content: config,
      kill_switch: wgClientKillSwitch,
      autostart: wgClientAutostart,
    });

    if (response.success && response.data?.client) {
      showWgClientModal = false;
      resetWgClientForm();
      await loadWireGuardClients();
      notifyVpn(translate('vpn.wg_client_created', { name }));
    } else {
      notifyVpn(response.error || translate('vpn.wg_client_create_failed'), 'error');
    }
    importingWgClient = false;
  }

  function resetWgClientForm() {
    wgClientName = '';
    wgClientConfig = '';
    wgClientKillSwitch = false;
    wgClientAutostart = true;
    wgClientFile = null;
  }

  async function startWireGuardClient(client: WireGuardClient) {
    wgClientBusyId = client.id;
    const response = await api.startWireGuardClient(client.id);
    if (response.success) {
      await loadWireGuardClients();
      notifyVpn(translate('vpn.wg_client_started', { name: client.name }));
    } else {
      notifyVpn(response.error || translate('vpn.wg_client_start_failed'), 'error');
    }
    wgClientBusyId = null;
  }

  async function stopWireGuardClient(client: WireGuardClient) {
    wgClientBusyId = client.id;
    const response = await api.stopWireGuardClient(client.id);
    if (response.success) {
      await loadWireGuardClients();
      notifyVpn(translate('vpn.wg_client_stopped', { name: client.name }));
    } else {
      notifyVpn(response.error || translate('vpn.wg_client_stop_failed'), 'error');
    }
    wgClientBusyId = null;
  }

  async function deleteWireGuardClient(client: WireGuardClient) {
    if (!(await dlg.ask(translate('vpn.wg_client_delete_confirm', { name: client.name })))) return;

    wgClientBusyId = client.id;
    const response = await api.deleteWireGuardClient(client.id);
    if (response.success) {
      await loadWireGuardClients();
      notifyVpn(translate('vpn.wg_client_deleted', { name: client.name }));
    } else {
      notifyVpn(response.error || translate('vpn.wg_client_delete_failed'), 'error');
    }
    wgClientBusyId = null;
  }

  async function toggleWgClientAutostart(client: WireGuardClient, autostart: boolean) {
    wgClientBusyId = client.id;
    const response = await api.setWireGuardClientAutostart(client.id, autostart);
    if (response.success) {
      await loadWireGuardClients();
      notifyVpn(translate('vpn.wg_client_autostart_updated'));
    } else {
      notifyVpn(response.error || translate('vpn.wg_client_update_failed'), 'error');
    }
    wgClientBusyId = null;
  }

  async function createWireGuardPeer() {
    const name = wgPeerName.trim();
    if (!name) return;

    creatingWg = true;
    const response = await api.createWireGuardPeer(name);

    if (response.success && response.data) {
      showWireGuardModal = false;
      wgPeerName = '';

      newPeerName = response.data.name;
      newPeerConfig = response.data.client_config;
      qrCopied = false;
      try {
        newPeerQrDataUrl = await QRCode.toDataURL(newPeerConfig, { width: 320, margin: 1 });
      } catch {
        newPeerQrDataUrl = '';
      }
      showPeerResultModal = true;

      await Promise.all([loadWireGuardServer(), loadWireGuardPeers()]);
    } else {
      notifyVpn(response.error || translate('vpn.peer_create_failed'), 'error');
    }

    creatingWg = false;
  }

  async function showPeerConfig(peer: WireGuardPeer) {
    configLoading = true;
    const response = await api.getWireGuardPeerConfig(peer.id);
    if (response.success && response.data) {
      configPeerName = response.data.name || peer.name;
      configPeerConfig = response.data.client_config;
      configQrCopied = false;
      try {
        configPeerQrDataUrl = await QRCode.toDataURL(configPeerConfig, { width: 320, margin: 1 });
      } catch {
        configPeerQrDataUrl = '';
      }
      showPeerConfigModal = true;
    } else {
      // silent fail — the peer might have been deleted
    }
    configLoading = false;
  }

  function downloadConfig(name: string, config: string) {
    const blob = new Blob([config], { type: 'text/plain' });
    const url = URL.createObjectURL(blob);
    const a = document.createElement('a');
    a.href = url;
    a.download = `${name.replace(/[^a-zA-Z0-9_-]/g, '_') || 'homevault-vpn'}.conf`;
    document.body.appendChild(a);
    a.click();
    document.body.removeChild(a);
    URL.revokeObjectURL(url);
  }

  async function copyConfig(config: string) {
    try {
      await navigator.clipboard.writeText(config);
      configQrCopied = true;
      setTimeout(() => { configQrCopied = false; }, 1500);
    } catch {
      // Clipboard permission denied
    }
  }

  async function revokeWireGuardPeer(peer: WireGuardPeer) {
    if (!(await dlg.ask(translate('vpn.revoke_peer_confirm', { name: peer.name })))) return;

    revokingPeerId = peer.id;
    const response = await api.revokeWireGuardPeer(peer.id);
    if (response.success) {
      notifyVpn(translate('vpn.peer_revoked', { name: peer.name }));
      await loadWireGuardPeers();
    } else {
      notifyVpn(response.error || translate('vpn.peer_revoke_failed'), 'error');
    }
    revokingPeerId = null;
  }

  function downloadPeerConfig() {
    const blob = new Blob([newPeerConfig], { type: 'text/plain' });
    const url = URL.createObjectURL(blob);
    const a = document.createElement('a');
    a.href = url;
    a.download = `${newPeerName.replace(/[^a-zA-Z0-9_-]/g, '_') || 'homevault-vpn'}.conf`;
    document.body.appendChild(a);
    a.click();
    document.body.removeChild(a);
    URL.revokeObjectURL(url);
  }

  async function copyPeerConfig() {
    try {
      await navigator.clipboard.writeText(newPeerConfig);
      qrCopied = true;
      setTimeout(() => { qrCopied = false; }, 1500);
    } catch {
      // Clipboard permission denied/unavailable — the Download button still works.
    }
  }

  function formatLastHandshake(epochSeconds: number | null): string {
    if (!epochSeconds) return translate('vpn.never_connected');
    return new Date(epochSeconds * 1000).toLocaleString();
  }

  // ---------------------------------------------------------------------------
  // OpenVPN Actions
  // ---------------------------------------------------------------------------

  function handleFileUpload(event: Event) {
    const target = event.target as HTMLInputElement;
    const file = target.files?.[0];
    if (file) {
      ovpnFile = file;
      ovpnName = file.name.replace('.ovpn', '').replace('.conf', '');
      
      const reader = new FileReader();
      reader.onload = (e) => {
        ovpnConfigContent = e.target?.result as string;
      };
      reader.readAsText(file);
    }
  }

  async function createOpenVPN() {
    if (!ovpnName || !ovpnConfigContent) {
      return;
    }

    creatingOvpn = true;

    const response = await api.createOpenVPNConnection({
      name: ovpnName,
      config_content: ovpnConfigContent,
      username: ovpnUsername || undefined,
      password: ovpnPassword || undefined,
    });

    if (response.success) {
      showOpenVPNModal = false;
      resetOpenVPNForm();
      await loadConnections();
      notifyVpn(translate('vpn.ovpn_created', { name: ovpnName }));
    } else {
      notifyVpn(response.error || translate('vpn.ovpn_create_failed'), 'error');
    }

    creatingOvpn = false;
  }

  function resetOpenVPNForm() {
    ovpnName = '';
    ovpnConfigContent = '';
    ovpnUsername = '';
    ovpnPassword = '';
    ovpnFile = null;
  }

  function startOpenVPN(name: string) {
    vpnCredsTarget = name;
    vpnCredsUsername = '';
    vpnCredsPassword = '';
    showVpnCredsModal = true;
  }

  async function doStartOpenVPN() {
    const name = vpnCredsTarget;
    const username = vpnCredsUsername.trim();
    const password = vpnCredsPassword;
    showVpnCredsModal = false;

    // Empty fields = the server accepts the connection without credentials
    // (previously this was a separate confirm() + two prompt() dialogs).
    const credentials = username && password ? { username, password } : undefined;

    // The backend verifies the tunnel actually connects before answering
    // (up to ~10 s): surface that as a per-card connecting state instead of
    // an apparently frozen button.
    connectingName = name;
    try {
      const response = await api.startOpenVPNConnection(name, credentials);
      if (response.success) {
        await loadConnections();
        notifyVpn(translate('vpn.ovpn_started', { name }));
      } else {
        notifyVpn(response.error || translate('vpn.ovpn_start_failed'), 'error');
      }
    } finally {
      connectingName = null;
    }
  }

  async function stopOpenVPN(name: string) {
    const response = await api.stopOpenVPNConnection(name);
    if (response.success) {
      await loadConnections();
      notifyVpn(translate('vpn.ovpn_stopped', { name }));
    } else {
      notifyVpn(response.error || translate('vpn.ovpn_stop_failed'), 'error');
    }
  }

  async function deleteOpenVPN(name: string) {
    if (!(await dlg.ask(translate('vpn.delete_ovpn_confirm', { name })))) return;

    const response = await api.deleteOpenVPNConnection(name);
    if (response.success) {
      await loadConnections();
      notifyVpn(translate('vpn.ovpn_deleted', { name }));
    } else {
      notifyVpn(response.error || translate('vpn.ovpn_delete_failed'), 'error');
    }
  }

  // ---------------------------------------------------------------------------
  // Helpers
  // ---------------------------------------------------------------------------

  function getStatusVariant(status: VpnStatus): string {
    switch (status) {
      case 'connected': return 'success';
      case 'disconnected': return 'error';
      case 'connecting': return 'info';
      case 'error': return 'error';
      default: return 'warning';
    }
  }

  function getStatusLabel(status: VpnStatus): string {
    return translate(`vpn.status_${status}`);
  }

  const detailLabel = 'text-xs text-[var(--hv-shell-text-2)]';
  const detailValue = 'text-sm font-semibold text-[var(--hv-shell-text-strong)]';
  const detailMono = cn(detailValue, 'font-[family-name:var(--hv-font-mono)]');
  const badge = 'inline-flex items-center gap-1.5 px-2 py-1 rounded-full text-c-xs font-bold uppercase tracking-wide';
</script>
<PageLayout
  title="VPN"
  data-testid="vpn-page"
  tabs={capabilities?.wireguard || capabilities?.openvpn ? [
    ...(capabilities?.wireguard ? [{
      id: 'wireguard',
      label: 'WireGuard',
      icon: IconShield,
      count: wgPeers.length > 0 ? wgPeers.length : undefined
    }] : []),
    ...(capabilities?.wireguard ? [{
      id: 'wireguard-client',
      label: $t('vpn.tab_wireguard_client'),
      icon: IconShield,
      count: wireguardClientCount > 0 ? wireguardClientCount : undefined
    }] : []),
    ...(capabilities?.openvpn ? [{
      id: 'openvpn',
      label: 'OpenVPN',
      icon: IconShield,
      count: openvpnConnections.length > 0 ? openvpnConnections.length : undefined
    }] : [])
  ] : undefined}
  bind:activeTab
>
  {#snippet icon()}
    <IconShield strokeWidth={1.8} />
  {/snippet}
  <!-- Page-level actions: a WindowToolbar row under the header, never inside
       it (the header bar is icon + title + window controls only). -->
  {#snippet toolbarRight()}
    <Button variant="ghost" onclick={loadAll}>
      <IconLoader size={16} />
      <span>{$t('vpn.refresh')}</span>
    </Button>
    {#if activeTab === 'wireguard' && capabilities?.wireguard}
      <Button variant="primary" data-testid="vpn-btn" onclick={() => { showWireGuardModal = true; }}>
        <IconPlus size={16} />
        <span>{$t('vpn.new_wireguard')}</span>
      </Button>
    {/if}
    {#if activeTab === 'wireguard-client' && capabilities?.wireguard}
      <Button variant="primary" data-testid="vpn-import-client-btn" onclick={() => { showWgClientModal = true; }}>
        <IconPlus size={16} />
        <span>{$t('vpn.new_wg_client')}</span>
      </Button>
    {/if}
    {#if activeTab === 'openvpn' && capabilities?.openvpn}
      <Button variant="primary" data-testid="vpn-btn" onclick={() => { showOpenVPNModal = true; }}>
        <IconPlus size={16} />
        <span>{$t('vpn.new_openvpn')}</span>
      </Button>
    {/if}
  {/snippet}

  {#if loading}
    <div data-testid="vpn-loading" class="flex items-center justify-center gap-3 py-16 text-[var(--hv-shell-text)]">
      <Spinner />
      <span>{$t('vpn.loading')}</span>
    </div>
  {:else if !capabilities?.wireguard && !capabilities?.openvpn}
    <EmptyState data-testid="vpn-empty-title" title={$t('vpn.no_support_title')} description={$t('vpn.no_support_desc')}>
      {#snippet iconSnippet()}<IconShield size={32} strokeWidth={1.8} />{/snippet}
    </EmptyState>
  {:else}
    <div class="grid grid-cols-2 @min-[880px]:grid-cols-4 gap-3 mb-5">
      <StatCard data-testid="vpn-stat-card" label={$t('vpn.stat_total')} value={`${totalConnections}`} tone={'info'}>
        {#snippet iconSnippet()}<IconShield size={20} />{/snippet}
      </StatCard>
      <StatCard data-testid="vpn-stat-card" label={$t('vpn.stat_connected')} value={`${connectedCount}`} tone={'success'}>
        {#snippet iconSnippet()}<IconPower size={20} />{/snippet}
      </StatCard>
      <StatCard data-testid="vpn-stat-card" label="WireGuard" value={`${wireguardCount}`} tone={'info'}>
        {#snippet iconSnippet()}<IconShield size={20} />{/snippet}
      </StatCard>
      <StatCard data-testid="vpn-stat-card" label="OpenVPN" value={`${openvpnCount}`} tone={'warning'}>
        {#snippet iconSnippet()}<IconShield size={20} />{/snippet}
      </StatCard>
    </div>

    {#if activeTab === 'wireguard'}
      <!-- Public endpoint: manual override, then DDNS, then the detected IP. -->
      <div
        data-testid="vpn-endpoint-card"
        class="px-4 py-3 rounded-lg bg-card border border-border mb-4"
      >
        <div class="flex items-center gap-3 flex-wrap">
          <span class={detailLabel}>{$t('vpn.server_endpoint')}</span>
          <span data-testid="vpn-endpoint-value" class={detailMono}>
            {wgEndpoint?.endpoint_host
              ? `${wgEndpoint.endpoint_host}:${wgServer?.listen_port ?? 51820}`
              : $t('vpn.endpoint_unset')}
          </span>
          {#if wgEndpoint}
            <span
              data-testid="vpn-endpoint-source"
              class="px-2 py-0.5 rounded-full border border-[var(--hv-shell-border)] text-c-xs text-[var(--hv-shell-text-2)]"
            >
              {$t(`vpn.endpoint_source_${wgEndpoint.endpoint_source}`)}
            </span>
          {/if}
          <Button variant="ghost" onclick={openEndpointModal} data-testid="vpn-endpoint-edit">
            {$t('vpn.endpoint_edit')}
          </Button>
        </div>
        {#if wgEndpoint?.endpoint_warning}
          <p class="m-0 mt-2 text-c-xs text-[var(--hv-warning)]" data-testid="vpn-endpoint-warning">
            {$t(wgEndpoint.endpoint_host ? 'vpn.endpoint_warning_private' : 'vpn.endpoint_warning_unset')}
          </p>
        {/if}
      </div>

      {#if wgServer}
        <div class="flex items-center gap-3 flex-wrap px-4 py-3 rounded-lg bg-card border border-border mb-4">
          <span class="flex items-center gap-2 text-sm font-semibold text-[var(--hv-shell-text-strong)]">
            <span class={cn('w-2.5 h-2.5 rounded-full', wgServer.running ? 'bg-[var(--hv-success)]' : 'bg-[var(--hv-shell-text-3)]')}></span>
            {wgServer.running ? $t('vpn.server_running') : $t('vpn.server_stopped')}
          </span>
          {#if wgServer.endpoint_host}
            <span class="ml-auto text-xs"><span class={detailLabel}>{$t('vpn.server_endpoint')}</span> <span class={detailMono}>{wgServer.endpoint_host}:{wgServer.listen_port}</span></span>
          {/if}
        </div>
      {/if}

      {#if wgPeers.length === 0}
        <EmptyState data-testid="vpn-empty-title" title={$t('vpn.no_wg_title')} description={$t('vpn.no_wg_desc')}>
          {#snippet iconSnippet()}<IconDeviceMobile size={32} strokeWidth={1.8} />{/snippet}
          {#snippet action()}<Button variant="primary" data-testid="vpn-btn" onclick={() => { showWireGuardModal = true; }}><IconPlus size={16} /> <span>{$t('vpn.new_wireguard')}</span></Button>{/snippet}
        </EmptyState>
      {:else}
        <div class="grid gap-4 [grid-template-columns:repeat(auto-fill,minmax(280px,1fr))]">
          {#each wgPeers as peer (peer.id)}
            <div data-testid="vpn-card" class="rounded-2xl bg-card border border-border shadow-[var(--hv-shell-shadow-card)] overflow-hidden">
              <div class="flex items-center justify-between gap-3 px-4 pt-4">
                <div class="flex items-center gap-3 min-w-0">
                  <span class="grid place-items-center w-8 h-8 rounded-[9px] flex-none bg-[var(--hv-primary-light)] text-[var(--hv-primary)]"><IconDeviceMobile size={20} /></span>
                  <div class="min-w-0">
                    <div data-testid="vpn-card-name" class="text-sm font-bold text-[var(--hv-shell-text-strong)] truncate">{peer.name}</div>
                    <div class="text-xs text-[var(--hv-shell-text-2)]">WireGuard</div>
                  </div>
                </div>
                <span class={cn(badge, peer.connected ? 'bg-[var(--hv-success-bg)] text-[var(--hv-success)]' : 'bg-[var(--hv-shell-border-soft)] text-[var(--hv-shell-text-2)]')}>
                  <span class={cn('w-1.5 h-1.5 rounded-full', peer.connected ? 'bg-[var(--hv-success)]' : 'bg-[var(--hv-shell-text-3)]')}></span>
                  {peer.connected ? $t('vpn.status_connected') : $t('vpn.status_disconnected')}
                </span>
              </div>
              <div class="px-4 py-3 flex flex-col gap-1.5">
                <div class="flex items-center justify-between gap-2"><span class={detailLabel}>{$t('vpn.tunnel_ip')}</span><span class={detailMono}>{peer.peer_ip}</span></div>
                <div class="flex items-center justify-between gap-2"><span class={detailLabel}>{$t('vpn.last_handshake_label')}</span><span class={detailValue}>{formatLastHandshake(peer.latest_handshake)}</span></div>
              </div>
              <div class="px-4 pb-4 flex gap-2">
                <button data-testid="vpn-btn" class="grid place-items-center w-11 h-11 rounded-lg border border-[var(--hv-shell-border-strong)] bg-[var(--hv-shell-border-soft)] text-[var(--hv-shell-text-strong)] cursor-pointer hover:bg-[var(--hv-shell-border)]" onclick={() => showPeerConfig(peer)} disabled={configLoading} title={$t('vpn.show_config_title')}><IconDeviceMobile size={14} /></button>
                <button data-testid="vpn-btn" class="grid place-items-center w-11 h-11 rounded-lg border border-[var(--hv-shell-border-strong)] bg-[var(--hv-shell-border-soft)] text-[var(--hv-shell-text-strong)] cursor-pointer hover:bg-[var(--hv-error-bg)] hover:text-[var(--hv-error)]" onclick={() => revokeWireGuardPeer(peer)} disabled={revokingPeerId === peer.id} title={$t('vpn.revoke_device_title')}><IconTrash size={14} /></button>
              </div>
            </div>
          {/each}
        </div>
      {/if}

    {:else if activeTab === 'wireguard-client'}
      {#if wgClients.length === 0}
        <EmptyState data-testid="vpn-client-empty-title" title={$t('vpn.no_wg_client_title')} description={$t('vpn.no_wg_client_desc')}>
          {#snippet iconSnippet()}<IconShield size={32} strokeWidth={1.8} />{/snippet}
          {#snippet action()}<Button variant="primary" data-testid="vpn-import-client-btn" onclick={() => { showWgClientModal = true; }}><IconPlus size={16} /> <span>{$t('vpn.new_wg_client')}</span></Button>{/snippet}
        </EmptyState>
      {:else}
        <div class="grid gap-4 [grid-template-columns:repeat(auto-fill,minmax(300px,1fr))]">
          {#each wgClients as client (client.id)}
            <div data-testid="vpn-client-card" class="rounded-2xl bg-card border border-border shadow-[var(--hv-shell-shadow-card)] overflow-hidden">
              <div class="flex items-center justify-between gap-3 px-4 pt-4">
                <div class="flex items-center gap-3 min-w-0">
                  <span class="grid place-items-center w-8 h-8 rounded-[9px] flex-none bg-[var(--hv-primary-light)] text-[var(--hv-primary)]"><IconShield size={20} /></span>
                  <div class="min-w-0">
                    <div data-testid="vpn-client-name" class="text-sm font-bold text-[var(--hv-shell-text-strong)] truncate">{client.name}</div>
                    <div class="text-xs text-[var(--hv-shell-text-2)]">WireGuard · {client.interface}</div>
                  </div>
                </div>
                <span class={cn(badge, client.connected ? 'bg-[var(--hv-success-bg)] text-[var(--hv-success)]' : client.interface_up ? 'bg-[var(--hv-warning-bg)] text-[var(--hv-warning)]' : 'bg-[var(--hv-shell-border-soft)] text-[var(--hv-shell-text-2)]')}>
                  <span class={cn('w-1.5 h-1.5 rounded-full', client.connected ? 'bg-[var(--hv-success)]' : client.interface_up ? 'bg-[var(--hv-warning)]' : 'bg-[var(--hv-shell-text-3)]')}></span>
                  {client.connected ? $t('vpn.status_connected') : client.interface_up ? $t('vpn.wg_client_status_up') : $t('vpn.status_disconnected')}
                </span>
              </div>
              <div class="px-4 py-3 flex flex-col gap-1.5">
                <div class="flex items-center justify-between gap-2"><span class={detailLabel}>{$t('vpn.wg_client_endpoint_label')}</span><span class={detailMono}>{client.endpoint || $t('vpn.wg_client_no_endpoint')}</span></div>
                <div class="flex items-center justify-between gap-2"><span class={detailLabel}>{$t('vpn.wg_client_address_label')}</span><span class={detailMono}>{client.addresses.join(', ')}</span></div>
                <div class="flex items-center justify-between gap-2"><span class={detailLabel}>{$t('vpn.wg_client_peers_label')}</span><span class={detailValue}>{client.peer_count}</span></div>
                <div class="flex items-center gap-2 flex-wrap mt-1">
                  {#if client.kill_switch}<span class={cn(badge, 'bg-[var(--hv-error-bg)] text-[var(--hv-error)]')}>{$t('vpn.wg_client_kill_switch_badge')}</span>{/if}
                  <label for={`wg-client-autostart-${client.id}`} class="inline-flex items-center gap-1.5 min-h-11 px-1 text-xs text-[var(--hv-shell-text-2)] cursor-pointer">
                    <input id={`wg-client-autostart-${client.id}`} type="checkbox" checked={client.autostart} disabled={wgClientBusyId === client.id} onchange={(e) => toggleWgClientAutostart(client, e.currentTarget.checked)} class="w-4 h-4 accent-[var(--hv-primary)]" />
                    <span>{$t('vpn.wg_client_autostart')}</span>
                  </label>
                </div>
              </div>
              <div class="px-4 pb-4 flex gap-2">
                {#if client.interface_up}
                  <button data-testid="vpn-client-stop" class="grid place-items-center w-11 h-11 rounded-lg border border-[var(--hv-shell-border-strong)] bg-[var(--hv-shell-border-soft)] text-[var(--hv-shell-text-strong)] cursor-pointer hover:bg-[var(--hv-shell-border)]" onclick={() => stopWireGuardClient(client)} disabled={wgClientBusyId === client.id} aria-label={$t('vpn.wg_client_stop')} title={$t('vpn.wg_client_stop')}><IconPower size={14} /></button>
                {:else}
                  <button data-testid="vpn-client-start" class="grid place-items-center w-11 h-11 rounded-lg border-none bg-[var(--hv-success)] text-white cursor-pointer hover:brightness-110" onclick={() => startWireGuardClient(client)} disabled={wgClientBusyId === client.id} aria-label={$t('vpn.wg_client_start')} title={$t('vpn.wg_client_start')}><IconPower size={14} /></button>
                {/if}
                <button data-testid="vpn-client-delete" class="grid place-items-center w-11 h-11 rounded-lg border border-[var(--hv-shell-border-strong)] bg-[var(--hv-shell-border-soft)] text-[var(--hv-shell-text-strong)] cursor-pointer hover:bg-[var(--hv-error-bg)] hover:text-[var(--hv-error)]" onclick={() => deleteWireGuardClient(client)} disabled={wgClientBusyId === client.id} aria-label={$t('vpn.wg_client_delete')} title={$t('vpn.wg_client_delete')}><IconTrash size={14} /></button>
              </div>
            </div>
          {/each}
        </div>
      {/if}

    {:else if activeTab === 'openvpn'}
      {#if openvpnConnections.length === 0}
        <EmptyState data-testid="vpn-empty-title" title={$t('vpn.no_ovpn_title')} description={$t('vpn.no_ovpn_desc')}>
          {#snippet iconSnippet()}<IconShield size={32} strokeWidth={1.8} />{/snippet}
          {#snippet action()}<Button variant="primary" data-testid="vpn-btn" onclick={() => { showOpenVPNModal = true; }}><IconPlus size={16} /> <span>{$t('vpn.new_connection')}</span></Button>{/snippet}
        </EmptyState>
      {:else}
        <div class="grid gap-4 [grid-template-columns:repeat(auto-fill,minmax(280px,1fr))]">
          {#each openvpnConnections as conn}
            <div data-testid="vpn-card" class="rounded-2xl bg-card border border-border shadow-[var(--hv-shell-shadow-card)] overflow-hidden">
              <div class="flex items-center justify-between gap-3 px-4 pt-4">
                <div class="flex items-center gap-3 min-w-0">
                  <span class="grid place-items-center w-8 h-8 rounded-[9px] flex-none bg-[var(--hv-warning-bg)] text-[var(--hv-warning)]"><IconShield size={20} /></span>
                  <div class="min-w-0">
                    <div data-testid="vpn-card-name" class="text-sm font-bold text-[var(--hv-shell-text-strong)] truncate">{conn.name}</div>
                    <div class="text-xs text-[var(--hv-shell-text-2)]">OpenVPN</div>
                  </div>
                </div>
                <span class={cn(badge, conn.status === 'connected' ? 'bg-[var(--hv-success-bg)] text-[var(--hv-success)]' : conn.status === 'error' ? 'bg-[var(--hv-error-bg)] text-[var(--hv-error)]' : 'bg-[var(--hv-shell-border-soft)] text-[var(--hv-shell-text-2)]')}>
                  <span class={cn('w-1.5 h-1.5 rounded-full', conn.status === 'connected' ? 'bg-[var(--hv-success)]' : conn.status === 'error' ? 'bg-[var(--hv-error)]' : 'bg-[var(--hv-shell-text-3)]')}></span>
                  {getStatusLabel(conn.status)}
                </span>
              </div>
              <div class="px-4 py-3 flex flex-col gap-1.5">
                {#if conn.interface}<div class="flex items-center justify-between gap-2"><span class={detailLabel}>{$t('vpn.interface_label')}</span><span class={detailValue}>{conn.interface}</span></div>{/if}
                {#if conn.local_ip}<div class="flex items-center justify-between gap-2"><span class={detailLabel}>{$t('vpn.local_ip')}</span><span class={detailValue}>{conn.local_ip}</span></div>{/if}
              </div>
              <div class="px-4 pb-4 flex gap-2">
                {#if connectingName === conn.name}
                  <button data-testid="vpn-btn" class="grid place-items-center w-11 h-11 rounded-lg border border-[var(--hv-shell-border-strong)] bg-[var(--hv-shell-border-soft)] text-[var(--hv-shell-text-strong)] cursor-wait opacity-70" disabled aria-disabled="true" title={$t('vpn.connecting')}><span class="animate-spin"><IconLoader size={14} /></span></button>
                {:else if conn.status === 'connected'}
                  <button data-testid="vpn-btn" class="grid place-items-center w-11 h-11 rounded-lg border border-[var(--hv-shell-border-strong)] bg-[var(--hv-shell-border-soft)] text-[var(--hv-shell-text-strong)] cursor-pointer hover:bg-[var(--hv-shell-border)]" onclick={() => stopOpenVPN(conn.name)} title={$t('vpn.stop_connection_title')}><IconPower size={14} /></button>
                {:else}
                  <button data-testid="vpn-btn" class="grid place-items-center w-11 h-11 rounded-lg border-none bg-[var(--hv-success)] text-white cursor-pointer hover:brightness-110" onclick={() => startOpenVPN(conn.name)} title={$t('vpn.start_connection_title')}><IconPower size={14} /></button>
                {/if}
                <button data-testid="vpn-btn" class="grid place-items-center w-11 h-11 rounded-lg border border-[var(--hv-shell-border-strong)] bg-[var(--hv-shell-border-soft)] text-[var(--hv-shell-text-strong)] cursor-pointer hover:bg-[var(--hv-error-bg)] hover:text-[var(--hv-error)]" onclick={() => deleteOpenVPN(conn.name)} title={$t('vpn.delete_connection_title')}><IconTrash size={14} /></button>
              </div>
            </div>
          {/each}
        </div>
      {/if}
    {/if}
  {/if}
  <ConfirmDialog
    {dlg}
    onconfirm={() => dlg.settle(true)}
    oncancel={() => dlg.settle(false)}
  />
</PageLayout>

<!-- Add WireGuard Device Modal -->
<Modal open={showWireGuardModal} title={$t('vpn.create_wg_title')} size="sm" onclose={() => { showWireGuardModal = false; wgPeerName = ''; }}>
  <div class="flex flex-col gap-1.5">
    <label for="wg-peer-name" class="text-c-label font-semibold text-[var(--hv-shell-text-strong)]">{$t('vpn.device_name')} <span class="text-[var(--hv-error)]">*</span></label>
    <input id="wg-peer-name" class="w-full h-11 px-3 border border-[var(--hv-shell-border-strong)] rounded-lg bg-[var(--hv-shell-border-soft)] text-sm outline-none focus:border-[var(--hv-primary)]" type="text" bind:value={wgPeerName} placeholder={$t('vpn.device_name_placeholder')} onkeydown={(e) => { if (e.key === 'Enter' && wgPeerName.trim()) createWireGuardPeer(); }} />
    <span class="text-c-xs text-[var(--hv-shell-text-2)]">{$t('vpn.device_name_hint')}</span>
  </div>
  {#snippet footer()}
    <div class="flex gap-2.5 justify-end">
      <Button variant="ghost" onclick={() => { showWireGuardModal = false; wgPeerName = ''; }} disabled={creatingWg}>{$t('vpn.cancel')}</Button>
      <Button variant="primary" onclick={createWireGuardPeer} disabled={creatingWg || !wgPeerName.trim()}>{creatingWg ? $t('vpn.creating_device') : $t('vpn.create_device')}</Button>
    </div>
  {/snippet}
</Modal>

<!-- OpenVPN Creation Modal -->
<Modal open={showOpenVPNModal} title={$t('vpn.create_ovpn_title')} size="md" onclose={() => showOpenVPNModal = false}>
  <div class="mb-3">
    <div class="text-xs font-bold uppercase tracking-wide text-[var(--hv-shell-text-3)] mb-2">{$t('vpn.connection_details')}</div>
    <div class="flex flex-col gap-1.5">
      <label for="ovpn-name" class="text-c-label font-semibold text-[var(--hv-shell-text-strong)]">{$t('vpn.connection_name')} <span class="text-[var(--hv-error)]">*</span></label>
      <input id="ovpn-name" class="w-full h-11 px-3 border border-[var(--hv-shell-border-strong)] rounded-lg bg-[var(--hv-shell-border-soft)] text-sm outline-none focus:border-[var(--hv-primary)]" type="text" bind:value={ovpnName} placeholder="my-openvpn" />
    </div>
  </div>
  <div class="mb-3">
    <div class="text-xs font-bold uppercase tracking-wide text-[var(--hv-shell-text-3)] mb-2">{$t('vpn.configuration_file')}</div>
    <div class="flex flex-col gap-1.5">
      <input type="file" accept=".ovpn,.conf" onchange={handleFileUpload} id="ovpn-file" class="sr-only" />
      <label for="ovpn-file" class="flex items-center gap-2 h-11 px-4 border border-[var(--hv-shell-border-strong)] rounded-lg bg-[var(--hv-shell-fill)] text-c-label font-semibold text-[var(--hv-shell-text-strong)] cursor-pointer hover:bg-[var(--hv-shell-border)]"><IconUpload size={18} /> {ovpnFile ? ovpnFile.name : $t('vpn.choose_file')}</label>
    </div>
    {#if ovpnConfigContent}
      <div data-testid="vpn-file-preview" class="flex items-center gap-1.5 mt-2 text-xs text-[var(--hv-success)]"><IconCheck size={14} /> {translate('vpn.config_loaded', { bytes: String(ovpnConfigContent.length) })}</div>
    {/if}
  </div>
  <div>
    <div class="text-xs font-bold uppercase tracking-wide text-[var(--hv-shell-text-3)] mb-2">{$t('vpn.auth_optional')}</div>
    <div class="grid grid-cols-2 @max-[360px]:grid-cols-1 gap-3">
      <div class="flex flex-col gap-1.5"><label for="ovpn-user" class="text-c-label font-semibold text-[var(--hv-shell-text-strong)]">{$t('vpn.username_label')}</label><input id="ovpn-user" class="w-full h-11 px-3 border border-[var(--hv-shell-border-strong)] rounded-lg bg-[var(--hv-shell-border-soft)] text-sm outline-none focus:border-[var(--hv-primary)]" type="text" bind:value={ovpnUsername} placeholder="username" /></div>
      <div class="flex flex-col gap-1.5"><label for="ovpn-pass" class="text-c-label font-semibold text-[var(--hv-shell-text-strong)]">{$t('vpn.password_label')}</label><input id="ovpn-pass" class="w-full h-11 px-3 border border-[var(--hv-shell-border-strong)] rounded-lg bg-[var(--hv-shell-border-soft)] text-sm outline-none focus:border-[var(--hv-primary)]" type="password" bind:value={ovpnPassword} placeholder="password" /></div>
    </div>
  </div>
  {#snippet footer()}
    <div class="flex gap-2.5 justify-end">
      <Button variant="ghost" onclick={() => showOpenVPNModal = false} disabled={creatingOvpn}>{$t('vpn.cancel')}</Button>
      <Button variant="primary" onclick={createOpenVPN} disabled={creatingOvpn || !ovpnConfigContent}>{creatingOvpn ? $t('vpn.creating') : $t('vpn.create_connection')}</Button>
    </div>
  {/snippet}
</Modal>

<!-- OpenVPN Auth Credentials Modal -->
<Modal open={showVpnCredsModal} title={$t('vpn.auth_optional')} size="sm" onclose={() => showVpnCredsModal = false}>
  <div class="grid grid-cols-2 @max-[320px]:grid-cols-1 gap-3">
    <div class="flex flex-col gap-1.5"><label for="vpn-creds-user" class="text-c-label font-semibold text-[var(--hv-shell-text-strong)]">{$t('vpn.username_label')}</label><input id="vpn-creds-user" class="w-full h-11 px-3 border border-[var(--hv-shell-border-strong)] rounded-lg bg-[var(--hv-shell-border-soft)] text-sm outline-none focus:border-[var(--hv-primary)]" type="text" bind:value={vpnCredsUsername} placeholder="username" /></div>
    <div class="flex flex-col gap-1.5"><label for="vpn-creds-pass" class="text-c-label font-semibold text-[var(--hv-shell-text-strong)]">{$t('vpn.password_label')}</label><input id="vpn-creds-pass" class="w-full h-11 px-3 border border-[var(--hv-shell-border-strong)] rounded-lg bg-[var(--hv-shell-border-soft)] text-sm outline-none focus:border-[var(--hv-primary)]" type="password" bind:value={vpnCredsPassword} placeholder="password" /></div>
  </div>
  {#snippet footer()}
    <div class="flex gap-2.5 justify-end">
      <Button variant="ghost" onclick={() => showVpnCredsModal = false}>{$t('vpn.cancel')}</Button>
      <Button variant="primary" onclick={doStartOpenVPN}>{$t('vpn.start_connection_title')}</Button>
    </div>
  {/snippet}
</Modal>

<!-- Device Added — QR Code Modal -->
<Modal open={showPeerResultModal} title={$t('vpn.device_created_title')} size="sm" onclose={() => { showPeerResultModal = false; newPeerConfig = ''; newPeerQrDataUrl = ''; }}>
  <div class="flex flex-col items-center gap-2 text-center">
    {#if newPeerQrDataUrl}<img class="w-48 h-48" src={newPeerQrDataUrl} alt={$t('vpn.scan_qr_instructions')} />{/if}
    <div class="text-sm font-semibold text-[var(--hv-shell-text-strong)]">{newPeerName}</div>
    <p class="m-0 text-xs text-[var(--hv-shell-text)]">{$t('vpn.scan_qr_instructions')}</p>
    <div class="flex gap-2.5 mt-2">
      <Button variant="ghost" onclick={downloadPeerConfig}><IconDownload size={16} /> <span>{$t('vpn.download_config')}</span></Button>
      <Button variant="ghost" onclick={copyPeerConfig}>{#if qrCopied}<IconCheck size={16} /> <span>{$t('vpn.copied')}</span>{:else}<IconCopy size={16} /> <span>{$t('vpn.copy_config')}</span>{/if}</Button>
    </div>
    <p class="m-0 text-c-xs text-[var(--hv-shell-text-2)]">{$t('vpn.qr_one_time_notice')}</p>
  </div>
  {#snippet footer()}
    <div class="flex justify-end"><Button variant="primary" onclick={() => { showPeerResultModal = false; newPeerConfig = ''; newPeerQrDataUrl = ''; }}>{$t('vpn.close')}</Button></div>
  {/snippet}
</Modal>

<!-- Re-display existing peer's config / QR code -->
<Modal open={showPeerConfigModal} title={configPeerName} size="sm" onclose={() => { showPeerConfigModal = false; configPeerConfig = ''; configPeerQrDataUrl = ''; }}>
  <div class="flex flex-col items-center gap-2 text-center">
    {#if configPeerQrDataUrl}
      <img class="w-48 h-48" src={configPeerQrDataUrl} alt={$t('vpn.scan_qr_instructions')} />
    {:else if configLoading}
      <div class="py-8"><IconLoader size={32} class="vpn-spin" /></div>
    {/if}
    <p class="m-0 text-xs text-[var(--hv-shell-text)]">{$t('vpn.scan_qr_instructions')}</p>
    <div class="flex gap-2.5 mt-2">
      <Button variant="ghost" onclick={() => downloadConfig(configPeerName, configPeerConfig)}><IconDownload size={16} /> <span>{$t('vpn.download_config')}</span></Button>
      <Button variant="ghost" onclick={() => copyConfig(configPeerConfig)}>{#if configQrCopied}<IconCheck size={16} /> <span>{$t('vpn.copied')}</span>{:else}<IconCopy size={16} /> <span>{$t('vpn.copy_config')}</span>{/if}</Button>
    </div>
  </div>
  {#snippet footer()}
    <div class="flex justify-end"><Button variant="primary" onclick={() => { showPeerConfigModal = false; configPeerConfig = ''; configPeerQrDataUrl = ''; }}>{$t('vpn.close')}</Button></div>
  {/snippet}
</Modal>

<!-- Public endpoint: manual override, or automatic (DDNS then public IP). -->
<Modal open={showEndpointModal} title={$t('vpn.endpoint_title')} size="sm" onclose={() => (showEndpointModal = false)}>
  <div class="flex flex-col gap-1.5">
    <label for="wg-endpoint-host" class="text-c-label font-semibold text-[var(--hv-shell-text-strong)]">{$t('vpn.endpoint_field')}</label>
    <input
      id="wg-endpoint-host"
      class="w-full h-11 px-3 border border-[var(--hv-shell-border-strong)] rounded-lg bg-[var(--hv-shell-border-soft)] text-sm outline-none focus:border-[var(--hv-primary)]"
      type="text"
      bind:value={endpointDraft}
      placeholder="home.example.com"
      data-testid="vpn-endpoint-input"
    />
    <span class="text-c-xs text-[var(--hv-shell-text-2)]">{$t('vpn.endpoint_hint')}</span>
    {#if endpointError}
      <p role="alert" class="m-0 text-c-xs text-[var(--hv-error)]" data-testid="vpn-endpoint-error">
        {$t('vpn.endpoint_failed')}
      </p>
    {/if}
  </div>
  {#snippet footer()}
    <div class="flex gap-2.5 justify-end">
      <Button variant="ghost" onclick={() => saveEndpoint('')} disabled={endpointSaving}>{$t('vpn.endpoint_automatic')}</Button>
      <Button variant="primary" onclick={() => saveEndpoint(endpointDraft)} disabled={endpointSaving} data-testid="vpn-endpoint-save">
        {endpointSaving ? $t('vpn.endpoint_saving') : $t('vpn.endpoint_save')}
      </Button>
    </div>
  {/snippet}
</Modal>

<!-- Import a WireGuard client configuration (provider tunnel) -->
<Modal open={showWgClientModal} title={$t('vpn.import_wg_client_title')} size="md" onclose={() => { showWgClientModal = false; resetWgClientForm(); }}>
  <div class="mb-3">
    <div class="flex flex-col gap-1.5">
      <label for="wg-client-name" class="text-c-label font-semibold text-[var(--hv-shell-text-strong)]">{$t('vpn.connection_name')} <span class="text-[var(--hv-error)]">*</span></label>
      <input id="wg-client-name" class="w-full h-11 px-3 border border-[var(--hv-shell-border-strong)] rounded-lg bg-[var(--hv-shell-border-soft)] text-sm outline-none focus:border-[var(--hv-primary)]" type="text" bind:value={wgClientName} placeholder="my-provider" />
    </div>
  </div>
  <div class="mb-3">
    <div class="text-xs font-bold uppercase tracking-wide text-[var(--hv-shell-text-3)] mb-2">{$t('vpn.configuration_file')}</div>
    <div class="flex flex-col gap-1.5">
      <input type="file" accept=".conf,.txt" onchange={handleWgClientFileUpload} id="wg-client-file" class="sr-only" />
      <label for="wg-client-file" class="flex items-center gap-2 h-11 px-4 border border-[var(--hv-shell-border-strong)] rounded-lg bg-[var(--hv-shell-fill)] text-c-label font-semibold text-[var(--hv-shell-text-strong)] cursor-pointer hover:bg-[var(--hv-shell-border)]"><IconUpload size={18} /> {wgClientFile ? wgClientFile.name : $t('vpn.choose_conf_file')}</label>
      <label for="wg-client-config" class="text-c-label font-semibold text-[var(--hv-shell-text-strong)] mt-2">{$t('vpn.wg_client_paste_label')}</label>
      <textarea id="wg-client-config" rows="7" class="w-full px-3 py-2 border border-[var(--hv-shell-border-strong)] rounded-lg bg-[var(--hv-shell-border-soft)] text-xs font-[family-name:var(--hv-font-mono)] outline-none focus:border-[var(--hv-primary)]" bind:value={wgClientConfig} placeholder="[Interface]&#10;PrivateKey = …&#10;Address = …"></textarea>
    </div>
  </div>
  <div class="flex flex-col gap-2">
    <label for="wg-client-kill-switch" class="flex items-start gap-2 min-h-11 text-sm text-[var(--hv-shell-text-strong)] cursor-pointer">
      <input id="wg-client-kill-switch" type="checkbox" bind:checked={wgClientKillSwitch} class="mt-1 w-4 h-4 accent-[var(--hv-primary)]" />
      <span>{$t('vpn.wg_client_kill_switch')}</span>
    </label>
    <label for="wg-client-autostart" class="flex items-start gap-2 min-h-11 text-sm text-[var(--hv-shell-text-strong)] cursor-pointer">
      <input id="wg-client-autostart" type="checkbox" bind:checked={wgClientAutostart} class="mt-1 w-4 h-4 accent-[var(--hv-primary)]" />
      <span>{$t('vpn.wg_client_autostart')}</span>
    </label>
  </div>
  {#snippet footer()}
    <div class="flex gap-2.5 justify-end">
      <Button variant="ghost" onclick={() => { showWgClientModal = false; resetWgClientForm(); }} disabled={importingWgClient}>{$t('vpn.cancel')}</Button>
      <Button variant="primary" onclick={importWireGuardClient} disabled={importingWgClient || !wgClientName.trim() || !wgClientConfig.trim()}>{importingWgClient ? $t('vpn.wg_client_importing') : $t('vpn.wg_client_import')}</Button>
    </div>
  {/snippet}
</Modal>
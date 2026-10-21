<!-- ========================================================================
     File          : Docker.svelte
     Author        : Nicolas Le Roux
     Created       : 28/09/2026
     Last modified : 21/10/2026 by Nicolas Le Roux
     Version       : 1.0.0

     Description:
     The Docker app: container list with live status refresh, lifecycle
     actions (start/stop/restart/remove), logs, image pull/remove, network
     inspection, port checks and per-container resource stats.

     Dependencies:
       - svelte (>= 5), theme primitives, app/ composites, utils
         (autoRevalidate), stores/i18n

     Usage:
     Opened as a desktop window app (admin only).
     ======================================================================== -->
<script lang="ts">
  import { IconSearch, IconX, IconRefresh, IconTrash, IconPlayerPlay, IconPlayerPause, IconSquare, IconDownload, IconCheck, IconCopy, IconPhoto as ImageIcon, IconAlertTriangle, IconContainer, IconPlus, IconTerminal, IconExternalLink, IconDeviceTv, IconCloud, IconShield, IconLock, IconHome, IconGlobe, IconDatabase, IconActivity, IconPackage, IconPackages, IconServer, IconNetwork, IconKey, IconShieldOff, IconGitBranch, IconRss, IconTextScan2, IconBook, IconLayoutDashboard, IconMusic, IconListCheck, IconPipeline, IconBoxMultiple, IconBox, IconPencil, IconStar, IconInfoCircle, IconTool, IconSettings, IconBolt } from '@tabler/icons-svelte';
  import { t, translate } from '$lib/stores/i18n';
  import { onAppVisible } from '$lib/utils/autoRevalidate';
  import { Button, Toggle, Modal, Alert } from '$lib/components/theme';
  import { PageLayout, ConfirmDialog, iconBtn, actionBtn, actionIconBtn, dangerHover, textInput, EmptyState } from '$lib/components/app';
  import { createConfirmState } from '$lib/utils/confirmDialog.svelte';
  import { cn } from '$lib/utils.js';
  import PathField from '$lib/components/theme/PathField.svelte';
  import WindowHeader from '$lib/components/desktop/WindowHeader.svelte';
  import WindowSidebarItem from '$lib/components/desktop/WindowSidebarItem.svelte';
  import WindowSidebarSection from '$lib/components/desktop/WindowSidebarSection.svelte';

  // ── Types matching the real backend envelope ──────────────────────────────
  interface ApiEnvelope<T = any> {
    success: boolean;
    data?: T;
    message?: string;
    error?: string;
  }

  interface ContainerPort {
    host_port: number;
    container_port: number;
    protocol: string;
  }
  interface Container {
    id: string;
    name: string;
    image: string;
    status: string;
    state: 'Running' | 'Exited' | 'Paused' | 'Restarting' | 'Dead' | 'Created' | 'Unknown';
    created: string;
    ports: ContainerPort[];
    memory_mb: number | null;
    cpu_percent: number | null;
    net_rx_bytes: number | null;
    net_tx_bytes: number | null;
    blk_read_bytes: number | null;
    blk_write_bytes: number | null;
    update_available: boolean;
    compose_project: string | null;
    networks: string[];
  }
  interface Image {
    id: string;
    repository: string;
    tag: string;
    size: string;
    created: string;
  }
  interface DockerNetwork {
    id: string;
    name: string;
    driver: string;
    scope: string;
    subnet: string | null;
    gateway: string | null;
    internal: boolean;
    containers: string[];
  }
  interface HubResult {
    name: string;
    description: string;
    star_count: number;
    pull_count: number;
    is_official: boolean;
  }
  interface ComposeProject {
    name: string;
    config_path: string;
    services: string[];
    status: string;
  }
  interface DockerVolume {
    name: string;
    driver: string;
    scope: string;
    mountpoint: string;
    in_use: boolean;
    used_by: string[];
  }
  interface DiskUsageRow {
    kind: string;
    total: number;
    active: number;
    size: string;
    reclaimable: string;
  }
  interface DiskUsage {
    rows: DiskUsageRow[];
    total_size: string;
    total_reclaimable: string;
  }
  interface PruneReport {
    target: string;
    reclaimed: string;
    message: string;
  }
  interface ContainerMount {
    kind: string;
    source: string;
    destination: string;
    read_only: boolean;
    name: string | null;
  }
  interface ContainerDetail {
    id: string;
    name: string;
    image: string;
    image_id: string;
    image_digest: string | null;
    created: string;
    state: string;
    status: string;
    exit_code: number | null;
    started_at: string | null;
    finished_at: string | null;
    restart_count: number;
    pid: number | null;
    command: string[];
    entrypoint: string[];
    working_dir: string;
    hostname: string;
    user: string;
    restart_policy: string;
    privileged: boolean;
    memory_limit: number | null;
    nano_cpus: number | null;
    env: string[];
    labels: [string, string][];
    mounts: ContainerMount[];
    ports: ContainerPort[];
    networks: string[];
    ip_address: string | null;
    health: string | null;
    log_driver: string | null;
    log_path: string | null;
    compose_project: string | null;
    compose_service: string | null;
  }
  interface ExecResult {
    stdout: string;
    stderr: string;
    exit_code: number;
  }
  interface CatalogApp {
    /** Namespaced id, "<store>:<app>" — used in every catalog API path. */
    id: string;
    /** Bare app id, for the Lucide-icon fallback lookup. */
    app_id: string;
    name: string;
    description: string;
    tagline: string | null;
    category: string;
    icon: string | null;
    has_icon: boolean;
    gallery_count: number;
    default_port: number;
    version: string;
    developer: string | null;
    website: string | null;
    repo: string | null;
    dependencies: string[];
    implements: string[];
    environment: EnvVarDecl[];
    missing_dependencies: string[];
    incompatibility_reason: string | null;
    store_id: string;
    store_name: string;
    installed: boolean;
    running: boolean;
    installed_port: number | null;
    proxy_port: number | null;
    proxy_auth_required: boolean;
    unsupported_on_host: boolean;
  }
  interface AppStore {
    id: string;
    name: string;
    git_url: string;
    enabled: boolean;
    last_synced_at: number | null;
    last_error: string | null;
    app_count: number;
  }
  interface CreatePortRow { host_port: string; container_port: string; protocol: string; }
  interface CreateEnvRow { key: string; value: string; }
  interface CreateVolumeRow { host_path: string; container_path: string; read_only: boolean; }
  interface EnvVarDecl { name: string; services: string[]; default: string | null; options: string[]; required: boolean; note: string | null; }
  interface InstallReviewDep { dependency: string; satisfied_by: string[]; providers: string[]; selected: string | null; }
  interface InstallReview { id: string; name: string; version: string; incompatibility_reason: string | null; dependencies: InstallReviewDep[]; environment: EnvVarDecl[]; default_port: number; }

  // ── Local authed fetch (keeps session + CSRF tokens in sync with api client) ─
  async function authedFetch<T = any>(method: string, endpoint: string, body?: any): Promise<ApiEnvelope<T>> {
    const headers: Record<string, string> = { 'Content-Type': 'application/json' };
    const st = localStorage.getItem('session-token');
    const ct = localStorage.getItem('csrf-token');
    if (st) headers['X-Session-Token'] = st;
    if (ct) headers['X-CSRF-Token'] = ct;
    try {
      const res = await fetch('/api' + endpoint, {
        method,
        headers,
        body: body !== undefined ? JSON.stringify(body) : undefined,
      });
      return await res.json();
    } catch (e) {
      return { success: false, error: e instanceof Error ? e.message : 'Network error' };
    }
  }

  // ── Image -> icon glyph (the shape varies per known app so the grid is
  //    scannable; the tint is always the app accent — one colour everywhere) ──
  const ICON_RULES: [string, any][] = [
    ['plex', IconDeviceTv], ['jellyfin', IconDeviceTv], ['emby', IconDeviceTv],
    ['nextcloud', IconCloud], ['owncloud', IconCloud],
    ['pihole', IconShield], ['adguard', IconShield],
    ['vaultwarden', IconLock], ['bitwarden', IconLock],
    ['homeassistant', IconHome], ['home-assistant', IconHome],
    ['nginx', IconGlobe], ['traefik', IconGlobe], ['caddy', IconGlobe],
    ['postgres', IconDatabase], ['mysql', IconDatabase], ['mariadb', IconDatabase], ['mongo', IconDatabase], ['redis', IconDatabase],
    ['grafana', IconActivity], ['prometheus', IconActivity], ['uptime', IconActivity],
    ['portainer', IconBoxMultiple],
    ['syncthing', IconServer],
    ['sonarr', IconDeviceTv], ['radarr', IconDeviceTv],
  ];
  function iconFor(image: string): { icon: any } {
    const name = image.toLowerCase();
    for (const [key, icon] of ICON_RULES) {
      if (name.includes(key)) return { icon };
    }
    return { icon: IconBox };
  }

  // ── View / data state ─────────────────────────────────────────────────────
  type Tab = 'containers' | 'images' | 'volumes' | 'networks' | 'discover' | 'catalog' | 'compose';
  let tab = $state<Tab>('containers');
  let dockerAvailable = $state<boolean | null>(null);
  let busy = $state(false);
  let banner = $state<{ kind: 'error' | 'ok'; text: string } | null>(null);

  let containers = $state<Container[]>([]);
  let images = $state<Image[]>([]);
  let networks = $state<DockerNetwork[]>([]);
  let projects = $state<ComposeProject[]>([]);
  let hubResults = $state<HubResult[]>([]);
  let hubQuery = $state('');
  let hubLoading = $state(false);
  const dlg = createConfirmState();

  // ── Docker app catalog (one-click templates) ──────────────────────────────
  let catalogApps = $state<CatalogApp[]>([]);
  let catalogLoading = $state(false);
  let catalogBusyId = $state<string | null>(null);
  let catalogPortModal = $state<{
    id: string;
    name: string;
    port: string;
    review: InstallReview | null;
    reviewLoading: boolean;
    alternatives: Record<string, string>;
    envValues: Record<string, string>;
  } | null>(null);
  let updateAllBusy = $state(false);
  let appLogsModal = $state<{ id: string; name: string; logs: string; loading: boolean } | null>(null);

  const CATALOG_ICONS: Record<string, any> = {
    movie: IconDeviceTv, cloud: IconCloud, image: ImageIcon,
    'key-round': IconKey, 'shield-ban': IconShieldOff, activity: IconActivity, 'git-branch': IconGitBranch,
    rss: IconRss, 'scan-text': IconTextScan2, 'book-open': IconBook, house: IconHome,
    'layout-dashboard': IconLayoutDashboard, music: IconMusic, 'list-checks': IconListCheck, workflow: IconPipeline,
  };
  function catalogIconFor(icon: string | null): any {
    return (icon && CATALOG_ICONS[icon]) || IconPackage;
  }

  // The manifest carries the description in every UI language
  // (description / description_fr / _de / _es); the backend already picks
  // the active locale and sends the resolved string, so just use it.
  function catalogDescFor(app: CatalogApp): string {
    return app.description;
  }

  // ── Community app stores (extra git-repo catalog sources) ─────────────────
  let appStores = $state<AppStore[]>([]);
  let storesLoading = $state(false);
  let storeUrl = $state('');
  let storeBusy = $state(false);
  let showStoresPanel = $state(false);

  async function loadStores() {
    storesLoading = true;
    const res = await authedFetch<AppStore[]>('GET', '/docker/stores');
    storesLoading = false;
    if (res.success && res.data) appStores = res.data;
  }

  async function addStore() {
    const url = storeUrl.trim();
    if (!url) return;
    storeBusy = true;
    const res = await authedFetch('POST', '/docker/stores', { git_url: url });
    storeBusy = false;
    if (res.success) {
      storeUrl = '';
      setBanner('ok', translate('docker.stores_added'));
      await loadStores();
      await loadCatalog();
    } else {
      setBanner('error', res.message || res.error || translate('docker.stores_add_failed'));
    }
  }

  async function syncStore(store: AppStore) {
    storeBusy = true;
    const res = await authedFetch('POST', `/docker/stores/${encodeURIComponent(store.id)}/sync`);
    storeBusy = false;
    if (res.success) setBanner('ok', translate('docker.stores_synced'));
    else setBanner('error', res.message || res.error || translate('docker.stores_sync_failed'));
    await loadStores();
    await loadCatalog();
  }

  async function removeStore(store: AppStore) {
    if (!(await dlg.ask(translate('docker.stores_remove_confirm', { name: store.name })))) return;
    storeBusy = true;
    const res = await authedFetch('DELETE', `/docker/stores/${encodeURIComponent(store.id)}`);
    storeBusy = false;
    if (res.success) setBanner('ok', translate('docker.stores_removed'));
    else setBanner('error', res.message || res.error || translate('docker.stores_sync_failed'));
    await loadStores();
    await loadCatalog();
  }

  // ── Container logs (live) ─────────────────────────────────────────────────
  let containerLogs = $state<string[]>([]);
  let logContainerName = $state<string | null>(null);
  let logTail = $state(200);
  let logTimestamps = $state(false);
  let logFollow = $state(true);
  let logStream: EventSource | null = null;
  let logState = $state<'idle' | 'connecting' | 'live' | 'ended' | 'error'>('idle');
  let logError = $state('');
  let logBox = $state<HTMLElement | null>(null);
  const LOG_TAIL_CHOICES = [100, 200, 500, 1000, 5000];

  // Which container's "Open" button has its new-tab / new-window menu open.
  let urlMenuFor = $state<string | null>(null);

  /** Where the open popover menu is drawn (viewport coordinates). */
  let menuPos = $state<{ top: number; left: number } | null>(null);

  /** Anchors a menu to its trigger, in viewport coordinates.
   *  The menus are `position: fixed` rather than `absolute` because the
   *  container card is `overflow-hidden` (it rounds its own corners): an
   *  absolutely-positioned menu is clipped away by the card — invisible and
   *  unclickable — as soon as the card is shorter than the menu. */
  function menuAnchor(el: HTMLElement, width: number, height: number) {
    const r = el.getBoundingClientRect();
    const left = Math.max(8, Math.min(r.right - width, window.innerWidth - width - 8));
    let top = r.bottom + 4;
    if (top + height > window.innerHeight - 8) top = Math.max(8, r.top - height - 4);
    return { top, left };
  }

  /** Escape closes an open popover menu, like every other overlay.
   *  A window listener (rather than a handler on the menu itself) is what
   *  makes it work while focus is still on the trigger button. A scroll or a
   *  resize closes it too: the menu is fixed-positioned, so it would
   *  otherwise drift away from the button it belongs to. */
  $effect(() => {
    if (urlMenuFor === null && ctrMenuFor === null) return;
    const close = () => {
      urlMenuFor = null;
      ctrMenuFor = null;
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') close();
    };
    window.addEventListener('keydown', onKey);
    window.addEventListener('resize', close);
    window.addEventListener('scroll', close, true);
    return () => {
      window.removeEventListener('keydown', onKey);
      window.removeEventListener('resize', close);
      window.removeEventListener('scroll', close, true);
    };
  });

  /** Open a container's web UI, letting the user pick a new tab or a real
   *  new window (a size+position feature string is what makes the browser
   *  spawn a window rather than a tab). */
  function openContainerUrl(url: string, mode: 'tab' | 'window') {
    urlMenuFor = null;
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

  // ── Host-port conflict check (create-container + catalog-install forms) ────
  type PortCheck = { available: boolean; used_by: string | null; detail: string | null };
  // Keyed by the port string; value 'checking' while the request is in flight.
  let portChecks = $state<Record<string, PortCheck | 'checking'>>({});
  let portDebounce: ReturnType<typeof setTimeout> | undefined;

  function isValidPort(portStr: string): boolean {
    const n = Number(portStr);
    return /^\d+$/.test(portStr.trim()) && Number.isInteger(n) && n >= 1 && n <= 65535;
  }

  async function checkPortValue(portStr: string) {
    const v = portStr.trim();
    if (!isValidPort(v) || portChecks[v]) return;
    portChecks = { ...portChecks, [v]: 'checking' };
    const res = await authedFetch<PortCheck>('GET', `/docker/port-check?port=${Number(v)}`);
    const result: PortCheck = res.success && res.data
      ? res.data
      : { available: true, used_by: null, detail: null };
    portChecks = { ...portChecks, [v]: result };
  }

  function onPortInput(portStr: string) {
    clearTimeout(portDebounce);
    portDebounce = setTimeout(() => checkPortValue(portStr), 400);
  }

  type PortHint = { kind: 'ok' | 'bad' | 'checking' | 'none'; text: string };
  function portHint(portStr: string, siblings: string[] = []): PortHint {
    const v = (portStr ?? '').trim();
    if (!v) return { kind: 'none', text: '' };
    if (!isValidPort(v)) return { kind: 'bad', text: translate('docker.port_invalid') };
    if (siblings.filter((s) => s.trim() === v).length > 1) {
      return { kind: 'bad', text: translate('docker.port_duplicate') };
    }
    const r = portChecks[v];
    if (r === 'checking') return { kind: 'checking', text: translate('docker.port_checking') };
    if (!r) return { kind: 'none', text: '' };
    if (r.available) return { kind: 'ok', text: translate('docker.port_available') };
    if (r.used_by === 'container') {
      return { kind: 'bad', text: translate('docker.port_used_container', { name: r.detail ?? '?' }) };
    }
    if (r.used_by === 'homevault') return { kind: 'bad', text: translate('docker.port_used_homevault') };
    return { kind: 'bad', text: translate('docker.port_used_host') };
  }

  // `true` when any filled host-port field in the create form is invalid,
  // duplicated, or already taken — used to block the Create button.
  function createPortsHaveConflict(): boolean {
    const hosts = cPorts.map((p) => p.host_port);
    return cPorts.some((p) => p.host_port.trim() && portHint(p.host_port, hosts).kind === 'bad');
  }

  // ── Create container modal ────────────────────────────────────────────────
  let modalOpen = $state(false);
  let modalImagePreset = $state('');
  let cName = $state('');
  let cImage = $state('');
  let cPorts = $state<CreatePortRow[]>([{ host_port: '', container_port: '', protocol: 'tcp' }]);
  let cVolumes = $state<CreateVolumeRow[]>([{ host_path: '', container_path: '', read_only: false }]);
  let cEnv = $state<CreateEnvRow[]>([{ key: '', value: '' }]);
  let cRestart = $state('unless-stopped');
  let cMemory = $state('');
  let cCpu = $state('');
  let cPrivileged = $state(false);
  let cAutoRemove = $state(false);
  let cNetwork = $state('');
  let modalBusy = $state(false);

  // ── Network create modal ──────────────────────────────────────────────────
  let netModalOpen = $state(false);
  let pullModalOpen = $state(false);
  let pullImageName = $state('');
  let nName = $state('');
  let nDriver = $state('bridge');
  let nSubnet = $state('');
  let nGateway = $state('');
  let nInternal = $state(false);

  // ── Compose ───────────────────────────────────────────────────────────────
  let composeName = $state('');
  let composeYaml = $state('');
  let composeStatus = $state<{ text: string; color: string } | null>(null);
  let composeDeploying = $state(false);
  let composeValidating = $state(false);
  let composeAvailable = $state<boolean | null>(null);

  // ── Daemon-down recovery (start daemon / view daemon logs) ───────────────
  let daemonStarting = $state(false);
  let daemonLogs = $state('');
  let daemonLogsOpen = $state(false);

  // ── Containers filter (client-side: search + status chips) ──────────────
  let cSearch = $state('');
  let cFilter = $state<'all' | 'running' | 'stopped'>('all');
  let filteredContainers = $derived(
    containers.filter((c) => {
      if (cFilter === 'running' && c.state !== 'Running') return false;
      if (cFilter === 'stopped' && c.state === 'Running') return false;
      const q = cSearch.trim().toLowerCase();
      if (q && !`${c.name} ${c.image}`.toLowerCase().includes(q)) return false;
      return true;
    })
  );

  // Compact SI byte formatter for the container I/O chips.
  function fmtBytes(n: number | null): string {
    if (n === null || n === undefined) return '—';
    if (n < 1000) return `${n} B`;
    const units = ['kB', 'MB', 'GB', 'TB'];
    let v = n / 1000;
    let i = 0;
    while (v >= 1000 && i < units.length - 1) { v /= 1000; i++; }
    return `${v < 10 ? v.toFixed(1) : Math.round(v)} ${units[i]}`;
  }

  // ── Derived ───────────────────────────────────────────────────────────────
  let runningCount = $derived(containers.filter(c => c.state === 'Running').length);
  let totalCpu = $derived(Math.round(containers.reduce((s, c) => s + (c.cpu_percent ?? 0), 0)));
  let totalMemGb = $derived(containers.reduce((s, c) => s + (c.memory_mb ?? 0), 0) / 1024);

  const TAB_DEFS: { id: Tab; label: string }[] = [
    { id: 'containers', label: 'docker.tab_containers' },
    { id: 'images', label: 'docker.tab_images' },
    { id: 'volumes', label: 'docker.tab_volumes' },
    { id: 'networks', label: 'docker.tab_networks' },
    { id: 'discover', label: 'docker.tab_discover' },
    { id: 'catalog', label: 'docker.tab_catalog' },
    { id: 'compose', label: 'docker.tab_compose' },
  ];
  function tabCount(id: Tab): number | null {
    switch (id) {
      case 'containers': return containers.length || null;
      case 'images': return images.length || null;
      case 'volumes': return volumes.length || null;
      case 'networks': return networks.length || null;
      case 'catalog': return catalogApps.length || null;
      case 'compose': return projects.length || null;
      default: return null;
    }
  }

  function dismissBanner() { banner = null; }
  function setBanner(kind: 'error' | 'ok', text: string) {
    banner = { kind, text };
    if (kind === 'ok') setTimeout(dismissBanner, 4000);
  }

  function copyText(text: string) {
    navigator.clipboard?.writeText(text).then(
      () => setBanner('ok', translate('docker.url_copied')),
      () => {}
    );
  }

  function refreshCurrent() {
    if (tab === 'containers') loadContainers();
    else if (tab === 'images') loadImages();
    else if (tab === 'volumes') { loadVolumes(); loadDiskUsage(); }
    else if (tab === 'networks') loadNetworks();
    else if (tab === 'compose') loadProjects();
    else if (tab === 'catalog') { loadCatalog(); loadStores(); loadCatalogUpdates(); }
    else init();
  }

  // ── Loaders ───────────────────────────────────────────────────────────────
  async function init() {
    // Fire the availability probe and the container list in parallel — the
    // availability check (`docker info`) is a separate subprocess round-trip,
    // so awaiting it first would add its latency to the critical path before
    // any container data could render. The list is fetched without stats
    // (`?stats=false`) so it renders instantly; `fillContainerStats` fills the
    // CPU/memory in afterwards.
    const [avail, containersRes] = await Promise.all([
      authedFetch<{ available: boolean }>('GET', '/docker/available'),
      authedFetch<Container[]>('GET', '/docker/containers?stats=false'),
    ]);
    dockerAvailable = avail.success && !!avail.data?.available;
    if (!dockerAvailable) return;
    if (containersRes.success && containersRes.data) containers = containersRes.data;
    else setBanner('error', containersRes.message || containersRes.error || translate('docker.load_containers_failed'));
    fillContainerStats();
  }

  async function loadContainers() {
    busy = true;
    const res = await authedFetch<Container[]>('GET', '/docker/containers?stats=false');
    busy = false;
    if (res.success && res.data) containers = res.data;
    else setBanner('error', res.message || res.error || translate('docker.load_containers_failed'));
    fillContainerStats();
  }

  // Fetch the live CPU/memory stats (`docker stats --no-stream`, the slow
  // part) in the background and merge them into the already-rendered list, so
  // the containers appear immediately and the metrics catch up a moment later.
  async function fillContainerStats() {
    const res = await authedFetch<Container[]>('GET', '/docker/containers?stats=true');
    if (!res.success || !res.data) return;
    const byId = new Map(res.data.map((c) => [c.id, c]));
    containers = containers.map((c) => {
      const s = byId.get(c.id);
      return s
        ? {
            ...c,
            memory_mb: s.memory_mb,
            cpu_percent: s.cpu_percent,
            net_rx_bytes: s.net_rx_bytes,
            net_tx_bytes: s.net_tx_bytes,
            blk_read_bytes: s.blk_read_bytes,
            blk_write_bytes: s.blk_write_bytes,
          }
        : c;
    });
  }

  async function loadImages() {
    busy = true;
    const res = await authedFetch<Image[]>('GET', '/docker/images');
    busy = false;
    if (res.success && res.data) images = res.data;
    else setBanner('error', res.message || res.error || translate('docker.load_images_failed'));
  }

  async function loadNetworks() {
    busy = true;
    const res = await authedFetch<DockerNetwork[]>('GET', '/docker/networks');
    busy = false;
    if (res.success && res.data) networks = res.data;
    else setBanner('error', res.message || res.error || translate('docker.load_networks_failed'));
  }

  async function loadProjects() {
    busy = true;
    const res = await authedFetch<ComposeProject[]>('GET', '/docker/compose/projects');
    busy = false;
    if (res.success && res.data) projects = res.data;
    else setBanner('error', res.message || res.error || translate('docker.load_projects_failed'));
  }

  async function loadComposeAvailable() {
    const res = await authedFetch<{ available: boolean }>('GET', '/docker/compose/available');
    composeAvailable = res.success && !!res.data?.available;
  }

  async function loadCatalog() {
    catalogLoading = true;
    const res = await authedFetch<CatalogApp[]>('GET', '/docker/catalog');
    catalogLoading = false;
    if (res.success && res.data) catalogApps = res.data;
    else setBanner('error', res.message || res.error || translate('docker.catalog_load_failed'));
  }

  // ── App updates (image digest / package version) ─────────────────────────
  interface AppUpdateStatus {
    id: string;
    name: string;
    installed_version: string | null;
    available_version: string;
    package_update_available: boolean;
    image_update_available: boolean;
  }
  let catalogUpdates = $state<Record<string, AppUpdateStatus>>({});
  let updatesChecking = $state(false);
  let updateBusyId = $state<string | null>(null);

  async function loadCatalogUpdates() {
    updatesChecking = true;
    const res = await authedFetch<AppUpdateStatus[]>('GET', '/docker/catalog/updates');
    updatesChecking = false;
    if (res.success && res.data) {
      const map: Record<string, AppUpdateStatus> = {};
      for (const u of res.data) map[u.id] = u;
      catalogUpdates = map;
    }
  }

  // The URL the "Open" button points at: the per-app reverse proxy when it
  // is up, else the app's own published port. Always http:// — the proxy
  // listener is plain HTTP even under an HTTPS dashboard, and opening it in
  // a new tab is a top-level navigation (allowed from HTTPS pages), not an
  // embedded subresource, so it is never blocked as mixed content.
  function appOpenUrl(app: CatalogApp): string | null {
    const port = app.proxy_port ?? app.installed_port;
    return port ? `http://${location.hostname}:${port}` : null;
  }

  async function toggleAppProxyAuth(app: CatalogApp) {
    const res = await authedFetch('POST', `/docker/catalog/${encodeURIComponent(app.id)}/proxy`, {
      auth_required: !app.proxy_auth_required,
    });
    if (res.success) {
      await loadCatalog();
    } else {
      setBanner('error', res.message || res.error || translate('docker.catalog_update_failed'));
    }
  }

  async function updateCatalogApp(app: CatalogApp) {
    updateBusyId = app.id;
    const res = await authedFetch('POST', `/docker/catalog/${encodeURIComponent(app.id)}/update`);
    updateBusyId = null;
    if (res.success) {
      setBanner('ok', translate('docker.catalog_updated', { name: app.name }));
      delete catalogUpdates[app.id];
      catalogUpdates = { ...catalogUpdates };
      await loadContainers();
    } else {
      setBanner('error', res.message || res.error || translate('docker.catalog_update_failed'));
    }
  }

  // Grouped update queue: one call updates every app with a pending update,
  // reporting per-app outcomes so a single failure never hides the rest.
  async function updateAllCatalogApps() {
    updateAllBusy = true;
    const res = await authedFetch<{ updated: string[]; failed: { id: string; error: string }[] }>('POST', '/docker/catalog/update-all');
    updateAllBusy = false;
    if (res.success && res.data) {
      const ok = res.data.updated.length;
      const fail = res.data.failed.length;
      setBanner(fail === 0 ? 'ok' : 'error', translate('docker.catalog_update_all_result', { ok: String(ok), fail: String(fail) }));
      await loadCatalog();
      await loadCatalogUpdates();
      await loadContainers();
    } else {
      setBanner('error', res.message || res.error || translate('docker.catalog_update_failed'));
    }
  }

  // Per-app compose logs viewer (same Modal pattern as container logs).
  async function openAppLogs(app: CatalogApp) {
    appLogsModal = { id: app.id, name: app.name, logs: '', loading: true };
    const res = await authedFetch<{ logs: string }>('GET', `/docker/catalog/${encodeURIComponent(app.id)}/logs?tail=200`);
    if (!appLogsModal || appLogsModal.id !== app.id) return;
    if (res.success && res.data) appLogsModal.logs = res.data.logs;
    else {
      appLogsModal.logs = '';
      setBanner('error', res.message || res.error || translate('docker.catalog_logs_failed'));
    }
    appLogsModal.loading = false;
  }

  // Client-side search across the merged catalog (built-in + stores).
  let catalogSearch = $state('');
  let filteredCatalog = $derived(
    catalogSearch.trim()
      ? catalogApps.filter((a) => {
          const q = catalogSearch.trim().toLowerCase();
          return (
            a.name.toLowerCase().includes(q) ||
            a.description.toLowerCase().includes(q) ||
            a.category.toLowerCase().includes(q) ||
            a.store_name.toLowerCase().includes(q)
          );
        })
      : catalogApps,
  );

  function openCatalogInstall(app: CatalogApp) {
    portChecks = {};
    catalogPortModal = {
      id: app.id,
      name: app.name,
      port: String(app.default_port),
      review: null,
      reviewLoading: true,
      alternatives: {},
      envValues: {},
    };
    checkPortValue(String(app.default_port));
    loadInstallReview(app.id);
  }

  // Pre-install review (dependencies with alternatives, declared settings,
  // compatibility) so the dialog offers choices before the image pull.
  async function loadInstallReview(id: string) {
    const res = await authedFetch<InstallReview>('GET', `/docker/catalog/${encodeURIComponent(id)}/install-review`);
    if (!catalogPortModal || catalogPortModal.id !== id) return;
    if (res.success && res.data) {
      const review = res.data;
      const alternatives: Record<string, string> = {};
      for (const dep of review.dependencies) {
        if (dep.satisfied_by.length === 0 && dep.selected) alternatives[dep.dependency] = dep.selected;
      }
      const envValues: Record<string, string> = {};
      for (const decl of review.environment) {
        if (decl.default) envValues[decl.name] = decl.default;
      }
      catalogPortModal.review = review;
      catalogPortModal.alternatives = alternatives;
      catalogPortModal.envValues = envValues;
    }
    catalogPortModal.reviewLoading = false;
  }

  async function confirmCatalogInstall() {
    if (!catalogPortModal) return;
    const { id, name, port, alternatives, envValues } = catalogPortModal;
    const parsedPort = parseInt(port, 10);
    catalogBusyId = id;
    const environment: Record<string, string> = {};
    for (const [k, v] of Object.entries(envValues)) {
      if (v.trim() !== '') environment[k] = v;
    }
    const res = await authedFetch('POST', `/docker/catalog/${encodeURIComponent(id)}/install`, {
      port: Number.isFinite(parsedPort) ? parsedPort : undefined,
      alternatives,
      environment,
    });
    catalogBusyId = null;
    catalogPortModal = null;
    if (res.success) setBanner('ok', translate('docker.catalog_installed', { name }));
    else setBanner('error', res.message || res.error || translate('docker.catalog_install_failed'));
    await loadCatalog();
    if (projects.length || tab === 'compose') await loadProjects();
    // The catalog app runs as one or more real containers — refresh that
    // list (and its tab badge) even though we're on the catalog tab.
    await loadContainers();
  }

  async function uninstallCatalogApp(app: CatalogApp) {
    if (!(await dlg.ask(translate('docker.catalog_uninstall_confirm', { name: app.name })))) return;
    catalogBusyId = app.id;
    const res = await authedFetch('DELETE', `/docker/catalog/${encodeURIComponent(app.id)}`);
    catalogBusyId = null;
    if (res.success) setBanner('ok', translate('docker.catalog_uninstalled', { name: app.name }));
    else setBanner('error', res.message || res.error || translate('docker.catalog_uninstall_failed'));
    await loadCatalog();
    if (projects.length || tab === 'compose') await loadProjects();
    await loadContainers();
  }

  // Arrow-key navigation across the left nav rail (WAI-ARIA tabs pattern).
  function onNavKeydown(e: KeyboardEvent, index: number) {
    const last = TAB_DEFS.length - 1;
    let next = -1;
    if (e.key === 'IconArrowDown' || e.key === 'IconArrowRight') next = index === last ? 0 : index + 1;
    else if (e.key === 'IconArrowUp' || e.key === 'IconArrowLeft') next = index === 0 ? last : index - 1;
    else if (e.key === 'Home') next = 0;
    else if (e.key === 'End') next = last;
    else return;
    e.preventDefault();
    switchTab(TAB_DEFS[next].id);
    const strip = (e.currentTarget as HTMLElement).parentElement;
    (strip?.children[next] as HTMLElement | undefined)?.focus();
  }

  async function switchTab(t: Tab) {
    tab = t;
    // Containers change from outside this tab (catalog / compose / Hub), so
    // always re-fetch when landing here rather than trusting a stale list.
    if (t === 'containers') loadContainers();
    if (t === 'images' && images.length === 0) loadImages();
    if (t === 'volumes') { loadVolumes(); loadDiskUsage(); }
    if (t === 'networks' && networks.length === 0) loadNetworks();
    if (t === 'compose' && projects.length === 0) loadProjects();
    if (t === 'compose' && composeAvailable === null) loadComposeAvailable();
    if (t === 'catalog' && catalogApps.length === 0) loadCatalog();
    if (t === 'catalog') { loadStores(); loadCatalogUpdates(); }
  }

  // ── Container actions (real) ──────────────────────────────────────────────
  async function containerAction(name: string, action: 'start' | 'stop' | 'restart') {
    busy = true;
    const res = await authedFetch('PUT', `/docker/containers/${encodeURIComponent(name)}/${action}`);
    busy = false;
    const actionKey = action === 'start' ? 'container_started' : action === 'stop' ? 'container_stopped' : 'container_restarted';
    if (res.success) { setBanner('ok', translate(`docker.${actionKey}`, { name })); }
    else setBanner('error', res.message || res.error || translate('docker.action_failed', { action }));
    await loadContainers();
    if (logContainerName === name) await openLogs(name);
  }

  async function removeContainer(name: string, force: boolean) {
    if (!(await dlg.ask(translate('docker.container_delete_confirm', { name }) + (force ? translate('docker.container_force') : '')))) return;
    busy = true;
    const res = await authedFetch('DELETE', `/docker/containers/${encodeURIComponent(name)}?force=${force}`);
    busy = false;
    if (res.success) setBanner('ok', translate('docker.container_deleted', { name }));
    else setBanner('error', res.message || res.error || translate('docker.container_delete_failed'));
    if (logContainerName === name) closeLogs();
    await loadContainers();
  }

  // ── Container logs: live tail over SSE ────────────────────────────────────
  // `EventSource` cannot send the `X-Session-Token` header, so the session is
  // exchanged for a single-use ticket first (same pattern as the notifications
  // watcher and the terminal). The session token never travels in a URL.
  async function openLogs(name: string) {
    logContainerName = name;
    containerLogs = [];
    await startLogStream();
  }

  async function startLogStream() {
    const name = logContainerName;
    if (!name) return;
    stopLogStream();
    logState = 'connecting';
    logError = '';
    const res = await authedFetch<{ ticket: string; url: string }>(
      'POST',
      `/docker/containers/${encodeURIComponent(name)}/logs/ticket`,
      { tail: logTail, follow: logFollow, timestamps: logTimestamps },
    );
    if (!res.success || !res.data?.url) {
      logState = 'error';
      logError = res.message || res.error || translate('docker.logs_failed');
      return;
    }
    const es = new EventSource(res.data.url);
    logStream = es;
    es.addEventListener('log', (ev) => {
      const msg = ev as MessageEvent<string>;
      // Cap the buffer: a chatty container left on screen for an hour must not
      // grow the DOM without bound.
      containerLogs = [...containerLogs.slice(-4000), msg.data];
      logState = 'live';
    });
    es.addEventListener('end', () => {
      logState = logFollow ? 'ended' : 'live';
      stopLogStream();
    });
    es.onerror = () => {
      // A non-follow stream closes itself once the tail is delivered; only a
      // follow stream treats the close as a real error.
      logState = logFollow ? 'error' : 'live';
      if (logFollow) logError = translate('docker.logs_disconnected');
      stopLogStream();
    };
  }

  function stopLogStream() {
    if (logStream) { logStream.close(); logStream = null; }
  }

  function closeLogs() {
    stopLogStream();
    logContainerName = null;
    containerLogs = [];
    logState = 'idle';
    logError = '';
  }

  async function downloadLogs() {
    const blob = new Blob([containerLogs.join('\n')], { type: 'text/plain' });
    const url = URL.createObjectURL(blob);
    const a = document.createElement('a');
    a.href = url;
    a.download = `${logContainerName ?? 'container'}.log`;
    a.click();
    URL.revokeObjectURL(url);
  }

  // ── Edit a container's docker-compose.yml (containers tab) ────────────────
  // Only offered for containers that carry a `com.docker.compose.project`
  // label. The project must also be one HomeVault manages (created via the
  // Compose tab / the app catalog stores its files under compose_dir()); if
  // the GET below 404s we tell the user it can't be edited here.
  type EditCompose = {
    project: string;
    container: string;
    yaml: string;
    loading: boolean;
    busy: boolean;
    managed: boolean;
    status: { text: string; color: string } | null;
  };
  let editCompose = $state<EditCompose | null>(null);

  async function openEditCompose(c: Container) {
    if (!c.compose_project) return;
    editCompose = {
      project: c.compose_project,
      container: c.name,
      yaml: '',
      loading: true,
      busy: false,
      managed: true,
      status: null,
    };
    const res = await authedFetch<{ config: string }>(
      'GET',
      `/docker/compose/projects/${encodeURIComponent(c.compose_project)}/config`,
    );
    if (!editCompose) return; // closed while loading
    if (res.success && res.data) {
      editCompose = { ...editCompose, yaml: res.data.config, loading: false };
    } else {
      editCompose = {
        ...editCompose,
        loading: false,
        managed: false,
        status: { text: translate('docker.edit_compose_not_managed'), color: 'var(--hv-error)' },
      };
    }
  }
  function closeEditCompose() { editCompose = null; }

  async function validateEditCompose() {
    if (!editCompose) return;
    editCompose = { ...editCompose, busy: true, status: { text: translate('docker.validating'), color: 'var(--hv-shell-text-2)' } };
    const res = await authedFetch('POST', '/docker/compose/validate', { config: editCompose.yaml });
    if (!editCompose) return;
    editCompose = {
      ...editCompose,
      busy: false,
      status: res.success
        ? { text: translate('docker.compose_valid'), color: 'var(--hv-success)' }
        : { text: res.message || res.error || translate('docker.compose_invalid'), color: 'var(--hv-error)' },
    };
  }

  async function saveEditCompose(redeploy: boolean) {
    if (!editCompose) return;
    const { project, yaml } = editCompose;
    editCompose = {
      ...editCompose,
      busy: true,
      status: { text: translate(redeploy ? 'docker.deploying' : 'docker.validating'), color: 'var(--hv-shell-text-2)' },
    };
    const put = await authedFetch('PUT', `/docker/compose/projects/${encodeURIComponent(project)}`, { config: yaml });
    if (!put.success) {
      if (editCompose) {
        editCompose = { ...editCompose, busy: false, status: { text: put.message || put.error || translate('docker.edit_compose_save_failed'), color: 'var(--hv-error)' } };
      }
      return;
    }
    if (redeploy) {
      const up = await authedFetch('POST', `/docker/compose/projects/${encodeURIComponent(project)}/restart`);
      if (!up.success) {
        if (editCompose) {
          editCompose = { ...editCompose, busy: false, status: { text: up.message || up.error || translate('docker.deploy_failed'), color: 'var(--hv-error)' } };
        }
        await loadContainers();
        return;
      }
    }
    setBanner('ok', translate(redeploy ? 'docker.edit_compose_redeployed' : 'docker.edit_compose_saved'));
    editCompose = null;
    await loadContainers();
  }

  // ── Image actions (real) ──────────────────────────────────────────────────
  function openPullModal() {
    pullImageName = '';
    pullModalOpen = true;
  }

  async function pullImage() {
    const image = pullImageName.trim();
    if (!image) return;
    pullModalOpen = false;
    const [repo, tag] = image.split(':');
    busy = true;
    const res = await authedFetch('POST', '/docker/images/pull', { image: repo, tag: tag || 'latest' });
    busy = false;
    if (res.success) setBanner('ok', translate('docker.image_pulled', { name: image }));
    else setBanner('error', res.message || res.error || translate('docker.pull_failed2'));
    await loadImages();
  }

  async function removeImage(id: string) {
    if (!(await dlg.ask(translate('docker.image_delete_confirm', { id })))) return;
    busy = true;
    const res = await authedFetch('DELETE', `/docker/images/${encodeURIComponent(id)}?force=true`, { force: true });
    busy = false;
    if (res.success) setBanner('ok', translate('docker.image_deleted'));
    else setBanner('error', res.message || res.error || translate('docker.container_delete_failed'));
    await loadImages();
  }

  // ── Network actions (real) ────────────────────────────────────────────────
  function openNetModal() {
    nName = ''; nDriver = 'bridge'; nSubnet = ''; nGateway = ''; nInternal = false;
    netModalOpen = true;
  }
  async function createNetwork() {
    if (!nName.trim()) { setBanner('error', translate('docker.network_name_required')); return; }
    busy = true;
    const res = await authedFetch('POST', '/docker/networks', {
      name: nName, driver: nDriver, subnet: nSubnet || null, gateway: nGateway || null, internal: nInternal,
    });
    busy = false;
    if (res.success) { setBanner('ok', translate('docker.network_created', { name: nName })); netModalOpen = false; }
    else setBanner('error', res.message || res.error || translate('docker.network_create_failed'));
    await loadNetworks();
  }
  async function removeNetwork(name: string) {
    if (!(await dlg.ask(translate('docker.network_delete_confirm', { name })))) return;
    busy = true;
    const res = await authedFetch('DELETE', `/docker/networks/${encodeURIComponent(name)}`);
    busy = false;
    if (res.success) setBanner('ok', translate('docker.network_deleted'));
    else setBanner('error', res.message || res.error || translate('docker.container_delete_failed'));
    await loadNetworks();
  }

  // ── Docker Hub search (real) ──────────────────────────────────────────────
  async function searchHub() {
    const q = hubQuery.trim();
    if (!q) { hubResults = []; return; }
    hubLoading = true;
    const res = await authedFetch<HubResult[]>('GET', `/docker/hub/search?q=${encodeURIComponent(q)}`);
    hubLoading = false;
    if (res.success && res.data) hubResults = res.data;
    else setBanner('error', res.message || res.error || translate('docker.hub_search_failed'));
  }

  // ── Create container modal (real) ─────────────────────────────────────────
  function openCreateModal(presetImage = '') {
    cName = '';
    cImage = presetImage;
    modalImagePreset = presetImage;
    cPorts = [{ host_port: '', container_port: '', protocol: 'tcp' }];
    cVolumes = [{ host_path: '', container_path: '', read_only: false }];
    cEnv = [{ key: '', value: '' }];
    cRestart = 'unless-stopped';
    cMemory = ''; cCpu = ''; cPrivileged = false; cAutoRemove = false; cNetwork = '';
    portChecks = {};
    modalOpen = true;
  }
  function closeModal() { modalOpen = false; }

  function addPort() { cPorts = [...cPorts, { host_port: '', container_port: '', protocol: 'tcp' }]; }
  function removePort(i: number) { cPorts = cPorts.filter((_, idx) => idx !== i); }
  function addVolume() { cVolumes = [...cVolumes, { host_path: '', container_path: '', read_only: false }]; }
  function removeVolume(i: number) { cVolumes = cVolumes.filter((_, idx) => idx !== i); }
  function addEnv() { cEnv = [...cEnv, { key: '', value: '' }]; }
  function removeEnv(i: number) { cEnv = cEnv.filter((_, idx) => idx !== i); }

  async function submitCreate() {
    if (!cName.trim()) { setBanner('error', translate('docker.container_name_required')); return; }
    if (!cImage.trim()) { setBanner('error', translate('docker.image_required')); return; }

    const ports = cPorts
      .filter(p => p.host_port && p.container_port)
      .map(p => ({ host_port: Number(p.host_port), container_port: Number(p.container_port), protocol: p.protocol }));
    const volumes = cVolumes
      .filter(v => v.host_path && v.container_path)
      .map(v => ({ host_path: v.host_path, container_path: v.container_path, read_only: v.read_only }));
    const env = cEnv.filter(e => e.key && e.value).map(e => ({ key: e.key, value: e.value }));

    modalBusy = true;
    const res = await authedFetch('POST', '/docker/containers', {
      name: cName,
      image: cImage,
      ports,
      volumes,
      env,
      network: cNetwork || null,
      restart_policy: cRestart,
      memory_limit: cMemory || null,
      cpu_limit: cCpu || null,
      privileged: cPrivileged,
      auto_remove: cAutoRemove,
    });
    modalBusy = false;
    if (res.success) { setBanner('ok', translate('docker.container_created', { name: cName })); closeModal(); await loadContainers(); }
    else setBanner('error', res.message || res.error || translate('docker.create_failed2'));
  }

  // ── Compose (real) ────────────────────────────────────────────────────────
  async function deployCompose() {
    if (composeAvailable === false) { composeStatus = { text: translate('docker.unavailable_desc'), color: 'var(--hv-error)' }; return; }
    if (!composeName.trim()) { composeStatus = { text: translate('docker.stack_name_required'), color: 'var(--hv-error)' }; return; }
    if (!composeYaml.trim()) { composeStatus = { text: translate('docker.paste_compose_first'), color: 'var(--hv-error)' }; return; }
    composeDeploying = true;
    composeStatus = { text: translate('docker.deploying'), color: 'var(--hv-shell-text-2)' };
    const res = await authedFetch('POST', '/docker/compose/projects', { name: composeName, config: composeYaml });
    composeDeploying = false;
    if (res.success) { composeStatus = { text: translate('docker.stack_deployed'), color: 'var(--hv-success)' }; composeName = ''; composeYaml = ''; await loadProjects(); await loadContainers(); }
    else composeStatus = { text: res.message || res.error || translate('docker.deploy_failed'), color: 'var(--hv-error)' };
  }

  // Validate the compose YAML without deploying it, so mistakes surface before
  // the Deploy button is pressed.
  async function validateCompose() {
    if (composeAvailable === false) { composeStatus = { text: translate('docker.unavailable_desc'), color: 'var(--hv-error)' }; return; }
    if (!composeYaml.trim()) { composeStatus = { text: translate('docker.paste_compose_first'), color: 'var(--hv-error)' }; return; }
    composeValidating = true;
    composeStatus = { text: translate('docker.validating'), color: 'var(--hv-shell-text-2)' };
    const res = await authedFetch('POST', '/docker/compose/validate', { config: composeYaml });
    composeValidating = false;
    if (res.success) composeStatus = { text: translate('docker.compose_valid'), color: 'var(--hv-success)' };
    else composeStatus = { text: res.message || res.error || translate('docker.compose_invalid'), color: 'var(--hv-error)' };
  }

  // ── Daemon-down recovery ──────────────────────────────────────────────────
  async function startDaemon() {
    daemonStarting = true;
    const res = await authedFetch('POST', '/docker/daemon/start');
    daemonStarting = false;
    if (res.success) { setBanner('ok', translate('docker.daemon_started')); await init(); }
    else setBanner('error', res.message || res.error || translate('docker.daemon_start_failed'));
  }

  async function openDaemonLogs() {
    daemonLogsOpen = true;
    daemonLogs = translate('docker.logs_loading');
    const res = await authedFetch<{ logs: string }>('GET', '/docker/daemon/logs');
    if (res.success && res.data) daemonLogs = res.data.logs || translate('docker.logs_empty');
    else daemonLogs = res.message || res.error || translate('docker.logs_failed');
  }
  function closeDaemonLogs() { daemonLogsOpen = false; daemonLogs = ''; }
  async function composeAction(name: string, action: 'start' | 'stop' | 'restart') {
    busy = true;
    const res = await authedFetch('POST', `/docker/compose/projects/${encodeURIComponent(name)}/${action}`);
    busy = false;
    const actionKey = action === 'start' ? 'stack_started' : action === 'stop' ? 'stack_stopped' : 'stack_restarted';
    if (res.success) setBanner('ok', translate(`docker.${actionKey}`, { name }));
    else setBanner('error', res.message || res.error || translate('docker.action_failed', { action }));
    await loadProjects();
    await loadContainers();
  }
  async function removeProject(name: string) {
    if (!(await dlg.ask(translate('docker.stack_delete_confirm', { name })))) return;
    busy = true;
    const res = await authedFetch('DELETE', `/docker/compose/projects/${encodeURIComponent(name)}`);
    busy = false;
    if (res.success) setBanner('ok', translate('docker.stack_deleted'));
    else setBanner('error', res.message || res.error || translate('docker.stack_delete_failed'));
    await loadProjects();
    await loadContainers();
  }

  // ── Volumes ───────────────────────────────────────────────────────────────
  let volumes = $state<DockerVolume[]>([]);
  let volumesLoading = $state(false);
  let volumeModalOpen = $state(false);
  let vName = $state('');
  let vDriver = $state('local');
  let volumeBusy = $state(false);

  async function loadVolumes() {
    volumesLoading = true;
    const res = await authedFetch<DockerVolume[]>('GET', '/docker/volumes');
    volumesLoading = false;
    if (res.success && res.data) volumes = res.data;
    else setBanner('error', res.message || res.error || translate('docker.load_volumes_failed'));
  }

  function openVolumeModal() {
    vName = '';
    vDriver = 'local';
    volumeModalOpen = true;
  }

  async function createVolume() {
    if (!vName.trim()) { setBanner('error', translate('docker.volume_name_required')); return; }
    volumeBusy = true;
    const res = await authedFetch('POST', '/docker/volumes', {
      name: vName.trim(),
      driver: vDriver || null,
    });
    volumeBusy = false;
    if (res.success) {
      setBanner('ok', translate('docker.volume_created', { name: vName.trim() }));
      volumeModalOpen = false;
      await loadVolumes();
    } else {
      setBanner('error', res.message || res.error || translate('docker.volume_create_failed'));
    }
  }

  async function deleteVolume(v: DockerVolume) {
    // A volume in use needs the explicit force confirmation: it detaches data
    // from a running app and cannot be undone.
    const key = v.in_use ? 'docker.volume_force_delete_confirm' : 'docker.volume_delete_confirm';
    if (!(await dlg.ask(translate(key, { name: v.name })))) return;
    volumeBusy = true;
    const res = await authedFetch('DELETE', `/docker/volumes/${encodeURIComponent(v.name)}?force=${v.in_use}`);
    volumeBusy = false;
    if (res.success) setBanner('ok', translate('docker.volume_deleted', { name: v.name }));
    else setBanner('error', res.message || res.error || translate('docker.volume_delete_failed'));
    await loadVolumes();
    await loadDiskUsage();
  }

  // ── Disk usage / prune ────────────────────────────────────────────────────
  let diskUsage = $state<DiskUsage | null>(null);
  let pruneBusy = $state<string | null>(null);

  async function loadDiskUsage() {
    const res = await authedFetch<DiskUsage>('GET', '/docker/system/df');
    if (res.success && res.data) diskUsage = res.data;
  }

  /// Localise a `docker system df` row kind ("Images", "Local Volumes", …).
  function diskKindLabel(kind: string): string {
    const map: Record<string, string> = {
      Images: 'docker.disk_images',
      Containers: 'docker.disk_containers',
      'Local Volumes': 'docker.disk_local_volumes',
      'Build Cache': 'docker.disk_build_cache',
    };
    const key = map[kind];
    return key ? translate(key) : kind;
  }

  async function prune(target: 'images' | 'volumes' | 'containers' | 'networks' | 'builders' | 'system', all = false) {
    const key = all ? 'docker.prune_all_confirm' : 'docker.prune_confirm';
    if (!(await dlg.ask(translate(key, { target: translate(`docker.prune_${target}`) })))) return;
    pruneBusy = target;
    const res = await authedFetch<PruneReport>('POST', '/docker/prune', { target, all });
    pruneBusy = null;
    if (res.success && res.data) {
      setBanner('ok', translate('docker.pruned', { target: translate(`docker.prune_${target}`), reclaimed: res.data.reclaimed || '0B' }));
    } else {
      setBanner('error', res.message || res.error || translate('docker.prune_failed'));
    }
    await loadDiskUsage();
    await loadImages();
    await loadVolumes();
    await loadContainers();
  }

  // ── Container extras: inspect, pause, kill, rename, limits, exec, update ──
  let ctrMenuFor = $state<string | null>(null);
  let detailModal = $state<{ name: string; detail: ContainerDetail | null; loading: boolean; error: string } | null>(null);
  let execModal = $state<{
    name: string;
    command: string;
    running: boolean;
    result: ExecResult | null;
    error: string;
  } | null>(null);
  let renameModal = $state<{ name: string; value: string; busy: boolean } | null>(null);
  let limitsModal = $state<{
    name: string;
    memory: string;
    cpus: string;
    restart: string;
    busy: boolean;
  } | null>(null);
  let recreateBusy = $state<string | null>(null);

  async function containerExtra(name: string, action: 'pause' | 'unpause') {
    busy = true;
    const res = await authedFetch('PUT', `/docker/containers/${encodeURIComponent(name)}/${action}`);
    busy = false;
    const key = action === 'pause' ? 'docker.container_paused' : 'docker.container_unpaused';
    if (res.success) setBanner('ok', translate(key, { name }));
    else setBanner('error', res.message || res.error || translate('docker.action_failed', { action }));
    await loadContainers();
  }

  async function killContainer(name: string) {
    if (!(await dlg.ask(translate('docker.kill_confirm', { name })))) return;
    busy = true;
    const res = await authedFetch('POST', `/docker/containers/${encodeURIComponent(name)}/kill`, { signal: 'SIGKILL' });
    busy = false;
    if (res.success) setBanner('ok', translate('docker.container_killed', { name }));
    else setBanner('error', res.message || res.error || translate('docker.action_failed', { action: 'kill' }));
    await loadContainers();
  }

  async function openInspect(name: string) {
    ctrMenuFor = null;
    detailModal = { name, detail: null, loading: true, error: '' };
    const res = await authedFetch<ContainerDetail>('GET', `/docker/containers/${encodeURIComponent(name)}/inspect`);
    if (!detailModal || detailModal.name !== name) return;
    if (res.success && res.data) {
      detailModal = { ...detailModal, detail: res.data, loading: false };
    } else {
      detailModal = { ...detailModal, loading: false, error: res.message || res.error || translate('docker.inspect_failed') };
    }
  }

  function openRename(name: string) {
    ctrMenuFor = null;
    renameModal = { name, value: name, busy: false };
  }

  async function submitRename() {
    if (!renameModal) return;
    const { name, value } = renameModal;
    if (!value.trim() || value.trim() === name) { renameModal = null; return; }
    renameModal = { ...renameModal, busy: true };
    const res = await authedFetch('PUT', `/docker/containers/${encodeURIComponent(name)}/rename`, { name: value.trim() });
    if (res.success) {
      setBanner('ok', translate('docker.container_renamed', { name: value.trim() }));
      renameModal = null;
      await loadContainers();
    } else {
      renameModal = { ...renameModal, busy: false };
      setBanner('error', res.message || res.error || translate('docker.action_failed', { action: 'rename' }));
    }
  }

  async function openLimits(name: string) {
    ctrMenuFor = null;
    limitsModal = { name, memory: '', cpus: '', restart: '', busy: false };
    const res = await authedFetch<ContainerDetail>('GET', `/docker/containers/${encodeURIComponent(name)}/inspect`);
    if (!limitsModal || limitsModal.name !== name || !res.success || !res.data) return;
    const d = res.data;
    limitsModal = {
      ...limitsModal,
      memory: d.memory_limit && d.memory_limit > 0 ? `${Math.round(d.memory_limit / 1024 / 1024)}m` : '',
      cpus: d.nano_cpus && d.nano_cpus > 0 ? (d.nano_cpus / 1e9).toFixed(2) : '',
      restart: d.restart_policy || '',
    };
  }

  async function submitLimits() {
    if (!limitsModal) return;
    const { name, memory, cpus, restart } = limitsModal;
    limitsModal = { ...limitsModal, busy: true };
    const res = await authedFetch('POST', `/docker/containers/${encodeURIComponent(name)}/update`, {
      memory_limit: memory.trim() || null,
      cpu_limit: cpus.trim() || null,
      restart_policy: restart || null,
    });
    if (res.success) {
      setBanner('ok', translate('docker.container_updated', { name }));
      limitsModal = null;
      await loadContainers();
    } else {
      limitsModal = { ...limitsModal, busy: false };
      setBanner('error', res.message || res.error || translate('docker.action_failed', { action: 'update' }));
    }
  }

  function openExec(name: string) {
    ctrMenuFor = null;
    execModal = { name, command: '', running: false, result: null, error: '' };
  }

  async function runExec() {
    if (!execModal) return;
    const argv = execModal.command.trim().split(/\s+/).filter(Boolean);
    if (argv.length === 0) return;
    execModal = { ...execModal, running: true, error: '', result: null };
    const res = await authedFetch<ExecResult>('POST', `/docker/containers/${encodeURIComponent(execModal.name)}/exec`, {
      command: argv,
      timeout_secs: 30,
    });
    if (!execModal) return;
    if (res.success && res.data) {
      execModal = { ...execModal, running: false, result: res.data };
    } else {
      execModal = { ...execModal, running: false, error: res.message || res.error || translate('docker.exec_failed') };
    }
  }

  async function updateContainerImage(c: Container) {
    ctrMenuFor = null;
    if (!(await dlg.ask(translate('docker.recreate_confirm', { name: c.name })))) return;
    recreateBusy = c.name;
    const res = await authedFetch('POST', `/docker/containers/${encodeURIComponent(c.name)}/recreate`);
    recreateBusy = null;
    if (res.success) setBanner('ok', translate('docker.container_recreated', { name: c.name }));
    else setBanner('error', res.message || res.error || translate('docker.recreate_failed'));
    await loadContainers();
  }

  // ── Image build ───────────────────────────────────────────────────────────
  let buildModal = $state<{
    context: string;
    dockerfile: string;
    tag: string;
    noCache: boolean;
    pull: boolean;
    running: boolean;
    log: string;
    error: string;
  } | null>(null);

  function openBuildModal() {
    buildModal = { context: '', dockerfile: '', tag: '', noCache: false, pull: false, running: false, log: '', error: '' };
  }

  async function submitBuild() {
    if (!buildModal) return;
    if (!buildModal.context.trim() || !buildModal.tag.trim()) return;
    buildModal = { ...buildModal, running: true, log: '', error: '' };
    const res = await authedFetch<{ log: string }>('POST', '/docker/images/build', {
      context: buildModal.context.trim(),
      dockerfile: buildModal.dockerfile.trim() || null,
      tag: buildModal.tag.trim(),
      no_cache: buildModal.noCache,
      pull: buildModal.pull,
      build_args: [],
    });
    if (!buildModal) return;
    if (res.success) {
      buildModal = { ...buildModal, running: false, log: res.data?.log ?? '' };
      setBanner('ok', translate('docker.build_success', { tag: buildModal.tag.trim() }));
      await loadImages();
    } else {
      buildModal = { ...buildModal, running: false, error: res.message || res.error || translate('docker.build_failed') };
    }
  }

  // ── Private registries ────────────────────────────────────────────────────
  let registryModal = $state<{
    registries: string[];
    loading: boolean;
    host: string;
    username: string;
    password: string;
    busy: boolean;
  } | null>(null);

  async function openRegistries() {
    registryModal = { registries: [], loading: true, host: '', username: '', password: '', busy: false };
    const res = await authedFetch<string[]>('GET', '/docker/registries');
    if (!registryModal) return;
    registryModal = { ...registryModal, loading: false, registries: res.success && res.data ? res.data : [] };
  }

  async function registryLogin() {
    if (!registryModal) return;
    registryModal = { ...registryModal, busy: true };
    const res = await authedFetch('POST', '/docker/registries/login', {
      registry: registryModal.host.trim(),
      username: registryModal.username.trim(),
      password: registryModal.password,
    });
    if (!registryModal) return;
    if (res.success) {
      setBanner('ok', translate('docker.registry_logged_in'));
      registryModal = { ...registryModal, busy: false, host: '', username: '', password: '' };
      await openRegistries();
    } else {
      registryModal = { ...registryModal, busy: false };
      setBanner('error', res.message || res.error || translate('docker.registry_login_failed'));
    }
  }

  async function registryLogout(host: string) {
    if (!registryModal) return;
    registryModal = { ...registryModal, busy: true };
    const res = await authedFetch('POST', '/docker/registries/logout', { registry: host });
    if (!registryModal) return;
    if (res.success) setBanner('ok', translate('docker.registry_logged_out'));
    else setBanner('error', res.message || res.error || translate('docker.registry_logout_failed'));
    await openRegistries();
  }

  // ── Daemon configuration (daemon.json) ────────────────────────────────────
  let daemonConfigModal = $state<{ config: string; path: string; loading: boolean; busy: boolean; error: string } | null>(null);

  async function openDaemonConfig() {
    daemonConfigModal = { config: '', path: '/etc/docker/daemon.json', loading: true, busy: false, error: '' };
    const res = await authedFetch<{ config: string; path: string }>('GET', '/docker/daemon/config');
    if (!daemonConfigModal) return;
    if (res.success && res.data) {
      daemonConfigModal = {
        ...daemonConfigModal,
        loading: false,
        config: res.data.config || '{\n  \n}\n',
        path: res.data.path,
      };
    } else {
      daemonConfigModal = { ...daemonConfigModal, loading: false, error: res.message || res.error || '' };
    }
  }

  async function saveDaemonConfig() {
    if (!daemonConfigModal) return;
    // Validate locally first: a malformed daemon.json stops dockerd from
    // starting, which would take every container down.
    try {
      const parsed = JSON.parse(daemonConfigModal.config);
      if (typeof parsed !== 'object' || parsed === null || Array.isArray(parsed)) {
        daemonConfigModal = { ...daemonConfigModal, error: translate('docker.daemon_config_invalid') };
        return;
      }
    } catch {
      daemonConfigModal = { ...daemonConfigModal, error: translate('docker.daemon_config_invalid') };
      return;
    }
    daemonConfigModal = { ...daemonConfigModal, busy: true, error: '' };
    const res = await authedFetch('PUT', '/docker/daemon/config', { config: daemonConfigModal.config });
    if (!daemonConfigModal) return;
    if (res.success) {
      setBanner('ok', translate('docker.daemon_config_saved'));
      daemonConfigModal = null;
    } else {
      daemonConfigModal = { ...daemonConfigModal, busy: false, error: res.message || res.error || translate('docker.daemon_config_save_failed') };
    }
  }

  // ── Compose: pull newest images and redeploy ──────────────────────────────
  async function updateProject(name: string) {
    busy = true;
    const res = await authedFetch('POST', `/docker/compose/projects/${encodeURIComponent(name)}/update`);
    busy = false;
    if (res.success) setBanner('ok', translate('docker.stack_updated', { name }));
    else setBanner('error', res.message || res.error || translate('docker.stack_update_failed'));
    await loadProjects();
    await loadContainers();
  }

  $effect(() => { init(); });
  // No manual refresh button — revalidate the current tab on window focus +
  // a slow poll (container/compose state changes out of band).
  $effect(() => onAppVisible(refreshCurrent, { pollMs: 45_000 }));
  // Keep the newest line in view while following a live log.
  $effect(() => {
    if (logBox && containerLogs.length && logFollow) logBox.scrollTop = logBox.scrollHeight;
  });
  const metric = 'px-3 py-2.5 rounded-xl bg-[var(--hv-shell-border-soft)] border border-[var(--hv-shell-border)]';
  const dkEmpty = 'flex flex-col items-center gap-2 py-14 text-center text-[var(--hv-shell-text-2)]';
  const field = 'flex flex-col gap-1.5';
  const fieldLabel = 'text-xs font-semibold text-[var(--hv-shell-text-strong)]';
  const row3 = 'grid grid-cols-[1fr_1fr_auto_auto] items-center gap-2';
  // Shared control shapes (iconBtn / actionBtn / actionIconBtn / dangerHover /
  // textInput) come from components/app controlClasses.ts — see
  // docs/guides/design-system.md §4.3.
</script>
<PageLayout
  title="Docker"
  data-testid="docker-page"
  sidebarLabel={$t('docker.title')}
  sidebarTestId="docker-sidebar"
>
  {#snippet icon()}
    <IconContainer stroke={1.7} />
  {/snippet}

  {#snippet sidebar()}
    <div class="flex flex-col gap-0.5" role="tablist" aria-orientation="vertical" aria-label={$t('docker.title')}>
      {#each TAB_DEFS as tb, i (tb.id)}
        {@const count = tabCount(tb.id)}
        <WindowSidebarItem
          label={$t(tb.label)}
          testId="docker-nav-item"
          role="tab"
          ariaSelected={tab === tb.id}
          tabindex={tab === tb.id ? 0 : -1}
          active={tab === tb.id}
          count={count ?? undefined}
          countTitle={count !== null ? String(count) : undefined}
          onclick={() => switchTab(tb.id)}
          onkeydown={(e) => onNavKeydown(e, i)}
        />
      {/each}
    </div>

    <WindowSidebarSection label={$t('docker.metrics_heading')}>
      <ul class="list-none m-0 p-0 flex flex-col gap-2">
        <li data-testid="docker-metric" class={metric}>
          <h3 class="m-0 text-c-xs font-medium text-[var(--hv-shell-text-2)]">{$t('docker.stat_active')}</h3>
          <div class="text-xl font-bold text-[var(--hv-shell-text-strong)]">{runningCount}<span class="text-sm text-[var(--hv-shell-text-2)]">/ {containers.length}</span></div>
        </li>
        <li data-testid="docker-metric" class={metric}>
          <h3 class="m-0 text-c-xs font-medium text-[var(--hv-shell-text-2)]">{$t('docker.stat_cpu')}</h3>
          <div class="text-xl font-bold text-[var(--hv-shell-text-strong)]">{totalCpu}<span class="text-sm text-[var(--hv-shell-text-2)]">%</span></div>
        </li>
        <li data-testid="docker-metric" class={metric}>
          <h3 class="m-0 text-c-xs font-medium text-[var(--hv-shell-text-2)]">{$t('docker.stat_memory')}</h3>
          <div class="text-xl font-bold text-[var(--hv-shell-text-strong)]">{totalMemGb.toFixed(1)}<span class="text-sm text-[var(--hv-shell-text-2)]">GB</span></div>
        </li>
        <li data-testid="docker-metric" class={metric}>
          <h3 class="m-0 text-c-xs font-medium text-[var(--hv-shell-text-2)]">{$t('docker.stat_stopped')}</h3>
          <div class="text-xl font-bold text-[var(--hv-shell-text-strong)]">{containers.length - runningCount}</div>
        </li>
      </ul>
    </WindowSidebarSection>
  {/snippet}

  {#if dockerAvailable !== false}
    <div data-testid="docker-main-toolbar" class="flex items-center gap-2 px-3 py-2 border-b border-[var(--hv-shell-border)] bg-[var(--hv-shell-fill)] flex-wrap">
      {#if tab === 'containers' && containers.length > 0}
        <div data-testid="docker-search" class="flex items-center gap-2 h-10 px-3 rounded-lg bg-[var(--hv-shell-glass)] border border-[var(--hv-shell-border)] text-[var(--hv-shell-text-2)]">
          <IconSearch size={20} stroke={1.9} />
          <input type="text" class="flex-1 min-w-[140px] border-none bg-transparent text-sm text-[var(--hv-shell-text-strong)] outline-none" bind:value={cSearch} placeholder={$t('docker.filter_placeholder')}>
        </div>
        <div class="flex gap-1" role="group" aria-label={$t('docker.filter_placeholder')}>
          <button data-testid="docker-chip" class:dk-chip-active={cFilter === 'all'} class={cn('h-9 px-3 rounded-lg border border-[var(--hv-shell-border)] bg-[var(--hv-shell-glass)] text-c-sm font-medium cursor-pointer', cFilter === 'all' && 'bg-[var(--hv-primary-light)] border-[var(--hv-primary)]/50 text-[var(--hv-primary)]')} onclick={() => cFilter = 'all'}>{$t('docker.filter_all')}</button>
          <button data-testid="docker-chip" class:dk-chip-active={cFilter === 'running'} class={cn('h-9 px-3 rounded-lg border border-[var(--hv-shell-border)] bg-[var(--hv-shell-glass)] text-c-sm font-medium cursor-pointer', cFilter === 'running' && 'bg-[var(--hv-primary-light)] border-[var(--hv-primary)]/50 text-[var(--hv-primary)]')} onclick={() => cFilter = 'running'}>{$t('docker.filter_running')}</button>
          <button data-testid="docker-chip" class:dk-chip-active={cFilter === 'stopped'} class={cn('h-9 px-3 rounded-lg border border-[var(--hv-shell-border)] bg-[var(--hv-shell-glass)] text-c-sm font-medium cursor-pointer', cFilter === 'stopped' && 'bg-[var(--hv-primary-light)] border-[var(--hv-primary)]/50 text-[var(--hv-primary)]')} onclick={() => cFilter = 'stopped'}>{$t('docker.filter_stopped')}</button>
        </div>
      {/if}
      <div data-testid="docker-toolbar-actions" class="ml-auto flex gap-2">
        {#if tab === 'containers'}
          <Button variant="secondary" onclick={openDaemonConfig}><IconSettings size={18} stroke={1.9} /> {$t('docker.daemon_config')}</Button>
          <Button variant="primary" data-testid="docker-top-btn" onclick={() => openCreateModal()}><IconPlus size={18} stroke={2} /> {$t('docker.new_container')}</Button>
        {:else if tab === 'images'}
          <Button variant="secondary" onclick={openRegistries}><IconLock size={18} stroke={1.9} /> {$t('docker.registries')}</Button>
          <Button variant="secondary" onclick={openBuildModal}><IconTool size={18} stroke={1.9} /> {$t('docker.build_image')}</Button>
          <Button variant="primary" data-testid="docker-top-btn" onclick={openPullModal}><IconDownload size={18} stroke={2} /> {$t('docker.pull_image2')}</Button>
        {:else if tab === 'volumes'}
          <Button variant="primary" data-testid="docker-top-btn" onclick={openVolumeModal}><IconPlus size={18} stroke={2} /> {$t('docker.new_volume')}</Button>
        {:else if tab === 'networks'}
          <Button variant="primary" data-testid="docker-top-btn" onclick={openNetModal}><IconPlus size={18} stroke={2} /> {$t('docker.create_network')}</Button>
        {/if}
      </div>
    </div>
  {/if}

  {#if banner}
    <div class="px-3 pt-3">
      <Alert variant={banner.kind === 'error' ? 'error' : 'success'} dismissible ondismiss={dismissBanner}>{banner.text}</Alert>
    </div>
  {/if}

  {#if dockerAvailable === false}
    <div data-testid="docker-daemon-down" class="flex flex-col items-center gap-3 py-16 text-center px-6">
      <IconAlertTriangle size={56} stroke={1.5} class="text-[var(--hv-warning)]" />
      <div class="text-lg font-semibold text-[var(--hv-shell-text-strong)]">{$t('docker.unavailable')}</div>
      <p class="m-0 text-sm text-[var(--hv-shell-text)]">{$t('docker.unavailable_desc')}</p>
      <div class="flex gap-2">
        <Button variant="primary" onclick={startDaemon} disabled={daemonStarting}><IconPlayerPlay size={18} stroke={1.9} /> {daemonStarting ? $t('docker.daemon_starting') : $t('docker.daemon_start')}</Button>
        <Button variant="secondary" onclick={openDaemonLogs}><IconTerminal size={18} stroke={1.9} /> {$t('docker.daemon_logs')}</Button>
      </div>
    </div>

  {:else if tab === 'containers'}
    {#if containers.length === 0}
      <EmptyState data-testid="docker-empty" title={$t('docker.no_containers2')} description={$t('docker.no_containers_desc')}>
        {#snippet iconSnippet()}<IconContainer size={32} stroke={1.5} />{/snippet}
        {#snippet action()}<Button variant="primary" onclick={() => openCreateModal()}><IconPlus size={16} stroke={2} /> {$t('docker.new_container')}</Button>{/snippet}
      </EmptyState>
    {:else}
      <div data-testid="docker-card-grid" class="grid gap-3 p-3 [grid-template-columns:repeat(auto-fill,minmax(340px,1fr))]">
        {#each filteredContainers as c (c.id)}
          {@const ic = iconFor(c.image)}
          <div data-testid="docker-ctr-card" class="rounded-2xl bg-[var(--hv-shell-card)] border border-[var(--hv-shell-border)] shadow-[var(--hv-shell-shadow-card)] overflow-hidden">
            <div class="px-4 pt-3">
              <div class="flex items-center gap-3">
                <span class="grid place-items-center w-11 h-11 rounded-xl flex-none bg-[var(--hv-primary-light)] text-[var(--hv-primary)]"><ic.icon size={22} stroke={1.9} /></span>
                <div class="flex-1 min-w-0">
                  <div class="flex items-center gap-2 flex-wrap">
                    <span class="text-sm font-bold text-[var(--hv-shell-text-strong)] truncate">{c.name}</span>
                    {#if c.state === 'Running'}
                      <span class="inline-flex items-center gap-1 text-c-xs font-bold text-[var(--hv-success)]"><span class="w-1.5 h-1.5 rounded-full bg-[var(--hv-success)]"></span>{$t('docker.state_active')}</span>
                    {:else if c.state === 'Restarting'}
                      <span class="inline-flex items-center gap-1 text-c-xs font-bold text-[var(--hv-warning)]"><span class="w-1.5 h-1.5 rounded-full bg-[var(--hv-warning)]"></span>{$t('docker.state_restarting')}</span>
                    {:else if c.state === 'Paused'}
                      <span class="inline-flex items-center gap-1 text-c-xs font-bold text-[var(--hv-warning)]"><span class="w-1.5 h-1.5 rounded-full bg-[var(--hv-warning)]"></span>{$t('docker.state_paused')}</span>
                    {:else}
                      <span class="inline-flex items-center gap-1 text-c-xs font-bold text-[var(--hv-shell-text-2)]"><span class="w-1.5 h-1.5 rounded-full bg-[var(--hv-shell-text-3)]"></span>{$t('docker.state_stopped')}</span>
                    {/if}
                    {#if c.update_available}
                      <span class="inline-flex items-center gap-1 text-c-xs font-bold text-[var(--hv-warning)]"><IconDownload size={12} stroke={2} />{$t('docker.update_available')}</span>
                    {/if}
                  </div>
                  <div class="text-xs text-[var(--hv-shell-text-2)] truncate">{c.image}</div>
                </div>
              </div>
              <div class="flex flex-wrap gap-1.5 mt-2">
                {#if c.ports.length}<span class="px-2 py-0.5 rounded-md bg-[var(--hv-shell-border-soft)] text-c-xs text-[var(--hv-shell-text-2)]">{c.ports.map(p => `${p.host_port}:${p.container_port}`).join(' · ')}</span>{/if}
                {#if c.memory_mb !== null}<span class="px-2 py-0.5 rounded-md bg-[var(--hv-shell-border-soft)] text-c-xs text-[var(--hv-shell-text-2)]">{c.memory_mb} MB</span>{/if}
                {#if c.cpu_percent !== null}<span class="px-2 py-0.5 rounded-md bg-[var(--hv-shell-border-soft)] text-c-xs text-[var(--hv-shell-text-2)]">{c.cpu_percent}% CPU</span>{/if}
                {#if c.net_rx_bytes !== null || c.net_tx_bytes !== null}<span class="px-2 py-0.5 rounded-md bg-[var(--hv-shell-border-soft)] text-c-xs text-[var(--hv-shell-text-2)]" title={$t('docker.ctr_netio')}>↓{fmtBytes(c.net_rx_bytes)} ↑{fmtBytes(c.net_tx_bytes)}</span>{/if}
                {#if (c.blk_read_bytes ?? 0) > 0 || (c.blk_write_bytes ?? 0) > 0}<span class="px-2 py-0.5 rounded-md bg-[var(--hv-shell-border-soft)] text-c-xs text-[var(--hv-shell-text-2)]" title={$t('docker.ctr_blockio')}>⇄ {fmtBytes((c.blk_read_bytes ?? 0) + (c.blk_write_bytes ?? 0))}</span>{/if}
                {#if c.compose_project}<span class="px-2 py-0.5 rounded-md bg-[var(--hv-shell-border-soft)] text-c-xs text-[var(--hv-shell-text-2)]">{c.compose_project}</span>{/if}
              </div>
            </div>
            {#if c.ports.length}
              {@const url = `http://${location.hostname}:${c.ports[0].host_port}`}
              <div class="flex items-center gap-2 px-4 py-2 mt-2 border-t border-[var(--hv-shell-border-soft)]">
                <IconGlobe size={20} stroke={1.8} class="text-[var(--hv-shell-text-2)]" />
                <span class="flex-1 text-c-base text-[var(--hv-shell-text-2)] font-[family-name:var(--hv-font-mono)] truncate">{url}</span>
                <div class="relative">
                  <button class="inline-flex items-center gap-1 h-8 px-2.5 rounded-lg border-none bg-transparent text-[var(--hv-primary)] text-xs font-semibold cursor-pointer hover:bg-[var(--hv-primary-light)]" aria-haspopup="menu" aria-expanded={urlMenuFor === c.id} onclick={(e) => { e.stopPropagation(); if (urlMenuFor === c.id) { urlMenuFor = null; return; } ctrMenuFor = null; urlMenuFor = c.id; menuPos = menuAnchor(e.currentTarget as HTMLElement, 176, 92); }}><IconExternalLink size={17} stroke={2} /> {$t('docker.open_app')}</button>
                  {#if urlMenuFor === c.id && menuPos}
                    <button type="button" class="fixed inset-0 z-40 cursor-default" aria-label={$t('common.close')} onclick={() => urlMenuFor = null}></button>
                    <div style="top: {menuPos.top}px; left: {menuPos.left}px" class="fixed z-50 w-44 rounded-xl bg-[var(--hv-shell-glass)] border border-[var(--hv-shell-border)] shadow-[var(--hv-shell-shadow-card)] p-1" role="menu">
                      <button type="button" role="menuitem" class="w-full px-3 py-2 rounded-lg text-left text-c-base cursor-pointer hover:bg-[var(--hv-subtle-hover)]" onclick={() => openContainerUrl(url, 'tab')}>{$t('docker.open_in_new_tab')}</button>
                      <button type="button" role="menuitem" class="w-full px-3 py-2 rounded-lg text-left text-c-base cursor-pointer hover:bg-[var(--hv-subtle-hover)]" onclick={() => openContainerUrl(url, 'window')}>{$t('docker.open_in_new_window')}</button>
                    </div>
                  {/if}
                </div>
                <button class="hv-ic grid place-items-center w-7 h-7 rounded-lg border-none bg-transparent text-[var(--hv-shell-text-2)] cursor-pointer hover:text-[var(--hv-shell-text-strong)]" onclick={() => copyText(url)} aria-label={$t('docker.copy_address')}><IconCopy size={17} stroke={1.9} /></button>
              </div>
            {:else}
              <div class="flex items-center gap-2 px-4 py-2 mt-2 border-t border-[var(--hv-shell-border-soft)] text-c-base text-[var(--hv-shell-text-3)]">
                <IconGlobe size={20} stroke={1.8} />
                <span>{$t('docker.no_web_ui')}</span>
              </div>
            {/if}
            <div data-testid="docker-ctr-actions" class="flex gap-2 px-4 py-3">
              {#if c.state === 'Running'}
                <button class={actionBtn} onclick={() => containerAction(c.name, 'stop')}><IconSquare size={17} /> {$t('docker.btn_stop')}</button>
              {:else}
                <button class={cn(actionBtn, 'bg-[var(--hv-success)] border-[var(--hv-success)] text-white hover:brightness-110')} onclick={() => containerAction(c.name, 'start')}><IconPlayerPlay size={17} /> {$t('docker.btn_start')}</button>
              {/if}
              <button class={actionBtn} onclick={() => containerAction(c.name, 'restart')}><IconRefresh size={17} /> {$t('docker.btn_restart')}</button>
              <button class={actionBtn} onclick={() => openLogs(c.name)}><IconTerminal size={17} /> {$t('docker.logs')}</button>
              {#if c.compose_project}
                <button class={actionBtn} onclick={() => openEditCompose(c)}><IconPencil size={17} /> {$t('common.edit')}</button>
              {/if}
              <!-- Secondary actions live behind one menu: the card stays scannable
                   and the common path (start/stop/restart/logs) keeps its place. -->
              <div class="relative ml-auto">
                <button
                  class={actionIconBtn}
                  aria-haspopup="menu"
                  aria-expanded={ctrMenuFor === c.id}
                  aria-label={$t('docker.more_actions')}
                  title={$t('docker.more_actions')}
                  onclick={(e) => { if (ctrMenuFor === c.id) { ctrMenuFor = null; return; } urlMenuFor = null; ctrMenuFor = c.id; menuPos = menuAnchor(e.currentTarget as HTMLElement, 224, 316); }}
                ><IconBolt size={17} /></button>
                {#if ctrMenuFor === c.id && menuPos}
                  <button type="button" class="fixed inset-0 z-40 cursor-default" aria-label={$t('common.close')} onclick={() => ctrMenuFor = null}></button>
                  <div style="top: {menuPos.top}px; left: {menuPos.left}px" class="fixed z-50 w-56 rounded-xl bg-[var(--hv-shell-glass)] border border-[var(--hv-shell-border)] shadow-[var(--hv-shell-shadow-card)] p-1" role="menu">
                    <button type="button" role="menuitem" class="w-full px-3 py-2 rounded-lg text-left text-c-base cursor-pointer hover:bg-[var(--hv-subtle-hover)]" onclick={() => openInspect(c.name)}>{$t('docker.inspect')}</button>
                    {#if c.state === 'Paused'}
                      <button type="button" role="menuitem" class="w-full px-3 py-2 rounded-lg text-left text-c-base cursor-pointer hover:bg-[var(--hv-subtle-hover)]" onclick={() => { ctrMenuFor = null; containerExtra(c.name, 'unpause'); }}>{$t('docker.unpause')}</button>
                    {:else if c.state === 'Running'}
                      <button type="button" role="menuitem" class="w-full px-3 py-2 rounded-lg text-left text-c-base cursor-pointer hover:bg-[var(--hv-subtle-hover)]" onclick={() => { ctrMenuFor = null; containerExtra(c.name, 'pause'); }}>{$t('docker.pause')}</button>
                      <button type="button" role="menuitem" class="w-full px-3 py-2 rounded-lg text-left text-c-base cursor-pointer hover:bg-[var(--hv-subtle-hover)]" onclick={() => { ctrMenuFor = null; openExec(c.name); }}>{$t('docker.exec')}</button>
                      <button type="button" role="menuitem" class="w-full px-3 py-2 rounded-lg text-left text-c-base cursor-pointer hover:bg-[var(--hv-subtle-hover)]" onclick={() => { ctrMenuFor = null; killContainer(c.name); }}>{$t('docker.kill')}</button>
                    {/if}
                    <button type="button" role="menuitem" class="w-full px-3 py-2 rounded-lg text-left text-c-base cursor-pointer hover:bg-[var(--hv-subtle-hover)]" onclick={() => openRename(c.name)}>{$t('docker.rename')}</button>
                    <button type="button" role="menuitem" class="w-full px-3 py-2 rounded-lg text-left text-c-base cursor-pointer hover:bg-[var(--hv-subtle-hover)]" onclick={() => openLimits(c.name)}>{$t('docker.update_limits')}</button>
                    <button type="button" role="menuitem" class="w-full px-3 py-2 rounded-lg text-left text-c-base cursor-pointer hover:bg-[var(--hv-subtle-hover)]" disabled={recreateBusy === c.name} onclick={() => updateContainerImage(c)}>{recreateBusy === c.name ? $t('docker.updating_image') : $t('docker.update_image')}</button>
                  </div>
                {/if}
              </div>
              <button class={cn(actionIconBtn, dangerHover)} onclick={() => removeContainer(c.name, c.state === 'Running')} aria-label={$t('common.delete')}><IconTrash size={17} /></button>
            </div>
          </div>
        {/each}
      </div>
    {/if}

  {:else if tab === 'images'}
    {#if images.length === 0}
      <EmptyState data-testid="docker-empty" title={$t('docker.no_images2')} description={$t('docker.pull_image_desc')}>
        {#snippet iconSnippet()}<ImageIcon size={32} stroke={1.5} />{/snippet}
        {#snippet action()}<Button variant="primary" onclick={openPullModal}><IconDownload size={16} stroke={2} /> {$t('docker.pull_image2')}</Button>{/snippet}
      </EmptyState>
    {:else}
      <div data-testid="docker-panel" class="p-3">
        <div class="flex items-center gap-2 px-1 pb-2"><span class="text-sm font-semibold text-[var(--hv-shell-text-strong)]">{$t('docker.local_images')}</span><span class="text-xs text-[var(--hv-shell-text-2)]">{images.length}</span></div>
        <div class="flex flex-col gap-1.5">
          {#each images as img (img.id)}
            <div data-testid="docker-row" class="flex items-center gap-3 px-3 py-2.5 rounded-xl bg-[var(--hv-shell-card)] border border-[var(--hv-shell-border)]">
              <ImageIcon size={24} stroke={1.7} class="text-[var(--hv-shell-text-2)] flex-none" />
              <div class="flex-1 min-w-0">
                <div class="text-c-base font-semibold text-[var(--hv-shell-text-strong)] truncate">{img.repository}:{img.tag}</div>
                <div class="text-c-xs text-[var(--hv-shell-text-2)]">{img.id} · {img.created}</div>
              </div>
              <span class="text-xs text-[var(--hv-shell-text-2)] flex-none">{img.size}</span>
              <button class={actionBtn} onclick={() => openCreateModal(`${img.repository}:${img.tag}`)}>{$t('docker.deploy_image')}</button>
              <button class={cn(actionIconBtn, dangerHover)} onclick={() => removeImage(img.id)} aria-label={$t('common.delete')}><IconTrash size={17} /></button>
            </div>
          {/each}
        </div>
      </div>
    {/if}

  {:else if tab === 'volumes'}
    <!-- Disk usage + reclaim. `docker system df` is the only honest source of
         "what is taking space"; the prune buttons sit right next to it so the
         number and the fix are in the same place. -->
    <div data-testid="docker-panel" class="p-3">
      <div class="rounded-2xl bg-[var(--hv-shell-card)] border border-[var(--hv-shell-border)] shadow-[var(--hv-shell-shadow-card)] overflow-hidden">
        <div class="flex items-center gap-2 px-4 py-3 border-b border-[var(--hv-shell-border-soft)]">
          <IconDatabase size={20} stroke={1.8} class="text-[var(--hv-shell-text-2)]" />
          <span class="text-sm font-semibold text-[var(--hv-shell-text-strong)]">{$t('docker.disk_usage')}</span>
          {#if diskUsage}
            <span class="ml-auto text-xs text-[var(--hv-shell-text-2)]">
              {$t('docker.disk_reclaimable_total')}
              <strong class="text-[var(--hv-warning)]">{diskUsage.total_reclaimable || '0B'}</strong>
            </span>
          {/if}
        </div>
        {#if diskUsage && diskUsage.rows.length}
          <table class="w-full text-sm">
            <thead>
              <tr class="text-left text-c-xs uppercase tracking-wide text-[var(--hv-shell-text-3)]">
                <th scope="col" class="px-4 py-2 font-semibold">{$t('docker.disk_kind')}</th>
                <th scope="col" class="px-4 py-2 font-semibold">{$t('docker.disk_total')}</th>
                <th scope="col" class="px-4 py-2 font-semibold">{$t('docker.disk_active')}</th>
                <th scope="col" class="px-4 py-2 font-semibold">{$t('docker.disk_size')}</th>
                <th scope="col" class="px-4 py-2 font-semibold">{$t('docker.disk_reclaimable')}</th>
              </tr>
            </thead>
            <tbody>
              {#each diskUsage.rows as row (row.kind)}
                <tr class="border-t border-[var(--hv-shell-border-soft)]">
                  <td class="px-4 py-2 text-[var(--hv-shell-text-strong)]">{diskKindLabel(row.kind)}</td>
                  <td class="px-4 py-2 text-[var(--hv-shell-text-2)]">{row.total}</td>
                  <td class="px-4 py-2 text-[var(--hv-shell-text-2)]">{row.active}</td>
                  <td class="px-4 py-2 text-[var(--hv-shell-text-2)]">{row.size}</td>
                  <td class="px-4 py-2 text-[var(--hv-shell-text-2)]">{row.reclaimable}</td>
                </tr>
              {/each}
            </tbody>
          </table>
        {:else}
          <p class="m-0 px-4 py-3 text-xs text-[var(--hv-shell-text-2)]" role="status">{$t('docker.disk_loading')}</p>
        {/if}
        <div class="flex flex-wrap gap-2 px-4 py-3 border-t border-[var(--hv-shell-border-soft)]">
          <Button variant="secondary" disabled={pruneBusy !== null} onclick={() => prune('images')}>{$t('docker.prune_images')}</Button>
          <Button variant="secondary" disabled={pruneBusy !== null} onclick={() => prune('volumes')}>{$t('docker.prune_volumes')}</Button>
          <Button variant="secondary" disabled={pruneBusy !== null} onclick={() => prune('builders')}>{$t('docker.prune_builders')}</Button>
          <Button variant="danger" disabled={pruneBusy !== null} onclick={() => prune('system', true)}>{$t('docker.prune_system')}</Button>
        </div>
      </div>
    </div>

    {#if volumesLoading && volumes.length === 0}
      <div class="grid gap-3 p-3 [grid-template-columns:repeat(auto-fill,minmax(300px,1fr))]" role="status">
        <span class="sr-only">{$t('common.loading')}</span>
        {#each Array(3) as _}
          <div class="flex flex-col gap-1.5 p-4 rounded-2xl bg-[var(--hv-shell-card)] border border-[var(--hv-shell-border)]" aria-hidden="true">
            <span class="hv-skeleton-text block w-1/2"></span>
            <span class="hv-skeleton-text block w-3/4"></span>
          </div>
        {/each}
      </div>
    {:else if volumes.length === 0}
      <EmptyState data-testid="docker-empty" title={$t('docker.no_volumes')} description={$t('docker.no_volumes_desc')}>
        {#snippet iconSnippet()}<IconDatabase size={32} stroke={1.5} />{/snippet}
        {#snippet action()}<Button variant="primary" onclick={openVolumeModal}><IconPlus size={16} stroke={2} /> {$t('docker.new_volume')}</Button>{/snippet}
      </EmptyState>
    {:else}
      <div class="px-3 pb-3">
        <div class="flex flex-col gap-1.5">
          {#each volumes as v (v.name)}
            <div data-testid="docker-row" class="flex items-center gap-3 px-3 py-2.5 rounded-xl bg-[var(--hv-shell-card)] border border-[var(--hv-shell-border)]">
              <IconDatabase size={22} stroke={1.7} class="text-[var(--hv-shell-text-2)] flex-none" />
              <div class="flex-1 min-w-0">
                <div class="flex items-center gap-2">
                  <span class="text-c-base font-semibold text-[var(--hv-shell-text-strong)] truncate">{v.name}</span>
                  {#if v.in_use}
                    <span class="inline-flex items-center gap-1 px-2 py-0.5 rounded-full bg-[var(--hv-primary-light)] text-[var(--hv-primary)] text-c-xs font-bold">{$t('docker.volume_in_use')}</span>
                  {:else}
                    <span class="inline-flex items-center gap-1 px-2 py-0.5 rounded-full bg-[var(--hv-shell-border-soft)] text-[var(--hv-shell-text-2)] text-c-xs font-bold">{$t('docker.volume_unused')}</span>
                  {/if}
                </div>
                <div class="text-c-xs text-[var(--hv-shell-text-2)] font-[family-name:var(--hv-font-mono)] truncate">{v.mountpoint}</div>
                {#if v.used_by.length}
                  <div class="text-c-xs text-[var(--hv-shell-text-2)] truncate">{$t('docker.volume_used_by', { names: v.used_by.join(', ') })}</div>
                {/if}
              </div>
              <span class="px-2 py-0.5 rounded-md bg-[var(--hv-shell-border-soft)] text-c-xs text-[var(--hv-shell-text-2)] flex-none">{v.driver}</span>
              <button class={cn(actionIconBtn, dangerHover)} disabled={volumeBusy} onclick={() => deleteVolume(v)} aria-label={$t('docker.volume_delete_aria', { name: v.name })}><IconTrash size={17} /></button>
            </div>
          {/each}
        </div>
      </div>
    {/if}

  {:else if tab === 'networks'}
    {#if networks.length === 0}
      <EmptyState data-testid="docker-empty" title={$t('docker.no_networks')} description={$t('docker.create_network_title')}>
        {#snippet iconSnippet()}<IconNetwork size={32} stroke={1.5} />{/snippet}
        {#snippet action()}<Button variant="primary" onclick={openNetModal}><IconPlus size={16} stroke={2} /> {$t('docker.create_network')}</Button>{/snippet}
      </EmptyState>
    {:else}
      <div class="grid gap-3 p-3 [grid-template-columns:repeat(auto-fill,minmax(280px,1fr))]">
        {#each networks as net (net.id)}
          <div data-testid="docker-net-card" class="rounded-2xl bg-[var(--hv-shell-card)] border border-[var(--hv-shell-border)] shadow-[var(--hv-shell-shadow-card)] overflow-hidden">
            <div class="flex items-center gap-3 px-4 pt-4">
              <span class="grid place-items-center w-10 h-10 rounded-xl flex-none bg-[var(--hv-primary-light)] text-[var(--hv-primary)]"><IconNetwork size={24} stroke={1.7} /></span>
              <div class="min-w-0">
                <div class="text-sm font-bold text-[var(--hv-shell-text-strong)] truncate">{net.name}</div>
                <div class="text-xs text-[var(--hv-shell-text-2)]">{net.driver}</div>
              </div>
            </div>
            <div class="px-4 py-3 flex flex-col gap-1.5 text-sm">
              <div class="flex justify-between"><span class="text-xs text-[var(--hv-shell-text-2)]">{$t('docker.subnet_label')}</span><span class="font-[family-name:var(--hv-font-mono)] text-xs text-[var(--hv-shell-text-strong)]">{net.subnet || '—'}</span></div>
              <div class="flex justify-between"><span class="text-xs text-[var(--hv-shell-text-2)]">{$t('docker.containers_label')}</span><span class="text-xs text-[var(--hv-shell-text-strong)]">{$t('docker.attached_count', { count: String(net.containers.length) })}</span></div>
            </div>
            <div class="px-4 pb-4"><Button variant="danger" onclick={() => removeNetwork(net.name)}><IconTrash size={16} /> {$t('common.delete')}</Button></div>
          </div>
        {/each}
      </div>
    {/if}

  {:else if tab === 'discover'}
    <div class="flex items-center gap-2 p-3">
      <div class="flex items-center gap-2 h-10 px-3 flex-1 max-w-md rounded-lg bg-[var(--hv-shell-card)] border border-[var(--hv-shell-border)] text-[var(--hv-shell-text-2)]">
        <IconSearch size={20} stroke={1.9} />
        <input type="text" class="flex-1 min-w-0 border-none bg-transparent text-sm text-[var(--hv-shell-text-strong)] outline-none" bind:value={hubQuery} onkeydown={(e) => { if (e.key === 'Enter') searchHub(); }} placeholder={$t('docker.search_placeholder')}>
      </div>
      <button class={cn('h-10 px-4 rounded-lg border border-[var(--hv-shell-border)] bg-[var(--hv-shell-card)] text-c-base font-medium cursor-pointer inline-flex items-center gap-1.5', 'bg-[var(--hv-primary-light)] border-[var(--hv-primary)]/50 text-[var(--hv-primary)]')} onclick={searchHub} disabled={hubLoading}>
        <IconSearch size={15} stroke={2} /> {hubLoading ? $t('docker.searching') : $t('common.search')}
      </button>
    </div>
    {#if hubLoading}
      <div data-testid="docker-empty" class="grid gap-3 p-3 [grid-template-columns:repeat(auto-fill,minmax(240px,1fr))]" role="status">
        <span class="sr-only">{$t('docker.searching')}</span>
        {#each Array(6) as _, i}
          <div class="flex flex-col gap-1.5 p-4 rounded-2xl bg-[var(--hv-shell-card)] border border-[var(--hv-shell-border)] shadow-[var(--hv-shell-shadow-card)]" aria-hidden="true">
            <span class="hv-skeleton-rect block w-11 h-11"></span>
            <span class="hv-skeleton-text block" style="width: {55 + ((i * 31) % 30)}%"></span>
            <span class="hv-skeleton-text block" style="width: {70 + ((i * 17) % 25)}%"></span>
          </div>
        {/each}
      </div>
    {:else if hubResults.length === 0}
      <div data-testid="docker-empty" class={dkEmpty}><p class="m-0 text-sm">{$t('docker.no_results')}</p></div>
    {:else}
      <div class="grid gap-3 p-3 [grid-template-columns:repeat(auto-fill,minmax(240px,1fr))]">
        {#each hubResults as r (r.name)}
          <div data-testid="docker-app-card" class="flex flex-col gap-1.5 p-4 rounded-2xl bg-[var(--hv-shell-card)] border border-[var(--hv-shell-border)] shadow-[var(--hv-shell-shadow-card)]">
            <span class="grid place-items-center w-11 h-11 rounded-xl bg-[var(--hv-primary-light)] text-[var(--hv-primary)]"><ImageIcon size={26} stroke={1.7} /></span>
            <div class="text-sm font-bold text-[var(--hv-shell-text-strong)]">{r.name}{#if r.is_official}<span class="ml-1 px-1.5 py-0.5 rounded bg-[var(--hv-success-bg)] text-[var(--hv-success)] text-micro font-bold uppercase">{$t('docker.official')}</span>{/if}</div>
            <div class="text-c-xs text-[var(--hv-shell-text-2)]"><IconStar size={12} fill="currentColor" /> {r.star_count} · {r.pull_count}</div>
            <p class="m-0 text-xs text-[var(--hv-shell-text)] leading-relaxed flex-1">{r.description || $t('docker.no_description')}</p>
            <button class={cn(actionBtn, 'mt-2 bg-[var(--hv-primary)] border-[var(--hv-primary)] text-white hover:brightness-110')} onclick={() => openCreateModal(r.name)}>{$t('docker.install')}</button>
          </div>
        {/each}
      </div>
    {/if}

  {:else if tab === 'catalog'}
    <div class="flex items-center gap-2 p-3">
      <div class="flex items-center gap-2 h-10 px-3 flex-1 max-w-md rounded-lg bg-[var(--hv-shell-card)] border border-[var(--hv-shell-border)] text-[var(--hv-shell-text-2)]">
        <IconSearch size={16} stroke={1.9} />
        <input type="search" class="flex-1 min-w-0 border-none bg-transparent text-sm text-[var(--hv-shell-text-strong)] outline-none" placeholder={$t('docker.catalog_search')} bind:value={catalogSearch} aria-label={$t('docker.catalog_search')} />
      </div>
      <button type="button" class="hv-btn hv-btn-secondary" aria-expanded={showStoresPanel} onclick={() => { showStoresPanel = !showStoresPanel; if (showStoresPanel) loadStores(); }}><IconPackage size={17} stroke={1.9} /> {$t('docker.stores_title')}</button>
    </div>
    {#if Object.keys(catalogUpdates).length > 0}
      <div class="flex items-center gap-2 mx-3 px-3 py-2 rounded-lg bg-[var(--hv-warning-bg)] text-[var(--hv-warning)] text-xs font-semibold"><IconDownload size={16} stroke={2} /> {$t('docker.catalog_updates_pending', { count: String(Object.keys(catalogUpdates).length) })}
        <button class={cn(actionBtn, 'ml-auto')} disabled={updateAllBusy} onclick={updateAllCatalogApps}>{updateAllBusy ? $t('docker.catalog_updating_all') : $t('docker.catalog_update_all')}</button>
      </div>
    {/if}
    {#if showStoresPanel}
      <div class="mx-3 p-3 rounded-xl bg-[var(--hv-shell-card)] border border-[var(--hv-shell-border)]">
        <p class="m-0 text-xs text-[var(--hv-shell-text)]">{$t('docker.stores_desc')}</p>
        <form class="flex gap-2 mt-2" onsubmit={(e) => { e.preventDefault(); addStore(); }}>
          <input type="url" class={textInput} placeholder={$t('docker.stores_url_placeholder')} bind:value={storeUrl} aria-label={$t('docker.stores_add')} />
          <Button type="submit" variant="primary" disabled={storeBusy || !storeUrl.trim()}><IconPlus size={16} stroke={2} /> {$t('docker.stores_add')}</Button>
        </form>
        <div class="flex flex-col gap-1.5 mt-2">
          <div class="flex items-center gap-2 px-3 py-2 rounded-lg bg-[var(--hv-shell-border-soft)] border border-[var(--hv-shell-border)]">
            <div class="flex-1"><span class="block text-c-base font-semibold text-[var(--hv-shell-text-strong)]">{$t('docker.stores_builtin')} · HomeVault</span><span class="block text-c-xs text-[var(--hv-shell-text-2)]">{$t('docker.stores_app_count', { count: String(catalogApps.filter((a) => a.store_id === 'homevault').length) })}</span></div>
          </div>
          {#if storesLoading}
            <div class="flex items-center gap-2 px-3 py-2 rounded-lg bg-[var(--hv-shell-border-soft)] border border-[var(--hv-shell-border)]" aria-hidden="true">
              <span class="hv-skeleton-text block flex-1"></span>
              <span class="hv-skeleton-text block w-16"></span>
            </div>
          {/if}
          {#each appStores as s (s.id)}
            <div class="flex items-center gap-2 px-3 py-2 rounded-lg bg-[var(--hv-shell-border-soft)] border border-[var(--hv-shell-border)]">
              <div class="flex-1 min-w-0"><span class="block text-c-base font-semibold text-[var(--hv-shell-text-strong)]">{s.name}</span><span class="block text-c-xs text-[var(--hv-shell-text-2)]">{s.git_url}{#if s.last_error}<span class="text-[var(--hv-error)]"> · {s.last_error}</span>{:else if !s.last_synced_at}<span> · {$t('docker.stores_never_synced')}</span>{/if}</span></div>
              <span class="px-2 py-0.5 rounded-full bg-[var(--hv-primary-light)] text-[var(--hv-primary)] text-c-xs font-bold">{ $t('docker.stores_app_count', { count: String(s.app_count) })}</span>
              <button class={actionIconBtn} disabled={storeBusy} onclick={() => syncStore(s)} aria-label={$t('docker.stores_sync')}><IconRefresh size={16} /></button>
              <button class={cn(actionIconBtn, dangerHover)} disabled={storeBusy} onclick={() => removeStore(s)} aria-label={$t('docker.stores_remove')}><IconTrash size={16} /></button>
            </div>
          {/each}
        </div>
      </div>
    {/if}
    {#if catalogLoading}
      <div data-testid="docker-empty" class="grid gap-3 p-3 [grid-template-columns:repeat(auto-fill,minmax(240px,1fr))]" role="status">
        <span class="sr-only">{$t('common.loading')}</span>
        {#each Array(6) as _, i}
          <div class="flex flex-col gap-1.5 p-4 rounded-2xl bg-[var(--hv-shell-card)] border border-[var(--hv-shell-border)] shadow-[var(--hv-shell-shadow-card)]" aria-hidden="true">
            <span class="hv-skeleton-rect block w-11 h-11"></span>
            <span class="hv-skeleton-text block" style="width: {55 + ((i * 31) % 30)}%"></span>
            <span class="hv-skeleton-text block" style="width: {70 + ((i * 17) % 25)}%"></span>
          </div>
        {/each}
      </div>
    {:else}
      <div class="grid gap-3 p-3 [grid-template-columns:repeat(auto-fill,minmax(240px,1fr))]">
        {#each filteredCatalog as app (app.id)}
          {@const CatalogIcon = catalogIconFor(app.icon)}
          <div data-testid="docker-app-card" class="flex flex-col gap-1.5 p-4 rounded-2xl bg-[var(--hv-shell-card)] border border-[var(--hv-shell-border)] shadow-[var(--hv-shell-shadow-card)]">
            <span class="grid place-items-center w-11 h-11 rounded-xl bg-[var(--hv-primary-light)] text-[var(--hv-primary)]">
              {#if app.has_icon}<img src={`/api/docker/catalog/${encodeURIComponent(app.id)}/icon`} alt="" width="26" height="26" />{:else}<CatalogIcon size={26} stroke={1.7} />{/if}
            </span>
            <div class="text-sm font-bold text-[var(--hv-shell-text-strong)]">{app.name}</div>
            <div class="flex items-center gap-1.5 text-c-xs text-[var(--hv-shell-text-2)]">
              {#if app.store_id !== 'homevault'}<span class="px-1.5 py-0.5 rounded-full bg-[var(--hv-primary-light)] text-[var(--hv-primary)] font-bold">{app.store_name}</span>{/if}
              <span>v{app.version}</span>
              {#if app.developer}<span>{$t('docker.catalog_by', { dev: app.developer })}</span>{/if}
            </div>
            <p class="m-0 text-xs text-[var(--hv-shell-text)] leading-relaxed flex-1">{catalogDescFor(app)}</p>
            {#if app.gallery_count > 0}
              <div class="flex gap-1 overflow-hidden">{#each Array(app.gallery_count) as _, i}<img src={`/api/docker/catalog/${encodeURIComponent(app.id)}/gallery/${i + 1}`} alt={`${app.name} ${i + 1}`} loading="lazy" class="w-8 h-8 object-cover rounded" />{/each}</div>
            {/if}
            {#if app.incompatibility_reason}<span class="inline-flex items-center gap-1 px-2 py-0.5 rounded bg-[var(--hv-error-bg)] text-[var(--hv-error)] text-c-xs font-bold"><IconAlertTriangle size={13} stroke={2} />{$t('docker.catalog_incompatible', { reason: app.incompatibility_reason })}</span>
            {:else if app.unsupported_on_host}<span class="inline-flex items-center gap-1 px-2 py-0.5 rounded bg-[var(--hv-error-bg)] text-[var(--hv-error)] text-c-xs font-bold"><IconAlertTriangle size={13} stroke={2} />{$t('docker.catalog_arch_unsupported')}</span>{/if}
            {#if app.missing_dependencies.length}<span class="inline-flex items-center gap-1 px-2 py-0.5 rounded bg-[var(--hv-error-bg)] text-[var(--hv-error)] text-c-xs font-bold"><IconAlertTriangle size={13} stroke={2} />{$t('docker.catalog_dep_missing', { deps: app.missing_dependencies.join(', ') })}</span>{/if}
            {#if app.installed}
              {#if catalogUpdates[app.id]}
                <span class="inline-flex items-center gap-1 px-2 py-0.5 rounded bg-[var(--hv-warning-bg)] text-[var(--hv-warning)] text-c-xs font-bold"><IconDownload size={13} stroke={2} />{$t('docker.catalog_update_available')}</span>
              {:else}
                <span class="inline-flex items-center gap-1 px-2 py-0.5 rounded bg-[var(--hv-success-bg)] text-[var(--hv-success)] text-c-xs font-bold"><span class="w-1.5 h-1.5 rounded-full bg-[var(--hv-success)]"></span>{$t('docker.catalog_installed_badge')}</span>
              {/if}
              <div class="flex flex-wrap gap-1.5 mt-1">
                {#if catalogUpdates[app.id]}
                  <button class={cn(actionBtn, 'text-c-base')} disabled={updateBusyId === app.id} onclick={() => updateCatalogApp(app)}><IconDownload size={15} stroke={2} /> {updateBusyId === app.id ? $t('docker.catalog_updating') : $t('docker.catalog_update_now')}</button>
                {/if}
                {#if app.running && appOpenUrl(app)}
                  <a class={cn(actionBtn, 'no-underline')} href={appOpenUrl(app)} target="_blank" rel="noopener noreferrer"><IconExternalLink size={15} stroke={2} /> {$t('docker.open_app')}</a>
                {/if}
                {#if app.proxy_port}
                  <label class="flex items-center gap-1.5 text-c-sm text-[var(--hv-shell-text-2)] cursor-pointer"><input type="checkbox" class="accent-[var(--hv-primary)]" checked={app.proxy_auth_required} onchange={() => toggleAppProxyAuth(app)} /> {$t('docker.proxy_require_login')}</label>
                {/if}
                <button class={cn(actionBtn, 'text-c-base')} onclick={() => openAppLogs(app)}><IconTerminal size={15} stroke={2} /> {$t('docker.catalog_app_logs')}</button>
                <button class={cn(actionBtn, dangerHover, 'text-c-base')} disabled={catalogBusyId === app.id} onclick={() => uninstallCatalogApp(app)}><IconTrash size={15} stroke={2} /> {$t('docker.catalog_uninstall')}</button>
              </div>
            {:else}
              <button class={cn(actionBtn, 'mt-1 bg-[var(--hv-primary)] border-[var(--hv-primary)] text-white hover:brightness-110')} disabled={catalogBusyId === app.id || app.unsupported_on_host || !!app.incompatibility_reason} onclick={() => openCatalogInstall(app)}><IconDownload size={15} stroke={2} /> {$t('docker.catalog_install')}</button>
            {/if}
          </div>
        {/each}
      </div>
    {/if}

  {:else if tab === 'compose'}
    {#if composeAvailable === false}
      <EmptyState data-testid="docker-empty" title={$t('docker.unavailable_desc')}>
        {#snippet iconSnippet()}<IconAlertTriangle size={30} stroke={1.5} />{/snippet}
      </EmptyState>
    {/if}
    <div class="grid grid-cols-1 @min-[900px]:grid-cols-[1fr_260px] gap-3 p-3">
      <div class="flex flex-col">
        <div class="flex items-center gap-2 px-3 py-2 rounded-t-xl bg-[var(--hv-shell-border-soft)] border border-[var(--hv-shell-border)] border-b-0">
          <span class="font-[family-name:var(--hv-font-mono)] text-c-base text-[var(--hv-shell-text-strong)]">docker-compose.yml</span>
          <span class="text-c-xs text-[var(--hv-shell-text-2)]">{$t('docker.compose_file')}</span>
        </div>
        <textarea data-testid="docker-code-area" class="w-full min-h-[300px] p-3 rounded-b-xl border border-[var(--hv-shell-border)] bg-[var(--hv-shell-border-soft)] text-c-base font-[family-name:var(--hv-font-mono)] outline-none focus:border-[var(--hv-primary)]" bind:value={composeYaml} disabled={composeAvailable === false} placeholder={'services:\n  app:\n    image: nginx:latest\n    ports:\n      - "8080:80"\n    restart: unless-stopped'}></textarea>
      </div>
      <div class="flex flex-col gap-2.5">
        <div class={field}>
          <label class={fieldLabel} for="composeName">{$t('docker.stack_name')}</label>
          <input type="text" id="composeName" class={textInput} bind:value={composeName} disabled={composeAvailable === false}>
        </div>
        <Button variant="primary" onclick={deployCompose} disabled={composeDeploying || composeAvailable === false}><IconCheck size={18} stroke={2} /> {$t('docker.deploy')}</Button>
        <Button variant="secondary" data-testid="docker-compose-validate" onclick={validateCompose} disabled={composeValidating || composeAvailable === false}><IconCheck size={18} stroke={2} /> {composeValidating ? $t('docker.validating') : $t('docker.validate_syntax')}</Button>
        {#if composeStatus}<div data-testid="docker-compose-status" style="color:{composeStatus.color};">{composeStatus.text}</div>{/if}
        <div class="p-3 rounded-xl bg-[var(--hv-shell-border-soft)] border border-[var(--hv-shell-border)]">
          <div class="text-c-base font-semibold text-[var(--hv-shell-text-strong)]">{$t('docker.deploy_title')}</div>
          <p class="m-0 mt-1 text-xs text-[var(--hv-shell-text)]">{$t('docker.deploy_desc')}</p>
        </div>
      </div>
    </div>
    {#if projects.length}
      <h3 class="px-3 pt-1 text-c-card-title font-semibold text-[var(--hv-shell-text-strong)]">{$t('docker.deployed_stacks')}</h3>
      <div class="px-3 pb-3">
        <div data-testid="docker-panel" class="flex flex-col gap-1.5">
          {#each projects as p (p.name)}
            <div data-testid="docker-row" class="flex items-center gap-3 px-3 py-2.5 rounded-xl bg-[var(--hv-shell-card)] border border-[var(--hv-shell-border)]">
              <div class="flex-1 min-w-0">
                <div class="text-c-base font-semibold text-[var(--hv-shell-text-strong)]">{p.name}</div>
                <div class="text-c-xs text-[var(--hv-shell-text-2)]">{p.services.join(', ') || '—'}</div>
              </div>
              <span class={cn('inline-flex items-center gap-1 text-c-xs font-bold', p.status === 'running' ? 'text-[var(--hv-success)]' : 'text-[var(--hv-shell-text-2)]')}><span class={cn('w-1.5 h-1.5 rounded-full', p.status === 'running' ? 'bg-[var(--hv-success)]' : 'bg-[var(--hv-shell-text-3)]')}></span>{p.status}</span>
              {#if p.status === 'running'}
                <button class={actionIconBtn} onclick={() => composeAction(p.name, 'stop')} aria-label={$t('docker.btn_stop')}><IconSquare size={16} /></button>
              {:else}
                <button class={actionIconBtn} onclick={() => composeAction(p.name, 'start')} aria-label={$t('docker.btn_start')}><IconPlayerPlay size={16} /></button>
              {/if}
              <button class={actionIconBtn} onclick={() => composeAction(p.name, 'restart')} aria-label={$t('docker.btn_restart')}><IconRefresh size={16} /></button>
              <button class={actionIconBtn} disabled={busy} onclick={() => updateProject(p.name)} aria-label={$t('docker.stack_update')} title={$t('docker.stack_update')}><IconDownload size={16} /></button>
              <button class={cn(actionIconBtn, dangerHover)} onclick={() => removeProject(p.name)} aria-label={$t('common.delete')}><IconTrash size={16} /></button>
            </div>
          {/each}
        </div>
      </div>
    {/if}
  {/if}
<ConfirmDialog
    {dlg}
    onconfirm={() => dlg.settle(true)}
    oncancel={() => dlg.settle(false)}
  />
</PageLayout><!-- CATALOG INSTALL MODAL -->
{#if catalogPortModal}
  {@const cp = catalogPortModal}
  <Modal open={!!cp} title={$t('docker.catalog_install_title', { name: cp.name })} size="md" onclose={() => catalogPortModal = null}>
    {#snippet icon()}<IconDownload size={22} stroke={1.75} />{/snippet}
    {#snippet children()}
      <p class="m-0 mb-3 text-sm text-[var(--hv-shell-text)]">{$t('docker.catalog_install_desc')}</p>
      <div class={field}>
        <label class={fieldLabel} for="catalogPort">{$t('docker.catalog_port')}</label>
        <input type="number" id="catalogPort" class={textInput} bind:value={cp.port} oninput={() => onPortInput(cp.port)} />
        {#if portHint(cp.port).kind !== 'none'}
          {@const h = portHint(cp.port)}
          <span class={cn('text-c-xs', h.kind === 'bad' ? 'text-[var(--hv-error)]' : 'text-[var(--hv-warning)]')}>{h.text}</span>
        {/if}
      </div>
      {#if cp.reviewLoading}
        <p class="m-0 mt-3 text-xs text-[var(--hv-shell-text-2)]" role="status">{$t('docker.catalog_review_loading')}</p>
      {:else if cp.review}
        {#if cp.review.incompatibility_reason}
          <p class="m-0 mt-3 px-3 py-2 rounded-lg bg-[var(--hv-error-bg)] text-[var(--hv-error)] text-xs font-semibold">{$t('docker.catalog_review_blocked', { reason: cp.review.incompatibility_reason })}</p>
        {/if}
        {#if cp.review.dependencies.length > 0}
          <div class="mt-3 flex flex-col gap-2">
            <span class="text-xs font-bold uppercase tracking-wide text-[var(--hv-shell-text-3)]">{$t('docker.catalog_review_deps')}</span>
            {#each cp.review.dependencies as dep (dep.dependency)}
              <div class="flex flex-col gap-1">
                {#if dep.satisfied_by.length > 0}
                  <span class="text-xs text-[var(--hv-success)] font-semibold">{dep.dependency} — {$t('docker.catalog_review_dep_ok', { by: dep.satisfied_by.join(', ') })}</span>
                {:else if dep.providers.length > 0}
                  <label class={fieldLabel} for={`dep-${dep.dependency}`}>{$t('docker.catalog_review_dep_pick', { dep: dep.dependency })}</label>
                  <select id={`dep-${dep.dependency}`} class={textInput} bind:value={cp.alternatives[dep.dependency]}>
                    {#each dep.providers as p}<option value={p}>{p}</option>{/each}
                  </select>
                {:else}
                  <span class="text-xs text-[var(--hv-error)] font-semibold">{$t('docker.catalog_review_dep_none', { dep: dep.dependency })}</span>
                {/if}
              </div>
            {/each}
          </div>
        {/if}
        {#if cp.review.environment.length > 0}
          <div class="mt-3 flex flex-col gap-2">
            <span class="text-xs font-bold uppercase tracking-wide text-[var(--hv-shell-text-3)]">{$t('docker.catalog_review_env')}</span>
            {#each cp.review.environment as decl (decl.name)}
              <div class={field}>
                <label class={fieldLabel} for={`env-${decl.name}`}>{decl.name}{#if decl.required} *{/if}</label>
                {#if decl.options.length > 0}
                  <select id={`env-${decl.name}`} class={textInput} bind:value={cp.envValues[decl.name]}>
                    {#if !decl.required && !decl.default}<option value=""></option>{/if}
                    {#each decl.options as opt}<option value={opt}>{opt}</option>{/each}
                  </select>
                {:else}
                  <input type="text" id={`env-${decl.name}`} class={textInput} bind:value={cp.envValues[decl.name]} placeholder={decl.default ?? ''} />
                {/if}
                {#if decl.note}<span class="text-c-xs text-[var(--hv-shell-text-2)]">{decl.note}</span>{/if}
              </div>
            {/each}
          </div>
        {/if}
      {/if}
    {/snippet}
    {#snippet footer()}
      <div class="flex gap-2.5 justify-end">
        <Button variant="secondary" onclick={() => catalogPortModal = null}>{$t('common.cancel')}</Button>
        <Button variant="primary" disabled={catalogBusyId === cp.id || portHint(cp.port).kind === 'bad' || cp.reviewLoading || !!cp.review?.incompatibility_reason} onclick={confirmCatalogInstall}><IconCheck size={14} stroke={2} /> {$t('docker.catalog_install')}</Button>
      </div>
    {/snippet}
  </Modal>
{/if}

<!-- CONTAINER LOGS MODAL (live tail over SSE) -->
{#if logContainerName}
  <Modal open={!!logContainerName} title={$t('docker.title_logs', { name: logContainerName ?? '' })} size="lg" onclose={closeLogs}>
    {#snippet icon()}<IconTerminal size={22} stroke={1.75} />{/snippet}
    {#snippet children()}
      <div class="flex flex-wrap items-end gap-3 mb-2">
        <div class={cn(field, 'w-32')}>
          <label class={fieldLabel} for="logTail">{$t('docker.logs_tail')}</label>
          <select id="logTail" class={textInput} bind:value={logTail} onchange={startLogStream}>
            {#each LOG_TAIL_CHOICES as n (n)}<option value={n}>{n}</option>{/each}
          </select>
        </div>
        <label class="flex items-center gap-1.5 text-c-sm text-[var(--hv-shell-text-2)] cursor-pointer pb-2">
          <input type="checkbox" class="accent-[var(--hv-primary)]" bind:checked={logTimestamps} onchange={startLogStream} /> {$t('docker.logs_timestamps')}
        </label>
        <label class="flex items-center gap-1.5 text-c-sm text-[var(--hv-shell-text-2)] cursor-pointer pb-2">
          <input type="checkbox" class="accent-[var(--hv-primary)]" bind:checked={logFollow} onchange={startLogStream} /> {$t('docker.logs_follow')}
        </label>
        <span class="ml-auto pb-2 inline-flex items-center gap-1.5 text-c-xs font-semibold" role="status" aria-live="polite">
          {#if logState === 'live'}
            <span class="w-1.5 h-1.5 rounded-full bg-[var(--hv-success)]"></span><span class="text-[var(--hv-success)]">{$t('docker.logs_live')}</span>
          {:else if logState === 'connecting'}
            <span class="w-1.5 h-1.5 rounded-full bg-[var(--hv-warning)]"></span><span class="text-[var(--hv-shell-text-2)]">{$t('docker.logs_connecting')}</span>
          {:else if logState === 'ended'}
            <span class="w-1.5 h-1.5 rounded-full bg-[var(--hv-shell-text-3)]"></span><span class="text-[var(--hv-shell-text-2)]">{$t('docker.logs_ended')}</span>
          {:else if logState === 'error'}
            <span class="w-1.5 h-1.5 rounded-full bg-[var(--hv-error)]"></span><span class="text-[var(--hv-error)]">{$t('docker.logs_disconnected')}</span>
          {/if}
        </span>
      </div>
      <p class="m-0 mb-2 text-xs text-[var(--hv-shell-text-2)] font-[family-name:var(--hv-font-mono)]">{$t('docker.logs_cmd')}</p>
      <div
        bind:this={logBox}
        data-testid="docker-logs"
        class="h-72 overflow-auto p-3 rounded-xl bg-[var(--hv-shell-border-soft)] border border-[var(--hv-shell-border)] font-[family-name:var(--hv-font-mono)] text-c-sm text-[var(--hv-shell-text-strong)]"
        role="log"
        aria-live="polite"
      >
        {#if containerLogs.length === 0}
          <div class="text-[var(--hv-shell-text-2)]">{logState === 'error' ? logError : $t('docker.logs_empty')}</div>
        {:else}
          {#each containerLogs as line, i (i)}<div class="whitespace-pre-wrap">{line}</div>{/each}
        {/if}
      </div>
    {/snippet}
    {#snippet footer()}
      <div class="flex gap-2.5 justify-end">
        <Button variant="secondary" type="button" disabled={containerLogs.length === 0} onclick={downloadLogs}><IconDownload size={15} stroke={2} /> {$t('docker.logs_download')}</Button>
        <Button variant="secondary" type="button" onclick={startLogStream}>{$t('docker.logs_refresh')}</Button>
        <Button variant="primary" type="button" onclick={closeLogs}>{$t('common.close')}</Button>
      </div>
    {/snippet}
  </Modal>
{/if}

<!-- CATALOG APP LOGS MODAL -->
{#if appLogsModal}
  {@const al = appLogsModal}
  <Modal open={!!al} title={$t('docker.catalog_logs_title', { name: al.name })} size="lg" onclose={() => appLogsModal = null}>
    {#snippet icon()}<IconTerminal size={22} stroke={1.75} />{/snippet}
    {#snippet children()}
      {#if al.loading}
        <p class="m-0 text-sm text-[var(--hv-shell-text-2)]" role="status">{$t('common.loading')}</p>
      {:else if al.logs}
        <div class="h-72 overflow-auto p-3 rounded-xl bg-[var(--hv-shell-border-soft)] border border-[var(--hv-shell-border)] font-[family-name:var(--hv-font-mono)] text-c-sm text-[var(--hv-shell-text-strong)]">
          {#each al.logs.split('\n') as line}<div class="whitespace-pre-wrap">{line}</div>{/each}
        </div>
      {:else}
        <p class="m-0 text-sm text-[var(--hv-shell-text-2)]">{$t('docker.catalog_logs_failed')}</p>
      {/if}
    {/snippet}
  </Modal>
{/if}

<!-- EDIT COMPOSE MODAL -->
{#if editCompose}
  {@const ec = editCompose}
  <Modal open={!!ec} title={$t('docker.edit_compose_title', { name: ec.project })} size="lg" onclose={closeEditCompose}>
    {#snippet icon()}<IconPencil size={22} stroke={1.75} />{/snippet}
    {#snippet children()}
      <p class="m-0 mb-3 text-sm text-[var(--hv-shell-text)]">{ec.container} · docker-compose.yml</p>
      {#if ec.loading}
        <div data-testid="docker-compose-status">{$t('docker.logs_loading')}</div>
      {:else if !ec.managed}
        <div data-testid="docker-compose-status" style="color:var(--hv-error);">{$t('docker.edit_compose_not_managed')}</div>
      {:else}
        <div>
          <div class="flex items-center gap-2 px-3 py-2 rounded-t-xl bg-[var(--hv-shell-border-soft)] border border-[var(--hv-shell-border)] border-b-0">
            <span class="font-[family-name:var(--hv-font-mono)] text-c-base text-[var(--hv-shell-text-strong)]">docker-compose.yml</span>
            <span class="text-c-xs text-[var(--hv-shell-text-2)]">{$t('docker.compose_file')}</span>
          </div>
          <textarea data-testid="docker-code-area" class="w-full min-h-[260px] p-3 rounded-b-xl border border-[var(--hv-shell-border)] bg-[var(--hv-shell-border-soft)] text-c-base font-[family-name:var(--hv-font-mono)] outline-none focus:border-[var(--hv-primary)]" bind:value={ec.yaml} disabled={ec.busy}></textarea>
          {#if ec.status}<div data-testid="docker-compose-status" class="mt-2" style="color:{ec.status.color};">{ec.status.text}</div>{/if}
        </div>
      {/if}
    {/snippet}
    {#snippet footer()}
      <div class="flex gap-2.5 justify-end flex-wrap">
        {#if !ec.loading && !ec.managed}
          <Button variant="secondary" onclick={closeEditCompose}>{$t('common.close')}</Button>
        {:else if !ec.loading && ec.managed}
          <Button variant="secondary" onclick={closeEditCompose} disabled={ec.busy}>{$t('common.cancel')}</Button>
          <Button variant="secondary" onclick={validateEditCompose} disabled={ec.busy}>{$t('docker.validate_syntax')}</Button>
          <Button variant="secondary" onclick={() => saveEditCompose(false)} disabled={ec.busy}>{$t('common.save')}</Button>
          <Button variant="primary" onclick={() => saveEditCompose(true)} disabled={ec.busy}><IconCheck size={14} stroke={2} /> {$t('docker.save_redeploy')}</Button>
        {/if}
      </div>
    {/snippet}
  </Modal>
{/if}

<!-- DAEMON LOGS MODAL -->
{#if daemonLogsOpen}
  <Modal open={daemonLogsOpen} title={$t('docker.daemon_logs_title')} size="lg" onclose={closeDaemonLogs}>
    {#snippet icon()}<IconTerminal size={22} stroke={1.75} />{/snippet}
    {#snippet children()}
      <p class="m-0 mb-2 text-xs text-[var(--hv-shell-text-2)]">{$t('docker.daemon_logs_cmd')}</p>
      <div class="h-72 overflow-auto p-3 rounded-xl bg-[var(--hv-shell-border-soft)] border border-[var(--hv-shell-border)] font-[family-name:var(--hv-font-mono)] text-c-sm text-[var(--hv-shell-text-strong)]">
        {#each daemonLogs.split('\n') as line}<div class="whitespace-pre-wrap">{line}</div>{/each}
      </div>
    {/snippet}
  </Modal>
{/if}

<!-- CREATE CONTAINER MODAL -->
{#if modalOpen}
  <Modal open={modalOpen} title={$t('docker.modal_new_container')} size="lg" onclose={closeModal}>
    {#snippet icon()}<IconContainer size={22} stroke={1.75} />{/snippet}
    {#snippet children()}
      <p class="m-0 mb-3 text-sm text-[var(--hv-shell-text)]">{modalImagePreset ? modalImagePreset : $t('docker.modal_config')}</p>
      <div class="flex flex-col gap-3">
        <div class={field}>
          <label class={fieldLabel} for="cName">{$t('docker.container_name')}</label>
          <input type="text" id="cName" class={textInput} bind:value={cName} placeholder={$t('docker.ph_name')}>
        </div>
        <div class={field}>
          <label class={fieldLabel} for="cImage">{$t('docker.image')}</label>
          <input type="text" id="cImage" class={textInput} bind:value={cImage} placeholder={$t('docker.ph_image')}>
        </div>
        <div data-testid="docker-section" class="flex items-center gap-2 text-sm font-semibold text-[var(--hv-shell-text-strong)]">{$t('docker.ports_section')} <Button variant="secondary" size="sm" type="button" onclick={addPort}><IconPlus size={12} stroke={2} /> {$t('docker.add_port2')}</Button></div>
        {#each cPorts as p, i}
          {@const ph = portHint(p.host_port, cPorts.map(x => x.host_port))}
          <div data-testid="docker-row3" class={row3}>
            <input type="text" class={cn(textInput, ph.kind === 'bad' && 'border-[var(--hv-error)]')} class:dk-input-bad={ph.kind === 'bad'} bind:value={p.host_port} placeholder={$t('docker.ph_host')} oninput={() => onPortInput(p.host_port)} />
            <input type="text" class={textInput} bind:value={p.container_port} placeholder={$t('docker.ph_container')} />
            <select class={textInput} bind:value={p.protocol}><option value="tcp">TCP</option><option value="udp">UDP</option></select>
            <button data-testid="docker-icon-btn" class={iconBtn} type="button" onclick={() => removePort(i)} aria-label={$t('common.delete')}><IconX size={14} /></button>
          </div>
          {#if ph.kind !== 'none'}<span class={cn('text-c-xs', ph.kind === 'bad' ? 'text-[var(--hv-error)]' : 'text-[var(--hv-warning)]')}>{ph.text}</span>{/if}
        {/each}
        <div data-testid="docker-section" class="flex items-center gap-2 text-sm font-semibold text-[var(--hv-shell-text-strong)]">{$t('docker.volumes_section')} <Button variant="secondary" size="sm" type="button" onclick={addVolume}><IconPlus size={12} stroke={2} /> {$t('docker.add_volume2')}</Button></div>
        {#each cVolumes as v, i}
          <div class={cn(row3, 'grid-cols-[minmax(0,1fr)_minmax(0,1fr)_auto_auto]')}>
            <PathField bind:value={v.host_path} mode="folder" placeholder={$t('docker.ph_host_path')} />
            <input type="text" class={textInput} bind:value={v.container_path} placeholder={$t('docker.ph_container_path')} />
            <label class="flex items-center gap-1.5 text-c-sm text-[var(--hv-shell-text-2)] cursor-pointer"><input type="checkbox" class="accent-[var(--hv-primary)]" bind:checked={v.read_only}> {$t('docker.ro')}</label>
            <button data-testid="docker-icon-btn" class={iconBtn} type="button" onclick={() => removeVolume(i)} aria-label={$t('common.delete')}><IconX size={14} /></button>
          </div>
        {/each}
        <div data-testid="docker-section" class="flex items-center gap-2 text-sm font-semibold text-[var(--hv-shell-text-strong)]">{$t('docker.env_section')} <Button variant="secondary" size="sm" type="button" onclick={addEnv}><IconPlus size={12} stroke={2} /> {$t('docker.add_env2')}</Button></div>
        {#each cEnv as e, i}
          <div data-testid="docker-row3" class={row3}>
            <input type="text" class={textInput} bind:value={e.key} placeholder={$t('docker.ph_key')} />
            <input type="text" class={textInput} bind:value={e.value} placeholder={$t('docker.ph_value')} />
            <span></span>
            <button data-testid="docker-icon-btn" class={iconBtn} type="button" onclick={() => removeEnv(i)} aria-label={$t('common.delete')}><IconX size={14} /></button>
          </div>
        {/each}
        <div class="grid grid-cols-2 @max-[360px]:grid-cols-1 gap-3">
          <div class={field}>
            <label class={fieldLabel} for="cRestart">{$t('docker.restart_policy2')}</label>
            <select id="cRestart" class={textInput} bind:value={cRestart}>
              <option value="no">{$t('docker.restart_no')}</option>
              <option value="on-failure">{$t('docker.restart_on_failure')}</option>
              <option value="always">{$t('docker.restart_always')}</option>
              <option value="unless-stopped">{$t('docker.restart_unless')}</option>
            </select>
          </div>
          <div class={field}>
            <label class={fieldLabel} for="cNetwork">{$t('docker.network_label')}</label>
            <input type="text" id="cNetwork" class={textInput} bind:value={cNetwork} placeholder="bridge">
          </div>
        </div>
        <div class="grid grid-cols-2 @max-[360px]:grid-cols-1 gap-3">
          <div class={field}>
            <label class={fieldLabel} for="cMemory">{$t('docker.memory_label')}</label>
            <input type="text" id="cMemory" class={textInput} bind:value={cMemory} placeholder={$t('docker.ph_memory')}>
          </div>
          <div class={field}>
            <label class={fieldLabel} for="cCpu">{$t('docker.cpu_label')}</label>
            <input type="text" id="cCpu" class={textInput} bind:value={cCpu} placeholder={$t('docker.ph_cpu')}>
          </div>
        </div>
        <Toggle bind:checked={cPrivileged} label={$t('docker.privileged2')} description={$t('docker.privileged_desc')} />
        <Toggle bind:checked={cAutoRemove} label={$t('docker.auto_remove2')} description={$t('docker.auto_remove_desc')} />
      </div>
    {/snippet}
    {#snippet footer()}
      <div class="flex gap-2.5 justify-end">
        <Button variant="secondary" type="button" onclick={closeModal}>{$t('common.cancel')}</Button>
        <Button variant="primary" type="button" onclick={submitCreate} disabled={modalBusy || createPortsHaveConflict()}>{modalBusy ? $t('docker.creating2') : $t('docker.create_container2')}</Button>
      </div>
    {/snippet}
  </Modal>
{/if}

<!-- CREATE NETWORK MODAL -->
{#if netModalOpen}
  <Modal open={pullModalOpen} title={$t('docker.pull_image2')} size="sm" onclose={() => pullModalOpen = false}>
    {#snippet children()}
      <div class={field}>
        <label class={fieldLabel} for="pullImageName">{$t('docker.pull_image2')}</label>
        <input type="text" id="pullImageName" class={textInput} bind:value={pullImageName} placeholder={$t('docker.ph_image')}>
      </div>
    {/snippet}
    {#snippet footer()}
      <div class="flex gap-2.5 justify-end">
        <Button variant="secondary" type="button" onclick={() => pullModalOpen = false}>{$t('common.cancel')}</Button>
        <Button variant="primary" type="button" onclick={pullImage} disabled={!pullImageName.trim()}>{$t('docker.pull_image2')}</Button>
      </div>
    {/snippet}
  </Modal>
  <Modal open={netModalOpen} title={$t('docker.create_network_title')} size="md" onclose={() => netModalOpen = false}>
    {#snippet icon()}<IconNetwork size={22} stroke={1.75} />{/snippet}
    {#snippet children()}
      <p class="m-0 mb-3 text-sm text-[var(--hv-shell-text)]">{$t('docker.modal_config')}</p>
      <div class="flex flex-col gap-3">
        <div class={field}>
          <label class={fieldLabel} for="nName">{$t('docker.container_name')}</label>
          <input type="text" id="nName" class={textInput} bind:value={nName}>
        </div>
        <div class={field}>
          <label class={fieldLabel} for="nDriver">{$t('docker.driver')}</label>
          <select id="nDriver" class={textInput} bind:value={nDriver}><option value="bridge">bridge</option><option value="macvlan">macvlan</option><option value="overlay">overlay</option></select>
        </div>
        <div class="grid grid-cols-2 @max-[360px]:grid-cols-1 gap-3">
          <div class={field}>
            <label class={fieldLabel} for="nSubnet">{$t('docker.subnet_opt')}</label>
            <input type="text" id="nSubnet" class={textInput} bind:value={nSubnet} placeholder={$t('docker.ph_subnet')}>
          </div>
          <div class={field}>
            <label class={fieldLabel} for="nGateway">{$t('docker.gateway_opt')}</label>
            <input type="text" id="nGateway" class={textInput} bind:value={nGateway} placeholder={$t('docker.ph_gateway')}>
          </div>
        </div>
        <Toggle bind:checked={nInternal} label={$t('docker.internal_network')} />
      </div>
    {/snippet}
    {#snippet footer()}
      <div class="flex gap-2.5 justify-end">
        <Button variant="secondary" type="button" onclick={() => netModalOpen = false}>{$t('common.cancel')}</Button>
        <Button variant="primary" type="button" onclick={createNetwork}>{$t('common.create')}</Button>
      </div>
    {/snippet}
  </Modal>
{/if}
<!-- CREATE VOLUME MODAL -->
{#if volumeModalOpen}
  <Modal open={volumeModalOpen} title={$t('docker.new_volume')} size="sm" onclose={() => volumeModalOpen = false}>
    {#snippet icon()}<IconDatabase size={22} stroke={1.75} />{/snippet}
    {#snippet children()}
      <div class="flex flex-col gap-3">
        <div class={field}>
          <label class={fieldLabel} for="vName">{$t('docker.volume_name')}</label>
          <input type="text" id="vName" class={textInput} bind:value={vName} placeholder="my-data" />
          <span class="text-c-xs text-[var(--hv-shell-text-2)]">{$t('docker.volume_name_hint')}</span>
        </div>
        <div class={field}>
          <label class={fieldLabel} for="vDriver">{$t('docker.volume_driver')}</label>
          <input type="text" id="vDriver" class={textInput} bind:value={vDriver} placeholder="local" />
        </div>
      </div>
    {/snippet}
    {#snippet footer()}
      <div class="flex gap-2.5 justify-end">
        <Button variant="secondary" type="button" onclick={() => volumeModalOpen = false}>{$t('common.cancel')}</Button>
        <Button variant="primary" type="button" disabled={volumeBusy || !vName.trim()} onclick={createVolume}>{$t('common.create')}</Button>
      </div>
    {/snippet}
  </Modal>
{/if}

<!-- CONTAINER DETAIL (INSPECT) MODAL -->
{#if detailModal}
  {@const dm = detailModal}
  <Modal open={!!dm} title={$t('docker.inspect_title', { name: dm.name })} size="lg" onclose={() => detailModal = null}>
    {#snippet icon()}<IconInfoCircle size={22} stroke={1.75} />{/snippet}
    {#snippet children()}
      {#if dm.loading}
        <p class="m-0 text-sm text-[var(--hv-shell-text-2)]" role="status">{$t('common.loading')}</p>
      {:else if dm.error}
        <p class="m-0 text-sm text-[var(--hv-error)]">{dm.error}</p>
      {:else if dm.detail}
        {@const d = dm.detail}
        <div class="flex flex-col gap-3 text-sm">
          <dl class="grid grid-cols-[minmax(0,140px)_minmax(0,1fr)] gap-x-3 gap-y-1.5 m-0">
            <dt class="text-[var(--hv-shell-text-2)]">{$t('docker.inspect_image')}</dt>
            <dd class="m-0 font-[family-name:var(--hv-font-mono)] text-c-sm break-all">{d.image}</dd>
            <dt class="text-[var(--hv-shell-text-2)]">{$t('docker.inspect_state')}</dt>
            <dd class="m-0">{d.state}{#if d.health} · {$t('docker.inspect_health')}: {d.health}{/if}</dd>
            <dt class="text-[var(--hv-shell-text-2)]">{$t('docker.inspect_created')}</dt>
            <dd class="m-0">{d.created}</dd>
            <dt class="text-[var(--hv-shell-text-2)]">{$t('docker.inspect_restart_policy')}</dt>
            <dd class="m-0">{d.restart_policy} · {$t('docker.inspect_restarts', { count: String(d.restart_count) })}</dd>
            {#if d.exit_code !== null && d.state !== 'running'}
              <dt class="text-[var(--hv-shell-text-2)]">{$t('docker.inspect_exit_code')}</dt>
              <dd class="m-0">{d.exit_code}</dd>
            {/if}
            <dt class="text-[var(--hv-shell-text-2)]">{$t('docker.inspect_command')}</dt>
            <dd class="m-0 font-[family-name:var(--hv-font-mono)] text-c-sm break-all">{[...(d.entrypoint ?? []), ...(d.command ?? [])].join(' ') || '—'}</dd>
            {#if d.working_dir}
              <dt class="text-[var(--hv-shell-text-2)]">{$t('docker.inspect_working_dir')}</dt>
              <dd class="m-0 font-[family-name:var(--hv-font-mono)] text-c-sm">{d.working_dir}</dd>
            {/if}
            <dt class="text-[var(--hv-shell-text-2)]">{$t('docker.inspect_hostname')}</dt>
            <dd class="m-0">{d.hostname || '—'}{#if d.user} · {$t('docker.inspect_user')}: {d.user}{/if}</dd>
            <dt class="text-[var(--hv-shell-text-2)]">{$t('docker.inspect_networks')}</dt>
            <dd class="m-0">{d.networks.join(', ') || '—'}{#if d.ip_address} · {d.ip_address}{/if}</dd>
            <dt class="text-[var(--hv-shell-text-2)]">{$t('docker.inspect_limits')}</dt>
            <dd class="m-0">
              {#if d.memory_limit && d.memory_limit > 0}{Math.round(d.memory_limit / 1024 / 1024)} MB{:else}{$t('docker.inspect_unlimited')}{/if}
              {#if d.nano_cpus && d.nano_cpus > 0} · {(d.nano_cpus / 1e9).toFixed(2)} CPU{/if}
              {#if d.privileged} · {$t('docker.privileged2')}{/if}
            </dd>
            {#if d.log_driver}
              <dt class="text-[var(--hv-shell-text-2)]">{$t('docker.inspect_log_driver')}</dt>
              <dd class="m-0 font-[family-name:var(--hv-font-mono)] text-c-sm break-all">{d.log_driver}</dd>
            {/if}
          </dl>

          {#if d.ports.length}
            <section>
              <h3 class="m-0 mb-1 text-xs font-bold uppercase tracking-wide text-[var(--hv-shell-text-3)]">{$t('docker.inspect_ports')}</h3>
              <div class="flex flex-wrap gap-1.5">
                {#each d.ports as p (p.host_port + '-' + p.container_port + '-' + p.protocol)}
                  <span class="px-2 py-0.5 rounded-md bg-[var(--hv-shell-border-soft)] text-c-xs font-[family-name:var(--hv-font-mono)]">{p.host_port} → {p.container_port}/{p.protocol}</span>
                {/each}
              </div>
            </section>
          {/if}

          {#if d.mounts.length}
            <section>
              <h3 class="m-0 mb-1 text-xs font-bold uppercase tracking-wide text-[var(--hv-shell-text-3)]">{$t('docker.inspect_mounts')}</h3>
              <ul class="m-0 p-0 list-none flex flex-col gap-1">
                {#each d.mounts as m (m.destination)}
                  <li class="font-[family-name:var(--hv-font-mono)] text-c-xs text-[var(--hv-shell-text-2)] break-all">{m.source} → {m.destination}{#if m.read_only} (ro){/if}</li>
                {/each}
              </ul>
            </section>
          {/if}

          {#if d.env.length}
            <section>
              <h3 class="m-0 mb-1 text-xs font-bold uppercase tracking-wide text-[var(--hv-shell-text-3)]">{$t('docker.inspect_env')}</h3>
              <ul class="m-0 p-0 list-none flex flex-col gap-1 max-h-32 overflow-auto">
                {#each d.env as e (e)}
                  <li class="font-[family-name:var(--hv-font-mono)] text-c-xs text-[var(--hv-shell-text-2)] break-all">{e}</li>
                {/each}
              </ul>
            </section>
          {/if}

          {#if d.labels.length}
            <section>
              <h3 class="m-0 mb-1 text-xs font-bold uppercase tracking-wide text-[var(--hv-shell-text-3)]">{$t('docker.inspect_labels')}</h3>
              <ul class="m-0 p-0 list-none flex flex-col gap-1 max-h-32 overflow-auto">
                {#each d.labels as [k, v] (k)}
                  <li class="font-[family-name:var(--hv-font-mono)] text-c-xs text-[var(--hv-shell-text-2)] break-all">{k}={v}</li>
                {/each}
              </ul>
            </section>
          {/if}
        </div>
      {/if}
    {/snippet}
    {#snippet footer()}
      <div class="flex gap-2.5 justify-end">
        <Button variant="primary" type="button" onclick={() => detailModal = null}>{$t('common.close')}</Button>
      </div>
    {/snippet}
  </Modal>
{/if}

<!-- EXEC MODAL -->
{#if execModal}
  {@const ex = execModal}
  <Modal open={!!ex} title={$t('docker.exec_title', { name: ex.name })} size="lg" onclose={() => execModal = null}>
    {#snippet icon()}<IconTerminal size={22} stroke={1.75} />{/snippet}
    {#snippet children()}
      <form class="flex flex-col gap-3" onsubmit={(e) => { e.preventDefault(); runExec(); }}>
        <div class={field}>
          <label class={fieldLabel} for="execCommand">{$t('docker.exec_command')}</label>
          <input type="text" id="execCommand" class={textInput} bind:value={ex.command} placeholder="ls -la /config" autocomplete="off" spellcheck="false" />
          <span class="text-c-xs text-[var(--hv-shell-text-2)]">{$t('docker.exec_hint')}</span>
        </div>
      </form>
      {#if ex.error}
        <p class="m-0 mt-3 text-sm text-[var(--hv-error)]">{ex.error}</p>
      {/if}
      {#if ex.result}
        <div class="mt-3">
          <div class="flex items-center gap-2 mb-1">
            <span class="text-xs font-bold uppercase tracking-wide text-[var(--hv-shell-text-3)]">{$t('docker.exec_output')}</span>
            <span class={cn('px-2 py-0.5 rounded-md text-c-xs font-bold', ex.result.exit_code === 0 ? 'bg-[var(--hv-success-bg)] text-[var(--hv-success)]' : 'bg-[var(--hv-error-bg)] text-[var(--hv-error)]')}>
              {$t('docker.exec_exit_code', { code: String(ex.result.exit_code) })}
            </span>
          </div>
          <pre class="m-0 h-56 overflow-auto p-3 rounded-xl bg-[var(--hv-shell-border-soft)] border border-[var(--hv-shell-border)] font-[family-name:var(--hv-font-mono)] text-c-sm whitespace-pre-wrap break-all">{ex.result.stdout}{#if ex.result.stderr}{'\n'}{ex.result.stderr}{/if}</pre>
        </div>
      {/if}
    {/snippet}
    {#snippet footer()}
      <div class="flex gap-2.5 justify-end">
        <Button variant="secondary" type="button" onclick={() => execModal = null}>{$t('common.close')}</Button>
        <Button variant="primary" type="button" disabled={ex.running || !ex.command.trim()} onclick={runExec}>
          {ex.running ? $t('docker.exec_running') : $t('docker.exec_run')}
        </Button>
      </div>
    {/snippet}
  </Modal>
{/if}

<!-- RENAME MODAL -->
{#if renameModal}
  {@const rn = renameModal}
  <Modal open={!!rn} title={$t('docker.rename_title', { name: rn.name })} size="sm" onclose={() => renameModal = null}>
    {#snippet icon()}<IconPencil size={22} stroke={1.75} />{/snippet}
    {#snippet children()}
      <div class={field}>
        <label class={fieldLabel} for="renameValue">{$t('docker.new_name')}</label>
        <input type="text" id="renameValue" class={textInput} bind:value={rn.value} autocomplete="off" />
      </div>
    {/snippet}
    {#snippet footer()}
      <div class="flex gap-2.5 justify-end">
        <Button variant="secondary" type="button" onclick={() => renameModal = null}>{$t('common.cancel')}</Button>
        <Button variant="primary" type="button" disabled={rn.busy || !rn.value.trim()} onclick={submitRename}>{$t('common.save')}</Button>
      </div>
    {/snippet}
  </Modal>
{/if}

<!-- LIMITS MODAL (docker update, in place) -->
{#if limitsModal}
  {@const lm = limitsModal}
  <Modal open={!!lm} title={$t('docker.update_limits')} size="md" onclose={() => limitsModal = null}>
    {#snippet icon()}<IconBolt size={22} stroke={1.75} />{/snippet}
    {#snippet children()}
      <p class="m-0 mb-3 text-sm text-[var(--hv-shell-text)]">{$t('docker.update_limits_desc', { name: lm.name })}</p>
      <div class="flex flex-col gap-3">
        <div class={field}>
          <label class={fieldLabel} for="limitMemory">{$t('docker.memory_label')}</label>
          <input type="text" id="limitMemory" class={textInput} bind:value={lm.memory} placeholder={$t('docker.ph_memory')} />
        </div>
        <div class={field}>
          <label class={fieldLabel} for="limitCpu">{$t('docker.cpu_label')}</label>
          <input type="text" id="limitCpu" class={textInput} bind:value={lm.cpus} placeholder={$t('docker.ph_cpu')} />
        </div>
        <div class={field}>
          <label class={fieldLabel} for="limitRestart">{$t('docker.restart_policy2')}</label>
          <select id="limitRestart" class={textInput} bind:value={lm.restart}>
            <option value="">{$t('docker.restart_unchanged')}</option>
            <option value="no">{$t('docker.restart_no')}</option>
            <option value="on-failure">{$t('docker.restart_on_failure')}</option>
            <option value="always">{$t('docker.restart_always')}</option>
            <option value="unless-stopped">{$t('docker.restart_unless')}</option>
          </select>
        </div>
      </div>
    {/snippet}
    {#snippet footer()}
      <div class="flex gap-2.5 justify-end">
        <Button variant="secondary" type="button" onclick={() => limitsModal = null}>{$t('common.cancel')}</Button>
        <Button variant="primary" type="button" disabled={lm.busy} onclick={submitLimits}>{$t('common.save')}</Button>
      </div>
    {/snippet}
  </Modal>
{/if}

<!-- BUILD IMAGE MODAL -->
{#if buildModal}
  {@const bd = buildModal}
  <Modal open={!!bd} title={$t('docker.build_image')} size="md" onclose={() => buildModal = null}>
    {#snippet icon()}<IconTool size={22} stroke={1.75} />{/snippet}
    {#snippet children()}
      <div class="flex flex-col gap-3">
        <div class={field}>
          <label class={fieldLabel} for="buildContext">{$t('docker.build_context')}</label>
          <PathField bind:value={bd.context} mode="folder" placeholder="/var/lib/homevault/apps/myapp" />
          <span class="text-c-xs text-[var(--hv-shell-text-2)]">{$t('docker.build_context_hint')}</span>
        </div>
        <div class={field}>
          <label class={fieldLabel} for="buildDockerfile">{$t('docker.build_dockerfile')}</label>
          <input type="text" id="buildDockerfile" class={textInput} bind:value={bd.dockerfile} placeholder="Dockerfile" />
        </div>
        <div class={field}>
          <label class={fieldLabel} for="buildTag">{$t('docker.build_tag')}</label>
          <input type="text" id="buildTag" class={textInput} bind:value={bd.tag} placeholder="myapp:1.0" />
        </div>
        <Toggle bind:checked={bd.noCache} label={$t('docker.build_no_cache')} />
        <Toggle bind:checked={bd.pull} label={$t('docker.build_pull')} />
        {#if bd.error}
          <p class="m-0 text-sm text-[var(--hv-error)] whitespace-pre-wrap break-all">{bd.error}</p>
        {/if}
        {#if bd.log}
          <pre class="m-0 h-40 overflow-auto p-3 rounded-xl bg-[var(--hv-shell-border-soft)] border border-[var(--hv-shell-border)] font-[family-name:var(--hv-font-mono)] text-c-xs whitespace-pre-wrap break-all">{bd.log}</pre>
        {/if}
      </div>
    {/snippet}
    {#snippet footer()}
      <div class="flex gap-2.5 justify-end">
        <Button variant="secondary" type="button" onclick={() => buildModal = null}>{$t('common.close')}</Button>
        <Button variant="primary" type="button" disabled={bd.running || !bd.context.trim() || !bd.tag.trim()} onclick={submitBuild}>
          {bd.running ? $t('docker.build_running') : $t('docker.build_run')}
        </Button>
      </div>
    {/snippet}
  </Modal>
{/if}

<!-- PRIVATE REGISTRIES MODAL -->
{#if registryModal}
  {@const rg = registryModal}
  <Modal open={!!rg} title={$t('docker.registries')} size="md" onclose={() => registryModal = null}>
    {#snippet icon()}<IconLock size={22} stroke={1.75} />{/snippet}
    {#snippet children()}
      <p class="m-0 mb-3 text-sm text-[var(--hv-shell-text)]">{$t('docker.registries_desc')}</p>
      {#if rg.loading}
        <p class="m-0 text-sm text-[var(--hv-shell-text-2)]" role="status">{$t('common.loading')}</p>
      {:else if rg.registries.length === 0}
        <p class="m-0 text-sm text-[var(--hv-shell-text-2)]">{$t('docker.registry_none')}</p>
      {:else}
        <ul class="m-0 p-0 list-none flex flex-col gap-1.5">
          {#each rg.registries as host (host)}
            <li class="flex items-center gap-2 px-3 py-2 rounded-lg bg-[var(--hv-shell-border-soft)] border border-[var(--hv-shell-border)]">
              <span class="flex-1 min-w-0 text-c-base font-[family-name:var(--hv-font-mono)] truncate">{host}</span>
              <button class={cn(actionIconBtn, dangerHover)} disabled={rg.busy} onclick={() => registryLogout(host)} aria-label={$t('docker.registry_logout_aria', { host })}><IconTrash size={16} /></button>
            </li>
          {/each}
        </ul>
      {/if}
      <form class="flex flex-col gap-2.5 mt-4" onsubmit={(e) => { e.preventDefault(); registryLogin(); }}>
        <span class="text-xs font-bold uppercase tracking-wide text-[var(--hv-shell-text-3)]">{$t('docker.registry_add')}</span>
        <div class={field}>
          <label class={fieldLabel} for="registryHost">{$t('docker.registry_host')}</label>
          <input type="text" id="registryHost" class={textInput} bind:value={rg.host} placeholder="ghcr.io" autocomplete="off" spellcheck="false" />
        </div>
        <div class={field}>
          <label class={fieldLabel} for="registryUser">{$t('docker.registry_username')}</label>
          <input type="text" id="registryUser" class={textInput} bind:value={rg.username} autocomplete="username" />
        </div>
        <div class={field}>
          <label class={fieldLabel} for="registryPassword">{$t('docker.registry_password')}</label>
          <input type="password" id="registryPassword" class={textInput} bind:value={rg.password} autocomplete="current-password" />
        </div>
      </form>
    {/snippet}
    {#snippet footer()}
      <div class="flex gap-2.5 justify-end">
        <Button variant="secondary" type="button" onclick={() => registryModal = null}>{$t('common.close')}</Button>
        <Button variant="primary" type="button" disabled={rg.busy || !rg.username.trim() || !rg.password} onclick={registryLogin}>{$t('docker.registry_login')}</Button>
      </div>
    {/snippet}
  </Modal>
{/if}

<!-- DAEMON CONFIG MODAL -->
{#if daemonConfigModal}
  {@const dc = daemonConfigModal}
  <Modal open={!!dc} title={$t('docker.daemon_config')} size="lg" onclose={() => daemonConfigModal = null}>
    {#snippet icon()}<IconSettings size={22} stroke={1.75} />{/snippet}
    {#snippet children()}
      <p class="m-0 mb-2 text-sm text-[var(--hv-shell-text)]">{$t('docker.daemon_config_desc')}</p>
      <p class="m-0 mb-2 text-xs text-[var(--hv-warning)] font-semibold">{$t('docker.daemon_config_warning')}</p>
      {#if dc.loading}
        <p class="m-0 text-sm text-[var(--hv-shell-text-2)]" role="status">{$t('common.loading')}</p>
      {:else}
        <label class={fieldLabel} for="daemonConfig">{$t('docker.daemon_config_file', { path: dc.path })}</label>
        <textarea
          id="daemonConfig"
          class="w-full min-h-[240px] mt-1 p-3 rounded-xl border border-[var(--hv-shell-border)] bg-[var(--hv-shell-border-soft)] text-c-base font-[family-name:var(--hv-font-mono)] outline-none focus:border-[var(--hv-primary)]"
          bind:value={dc.config}
          spellcheck="false"
          disabled={dc.busy}
        ></textarea>
        {#if dc.error}
          <p class="m-0 mt-2 text-sm text-[var(--hv-error)] whitespace-pre-wrap break-all">{dc.error}</p>
        {/if}
      {/if}
    {/snippet}
    {#snippet footer()}
      <div class="flex gap-2.5 justify-end">
        <Button variant="secondary" type="button" onclick={() => daemonConfigModal = null}>{$t('common.cancel')}</Button>
        <Button variant="primary" type="button" disabled={dc.loading || dc.busy} onclick={saveDaemonConfig}>{$t('common.save')}</Button>
      </div>
    {/snippet}
  </Modal>
{/if}

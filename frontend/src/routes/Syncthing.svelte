<!-- ========================================================================
     File          : Syncthing.svelte
     Author        : Nicolas Le Roux
     Created       : 28/09/2026
     Last modified : 28/09/2026 by Nicolas Le Roux
     Version       : 1.0.0

     Description:
     The Syncthing app: device and folder management, connection state,
     rescan and service control through the backend's REST proxy.

     Dependencies:
       - svelte (>= 5), theme + app/ composites, $lib/services/api

     Usage:
     Opened as a desktop window app (admin only).
     ======================================================================== -->
<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { api } from '$lib/services/api';
  import { t, translate } from '$lib/stores/i18n';
  import { Spinner, Button } from '$lib/components/theme';
  import { PageLayout, StatCard, EmptyState } from '$lib/components/app';
  import { cn } from '$lib/utils.js';
  import WindowHeader from '$lib/components/desktop/WindowHeader.svelte';
  import { IconRefresh, IconPlayerPause, IconPlayerPlay, IconFolder, IconDeviceMobile, IconServer, IconCircleCheck, IconAlertCircle, IconClock, IconActivity } from '@tabler/icons-svelte';

  // ---------------------------------------------------------------------------
  // Types
  // ---------------------------------------------------------------------------

  interface SyncthingStatus {
    available: boolean;
    version: string | null;
    my_id: string | null;
    uptime: number | null;
    last_seen: string | null;
  }

  interface DeviceWithStatus {
    device_id: string;
    name: string;
    addresses: string[];
    compression: string;
    paused: boolean;
    connected: boolean;
    address: string | null;
    client_version: string | null;
    completion: number;
    in_bytes_total: number;
    out_bytes_total: number;
  }

  interface FolderStatus {
    id: string;
    label: string;
    path: string;
    paused: boolean;
    global_bytes: number;
    global_files: number;
    local_bytes: number;
    local_files: number;
    need_bytes: number;
    need_files: number;
    in_sync_files: number;
    receive_only: boolean;
    devices: string[];
  }

  // ---------------------------------------------------------------------------
  // State
  // ---------------------------------------------------------------------------

  let status: SyncthingStatus = { available: false, version: null, my_id: null, uptime: null, last_seen: null };
  let devices: DeviceWithStatus[] = [];
  let folders: FolderStatus[] = [];
  let loading = true;
  let acting = false;
  let activeTab: 'overview' | 'devices' | 'folders' = 'overview';
  let refreshInterval: ReturnType<typeof setInterval>;

  // Master-detail split — the devices/folders tabs show a compact list (left)
  // + a detail pane (right); narrow collapses to a single pane.
  let selectedDevice: DeviceWithStatus | null = null;
  let selectedFolder: FolderStatus | null = null;
  let splitEl: HTMLDivElement;
  let narrow = false;
  let splitRO: ResizeObserver | null = null;
  $: selectedDetail =
    (activeTab === 'devices' && !!selectedDevice) || (activeTab === 'folders' && !!selectedFolder);

  type TabDef = { id: 'overview' | 'devices' | 'folders'; label: string; icon: any };

  const tabs: TabDef[] = [
    { id: "overview", label: "Overview", icon: IconActivity },
    { id: 'devices', label: 'Devices', icon: IconDeviceMobile },
    { id: 'folders', label: 'Folders', icon: IconFolder },
  ];

  // ---------------------------------------------------------------------------
  // Lifecycle
  // ---------------------------------------------------------------------------

  onMount(async () => {
    await loadAll();
    refreshInterval = setInterval(loadData, 10000);

    // Master-detail split: `narrow` measures the split container itself (the
    // window floats, so a viewport media query would lie). Below 720px only
    // one pane shows.
    splitRO = new ResizeObserver((entries) => {
      narrow = (entries[0]?.contentRect.width ?? 0) < 720;
    });
    if (splitEl) splitRO.observe(splitEl);
  });

  onDestroy(() => {
    if (refreshInterval) clearInterval(refreshInterval);
    splitRO?.disconnect();
  });

  // ---------------------------------------------------------------------------
  // Data Loading
  // ---------------------------------------------------------------------------

  async function loadAll() {
    loading = true;
    await loadData();
    loading = false;
  }

  async function loadData() {
    await Promise.all([
      loadStatus(),
      loadDevices(),
      loadFolders(),
    ]);
  }

  async function loadStatus() {
    const res = await api.getSyncthingStatus();
    if (res.success && res.data) {
      status = res.data;
    }
  }

  async function loadDevices() {
    const res = await api.getSyncthingDevices();
    if (res.success && res.data) {
      devices = res.data;
    }
  }

  async function loadFolders() {
    const res = await api.getSyncthingFolders();
    if (res.success && res.data) {
      folders = res.data;
    }
  }

  // ---------------------------------------------------------------------------
  // Actions
  // ---------------------------------------------------------------------------

  async function toggleDevice(device: DeviceWithStatus) {
    acting = true;
    const res = device.paused
      ? await api.resumeSyncthingDevice(device.device_id)
      : await api.pauseSyncthingDevice(device.device_id);
    if (res.success) {
      await loadDevices();
    } else {
    }
    acting = false;
  }

  async function scanFolder(folderId: string) {
    acting = true;
    const res = await api.scanSyncthingFolder(folderId);
    if (res.success) {
    } else {
    }
    acting = false;
  }

  async function restartSyncthing() {
    acting = true;
    const res = await api.restartSyncthing();
    if (res.success) {
    } else {
    }
    acting = false;
  }

  // ---------------------------------------------------------------------------
  // Helpers
  // ---------------------------------------------------------------------------

  function formatBytes(bytes: number): string {
    if (bytes === 0) return '0 B';
    const units = ['B', 'KB', 'MB', 'GB', 'TB'];
    const i = Math.floor(Math.log(bytes) / Math.log(1024));
    const val = bytes / Math.pow(1024, i);
    return val.toFixed(i > 0 ? 1 : 0) + ' ' + units[i];
  }

  function formatUptime(seconds: number): string {
    const d = Math.floor(seconds / 86400);
    const h = Math.floor((seconds % 86400) / 3600);
    const m = Math.floor((seconds % 3600) / 60);
    if (d > 0) return `${d}d ${h}h ${m}m`;
    if (h > 0) return `${h}h ${m}m`;
    return `${m}m`;
  }

  function shortId(id: string): string {
    if (id.length <= 12) return id;
    return id.slice(0, 7) + '\u2026' + id.slice(-7);
  }

  function deviceLabel(deviceId: string): string {
    const d = devices.find((x) => x.device_id === deviceId);
    return d?.name || deviceId;
  }

  function syncPercent(folder: FolderStatus): number {
    if (folder.global_files === 0) return 100;
    return Math.round((folder.in_sync_files / folder.global_files) * 100);
  }

  $: connectedDevices = devices.filter(d => d.connected).length;
  $: pausedDevices = devices.filter(d => d.paused).length;
  $: syncedFolders = folders.filter(f => f.need_files === 0).length;
  $: totalSynced = folders.reduce((sum, f) => sum + f.in_sync_files, 0);
  $: totalFiles = folders.reduce((sum, f) => sum + f.global_files, 0);
  const badge = 'inline-flex items-center px-2 py-0.5 rounded-full text-c-xs font-bold uppercase';
  const detailLabel = 'text-xs text-[var(--hv-shell-text-2)]';
  const detailValue = 'text-sm font-semibold text-[var(--hv-shell-text-strong)]';
  const progressTrack = 'h-2 rounded-full bg-[var(--hv-shell-border-soft)] overflow-hidden';
</script>
<PageLayout
  title="Syncthing"
  data-testid="syncthing-page"
  tabs={[
    { id: 'overview', label: $t('syncthing.tab_overview'), icon: IconActivity },
    { id: 'devices', label: $t('syncthing.tab_devices'), icon: IconDeviceMobile },
    { id: 'folders', label: $t('syncthing.tab_folders'), icon: IconFolder }
  ]}
  bind:activeTab
  footerMode="status"
>
  {#snippet icon()}
    <IconFolder stroke={1.8} />
  {/snippet}
  <!-- Page-level action: a WindowToolbar row under the header, never inside
       it (the header bar is icon + title + window controls only). -->
  {#snippet toolbarRight()}
    <Button variant="primary" data-testid="syncthing-btn-primary" onclick={restartSyncthing} disabled={acting || !status.available}>
      <IconRefresh size={16} />
      {$t('syncthing.restart')}
    </Button>
  {/snippet}

  {#if loading}
    <div data-testid="syncthing-loading" class="flex items-center justify-center gap-3 py-16 text-[var(--hv-shell-text)]">
      <Spinner />
      <span>{$t('syncthing.loading')}</span>
    </div>
  {:else if !status.available}
    <EmptyState data-testid="syncthing-empty" title={$t('syncthing.not_available_title')} description={$t('syncthing.not_available_desc')}>
      {#snippet iconSnippet()}<IconAlertCircle size={32} stroke={1.8} />{/snippet}
    </EmptyState>
  {:else}
    <div data-testid="syncthing-stats" class="grid grid-cols-2 @min-[880px]:grid-cols-4 gap-3 mb-5">
      <StatCard data-testid="syncthing-stat-card" label={$t('syncthing.stat_devices')} value={`${devices.length}`} tone={'info'}>
        {#snippet iconSnippet()}<IconDeviceMobile size={20} />{/snippet}
      </StatCard>
      <StatCard data-testid="syncthing-stat-card" label={$t('syncthing.stat_folders')} value={`${folders.length}`} tone={'info'}>
        {#snippet iconSnippet()}<IconFolder size={20} />{/snippet}
      </StatCard>
      <StatCard data-testid="syncthing-stat-card" label={$t('syncthing.stat_connected')} value={`${connectedDevices}/${devices.length}`} tone={'success'}>
        {#snippet iconSnippet()}<IconActivity size={20} />{/snippet}
      </StatCard>
      <StatCard data-testid="syncthing-stat-card" label={$t('syncthing.stat_uptime')} value={`${status.uptime ? formatUptime(status.uptime) : $t('syncthing.na')}`} tone={'warning'}>
        {#snippet iconSnippet()}<IconClock size={20} />{/snippet}
      </StatCard>
    </div>

    {#if activeTab === 'overview'}
      <div class="flex flex-col gap-4">
        <div data-testid="syncthing-card" class="px-5 py-4 rounded-2xl bg-[var(--hv-shell-card)] border border-[var(--hv-shell-border)] shadow-[var(--hv-shell-shadow-card)]">
          <div class="flex items-center gap-2 mb-2"><span class="grid place-items-center w-9 h-9 rounded-lg bg-[var(--hv-primary-light)] text-[var(--hv-primary)]"><IconServer size={18} /></span><span class="text-sm font-semibold text-[var(--hv-shell-text-strong)]">{$t('syncthing.device_id_title')}</span></div>
          <code class="block text-sm font-[family-name:var(--hv-font-mono)] text-[var(--hv-shell-text-strong)] break-all">{status.my_id || $t('syncthing.unknown')}</code>
        </div>

        <div data-testid="syncthing-card" class="px-5 py-4 rounded-2xl bg-[var(--hv-shell-card)] border border-[var(--hv-shell-border)] shadow-[var(--hv-shell-shadow-card)]">
          <div class="flex items-center gap-2 mb-2"><span class="grid place-items-center w-9 h-9 rounded-lg bg-[var(--hv-primary-light)] text-[var(--hv-primary)]"><IconFolder size={18} /></span><span class="text-sm font-semibold text-[var(--hv-shell-text-strong)]">{$t('syncthing.sync_status_title')}</span></div>
          <div>
            <div class="flex items-center justify-between text-xs text-[var(--hv-shell-text-2)]">
              <span>{translate('syncthing.sync_files_progress', { synced: String(totalSynced), total: String(totalFiles) })}</span>
              <span class="font-bold text-[var(--hv-shell-text-strong)]">{totalFiles > 0 ? Math.round((totalSynced / totalFiles) * 100) : 100}%</span>
            </div>
            <div class={progressTrack}><div class={cn('h-full rounded-full', totalFiles === 0 || totalSynced === totalFiles ? 'bg-[var(--hv-success)]' : 'bg-[var(--hv-primary)]')} style="width: {totalFiles > 0 ? Math.round((totalSynced / totalFiles) * 100) : 100}%"></div></div>
          </div>
        </div>

        <div data-testid="syncthing-card" class="px-5 py-4 rounded-2xl bg-[var(--hv-shell-card)] border border-[var(--hv-shell-border)] shadow-[var(--hv-shell-shadow-card)]">
          <div class="flex items-center gap-2 mb-2"><span class="grid place-items-center w-9 h-9 rounded-lg bg-[var(--hv-primary-light)] text-[var(--hv-primary)]"><IconFolder size={18} /></span><span class="flex-1 text-sm font-semibold text-[var(--hv-shell-text-strong)]">{$t('syncthing.stat_folders')}</span><span class={badge}>{translate('syncthing.folders_synced_badge', { synced: String(syncedFolders), total: String(folders.length) })}</span></div>
          <div class="flex flex-col gap-2">
            {#each folders as folder}
              <div data-testid="syncthing-folder-row" class="flex items-center gap-3 px-3 py-2.5 rounded-xl bg-[var(--hv-shell-border-soft)] border border-[var(--hv-shell-border)]">
                <div class="flex-1 min-w-0">
                  <div class="text-sm font-semibold text-[var(--hv-shell-text-strong)] truncate">{folder.label || folder.id}</div>
                  <div class="flex items-center gap-2 mt-0.5">
                    {#if folder.receive_only}<span class={cn(badge, 'bg-[var(--hv-primary-light)] text-[var(--hv-primary)]')}>{$t('syncthing.receive_only_badge')}</span>{/if}
                    <span class="text-xs text-[var(--hv-shell-text-2)]">{translate('syncthing.folder_files_count', { count: String(folder.global_files) })}</span>
                  </div>
                </div>
                <div class="w-32 flex-none">
                  <div class={progressTrack}><div class={cn('h-full rounded-full', syncPercent(folder) === 100 ? 'bg-[var(--hv-success)]' : folder.need_files > 0 && syncPercent(folder) < 80 ? 'bg-[var(--hv-warning)]' : 'bg-[var(--hv-primary)]')} style="width: {syncPercent(folder)}%"></div></div>
                  <span class="block text-right text-c-xs font-bold text-[var(--hv-shell-text-strong)]">{syncPercent(folder)}%</span>
                </div>
                <span class={cn(badge, folder.need_files > 0 ? 'bg-[var(--hv-warning-bg)] text-[var(--hv-warning)]' : 'bg-[var(--hv-success-bg)] text-[var(--hv-success)]')}>{folder.need_files > 0 ? translate('syncthing.folder_pending_badge', { count: String(folder.need_files) }) : $t('syncthing.synced_badge')}</span>
                <button data-testid="syncthing-action-btn" class="grid place-items-center w-9 h-9 flex-none rounded-lg border border-[var(--hv-shell-border-strong)] bg-[var(--hv-shell-fill)] text-[var(--hv-shell-text-strong)] cursor-pointer hover:bg-[var(--hv-shell-border)]" onclick={() => scanFolder(folder.id)} disabled={acting} title={$t('syncthing.scan_folder_title')} aria-label={$t('syncthing.scan_folder_title')}><IconRefresh size={14} /></button>
              </div>
            {/each}
            {#if folders.length === 0}
              <div data-testid="syncthing-empty" class="text-xs text-[var(--hv-shell-text)] py-4 text-center">{$t('syncthing.no_folders')}</div>
            {/if}
          </div>
        </div>
      </div>

    {:else}
    <div data-testid="syncthing-split" bind:this={splitEl} class="flex flex-1 min-h-0">
      {#if !(narrow && selectedDetail)}
        <div data-testid="syncthing-list-pane" class="flex-none min-w-0 border-r border-[var(--hv-shell-border)] overflow-y-auto" style="width: {narrow ? '100%' : '320px'};">
          {#if activeTab === 'devices'}
            <div class="flex flex-col gap-1" data-testid="syncthing-device-list">
              {#each devices as device}
                <button
                  data-testid="syncthing-device-list-row"
                  class:active={selectedDevice?.device_id === device.device_id}
                  aria-current={selectedDevice?.device_id === device.device_id ? 'true' : undefined}
                  onclick={() => { selectedDevice = device; selectedFolder = null; }}
                  class={cn('flex items-center gap-2.5 w-full min-h-[52px] px-2.5 rounded-lg border-none bg-transparent text-left cursor-pointer transition-colors hover:bg-[var(--hv-subtle-hover)]', selectedDevice?.device_id === device.device_id && 'bg-[var(--hv-primary-selected-bg)]')}
                >
                  <span class={cn('grid place-items-center w-9 h-9 rounded-full flex-none', device.connected ? 'bg-[var(--hv-success-bg)] text-[var(--hv-success)]' : device.paused ? 'bg-[var(--hv-warning-bg)] text-[var(--hv-warning)]' : 'bg-[var(--hv-shell-border-soft)] text-[var(--hv-shell-text-2)]')}><IconDeviceMobile size={17} /></span>
                  <span class="flex-1 min-w-0 flex flex-col">
                    <span class="text-c-base font-semibold text-[var(--hv-shell-text-strong)] truncate">{device.name || $t('syncthing.unnamed_device')}</span>
                    <code class="text-c-xs text-[var(--hv-shell-text-2)] font-[family-name:var(--hv-font-mono)] truncate">{shortId(device.device_id)}</code>
                  </span>
                  <span class={cn(badge, device.connected ? 'bg-[var(--hv-success-bg)] text-[var(--hv-success)]' : device.paused ? 'bg-[var(--hv-warning-bg)] text-[var(--hv-warning)]' : 'bg-[var(--hv-error-bg)] text-[var(--hv-error)]')}>{device.connected ? $t('syncthing.connected_badge') : device.paused ? $t('syncthing.paused_badge') : $t('syncthing.disconnected_badge')}</span>
                </button>
              {/each}
              {#if devices.length === 0}
                <div data-testid="syncthing-empty" class="text-xs text-[var(--hv-shell-text)] py-8 text-center">{$t('syncthing.no_devices')}</div>
              {/if}
            </div>
          {:else}
            <div class="flex flex-col gap-1" data-testid="syncthing-folder-list">
              {#each folders as folder}
                <button
                  data-testid="syncthing-folder-list-row"
                  class:active={selectedFolder?.id === folder.id}
                  aria-current={selectedFolder?.id === folder.id ? 'true' : undefined}
                  onclick={() => { selectedFolder = folder; selectedDevice = null; }}
                  class={cn('flex items-center gap-2.5 w-full min-h-[52px] px-2.5 rounded-lg border-none bg-transparent text-left cursor-pointer transition-colors hover:bg-[var(--hv-subtle-hover)]', selectedFolder?.id === folder.id && 'bg-[var(--hv-primary-selected-bg)]')}
                >
                  <span class="grid place-items-center w-9 h-9 rounded-lg flex-none bg-[var(--hv-primary-light)] text-[var(--hv-primary)]"><IconFolder size={17} /></span>
                  <span class="flex-1 min-w-0 flex flex-col">
                    <span class="text-c-base font-semibold text-[var(--hv-shell-text-strong)] truncate">{folder.label || folder.id}</span>
                    <span class="text-c-xs text-[var(--hv-shell-text-2)] truncate">{folder.path}</span>
                  </span>
                  <span class="text-c-xs font-bold text-[var(--hv-shell-text-strong)]">{syncPercent(folder)}%</span>
                </button>
              {/each}
              {#if folders.length === 0}
                <div data-testid="syncthing-empty" class="text-xs text-[var(--hv-shell-text)] py-8 text-center">{$t('syncthing.no_folders')}</div>
              {/if}
            </div>
          {/if}
        </div>
      {/if}

      {#if activeTab === 'devices' && selectedDevice}
        <div data-testid="syncthing-detail-pane" class="flex-1 min-w-0 min-h-0 overflow-y-auto px-5">
          <div class="rounded-2xl bg-[var(--hv-shell-card)] border border-[var(--hv-shell-border)] shadow-[var(--hv-shell-shadow-card)] overflow-hidden">
            <div class="flex items-center gap-3 px-4 pt-4">
              <span class={cn('grid place-items-center w-11 h-11 rounded-full flex-none', selectedDevice.connected ? 'bg-[var(--hv-success-bg)] text-[var(--hv-success)]' : selectedDevice.paused ? 'bg-[var(--hv-warning-bg)] text-[var(--hv-warning)]' : 'bg-[var(--hv-shell-border-soft)] text-[var(--hv-shell-text-2)]')}><IconDeviceMobile size={22} /></span>
              <div class="flex-1 min-w-0">
                <div class="text-c-md font-bold text-[var(--hv-shell-text-strong)] truncate">{selectedDevice.name || $t('syncthing.unnamed_device')}</div>
                <code class="text-c-xs text-[var(--hv-shell-text-2)] font-[family-name:var(--hv-font-mono)] break-all">{selectedDevice.device_id}</code>
              </div>
              <span class={cn(badge, selectedDevice.connected ? 'bg-[var(--hv-success-bg)] text-[var(--hv-success)]' : selectedDevice.paused ? 'bg-[var(--hv-warning-bg)] text-[var(--hv-warning)]' : 'bg-[var(--hv-error-bg)] text-[var(--hv-error)]')}>{selectedDevice.connected ? $t('syncthing.connected_badge') : selectedDevice.paused ? $t('syncthing.paused_badge') : $t('syncthing.disconnected_badge')}</span>
            </div>
            <div class="px-4 py-3 flex flex-col gap-1.5">
              <div class="flex items-center justify-between gap-2"><span class={detailLabel}>{$t('syncthing.address_label')}</span><span class={detailValue}>{selectedDevice.address || '-'}</span></div>
              <div class="flex items-center justify-between gap-2"><span class={detailLabel}>{$t('syncthing.version_label')}</span><span class={detailValue}>{selectedDevice.client_version || '-'}</span></div>
              <div class="flex items-center justify-between gap-2"><span class={detailLabel}>{$t('syncthing.completion_label')}</span><span class={detailValue}>{selectedDevice.completion}%</span></div>
              <div class="flex items-center justify-between gap-2"><span class={detailLabel}>{$t('syncthing.downloaded_label')}</span><span class={detailValue}>{formatBytes(selectedDevice.in_bytes_total)}</span></div>
              <div class="flex items-center justify-between gap-2"><span class={detailLabel}>{$t('syncthing.uploaded_label')}</span><span class={detailValue}>{formatBytes(selectedDevice.out_bytes_total)}</span></div>
            </div>
            <div class="px-4 pb-4">
              <Button variant={selectedDevice.paused ? 'success' : 'warning'} size="sm" data-testid="syncthing-btn-primary" onclick={() => toggleDevice(selectedDevice!)} disabled={acting}>
                {#if selectedDevice.paused}<IconPlayerPlay size={14} /> <span>{$t('syncthing.resume_button')}</span>{:else}<IconPlayerPause size={14} /> <span>{$t('syncthing.pause_button')}</span>{/if}
              </Button>
            </div>
          </div>
        </div>
      {:else if activeTab === 'folders' && selectedFolder}
        <div data-testid="syncthing-detail-pane" class="flex-1 min-w-0 min-h-0 overflow-y-auto px-5">
          <div class="rounded-2xl bg-[var(--hv-shell-card)] border border-[var(--hv-shell-border)] shadow-[var(--hv-shell-shadow-card)] overflow-hidden">
            <div class="flex items-center gap-3 px-4 pt-4">
              <span class="grid place-items-center w-11 h-11 rounded-lg flex-none bg-[var(--hv-primary-light)] text-[var(--hv-primary)]"><IconFolder size={22} /></span>
              <div class="flex-1 min-w-0">
                <div class="text-c-md font-bold text-[var(--hv-shell-text-strong)] truncate">{selectedFolder.label || selectedFolder.id}</div>
                <code class="text-c-xs text-[var(--hv-shell-text-2)] font-[family-name:var(--hv-font-mono)] break-all block">{selectedFolder.path}</code>
              </div>
              {#if selectedFolder.receive_only}<span class={cn(badge, 'bg-[var(--hv-primary-light)] text-[var(--hv-primary)]')}>{$t('syncthing.receive_only_badge')}</span>{/if}
            </div>
            <div class="px-4 py-3">
              <div class="flex items-center justify-between text-xs text-[var(--hv-shell-text-2)]"><span>{$t('syncthing.sync_progress_label')}</span><span class="font-bold text-[var(--hv-shell-text-strong)]">{syncPercent(selectedFolder)}%</span></div>
              <div class={progressTrack}><div class={cn('h-full rounded-full', syncPercent(selectedFolder) === 100 ? 'bg-[var(--hv-success)]' : selectedFolder.need_files > 0 && syncPercent(selectedFolder) < 80 ? 'bg-[var(--hv-warning)]' : 'bg-[var(--hv-primary)]')} style="width: {syncPercent(selectedFolder)}%"></div></div>
              <div class="grid grid-cols-3 gap-2 mt-3">
                <div class="flex flex-col"><span class="text-micro uppercase tracking-wide text-[var(--hv-shell-text-3)]">{$t('syncthing.files_stat_label')}</span><span class="text-sm font-semibold text-[var(--hv-shell-text-strong)]">{selectedFolder.global_files}</span></div>
                <div class="flex flex-col"><span class="text-micro uppercase tracking-wide text-[var(--hv-shell-text-3)]">{$t('syncthing.size_stat_label')}</span><span class="text-sm font-semibold text-[var(--hv-shell-text-strong)]">{formatBytes(selectedFolder.global_bytes)}</span></div>
                <div class="flex flex-col"><span class="text-micro uppercase tracking-wide text-[var(--hv-shell-text-3)]">{$t('syncthing.pending_stat_label')}</span><span class={cn('text-sm font-semibold', selectedFolder.need_files > 0 ? 'text-[var(--hv-warning)]' : 'text-[var(--hv-success)]')}>{#if selectedFolder.need_files > 0}{selectedFolder.need_files} ({formatBytes(selectedFolder.need_bytes)}){:else}{$t('syncthing.none_label')}{/if}</span></div>
              </div>
            </div>
            {#if selectedFolder.devices.length > 0}
              <div class="px-4 pb-3">
                <span class={detailLabel}>{$t('syncthing.shares_devices_label')}</span>
                <div class="flex flex-wrap gap-1.5 mt-1.5">
                  {#each selectedFolder.devices as devId}
                    <span class="inline-flex items-center gap-1.5 px-2.5 py-1 rounded-lg bg-[var(--hv-shell-border-soft)] border border-[var(--hv-shell-border)] text-c-xs font-medium text-[var(--hv-shell-text-strong)]"><IconDeviceMobile size={13} /> {deviceLabel(devId)}</span>
                  {/each}
                </div>
              </div>
            {/if}
            <div class="px-4 pb-4">
              <Button variant="ghost" size="sm" data-testid="syncthing-btn-primary" onclick={() => scanFolder(selectedFolder!.id)} disabled={acting}><IconRefresh size={14} /> {$t('syncthing.scan_button')}</Button>
            </div>
          </div>
        </div>
      {/if}
    </div>
    {/if}
  {/if}

  {#snippet footer()}
    {#if status.available}
      <span data-testid="syncthing-footer" class="flex items-center gap-2 min-w-0 flex-wrap">
        <span>{folders.length} {$t('syncthing.stat_folders')}</span>
        <span class="text-[var(--hv-shell-text-3)]">&middot;</span>
        <span>{devices.length} {$t('syncthing.stat_devices')}</span>
        <span class="text-[var(--hv-shell-text-3)]">&middot;</span>
        <span>{connectedDevices} {$t('syncthing.stat_connected')}</span>
        <span class="text-[var(--hv-shell-text-3)]">&middot;</span>
        <span class={connectedDevices > 0 ? 'text-[var(--hv-success)]' : 'text-[var(--hv-warning)]'}>
          {connectedDevices > 0 ? $t('syncthing.connected_badge') : $t('syncthing.disconnected_badge')}
        </span>
      </span>
    {/if}
  {/snippet}
</PageLayout>
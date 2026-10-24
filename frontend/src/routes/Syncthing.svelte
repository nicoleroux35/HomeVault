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
  import { Spinner, Button, Alert, Modal, Checkbox, PathField } from '$lib/components/theme';
  import { PageLayout, StatCard, EmptyState } from '$lib/components/app';
  import { cn } from '$lib/utils.js';
  import WindowHeader from '$lib/components/desktop/WindowHeader.svelte';
  import { IconRefresh, IconPlayerPause, IconPlayerPlay, IconFolder, IconDeviceMobile, IconServer, IconCircleCheck, IconAlertCircle, IconClock, IconActivity, IconPlus, IconTrash } from '@tabler/icons-svelte';

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
  // One visible reason for every failure this screen can produce. The backend
  // answers `{success:false,error:…}` for a refused action and — since
  // ROADMAP 18.2 — for a `config.xml` whose API key cannot be sent as a
  // header. Before, the three action handlers below had empty `else {}`
  // branches, so a refused pause/scan/restart was indistinguishable from a
  // slow one (simplicity rule 5).
  let errorMessage: string | null = null;
  // A confirmation of the last completed edit. It replaces nothing: an error
  // still wins, and a successful poll never clears it (only the next action
  // does), so a message cannot disappear before it has been read.
  let savedMessage: string | null = null;
  let activeTab: 'overview' | 'devices' | 'folders' = 'overview';
  let refreshInterval: ReturnType<typeof setInterval>;

  // ---------------------------------------------------------------------------
  // Folder / device management (ROADMAP 18.1)
  // ---------------------------------------------------------------------------
  //
  // Until this landed the screen was read-only over the daemon's topology: it
  // could pause a paired device and rescan a folder, but not add either, so a
  // share or a peer could not be onboarded from HomeVault at all.
  //
  // The server owns every rule (ID charset, a path inside a managed storage
  // root, the sync-mode allowlist, a known device ID) and its refusal is shown
  // *inside* the dialog, so the typed values stay in place instead of being
  // thrown away with the dialog.
  let showFolderModal = false;
  let showDeviceModal = false;
  let formError: string | null = null;
  let newFolder = { id: '', label: '', path: '', type: 'sendreceive', devices: [] as string[] };
  let newDevice = { device_id: '', name: '' };
  let removeTarget: { kind: 'folder' | 'device'; id: string; name: string } | null = null;

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
      errorMessage = null;
    } else if (res.error) {
      // The only error `/syncthing/status` reports is a configuration fault
      // (the API key in `config.xml` cannot be sent), so it is shown instead of
      // being swallowed into the "not available" panel.
      errorMessage = res.error;
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
    errorMessage = null;
    savedMessage = null;
    const res = device.paused
      ? await api.resumeSyncthingDevice(device.device_id)
      : await api.pauseSyncthingDevice(device.device_id);
    if (res.success) {
      await loadDevices();
    } else {
      errorMessage = res.error || $t('syncthing.action_failed');
    }
    acting = false;
  }

  async function scanFolder(folderId: string) {
    acting = true;
    errorMessage = null;
    savedMessage = null;
    const res = await api.scanSyncthingFolder(folderId);
    if (res.success) {
      await loadFolders();
    } else {
      errorMessage = res.error || $t('syncthing.action_failed');
    }
    acting = false;
  }

  async function restartSyncthing() {
    acting = true;
    errorMessage = null;
    savedMessage = null;
    const res = await api.restartSyncthing();
    if (res.success) {
      await loadStatus();
    } else {
      errorMessage = res.error || $t('syncthing.action_failed');
    }
    acting = false;
  }

  // --- add a folder -------------------------------------------------------

  function openFolderModal() {
    // Defaults that make the common case one dialog: two-way sync, shared with
    // every device already paired (that is what "sync this folder" means to a
    // home user), no label required.
    newFolder = {
      id: '',
      label: '',
      path: '',
      type: 'sendreceive',
      devices: devices.map((d) => d.device_id),
    };
    formError = null;
    showFolderModal = true;
  }

  function toggleFolderDevice(deviceId: string, checked: boolean) {
    newFolder.devices = checked
      ? [...newFolder.devices, deviceId]
      : newFolder.devices.filter((id) => id !== deviceId);
  }

  async function createFolder() {
    acting = true;
    formError = null;
    // Trim here as well as on the server: the payload is then exactly what the
    // daemon stores, so the success message cannot quote a padded name.
    const created = {
      id: newFolder.id.trim(),
      label: newFolder.label.trim(),
      path: newFolder.path.trim(),
      type: newFolder.type,
      devices: newFolder.devices,
    };
    const res = await api.createSyncthingFolder(created);
    acting = false;
    if (!res.success) {
      // Stay open with the values intact: this is a validation error the user
      // can fix, not a dead end.
      formError = res.error || $t('syncthing.action_failed');
      return;
    }
    showFolderModal = false;
    savedMessage = translate('syncthing.folder_added', { name: created.label || created.id });
    await loadData();
  }

  // --- pair a device ------------------------------------------------------

  function openDeviceModal() {
    newDevice = { device_id: '', name: '' };
    formError = null;
    showDeviceModal = true;
  }

  async function createDevice() {
    acting = true;
    formError = null;
    // Same trim as the folder dialog: the payload is what the daemon stores,
    // and dashes/case are the server's job to normalise.
    const paired = { device_id: newDevice.device_id.trim(), name: newDevice.name.trim() };
    const res = await api.createSyncthingDevice(paired);
    acting = false;
    if (!res.success) {
      formError = res.error || $t('syncthing.action_failed');
      return;
    }
    showDeviceModal = false;
    savedMessage = translate('syncthing.device_added', {
      name: paired.name || paired.device_id,
    });
    await loadData();
  }

  // --- remove -------------------------------------------------------------

  function askRemoveFolder(folder: FolderStatus) {
    removeTarget = { kind: 'folder', id: folder.id, name: folder.label || folder.id };
  }

  function askRemoveDevice(device: DeviceWithStatus) {
    removeTarget = {
      kind: 'device',
      id: device.device_id,
      name: device.name || $t('syncthing.unnamed_device'),
    };
  }

  async function confirmRemove() {
    const target = removeTarget;
    if (!target) return;
    acting = true;
    const res =
      target.kind === 'folder'
        ? await api.deleteSyncthingFolder(target.id)
        : await api.deleteSyncthingDevice(target.id);
    acting = false;
    removeTarget = null;
    if (!res.success) {
      // A removal is destructive, so a refusal is reported on the page (it has
      // no dialog left to stay in) rather than swallowed.
      errorMessage = res.error || $t('syncthing.action_failed');
      return;
    }
    if (target.kind === 'folder') selectedFolder = null;
    else selectedDevice = null;
    savedMessage = translate(
      target.kind === 'folder' ? 'syncthing.folder_removed' : 'syncthing.device_removed',
      { name: target.name },
    );
    await loadData();
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
  // Dialog form styling — the same field/label/input triple the other app
  // dialogs use (Shares, Users), so a form looks the same wherever it opens.
  const field = 'flex flex-col gap-2';
  const fieldLabel = 'text-c-base font-semibold text-[var(--hv-shell-text-strong)]';
  const fieldHelp = 'text-xs text-[var(--hv-shell-text-2)]';
  const inp =
    'w-full box-border h-12 px-3 border border-[var(--hv-shell-border-strong)] rounded-lg bg-[var(--hv-shell-border-soft)] ' +
    'text-[var(--hv-shell-text-strong)] text-c-card-title transition-colors ' +
    'focus-visible:outline-none focus-visible:shadow-[var(--hv-focus-ring)] disabled:opacity-60';
  $: canCreateFolder = newFolder.id.trim().length > 0 && newFolder.path.trim().length > 0 && !acting;
  $: canCreateDevice = newDevice.device_id.trim().length > 0 && !acting;
  $: folderTypeOptions = [
    { value: 'sendreceive', label: $t('syncthing.type_sendreceive') },
    { value: 'sendonly', label: $t('syncthing.type_sendonly') },
    { value: 'receiveonly', label: $t('syncthing.type_receiveonly') },
  ];
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

  {#if errorMessage}
    <div data-testid="syncthing-error" class="mb-4">
      <Alert variant="error" title={$t('syncthing.error_title')}>
        {errorMessage}
      </Alert>
    </div>
  {/if}

  {#if savedMessage}
    <div data-testid="syncthing-success" class="mb-4">
      <Alert variant="success" title={$t('syncthing.success_title')} dismissible ondismiss={() => (savedMessage = null)}>
        {savedMessage}
      </Alert>
    </div>
  {/if}

  {#if loading}
    <div data-testid="syncthing-loading" class="flex items-center justify-center gap-3 py-16 text-[var(--hv-shell-text)]">
      <Spinner />
      <span>{$t('syncthing.loading')}</span>
    </div>
  {:else if !status.available && !errorMessage}
    <!-- Only when the backend actually said "unavailable". A configuration
         fault (a key that cannot be sent) is explained by the banner above;
         claiming "install Syncthing" there would be a lie. -->
    <EmptyState data-testid="syncthing-empty" title={$t('syncthing.not_available_title')} description={$t('syncthing.not_available_desc')}>
      {#snippet iconSnippet()}<IconAlertCircle size={32} stroke={1.8} />{/snippet}
    </EmptyState>
  {:else if status.available}
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
            <div class="flex items-center justify-between gap-2 px-1 py-2">
              <span class="text-c-xs font-bold uppercase tracking-wide text-[var(--hv-shell-text-3)]">{$t('syncthing.stat_devices')}</span>
              <Button variant="ghost" size="sm" data-testid="syncthing-add-device" onclick={openDeviceModal} disabled={acting}>
                <IconPlus size={16} /> <span>{$t('syncthing.add_device')}</span>
              </Button>
            </div>
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
            <div class="flex items-center justify-between gap-2 px-1 py-2">
              <span class="text-c-xs font-bold uppercase tracking-wide text-[var(--hv-shell-text-3)]">{$t('syncthing.stat_folders')}</span>
              <Button variant="ghost" size="sm" data-testid="syncthing-add-folder" onclick={openFolderModal} disabled={acting}>
                <IconPlus size={16} /> <span>{$t('syncthing.add_folder')}</span>
              </Button>
            </div>
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
            <div class="flex flex-wrap items-center gap-2 px-4 pb-4">
              <Button variant={selectedDevice.paused ? 'success' : 'warning'} size="sm" data-testid="syncthing-btn-primary" onclick={() => toggleDevice(selectedDevice!)} disabled={acting}>
                {#if selectedDevice.paused}<IconPlayerPlay size={14} /> <span>{$t('syncthing.resume_button')}</span>{:else}<IconPlayerPause size={14} /> <span>{$t('syncthing.pause_button')}</span>{/if}
              </Button>
              <Button variant="danger" size="sm" data-testid="syncthing-remove-device" onclick={() => askRemoveDevice(selectedDevice!)} disabled={acting}>
                <IconTrash size={14} /> <span>{$t('syncthing.remove_button')}</span>
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
            <div class="flex flex-wrap items-center gap-2 px-4 pb-4">
              <Button variant="ghost" size="sm" data-testid="syncthing-btn-primary" onclick={() => scanFolder(selectedFolder!.id)} disabled={acting}><IconRefresh size={14} /> {$t('syncthing.scan_button')}</Button>
              <Button variant="danger" size="sm" data-testid="syncthing-remove-folder" onclick={() => askRemoveFolder(selectedFolder!)} disabled={acting}><IconTrash size={14} /> {$t('syncthing.remove_button')}</Button>
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

<!-- Add a folder (ROADMAP 18.1) -->
<Modal
  open={showFolderModal}
  title={$t('syncthing.folder_modal_title')}
  size="md"
  onclose={() => { showFolderModal = false; formError = null; }}
>
  {#snippet icon()}<IconFolder size={22} stroke={1.8} />{/snippet}
  {#snippet children()}
    <div class="flex flex-col gap-4">
      {#if formError}
        <div data-testid="syncthing-form-error">
          <Alert variant="error" title={$t('syncthing.action_failed')}>{formError}</Alert>
        </div>
      {/if}

      <label class={field} data-testid="syncthing-folder-id-field">
        <span class={fieldLabel}>{$t('syncthing.folder_id_label')} <span class="text-[var(--hv-error)]" aria-hidden="true">*</span></span>
        <input
          id="hv-syncthing-folder-id"
          data-testid="syncthing-folder-id-input"
          class={inp}
          bind:value={newFolder.id}
          maxlength={64}
          required
          autocomplete="off"
          placeholder="photos"
          aria-describedby="hv-syncthing-folder-id-help"
          onkeydown={(e) => { if (e.key === 'Enter' && canCreateFolder) createFolder(); }}
        />
        <span id="hv-syncthing-folder-id-help" class={fieldHelp}>{$t('syncthing.folder_id_help')}</span>
      </label>

      <label class={field}>
        <span class={fieldLabel}>{$t('syncthing.folder_label_label')}</span>
        <input
          id="hv-syncthing-folder-label"
          data-testid="syncthing-folder-label-input"
          class={inp}
          bind:value={newFolder.label}
          maxlength={128}
          autocomplete="off"
          placeholder={$t('syncthing.optional')}
          onkeydown={(e) => { if (e.key === 'Enter' && canCreateFolder) createFolder(); }}
        />
        <span class={fieldHelp}>{$t('syncthing.folder_label_help')}</span>
      </label>

      <div class={field} data-testid="syncthing-folder-path-field">
        <label class={fieldLabel} for="hv-syncthing-folder-path"
          >{$t('syncthing.folder_path_label')} <span class="text-[var(--hv-error)]" aria-hidden="true">*</span></label
        >
        <!-- PathField (not the read-only PathPickerField) on purpose: Syncthing
             creates the target directory, so a folder that does not exist yet
             is a legitimate answer and the picker cannot express it. -->
        <PathField
          id="hv-syncthing-folder-path"
          bind:value={newFolder.path}
          mode="folder"
          placeholder="/mnt/pools/data/photos"
          required
        />
        <span class={fieldHelp}>{$t('syncthing.folder_path_help')}</span>
      </div>

      <label class={field}>
        <span class={fieldLabel}>{$t('syncthing.folder_type_label')}</span>
        <select id="hv-syncthing-folder-type" data-testid="syncthing-folder-type-select" class={inp} bind:value={newFolder.type}>
          {#each folderTypeOptions as opt}
            <option value={opt.value}>{opt.label}</option>
          {/each}
        </select>
      </label>

      {#if devices.length > 0}
        <div class={field} data-testid="syncthing-folder-devices-field">
          <span class={fieldLabel}>{$t('syncthing.share_with_label')}</span>
          <span class={fieldHelp}>{$t('syncthing.share_with_help')}</span>
          <div class="flex flex-col gap-1">
            {#each devices as device (device.device_id)}
              <Checkbox
                checked={newFolder.devices.includes(device.device_id)}
                onchange={(checked) => toggleFolderDevice(device.device_id, checked)}
                label={device.name || shortId(device.device_id)}
              />
            {/each}
          </div>
        </div>
      {/if}
    </div>
  {/snippet}
  {#snippet footer()}
    <Button variant="ghost" size="lg" onclick={() => (showFolderModal = false)}>{$t('common.cancel')}</Button>
    <Button variant="primary" size="lg" data-testid="syncthing-folder-submit" onclick={createFolder} loading={acting} disabled={!canCreateFolder}>
      {$t('common.create')}
    </Button>
  {/snippet}
</Modal>

<!-- Pair a device (ROADMAP 18.1) -->
<Modal
  open={showDeviceModal}
  title={$t('syncthing.device_modal_title')}
  size="md"
  onclose={() => { showDeviceModal = false; formError = null; }}
>
  {#snippet icon()}<IconDeviceMobile size={22} stroke={1.8} />{/snippet}
  {#snippet children()}
    <div class="flex flex-col gap-4">
      {#if formError}
        <div data-testid="syncthing-form-error">
          <Alert variant="error" title={$t('syncthing.action_failed')}>{formError}</Alert>
        </div>
      {/if}

      <label class={field} data-testid="syncthing-device-id-field">
        <span class={fieldLabel}>{$t('syncthing.device_id_label')} <span class="text-[var(--hv-error)]" aria-hidden="true">*</span></span>
        <input
          id="hv-syncthing-device-id"
          data-testid="syncthing-device-id-input"
          class={cn(inp, 'font-[family-name:var(--hv-font-mono)]')}
          bind:value={newDevice.device_id}
          required
          autocomplete="off"
          spellcheck="false"
          placeholder="AAAAAAA-BBBBBBB-…"
          aria-describedby="hv-syncthing-device-id-help"
          onkeydown={(e) => { if (e.key === 'Enter' && canCreateDevice) createDevice(); }}
        />
        <span id="hv-syncthing-device-id-help" class={fieldHelp}>{$t('syncthing.device_id_help')}</span>
      </label>

      <label class={field}>
        <span class={fieldLabel}>{$t('syncthing.device_name_label')}</span>
        <input
          id="hv-syncthing-device-name"
          data-testid="syncthing-device-name-input"
          class={inp}
          bind:value={newDevice.name}
          maxlength={128}
          autocomplete="off"
          placeholder={$t('syncthing.optional')}
          onkeydown={(e) => { if (e.key === 'Enter' && canCreateDevice) createDevice(); }}
        />
        <span class={fieldHelp}>{$t('syncthing.device_name_help')}</span>
      </label>
    </div>
  {/snippet}
  {#snippet footer()}
    <Button variant="ghost" size="lg" onclick={() => (showDeviceModal = false)}>{$t('common.cancel')}</Button>
    <Button variant="primary" size="lg" data-testid="syncthing-device-submit" onclick={createDevice} loading={acting} disabled={!canCreateDevice}>
      {$t('syncthing.pair_button')}
    </Button>
  {/snippet}
</Modal>

<!-- Remove confirmation: names the consequence, because the action is
     destructive and a home user must know the files stay on disk. -->
<Modal
  open={!!removeTarget}
  title={removeTarget?.kind === 'folder'
    ? translate('syncthing.remove_folder_title', { name: removeTarget?.name ?? '' })
    : translate('syncthing.remove_device_title', { name: removeTarget?.name ?? '' })}
  size="md"
  onclose={() => (removeTarget = null)}
>
  {#snippet icon()}<IconTrash size={22} stroke={1.8} />{/snippet}
  {#snippet children()}
    <p data-testid="syncthing-remove-desc" class="text-sm text-[var(--hv-shell-text)]">
      {removeTarget?.kind === 'folder'
        ? $t('syncthing.remove_folder_desc')
        : $t('syncthing.remove_device_desc')}
    </p>
  {/snippet}
  {#snippet footer()}
    <Button variant="ghost" size="lg" onclick={() => (removeTarget = null)}>{$t('common.cancel')}</Button>
    <Button variant="danger" size="lg" data-testid="syncthing-confirm-remove" onclick={confirmRemove} loading={acting}>
      {$t('syncthing.remove_button')}
    </Button>
  {/snippet}
</Modal>
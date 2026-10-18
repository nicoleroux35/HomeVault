<!-- ========================================================================
     File          : CloudSync.svelte
     Author        : Nicolas Le Roux
     Created       : 28/09/2026
     Last modified : 28/09/2026 by Nicolas Le Roux
     Version       : 1.0.0

     Description:
     The Cloud Sync app: rclone remote management, mount start/stop and
     one-shot copy/sync operations between local folders and cloud remotes.

     Dependencies:
       - svelte (>= 5), theme + app/ composites, $lib/services/api

     Usage:
     Opened as a desktop window app.
     ======================================================================== -->
<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { Icon } from '$lib/components/theme';
  import { api } from '$lib/services/api';
  import { Button, Spinner, Modal } from '$lib/components/theme';
  import { PageLayout, ConfirmDialog, StatCard, SectionCard, EmptyState } from '$lib/components/app';
  import { createConfirmState } from '$lib/utils/confirmDialog.svelte';
  import { cn } from '$lib/utils.js';
  import WindowHeader from '$lib/components/desktop/WindowHeader.svelte';
  import { t, translate } from '$lib/stores/i18n';
  import FolderPicker from '$lib/components/theme/FolderPicker.svelte';
  import { IconCheck, IconCloud, IconCloudOff, IconCloudCheck, IconCloudDownload, IconCloud as CloudSyncIcon, IconCloudUpload, IconDatabase, IconFileCode, IconFolderOpen, IconServer, IconLoader, IconPlus, IconX, IconArrowRight, IconGlobe, IconLink, IconRefresh } from '@tabler/icons-svelte';

  // ---------------------------------------------------------------------------
  // Types
  // ---------------------------------------------------------------------------

  interface Remote {
    name: string;
    type: string;
  }

  interface Provider {
    name: string;
    description: string;
    tier: string;
  }

  interface Mount {
    remote: string;
    mount_point: string;
    path: string | null;
    is_active: boolean;
    pid: number | null;
  }

  interface SyncJob {
    id: number;
    name: string;
    source: string;
    destination: string;
    direction: 'local-to-cloud' | 'cloud-to-local' | 'bidirectional';
    status: 'idle' | 'running' | 'completed' | 'error';
    last_run: string | null;
    stats: {
      files_synced: number;
      bytes_transferred: number;
      errors: number;
    };
  }

  interface RcloneStatus {
    available: boolean;
    fusermount: boolean;
  }

  // ---------------------------------------------------------------------------
  // State
  // ---------------------------------------------------------------------------

  let remotes: Remote[] = [];
  let providers: Provider[] = [];
  let mounts: Mount[] = [];
  let syncJobs: SyncJob[] = [];
  let rcloneStatus: RcloneStatus = { available: false, fusermount: false };

  let loading = true;
  let syncing = false;
  let activeTab: 'remotes' | 'mounts' | 'sync' = 'remotes';
  const dlg = createConfirmState();

  // Modal state
  let showAddRemoteModal = false;
  let showSyncModal = false;
  let showConfigModal = false;

  // Add remote form
  let newRemoteName = '';
  let newRemoteProvider = '';
  let newRemoteConfig = '';

  // Sync form
  let syncSourceType: 'local' | 'cloud' = 'local';
  let syncSourceRemote = '';
  let syncSourcePath = '';
  let syncDestType: 'cloud' | 'local' = 'cloud';
  let syncDestRemote = '';
  let syncDestPath = '';
  let syncDelete = false;
  let syncDryRun = true;

  // Folder picker for local source/destination paths. `syncSourceRemote`/
  // `syncDestRemote` hold a real filesystem path (not an rclone remote
  // name) when the corresponding type is 'local' — the backend's
  // `build_rclone_path` treats it as a literal path prefix in that case.
  let folderPickerTarget: 'source' | 'dest' | null = null;

  function openFolderPicker(target: 'source' | 'dest') {
    folderPickerTarget = target;
  }

  function handleFolderPickerSelect(path: string) {
    if (folderPickerTarget === 'source') syncSourceRemote = path;
    else if (folderPickerTarget === 'dest') syncDestRemote = path;
    folderPickerTarget = null;
  }

  // Config
  let configContent = '';
  let savingConfig = false;

  // Auto-refresh interval
  let refreshInterval: ReturnType<typeof setInterval>;

  // ---------------------------------------------------------------------------
  // Lifecycle
  // ---------------------------------------------------------------------------

  onMount(async () => {
    await loadAll();
    refreshInterval = setInterval(() => {
      loadRemotes();
      loadMounts();
    }, 15000);
  });

  onDestroy(() => {
    if (refreshInterval) clearInterval(refreshInterval);
  });

  // ---------------------------------------------------------------------------
  // Data Loading
  // ---------------------------------------------------------------------------

  async function loadAll() {
    await Promise.all([
      loadRcloneStatus(),
      loadRemotes(),
      loadProviders(),
      loadMounts(),
      loadSyncJobs(),
    ]);
  }

  async function loadRcloneStatus() {
    const response = await api.getRcloneStatus();
    if (response.success && response.data) {
      rcloneStatus = response.data;
    }
  }

  async function loadRemotes() {
    loading = true;
    const response = await api.getCloudRemotes();
    if (response.success && response.data) {
      remotes = response.data;
    } else {
    }
    loading = false;
  }

  async function loadProviders() {
    const response = await api.getCloudProviders();
    if (response.success && response.data) {
      providers = response.data;
    }
  }

  async function loadMounts() {
    const response = await api.getCloudMounts();
    if (response.success && response.data) {
      mounts = response.data;
    }
  }

  async function loadSyncJobs() {
    const response = await api.listSyncJobs();
    if (response.success && Array.isArray(response.data)) {
      syncJobs = response.data.map((r: any) => ({
        id: Number(r.id),
        name: r.name || '',
        source: r.source,
        destination: r.destination,
        direction: r.direction === 'local-to-cloud' ? ('local-to-cloud' as const) : ('cloud-to-local' as const),
        status: 'idle' as const,
        last_run: null,
        stats: { files_synced: 0, bytes_transferred: 0, errors: 0 },
      }));
    } else {
      syncJobs = [];
    }
  }

  async function loadConfig() {
    const response = await api.getCloudConfig();
    if (response.success && response.data) {
      configContent = response.data;
    }
  }

  // ---------------------------------------------------------------------------
  // Remote Actions
  // ---------------------------------------------------------------------------

  async function addRemote() {
    if (!newRemoteName || !newRemoteProvider) {
      return;
    }

    savingConfig = true;
    const response = await api.addCloudRemote({
      name: newRemoteName,
      provider: newRemoteProvider,
      config: newRemoteConfig,
    });

    if (response.success) {
      showAddRemoteModal = false;
      newRemoteName = '';
      newRemoteProvider = '';
      newRemoteConfig = '';
      await loadRemotes();
    } else {
    }
    savingConfig = false;
  }

  async function deleteRemote(name: string) {
    if (!(await dlg.ask(translate('rclone.confirm_delete_remote', { name })))) return;

    const response = await api.deleteCloudRemote(name);
    if (response.success) {
      await loadRemotes();
    } else {
    }
  }

  // ---------------------------------------------------------------------------
  // Mount Actions
  // ---------------------------------------------------------------------------

  async function mountRemote(remote: Remote) {
    const mountPoint = `/mnt/rclone/${remote.name}`;
    const response = await api.mountCloudRemote(remote.name, mountPoint);

    if (response.success) {
      await loadMounts();
    } else {
    }
  }

  async function unmountRemote(mountPoint: string) {
    const response = await api.unmountCloudRemote(mountPoint);

    if (response.success) {
      await loadMounts();
    } else {
    }
  }

  // ---------------------------------------------------------------------------
  // Sync Actions
  // ---------------------------------------------------------------------------

  async function startSync() {
    syncing = true;

    const response = await api.syncCloud({
      source_type: syncSourceType,
      source: syncSourceRemote,
      source_path: syncSourcePath || undefined,
      dest_type: syncDestType,
      destination: syncDestRemote,
      dest_path: syncDestPath || undefined,
      delete: syncDelete,
      dry_run: syncDryRun,
      update: true,
    });

    if (response.success) {
      showSyncModal = false;
    } else {
    }
    syncing = false;
  }

  async function saveSyncJob() {
    const response = await api.createSyncJob({
      name: `job-${Date.now()}`,
      source_type: syncSourceType,
      source: syncSourceRemote,
      source_path: syncSourcePath || undefined,
      dest_type: syncDestType,
      destination: syncDestRemote,
      dest_path: syncDestPath || undefined,
    });
    if (response.success) {
      await loadSyncJobs();
      showSyncModal = false;
    }
  }

  async function runSyncJobById(job: SyncJob) {
    const response = await api.runSyncJob(job.id);
    job.last_run = new Date().toISOString();
    job.status = response.success ? 'completed' : 'error';
    syncJobs = [...syncJobs];
  }

  async function deleteSyncJobById(job: SyncJob) {
    const response = await api.deleteSyncJob(job.id);
    if (response.success) {
      syncJobs = syncJobs.filter((j) => j.id !== job.id);
    }
  }

  // ---------------------------------------------------------------------------
  // Config Actions
  // ---------------------------------------------------------------------------

  async function saveConfig() {
    savingConfig = true;
    const response = await api.saveCloudConfig(configContent);

    if (response.success) {
      showConfigModal = false;
    } else {
    }
    savingConfig = false;
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

  function getProviderIcon(type: string): string {
    const lower = type.toLowerCase();
    if (lower.includes('drive') || lower.includes('google')) return 'brand_google';
    if (lower.includes('dropbox')) return 'brand_dropbox';
    if (lower.includes('onedrive') || lower.includes('microsoft')) return 'brand_windows';
    if (lower.includes('box')) return 'box';
    if (lower.includes('mega')) return 'megaphone';
    if (lower.includes('s3') || lower.includes('aws') || lower.includes('amazon')) return 'cloud';
    if (lower.includes('azure')) return 'brand_azure';
    if (lower.includes('crypt')) return 'lock';
    if (lower.includes('ftp') || lower.includes('sftp')) return 'upload';
    if (lower.includes('webdav')) return 'world';
    return 'cloud';
  }

  function getTierVariant(tier: string): 'success' | 'info' | 'warning' | 'error' {
    switch (tier) {
      case 'Tier 1': return 'success';
      case 'Tier 2': return 'info';
      case 'Tier 3': return 'warning';
      default: return 'error';
    }
  }

  function openAddRemoteModal() {
    newRemoteName = '';
    newRemoteProvider = '';
    newRemoteConfig = '';
    showAddRemoteModal = true;
  }

  function openSyncModal() {
    syncSourceType = 'local';
    syncSourceRemote = '';
    syncSourcePath = '';
    syncDestType = 'cloud';
    syncDestRemote = '';
    syncDestPath = '';
    syncDelete = false;
    syncDryRun = true;
    showSyncModal = true;
  }

  async function openConfigModal() {
    await loadConfig();
    showConfigModal = true;
  }

  // Filter remotes by type
  $: cloudRemotes = remotes.filter(r => !r.type.toLowerCase().includes('local'));
  $: totalRemotes = remotes.length;
  $: activeMounts = mounts.filter(m => m.is_active).length;
  $: totalSyncJobs = syncJobs.length;
  $: rcloneReady = rcloneStatus.available;
  const badge = 'inline-flex items-center gap-1.5 px-2 py-0.5 rounded-full text-c-xs font-bold uppercase';
  const inp = 'w-full h-11 px-3 border border-[var(--hv-shell-border-strong)] rounded-lg bg-[var(--hv-shell-border-soft)] text-sm outline-none focus:border-[var(--hv-primary)]';
  const selectCls = 'w-full h-11 px-3 border border-[var(--hv-shell-border-strong)] rounded-lg bg-[var(--hv-shell-border-soft)] text-sm outline-none focus:border-[var(--hv-primary)]';
  const textareaCls = 'w-full px-3 py-2.5 border border-[var(--hv-shell-border-strong)] rounded-lg bg-[var(--hv-shell-border-soft)] text-sm outline-none focus:border-[var(--hv-primary)] font-[family-name:var(--hv-font-mono)]';
  const fieldLabel = 'text-xs font-semibold text-[var(--hv-shell-text-strong)]';
  const th = 'text-start px-3 py-2 font-semibold text-[var(--hv-shell-text-2)] text-c-xs uppercase tracking-wide border-b border-[var(--hv-shell-border)]';
  const td = 'px-3 py-2 border-b border-[var(--hv-shell-border-soft)] text-sm text-[var(--hv-shell-text-strong)]';
</script>
<PageLayout
  title={$t('rclone.title')}
  data-testid="cloudsync-page"
  tabs={[
    { id: 'remotes', label: $t('rclone.remotes_tab'), icon: IconCloud, count: totalRemotes || undefined },
    { id: 'mounts', label: $t('rclone.tabs.mount'), icon: IconServer, count: mounts.length || undefined },
    { id: 'sync', label: $t('rclone.tabs.sync'), icon: IconRefresh, count: totalSyncJobs || undefined }
  ]}
  bind:activeTab
>
  {#snippet icon()}
    <CloudSyncIcon stroke={1.8} />
  {/snippet}

  <!-- Page-level primary action: a WindowToolbar row under the header, never
       inside it (the header bar is icon + title + window controls only). -->
  {#snippet toolbarRight()}
    <Button variant="primary" onclick={openAddRemoteModal}>
      <IconPlus size={16} stroke={2} />
      <span>{$t('rclone.add_remote')}</span>
    </Button>
  {/snippet}

  {#if loading}
    <div data-testid="cloudsync-loading" class="flex items-center justify-center gap-3 py-16 text-[var(--hv-shell-text)]">
      <Spinner />
      <span>{$t('rclone.loading')}</span>
    </div>
  {:else}
    <div data-testid="cloudsync-stats" class="grid grid-cols-2 @min-[880px]:grid-cols-4 gap-3 mb-5">
      <StatCard data-testid="cloudsync-stat-card" label={$t('rclone.remotes_tab')} value={`${totalRemotes}`} tone={'info'}>
        {#snippet iconSnippet()}<IconCloud size={20} />{/snippet}
      </StatCard>
      <StatCard data-testid="cloudsync-stat-card" label={$t('rclone.mount.active_mounts')} value={`${activeMounts}`} tone={'info'}>
        {#snippet iconSnippet()}<IconServer size={20} />{/snippet}
      </StatCard>
      <StatCard data-testid="cloudsync-stat-card" label={$t('rclone.sync_jobs')} value={`${totalSyncJobs}`} tone={'warning'}>
        {#snippet iconSnippet()}<IconRefresh size={20} />{/snippet}
      </StatCard>
      <StatCard data-testid="cloudsync-stat-card" label={$t('rclone.rclone_status')} value={`${rcloneReady ? $t('rclone.status_ready') : $t('rclone.status_missing')}`} tone={rcloneReady ? 'success' : 'error'}>
        {#snippet iconSnippet()}{#if rcloneReady}<IconCloudCheck size={20} />{:else}<IconCloudOff size={20} />{/if}{/snippet}
      </StatCard>
    </div>

    {#if activeTab === 'remotes'}
      <div class="flex items-center justify-between gap-3 mb-3">
        <div>
          <h2 class="m-0 text-c-card-title font-semibold text-[var(--hv-shell-text-strong)]">{$t('rclone.configured_remotes')}</h2>
          <p class="m-0 mt-0.5 text-xs text-[var(--hv-shell-text)]">{$t('rclone.configured_remotes_desc')}</p>
        </div>
        <Button variant="ghost" onclick={openConfigModal}><IconFileCode size={15} /> {$t('rclone.edit_config')}</Button>
      </div>

      {#if !rcloneReady}
        <SectionCard data-testid="cloudsync-empty">
          <EmptyState title={$t('rclone.errors.not_installed')} description={$t('rclone.not_installed_desc')}>
            {#snippet iconSnippet()}<IconCloudOff size={32} stroke={1.8} />{/snippet}
            {#snippet action()}<Button variant="primary" onclick={openConfigModal}>{$t('rclone.view_config_help')}</Button>{/snippet}
          </EmptyState>
        </SectionCard>
      {:else if remotes.length === 0}
        <SectionCard data-testid="cloudsync-empty">
          <EmptyState title={$t('rclone.no_remotes')} description={$t('rclone.no_remotes_desc')}>
            {#snippet iconSnippet()}<IconCloud size={32} stroke={1.8} />{/snippet}
            {#snippet action()}<Button variant="primary" onclick={openAddRemoteModal}><IconPlus size={15} /> {$t('rclone.add_remote')}</Button>{/snippet}
          </EmptyState>
        </SectionCard>
      {:else}
        <div class="grid gap-4 [grid-template-columns:repeat(auto-fill,minmax(260px,1fr))]">
          {#each remotes as remote}
            <div data-testid="cloudsync-remote-card" class="rounded-2xl bg-[var(--hv-shell-card)] border border-[var(--hv-shell-border)] shadow-[var(--hv-shell-shadow-card)] overflow-hidden">
              <div class="flex items-center gap-3 px-4 pt-4">
                <span class="grid place-items-center w-11 h-11 rounded-xl flex-none bg-[var(--hv-primary-light)] text-[var(--hv-primary)]"><Icon name={getProviderIcon(remote.type)} size={22} /></span>
                <div class="flex-1 min-w-0">
                  <div class="text-sm font-bold text-[var(--hv-shell-text-strong)] truncate">{remote.name}</div>
                  <div class="text-xs text-[var(--hv-shell-text-2)]">{remote.type}</div>
                </div>
                <span class={cn(badge, 'bg-[var(--hv-success-bg)] text-[var(--hv-success)]')}><span class="w-1.5 h-1.5 rounded-full bg-[var(--hv-success)]"></span> {$t('rclone.connected')}</span>
              </div>
              <div class="px-4 py-3 flex gap-2">
                <Button variant="ghost" size="sm" onclick={() => mountRemote(remote)}><IconServer size={14} /> {$t('rclone.tabs.mount')}</Button>
                <Button variant="danger" size="sm" onclick={() => deleteRemote(remote.name)}><IconX size={14} /> {$t('common.delete')}</Button>
              </div>
            </div>
          {/each}
        </div>
      {/if}

      {#if providers.length > 0}
        <SectionCard class="mt-5">
          <div class="mb-3">
            <h3 class="m-0 text-c-card-title font-semibold text-[var(--hv-shell-text-strong)]">{$t('rclone.supported_providers')}</h3>
            <p class="m-0 mt-0.5 text-xs text-[var(--hv-shell-text)]">{$t('rclone.supported_providers_desc')}</p>
          </div>
          <div class="grid gap-2 [grid-template-columns:repeat(auto-fill,minmax(180px,1fr))]">
            {#each providers as provider}
              <div class="flex items-center gap-2 px-3 py-2 rounded-lg bg-[var(--hv-shell-border-soft)] border border-[var(--hv-shell-border)]">
                <Icon name={getProviderIcon(provider.description)} size={16} class="text-[var(--hv-shell-text-2)]" />
                <span class="flex-1 text-xs font-medium text-[var(--hv-shell-text-strong)] truncate">{provider.description}</span>
                <span class={cn(badge, provider.tier === 'Tier 1' ? 'bg-[var(--hv-success-bg)] text-[var(--hv-success)]' : provider.tier === 'Tier 2' ? 'bg-[var(--hv-warning-bg)] text-[var(--hv-warning)]' : 'bg-[var(--hv-shell-border-soft)] text-[var(--hv-shell-text-2)]')}>{provider.tier}</span>
              </div>
            {/each}
          </div>
        </SectionCard>
      {/if}

    {:else if activeTab === 'mounts'}
      <div class="mb-3">
        <h2 class="m-0 text-c-card-title font-semibold text-[var(--hv-shell-text-strong)]">{$t('rclone.mount.active_mounts')}</h2>
        <p class="m-0 mt-0.5 text-xs text-[var(--hv-shell-text)]">{$t('rclone.active_mounts_desc')}</p>
      </div>

      {#if mounts.length === 0}
        <SectionCard data-testid="cloudsync-empty">
          <EmptyState title={$t('rclone.no_active_mounts')} description={$t('rclone.no_active_mounts_desc')}>
            {#snippet iconSnippet()}<IconServer size={32} stroke={1.8} />{/snippet}
            {#snippet action()}<Button variant="primary" onclick={openAddRemoteModal}><IconPlus size={15} /> {$t('rclone.add_remote')}</Button>{/snippet}
          </EmptyState>
        </SectionCard>
      {:else}
        <SectionCard class="overflow-x-auto">
          <table class="w-full border-collapse">
            <thead><tr><th class={th}>{$t('rclone.mount.remote')}</th><th class={th}>{$t('rclone.mount.point')}</th><th class={th}>{$t('rclone.path')}</th><th class={th}>{$t('common.status')}</th><th class={th}>{$t('common.actions')}</th></tr></thead>
            <tbody>
              {#each mounts as mount}
                <tr>
                  <td class={td}><span class="inline-flex items-center gap-2"><IconCloud size={16} class="text-[var(--hv-shell-text-2)]" /><strong>{mount.remote}</strong></span></td>
                  <td class={td}><code class="text-xs font-[family-name:var(--hv-font-mono)]">{mount.mount_point}</code></td>
                  <td class={td}>{#if mount.path}<code class="text-xs font-[family-name:var(--hv-font-mono)]">{mount.path}</code>{:else}<span class="text-xs text-[var(--hv-shell-text-2)]">{$t('rclone.browse.root')}</span>{/if}</td>
                  <td class={td}>{#if mount.is_active}<span class={cn(badge, 'bg-[var(--hv-success-bg)] text-[var(--hv-success)]')}><IconCheck size={12} /> {$t('rclone.mount.mounted')}</span>{:else}<span class={cn(badge, 'bg-[var(--hv-error-bg)] text-[var(--hv-error)]')}><IconX size={12} /> {$t('rclone.mount.not_mounted')}</span>{/if}</td>
                  <td class={td}><div class="flex gap-2">{#if mount.is_active}<Button variant="danger" size="sm" onclick={() => unmountRemote(mount.mount_point)}><IconX size={13} /> {$t('rclone.mount.stop')}</Button>{:else}<Button variant="primary" size="sm" onclick={() => mountRemote({ name: mount.remote, type: 'cloud' })}><IconServer size={13} /> {$t('rclone.mount.start')}</Button>{/if}</div></td>
                </tr>
              {/each}
            </tbody>
          </table>
        </SectionCard>
      {/if}

      <SectionCard class="mt-4">
        <div class="mb-3">
          <h3 class="m-0 text-c-card-title font-semibold text-[var(--hv-shell-text-strong)]">{$t('rclone.mount_instructions')}</h3>
          <p class="m-0 mt-0.5 text-xs text-[var(--hv-shell-text)]">{$t('rclone.mount_instructions_desc')}</p>
        </div>
        <div class="flex flex-col gap-3">
          <div class="flex gap-3">
            <span class="flex-none grid place-items-center w-8 h-8 rounded-full bg-[var(--hv-primary)] text-white text-sm font-bold">1</span>
            <div><h4 class="m-0 text-sm font-semibold text-[var(--hv-shell-text-strong)]">{$t('rclone.instruction_1_title')}</h4><p class="m-0 mt-0.5 text-xs text-[var(--hv-shell-text)]">{$t('rclone.instruction_1_desc')}</p></div>
          </div>
          <div class="flex gap-3">
            <span class="flex-none grid place-items-center w-8 h-8 rounded-full bg-[var(--hv-primary)] text-white text-sm font-bold">2</span>
            <div><h4 class="m-0 text-sm font-semibold text-[var(--hv-shell-text-strong)]">{$t('rclone.instruction_2_title')}</h4><p class="m-0 mt-0.5 text-xs text-[var(--hv-shell-text)]">{$t('rclone.instruction_2_desc')}</p></div>
          </div>
          <div class="flex gap-3">
            <span class="flex-none grid place-items-center w-8 h-8 rounded-full bg-[var(--hv-primary)] text-white text-sm font-bold">3</span>
            <div><h4 class="m-0 text-sm font-semibold text-[var(--hv-shell-text-strong)]">{$t('rclone.instruction_3_title')}</h4><p class="m-0 mt-0.5 text-xs text-[var(--hv-shell-text)]">{$t('rclone.instruction_3_desc')}</p></div>
          </div>
        </div>
      </SectionCard>

    {:else if activeTab === 'sync'}
      <div class="flex items-center justify-between gap-3 mb-3">
        <div>
          <h2 class="m-0 text-c-card-title font-semibold text-[var(--hv-shell-text-strong)]">{$t('rclone.sync_jobs_title')}</h2>
          <p class="m-0 mt-0.5 text-xs text-[var(--hv-shell-text)]">{$t('rclone.sync_jobs_desc')}</p>
        </div>
        <Button variant="primary" onclick={openSyncModal}><IconCloudUpload size={15} /> {$t('rclone.new_sync')}</Button>
      </div>

      {#if syncJobs.length === 0}
        <SectionCard data-testid="cloudsync-empty">
          <EmptyState title={$t('rclone.no_sync_jobs')} description={$t('rclone.no_sync_jobs_desc')}>
            {#snippet iconSnippet()}<IconCloud size={32} stroke={1.8} />{/snippet}
            {#snippet action()}<Button variant="primary" onclick={openSyncModal}><IconCloudUpload size={15} /> {$t('rclone.create_sync_job')}</Button>{/snippet}
          </EmptyState>
        </SectionCard>
      {:else}
        <SectionCard class="overflow-x-auto">
          <table class="w-full border-collapse">
            <thead><tr><th class={th}>{$t('common.name')}</th><th class={th}>{$t('rclone.sync.source')}</th><th class={th}>{$t('rclone.sync.destination')}</th><th class={th}>{$t('rclone.direction')}</th><th class={th}>{$t('rclone.last_run')}</th><th class={th}>{$t('common.status')}</th><th class={th}>{$t('common.actions')}</th></tr></thead>
            <tbody>
              {#each syncJobs as job}
                <tr>
                  <td class={td}><span class="inline-flex items-center gap-2"><IconCloud size={16} class="text-[var(--hv-shell-text-2)]" /><strong>{job.id}</strong></span></td>
                  <td class={td}><code class="text-xs font-[family-name:var(--hv-font-mono)]">{job.source}</code></td>
                  <td class={td}><code class="text-xs font-[family-name:var(--hv-font-mono)]">{job.destination}</code></td>
                  <td class={td}><span class={cn(badge, job.direction === 'local-to-cloud' ? 'bg-[var(--hv-primary-light)] text-[var(--hv-primary)]' : 'bg-[var(--hv-warning-bg)] text-[var(--hv-warning)]')}>{job.direction === 'local-to-cloud' ? $t('rclone.direction_local_to_cloud') : $t('rclone.direction_cloud_to_local')}</span></td>
                  <td class={td}>{#if job.last_run}{new Date(job.last_run).toLocaleString()}{:else}<span class="text-xs text-[var(--hv-shell-text-2)]">{$t('rclone.never')}</span>{/if}</td>
                  <td class={td}><span class={cn(badge, job.status === 'completed' ? 'bg-[var(--hv-success-bg)] text-[var(--hv-success)]' : job.status === 'error' ? 'bg-[var(--hv-error-bg)] text-[var(--hv-error)]' : job.status === 'running' ? 'bg-[var(--hv-warning-bg)] text-[var(--hv-warning)]' : 'bg-[var(--hv-shell-border-soft)] text-[var(--hv-shell-text-2)]')}>{$t(`rclone.job_status_${job.status}`)}</span></td>
                  <td class={td}><div class="flex gap-2"><Button variant="primary" size="sm" onclick={() => runSyncJobById(job)}><IconRefresh size={13} /> {$t('rclone.run')}</Button><Button variant="danger" size="sm" onclick={() => deleteSyncJobById(job)}><IconX size={13} /> {$t('common.delete')}</Button></div></td>
                </tr>
              {/each}
            </tbody>
          </table>
        </SectionCard>
      {/if}

      <SectionCard class="mt-4">
        <div class="mb-3">
          <h3 class="m-0 text-c-card-title font-semibold text-[var(--hv-shell-text-strong)]">{$t('rclone.sync_options')}</h3>
          <p class="m-0 mt-0.5 text-xs text-[var(--hv-shell-text)]">{$t('rclone.sync_options_desc')}</p>
        </div>
        <div class="flex flex-col gap-3">
          <div class="flex items-start gap-3">
            <span class="grid place-items-center w-11 h-11 rounded-xl flex-none bg-[var(--hv-primary-light)] text-[var(--hv-primary)]"><IconCloudUpload size={20} /></span>
            <div><h4 class="m-0 text-sm font-semibold text-[var(--hv-shell-text-strong)]">{$t('rclone.direction_local_to_cloud')}</h4><p class="m-0 mt-0.5 text-xs text-[var(--hv-shell-text)]">{$t('rclone.local_to_cloud_desc')}</p></div>
          </div>
          <div class="flex items-start gap-3">
            <span class="grid place-items-center w-11 h-11 rounded-xl flex-none bg-[var(--hv-success-bg)] text-[var(--hv-success)]"><IconCloudDownload size={20} /></span>
            <div><h4 class="m-0 text-sm font-semibold text-[var(--hv-shell-text-strong)]">{$t('rclone.direction_cloud_to_local')}</h4><p class="m-0 mt-0.5 text-xs text-[var(--hv-shell-text)]">{$t('rclone.cloud_to_local_desc')}</p></div>
          </div>
          <div class="flex items-start gap-3">
            <span class="grid place-items-center w-11 h-11 rounded-xl flex-none bg-[var(--hv-warning-bg)] text-[var(--hv-warning)]"><IconCloud size={20} /></span>
            <div><h4 class="m-0 text-sm font-semibold text-[var(--hv-shell-text-strong)]">{$t('rclone.two_way_sync')}</h4><p class="m-0 mt-0.5 text-xs text-[var(--hv-shell-text)]">{$t('rclone.two_way_sync_desc')}</p></div>
          </div>
        </div>
      </SectionCard>
    {/if}
  {/if}
  <ConfirmDialog
    {dlg}
    onconfirm={() => dlg.settle(true)}
    oncancel={() => dlg.settle(false)}
  />
</PageLayout>

<!-- Add Remote Modal -->
<Modal open={showAddRemoteModal} title={$t('rclone.add_cloud_remote')} size="md" onclose={() => showAddRemoteModal = false}>
  <div class="flex flex-col gap-3">
    <div class="flex flex-col gap-1.5">
      <label for="remote-name" class={fieldLabel}>{$t('rclone.remote_name')} <span class="text-[var(--hv-error)]">*</span></label>
      <input type="text" id="remote-name" class={inp} bind:value={newRemoteName} placeholder="e.g., gdrive, dropbox, s3backup" />
      <span class="text-c-xs text-[var(--hv-shell-text-2)]">{$t('rclone.remote_name_hint')}</span>
    </div>
    <div class="flex flex-col gap-1.5">
      <label for="remote-provider" class={fieldLabel}>{$t('rclone.provider')}</label>
      <select id="remote-provider" class={selectCls} bind:value={newRemoteProvider}>
        <option value="">{$t('rclone.select_provider')}</option>
        {#each providers as provider}
          <option value={provider.name}>{provider.description} ({provider.tier})</option>
        {/each}
      </select>
    </div>
    <div class="flex flex-col gap-1.5">
      <label for="remote-config" class={fieldLabel}>{$t('rclone.configuration_advanced')}</label>
      <textarea id="remote-config" class={textareaCls} bind:value={newRemoteConfig} placeholder={$t('rclone.remote_config_placeholder')} rows="6"></textarea>
      <span class="text-c-xs text-[var(--hv-shell-text-2)]">{$t('rclone.remote_config_hint')}</span>
    </div>
  </div>
  {#snippet footer()}
    <div class="flex gap-2.5 justify-end">
      <Button variant="ghost" onclick={() => showAddRemoteModal = false} disabled={savingConfig}>{$t('common.cancel')}</Button>
      <Button variant="primary" onclick={addRemote} disabled={savingConfig}>{savingConfig ? $t('rclone.adding') : $t('rclone.add_remote')}</Button>
    </div>
  {/snippet}
</Modal>

<!-- Sync Modal -->
<Modal open={showSyncModal} title={$t('rclone.new_sync_job')} size="lg" onclose={() => showSyncModal = false}>
  <div class="grid grid-cols-1 @min-[700px]:grid-cols-[1fr_auto_1fr] gap-4 items-start">
    <div>
      <h3 class="m-0 mb-2 text-sm font-semibold text-[var(--hv-shell-text-strong)] inline-flex items-center gap-1.5"><IconDatabase size={16} /> {$t('rclone.sync.source')}</h3>
      <div class="flex flex-col gap-2.5">
        <div class="flex flex-col gap-1.5">
          <span class={fieldLabel}>{$t('rclone.type_label')}</span>
          <div class="flex gap-3">
            <label class="flex items-center gap-1.5 text-c-label text-[var(--hv-shell-text-strong)] cursor-pointer"><input type="radio" id="source-type-local" name="source-type" value="local" class="accent-[var(--hv-primary)]" bind:group={syncSourceType} /><IconServer size={14} /> <span>{$t('rclone.type.local')}</span></label>
            <label class="flex items-center gap-1.5 text-c-label text-[var(--hv-shell-text-strong)] cursor-pointer"><input type="radio" id="source-type-cloud" name="source-type" value="cloud" class="accent-[var(--hv-primary)]" bind:group={syncSourceType} /><IconCloud size={14} /> <span>{$t('rclone.type.cloud')}</span></label>
          </div>
        </div>
        {#if syncSourceType === 'local'}
          <div class="flex flex-col gap-1.5">
            <label for="sync-source-remote" class={fieldLabel}>{$t('rclone.local_path')}</label>
            <div class="flex gap-2">
              <input type="text" id="sync-source-remote" class={inp} bind:value={syncSourceRemote} placeholder="/mnt/data" readonly />
              <Button type="button" variant="ghost" size="sm" onclick={() => openFolderPicker('source')}><IconFolderOpen size={14} /> {$t('rclone.browse_button')}</Button>
            </div>
          </div>
        {:else}
          <div class="flex flex-col gap-1.5">
            <label for="sync-source-remote" class={fieldLabel}>{$t('rclone.remote')}</label>
            <select id="sync-source-remote" class={selectCls} bind:value={syncSourceRemote}><option value="">{$t('rclone.select_remote')}</option>{#each cloudRemotes as remote}<option value={remote.name}>{remote.name}</option>{/each}</select>
          </div>
        {/if}
        <div class="flex flex-col gap-1.5">
          <label for="sync-source-path" class={fieldLabel}>{$t('rclone.path_optional')}</label>
          <input type="text" id="sync-source-path" class={inp} bind:value={syncSourcePath} placeholder="/folder/path" />
        </div>
      </div>
    </div>
    <div class="justify-self-center self-center text-[var(--hv-shell-text-3)]"><IconArrowRight size={28} /></div>
    <div>
      <h3 class="m-0 mb-2 text-sm font-semibold text-[var(--hv-shell-text-strong)] inline-flex items-center gap-1.5"><IconCloud size={16} /> {$t('rclone.sync.destination')}</h3>
      <div class="flex flex-col gap-2.5">
        <div class="flex flex-col gap-1.5">
          <span class={fieldLabel}>{$t('rclone.type_label')}</span>
          <div class="flex gap-3">
            <label class="flex items-center gap-1.5 text-c-label text-[var(--hv-shell-text-strong)] cursor-pointer"><input type="radio" id="dest-type-cloud" name="dest-type" value="cloud" class="accent-[var(--hv-primary)]" bind:group={syncDestType} /><IconCloud size={14} /> <span>{$t('rclone.type.cloud')}</span></label>
            <label class="flex items-center gap-1.5 text-c-label text-[var(--hv-shell-text-strong)] cursor-pointer"><input type="radio" id="dest-type-local" name="dest-type" value="local" class="accent-[var(--hv-primary)]" bind:group={syncDestType} /><IconServer size={14} /> <span>{$t('rclone.type.local')}</span></label>
          </div>
        </div>
        {#if syncDestType === 'local'}
          <div class="flex flex-col gap-1.5">
            <label for="sync-dest-remote" class={fieldLabel}>{$t('rclone.local_path')}</label>
            <div class="flex gap-2">
              <input type="text" id="sync-dest-remote" class={inp} bind:value={syncDestRemote} placeholder="/mnt/data" readonly />
              <Button type="button" variant="ghost" size="sm" onclick={() => openFolderPicker('dest')}><IconFolderOpen size={14} /> {$t('rclone.browse_button')}</Button>
            </div>
          </div>
        {:else}
          <div class="flex flex-col gap-1.5">
            <label for="sync-dest-remote" class={fieldLabel}>{$t('rclone.remote')}</label>
            <select id="sync-dest-remote" class={selectCls} bind:value={syncDestRemote}><option value="">{$t('rclone.select_remote')}</option>{#each cloudRemotes as remote}<option value={remote.name}>{remote.name}</option>{/each}</select>
          </div>
        {/if}
        <div class="flex flex-col gap-1.5">
          <label for="sync-dest-path" class={fieldLabel}>{$t('rclone.path_optional')}</label>
          <input type="text" id="sync-dest-path" class={inp} bind:value={syncDestPath} placeholder="/folder/path" />
        </div>
      </div>
    </div>
  </div>
  <div class="mt-4 pt-3 border-t border-[var(--hv-shell-border-soft)]">
    <h4 class="m-0 mb-2 text-sm font-semibold text-[var(--hv-shell-text-strong)]">{$t('rclone.sync_options')}</h4>
    <div class="flex gap-4">
      <label class="flex items-center gap-1.5 text-c-label text-[var(--hv-shell-text-strong)] cursor-pointer"><input type="checkbox" class="accent-[var(--hv-primary)]" bind:checked={syncDelete} /> <span>{$t('rclone.sync.delete_extra')}</span></label>
      <label class="flex items-center gap-1.5 text-c-label text-[var(--hv-shell-text-strong)] cursor-pointer"><input type="checkbox" class="accent-[var(--hv-primary)]" bind:checked={syncDryRun} /> <span>{$t('rclone.dry_run_hint')}</span></label>
    </div>
  </div>
  {#snippet footer()}
    <div class="flex gap-2.5 justify-end">
      <Button variant="ghost" onclick={() => showSyncModal = false} disabled={syncing}>{$t('common.cancel')}</Button>
      <Button variant="ghost" onclick={saveSyncJob} disabled={syncing || !syncSourceRemote || !syncDestRemote}>{$t('rclone.save_job')}</Button>
      <Button variant="primary" onclick={startSync} disabled={syncing || !syncSourceRemote || !syncDestRemote}>{#if syncing}{$t('rclone.syncing')}{:else if syncDryRun}{$t('rclone.preview_sync')}{:else}{$t('rclone.start_sync')}{/if}</Button>
    </div>
  {/snippet}
</Modal>

<!-- Config Modal -->
<Modal open={showConfigModal} title={$t('rclone.settings.title')} size="lg" onclose={() => showConfigModal = false}>
  <div class="mb-3 px-3 py-2.5 rounded-lg bg-[var(--hv-warning-light)] border border-[var(--hv-warning)]/40 text-xs text-[var(--hv-warning-text)]">{$t('rclone.config_edit_warning')}</div>
  <textarea class={cn(textareaCls, 'min-h-[360px]')} bind:value={configContent} placeholder="[remote_name]
type = google drive
..." rows="20"></textarea>
  {#snippet footer()}
    <div class="flex gap-2.5 justify-end">
      <Button variant="ghost" onclick={() => showConfigModal = false} disabled={savingConfig}>{$t('common.cancel')}</Button>
      <Button variant="primary" onclick={saveConfig} disabled={savingConfig}>{savingConfig ? $t('rclone.saving') : $t('rclone.save_configuration')}</Button>
    </div>
  {/snippet}
</Modal>

<FolderPicker open={folderPickerTarget !== null} onselect={handleFolderPickerSelect} onclose={() => folderPickerTarget = null} />
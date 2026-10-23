<!-- ========================================================================
     File          : Downloader.svelte
     Author        : Nicolas Le Roux
     Created       : 28/09/2026
     Last modified : 23/10/2026 by Nicolas Le Roux
     Version       : 1.1.0

     Description:
     The Downloader app: aria2-backed download queue — add URLs/torrents,
     pause/resume/remove, per-task and global statistics, service control.

     Dependencies:
       - svelte (>= 5), theme + app/ composites, $lib/services/api

     Usage:
     Opened as a desktop window app (admin only).
     ======================================================================== -->
<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { api } from '$lib/services/api';
  import { t, translate } from '$lib/stores/i18n';
  import { Spinner, Button, Alert } from '$lib/components/theme';
  import { PageLayout, iconBtn, iconBtnDanger, StatCard } from '$lib/components/app';
  import { cn } from '$lib/utils.js';
  import { IconDownload, IconLink, IconPlayerPlay, IconPlayerPause, IconTrash, IconLoader, IconArrowDown, IconArrowUp, IconFile, IconCircleCheck, IconCircleX, IconClock, IconAlertCircle, IconActivity, IconPower } from '@tabler/icons-svelte';

  interface Aria2Task {
    gid: string;
    status: string;
    total_length: string;
    completed_length: string;
    download_speed: string;
    upload_speed: string;
    error_message: string | null;
    bittorrent: { name: string } | null;
    files: Aria2File[];
    dir: string;
    uris?: Array<{ uri: string; status: string }>;
  }

  interface Aria2File {
    index: string;
    path: string;
    length: string;
    completed_length: string;
    selected: string;
    uris: Array<{ uri: string; status: string }>;
  }

  interface GlobalStat {
    download_speed: string;
    upload_speed: string;
    num_active: string;
    num_waiting: string;
    num_stopped: string;
  }

  let tasks: Aria2Task[] = $state([]);
  let globalStat: GlobalStat | null = $state(null);
  let loading = $state(true);
  let aria2Available = $state(false);
  let aria2Running = $state(false);
  let newUrl = $state('');
  let subfolder = $state('');
  /** Absolute folder the server writes downloads to (reported by `/status`). */
  let downloadRoot = $state('');
  let showAddForm = $state(false);
  let addingUrl = $state(false);
  let startingService = $state(false);
  /** Last failed action, shown in a dismissible alert (never swallowed). */
  let actionError = $state<string | null>(null);
  let pollInterval: ReturnType<typeof setInterval> | null = null;

  onMount(async () => {
    await checkStatus();
    // The daemon is never started behind the user's back: opening the app used
    // to spawn aria2c with no consent (ROADMAP 17). The start panel below owns
    // that decision.
    if (aria2Running) {
      await loadData();
      startPolling();
    }
    loading = false;

    // Master-detail split: `narrow` measures the split container itself (the
    // window floats, so a viewport media query would lie). Below 720px only
    // one pane shows.
    splitRO = new ResizeObserver((entries) => {
      narrow = (entries[0]?.contentRect.width ?? 0) < 720;
    });
    if (splitEl) splitRO.observe(splitEl);
  });

  onDestroy(() => {
    stopPolling();
    splitRO?.disconnect();
  });

  function startPolling() {
    if (!pollInterval) pollInterval = setInterval(loadData, 2000);
  }

  function stopPolling() {
    if (pollInterval) clearInterval(pollInterval);
    pollInterval = null;
  }

  async function checkStatus() {
    const res = await api.getDownloaderStatus();
    if (res.success && res.data) {
      aria2Available = res.data.available;
      aria2Running = res.data.running;
      downloadRoot = res.data.dir ?? '';
    } else if (res.error) {
      actionError = res.error;
    }
  }

  async function startAria2() {
    actionError = null;
    startingService = true;
    const res = await api.startDownloader();
    startingService = false;
    if (!res.success) {
      actionError = res.error || translate('downloader.action_failed');
      return;
    }
    aria2Running = true;
    startPolling();
    await loadData();
  }

  async function loadData() {
    const [statRes, tasksRes] = await Promise.all([
      api.getDownloaderGlobalStat(),
      api.getDownloaderTasks(),
    ]);
    if (statRes.success && statRes.data) {
      globalStat = statRes.data;
    } else if (statRes.error) {
      actionError = statRes.error;
    }
    if (tasksRes.success && tasksRes.data) {
      tasks = tasksRes.data;
    } else if (tasksRes.error) {
      actionError = tasksRes.error;
    }
  }

  async function addUrl() {
    if (!newUrl.trim()) return;
    addingUrl = true;
    actionError = null;
    const res = await api.addDownloaderUrl(newUrl.trim(), subfolder.trim() || undefined);
    if (res.success) {
      newUrl = '';
      subfolder = '';
      showAddForm = false;
      await loadData();
    } else {
      actionError = res.error || translate('downloader.action_failed');
    }
    addingUrl = false;
  }

  async function pauseTask(gid: string) {
    actionError = null;
    const res = await api.pauseDownloader(gid);
    if (res.success) {
      await loadData();
    } else {
      actionError = res.error || translate('downloader.action_failed');
    }
  }

  async function unpauseTask(gid: string) {
    actionError = null;
    const res = await api.unpauseDownloader(gid);
    if (res.success) {
      await loadData();
    } else {
      actionError = res.error || translate('downloader.action_failed');
    }
  }

  async function removeTask(gid: string) {
    actionError = null;
    const res = await api.removeDownloader(gid);
    if (res.success) {
      await loadData();
    } else {
      actionError = res.error || translate('downloader.action_failed');
    }
  }

  function formatBytes(bytes: string): string {
    const n = parseInt(bytes);
    if (isNaN(n) || n === 0) return '0 B';
    const units = ['B', 'KB', 'MB', 'GB', 'TB'];
    const i = Math.floor(Math.log(n) / Math.log(1024));
    return (n / Math.pow(1024, i)).toFixed(1) + ' ' + units[i];
  }

  function formatSpeed(speed: string): string {
    const n = parseInt(speed);
    if (isNaN(n) || n === 0) return '--';
    if (n < 1024) return n + ' B/s';
    if (n < 1024 * 1024) return (n / 1024).toFixed(1) + ' KB/s';
    return (n / (1024 * 1024)).toFixed(2) + ' MB/s';
  }

  function getProgress(task: Aria2Task): number {
    const total = parseInt(task.total_length);
    const done = parseInt(task.completed_length);
    if (total === 0) return 0;
    return Math.min(100, Math.round((done / total) * 100));
  }

  function statusLabel(status: string): string {
    const key = `downloader.status_${status}`;
    const translated = translate(key);
    return translated === key ? status : translated;
  }

  function statusIcon(status: string) {
    if (status === 'active') return IconLoader;
    if (status === 'complete') return IconCircleCheck;
    if (status === 'error') return IconCircleX;
    if (status === 'paused') return IconClock;
    return IconAlertCircle;
  }

  function statusColor(status: string): string {
    const colors: Record<string, string> = {
      active: 'var(--hv-info)',
      waiting: 'var(--hv-text-tertiary)',
      paused: 'var(--hv-warning)',
      error: 'var(--hv-error)',
      complete: 'var(--hv-success)',
      removed: 'var(--hv-text-tertiary)',
    };
    return colors[status] || 'var(--hv-text-secondary)';
  }

  function fileName(task: Aria2Task): string {
    if (task.bittorrent?.name) return task.bittorrent.name;
    if (task.files?.length) {
      const path = task.files[0].path;
      const parts = path.split('/');
      return parts[parts.length - 1] || path;
    }
    return task.gid;
  }

  let activeTasks: Aria2Task[] = $state([]);
  let completedTasks: Aria2Task[] = $state([]);

  // Master-detail split — task list left, selected-task detail right. The
  // selection is tracked by gid and re-resolved on every poll so the detail
  // pane tracks a task's live progress instead of a stale snapshot.
  let selectedTaskGid = $state<string | null>(null);
  let selectedTask = $derived(
    [...activeTasks, ...completedTasks].find((t) => t.gid === selectedTaskGid) ?? null,
  );
  let splitEl = $state<HTMLDivElement | null>(null);
  let narrow = $state(false);
  let splitRO: ResizeObserver | null = null;
  $effect(() => {
    activeTasks = tasks.filter(t => t.status === 'active' || t.status === 'waiting' || t.status === 'paused');
    completedTasks = tasks.filter(t => t.status === 'complete' || t.status === 'error' || t.status === 'removed');
  });
  // Shared control shapes (iconBtn / iconBtnDanger) come from
  // components/app controlClasses.ts — see docs/guides/design-system.md §4.3.
  const progressTrack = 'h-2 rounded-full bg-[var(--hv-shell-border-soft)] overflow-hidden';
</script>
<PageLayout
  title={$t('downloader.title')}
  data-testid="downloader-page"
  footerMode="status"
>
  {#snippet icon()}
    <IconDownload stroke={1.8} />
  {/snippet}

  <!-- Page-level primary action: a WindowToolbar row under the header, never
       inside it (the header bar is icon + title + window controls only). -->
  {#snippet toolbarRight()}
    <!-- Only offered when the service is running: the form cannot do anything
         without it, and adding a link would start a background process the
         user has not asked for. -->
    {#if aria2Available && aria2Running}
      <Button variant="primary" data-testid="downloader-add-toggle" onclick={() => (showAddForm = !showAddForm)}>
        <IconLink size={16} />
        <span>{$t('downloader.add_link')}</span>
      </Button>
    {/if}
  {/snippet}

  {#if loading}
    <div data-testid="downloader-loading" class="flex items-center justify-center gap-3 py-16 text-[var(--hv-shell-text)]">
      <Spinner />
      <span>{$t('downloader.loading')}</span>
    </div>
  {:else if !aria2Available}
    <div data-testid="downloader-not-available" class="flex flex-col items-center gap-3 py-16 text-center">
      <IconCircleX size={48} class="text-[var(--hv-shell-border-strong)]" />
      <h2 class="m-0 text-lg font-semibold text-[var(--hv-shell-text-strong)]">{$t('downloader.not_installed_title')}</h2>
      <p class="m-0 text-sm text-[var(--hv-shell-text)]">{$t('downloader.not_installed_desc')}</p>
      <code class="px-3 py-1.5 rounded-lg bg-[var(--hv-shell-border-soft)] border border-[var(--hv-shell-border)] text-xs font-[family-name:var(--hv-font-mono)]">sudo apt install aria2</code>
    </div>
  {:else}
    {#if actionError}
      <div data-testid="downloader-error" class="mb-4">
        <Alert variant="error" title={$t('downloader.action_failed')} dismissible ondismiss={() => (actionError = null)}>
          {actionError}
        </Alert>
      </div>
    {/if}

    {#if !aria2Running}
      <div data-testid="downloader-service-stopped" class="flex flex-col items-center gap-3 py-16 text-center">
        <IconPower size={44} class="text-[var(--hv-shell-border-strong)]" />
        <h2 class="m-0 text-lg font-semibold text-[var(--hv-shell-text-strong)]">{$t('downloader.service_stopped_title')}</h2>
        <p class="m-0 max-w-[46ch] text-sm text-[var(--hv-shell-text)]">{$t('downloader.service_stopped_desc')}</p>
        <Button variant="primary" data-testid="downloader-start" onclick={startAria2} disabled={startingService}>
          {#if startingService}<IconLoader size={16} class="dl-spin" />{:else}<IconPower size={16} />{/if}
          <span>{$t('downloader.start_service')}</span>
        </Button>
      </div>
    {:else}
    {#if globalStat}
      <div data-testid="downloader-stats" class="grid grid-cols-2 @min-[880px]:grid-cols-4 gap-3 mb-5">
        <StatCard data-testid="downloader-stat-card" label={$t('downloader.stat_active')} value={`${globalStat.num_active}`} tone={'info'}>
          {#snippet iconSnippet()}<IconActivity size={20} />{/snippet}
        </StatCard>
        <StatCard data-testid="downloader-stat-card" label={$t('downloader.stat_speed_down')} value={`${formatSpeed(globalStat.download_speed)}`} tone={'success'}>
          {#snippet iconSnippet()}<IconArrowDown size={20} />{/snippet}
        </StatCard>
        <StatCard data-testid="downloader-stat-card" label={$t('downloader.stat_speed_up')} value={`${formatSpeed(globalStat.upload_speed)}`} tone={'info'}>
          {#snippet iconSnippet()}<IconArrowUp size={20} />{/snippet}
        </StatCard>
        <StatCard data-testid="downloader-stat-card" label={$t('downloader.completed')} value={`${completedTasks.length}`} tone={'success'}>
          {#snippet iconSnippet()}<IconCircleCheck size={20} />{/snippet}
        </StatCard>
      </div>
    {/if}

    {#if showAddForm}
      <div data-testid="downloader-add-form" class="px-4 py-3 rounded-2xl bg-[var(--hv-shell-card)] border border-[var(--hv-shell-border)] shadow-[var(--hv-shell-shadow-card)] mb-4">
        <div class="flex flex-wrap gap-2.5 items-center">
          <input
            data-testid="downloader-input"
            type="text"
            aria-label={$t('downloader.url_label')}
            class="flex-1 min-w-[200px] h-11 px-3 border border-[var(--hv-shell-border-strong)] rounded-lg bg-[var(--hv-shell-border-soft)] text-sm outline-none focus:border-[var(--hv-primary)]"
            placeholder={$t('downloader.url_placeholder')}
            bind:value={newUrl}
            onkeydown={(e) => e.key === 'Enter' && addUrl()}
          />
          <input
            data-testid="downloader-subfolder"
            type="text"
            aria-label={$t('downloader.subfolder_label')}
            class="w-[190px] h-11 px-3 border border-[var(--hv-shell-border-strong)] rounded-lg bg-[var(--hv-shell-border-soft)] text-sm outline-none focus:border-[var(--hv-primary)]"
            placeholder={$t('downloader.subfolder_placeholder')}
            bind:value={subfolder}
            onkeydown={(e) => e.key === 'Enter' && addUrl()}
          />
          <Button variant="primary" data-testid="downloader-btn-primary" onclick={addUrl} disabled={addingUrl || !newUrl.trim()}>
            {#if addingUrl}<IconLoader size={16} class="dl-spin" />{:else}<IconDownload size={16} />{/if}
            <span>{$t('downloader.download_button')}</span>
          </Button>
        </div>
        {#if downloadRoot}
          <p data-testid="downloader-destination" class="mt-2 mb-0 text-xs text-[var(--hv-shell-text-2)]">
            {$t('downloader.destination_root', { path: downloadRoot })}
          </p>
        {/if}
      </div>
    {/if}

    <div data-testid="downloader-split" bind:this={splitEl} class="flex flex-1 min-h-0">
      {#if !(narrow && selectedTask)}
        <div data-testid="downloader-list-pane" class="flex-none min-w-0 border-r border-[var(--hv-shell-border)] overflow-y-auto" style="width: {narrow ? '100%' : '360px'};">
          <div class="mb-5">
            <div class="flex items-center gap-2 mb-3">
              <h3 data-testid="downloader-section-title" class="m-0 text-c-card-title font-semibold text-[var(--hv-shell-text-strong)] inline-flex items-center gap-2"><IconDownload size={18} class="text-[var(--hv-primary)]" /> <span>{$t('downloader.active_downloads_title')}</span></h3>
              <span class="min-w-5 h-5 px-1.5 grid place-items-center rounded-full bg-[var(--hv-primary-light)] text-[var(--hv-primary)] text-c-xs font-bold">{activeTasks.length}</span>
            </div>
            {#if activeTasks.length === 0}
              <div class="flex flex-col items-center gap-3 py-10 text-center text-[var(--hv-shell-text)]">
                <IconDownload size={40} class="text-[var(--hv-shell-border-strong)]" />
                <div class="text-base font-semibold text-[var(--hv-shell-text-2)]">{$t('downloader.no_active_title')}</div>
                <div class="text-sm text-[var(--hv-shell-text)]">{$t('downloader.no_active_desc')}</div>
              </div>
            {:else}
              <div class="flex flex-col gap-2">
                {#each activeTasks as task (task.gid)}
                  <div
                    data-testid="downloader-card"
                    class={cn('px-3 py-2.5 rounded-xl bg-[var(--hv-shell-card)] border transition-colors hover:border-[var(--hv-primary)]/40', selectedTask?.gid === task.gid ? 'border-[var(--hv-primary)]' : 'border-[var(--hv-shell-border)]')}
                  >
                    <!-- The card is a container, not a button: the selectable
                         part is a real <button> so it never wraps the action
                         buttons (WCAG 4.1.2 — no nested interactive controls). -->
                    <button
                      type="button"
                      data-testid="downloader-card-select"
                      aria-current={selectedTask?.gid === task.gid ? 'true' : undefined}
                      class="flex w-full items-center justify-between gap-3 bg-transparent border-0 p-0 text-left cursor-pointer rounded-lg"
                      onclick={() => { selectedTaskGid = task.gid; }}
                    >
                      <div class="flex items-center gap-2.5 min-w-0">
                        <IconFile size={16} class="text-[var(--hv-shell-text-2)] flex-none" />
                        <div class="min-w-0">
                          <div data-testid="downloader-card-name" class="text-sm font-bold text-[var(--hv-shell-text-strong)] truncate">{fileName(task)}</div>
                          <div class="text-xs text-[var(--hv-shell-text-2)] truncate">{statusLabel(task.status)}</div>
                        </div>
                      </div>
                      <span class="text-c-xs font-bold text-[var(--hv-shell-text-strong)] flex-none">{getProgress(task)}%</span>
                    </button>
                    <div class="mt-2 flex items-center gap-2">
                      <div class={progressTrack}><div class={cn('h-full rounded-full', task.status === 'complete' ? 'bg-[var(--hv-success)]' : 'bg-[var(--hv-primary)]')} style="width: {getProgress(task)}%"></div></div>
                      <div class="flex gap-1 flex-none">
                        {#if task.status === 'paused'}
                          <button type="button" data-testid="downloader-action" class={iconBtn} onclick={() => unpauseTask(task.gid)} title={$t('downloader.resume_title')} aria-label={$t('downloader.resume_title')}><IconPlayerPlay size={13} /></button>
                        {:else if task.status === 'active' || task.status === 'waiting'}
                          <button type="button" data-testid="downloader-action" class={iconBtn} onclick={() => pauseTask(task.gid)} title={$t('downloader.pause_title')} aria-label={$t('downloader.pause_title')}><IconPlayerPause size={13} /></button>
                        {/if}
                        <button type="button" data-testid="downloader-action-danger" class={iconBtnDanger} onclick={() => removeTask(task.gid)} title={$t('downloader.delete_title')} aria-label={$t('downloader.delete_title')}><IconTrash size={13} /></button>
                      </div>
                    </div>
                  </div>
                {/each}
              </div>
            {/if}
          </div>

          {#if completedTasks.length > 0}
            <div>
              <div class="flex items-center gap-2 mb-3">
                <h3 data-testid="downloader-section-title" class="m-0 text-c-card-title font-semibold text-[var(--hv-shell-text-strong)] inline-flex items-center gap-2"><IconCircleCheck size={18} class="text-[var(--hv-success)]" /> <span>{$t('downloader.completed')}</span></h3>
                <span class="min-w-5 h-5 px-1.5 grid place-items-center rounded-full bg-[var(--hv-success-bg)] text-[var(--hv-success)] text-c-xs font-bold">{completedTasks.length}</span>
              </div>
              <div class="flex flex-col gap-2">
                {#each completedTasks as task (task.gid)}
                  <div
                    data-testid="downloader-card"
                    class={cn('flex items-center gap-3 px-3 py-2.5 rounded-xl bg-[var(--hv-shell-card)] border transition-colors hover:border-[var(--hv-primary)]/40', selectedTask?.gid === task.gid ? 'border-[var(--hv-primary)]' : 'border-[var(--hv-shell-border)]')}
                  >
                    <button
                      type="button"
                      data-testid="downloader-card-select"
                      aria-current={selectedTask?.gid === task.gid ? 'true' : undefined}
                      class="flex flex-1 min-w-0 items-center justify-between gap-3 bg-transparent border-0 p-0 text-left cursor-pointer rounded-lg"
                      onclick={() => { selectedTaskGid = task.gid; }}
                    >
                      <div class="flex items-center gap-2.5 min-w-0">
                        <IconFile size={16} class="text-[var(--hv-shell-text-2)] flex-none" />
                        <div class="min-w-0">
                          <div data-testid="downloader-card-name" class="text-sm font-bold text-[var(--hv-shell-text-strong)] truncate">{fileName(task)}</div>
                          <div class="text-xs text-[var(--hv-shell-text-2)] truncate">{formatBytes(task.total_length)}</div>
                        </div>
                      </div>
                      <span class={cn('px-2 py-0.5 rounded-full text-c-xs font-bold uppercase flex-none', task.status === 'complete' ? 'bg-[var(--hv-success-bg)] text-[var(--hv-success)]' : 'bg-[var(--hv-error-bg)] text-[var(--hv-error)]')}>{statusLabel(task.status)}</span>
                    </button>
                    <button type="button" data-testid="downloader-action-danger" class={iconBtnDanger} onclick={() => removeTask(task.gid)} title={$t('downloader.delete_title')} aria-label={$t('downloader.delete_title')}><IconTrash size={13} /></button>
                  </div>
                {/each}
              </div>
            </div>
          {/if}
        </div>
      {/if}

      {#if selectedTask}
        <div data-testid="downloader-detail-pane" class="flex-1 min-w-0 min-h-0 overflow-y-auto px-5">
          <div class="px-4 py-4 rounded-2xl bg-[var(--hv-shell-card)] border border-[var(--hv-shell-border)] shadow-[var(--hv-shell-shadow-card)]">
            <div class="flex items-center justify-between gap-3">
              <div class="flex items-center gap-3 min-w-0">
                <IconFile size={20} class="text-[var(--hv-shell-text-2)] flex-none" />
                <div class="min-w-0">
                  <div class="text-sm font-bold text-[var(--hv-shell-text-strong)] truncate">{fileName(selectedTask)}</div>
                  <div class="text-xs text-[var(--hv-shell-text-2)] truncate font-[family-name:var(--hv-font-mono)]">{selectedTask.dir}</div>
                </div>
              </div>
              <span class={cn('px-2 py-0.5 rounded-full text-c-xs font-bold uppercase flex-none', selectedTask.status === 'complete' ? 'bg-[var(--hv-success-bg)] text-[var(--hv-success)]' : selectedTask.status === 'error' || selectedTask.status === 'removed' ? 'bg-[var(--hv-error-bg)] text-[var(--hv-error)]' : 'bg-[var(--hv-primary-light)] text-[var(--hv-primary)]')}>{statusLabel(selectedTask.status)}</span>
            </div>
            <div class="mt-4">
              <div class={progressTrack}><div class={cn('h-full rounded-full transition-[width]', selectedTask.status === 'complete' ? 'bg-[var(--hv-success)]' : 'bg-[var(--hv-primary)]')} style="width: {getProgress(selectedTask)}%"></div></div>
              <div class="flex items-center justify-between mt-1.5 text-xs">
                <span class="inline-flex items-center gap-1 text-[var(--hv-shell-text-2)]" style="color: {statusColor(selectedTask.status)}">
                  {#if selectedTask.status === 'active'}<IconLoader size={12} class="dl-spin" />{:else}{@const StatusIcon = statusIcon(selectedTask.status)}<StatusIcon size={12} />{/if}
                  <span>{statusLabel(selectedTask.status)}</span>
                </span>
                <span class="font-bold text-[var(--hv-shell-text-strong)]">{getProgress(selectedTask)}%</span>
              </div>
            </div>
            <div class="grid grid-cols-2 gap-x-4 gap-y-2 mt-4 text-xs text-[var(--hv-shell-text-2)]">
              <span class="flex justify-between gap-2"><span>{$t('downloader.size_label')}</span><span class="font-semibold text-[var(--hv-shell-text-strong)]">{formatBytes(selectedTask.completed_length)} / {formatBytes(selectedTask.total_length)}</span></span>
              {#if selectedTask.status === 'active' && parseInt(selectedTask.download_speed) > 0}<span class="flex justify-between gap-2"><span>{$t('downloader.speed_label')}</span><span class="font-semibold text-[var(--hv-primary)]">{formatSpeed(selectedTask.download_speed)}</span></span>{/if}
              {#if selectedTask.uris && selectedTask.uris.length > 0}<span class="col-span-2 flex justify-between gap-2"><span>{$t('downloader.url_label')}</span><code class="font-[family-name:var(--hv-font-mono)] text-right text-[var(--hv-shell-text-strong)] truncate">{selectedTask.uris[0]?.uri || '-'}</code></span>{/if}
            </div>
            <div class="flex gap-2 mt-4 flex-wrap">
              {#if selectedTask.status === 'paused'}
                <Button variant="primary" size="sm" data-testid="downloader-btn-primary" onclick={() => unpauseTask(selectedTask.gid)}><IconPlayerPlay size={14} /> {$t('downloader.resume_title')}</Button>
              {:else if selectedTask.status === 'active' || selectedTask.status === 'waiting'}
                <Button variant="secondary" size="sm" data-testid="downloader-btn-primary" onclick={() => pauseTask(selectedTask.gid)}><IconPlayerPause size={14} /> {$t('downloader.pause_title')}</Button>
              {/if}
              <Button variant="danger" size="sm" onclick={() => removeTask(selectedTask.gid)}><IconTrash size={14} /> {$t('downloader.delete_title')}</Button>
            </div>
          </div>
        </div>
      {/if}
    </div>
    {/if}
  {/if}

  {#snippet footer()}
    {#if !loading && aria2Available}
      <span data-testid="downloader-footer" class="flex items-center gap-2 min-w-0 flex-wrap">
        <span>{activeTasks.length} {$t('downloader.stat_active')}</span>
        <span class="text-[var(--hv-shell-text-3)]">&middot;</span>
        <span>{completedTasks.length} {$t('downloader.completed')}</span>
      </span>
    {/if}
  {/snippet}
</PageLayout>
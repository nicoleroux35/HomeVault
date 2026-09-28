<script lang="ts">
  import { onMount } from 'svelte';
  import { api } from '$lib/services/api';
  import { onAppVisible } from '$lib/utils/autoRevalidate';
  import { t, translate, languageStore } from '$lib/stores/i18n';
  import { Button, Toggle, Alert, Spinner, Badge, Modal } from '$lib/components/theme';
  import { PageLayout, ConfirmDialog, StatCard, SectionCard, EmptyState } from '$lib/components/app';
  import { createConfirmState } from '$lib/utils/confirmDialog.svelte';
  import { activateOnEnterOrSpace } from '$lib/utils/activateOnEnterOrSpace';
  import { cn } from '$lib/utils.js';
  import WindowHeader from '$lib/components/desktop/WindowHeader.svelte';
  import PathField from '$lib/components/theme/PathField.svelte';
  import FolderPicker from '$lib/components/theme/FolderPicker.svelte';
  import { IconActivity, IconArchive, IconCalendarClock, IconCircleCheck, IconClock, IconDatabase, IconFileText, IconFolder, IconFolderOpen, IconServer, IconLoader, IconLock, IconPlus, IconSearch, IconShield, IconTrash, IconX } from '@tabler/icons-svelte';

  const LOCALE_MAP: Record<string, string> = { en: 'en-US', fr: 'fr-FR', de: 'de-DE', es: 'es-ES' };

  interface Repo {
    id: string;
    name: string;
    repository: string;
    password: string;
    retention_policy_id: string | null;
    source_paths: string[];
    exclude_patterns: string | null;
    tags: string | null;
    snapshot_source: string;
    snapshot_pool: string | null;
    snapshot_dataset: string | null;
    snapshot_create_before_backup: boolean;
    created_at: string;
    updated_at: string;
  }

  interface ResticSnapshot {
    id: string;
    short_id: string;
    time: string;
    tree: string;
    paths: string[];
    hostname: string;
    program_version: string;
    tags: string[] | null;
  }

  interface BackupJob {
    id: string;
    repo_id: string;
    snapshot_id: string | null;
    source: string;
    backup_type: string;
    status: string;
    files_new: number | null;
    files_changed: number | null;
    files_unmodified: number | null;
    data_added_bytes: number | null;
    total_files_processed: number | null;
    total_bytes: number | null;
    duration_secs: number | null;
    error_message: string | null;
    started_at: string;
    finished_at: string | null;
    created_at: string;
  }

  interface ResticSchedule {
    id: string;
    repo_id: string;
    name: string;
    cron_expression: string;
    enabled: boolean;
    last_run_at: string | null;
    next_run_at: string | null;
  }

  interface RetentionPolicy {
    id: string;
    name: string;
    max_count: number | null;
    max_age_days: number | null;
    max_size_gb: number | null;
    apply_to_all: boolean;
  }

  // Svelte 5 runes mode: every value read in the template that is later
  // reassigned MUST be `$state(...)` — a plain `let` reassignment does not
  // re-render (same bug class as Shares/Downloader/Rescue). Before this,
  // `resticLoading` never flipped, so the page hung on its spinner forever.
  let repos: Repo[] = $state([]);
  let selectedRepo: Repo | null = $state(null);
  let snapshots: ResticSnapshot[] = $state([]);
  let backupJobs: BackupJob[] = $state([]);
  let schedules: ResticSchedule[] = $state([]);
  let loadingSchedules = $state(false);
  let retentionPolicies: RetentionPolicy[] = $state([]);
  let resticLoading = $state(true);
  let loadingSnapshots = $state(false);
  let loadingJobs = $state(false);
  let resticAvailable = $state(false);
  let resticVersion = $state('');

  let activeView = $state<'repos' | 'repo-detail' | 'create-repo'>('repos');
  let activeRepoSection: 'snapshots' | 'jobs' | 'schedules' = $state('snapshots');

  // Master-detail split — the repo list stays mounted on the left; the
  // detail/create pane appears on the right (see onMount for the observer).
  let splitEl = $state<HTMLDivElement | null>(null);
  let narrow = $state(false);
  let detailOpen = $derived(
    (activeView === 'repo-detail' && !!selectedRepo) || activeView === 'create-repo',
  );

  let newRepo: {
    name: string;
    repository: string;
    password: string;
    source_paths: string;
    exclude_patterns: string;
    tags: string;
    snapshot_source: string;
    snapshot_pool: string;
    snapshot_dataset: string;
    snapshot_create_before_backup: boolean;
    retention_policy_id: string;
  } = $state({
    name: '',
    repository: '',
    password: '',
    source_paths: '',
    exclude_patterns: '',
    tags: '',
    snapshot_source: 'directory',
    snapshot_pool: '',
    snapshot_dataset: '',
    snapshot_create_before_backup: false,
    retention_policy_id: ''
  });

  let showScheduleModal = $state(false);
  let newSchedule = $state({ name: '', cron_expression: '0 2 * * *', enabled: true });

  let showPolicyManagerModal = $state(false);
  let newPolicy = $state({ name: '', max_count: 30, max_age_days: 90, apply_to_all: false });

  let backupTags = $state('');
  let backupExclude = $state('');
  let backupSnapshotName = $state('');
  let restoreTarget = $state('/data');
  let restoreInclude = $state('');
  let restoreExclude = $state('');

  let showCreateBackupModal = $state(false);
  let showResticRestoreModal = $state(false);
  // The snapshot whose Restore button opened the modal — the footer's confirm
  // must act on THIS row, not on `snapshots.find(...)` (which always returned
  // the first snapshot).
  let pendingRestoreSnapshot = $state<ResticSnapshot | null>(null);
  let showForgetModal = $state(false);
  let backupInProgress = $state(false);
  let checkResult: any = $state(null);
  let statsData: any = $state(null);

  let forgetKeepLast = $state(7);
  let forgetKeepDaily = $state(7);
  let forgetKeepWeekly = $state(4);
  let forgetKeepMonthly = $state(6);
  let forgetKeepYearly = $state(3);
  let forgetPrune = $state(true);

  let totalSnapshots = $state(0);
  let aggregateSize = $state(0);
  const dlg = createConfirmState();

  async function loadAllStats() {
    totalSnapshots = 0;
    aggregateSize = 0;
    for (const repo of repos) {
      const resp = await api.resticStats(repo.id);
      if (resp.success && resp.data) {
        totalSnapshots += resp.data.snapshots_count || 0;
        aggregateSize += resp.data.total_size || 0;
      }
    }
  }

  // Reload whatever the current view shows — replaces the manual refresh
  // button; called on window focus / visibility (see onMount).
  function revalidate() {
    if (!resticAvailable) return;
    if (activeView === 'repo-detail' && selectedRepo) {
      loadSnapshots();
      loadJobs();
      loadSchedules();
    } else {
      loadRepos();
      loadRetentionPolicies();
    }
  }

  onMount(() => {
    (async () => {
      await checkResticAvailable();
      if (resticAvailable) {
        await Promise.all([loadRepos(), loadRetentionPolicies()]);
      }
      resticLoading = false;
    })();
    const teardownRefresh = onAppVisible(revalidate);

    // Master-detail split: `narrow` measures the split container itself (the
    // window floats, so a viewport media query would lie). Below 720px only
    // one pane shows — the repo list, or the detail/create when open.
    const ro = new ResizeObserver((entries) => {
      narrow = (entries[0]?.contentRect.width ?? 0) < 720;
    });
    if (splitEl) ro.observe(splitEl);

    return () => {
      teardownRefresh();
      ro.disconnect();
    };
  });

  async function checkResticAvailable() {
    try {
      const resp = await api.resticAvailable();
      if (resp.success && resp.data) {
        resticAvailable = resp.data.available;
      }
      if (resticAvailable) {
        const vresp = await api.resticVersion();
        if (vresp.success && vresp.data) {
          resticVersion = vresp.data.version;
        }
      }
    } catch (e) {
      resticAvailable = false;
    }
  }

  async function loadRepos() {
    try {
      const resp = await api.resticListRepos();
      if (resp.success && resp.data) {
        repos = resp.data;
      }
    } catch (e) {
    }
  }

  async function loadRetentionPolicies() {
    const resp = await api.resticListRetentionPolicies();
    if (resp.success && resp.data) {
      retentionPolicies = resp.data;
    }
  }

  async function selectRepo(repo: Repo) {
    selectedRepo = repo;
    activeView = 'repo-detail';
    activeRepoSection = 'snapshots';
    await Promise.all([loadSnapshots(), loadJobs(), loadSchedules()]);
  }

  async function loadSnapshots() {
    if (!selectedRepo) return;
    loadingSnapshots = true;
    try {
      const resp = await api.resticListSnapshots(selectedRepo.id);
      if (resp.success && resp.data) {
        snapshots = resp.data;
      }
    } catch (e) {
      snapshots = [];
    }
    loadingSnapshots = false;
  }

  async function loadJobs() {
    if (!selectedRepo) return;
    loadingJobs = true;
    try {
      const resp = await api.resticBackupJobs(selectedRepo.id);
      if (resp.success && resp.data) {
        backupJobs = resp.data;
      }
    } catch (e) {
      backupJobs = [];
    }
    loadingJobs = false;
  }

  async function loadSchedules() {
    if (!selectedRepo) return;
    loadingSchedules = true;
    try {
      const resp = await api.resticListSchedules(selectedRepo.id);
      if (resp.success && resp.data) {
        schedules = resp.data;
      }
    } catch (e) {
      schedules = [];
    }
    loadingSchedules = false;
  }

  function openScheduleModal() {
    newSchedule = { name: '', cron_expression: '0 2 * * *', enabled: true };
    showScheduleModal = true;
  }

  async function createSchedule() {
    if (!selectedRepo) return;
    if (!newSchedule.name.trim() || !newSchedule.cron_expression.trim()) {
      return;
    }

    const resp = await api.resticCreateSchedule({
      repo_id: selectedRepo.id,
      name: newSchedule.name.trim(),
      cron_expression: newSchedule.cron_expression.trim(),
      enabled: newSchedule.enabled,
    });

    if (resp.success) {
      showScheduleModal = false;
      await loadSchedules();
    } else {
    }
  }

  async function toggleSchedule(schedule: ResticSchedule) {
    const resp = await api.resticSetScheduleEnabled(schedule.id, !schedule.enabled);
    if (resp.success) {
      await loadSchedules();
    } else {
    }
  }

  async function deleteSchedule(schedule: ResticSchedule) {
    if (!(await dlg.ask(translate('restic.schedule_delete_confirm', { name: schedule.name })))) return;

    const resp = await api.resticDeleteSchedule(schedule.id);
    if (resp.success) {
      await loadSchedules();
    } else {
    }
  }

  async function createRetentionPolicy() {
    if (!newPolicy.name.trim()) {
      return;
    }

    const resp = await api.resticCreateRetentionPolicy({
      name: newPolicy.name.trim(),
      max_count: newPolicy.max_count || undefined,
      max_age_days: newPolicy.max_age_days || undefined,
      apply_to_all: newPolicy.apply_to_all,
    });

    if (resp.success) {
      newPolicy = { name: '', max_count: 30, max_age_days: 90, apply_to_all: false };
      await loadRetentionPolicies();
    } else {
    }
  }

  async function deleteRetentionPolicy(policy: RetentionPolicy) {
    if (!(await dlg.ask(translate('restic.policy_delete_confirm', { name: policy.name })))) return;

    const resp = await api.resticDeleteRetentionPolicy(policy.id);
    if (resp.success) {
      await loadRetentionPolicies();
    } else {
    }
  }

  function handleAddRepo() {
    activeView = 'create-repo';
  }

  async function createRepo() {
    const sourcePaths = newRepo.source_paths
      .split('\n')
      .map(s => s.trim())
      .filter(s => s.length > 0);

    if (sourcePaths.length === 0) {
      return;
    }

    const resp = await api.resticCreateRepo({
      name: newRepo.name,
      repository: newRepo.repository,
      password: newRepo.password,
      source_paths: sourcePaths,
      exclude_patterns: newRepo.exclude_patterns || undefined,
      tags: newRepo.tags || undefined,
      snapshot_source: newRepo.snapshot_source || 'directory',
      snapshot_pool: newRepo.snapshot_pool || undefined,
      snapshot_dataset: newRepo.snapshot_dataset || undefined,
      snapshot_create_before_backup: newRepo.snapshot_create_before_backup,
      retention_policy_id: newRepo.retention_policy_id || undefined,
    });

    if (resp.success) {
      activeView = 'repos';
      resetNewRepoForm();
      await loadRepos();
    } else {
    }
  }

  function resetNewRepoForm() {
    newRepo = {
      name: '',
      repository: '',
      password: '',
      source_paths: '',
      exclude_patterns: '',
      tags: '',
      snapshot_source: 'directory',
      snapshot_pool: '',
      snapshot_dataset: '',
      snapshot_create_before_backup: false,
      retention_policy_id: ''
    };
  }

  let pickerOpen = $state(false);
  let restorePickerOpen = $state(false);

  let sourcePathList = $derived(newRepo.source_paths
    .split('\n')
    .map(s => s.trim())
    .filter(Boolean));

  function handleAddSourcePath(path: string) {
    if (path && !sourcePathList.includes(path)) {
      newRepo.source_paths = [...sourcePathList, path].join('\n');
    }
    pickerOpen = false;
  }

  function removeSourcePath(index: number) {
    const parts = [...sourcePathList];
    parts.splice(index, 1);
    newRepo.source_paths = parts.join('\n');
  }

  let restoreIncludeList = $derived(restoreInclude
    .split('\n')
    .map(s => s.trim())
    .filter(Boolean));

  function handleAddRestorePath(path: string) {
    if (path && !restoreIncludeList.includes(path)) {
      restoreInclude = [...restoreIncludeList, path].join('\n');
    }
    restorePickerOpen = false;
  }

  function removeRestorePath(index: number) {
    const parts = [...restoreIncludeList];
    parts.splice(index, 1);
    restoreInclude = parts.join('\n');
  }

  async function deleteRepo(repo: Repo) {
    if (!(await dlg.ask(translate('restic.repo_delete_confirm', { name: repo.name })))) return;

    const resp = await api.resticDeleteRepo(repo.id);
    if (resp.success) {
      if (selectedRepo?.id === repo.id) {
        selectedRepo = null;
        activeView = 'repos';
      }
      await loadRepos();
    } else {
    }
  }

  async function runBackup() {
    if (!selectedRepo) return;
    backupInProgress = true;

    const resp = await api.resticBackup({
      repo_id: selectedRepo.id,
      tags: backupTags || undefined,
      exclude_patterns: backupExclude || undefined,
      snapshot_name: backupSnapshotName || undefined
    });

    if (resp.success) {
      showCreateBackupModal = false;
      await Promise.all([loadSnapshots(), loadJobs()]);
    } else {
    }
    backupInProgress = false;
  }

  async function restoreSnapshot(snapshot: ResticSnapshot) {
    if (!selectedRepo) return;

    const resp = await api.resticRestore({
      repo_id: selectedRepo.id,
      snapshot_id: snapshot.short_id,
      target: restoreTarget,
      exclude_patterns: restoreExclude || undefined,
      include_paths: restoreInclude ? restoreInclude.split('\n').map(s => s.trim()).filter(s => s) : undefined
    });

    if (resp.success) {
      pendingRestoreSnapshot = null;
      showResticRestoreModal = false;
    } else {
    }
  }

  async function runCheck() {
    if (!selectedRepo) return;
    checkResult = null;

    const resp = await api.resticCheck({ repo_id: selectedRepo.id });
    if (resp.success && resp.data) {
      checkResult = resp.data.check;
      if (checkResult.healthy) {
      } else {
      }
    } else {
    }
  }

  async function runForget() {
    if (!selectedRepo) return;

    const resp = await api.resticForget({
      repo_id: selectedRepo.id,
      keep_last: forgetKeepLast,
      keep_daily: forgetKeepDaily,
      keep_weekly: forgetKeepWeekly,
      keep_monthly: forgetKeepMonthly,
      keep_yearly: forgetKeepYearly,
      prune: forgetPrune
    });

    if (resp.success) {
      showForgetModal = false;
      await loadSnapshots();
    } else {
    }
  }

  async function runPrune() {
    if (!selectedRepo) return;

    const resp = await api.resticPrune(selectedRepo.id);
    if (resp.success) {
    } else {
    }
  }

  async function runUnlock() {
    if (!selectedRepo) return;

    const resp = await api.resticUnlock(selectedRepo.id);
    if (resp.success) {
    } else {
    }
  }

  async function loadStats() {
    if (!selectedRepo) return;

    const resp = await api.resticStats(selectedRepo.id);
    if (resp.success && resp.data) {
      statsData = resp.data;
    } else {
      statsData = null;
    }
  }

  async function deleteSnapshot(snapshot: ResticSnapshot) {
    if (!selectedRepo) return;
    if (!(await dlg.ask(translate('restic.delete_snapshot_confirm', { id: snapshot.short_id, time: snapshot.time })))) return;

    const resp = await api.resticDeleteSnapshot(selectedRepo.id, snapshot.short_id);
    if (resp.success) {
      await loadSnapshots();
    } else {
    }
  }

  function formatBytes(bytes: number): string {
    if (bytes === 0) return '0 B';
    const k = 1024;
    const sizes = ['B', 'KB', 'MB', 'GB', 'TB'];
    const i = Math.floor(Math.log(bytes) / Math.log(k));
    return parseFloat((bytes / Math.pow(k, i)).toFixed(2)) + ' ' + sizes[i];
  }

  function formatDate(dateString: string): string {
    const date = new Date(dateString);
    return date.toLocaleDateString(LOCALE_MAP[$languageStore] || 'en-US', {
      year: 'numeric',
      month: 'short',
      day: 'numeric',
      hour: '2-digit',
      minute: '2-digit'
    });
  }

  function getJobStatusVariant(status: string): 'success' | 'warning' | 'error' | 'info' {
    switch (status) {
      case 'completed': return 'success';
      case 'running': return 'warning';
      case 'failed': return 'error';
      default: return 'info';
    }
  }
  const badge = 'inline-flex items-center px-2 py-0.5 rounded-full text-c-xs font-bold uppercase';
  const inp = 'w-full h-11 px-3 border border-[var(--hv-shell-border-strong)] rounded-lg bg-[var(--hv-shell-border-soft)] text-sm outline-none focus:border-[var(--hv-primary)]';
  const textareaCls = 'w-full px-3 py-2.5 border border-[var(--hv-shell-border-strong)] rounded-lg bg-[var(--hv-shell-border-soft)] text-sm outline-none focus:border-[var(--hv-primary)] font-[family-name:var(--hv-font-mono)]';
  const selectCls = 'w-full h-11 px-3 border border-[var(--hv-shell-border-strong)] rounded-lg bg-[var(--hv-shell-border-soft)] text-sm outline-none focus:border-[var(--hv-primary)]';
  const fieldLabel = 'text-xs font-semibold text-[var(--hv-shell-text-strong)]';
  const hint = 'text-c-xs text-[var(--hv-shell-text-2)]';
  const th = 'text-start px-3 py-2 font-semibold text-[var(--hv-shell-text-2)] text-c-xs uppercase tracking-wide border-b border-[var(--hv-shell-border)]';
  const td = 'px-3 py-2 border-b border-[var(--hv-shell-border-soft)] text-sm text-[var(--hv-shell-text-strong)]';
  const sourceChip = 'inline-flex items-center gap-2 px-3 py-1.5 rounded-lg bg-[var(--hv-shell-border-soft)] border border-[var(--hv-shell-border)] text-xs text-[var(--hv-shell-text-strong)]';
</script>
<PageLayout
  title={$t('restic.title')}
  data-testid="restic-page"
  tabs={activeView === 'repo-detail' ? [
    { id: 'snapshots', label: $t('restic.tab_snapshots'), icon: IconDatabase, count: snapshots.length || undefined },
    { id: 'jobs', label: $t('restic.tab_jobs'), icon: IconFileText, count: backupJobs.length || undefined },
    { id: 'schedules', label: $t('restic.tab_schedules'), icon: IconCalendarClock, count: schedules.length || undefined },
  ] : []}
  bind:activeTab={activeRepoSection}
  tabsLabel={$t('restic.subtitle')}
>
  {#snippet icon()}
    <IconServer stroke={1.8} />
  {/snippet}
  <!-- Page-level actions: a WindowToolbar row under the header, never inside
       it (the header bar is icon + title + window controls only). -->
  {#snippet toolbarLeft()}
    {#if resticAvailable && activeView === 'repo-detail' && selectedRepo}
      <Button variant="ghost" onclick={() => { activeView = 'repos'; selectedRepo = null; }}>{$t('restic.back_to_repos')}</Button>
    {/if}
  {/snippet}

  {#snippet toolbarRight()}
    {#if resticAvailable && activeView === 'repos'}
      <Button variant="primary" data-testid="restic-add-repo" onclick={handleAddRepo}>
        <IconPlus size={16} stroke={2} />
        <span>{$t('restic.add_repository')}</span>
      </Button>
    {:else if resticAvailable && activeView === 'repo-detail' && selectedRepo}
      <Button variant="primary" data-testid="restic-run-backup" onclick={() => showCreateBackupModal = true}>{$t('restic.run_backup')}</Button>
    {/if}
  {/snippet}

  {#if resticLoading}
    <div data-testid="restic-loading" class="flex items-center justify-center gap-3 py-16 text-[var(--hv-shell-text)]">
      <Spinner />
      <span>{$t('restic.loading')}</span>
    </div>
  {:else}
    <div data-testid="restic-stats" class="grid grid-cols-2 @min-[880px]:grid-cols-4 gap-3 mb-5">
      <StatCard data-testid="restic-stat-card" label={$t('restic.stat_repositories')} value={`${repos.length}`} tone={'info'}>
        {#snippet iconSnippet()}<IconServer size={20} />{/snippet}
      </StatCard>
      <StatCard data-testid="restic-stat-card" onClick={resticAvailable && repos.length > 0 ? loadAllStats : undefined} label={$t('restic.stat_snapshots')} value={`${totalSnapshots}`} tone={'info'}>
        {#snippet iconSnippet()}<IconDatabase size={20} />{/snippet}
      </StatCard>
      <StatCard data-testid="restic-stat-card" label={$t('restic.stat_total_size')} value={`${resticAvailable ? formatBytes(aggregateSize) : $t('common.na')}`} tone={'warning'}>
        {#snippet iconSnippet()}<IconActivity size={20} />{/snippet}
      </StatCard>
      <StatCard data-testid="restic-stat-card" label={`${translate('restic.stat_status', { version: resticVersion })}`} value={`${resticAvailable ? $t('restic.online') : $t('restic.offline')}`} tone={resticAvailable ? 'success' : 'error'}>
        {#snippet iconSnippet()}<IconShield size={20} />{/snippet}
      </StatCard>
    </div>


    {#if !resticAvailable}

      <EmptyState data-testid="restic-empty" title={$t('restic.not_installed_title')} description={$t('restic.not_installed_desc')}>
        {#snippet iconSnippet()}<IconServer size={32} stroke={1.8} />{/snippet}
        {#snippet action()}
          <div class="flex flex-col gap-1.5">
            <code class="px-3 py-1.5 rounded-lg bg-[var(--hv-shell-border-soft)] border border-[var(--hv-shell-border)] text-xs font-[family-name:var(--hv-font-mono)]">sudo apt install restic  # Debian/Ubuntu</code>
            <code class="px-3 py-1.5 rounded-lg bg-[var(--hv-shell-border-soft)] border border-[var(--hv-shell-border)] text-xs font-[family-name:var(--hv-font-mono)]">sudo pacman -S restic     # Arch</code>
            <code class="px-3 py-1.5 rounded-lg bg-[var(--hv-shell-border-soft)] border border-[var(--hv-shell-border)] text-xs font-[family-name:var(--hv-font-mono)]">brew install restic       # macOS</code>
          </div>
        {/snippet}
      </EmptyState>

    {:else}
    <div data-testid="restic-split" bind:this={splitEl} class="flex flex-1 min-h-0">
      {#if !(narrow && detailOpen)}
        <div data-testid="restic-list-pane" class="flex-none min-w-0 border-r border-[var(--hv-shell-border)] overflow-y-auto" style="width: {narrow ? '100%' : '420px'};">

      {#if repos.length === 0}
        <EmptyState data-testid="restic-empty" title={$t('restic.no_repos_title')} description={$t('restic.no_repos_desc')}>
          {#snippet iconSnippet()}<IconArchive size={32} stroke={1.8} />{/snippet}
          {#snippet action()}<Button variant="primary" data-testid="restic-btn-primary" onclick={() => activeView = 'create-repo'}><IconPlus size={15} /> <span>{$t('restic.add_repository')}</span></Button>{/snippet}
        </EmptyState>
      {:else}
        <div class="grid gap-4 [grid-template-columns:repeat(auto-fill,minmax(300px,1fr))]">
          {#each repos as repo}
            <div data-testid="restic-repo-card" class="rounded-2xl bg-[var(--hv-shell-card)] border border-[var(--hv-shell-border)] shadow-[var(--hv-shell-shadow-card)] overflow-hidden cursor-pointer hover:border-[var(--hv-shell-border-strong)]" onclick={() => selectRepo(repo)} onkeydown={(e) => activateOnEnterOrSpace(e, () => selectRepo(repo))} tabindex="0" role="button">
              <div class="flex items-center gap-3 px-4 pt-4">
                <span class="grid place-items-center w-11 h-11 rounded-xl flex-none bg-[var(--hv-primary-light)] text-[var(--hv-primary)]"><IconServer size={20} /></span>
                <div class="flex-1 min-w-0">
                  <div data-testid="restic-repo-name" class="text-sm font-bold text-[var(--hv-shell-text-strong)] truncate">{repo.name}</div>
                  <code class="text-c-xs text-[var(--hv-shell-text-2)] font-[family-name:var(--hv-font-mono)] truncate block">{repo.repository}</code>
                </div>
                <span class={cn(badge, repo.snapshot_source === 'snapshot' ? 'bg-[var(--hv-primary-light)] text-[var(--hv-primary)]' : 'bg-[var(--hv-shell-border-soft)] text-[var(--hv-shell-text-2)]')}>{repo.snapshot_source === 'snapshot' ? $t('restic.snapshot_mode_snapshot') : $t('restic.snapshot_mode_directory')}</span>
              </div>
              <div class="px-4 py-3 flex flex-col gap-1.5">
                <div class="flex items-start justify-between gap-2 text-sm"><span class="text-xs text-[var(--hv-shell-text-2)]">{$t('restic.source_label')}</span><span class="font-semibold text-[var(--hv-shell-text-strong)] text-right">{repo.source_paths.join(', ') || $t('restic.not_configured')}</span></div>
                {#if repo.tags}<div class="flex items-center justify-between gap-2 text-sm"><span class="text-xs text-[var(--hv-shell-text-2)]">{$t('restic.tags_label')}</span><span class="font-semibold text-[var(--hv-shell-text-strong)]">{repo.tags}</span></div>{/if}
              </div>
              <div class="px-4 pb-4 flex justify-end">
                <button data-testid="restic-action-danger" class="grid place-items-center w-9 h-9 flex-none rounded-lg border border-[var(--hv-shell-border-strong)] bg-[var(--hv-shell-fill)] text-[var(--hv-shell-text-strong)] cursor-pointer hover:bg-[var(--hv-error-bg)] hover:text-[var(--hv-error)] hv-ic" onclick={(e) => { e.stopPropagation(); deleteRepo(repo); }} title={$t('restic.remove_config_title')} aria-label={$t('restic.remove_config_title')}><IconTrash size={14} /></button>
              </div>
            </div>
          {/each}
        </div>
      {/if}

        </div>
      {/if}

      {#if activeView === 'repo-detail' && selectedRepo}
        <div data-testid="restic-detail-pane" class="flex-1 min-w-0 min-h-0 overflow-y-auto px-5">

      <div data-testid="restic-stats" class="grid grid-cols-2 @min-[880px]:grid-cols-4 gap-3 mb-4">
        <StatCard data-testid="restic-stat-card" label={selectedRepo.repository} value={`${selectedRepo.name}`} tone={'info'}>
          {#snippet iconSnippet()}<IconServer size={20} />{/snippet}
        </StatCard>
        <StatCard data-testid="restic-stat-card" label={$t('restic.stat_snapshots')} value={`${snapshots.length}`} tone={'info'}>
          {#snippet iconSnippet()}<IconDatabase size={20} />{/snippet}
        </StatCard>
        <StatCard data-testid="restic-stat-card" onClick={loadStats} label={$t('restic.stat_total_size')} value={`${statsData ? formatBytes(statsData.total_size) : $t('restic.click_to_load')}`} tone={'warning'}>
          {#snippet iconSnippet()}<IconActivity size={20} />{/snippet}
        </StatCard>
        <StatCard data-testid="restic-stat-card" onClick={runCheck} label={$t('restic.verify_integrity')} value={`${$t('restic.check_stat')}`} tone={'success'}>
          {#snippet iconSnippet()}<IconShield size={20} />{/snippet}
        </StatCard>
      </div>

      <div class="flex flex-wrap items-center gap-2 mb-4">
        <Button variant="ghost" onclick={() => { activeView = 'repos'; selectedRepo = null; }}>&larr; {$t('restic.back_to_repos')}</Button>
        <Button variant="primary" data-testid="restic-btn-primary" onclick={() => showCreateBackupModal = true}>{$t('restic.run_backup')}</Button>
        <div class="flex-1"></div>
        <Button variant="ghost" onclick={runPrune}>{$t('restic.prune')}</Button>
        <Button variant="ghost" onclick={runUnlock}>{$t('restic.unlock')}</Button>
        <Button variant="ghost" onclick={() => showForgetModal = true}>{$t('restic.forget')}</Button>
        <Button variant="danger" onclick={() => { if (selectedRepo) deleteRepo(selectedRepo); }}>{$t('restic.remove_config')}</Button>
      </div>

      {#if activeRepoSection === 'snapshots'}
        {#if loadingSnapshots}
          <div data-testid="restic-loading" class="flex items-center justify-center gap-3 py-12 text-[var(--hv-shell-text)]"><Spinner /> <span>{$t('restic.loading_snapshots')}</span></div>
        {:else if snapshots.length === 0}
          <EmptyState data-testid="restic-empty" title={$t('restic.no_snapshots_title')} description={$t('restic.no_snapshots_desc')}>
            {#snippet iconSnippet()}<IconDatabase size={32} stroke={1.8} />{/snippet}
            {#snippet action()}<Button variant="primary" onclick={() => showCreateBackupModal = true}>{$t('restic.run_backup')}</Button>{/snippet}
          </EmptyState>
        {:else}
          <SectionCard class="overflow-x-auto">
            <div class="mb-2"><h3 class="m-0 text-c-card-title font-semibold text-[var(--hv-shell-text-strong)]">{$t('restic.snapshots_table_title')}</h3><span class="text-xs text-[var(--hv-shell-text-2)]">{$t('restic.snapshots_table_subtitle')}</span></div>
            <table class="w-full border-collapse">
              <thead><tr><th class={th}>{$t('restic.col_id')}</th><th class={th}>{$t('restic.col_date')}</th><th class={th}>{$t('restic.col_host')}</th><th class={th}>{$t('restic.col_paths')}</th><th class={th}>{$t('restic.col_tags')}</th><th class={th}>{$t('restic.col_actions')}</th></tr></thead>
              <tbody>
                {#each snapshots as snap}
                  <tr>
                    <td class={td}><code class="text-xs font-[family-name:var(--hv-font-mono)] text-[var(--hv-primary)]">{snap.short_id}</code></td>
                    <td class={td}>{formatDate(snap.time)}</td>
                    <td class={td}>{snap.hostname}</td>
                    <td class={td}><div class="flex flex-wrap gap-1">{#each snap.paths as path}<code class="text-c-xs font-[family-name:var(--hv-font-mono)] text-[var(--hv-shell-text-2)] bg-[var(--hv-shell-border-soft)] px-1.5 py-0.5 rounded">{path}</code>{/each}</div></td>
                    <td class={td}>{#if snap.tags && snap.tags.length > 0}<div class="flex flex-wrap gap-1">{#each snap.tags as tag}<span class={cn(badge, 'bg-[var(--hv-primary-light)] text-[var(--hv-primary)]')}>{tag}</span>{/each}</div>{:else}<span class="text-xs text-[var(--hv-shell-text-2)]">-</span>{/if}</td>
                    <td class={td}><div class="flex gap-2"><Button variant="ghost" size="sm" onclick={() => { pendingRestoreSnapshot = snap; showResticRestoreModal = true; }}>{$t('restic.restore_btn')}</Button><button data-testid="restic-action-danger" class="grid place-items-center w-9 h-9 flex-none rounded-lg border border-[var(--hv-shell-border-strong)] bg-[var(--hv-shell-fill)] text-[var(--hv-shell-text-strong)] cursor-pointer hover:bg-[var(--hv-error-bg)] hover:text-[var(--hv-error)] hv-ic" onclick={() => deleteSnapshot(snap)} title={$t('restic.delete_snapshot_title')} aria-label={$t('restic.delete_snapshot_title')}><IconTrash size={14} /></button></div></td>
                  </tr>
                {/each}
              </tbody>
            </table>
          </SectionCard>
        {/if}
      {:else if activeRepoSection === 'jobs'}
        {#if loadingJobs}
          <div data-testid="restic-loading" class="flex items-center justify-center gap-3 py-12 text-[var(--hv-shell-text)]"><Spinner /> <span>{$t('restic.loading_jobs')}</span></div>
        {:else if backupJobs.length === 0}
          <EmptyState data-testid="restic-empty" title={$t('restic.no_jobs_title')} description={$t('restic.no_jobs_desc')}>
            {#snippet iconSnippet()}<IconFileText size={32} stroke={1.8} />{/snippet}
          </EmptyState>
        {:else}
          <SectionCard class="overflow-x-auto">
            <div class="mb-2"><h3 class="m-0 text-c-card-title font-semibold text-[var(--hv-shell-text-strong)]">{$t('restic.jobs_table_title')}</h3><span class="text-xs text-[var(--hv-shell-text-2)]">{$t('restic.jobs_table_subtitle')}</span></div>
            <table class="w-full border-collapse">
              <thead><tr><th class={th}>{$t('restic.col_date')}</th><th class={th}>{$t('restic.col_type')}</th><th class={th}>{$t('restic.col_status')}</th><th class={th}>{$t('restic.col_files_new')}</th><th class={th}>{$t('restic.col_files_changed')}</th><th class={th}>{$t('restic.col_data_added')}</th><th class={th}>{$t('restic.col_duration')}</th><th class={th}>{$t('restic.col_snapshot')}</th></tr></thead>
              <tbody>
                {#each backupJobs as job}
                  <tr>
                    <td class={td}>{formatDate(job.started_at)}</td>
                    <td class={td}><span class={cn(badge, 'bg-[var(--hv-primary-light)] text-[var(--hv-primary)]')}>{job.backup_type}</span></td>
                    <td class={td}><span class={cn(badge, job.status === 'completed' ? 'bg-[var(--hv-success-bg)] text-[var(--hv-success)]' : job.status === 'running' ? 'bg-[var(--hv-warning-bg)] text-[var(--hv-warning)]' : 'bg-[var(--hv-error-bg)] text-[var(--hv-error)]')}>{job.status}</span></td>
                    <td class={td}>{job.files_new ?? '-'}</td>
                    <td class={td}>{job.files_changed ?? '-'}</td>
                    <td class={td}>{job.data_added_bytes ? formatBytes(job.data_added_bytes) : '-'}</td>
                    <td class={td}>{job.duration_secs ? `${job.duration_secs}s` : '-'}</td>
                    <td class={td}>{#if job.snapshot_id}<code class="text-xs font-[family-name:var(--hv-font-mono)] text-[var(--hv-primary)]">{job.snapshot_id.slice(0, 8)}</code>{:else}-{/if}</td>
                  </tr>
                {/each}
              </tbody>
            </table>
          </SectionCard>
        {/if}
      {:else if activeRepoSection === 'schedules'}
        {#if loadingSchedules}
          <div data-testid="restic-loading" class="flex items-center justify-center gap-3 py-12 text-[var(--hv-shell-text)]"><Spinner /> <span>{$t('restic.loading')}</span></div>
        {:else}
          <div class="flex justify-end mb-3"><Button variant="primary" data-testid="restic-btn-primary" onclick={openScheduleModal}><IconPlus size={15} /> <span>{$t('restic.new_schedule')}</span></Button></div>
          {#if schedules.length === 0}
            <EmptyState data-testid="restic-empty" title={$t('restic.no_schedules_title')} description={$t('restic.no_schedules_desc')}>
              {#snippet iconSnippet()}<IconCalendarClock size={32} stroke={1.8} />{/snippet}
            </EmptyState>
          {:else}
            <SectionCard class="overflow-x-auto">
              <div class="mb-2"><h3 class="m-0 text-c-card-title font-semibold text-[var(--hv-shell-text-strong)]">{$t('restic.schedules_table_title')}</h3><span class="text-xs text-[var(--hv-shell-text-2)]">{$t('restic.schedules_table_subtitle')}</span></div>
              <table class="w-full border-collapse">
                <thead><tr><th class={th}>{$t('restic.col_name')}</th><th class={th}>{$t('restic.col_cron')}</th><th class={th}>{$t('restic.col_last_run')}</th><th class={th}>{$t('restic.col_next_run')}</th><th class={th}>{$t('restic.col_enabled')}</th><th class={th}>{$t('restic.col_actions')}</th></tr></thead>
                <tbody>
                  {#each schedules as schedule}
                    <tr>
                      <td class={td}>{schedule.name}</td>
                      <td class={td}><code class="text-xs font-[family-name:var(--hv-font-mono)] text-[var(--hv-shell-text-2)] bg-[var(--hv-shell-border-soft)] px-1.5 py-0.5 rounded">{schedule.cron_expression}</code></td>
                      <td class={td}>{schedule.last_run_at ? formatDate(schedule.last_run_at) : $t('restic.never')}</td>
                      <td class={td}>{schedule.next_run_at ? formatDate(schedule.next_run_at) : $t('restic.never')}</td>
                      <td class={td}>
                        <label class="relative inline-block w-10 h-5 cursor-pointer">
                          <input type="checkbox" class="sr-only" checked={schedule.enabled} onchange={() => toggleSchedule(schedule)} />
                          <span class={cn('block w-10 h-5 rounded-full transition-colors', schedule.enabled ? 'bg-[var(--hv-primary)]' : 'bg-[var(--hv-shell-border)]')}></span>
                          <span class={cn('absolute top-0.5 left-0.5 w-4 h-4 rounded-full bg-white transition-transform', schedule.enabled && 'translate-x-5')}></span>
                        </label>
                      </td>
                      <td class={td}><button data-testid="restic-action-danger" class="grid place-items-center w-9 h-9 flex-none rounded-lg border border-[var(--hv-shell-border-strong)] bg-[var(--hv-shell-fill)] text-[var(--hv-shell-text-strong)] cursor-pointer hover:bg-[var(--hv-error-bg)] hover:text-[var(--hv-error)] hv-ic" onclick={() => deleteSchedule(schedule)} title={$t('common.delete')} aria-label={$t('common.delete')}><IconTrash size={14} /></button></td>
                    </tr>
                  {/each}
                </tbody>
              </table>
            </SectionCard>
          {/if}
        {/if}
      {/if}
        </div>
      {:else if activeView === 'create-repo'}
        <div data-testid="restic-detail-pane" class="flex-1 min-w-0 min-h-0 overflow-y-auto px-5">

      <SectionCard class="max-w-[720px]">
        <div class="mb-4">
          <h2 class="m-0 text-c-lg font-semibold text-[var(--hv-shell-text-strong)]">{$t('restic.add_repo_title')}</h2>
          <p class="m-0 mt-0.5 text-xs text-[var(--hv-shell-text)]">{$t('restic.add_repo_subtitle')}</p>
        </div>
<div class="grid grid-cols-2 @max-[560px]:grid-cols-1 gap-3">
          <div class="flex flex-col gap-1.5">
            <label for="repo-name" class={fieldLabel}>{$t('restic.repo_name_label')} <span class="text-[var(--hv-error)]">*</span></label>
            <input id="repo-name" class={inp} type="text" bind:value={newRepo.name} placeholder={$t('restic.repo_name_placeholder')} required />
            <span class={hint}>{$t('restic.repo_name_hint')}</span>
          </div>
          <div class="flex flex-col gap-1.5">
            <label for="repo-url" class={fieldLabel}>{$t('restic.repo_url_label')} <span class="text-[var(--hv-error)]">*</span></label>
            <PathField id="repo-url" bind:value={newRepo.repository} mode="folder" placeholder={$t('restic.repo_url_placeholder')} required />
            <span class={hint}>{$t('restic.repo_url_hint')}</span>
          </div>
          <div class="flex flex-col gap-1.5">
            <label for="repo-password" class={fieldLabel}>{$t('restic.repo_password_label')} <span class="text-[var(--hv-error)]">*</span></label>
            <input id="repo-password" class={inp} type="password" bind:value={newRepo.password} placeholder={$t('restic.repo_password_placeholder')} required />
            <span class={hint}>{$t('restic.repo_password_hint')}</span>
          </div>
          <div class="flex flex-col gap-1.5">
            <label for="repo-tags" class={fieldLabel}>{$t('restic.tags_label')}</label>
            <input id="repo-tags" class={inp} type="text" bind:value={newRepo.tags} placeholder={$t('restic.tags_placeholder')} />
            <span class={hint}>{$t('restic.tags_hint')}</span>
          </div>
        </div>
        <div class="flex flex-col gap-1.5 mt-3">
          <span class={fieldLabel} id="restic-source-paths-label">{$t('restic.source_paths_label')} <span class="text-[var(--hv-error)]">*</span></span>
          <div class="flex flex-wrap gap-2" role="group" aria-labelledby="restic-source-paths-label">
            {#if sourcePathList.length === 0}
              <span class="text-xs text-[var(--hv-shell-text-2)]">{$t('restic.no_paths')}</span>
            {:else}
              {#each sourcePathList as path, i}
                <span class={sourceChip}><IconFolder size={14} /> <span class="restic-source-path">{path}</span> <button type="button" class="hv-ic border-none bg-transparent cursor-pointer text-[var(--hv-shell-text-2)] hover:text-[var(--hv-error)]" onclick={() => removeSourcePath(i)} aria-label={$t('common.remove')}><IconX size={14} /></button></span>
              {/each}
            {/if}
          </div>
          <div class="mt-1">
            <button type="button" data-testid="restic-btn-primary" class="inline-flex items-center gap-2 h-10 px-4 border border-[var(--hv-shell-border-strong)] rounded-xl bg-[var(--hv-shell-fill)] text-[var(--hv-shell-text-strong)] text-c-base font-semibold cursor-pointer hover:bg-[var(--hv-shell-border)]" onclick={() => (pickerOpen = true)}><IconFolderOpen size={14} /> {$t('restic.add_path')}</button>
          </div>
          <span class={hint}>{$t('restic.source_paths_hint')}</span>
        </div>
        <FolderPicker open={pickerOpen} mode="folder" onselect={handleAddSourcePath} onclose={() => (pickerOpen = false)} />
        <div class="flex flex-col gap-1.5 mt-3">
          <label for="repo-exclude" class={fieldLabel}>{$t('restic.exclude_patterns_label')}</label>
          <textarea id="repo-exclude" class={textareaCls} bind:value={newRepo.exclude_patterns} placeholder="*.tmp&#10;*.log&#10;node_modules" rows={2}></textarea>
        </div>
        <div class="flex flex-col gap-1.5 mt-3">
          <label for="repo-source-type" class={fieldLabel}>{$t('restic.source_type_label')}</label>
          <select id="repo-source-type" class={selectCls} bind:value={newRepo.snapshot_source}>
            <option value="directory">{$t('restic.source_type_directory')}</option>
            <option value="snapshot">{$t('restic.source_type_snapshot')}</option>
          </select>
        </div>
        <div class="flex flex-col gap-1.5 mt-3">
          <label for="repo-retention" class={fieldLabel}>{$t('restic.retention_policy_label')}</label>
          <div class="flex gap-2 items-center">
            <select id="repo-retention" class={selectCls} bind:value={newRepo.retention_policy_id}>
              <option value="">{$t('restic.retention_policy_none')}</option>
              {#each retentionPolicies as policy}
                <option value={policy.id}>{policy.name}</option>
              {/each}
            </select>
            <Button type="button" variant="ghost" size="sm" onclick={() => showPolicyManagerModal = true}>{$t('restic.manage_policies')}</Button>
          </div>
        </div>
        {#if newRepo.snapshot_source === 'snapshot'}
          <div class="grid grid-cols-2 @max-[560px]:grid-cols-1 gap-3 mt-3">
            <div class="flex flex-col gap-1.5">
              <label for="repo-pool" class={fieldLabel}>{$t('restic.pool_label')}</label>
              <input id="repo-pool" class={inp} type="text" bind:value={newRepo.snapshot_pool} placeholder="zpool" />
              <span class={hint}>{$t('restic.pool_hint')}</span>
            </div>
            <div class="flex flex-col gap-1.5">
              <label for="repo-dataset" class={fieldLabel}>{$t('restic.dataset_label')}</label>
              <input id="repo-dataset" class={inp} type="text" bind:value={newRepo.snapshot_dataset} placeholder="zpool/data" />
              <span class={hint}>{$t('restic.dataset_hint')}</span>
            </div>
          </div>
          <div class="mt-3"><Toggle bind:checked={newRepo.snapshot_create_before_backup} label={$t('restic.snapshot_before_backup')} /></div>
        {/if}
        <div class="flex gap-2.5 mt-4">
          <Button variant="ghost" onclick={() => activeView = 'repos'}>{$t('common.cancel')}</Button>
          <Button variant="primary" data-testid="restic-btn-primary" onclick={createRepo}>{$t('restic.initialize_repo')}</Button>
        </div>
      </SectionCard>

        </div>
      {/if}
    </div>
    {/if}
  {/if}
  <ConfirmDialog
    {dlg}
    onconfirm={() => dlg.settle(true)}
    oncancel={() => dlg.settle(false)}
  />
</PageLayout>

<!-- Run Restic Backup Modal -->
<Modal open={showCreateBackupModal && !!selectedRepo} title={$t('restic.run_backup_title')} size="md" onclose={() => showCreateBackupModal = false}>
  <div class="grid grid-cols-2 @max-[360px]:grid-cols-1 gap-3">
    <div class="flex flex-col gap-1.5"><label for="bk-snapshot" class={fieldLabel}>{$t('restic.snapshot_name_label')}</label><input id="bk-snapshot" class={inp} type="text" bind:value={backupSnapshotName} placeholder={$t('restic.snapshot_name_placeholder')} /><span class={hint}>{$t('restic.snapshot_name_hint')}</span></div>
    <div class="flex flex-col gap-1.5"><label for="bk-tags" class={fieldLabel}>{$t('restic.tags_label')}</label><input id="bk-tags" class={inp} type="text" bind:value={backupTags} placeholder={$t('restic.backup_tags_placeholder')} /><span class={hint}>{$t('restic.backup_tags_hint')}</span></div>
  </div>
  <div class="flex flex-col gap-1.5 mt-3"><label for="bk-exclude" class={fieldLabel}>{$t('restic.backup_exclude_label')}</label><textarea id="bk-exclude" class={textareaCls} bind:value={backupExclude} placeholder="*.log&#10;*.tmp" rows={2}></textarea></div>
  {#snippet footer()}
    <div class="flex gap-2.5 justify-end">
      <Button variant="ghost" onclick={() => showCreateBackupModal = false} disabled={backupInProgress}>{$t('common.cancel')}</Button>
      <Button variant="primary" data-testid="restic-btn-primary" onclick={runBackup} disabled={backupInProgress}>{backupInProgress ? $t('restic.running') : $t('restic.start_backup')}</Button>
    </div>
  {/snippet}
</Modal>

<!-- Restic Restore Modal -->
<Modal open={showResticRestoreModal && !!selectedRepo} title={$t('restic.restore_title')} size="md" onclose={() => { pendingRestoreSnapshot = null; showResticRestoreModal = false; }}>
  <Alert variant="warning">{$t('restic.restore_warning')}</Alert>
  <div class="flex flex-col gap-1.5 mt-3">
    <label for="rv-target" class={fieldLabel}>{$t('restic.restore_target_label')} <span class="text-[var(--hv-error)]">*</span></label>
    <PathField id="rv-target" bind:value={restoreTarget} mode="folder" placeholder="/data/restore" required />
  </div>
  <div class="flex flex-col gap-1.5 mt-3">
    <span class={fieldLabel} id="restic-restore-include-label">{$t('restic.restore_include_label')}</span>
    <div class="flex flex-wrap gap-2" role="group" aria-labelledby="restic-restore-include-label">
      {#if restoreIncludeList.length === 0}
        <span class="text-xs text-[var(--hv-shell-text-2)]">{$t('restic.no_paths')}</span>
      {:else}
        {#each restoreIncludeList as path, i}
          <span class={sourceChip}><IconFolder size={14} /> <span>{path}</span> <button type="button" class="hv-ic border-none bg-transparent cursor-pointer text-[var(--hv-shell-text-2)] hover:text-[var(--hv-error)]" onclick={() => removeRestorePath(i)} aria-label={$t('common.remove')}><IconX size={14} /></button></span>
        {/each}
      {/if}
    </div>
    <div class="mt-1">
      <button type="button" class="inline-flex items-center gap-2 h-10 px-4 border border-[var(--hv-shell-border-strong)] rounded-xl bg-[var(--hv-shell-fill)] text-[var(--hv-shell-text-strong)] text-c-base font-semibold cursor-pointer hover:bg-[var(--hv-shell-border)]" onclick={() => (restorePickerOpen = true)}><IconFolderOpen size={14} /> {$t('restic.add_include_path')}</button>
    </div>
    <span class={hint}>{$t('restic.restore_include_hint')}</span>
  </div>
  <div class="flex flex-col gap-1.5 mt-3"><label for="rv-exclude" class={fieldLabel}>{$t('restic.restore_exclude_label')}</label><textarea id="rv-exclude" class={textareaCls} bind:value={restoreExclude} placeholder="*.tmp" rows={2}></textarea></div>
  <FolderPicker open={restorePickerOpen} mode="folder" onselect={handleAddRestorePath} onclose={() => (restorePickerOpen = false)} />
  {#snippet footer()}
    <div class="flex gap-2.5 justify-end">
      <Button variant="ghost" onclick={() => { pendingRestoreSnapshot = null; showResticRestoreModal = false; }}>{$t('common.cancel')}</Button>
      <Button variant="danger" onclick={() => { if (pendingRestoreSnapshot) restoreSnapshot(pendingRestoreSnapshot); }} disabled={!pendingRestoreSnapshot}>{$t('restic.restore_btn')}</Button>
    </div>
  {/snippet}
</Modal>

<!-- Forget Modal -->
<Modal open={showForgetModal && !!selectedRepo} title={$t('restic.forget_title')} size="md" onclose={() => showForgetModal = false}>
  <p class="m-0 mb-3 text-sm text-[var(--hv-shell-text)]">{$t('restic.forget_desc')}</p>
  <div class="grid grid-cols-2 @max-[360px]:grid-cols-1 gap-3">
    <div class="flex flex-col gap-1.5"><label for="fk-last" class={fieldLabel}>{$t('restic.keep_last_label')}</label><input id="fk-last" class={inp} type="number" bind:value={forgetKeepLast} min="1" /></div>
    <div class="flex flex-col gap-1.5"><label for="fk-daily" class={fieldLabel}>{$t('restic.keep_daily_label')}</label><input id="fk-daily" class={inp} type="number" bind:value={forgetKeepDaily} min="0" /></div>
    <div class="flex flex-col gap-1.5"><label for="fk-weekly" class={fieldLabel}>{$t('restic.keep_weekly_label')}</label><input id="fk-weekly" class={inp} type="number" bind:value={forgetKeepWeekly} min="0" /></div>
    <div class="flex flex-col gap-1.5"><label for="fk-monthly" class={fieldLabel}>{$t('restic.keep_monthly_label')}</label><input id="fk-monthly" class={inp} type="number" bind:value={forgetKeepMonthly} min="0" /></div>
    <div class="flex flex-col gap-1.5"><label for="fk-yearly" class={fieldLabel}>{$t('restic.keep_yearly_label')}</label><input id="fk-yearly" class={inp} type="number" bind:value={forgetKeepYearly} min="0" /></div>
  </div>
  <div class="mt-3"><Toggle bind:checked={forgetPrune} label={$t('restic.prune_unused')} /></div>
  {#snippet footer()}
    <div class="flex gap-2.5 justify-end">
      <Button variant="ghost" onclick={() => showForgetModal = false}>{$t('common.cancel')}</Button>
      <Button variant="primary" onclick={runForget}>{$t('restic.apply_retention')}</Button>
    </div>
  {/snippet}
</Modal>

<!-- New Schedule Modal -->
<Modal open={showScheduleModal && !!selectedRepo} title={$t('restic.new_schedule')} size="md" onclose={() => showScheduleModal = false}>
  <div class="flex flex-col gap-1.5">
    <label for="sch-name" class={fieldLabel}>{$t('restic.schedule_name_label')} <span class="text-[var(--hv-error)]">*</span></label>
    <input id="sch-name" class={inp} type="text" bind:value={newSchedule.name} placeholder={$t('restic.schedule_name_placeholder')} required />
  </div>
  <div class="flex flex-col gap-1.5 mt-3">
    <label for="sch-cron" class={fieldLabel}>{$t('restic.cron_label')} <span class="text-[var(--hv-error)]">*</span></label>
    <input id="sch-cron" class={inp} type="text" bind:value={newSchedule.cron_expression} placeholder="0 2 * * *" required />
    <span class={hint}>{$t('restic.cron_hint')}</span>
  </div>
  <div class="flex gap-2 mt-3">
    <Button type="button" variant="ghost" size="sm" onclick={() => newSchedule.cron_expression = '0 2 * * *'}>{$t('restic.cron_preset_daily')}</Button>
    <Button type="button" variant="ghost" size="sm" onclick={() => newSchedule.cron_expression = '0 3 * * 0'}>{$t('restic.cron_preset_weekly')}</Button>
    <Button type="button" variant="ghost" size="sm" onclick={() => newSchedule.cron_expression = '0 4 1 * *'}>{$t('restic.cron_preset_monthly')}</Button>
  </div>
  {#snippet footer()}
    <div class="flex gap-2.5 justify-end">
      <Button variant="ghost" onclick={() => showScheduleModal = false}>{$t('common.cancel')}</Button>
      <Button variant="primary" onclick={createSchedule}>{$t('restic.create_schedule')}</Button>
    </div>
  {/snippet}
</Modal>

<!-- Retention Policy Manager Modal -->
<Modal open={showPolicyManagerModal} title={$t('restic.retention_policies_title')} size="md" onclose={() => showPolicyManagerModal = false}>
  <p class="m-0 mb-3 text-sm text-[var(--hv-shell-text)]">{$t('restic.retention_policies_desc')}</p>
  {#if retentionPolicies.length === 0}
    <p class="text-xs text-[var(--hv-shell-text-2)]">{$t('restic.no_policies')}</p>
  {:else}
    <div class="flex flex-col gap-2">
      {#each retentionPolicies as policy}
        <div class="flex items-center gap-3 px-3 py-2.5 rounded-xl bg-[var(--hv-shell-border-soft)] border border-[var(--hv-shell-border)]">
          <div class="flex-1 min-w-0">
            <div class="text-sm font-semibold text-[var(--hv-shell-text-strong)]">{policy.name}</div>
            <div class="text-xs text-[var(--hv-shell-text-2)]">{#if policy.max_count}{translate('restic.policy_summary_count', { count: String(policy.max_count) })}{/if}{#if policy.max_age_days} · {translate('restic.policy_summary_age', { days: String(policy.max_age_days) })}{/if}{#if policy.apply_to_all} · {$t('restic.policy_summary_default')}{/if}</div>
          </div>
          <button data-testid="restic-action-danger" class="grid place-items-center w-9 h-9 flex-none rounded-lg border border-[var(--hv-shell-border-strong)] bg-[var(--hv-shell-fill)] text-[var(--hv-shell-text-strong)] cursor-pointer hover:bg-[var(--hv-error-bg)] hover:text-[var(--hv-error)] hv-ic" onclick={() => deleteRetentionPolicy(policy)} title={$t('common.delete')} aria-label={$t('common.delete')}><IconTrash size={14} /></button>
        </div>
      {/each}
    </div>
  {/if}
  <div class="my-4 border-t border-[var(--hv-shell-border)]"></div>
  <div class="flex flex-col gap-1.5">
    <label for="pol-name" class={fieldLabel}>{$t('restic.policy_name_label')}</label>
    <input id="pol-name" class={inp} type="text" bind:value={newPolicy.name} placeholder={$t('restic.policy_name_placeholder')} />
  </div>
  <div class="grid grid-cols-2 @max-[360px]:grid-cols-1 gap-3 mt-3">
    <div class="flex flex-col gap-1.5"><label for="pol-count" class={fieldLabel}>{$t('restic.max_count_label')}</label><input id="pol-count" class={inp} type="number" bind:value={newPolicy.max_count} min="0" /></div>
    <div class="flex flex-col gap-1.5"><label for="pol-age" class={fieldLabel}>{$t('restic.max_age_days_label')}</label><input id="pol-age" class={inp} type="number" bind:value={newPolicy.max_age_days} min="0" /></div>
  </div>
  <div class="mt-3"><Toggle bind:checked={newPolicy.apply_to_all} label={$t('restic.apply_to_all_label')} /></div>
  {#snippet footer()}
    <div class="flex gap-2.5 justify-end">
      <Button variant="ghost" onclick={() => showPolicyManagerModal = false}>{$t('common.close')}</Button>
      <Button variant="primary" onclick={createRetentionPolicy}>{$t('restic.create_policy')}</Button>
    </div>
  {/snippet}
</Modal>
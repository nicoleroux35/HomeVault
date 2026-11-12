<!-- ========================================================================
     File          : Replication.svelte
     Author        : Nicolas Le Roux
     Created       : 31/10/2026 by Nicolas Le Roux
     Version       : 1.0.0

     Description:
     Snapshot replication (Backup → Replication): push ZFS/Btrfs snapshots to
     another dataset, locally or over SSH, incrementally and resumably. This
     screen creates and schedules the jobs and shows a dry run of the exact
     pipeline before anything is sent.

     Dependencies:
       - svelte (>= 5), theme + app composites, $lib/services/api

     Usage:
       Registered as the "Replication" desktop app.
     ======================================================================== -->
<script lang="ts">
  import { onMount } from 'svelte';
  import { api } from '$lib/services/api';
  import { t, translate } from '$lib/stores/i18n';
  import { Spinner, Button, Toggle, Modal, Alert } from '$lib/components/theme';
  import { PageLayout, ConfirmDialog, EmptyState } from '$lib/components/app';
  import { createConfirmState } from '$lib/utils/confirmDialog.svelte';
  import { notifyError, notifySuccess } from '$lib/utils/notify';
  import { cn } from '$lib/utils.js';
  import { IconPlus, IconPlayerPlay, IconTrash, IconTool, IconArrowsExchange } from '$lib/icons/lucide';

  interface ReplicationJob {
    id: number;
    name: string;
    source_dataset: string;
    backend: 'zfs' | 'btrfs';
    destination_kind: 'local' | 'ssh';
    destination_dataset: string;
    destination_host: string | null;
    destination_user: string | null;
    destination_port: number;
    interval_minutes: number;
    keep_last: number;
    bandwidth_kbps: number;
    enabled: boolean;
    next_run_at: string | null;
    last_run_at: string | null;
    last_snapshot: string | null;
    last_status: string | null;
    last_error: string | null;
    running: boolean;
    bandwidth_note: string | null;
  }

  interface Capabilities {
    zfs: boolean;
    btrfs: boolean;
    ssh: boolean;
    pv: boolean;
    max_keep_last: number;
    max_bandwidth_kbps: number;
    min_interval_minutes: number;
    max_interval_minutes: number;
    prefix: string;
  }

  let jobs = $state<ReplicationJob[]>([]);
  let capabilities = $state<Capabilities | null>(null);
  let loading = $state(true);
  let error = $state('');
  let busyId = $state<number | 'form' | null>(null);

  let modalOpen = $state(false);
  let editing = $state<number | null>(null);
  let formError = $state('');
  let form = $state({
    name: '',
    source_dataset: '',
    backend: 'zfs' as 'zfs' | 'btrfs',
    destination_kind: 'local' as 'local' | 'ssh',
    destination_dataset: '',
    destination_host: '',
    destination_user: '',
    destination_port: 22,
    interval_minutes: 1440,
    keep_last: 7,
    bandwidth_kbps: 0,
    enabled: true,
  });

  let preview = $state<{ snapshot: string; base: string | null; command: string; bandwidth_note: string | null } | null>(null);
  let previewBusy = $state<number | null>(null);
  let previewError = $state('');
  const dlg = createConfirmState();

  const field = 'flex flex-col gap-1.5';
  const fieldLabel = 'text-c-label font-semibold text-[var(--hv-shell-text-strong)]';
  const inp = 'w-full h-11 px-3 border border-[var(--hv-shell-border-strong)] rounded-lg bg-[var(--hv-shell-border-soft)] text-c-base text-[var(--hv-shell-text-strong)] outline-none focus:border-[var(--hv-primary)]';
  const hint = 'text-c-label text-[var(--hv-shell-text-2)]';
  const card = 'flex flex-col gap-3 p-4 rounded-xl bg-[var(--hv-shell-border-soft)] border border-[var(--hv-shell-border)]';

  onMount(load);

  async function load() {
    loading = true;
    error = '';
    try {
      const resp = await api.get('/replication');
      if (resp.success && resp.data) {
        jobs = resp.data.jobs as ReplicationJob[];
        capabilities = resp.data.capabilities as Capabilities;
      } else {
        error = resp.message || translate('replication.load_failed');
      }
    } catch {
      error = translate('replication.load_failed');
    } finally {
      loading = false;
    }
  }

  function openCreate() {
    editing = null;
    formError = '';
    form = {
      name: '',
      source_dataset: '',
      backend: 'zfs',
      destination_kind: 'local',
      destination_dataset: '',
      destination_host: '',
      destination_user: '',
      destination_port: 22,
      interval_minutes: 1440,
      keep_last: 7,
      bandwidth_kbps: 0,
      enabled: true,
    };
    modalOpen = true;
  }

  function openEdit(job: ReplicationJob) {
    editing = job.id;
    formError = '';
    form = {
      name: job.name,
      source_dataset: job.source_dataset,
      backend: job.backend,
      destination_kind: job.destination_kind,
      destination_dataset: job.destination_dataset,
      destination_host: job.destination_host ?? '',
      destination_user: job.destination_user ?? '',
      destination_port: job.destination_port,
      interval_minutes: job.interval_minutes,
      keep_last: job.keep_last,
      bandwidth_kbps: job.bandwidth_kbps,
      enabled: job.enabled,
    };
    modalOpen = true;
  }

  function payload() {
    return {
      name: form.name.trim(),
      source_dataset: form.source_dataset.trim(),
      backend: form.backend,
      destination_kind: form.destination_kind,
      destination_dataset: form.destination_dataset.trim(),
      destination_host: form.destination_kind === 'ssh' ? form.destination_host.trim() : null,
      destination_user: form.destination_kind === 'ssh' ? form.destination_user.trim() : null,
      destination_port: form.destination_kind === 'ssh' ? form.destination_port : 22,
      interval_minutes: form.interval_minutes,
      keep_last: form.keep_last,
      bandwidth_kbps: form.bandwidth_kbps,
      enabled: form.enabled,
    };
  }

  async function save() {
    if (!form.name.trim() || !form.source_dataset.trim() || !form.destination_dataset.trim()) {
      formError = translate('replication.required_fields');
      return;
    }
    busyId = 'form';
    formError = '';
    try {
      const resp = editing === null
        ? await api.post('/replication', payload())
        : await api.put(`/replication/${editing}`, payload());
      if (resp.success) {
        modalOpen = false;
        notifySuccess('common.saved');
        await load();
      } else {
        formError = resp.message || translate('replication.save_failed');
      }
    } catch {
      formError = translate('replication.save_failed');
    } finally {
      busyId = null;
    }
  }

  async function runNow(job: ReplicationJob) {
    if (!(await dlg.ask(translate('replication.run_confirm', { name: job.name })))) return;
    busyId = job.id;
    try {
      const resp = await api.post(`/replication/${job.id}/run`, {});
      if (resp.success) {
        notifySuccess('replication.run_done');
      } else {
        notifyError(resp);
      }
      await load();
    } finally {
      busyId = null;
    }
  }

  async function showPreview(job: ReplicationJob) {
    previewBusy = job.id;
    previewError = '';
    preview = null;
    try {
      const resp = await api.get(`/replication/${job.id}/preview`);
      if (resp.success && resp.data) {
        preview = resp.data as typeof preview;
      } else {
        previewError = resp.message || translate('replication.preview_failed');
      }
    } finally {
      previewBusy = null;
    }
  }

  async function removeJob(job: ReplicationJob) {
    if (!(await dlg.ask(translate('replication.delete_confirm', { name: job.name })))) return;
    busyId = job.id;
    try {
      const resp = await api.delete(`/replication/${job.id}`);
      if (resp.success) {
        notifySuccess('common.saved');
        await load();
      } else {
        notifyError(resp);
      }
    } finally {
      busyId = null;
    }
  }

  function scheduleLabel(job: ReplicationJob): string {
    if (job.interval_minutes === 0) return translate('replication.manual_only');
    const minutes = job.interval_minutes;
    if (minutes % 1440 === 0) {
      const days = minutes / 1440;
      return translate('replication.every_days', { days: String(days) });
    }
    if (minutes % 60 === 0) {
      return translate('replication.every_hours', { hours: String(minutes / 60) });
    }
    return translate('replication.every_minutes', { minutes: String(minutes) });
  }

  function destinationLabel(job: ReplicationJob): string {
    return job.destination_kind === 'ssh'
      ? `ssh://${job.destination_user}@${job.destination_host}:${job.destination_port}/${job.destination_dataset}`
      : job.destination_dataset;
  }
</script>

<PageLayout title={$t('replication.title')} data-testid="replication-page">
  {#snippet icon()}
    <IconArrowsExchange strokeWidth={1.8} />
  {/snippet}

  {#snippet toolbarRight()}
    <Button variant="primary" data-testid="replication-new" onclick={openCreate}>
      <IconPlus size={16} strokeWidth={1.8} /> {$t('replication.new_job')}
    </Button>
  {/snippet}

  {#if loading}
    <div data-testid="replication-loading" class="flex items-center justify-center gap-3 py-16 text-[var(--hv-shell-text)]">
      <Spinner />
      <span>{$t('common.loading')}</span>
    </div>
  {:else}
    <div data-testid="replication-content" class="flex flex-col gap-4 max-w-[880px]">
      {#if error}
        <Alert variant="error">{error}</Alert>
      {/if}
      {#if capabilities}
        <p class="m-0 text-c-label text-[var(--hv-shell-text-2)]">
          {$t('replication.capabilities', {
            zfs: capabilities.zfs ? $t('replication.available') : $t('replication.unavailable'),
            btrfs: capabilities.btrfs ? $t('replication.available') : $t('replication.unavailable'),
            ssh: capabilities.ssh ? $t('replication.available') : $t('replication.unavailable'),
            pv: capabilities.pv ? $t('replication.available') : $t('replication.unavailable'),
          })}
        </p>
      {/if}

      {#if jobs.length === 0}
        <EmptyState title={$t('replication.empty_title')} description={$t('replication.empty_desc')} />
      {:else}
        <div class="flex flex-col gap-3">
          {#each jobs as job (job.id)}
            <div data-testid="replication-job" class={card}>
              <div class="flex items-start justify-between gap-3 flex-wrap">
                <div class="flex flex-col min-w-0">
                  <span id={`replication-job-${job.id}`} class="font-semibold text-c-card-title text-[var(--hv-shell-text-strong)]">{job.name}</span>
                  <code class="font-[family-name:var(--hv-font-mono)] text-c-label text-[var(--hv-shell-text-2)] break-all">
                    {job.source_dataset} → {destinationLabel(job)}
                  </code>
                  <span class="text-c-label text-[var(--hv-shell-text-2)]">
                    {job.backend.toUpperCase()} · {scheduleLabel(job)} · {$t('replication.keep_last', { count: String(job.keep_last) })}
                    {#if job.bandwidth_kbps > 0}· {$t('replication.bandwidth', { rate: String(job.bandwidth_kbps) })}{/if}
                  </span>
                </div>
                <div class="flex items-center gap-1">
                  <button
                    type="button"
                    class="grid place-items-center w-9 h-9 flex-none rounded-lg border-none bg-transparent text-[var(--hv-shell-text-2)] cursor-pointer hover:bg-[var(--hv-subtle-hover)] hover:text-[var(--hv-shell-text-strong)] hv-ic"
                    aria-label={$t('replication.preview')}
                    data-testid="replication-preview-{job.id}"
                    disabled={previewBusy === job.id}
                    onclick={() => showPreview(job)}
                  ><IconTool size={17} strokeWidth={1.8} /></button>
                  <button
                    type="button"
                    class="grid place-items-center w-9 h-9 flex-none rounded-lg border-none bg-transparent text-[var(--hv-shell-text-2)] cursor-pointer hover:bg-[var(--hv-subtle-hover)] hover:text-[var(--hv-shell-text-strong)] hv-ic"
                    aria-label={$t('replication.run_now')}
                    data-testid="replication-run-{job.id}"
                    disabled={job.running || busyId === job.id}
                    onclick={() => runNow(job)}
                  ><IconPlayerPlay size={17} strokeWidth={1.8} /></button>
                  <button
                    type="button"
                    class="grid place-items-center w-9 h-9 flex-none rounded-lg border-none bg-transparent text-[var(--hv-shell-text-2)] cursor-pointer hover:bg-[var(--hv-subtle-hover)] hover:text-[var(--hv-shell-text-strong)] hv-ic"
                    aria-label={$t('tasks.edit', { name: job.name })}
                    data-testid="replication-edit-{job.id}"
                    onclick={() => openEdit(job)}
                  ><IconTool size={17} strokeWidth={1.8} /></button>
                  <button
                    type="button"
                    class="grid place-items-center w-9 h-9 flex-none rounded-lg border-none bg-transparent text-[var(--hv-shell-text-2)] cursor-pointer hover:bg-[var(--hv-error-bg)] hover:text-[var(--hv-error)] hv-ic"
                    aria-label={$t('tasks.delete', { name: job.name })}
                    data-testid="replication-delete-{job.id}"
                    onclick={() => removeJob(job)}
                  ><IconTrash size={17} strokeWidth={1.8} /></button>
                </div>
              </div>

              <div class="flex items-center gap-3 flex-wrap text-c-label text-[var(--hv-shell-text-2)]">
                <span class={job.enabled ? 'text-[var(--hv-success)]' : ''}>
                  {job.enabled ? $t('replication.enabled') : $t('tasks.enabled_off')}
                </span>
                {#if job.next_run_at}<span>→ {new Date(job.next_run_at).toLocaleString()}</span>{/if}
                {#if job.last_status}
                  <span class={job.last_status === 'success' ? 'text-[var(--hv-success)]' : 'text-[var(--hv-danger)]'}>
                    {job.last_status}{job.last_run_at ? ` · ${new Date(job.last_run_at).toLocaleString()}` : ''}
                  </span>
                {/if}
                {#if job.running}<span class="text-[var(--hv-primary)]">{$t('replication.running')}</span>{/if}
              </div>
              {#if job.last_snapshot}
                <span class="text-c-label text-[var(--hv-shell-text-2)]">{$t('replication.last_snapshot', { snapshot: job.last_snapshot })}</span>
              {/if}
              {#if job.last_error}
                <p class="m-0 text-c-label text-[var(--hv-danger)]" role="alert">{job.last_error}</p>
              {/if}
              {#if job.bandwidth_note}
                <p class="m-0 text-c-label text-[var(--hv-warning)]">{job.bandwidth_note}</p>
              {/if}
            </div>
          {/each}
        </div>
      {/if}
    </div>
  {/if}

  {#snippet footer()}
    {#if !loading}
      <span data-testid="replication-footer" class="flex items-center gap-2 flex-wrap">
        <span>{jobs.length} {$t('replication.footer_jobs')}</span>
        <span class="text-[var(--hv-shell-text-3)]">&middot;</span>
        <span>{jobs.filter((job) => job.enabled).length} {$t('replication.footer_enabled')}</span>
      </span>
    {/if}
  {/snippet}
</PageLayout>

<!-- Create / edit -->
<Modal open={modalOpen} title={editing === null ? $t('replication.new_job') : $t('replication.edit_job')} size="lg" onclose={() => (modalOpen = false)}>
  {#snippet icon()}
    <IconArrowsExchange size={22} strokeWidth={1.8} />
  {/snippet}
  {#snippet children()}
    <div class="flex flex-col gap-3">
      <label class={field}>
        <span class={fieldLabel}>{$t('replication.name')}</span>
        <input class={inp} bind:value={form.name} placeholder="nightly-to-nas2" data-testid="replication-form-name" />
      </label>
      <div class="grid gap-3 sm:grid-cols-2">
        <label class={field}>
          <span class={fieldLabel}>{$t('replication.backend')}</span>
          <select class={inp} bind:value={form.backend}>
            <option value="zfs">ZFS</option>
            <option value="btrfs">Btrfs</option>
          </select>
        </label>
        <label class={field}>
          <span class={fieldLabel}>{$t('replication.source_dataset')}</span>
          <input class={inp} bind:value={form.source_dataset} placeholder="tank/data" data-testid="replication-form-source" />
        </label>
      </div>
      <label class={field}>
        <span class={fieldLabel}>{$t('replication.destination_kind')}</span>
        <select class={inp} bind:value={form.destination_kind}>
          <option value="local">{$t('replication.kind_local')}</option>
          <option value="ssh">{$t('replication.kind_ssh')}</option>
        </select>
      </label>
      <label class={field}>
        <span class={fieldLabel}>{$t('replication.destination_dataset')}</span>
        <input class={inp} bind:value={form.destination_dataset} placeholder="backup/data" data-testid="replication-form-destination" />
      </label>
      {#if form.destination_kind === 'ssh'}
        <div class="grid gap-3 sm:grid-cols-3">
          <label class={field}>
            <span class={fieldLabel}>{$t('replication.host')}</span>
            <input class={inp} bind:value={form.destination_host} placeholder="nas2.lan" data-testid="replication-form-host" />
          </label>
          <label class={field}>
            <span class={fieldLabel}>{$t('replication.user')}</span>
            <input class={inp} bind:value={form.destination_user} placeholder="backup" />
          </label>
          <label class={field}>
            <span class={fieldLabel}>{$t('replication.port')}</span>
            <input class={inp} type="number" min="1" max="65535" bind:value={form.destination_port} />
          </label>
        </div>
        <p class={hint}>{$t('replication.ssh_hint')}</p>
      {/if}
      <div class="grid gap-3 sm:grid-cols-3">
        <label class={field}>
          <span class={fieldLabel}>{$t('replication.interval')}</span>
          <input class={inp} type="number" min="0" bind:value={form.interval_minutes} />
          <span class={hint}>{$t('replication.interval_hint')}</span>
        </label>
        <label class={field}>
          <span class={fieldLabel}>{$t('replication.keep_last_label')}</span>
          <input class={inp} type="number" min="1" bind:value={form.keep_last} />
          <span class={hint}>{$t('replication.keep_last_hint')}</span>
        </label>
        <label class={field}>
          <span class={fieldLabel}>{$t('replication.bandwidth_kbps')}</span>
          <input class={inp} type="number" min="0" bind:value={form.bandwidth_kbps} />
          <span class={hint}>{$t('replication.bandwidth_hint')}</span>
        </label>
      </div>
      <div class="flex items-center justify-between gap-4 flex-wrap">
        <span class="text-c-base font-semibold text-[var(--hv-shell-text-strong)]">{$t('replication.enabled')}</span>
        <Toggle bind:checked={form.enabled} label={$t('replication.enabled')} />
      </div>
      {#if formError}
        <Alert variant="error">{formError}</Alert>
      {/if}
    </div>
  {/snippet}
  {#snippet footer()}
    <Button variant="ghost" size="lg" onclick={() => (modalOpen = false)}>{$t('common.cancel')}</Button>
    <Button variant="primary" size="lg" onclick={save} disabled={busyId === 'form'}>{$t('common.save')}</Button>
  {/snippet}
</Modal>

<!-- Dry run -->
<Modal open={!!preview || !!previewError} title={$t('replication.preview')} size="lg" onclose={() => { preview = null; previewError = ''; }}>
  {#snippet icon()}
    <IconTool size={22} strokeWidth={1.8} />
  {/snippet}
  {#snippet children()}
    {#if previewError}
      <Alert variant="error">{previewError}</Alert>
    {:else if preview}
      <div class="flex flex-col gap-3">
        <p class="m-0 text-c-base text-[var(--hv-shell-text)] leading-relaxed">{$t('replication.preview_desc')}</p>
        <p class="m-0 text-c-label text-[var(--hv-shell-text-2)]">
          {$t('replication.preview_snapshot', { snapshot: preview.snapshot })}
          {#if preview.base}· {$t('replication.preview_base', { base: preview.base })}{/if}
        </p>
        <pre class="m-0 p-3 rounded-xl bg-[var(--hv-shell-border-soft)] border border-[var(--hv-shell-border)] overflow-x-auto text-c-label font-[family-name:var(--hv-font-mono)] text-[var(--hv-shell-text-strong)]" data-testid="replication-preview-command">{preview.command}</pre>
        {#if preview.bandwidth_note}
          <Alert variant="warning">{preview.bandwidth_note}</Alert>
        {/if}
      </div>
    {/if}
  {/snippet}
  {#snippet footer()}
    <Button variant="primary" size="lg" onclick={() => { preview = null; previewError = ''; }}>{$t('common.close')}</Button>
  {/snippet}
</Modal>

<ConfirmDialog {dlg} onconfirm={() => dlg.settle(true)} oncancel={() => dlg.settle(false)} />

<!-- ========================================================================
     File          : Tasks.svelte
     Author        : Nicolas Le Roux
     Created       : 13/10/2026 by Nicolas Le Roux
     Version       : 1.0.0

     Description:
     The Scheduled Tasks app (app id `tasks`): the generic cron engine's
     management screen — task table (name, type, cron, enabled, last run,
     next run), create/edit modal with a per-type payload form, per-task
     run history drawer, "Run now" and delete-with-confirm. Admin-only,
     like every endpoint it calls.

     Dependencies:
       - svelte (>= 5), theme + app/ composites, $lib/services/api

     Usage:
     Opened as a desktop window app.
     ======================================================================== -->
<script lang="ts">
  /**
   * Tasks — scheduled-task management (app id `tasks`).
   *
   * Payloads are edited as structured fields per task type and serialized
   * to the JSON the backend validates with `TaskAction::from_parts` (the
   * same gate runs server-side on every create/update — the client form is
   * convenience, not security).
   */
  import { onMount } from 'svelte';
  import { get } from 'svelte/store';
  import { api, type ScheduledTask, type ScheduledTaskRun } from '$lib/services/api';
  import { onAppVisible } from '$lib/utils/autoRevalidate';
  import { t, languageStore } from '$lib/stores/i18n';
  import { isAdmin } from '$lib/stores/auth';
  import { notificationsStore } from '$lib/stores/notifications';
  import { translate } from '$lib/stores/i18n';
  import { PageLayout, DataTable, EmptyState, SectionCard, ConfirmDialog } from '$lib/components/app';
  import { Modal, Button, Input, Select, Switch, Badge } from '$lib/components/theme';
  import { createConfirmState } from '$lib/utils/confirmDialog.svelte';
  import { IconClock, IconPlus, IconPlayerPlay, IconHistory, IconTrash } from '@tabler/icons-svelte';

  type TaskType = 'command' | 'snapshot' | 'smart_test' | 'power';

  interface TaskForm {
    id: string | null;
    name: string;
    task_type: TaskType;
    cron: string;
    enabled: boolean;
    // command
    command: string;
    args: string;
    // snapshot
    pool: string;
    recursive: boolean;
    // smart_test
    device: string;
    test_type: string;
    // power
    action: string;
  }

  const EMPTY_FORM: TaskForm = {
    id: null,
    name: '',
    task_type: 'snapshot',
    cron: '0 3 * * *',
    enabled: true,
    command: '',
    args: '',
    pool: '',
    recursive: false,
    device: '',
    test_type: 'short',
    action: 'reboot',
  };

  const TASK_TYPES: { id: TaskType; label: string }[] = [
    { id: 'snapshot', label: 'tasks.type_snapshot' },
    { id: 'smart_test', label: 'tasks.type_smart_test' },
    { id: 'power', label: 'tasks.type_power' },
    { id: 'command', label: 'tasks.type_command' },
  ];

  const SMART_TYPES = ['short', 'long', 'conveyance', 'offline'];
  const POWER_ACTIONS = ['reboot', 'shutdown', 'suspend'];

  let tasks: ScheduledTask[] = [];
  let loading = true;
  let loadError = false;
  let showForm = false;
  let form: TaskForm = { ...EMPTY_FORM };
  let formError = '';
  let saving = false;
  /** id of the task whose runs drawer is open, + its rows. */
  let runsTask: ScheduledTask | null = null;
  let runs: ScheduledTaskRun[] = [];
  let runsLoading = false;
  const dlg = createConfirmState();
  let busyId = '';

  onMount(() => {
    if (!$isAdmin) {
      window.location.href = '/desktop';
      return;
    }
    load();
    return onAppVisible(load);
  });

  async function load() {
    loading = true;
    loadError = false;
    try {
      const res = await api.getScheduledTasks();
      if (!res.success) {
        loadError = true;
        tasks = [];
        return;
      }
      tasks = res.data?.tasks ?? [];
    } catch {
      loadError = true;
      tasks = [];
    } finally {
      loading = false;
    }
  }

  function openCreate() {
    form = { ...EMPTY_FORM };
    formError = '';
    showForm = true;
  }

  function openEdit(task: ScheduledTask) {
    let payload: Record<string, unknown> = {};
    try {
      payload = JSON.parse(task.payload);
    } catch {
      payload = {};
    }
    form = {
      id: task.id,
      name: task.name,
      task_type: task.task_type as TaskType,
      cron: task.cron,
      enabled: task.enabled,
      command: String(payload.command ?? ''),
      args: Array.isArray(payload.args) ? payload.args.join(' ') : '',
      pool: String(payload.pool ?? ''),
      recursive: payload.recursive === true,
      device: String(payload.device ?? ''),
      test_type: String(payload.test_type ?? 'short'),
      action: String(payload.action ?? 'reboot'),
    };
    formError = '';
    showForm = true;
  }

  function buildPayload(): Record<string, unknown> {
    switch (form.task_type) {
      case 'command':
        return {
          command: form.command.trim(),
          args: form.args.trim() ? form.args.trim().split(/\s+/) : [],
        };
      case 'snapshot':
        return { pool: form.pool.trim(), recursive: form.recursive };
      case 'smart_test':
        return { device: form.device.trim(), test_type: form.test_type };
      case 'power':
        return { action: form.action };
    }
  }

  async function save() {
    if (!form.name.trim()) {
      formError = $t('tasks.error_name');
      return;
    }
    saving = true;
    formError = '';
    const body = {
      name: form.name.trim(),
      task_type: form.task_type,
      cron: form.cron.trim(),
      payload: buildPayload(),
      enabled: form.enabled,
    };
    try {
      const res = form.id
        ? await api.updateScheduledTask(form.id, body)
        : await api.createScheduledTask(body);
      if (!res.success) {
        formError = res.message || $t('tasks.error_save');
        return;
      }
      showForm = false;
      await load();
    } catch {
      formError = $t('tasks.error_save');
    } finally {
      saving = false;
    }
  }

  async function toggleEnabled(task: ScheduledTask, enabled: boolean) {
    busyId = task.id;
    try {
      const payload: Record<string, unknown> = JSON.parse(task.payload);
      await api.updateScheduledTask(task.id, {
        name: task.name,
        task_type: task.task_type,
        cron: task.cron,
        payload,
        enabled,
      });
      await load();
    } catch {
      await load();
    } finally {
      busyId = '';
    }
  }

  async function runNow(task: ScheduledTask) {
    busyId = task.id;
    try {
      const res = await api.runScheduledTaskNow(task.id);
      if (!res.success) {
        notificationsStore.add({ title: translate('desktop.app_tasks_name'), message: res.message || translate('tasks.error_save'), type: 'error' });
      }
      await load();
    } finally {
      busyId = '';
    }
  }

  async function openRuns(task: ScheduledTask) {
    runsTask = task;
    runsLoading = true;
    runs = [];
    try {
      const res = await api.getScheduledTaskRuns(task.id);
      runs = res.data?.runs ?? [];
    } catch {
      runs = [];
    } finally {
      runsLoading = false;
    }
  }

  async function askDelete(task: ScheduledTask) {
    if (await dlg.ask($t('tasks.delete_confirm_msg', { name: task.name }))) {
      try {
        await api.deleteScheduledTask(task.id);
      } finally {
        await load();
      }
    }
  }

  function statusBadge(status: string | null): { label: string; tone: 'success' | 'danger' | 'neutral' } {
    switch (status) {
      case 'success': return { label: $t('tasks.status_success'), tone: 'success' };
      case 'failed': return { label: $t('tasks.status_failed'), tone: 'danger' };
      case 'running': return { label: $t('tasks.status_running'), tone: 'neutral' };
      default: return { label: $t('tasks.status_never'), tone: 'neutral' };
    }
  }

  function formatDate(iso: string | null): string {
    if (!iso) return '—';
    const d = new Date(iso);
    if (Number.isNaN(d.getTime())) return '—';
    try {
      return new Intl.DateTimeFormat(get(languageStore), { dateStyle: 'medium', timeStyle: 'short' }).format(d);
    } catch {
      return iso.slice(0, 16).replace('T', ' ');
    }
  }

  const columns = [
    { id: 'name', label: 'tasks.col_name' },
    { id: 'type', label: 'tasks.col_type' },
    { id: 'cron', label: 'tasks.col_cron' },
    { id: 'status', label: 'tasks.col_last_run' },
    { id: 'next', label: 'tasks.col_next_run' },
    {
      id: 'actions',
      label: '',
      sortable: false,
      align: 'right' as const,
      // Four 44px hit squares can't share less than ~180px without their
      // transparent touch pads overlapping (pads catch pointer events even
      // when visually hidden) — width the cell instead of shrinking pads.
      class: 'min-w-[200px] whitespace-nowrap',
    },
  ];
</script>

<PageLayout
  title={$t('tasks.heading')}
  iconName="clock"
  data-testid="tasks-page"
>
  {#snippet icon()}
    <IconClock stroke={1.7} />
  {/snippet}

  {#snippet toolbarRight()}
    <Button
      data-testid="tasks-new-btn"
      onclick={openCreate}
    >
      <IconPlus size={16} stroke={2} aria-hidden="true" />
      {$t('tasks.new')}
    </Button>
  {/snippet}

  <div class="flex flex-col gap-4 max-w-[1100px]">
    {#if loading}
      <EmptyState data-testid="tasks-empty" icon="clock" title={$t('tasks.loading')} />
    {:else if loadError}
      <EmptyState data-testid="tasks-error" icon="alert-triangle" title={$t('tasks.error_load')} />
    {:else if tasks.length === 0}
      <EmptyState
        data-testid="tasks-empty"
        icon="clock"
        title={$t('tasks.empty_title')}
        description={$t('tasks.empty_desc')}
      />
    {:else}
      <SectionCard title={$t('tasks.list_title')} description={$t('tasks.list_desc')} data-testid="tasks-list">
        <DataTable
          {columns}
          rows={tasks}
          rowKey={(r) => r.id}
          label={$t('tasks.heading')}
        >
          {#snippet cell({ row, column })}
            {#if column.id === 'name'}
              <div class="flex flex-col">
                <span class="font-semibold text-[var(--hv-shell-text-strong)]">{row.name}</span>
                <div class="flex items-center gap-2 mt-1">
                  <Switch
                    checked={row.enabled}
                    disabled={busyId === row.id}
                    ariaLabel={$t('tasks.toggle_enabled', { name: row.name })}
                    data-testid={`tasks-toggle-${row.id}`}
                    onchange={(v: boolean) => toggleEnabled(row, v)}
                  />
                  <span class="text-c-label text-[var(--hv-shell-text-2)]">
                    {row.enabled ? $t('tasks.enabled_on') : $t('tasks.enabled_off')}
                  </span>
                </div>
              </div>
            {:else if column.id === 'type'}
              {$t(`tasks.type_${row.task_type}`)}
            {:else if column.id === 'cron'}
              <code class="text-c-label">{row.cron}</code>
            {:else if column.id === 'status'}
              {@const badge = statusBadge(row.last_status)}
              <div class="flex flex-col items-start gap-1">
                <Badge tone={badge.tone} data-testid={`tasks-status-${row.id}`}>{badge.label}</Badge>
                <span class="text-c-label text-[var(--hv-shell-text-2)]">{formatDate(row.last_run_at)}</span>
                {#if row.last_error}
                  <span class="text-c-label text-[var(--hv-danger)] max-w-[240px] truncate" title={row.last_error}>{row.last_error}</span>
                {/if}
              </div>
            {:else if column.id === 'next'}
              {formatDate(row.next_run_at)}
            {:else if column.id === 'actions'}
              <div class="flex items-center justify-end gap-1">
                <button
                  type="button"
                  class="grid place-items-center w-9 h-9 flex-none rounded-lg border-none bg-transparent text-[var(--hv-shell-text-2)] cursor-pointer hover:bg-[var(--hv-subtle-hover)] hover:text-[var(--hv-shell-text-strong)] hv-ic"
                  data-testid={`tasks-run-${row.id}`}
                  aria-label={$t('tasks.run_now', { name: row.name })}
                  disabled={busyId === row.id}
                  onclick={() => runNow(row)}
                >
                  <IconPlayerPlay size={18} stroke={1.8} aria-hidden="true" />
                </button>
                <button
                  type="button"
                  class="grid place-items-center w-9 h-9 flex-none rounded-lg border-none bg-transparent text-[var(--hv-shell-text-2)] cursor-pointer hover:bg-[var(--hv-subtle-hover)] hover:text-[var(--hv-shell-text-strong)] hv-ic"
                  data-testid={`tasks-runs-${row.id}`}
                  aria-label={$t('tasks.runs', { name: row.name })}
                  onclick={() => openRuns(row)}
                >
                  <IconHistory size={18} stroke={1.8} aria-hidden="true" />
                </button>
                <button
                  type="button"
                  class="grid place-items-center w-9 h-9 flex-none rounded-lg border-none bg-transparent text-[var(--hv-shell-text-2)] cursor-pointer hover:bg-[var(--hv-subtle-hover)] hover:text-[var(--hv-shell-text-strong)] hv-ic"
                  data-testid={`tasks-edit-${row.id}`}
                  aria-label={$t('tasks.edit', { name: row.name })}
                  onclick={() => openEdit(row)}
                >
                  <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M17 3a2.85 2.83 4 0 1 4 4L7.5 20.5 2 22l1.5-5.5Z"/></svg>
                </button>
                <button
                  type="button"
                  class="grid place-items-center w-9 h-9 flex-none rounded-lg border-none bg-transparent text-[var(--hv-shell-text-2)] cursor-pointer hover:bg-[var(--hv-subtle-hover)] hover:text-[var(--hv-shell-text-strong)] hv-ic"
                  data-testid={`tasks-delete-${row.id}`}
                  aria-label={$t('tasks.delete', { name: row.name })}
                  onclick={() => askDelete(row)}
                >
                  <IconTrash size={18} stroke={1.8} aria-hidden="true" />
                </button>
              </div>
            {/if}
          {/snippet}
        </DataTable>
      </SectionCard>
    {/if}
  </div>
</PageLayout>

<!-- Create / edit modal -->
<Modal
  open={showForm}
  title={form.id ? $t('tasks.edit_title') : $t('tasks.new_title')}
  size="lg"
  onclose={() => (showForm = false)}
>
  <div class="flex flex-col gap-4 p-1">
    {#if formError}
      <p data-testid="tasks-form-error" role="alert" class="m-0 text-c-label text-[var(--hv-danger)]">{formError}</p>
    {/if}
    <Input label={$t('tasks.field_name')} bind:value={form.name} maxlength={120} />
    <Select
      label={$t('tasks.field_type')}
      bind:value={form.task_type}
      options={TASK_TYPES.map((t2) => ({ value: t2.id, label: $t(t2.label) }))}
    />
    <Input
      label={$t('tasks.field_cron')}
      bind:value={form.cron}
      placeholder="0 3 * * *"
      helper={$t('tasks.field_cron_help')}
    />

    {#if form.task_type === 'command'}
      <Input label={$t('tasks.field_command')} bind:value={form.command} placeholder="/usr/local/bin/script.sh" helper={$t('tasks.field_command_help')} />
      <Input label={$t('tasks.field_args')} bind:value={form.args} helper={$t('tasks.field_args_help')} />
    {:else if form.task_type === 'snapshot'}
      <Input label={$t('tasks.field_pool')} bind:value={form.pool} placeholder="tank/data" helper={$t('tasks.field_pool_help')} />
      <Switch bind:checked={form.recursive} label={$t('tasks.field_recursive')} />
    {:else if form.task_type === 'smart_test'}
      <Input label={$t('tasks.field_device')} bind:value={form.device} placeholder="sda" helper={$t('tasks.field_device_help')} />
      <Select
        label={$t('tasks.field_test_type')}
        bind:value={form.test_type}
        options={SMART_TYPES.map((s) => ({ value: s, label: $t(`tasks.smart_${s}`) }))}
      />
    {:else if form.task_type === 'power'}
      <Select
        label={$t('tasks.field_action')}
        bind:value={form.action}
        options={POWER_ACTIONS.map((a) => ({ value: a, label: $t(`tasks.power_${a}`) }))}
      />
    {/if}
    <Switch bind:checked={form.enabled} label={$t('tasks.field_enabled')} data-testid="tasks-field-enabled" />
  </div>
  {#snippet footer()}
    <div class="flex justify-end gap-2">
      <Button variant="ghost" onclick={() => (showForm = false)}>{$t('common.cancel')}</Button>
      <Button onclick={save} disabled={saving} data-testid="tasks-save-btn">{$t('common.save')}</Button>
    </div>
  {/snippet}
</Modal>

<!-- Run history modal -->
<Modal
  open={runsTask !== null}
  title={runsTask ? $t('tasks.runs_title', { name: runsTask.name }) : ''}
  size="lg"
  onclose={() => (runsTask = null)}
>
  {#if runsLoading}
    <EmptyState icon="history" title={$t('tasks.loading')} />
  {:else if runs.length === 0}
    <EmptyState icon="history" title={$t('tasks.runs_empty')} />
  {:else}
    <div class="flex flex-col gap-3" data-testid="tasks-runs-list">
      {#each runs as run (run.id)}
        <div class="rounded-xl border border-[var(--hv-shell-border)] bg-[var(--hv-shell-fill)] p-3">
          <div class="flex items-center justify-between gap-2">
            {#if run.status === 'success'}<Badge tone="success">{$t('tasks.status_success')}</Badge>
            {:else if run.status === 'failed'}<Badge tone="danger">{$t('tasks.status_failed')}</Badge>
            {:else}<Badge tone="neutral">{$t('tasks.status_running')}</Badge>{/if}
            <span class="text-c-label text-[var(--hv-shell-text-2)]">{formatDate(run.started_at)}</span>
          </div>
          {#if run.output}
            <pre class="mt-2 m-0 p-2 rounded-lg bg-[var(--hv-shell-border-soft)] text-c-label whitespace-pre-wrap break-words max-h-40 overflow-auto">{run.output}</pre>
          {/if}
        </div>
      {/each}
    </div>
  {/if}
</Modal>

<ConfirmDialog
  {dlg}
  title={$t('tasks.delete_confirm_title')}
  confirmLabel={$t('common.delete')}
  onconfirm={() => dlg.settle(true)}
  oncancel={() => dlg.settle(false)}
/>

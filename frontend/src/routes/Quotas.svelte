<!-- ========================================================================
     File          : Quotas.svelte
     Author        : Nicolas Le Roux
     Created       : 28/09/2026
     Last modified : 28/09/2026 by Nicolas Le Roux
     Version       : 1.0.0

     Description:
     The read-only Quotas app: per-user transfer quota limits and current
     usage (management itself lives in Settings).

     Dependencies:
       - svelte (>= 5), theme + app/ composites, $lib/services/api

     Usage:
     Opened as a desktop window app.
     ======================================================================== -->
<script lang="ts">
  /**
   * Quotas — read-only transfer-quota overview (app id `quotas`).
   *
   * One row per configured subject (user or group), with the real usage the
   * per-subject quota row already carries (`upload_used_bytes` /
   * `download_used_bytes`). Data comes from the existing
   * `GET /transfer-quotas/{subject_type}` list endpoint, which is
   * `require_auth`; this screen is nonetheless admin-gated like `Users.svelte`
   * because editing lives there and least privilege is the default.
   *
   * READ-ONLY by design: no control on this screen writes. The Users app's
   * "Transfer quotas" card owns editing, and "Edit in Users" hands off to it
   * through the shared `hv-open-app` window event (same convention as
   * Explorer -> Shares; `Desktop.svelte` owns the actual `openApp()`).
   */
  import { onMount } from 'svelte';
  import { get } from 'svelte/store';
  import { api } from '$lib/services/api';
  import { onAppVisible } from '$lib/utils/autoRevalidate';
  import { t, languageStore } from '$lib/stores/i18n';
  import { isAdmin } from '$lib/stores/auth';
  import { PageLayout, DataTable, EmptyState } from '$lib/components/app';
  import { cn } from '$lib/utils.js';
  import { IconGauge, IconUsers, IconShield, IconArrowRight } from '@tabler/icons-svelte';

  type QuotaFilter = 'all' | 'user' | 'group';

  interface QuotaRow {
    subjectType: 'user' | 'group';
    subjectName: string;
    uploadEnabled: boolean;
    uploadLimitBytes: number;
    uploadUsedBytes: number;
    downloadEnabled: boolean;
    downloadLimitBytes: number;
    downloadUsedBytes: number;
    updatedAt: string;
    /** ROADMAP 23.2 — `lifetime` | `day` | `week` | `month`. */
    period: string;
    /** When the counters next reset (RFC3339); `null` for a lifetime quota. */
    periodResetsAt: string | null;
  }

  let rows: QuotaRow[] = [];
  let loading = true;
  let loadError = false;
  let filter: QuotaFilter = 'all';

  const FILTERS: { id: QuotaFilter; label: string }[] = [
    { id: 'all', label: 'quotas.filter_all' },
    { id: 'user', label: 'quotas.filter_users' },
    { id: 'group', label: 'quotas.filter_groups' },
  ];

  // Locale-aware byte formatting — units and decimal separator follow the
  // active UI language via Intl (never hard-coded "Go"/"Mo"). Same shape as
  // Users.svelte's; see docs/guides/quotas.md for the known duplication.
  const BYTE_UNITS = ['byte', 'kilobyte', 'megabyte', 'gigabyte', 'terabyte'];
  function formatBytes(bytes: number): string {
    if (!bytes) return '—';
    let i = 0;
    let value = bytes;
    while (value >= 1024 && i < BYTE_UNITS.length - 1) { value /= 1024; i++; }
    try {
      return new Intl.NumberFormat(get(languageStore), {
        style: 'unit', unit: BYTE_UNITS[i], unitDisplay: 'short',
        maximumFractionDigits: 1,
      }).format(value);
    } catch {
      return `${value.toFixed(1)} ${BYTE_UNITS[i]}`;
    }
  }
  function formatDate(iso: string): string {
    if (!iso) return '—';
    const d = new Date(iso);
    if (Number.isNaN(d.getTime())) return '—';
    try {
      return new Intl.DateTimeFormat(get(languageStore), { dateStyle: 'medium', timeStyle: 'short' }).format(d);
    } catch {
      return iso.slice(0, 16).replace('T', ' ');
    }
  }

  // The counting window, in plain language. An unknown value (a row written
  // by a newer backend) falls back to the raw string rather than a wrong label.
  const PERIOD_KEYS: Record<string, string> = {
    lifetime: 'quotas.period_lifetime',
    day: 'quotas.period_day',
    week: 'quotas.period_week',
    month: 'quotas.period_month',
  };
  function periodLabel(period: string): string {
    const key = PERIOD_KEYS[period];
    return key ? get(t)(key) : period;
  }
  function formatResetDate(iso: string | null): string {
    if (!iso) return '';
    const d = new Date(iso);
    if (Number.isNaN(d.getTime())) return '';
    try {
      return new Intl.DateTimeFormat(get(languageStore), { dateStyle: 'medium' }).format(d);
    } catch {
      return iso.slice(0, 10);
    }
  }

  // A disabled direction means "no limit" — not "zero allowed".
  function barPct(used: number, enabled: boolean, limit: number): number {
    if (!enabled || limit <= 0) return 0;
    return Math.min(100, Math.round((used / limit) * 100));
  }

  onMount(() => {
    if (!$isAdmin) {
      window.location.href = '/desktop';
      return;
    }
    load();
    // No manual refresh button — revalidate when the window regains attention.
    return onAppVisible(load);
  });

  async function load() {
    loading = true;
    loadError = false;
    try {
      const [userRes, groupRes] = await Promise.all([
        api.listTransferQuotas('user'),
        api.listTransferQuotas('group'),
      ]);
      if (!userRes.success || !groupRes.success) {
        loadError = true;
        rows = [];
        return;
      }
      const toRows = (subjectType: 'user' | 'group', data: any[]): QuotaRow[] =>
        (data || []).map((q) => ({
          subjectType,
          subjectName: q.subject_name,
          uploadEnabled: q.upload_enabled,
          uploadLimitBytes: q.upload_limit_bytes,
          uploadUsedBytes: q.upload_used_bytes,
          downloadEnabled: q.download_enabled,
          downloadLimitBytes: q.download_limit_bytes,
          downloadUsedBytes: q.download_used_bytes,
          updatedAt: q.updated_at,
          period: q.period || 'lifetime',
          periodResetsAt: q.period_resets_at || null,
        }));
      rows = [...toRows('user', userRes.data), ...toRows('group', groupRes.data)]
        .sort((a, b) => a.subjectType.localeCompare(b.subjectType) || a.subjectName.localeCompare(b.subjectName));
    } catch {
      loadError = true;
      rows = [];
    } finally {
      loading = false;
    }
  }

  /** Hand off to the Users app, which owns quota editing. */
  function editInUsers() {
    window.dispatchEvent(new CustomEvent('hv-open-app', { detail: { id: 'users' } }));
  }

  $: filtered = rows.filter((r) => filter === 'all' || r.subjectType === filter);
  $: limitedCount = rows.filter((r) => r.uploadEnabled || r.downloadEnabled).length;
</script>

<PageLayout
  title={$t('quotas.heading')}
  data-testid="quotas-page"
>
  {#snippet icon()}
    <IconGauge stroke={1.7} />
  {/snippet}

  {#snippet toolbarRight()}
    <button
      type="button"
      data-testid="quotas-edit-btn"
      class="inline-flex items-center gap-2 h-11 px-4 border border-[var(--hv-shell-border-strong)] rounded-lg bg-[var(--hv-shell-fill)] text-[var(--hv-shell-text-strong)] text-c-base font-semibold cursor-pointer transition-colors hover:bg-[var(--hv-shell-border)]"
      onclick={editInUsers}
    >
      {$t('quotas.manage_in_users')} <IconArrowRight size={16} stroke={2} aria-hidden="true" />
    </button>
  {/snippet}

  {#snippet toolbarLeft()}
    <div
      data-testid="quotas-filters"
      class="flex flex-wrap gap-1 p-1.5 rounded-2xl bg-[var(--hv-shell-fill)] border border-[var(--hv-shell-border)]"
      role="group"
      aria-label={$t('quotas.heading')}
    >
      {#each FILTERS as f (f.id)}
        <button
          type="button"
          data-testid="quotas-filter"
          class:active={filter === f.id}
          aria-pressed={filter === f.id}
          onclick={() => (filter = f.id)}
          class={cn(
            'inline-flex items-center gap-2 h-9 px-4 border-none rounded-lg bg-transparent text-[var(--hv-shell-text-2)] text-c-base font-medium cursor-pointer transition-colors hover:bg-[var(--hv-shell-border-soft)]',
            filter === f.id && 'bg-[var(--hv-primary-selected-bg)] text-[var(--hv-primary)] font-semibold',
          )}
        >
          {$t(f.label)}
        </button>
      {/each}
    </div>
  {/snippet}

  <div class="hv-quotas-body flex flex-col gap-4 max-w-[1100px]">
    <p data-testid="quotas-count" class="m-0 text-c-label text-[var(--hv-shell-text)]">
      {$t('users.list_count', { count: String(filtered.length) })} · {$t('users.sheet_quota')} ({limitedCount})
    </p>

    {#if loading}
      <EmptyState data-testid="quotas-empty" icon="gauge" title={$t('quotas.loading')} />
    {:else if loadError}
      <EmptyState data-testid="quotas-error" icon="alert-triangle" title={$t('quotas.error')} />
    {:else if filtered.length === 0}
      <EmptyState data-testid="quotas-empty" icon="gauge" title={$t('quotas.empty')} />
    {:else}
      <DataTable
        rows={filtered}
        rowKey={(r) => r.subjectType + ':' + r.subjectName}
        columns={[
          { id: 'type', label: '' },
          { id: 'name', label: '' },
          { id: 'upload', label: $t('quotas.upload') },
          { id: 'download', label: $t('quotas.download') },
        ]}
        label={$t('quotas.heading')}
        class="rounded-xl bg-card border border-border shadow-[var(--hv-shell-shadow-card)] p-2"
      >
        {#snippet cell({ row, column })}
          {#if column.id === 'type'}
            <span class="inline-flex items-center gap-1.5 text-c-xs font-bold uppercase tracking-wide text-[var(--hv-shell-text)]">
              {#if row.subjectType === 'user'}
                <IconUsers size={14} stroke={2} aria-hidden="true" /> {$t('quotas.type_user')}
              {:else}
                <IconShield size={14} stroke={2} aria-hidden="true" /> {$t('quotas.type_group')}
              {/if}
            </span>
          {:else if column.id === 'name'}
            <span class="flex flex-col gap-0.5 min-w-0">
              <span class="font-[family-name:var(--hv-font-mono)] text-c-base font-semibold text-[var(--hv-shell-text-strong)] truncate">{row.subjectName}</span>
              <span class="text-c-xs text-[var(--hv-shell-text)] truncate">{$t('quotas.updated', { date: formatDate(row.updatedAt) })}</span>
              <span data-testid="quotas-period" class="text-c-xs text-[var(--hv-shell-text-2)] truncate">
                {periodLabel(row.period)}{#if row.periodResetsAt} · {$t('quotas.resets_on', { date: formatResetDate(row.periodResetsAt) })}{/if}
              </span>
            </span>
          {:else if column.id === 'upload'}
            <span class="flex flex-col gap-1.5 min-w-[150px]">
              {#if row.uploadEnabled}
                <span class="text-c-xs text-[var(--hv-shell-text-2)] font-[family-name:var(--hv-font-mono)]">{$t('quotas.usage', { used: formatBytes(row.uploadUsedBytes), limit: formatBytes(row.uploadLimitBytes) })}</span>
                <span class="block h-2 w-full rounded-full bg-[var(--hv-shell-fill)] overflow-hidden" role="progressbar" aria-valuenow={barPct(row.uploadUsedBytes, row.uploadEnabled, row.uploadLimitBytes)} aria-valuemin="0" aria-valuemax="100">
                  <span class="block h-full rounded-full bg-[var(--hv-primary)]" style="width:{barPct(row.uploadUsedBytes, row.uploadEnabled, row.uploadLimitBytes)}%"></span>
                </span>
              {:else}
                <span class="text-c-xs text-[var(--hv-shell-text)]">{$t('quotas.unlimited')}</span>
              {/if}
            </span>
          {:else if column.id === 'download'}
            <span class="flex flex-col gap-1.5 min-w-[150px]">
              {#if row.downloadEnabled}
                <span class="text-c-xs text-[var(--hv-shell-text-2)] font-[family-name:var(--hv-font-mono)]">{$t('quotas.usage', { used: formatBytes(row.downloadUsedBytes), limit: formatBytes(row.downloadLimitBytes) })}</span>
                <span class="block h-2 w-full rounded-full bg-[var(--hv-shell-fill)] overflow-hidden" role="progressbar" aria-valuenow={barPct(row.downloadUsedBytes, row.downloadEnabled, row.downloadLimitBytes)} aria-valuemin="0" aria-valuemax="100">
                  <span class="block h-full rounded-full bg-[var(--hv-primary)]" style="width:{barPct(row.downloadUsedBytes, row.downloadEnabled, row.downloadLimitBytes)}%"></span>
                </span>
              {:else}
                <span class="text-c-xs text-[var(--hv-shell-text)]">{$t('quotas.unlimited')}</span>
              {/if}
            </span>
          {/if}
        {/snippet}
      </DataTable>
    {/if}
  </div>
</PageLayout>

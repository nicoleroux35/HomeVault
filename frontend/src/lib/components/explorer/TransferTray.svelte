<!-- ========================================================================
     File          : TransferTray.svelte
     Author        : Nicolas Le Roux
     Created       : 31/10/2026 by Nicolas Le Roux
     Version       : 1.0.0

     Description:
       The Explorer's transfer tray (HV-UX-01): three zones — uploads,
       downloads, copies — with a live badge each, a progress bar per row, the
       enriched detail line (bytes transferred / total, throughput, time
       remaining, copy method) and a per-row cancel button.

     Dependencies:
       - svelte (>= 5), $lib/services/api, stores/i18n,
         utils/transferFormat, @tabler/icons-svelte

     Usage:
       Rendered inside the Explorer's "Transfers" modal:
       `<TransferTray onclose={() => (showTransfers = false)} />`.
     ======================================================================== -->
<script lang="ts">
  /**
   * Live transfer tray for the File Explorer.
   *
   * Polls `GET /api/explorer/transfers/active` (never SSE): the same request
   * carries the chunked uploads — which are rows in `upload_sessions`, not
   * progress jobs — and the copy/move/checksum jobs, already grouped into
   * zones by the server. One poll, one source of truth, and it is correct
   * immediately after a restart because the upload half is read from the
   * database.
   *
   * Every row can be stopped: an upload through
   * `DELETE /api/explorer/upload/session`, a job through
   * `DELETE /api/progress/{id}` (which runs the producer's cancellation hook,
   * so the work actually stops rather than only disappearing from the list).
   */
  import { onDestroy, onMount } from 'svelte';
  import {
    api,
    type ActiveTransfers,
    type TransferHistoryEntry,
    type TransferZone,
  } from '$lib/services/api';
  import { t, languageStore } from '$lib/stores/i18n';
  import {
    formatBytes,
    progressPercent,
    transferDetailLine,
  } from '$lib/utils/transferFormat';
  import { IconAlertCircle, IconDownload, IconFolderSymlink, IconUpload, IconX } from '@tabler/icons-svelte';

  let { onclose }: { onclose: () => void } = $props();

  /** Poll cadence. 2 s matches the desktop activity feed; faster gains
   *  nothing visible and doubles the request rate for every open tray. */
  const POLL_MS = 2000;

  let data = $state<ActiveTransfers | null>(null);
  let zone = $state<TransferZone>('upload');
  let failed = $state(false);
  /** Rows whose cancellation has been requested (button shown disabled). */
  let cancelling = $state<string[]>([]);
  let timer: ReturnType<typeof setTimeout> | undefined;
  let stopped = false;
  /** The audit trail, loaded when the section is first opened (HV-ADV-02). */
  let history = $state<TransferHistoryEntry[]>([]);
  let historyLoaded = $state(false);

  const zones: { id: TransferZone; label: string; icon: typeof IconUpload }[] = [
    { id: 'upload', label: 'explorer.transfer_zone_uploads', icon: IconUpload },
    { id: 'download', label: 'explorer.transfer_zone_downloads', icon: IconDownload },
    { id: 'copy', label: 'explorer.transfer_zone_copies', icon: IconFolderSymlink },
  ];

  function count(zoneId: TransferZone): number {
    if (!data) return 0;
    if (zoneId === 'upload') return data.counts.uploads;
    if (zoneId === 'download') return data.counts.downloads;
    return data.counts.copies;
  }

  async function poll() {
    if (stopped) return;
    try {
      const res = await api.getActiveTransfers();
      if (res.success && res.data) {
        data = res.data;
        failed = false;
      } else {
        failed = true;
      }
    } catch {
      failed = true;
    }
    if (stopped) return;
    timer = setTimeout(poll, POLL_MS);
  }

  /** Read the last ten finished transfers. Loaded on demand: a user who opens
   *  the tray to cancel something should not wait for a 90-day query. */
  async function loadHistory() {
    if (historyLoaded) return;
    const res = await api.getTransferHistory({ limit: 10 });
    if (res.success && res.data) {
      history = res.data.entries;
      historyLoaded = true;
    }
  }

  onMount(() => {
    poll();
    return () => {
      stopped = true;
      if (timer) clearTimeout(timer);
    };
  });

  onDestroy(() => {
    stopped = true;
    if (timer) clearTimeout(timer);
  });

  /** Uploads are aborted through their session; jobs through the progress API. */
  async function cancel(kind: 'upload' | 'job', id: string, cancelPath: string | null) {
    if (cancelling.includes(id)) return;
    cancelling = [...cancelling, id];
    try {
      if (kind === 'upload') {
        await api.uploadSessionAbort(id);
      } else if (cancelPath) {
        // `cancelPath` is API-relative (`/progress/{id}`); the client prefixes
        // `/api`, so the session token and contract header come along.
        await api.delete(cancelPath);
      }
      // Re-read immediately so the row disappears now instead of on the next
      // tick — feedback for a click must not wait for a poll.
      const res = await api.getActiveTransfers();
      if (res.success && res.data) data = res.data;
    } finally {
      cancelling = cancelling.filter((c) => c !== id);
    }
  }

  let visibleRows = $derived(
    data
      ? zone === 'upload'
        ? data.uploads.map((u) => ({
            id: u.session_id,
            kind: 'upload' as const,
            kindLabel: 'upload' as const,
            name: u.filename,
            where: u.dir,
            state: u.state as string,
            progress: u.progress,
            bytes_processed: u.bytes_received,
            bytes_total: u.bytes_total,
            speed_bps: u.speed_bps,
            eta_seconds: u.eta_seconds,
            method: null as string | null,
            detail: null as string | null,
            cancelPath: null as string | null,
          }))
        : data.transfers
            .filter((row) => row.zone === zone)
            .map((row) => ({
              id: row.id,
              kind: (row.kind.startsWith('download') ? 'download' : 'job') as 'download' | 'job',
              kindLabel: row.kind.startsWith('download')
                ? ('download' as const)
                : row.kind === 'explorer_checksum'
                  ? ('checksum' as const)
                  : ('copy' as const),
              name: row.label,
              where: '',
              state: row.state,
              progress: row.progress,
              bytes_processed: row.bytes_processed,
              bytes_total: row.bytes_total,
              speed_bps: row.speed_bps,
              eta_seconds: row.eta_seconds,
              method: row.method,
              detail: row.detail,
              cancelPath: row.cancel_path,
            }))
      : [],
  );
</script>

<div class="flex flex-col gap-3" data-testid="transfer-tray">
  <div class="flex items-center gap-1.5" role="group" aria-label={$t('explorer.transfer_zones')}>
    {#each zones as z (z.id)}
      <button
        type="button"
        data-testid={`transfer-tab-${z.id}`}
        aria-pressed={zone === z.id}
        class="flex min-h-11 items-center gap-1.5 rounded-lg border px-3 text-c-sm cursor-pointer transition-colors {zone === z.id
          ? 'border-[var(--hv-primary)] bg-[var(--hv-primary-soft)] text-[var(--hv-primary)] font-semibold'
          : 'border-[var(--hv-border-soft)] bg-transparent text-[var(--hv-text-2)] hover:bg-[var(--hv-subtle-hover)]'}"
        onclick={() => (zone = z.id)}
      >
        <z.icon size={15} aria-hidden="true" />
        <span>{$t(z.label)}</span>
        <span
          data-testid={`transfer-count-${z.id}`}
          class="min-w-5 rounded-full px-1.5 py-0.5 text-center text-c-xs font-semibold {count(z.id) > 0
            ? 'bg-[var(--hv-primary)] text-[var(--hv-on-primary)]'
            : 'bg-[var(--hv-subtle)] text-[var(--hv-text-3)]'}"
          aria-label={$t('explorer.transfer_zone_count', { count: String(count(z.id)) })}
        >{count(z.id)}</span>
      </button>
    {/each}
  </div>

  {#if failed}
    <p class="m-0 flex items-center gap-2 text-c-sm text-[var(--hv-danger)]" role="status">
      <IconAlertCircle size={15} aria-hidden="true" />
      {$t('explorer.transfer_load_failed')}
    </p>
  {/if}

  {#if visibleRows.length === 0}
    <p class="m-0 py-6 text-center text-c-sm text-[var(--hv-text-2)]" data-testid="transfer-empty">
      {$t('explorer.transfer_empty')}
    </p>
  {:else}
    <ul class="m-0 flex list-none flex-col gap-1.5 p-0">
      {#each visibleRows as row (row.id)}
        <li
          class="rounded-lg border border-[var(--hv-border-soft)] bg-[var(--hv-surface)] p-2.5"
          data-testid="transfer-row"
        >
          <div class="flex items-center gap-2">
            <span class="min-w-0 flex-1 truncate text-c-sm font-medium text-[var(--hv-text)]">
              {row.name || $t(`explorer.transfer_kind_${row.kindLabel}`)}
            </span>
            <!-- Only offered when the row can really be stopped: an upload
                 through its session, a job through its producer's hook. A copy
                 job has no hook, and a button that only hides the row while
                 the bytes keep moving would be a lie. -->
            {#if row.kind === 'upload' || row.cancelPath}
              <button
                type="button"
                class="hv-ic inline-flex items-center justify-center rounded-lg border border-[var(--hv-border-soft)] bg-transparent p-1.5 text-[var(--hv-text-2)] cursor-pointer hover:bg-[var(--hv-subtle-hover)] disabled:opacity-50 disabled:cursor-progress"
                aria-label={$t('explorer.transfer_cancel', { name: row.name || row.id })}
                data-testid="transfer-cancel"
                disabled={cancelling.includes(row.id)}
                onclick={() => cancel(row.kind === 'upload' ? 'upload' : 'job', row.id, row.cancelPath)}
              >
                <IconX size={15} aria-hidden="true" />
              </button>
            {/if}
          </div>
          {#if row.state === 'failed' || row.state === 'cancelled'}
            <p class="m-0 mt-1 text-c-xs text-[var(--hv-danger)]">
              {$t(row.state === 'failed' ? 'explorer.transfer_state_failed' : 'explorer.transfer_state_cancelled')}
            </p>
          {:else}
            {@const pct = progressPercent(row.progress)}
            <div
              class="mt-1.5 h-1.5 w-full overflow-hidden rounded-full bg-[var(--hv-subtle)]"
              role="progressbar"
              aria-valuemin="0"
              aria-valuemax="100"
              aria-valuenow={pct === null ? undefined : Math.round(pct)}
              aria-label={$t('explorer.transfer_progress', { name: row.name || row.id })}
            >
              <div
                class="h-full rounded-full bg-[var(--hv-primary)] transition-[width] duration-300"
                style:width={pct === null ? '35%' : `${pct}%`}
                class:opacity-60={pct === null}
              ></div>
            </div>
            <p class="m-0 mt-1 truncate text-c-xs text-[var(--hv-text-2)]" data-testid="transfer-detail">
              {transferDetailLine(row, {
                language: $languageStore,
                etaLabel: $t('explorer.transfer_eta_left'),
                fallback: row.detail,
              })}
            </p>
            {#if row.where}
              <p class="m-0 mt-0.5 truncate text-c-xs text-[var(--hv-text-3)]">{row.where}</p>
            {/if}
          {/if}
        </li>
      {/each}
    </ul>
  {/if}

  <details class="rounded-lg border border-[var(--hv-border-soft)] p-2.5" data-testid="transfer-history" ontoggle={(e) => { if ((e.currentTarget as HTMLDetailsElement).open) loadHistory(); }}>
    <summary class="cursor-pointer text-c-sm font-medium text-[var(--hv-text)]">
      {$t('explorer.transfer_history_title')}
    </summary>
    <p class="m-0 mt-1 text-c-xs text-[var(--hv-text-2)]">{$t('explorer.transfer_history_desc')}</p>
    {#if historyLoaded && history.length === 0}
      <p class="m-0 mt-2 text-c-sm text-[var(--hv-text-2)]" data-testid="transfer-history-empty">
        {$t('explorer.transfer_history_empty')}
      </p>
    {:else}
      <ul class="m-0 mt-2 flex list-none flex-col gap-1 p-0">
        {#each history as entry (entry.id)}
          <li
            class="flex flex-wrap items-baseline gap-x-2 gap-y-0.5 rounded-md border border-[var(--hv-border-soft)] px-2 py-1 text-c-xs"
            data-testid="transfer-history-row"
          >
            <span class="font-medium text-[var(--hv-text)]">
              {$t(`explorer.transfer_kind_${entry.operation === 'upload'
                ? 'upload'
                : entry.operation === 'download'
                  ? 'download'
                  : entry.operation === 'checksum'
                    ? 'checksum'
                    : 'copy'}`)}
            </span>
            <span class="min-w-0 flex-1 truncate text-[var(--hv-text-2)]">{entry.dest_path ?? entry.source_path ?? ''}</span>
            <span class="text-[var(--hv-text-3)]">{formatBytes(entry.bytes_total)}</span>
            {#if entry.method}
              <span class="rounded bg-[var(--hv-subtle)] px-1 text-[var(--hv-text-2)]">{entry.method}</span>
            {/if}
            {#if entry.verified}
              <span class="text-[var(--hv-success)]">{$t('explorer.transfer_history_verified')}</span>
            {/if}
            {#if entry.status !== 'complete'}
              <span class="text-[var(--hv-danger)]">
                {$t(entry.status === 'failed' ? 'explorer.transfer_state_failed' : 'explorer.transfer_state_cancelled')}
              </span>
            {/if}
          </li>
        {/each}
      </ul>
    {/if}
  </details>

  <div class="flex justify-end">
    <button
      type="button"
      class="min-h-11 rounded-lg border border-[var(--hv-border-soft)] bg-transparent px-3 text-c-sm text-[var(--hv-text)] cursor-pointer hover:bg-[var(--hv-subtle-hover)]"
      onclick={onclose}
      data-testid="transfer-close"
    >{$t('explorer.transfer_close')}</button>
  </div>
</div>

<script lang="ts">
  import { onMount } from 'svelte';
  import { api } from '$lib/services/api';
  import { t, translate } from '$lib/stores/i18n';
  import { Modal, Button } from '$lib/components/theme';
  import { PageLayout } from '$lib/components/app';
  import { cn } from '$lib/utils.js';
  import { IconNotes, IconSearch, IconFilter, IconTrash, IconAlertTriangle, IconSettings } from '@tabler/icons-svelte';

  interface JournalSource {
    id: string;
    label: string;
  }

  interface JournalEntry {
    timestamp?: string | null;
    unit?: string | null;
    priority?: number | null;
    priority_text?: string | null;
    pid?: string | null;
    message: string;
  }

  let sources: JournalSource[] = [];
  let entries: JournalEntry[] = [];
  let loading = true;
  let loadingSources = true;
  let selectedSource = 'app';
  let unitFilter = '';
  let priorityFilter = 6; // info and above
  let searchFilter = '';
  let autoRefresh = false;
  let autoRefreshInterval: ReturnType<typeof setInterval> | null = null;
  let showClearConfirm = false;
  let clearing = false;

  let showCleanupSettings = false;
  let cleanupEnabled = false;
  let cleanupIntervalDays = 30;
  let savingCleanupSettings = false;

  const priorityOptions: Array<{ value: number; key: string }> = [
    { value: 0, key: 'journal.prio_emerg' },
    { value: 1, key: 'journal.prio_alert' },
    { value: 2, key: 'journal.prio_crit' },
    { value: 3, key: 'journal.prio_err' },
    { value: 4, key: 'journal.prio_warning' },
    { value: 5, key: 'journal.prio_notice' },
    { value: 6, key: 'journal.prio_info' },
    { value: 7, key: 'journal.prio_debug' },
  ];

  // Priority → left border colour for an entry row (appliance tokens).
  const PRIO_BORDER: Record<string, string> = {
    'jr-crit': 'border-l-[var(--hv-error)]',
    'jr-err': 'border-l-[var(--hv-error)]',
    'jr-warn': 'border-l-[var(--hv-warning)]',
    'jr-notice': 'border-l-[var(--hv-info)]',
    'jr-info': 'border-l-[var(--hv-shell-border)]',
  };

  function priorityClass(entry: JournalEntry): string {
    const p = entry.priority;
    if (p === undefined || p === null) {
      // For text logs without a priority, guess from keywords.
      const m = entry.message.toLowerCase();
      if (m.includes('error') || m.includes('err') || m.includes('failed')) return 'jr-err';
      if (m.includes('warn')) return 'jr-warn';
      return 'jr-info';
    }
    if (p <= 2) return 'jr-crit';
    if (p === 3) return 'jr-err';
    if (p === 4) return 'jr-warn';
    if (p === 5) return 'jr-notice';
    return 'jr-info';
  }

  // Translated display label for a log source, falling back to the
  // backend's static English label if no translation key exists yet for
  // that source id (same fallback pattern as Docker's catalog descriptions
  // and Widgets' catalog names).
  function sourceLabel(source: JournalSource): string {
    const key = `journal.source_${source.id}`;
    const translated = translate(key);
    return translated === key ? source.label : translated;
  }

  function formatTimestamp(ts?: string | null): string {
    if (!ts) return '';
    const d = new Date(ts);
    if (isNaN(d.getTime())) return ts;
    return d.toLocaleString();
  }

  // Shared control shape for this screen's native inputs/selects.
  const fieldInput =
    'bg-[var(--hv-shell-border-soft)] border border-[var(--hv-shell-border)] text-[var(--hv-shell-text-strong)] ' +
    'rounded-[var(--hv-radius-default)] px-2.5 py-1.5 text-c-base outline-none transition-colors ' +
    'focus:border-[var(--hv-primary)] min-w-[160px]';

  onMount(() => {
    (async () => {
      await loadSources();
      await loadEntries();
      await loadCleanupSettings();
    })();
    return () => {
      if (autoRefreshInterval) clearInterval(autoRefreshInterval);
    };
  });

  async function loadSources() {
    loadingSources = true;
    const res = await api.getJournalSources();
    if (res.success && res.data) {
      sources = res.data;
    } else {
    }
    loadingSources = false;
  }

  async function loadEntries() {
    loading = true;
    const res = await api.getJournalEntries({
      source: selectedSource,
      unit: selectedSource === 'system' ? unitFilter || undefined : undefined,
      priority: selectedSource === 'system' ? priorityFilter : undefined,
      limit: 200,
      search: searchFilter || undefined,
    });
    if (res.success && res.data) {
      entries = res.data;
    } else {
    }
    loading = false;
  }

  function applyFilters() {
    loadEntries();
  }

  function toggleAutoRefresh() {
    autoRefresh = !autoRefresh;
    if (autoRefresh) {
      autoRefreshInterval = setInterval(() => { loadEntries(); }, 5000);
    } else if (autoRefreshInterval) {
      clearInterval(autoRefreshInterval);
      autoRefreshInterval = null;
    }
  }

  async function confirmClearLogs() {
    clearing = true;
    const res = await api.clearJournalLogs();
    clearing = false;
    showClearConfirm = false;
    if (res.success) {
      if (selectedSource === 'app') {
        await loadEntries();
      }
    } else {
    }
  }

  async function loadCleanupSettings() {
    const res = await api.getJournalCleanupSettings();
    if (res.success && res.data) {
      cleanupEnabled = !!res.data.auto_cleanup_enabled;
      cleanupIntervalDays = res.data.interval_days || 30;
    }
  }

  async function saveCleanupSettings() {
    savingCleanupSettings = true;
    const days = Math.max(1, Math.floor(cleanupIntervalDays) || 1);
    const res = await api.updateJournalCleanupSettings({
      auto_cleanup_enabled: cleanupEnabled,
      interval_days: days,
    });
    savingCleanupSettings = false;
    if (res.success) {
      cleanupIntervalDays = days;
      showCleanupSettings = false;
    } else {
    }
  }

  async function copyEntry(entry: JournalEntry) {
    const line = [
      formatTimestamp(entry.timestamp),
      entry.unit || '',
      entry.priority_text || '',
      entry.pid || '',
      entry.message,
    ].filter(Boolean).join(' ');
    try {
      await navigator.clipboard.writeText(line);
    } catch {
    }
  }
</script>

<PageLayout
  title={$t('journal.title')}
  data-testid="journal-page"
>
  {#snippet icon()}
    <IconNotes stroke={1.8} />
  {/snippet}

  <!-- Page-level actions: a WindowToolbar row under the header, never inside
       it (the header bar is icon + title + window controls only). -->
  {#snippet toolbarRight()}
    <Button variant="danger" onclick={() => (showClearConfirm = true)} data-testid="journal-clear">
      <IconTrash size="14" /> {$t('journal.clear')}
    </Button>
    <Button variant="ghost" onclick={() => (showCleanupSettings = true)} data-testid="journal-settings">
      <IconSettings size="14" /> {$t('journal.cleanup_settings')}
    </Button>
  {/snippet}

  {#snippet toolbarLeft()}
    <div class="flex flex-wrap items-end gap-3">
      <div class="flex flex-col gap-1 text-c-sm text-muted-foreground">
        <label for="jr-source" class="font-semibold uppercase tracking-wide">{$t('journal.source')}</label>
        <select id="jr-source" class={fieldInput} bind:value={selectedSource} onchange={applyFilters} disabled={loadingSources}>
          {#each sources as s}
            <option value={s.id}>{sourceLabel(s)}</option>
          {/each}
        </select>
      </div>

      {#if selectedSource === 'system'}
        <div class="flex flex-col gap-1 text-c-sm text-muted-foreground">
          <label for="jr-unit" class="font-semibold uppercase tracking-wide">{$t('journal.unit')}</label>
          <input id="jr-unit" type="text" class={fieldInput} placeholder={$t('journal.unit_placeholder')} bind:value={unitFilter} onchange={applyFilters} />
        </div>

        <div class="flex flex-col gap-1 text-c-sm text-muted-foreground">
          <label for="jr-prio" class="font-semibold uppercase tracking-wide">{$t('journal.priority')}</label>
          <select id="jr-prio" class={fieldInput} bind:value={priorityFilter} onchange={applyFilters}>
            {#each priorityOptions as opt}
              <option value={opt.value}>{$t(opt.key)}</option>
            {/each}
          </select>
        </div>
      {/if}

      <div class="flex flex-col gap-1 text-c-sm text-muted-foreground min-w-[200px] flex-1">
        <label for="jr-search" class="font-semibold uppercase tracking-wide"><IconSearch size="14" /></label>
        <input id="jr-search" type="text" class={cn(fieldInput, 'w-full')} placeholder={$t('journal.search_placeholder')} bind:value={searchFilter} onkeydown={(e) => e.key === 'Enter' && applyFilters()} />
      </div>

      <!-- Auto-refresh belongs with the filters it re-runs. -->
      <label data-testid="journal-autorefresh" class="flex items-center gap-1.5 pb-2 text-c-base text-foreground cursor-pointer">
        <input type="checkbox" class="accent-[var(--hv-primary)]" checked={autoRefresh} onchange={toggleAutoRefresh} />
        {$t('journal.auto_refresh')}
      </label>

      <Button variant="primary" onclick={applyFilters} data-testid="journal-apply">
        <IconFilter size="14" /> {$t('journal.apply')}
      </Button>
    </div>
  {/snippet}

  {#if loading}
    <div data-testid="journal-loading" class="flex flex-col" role="status">
      <span class="sr-only">{$t('journal.loading')}</span>
      {#each Array(6) as _, i}
        <div class="flex flex-col gap-1.5 border-b border-[var(--hv-shell-border-soft)] px-2.5 py-2" aria-hidden="true">
          <span class="hv-skeleton-text block" style="width: {35 + ((i * 31) % 40)}%"></span>
          <span class="hv-skeleton-text block" style="width: {55 + ((i * 17) % 30)}%"></span>
        </div>
      {/each}
    </div>
  {:else if entries.length === 0}
    <div data-testid="journal-empty" class="py-8 text-center text-muted-foreground">{$t('journal.empty')}</div>
  {:else}
    <div data-testid="journal-list" class="flex flex-col">
      {#each entries as entry, i (i + '-' + entry.timestamp + '-' + entry.message.slice(0, 20) + '-' + entry.pid)}
        <div
          data-testid="journal-entry"
          class={cn(
            'flex flex-col gap-0.5 px-2.5 py-1.5 border-l-[3px] border-b border-[var(--hv-shell-border-soft)] cursor-pointer transition-colors hover:bg-[var(--hv-subtle-hover)] overflow-x-auto',
            PRIO_BORDER[priorityClass(entry)],
          )}
          role="button"
          tabindex="0"
          ondblclick={() => copyEntry(entry)}
          onkeydown={(e) => { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); copyEntry(entry); } }}
        >
          <div class="flex flex-wrap gap-2 text-c-xs text-muted-foreground">
            {#if entry.timestamp}<span>{formatTimestamp(entry.timestamp)}</span>{/if}
            {#if entry.unit}<span>{entry.unit}</span>{/if}
            {#if entry.priority_text}<span class="uppercase font-bold">{entry.priority_text}</span>{/if}
            {#if entry.pid}<span>pid {entry.pid}</span>{/if}
          </div>
          <div class="whitespace-pre-wrap break-words text-foreground font-[family-name:var(--hv-font-mono)] text-c-base">{entry.message}</div>
        </div>
      {/each}
    </div>
  {/if}

  {#snippet footer()}
    <span data-testid="journal-foot" class="text-c-sm text-muted-foreground">
      {entries.length} {$t('journal.entries_shown')}
    </span>
  {/snippet}
</PageLayout>

<Modal open={showClearConfirm} title={$t('journal.clear_confirm_title')} size="sm" onclose={() => (showClearConfirm = false)}>
  {#snippet icon()}
    <IconAlertTriangle size="28" stroke={2} />
  {/snippet}
  {#snippet children()}
    <p class="m-0 text-center text-[var(--hv-text-secondary)] text-c-md leading-relaxed">{$t('journal.clear_confirm_message')}</p>
  {/snippet}
  {#snippet footer()}
    <Button variant="secondary" onclick={() => (showClearConfirm = false)} disabled={clearing}>
      {$t('journal.clear_cancel')}
    </Button>
    <Button variant="danger" onclick={confirmClearLogs} loading={clearing}>
      <IconTrash size="16" /> {$t('journal.clear_confirm_button')}
    </Button>
  {/snippet}
</Modal>

<Modal open={showCleanupSettings} title={$t('journal.cleanup_settings')} size="sm" onclose={() => (showCleanupSettings = false)}>
  {#snippet icon()}
    <IconSettings size="24" stroke={2} />
  {/snippet}
  {#snippet children()}
    <div class="flex flex-col gap-3">
      <label data-testid="journal-cleanup-toggle" class="flex items-center gap-2 text-c-md text-foreground cursor-pointer">
        <input type="checkbox" class="accent-[var(--hv-primary)]" bind:checked={cleanupEnabled} />
        {$t('journal.cleanup_enable')}
      </label>
      <p class="m-0 text-c-base text-[var(--hv-text-secondary)] leading-relaxed">{$t('journal.cleanup_hint')}</p>
      <div class="flex items-center gap-2 text-c-md text-foreground">
        <label for="jr-cleanup-days">{$t('journal.cleanup_interval')}</label>
        <input
          id="jr-cleanup-days"
          type="number"
          min="1"
          step="1"
          class="w-[70px] bg-[var(--hv-shell-border-soft)] border border-[var(--hv-shell-border)] text-foreground rounded-[var(--hv-radius-default)] px-2 py-1 text-c-base disabled:opacity-50"
          bind:value={cleanupIntervalDays}
          disabled={!cleanupEnabled}
        />
        <span>{$t('journal.cleanup_days_suffix')}</span>
      </div>
    </div>
  {/snippet}
  {#snippet footer()}
    <Button variant="secondary" onclick={() => (showCleanupSettings = false)} disabled={savingCleanupSettings}>
      {$t('journal.clear_cancel')}
    </Button>
    <Button variant="primary" onclick={saveCleanupSettings} loading={savingCleanupSettings}>
      {$t('journal.cleanup_save')}
    </Button>
  {/snippet}
</Modal>
<!-- ========================================================================
     File          : AppUsageBreakdown.svelte
     Author        : Nicolas Le Roux
     Created       : 13/10/2026
     Last modified : 13/10/2026 by Nicolas Le Roux
     Version       : 1.0.0

     Description:
       Dashboard "Live usage by app" card: polls per-container CPU/memory/IO
       stats alongside host totals, keeps a short rolling history, and draws
       the stacked band chart plus a colour-chipped app list (deterministic
       per-app colours from appColor()).

     Dependencies:
       - svelte (>= 5), charts/AreaChart, utils/appColor

     Usage:
       A featured desktop widget on the Dashboard.
     ======================================================================== -->
<script lang="ts">
  /**
   * Dashboard "Live usage by app" card.
   *
   * Polls `/docker/containers?stats=true` (per-container CPU %, memory, IO)
   * alongside the host CPU/memory totals, keeps a short rolling history in
   * the browser, and renders:
   *
   *   - two stacked area charts (CPU % and memory) whose bands are the apps,
   *     so the composition of the host total is visible at a glance;
   *   - a per-app list with a stable colour chip, live CPU/memory, and direct
   *     actions (restart / stop / uninstall) that act on the container
   *     without leaving the Dashboard.
   *
   * Two invariants worth keeping:
   *
   * 1. **Stable per-app colour.** Each container gets a deterministic colour
   *    from `appColor(name)` (a hash → hue), never a per-render index. A
   *    colour that reshuffled every poll would make the stacked bands
   *    unreadable and the list chips meaningless. This is the offline
   *    equivalent of extracting a colour from the app icon: deterministic,
   *    no Canvas/network dependency, and identical across reloads.
   *
   * 2. **The list freezes while a menu is open.** Polling is paused whenever
   *    an action menu is open (and during an in-flight action), so rows never
   *    reorder or disappear under the user's cursor mid-click. The same
   *    reason the Docker app freezes its list during a menu.
   *
   * 3. **Admin-only, and only while visible.** `/docker/containers?stats=true`
   *    is `require_admin`, so a non-admin viewer used to issue a failing
   *    request every 5 s; the card now shows an admin-only notice and polls
   *    nothing. A hidden tab skips the tick entirely (ROADMAP 15.2), and the
   *    next visible tick refreshes immediately.
   */
  import { onMount, onDestroy } from 'svelte';
  import { api } from '$lib/services/api';
  import { t, translate } from '$lib/stores/i18n';
  import { isAdmin } from '$lib/stores/auth';
  import { SectionCard } from '$lib/components/app';
  import { AreaChart, type AreaSeries } from '$lib/components/charts';
  import { Spinner } from '$lib/components/theme';
  import { createConfirmState } from '$lib/utils/confirmDialog.svelte';
  import { ConfirmDialog } from '$lib/components/app';
  import { appColor } from '$lib/utils/appColor';
  import { cn } from '$lib/utils.js';
  import {
    IconPlayerStop,
    IconRefresh,
    IconTrash,
    IconDots,
    IconContainer
  } from '@tabler/icons-svelte';

  interface Container {
    id: string;
    name: string;
    image: string;
    state: string;
    cpu_percent: number | null;
    memory_mb: number | null;
    net_rx_bytes?: number | null;
    net_tx_bytes?: number | null;
  }

  /** How many samples the rolling charts keep (~2 min at 5 s). */
  const HISTORY = 24;

  let containers: Container[] = [];
  let refreshing = false;
  let hostCpuPct = 0;
  let hostMemPct = 0;
  let loading = true;
  let error = '';
  let updatedAt = '';
  /** Failure of the last restart/stop/uninstall, shown in the card. */
  let actionError = '';

  /** Rolling history, one object per sample. Keys are `cpu:<name>` /
   *  `mem:<name>` so the same rows can feed both stacked charts. */
  let history: Record<string, any>[] = [];

  /** The container whose action menu is open (freezes polling). */
  let menuFor: string | null = null;
  /** A container with an in-flight action (row shows a spinner). */
  let busyFor: string | null = null;

  const dlg = createConfirmState();

  /** The running containers, newest-name-stable order (sorted by name so the
   *  list never reorders between polls). */
  $: running = containers
    .filter((c) => c.state === 'Running' || c.state === 'running')
    .slice()
    .sort((a, b) => a.name.localeCompare(b.name));

  $: cpuSeries = running.map((c) => ({ key: `cpu:${c.name}`, label: c.name, color: appColor(c.name) }));
  $: memSeries = running.map((c) => ({ key: `mem:${c.name}`, label: c.name, color: appColor(c.name) }));

  /** The host CPU/memory percentages, used as the chart ceiling so the bands
   *  are drawn against the real total rather than the sampled sum (a
   *  container's CPU % can exceed the host % on multi-core, so the host value
   *  is the honest scale reference). */
  $: cpuMax = Math.max(100, ...history.map((h) => Number(h.hostCpu) || 0));
  $: memMax = 100;

  async function refresh() {
    // Freeze while a menu is open or an action is in flight — see the file
    // header. The next tick after the menu closes resumes normally.
    if (menuFor !== null || busyFor !== null) return;
    // The container endpoint is admin-only: polling it as a regular user is
    // a guaranteed 403 every 5 s. Skip the tick entirely.
    if (!$isAdmin || document.hidden || refreshing) return;
    refreshing = true;
    try {
      const [containersRes, detailsRes] = await Promise.all([
        api.getDockerContainersWithStats(),
        api.getServerDetails(),
      ]);
      if (containersRes.success) containers = (containersRes.data as Container[]) ?? [];
      if (detailsRes.success) {
        hostCpuPct = detailsRes.data?.cpu?.usage_percent ?? detailsRes.data?.cpu_usage ?? 0;
        hostMemPct = detailsRes.data?.memory?.usage_percent ?? 0;
      }
      error = '';
      pushHistory();
      updatedAt = new Date().toLocaleTimeString();
    } catch (e) {
      error = translate('dashboard.usage_load_failed');
      console.error('AppUsageBreakdown error:', e);
    } finally {
      loading = false;
      refreshing = false;
    }
  }

  function pushHistory() {
    const row: Record<string, any> = { label: updatedAt, hostCpu: hostCpuPct, hostMem: hostMemPct };
    for (const c of running) {
      row[`cpu:${c.name}`] = c.cpu_percent ?? 0;
      row[`mem:${c.name}`] = c.memory_mb ?? 0;
    }
    history = [...history, row].slice(-HISTORY);
  }

  async function act(name: string, action: 'restart' | 'stop' | 'uninstall') {
    menuFor = null;
    if (action === 'uninstall') {
      const ok = await dlg.ask(translate('dashboard.usage_uninstall_confirm', { name }));
      if (!ok) return;
    }
    busyFor = name;
    actionError = '';
    try {
      if (action === 'restart') await api.restartContainer(name);
      else if (action === 'stop') await api.stopContainer(name);
      else await api.removeDockerContainer(name, true);
      await refresh();
    } catch (e) {
      // A failed action must say so on screen, not only in the console.
      actionError = translate('dashboard.usage_action_failed', { name });
      console.error(`Container ${action} failed:`, e);
    } finally {
      busyFor = null;
    }
  }

  function toggleMenu(name: string) {
    menuFor = menuFor === name ? null : name;
  }

  function onVisibilityChange() {
    if (!document.hidden) refresh();
  }

  onMount(() => {
    refresh();
    const timer = setInterval(refresh, 5000);
    document.addEventListener('visibilitychange', onVisibilityChange);
    return () => {
      clearInterval(timer);
      document.removeEventListener('visibilitychange', onVisibilityChange);
    };
  });

  function fmtMem(mb: number | null): string {
    if (!mb) return '—';
    if (mb >= 1024) return (mb / 1024).toFixed(1) + ' Go';
    return mb.toFixed(0) + ' Mo';
  }
</script>

<SectionCard data-testid="dashboard-usage-card" class="col-span-12 [&>.hv-section-card-body]:flex [&>.hv-section-card-body]:flex-col">
  {#snippet titleSnippet()}
    <div class="flex items-center gap-3">
      <span class="grid place-items-center w-[52px] h-[52px] rounded-2xl flex-none bg-[var(--hv-primary-light)] text-[var(--hv-primary)]">
        <IconContainer size={24} stroke={1.7} />
      </span>
      <div class="flex-1 min-w-0">
        <div class="text-c-xl font-semibold text-[var(--hv-shell-text-strong)]">{$t('dashboard.usage_title')}</div>
        <div class="text-c-base text-[var(--hv-shell-text-2)] mt-0.5">
          {running.length} {$t('dashboard.usage_apps')} · {$t('dashboard.usage_updated')} {updatedAt || '—'}
        </div>
      </div>
    </div>
  {/snippet}
  {#snippet actions()}
    <button
      type="button"
      class="hv-ic"
      aria-label={$t('common.refresh')}
      onclick={() => refresh()}
    >
      <IconRefresh size={18} stroke={1.8} />
    </button>
  {/snippet}

  {#if !$isAdmin}
    <div data-testid="dashboard-usage-admin-only" class="py-10 text-center text-c-base text-[var(--hv-shell-text-2)]">
      {$t('dashboard.usage_admin_only')}
    </div>
  {:else if loading && containers.length === 0}
    <div class="flex items-center justify-center gap-3 py-12 text-[var(--hv-shell-text-3)]">
      <Spinner />
      <span>{$t('dashboard.usage_loading')}</span>
    </div>
  {:else if running.length === 0}
    <div data-testid="dashboard-usage-empty" class="py-10 text-center text-c-base text-[var(--hv-shell-text-2)]">
      {$t('dashboard.usage_empty')}
    </div>
  {:else}
    {#if error || actionError}
      <div role="alert" class="mb-4 px-4 py-2.5 rounded-md bg-[var(--hv-error-bg)] border border-[var(--hv-error)]/30 text-[var(--hv-error-text)] text-sm">{error || actionError}</div>
    {/if}

    <!-- Composition charts: one band per app, stacked to the host total. -->
    <div class="grid grid-cols-2 gap-5 mt-2 @max-[768px]:grid-cols-1">
      <div>
        <div class="flex items-baseline justify-between text-sm text-[var(--hv-shell-text-2)] mb-2">
          <span>{$t('dashboard.usage_cpu')}</span>
          <span class="font-[family-name:var(--hv-font-mono)] text-[var(--hv-shell-text-strong)]">{hostCpuPct.toFixed(0)} %</span>
        </div>
        <AreaChart
          data={history}
          series={cpuSeries as AreaSeries[]}
          max={cpuMax}
          height={110}
          ariaLabel={$t('dashboard.usage_cpu')}
        />
      </div>
      <div>
        <div class="flex items-baseline justify-between text-sm text-[var(--hv-shell-text-2)] mb-2">
          <span>{$t('dashboard.usage_memory')}</span>
          <span class="font-[family-name:var(--hv-font-mono)] text-[var(--hv-shell-text-strong)]">{hostMemPct.toFixed(0)} %</span>
        </div>
        <AreaChart
          data={history}
          series={memSeries as AreaSeries[]}
          max={memMax}
          height={110}
          ariaLabel={$t('dashboard.usage_memory')}
        />
      </div>
    </div>

    <!-- Per-app list with direct actions. -->
    <div data-testid="dashboard-usage-list" class="flex flex-col gap-1 mt-5 pt-4 border-t border-[var(--hv-shell-border-soft)]">
      {#each running as c (c.id)}
        <div
          data-testid="dashboard-usage-row"
          class={cn(
            'relative flex items-center gap-3 min-h-[52px] px-2 rounded-lg transition-colors',
            menuFor === c.name ? 'bg-[var(--hv-shell-fill)]' : 'hover:bg-[var(--hv-shell-fill)]'
          )}
        >
          <span
            class="w-3.5 h-3.5 rounded-full flex-none"
            style="background: {appColor(c.name)}"
            aria-hidden="true"
          ></span>
          <div class="flex-1 min-w-0">
            <div class="text-c-base font-semibold text-[var(--hv-shell-text-strong)] truncate">{c.name}</div>
            <div class="text-xs text-[var(--hv-shell-text-2)] truncate">{c.image}</div>
          </div>
          <div class="hidden @min-[560px]:flex items-center gap-5 font-[family-name:var(--hv-font-mono)] text-sm text-[var(--hv-shell-text-2)]">
            <span title={$t('dashboard.usage_cpu')}>{$t('dashboard.cpu')} {(c.cpu_percent ?? 0).toFixed(1)} %</span>
            <span title={$t('dashboard.usage_memory')}>{$t('dashboard.ram')} {fmtMem(c.memory_mb)}</span>
          </div>

          {#if busyFor === c.name}
            <span class="w-9 h-9 grid place-items-center"><Spinner /></span>
          {:else}
            <button
              type="button"
              class="hv-ic"
              aria-label={$t('dashboard.usage_actions_for', { name: c.name })}
              aria-haspopup="menu"
              aria-expanded={menuFor === c.name}
              data-testid="dashboard-usage-menu-btn"
              onclick={() => toggleMenu(c.name)}
            >
              <IconDots size={18} stroke={1.8} />
            </button>
          {/if}

          {#if menuFor === c.name}
            <!-- The menu is rendered inside the frozen row; the list cannot
                 reorder while it is open (see the file header). -->
            <div
              role="menu"
              data-testid="dashboard-usage-menu"
              class="absolute right-2 top-[calc(100%-4px)] z-20 min-w-[190px] py-1 rounded-lg border border-[var(--hv-shell-border)] bg-[var(--hv-shell-card)] shadow-[var(--hv-shell-shadow-card)]"
            >
              <button type="button" role="menuitem" class="hv-usage-menu-item" onclick={() => act(c.name, 'restart')}>
                <IconRefresh size={16} stroke={1.8} /> {$t('dashboard.usage_restart')}
              </button>
              <button type="button" role="menuitem" class="hv-usage-menu-item" onclick={() => act(c.name, 'stop')}>
                <IconPlayerStop size={16} stroke={1.8} /> {$t('dashboard.usage_stop')}
              </button>
              <button type="button" role="menuitem" class="hv-usage-menu-item hv-usage-menu-danger" onclick={() => act(c.name, 'uninstall')}>
                <IconTrash size={16} stroke={1.8} /> {$t('dashboard.usage_uninstall')}
              </button>
            </div>
          {/if}
        </div>
      {/each}
    </div>
  {/if}
</SectionCard>

<ConfirmDialog
  {dlg}
  onconfirm={() => dlg.settle(true)}
  oncancel={() => dlg.settle(false)}
/>

<style>
  :global(.hv-usage-menu-item) {
    display: flex;
    align-items: center;
    gap: 10px;
    width: 100%;
    min-height: 40px;
    padding: 0 12px;
    background: none;
    border: 0;
    text-align: left;
    font-size: var(--hv-text-sm);
    color: var(--hv-shell-text-strong);
    cursor: pointer;
  }
  :global(.hv-usage-menu-item:hover),
  :global(.hv-usage-menu-item:focus-visible) {
    background: var(--hv-shell-fill);
    outline: none;
    box-shadow: var(--hv-focus-ring);
  }
  :global(.hv-usage-menu-danger) {
    color: var(--hv-error-text);
  }
</style>

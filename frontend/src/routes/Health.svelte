<!-- ========================================================================
     File          : Health.svelte
     Author        : Nicolas Le Roux
     Created       : 28/09/2026
     Last modified : 28/09/2026 by Nicolas Le Roux
     Version       : 1.0.0

     Description:
     The Health app: system health check results (disks, services,
     resources) with per-check detail and re-run.

     Dependencies:
       - svelte (>= 5), theme + app/ composites, $lib/services/api

     Usage:
     Opened as a desktop window app.
     ======================================================================== -->
<script lang="ts">
  import { onMount } from 'svelte';
  import { api } from '$lib/services/api';
  import { t, translate } from '$lib/stores/i18n';
  import { onAppVisible } from '$lib/utils/autoRevalidate';
  import { Spinner } from '$lib/components/theme';
  import { PageLayout, EmptyState } from '$lib/components/app';
  import { IconHeart, IconActivity, IconHeartbeat } from '@tabler/icons-svelte';

  // Matches `HealthCheckResult` in src/health.rs, as actually returned by
  // GET /api/health (`data.checks`, each `{name, status, message, details}`).
  type CheckStatus = 'healthy' | 'warning' | 'unhealthy' | 'unknown';

  interface HealthCheck {
    name: string;
    status: CheckStatus;
    message: string;
    details?: Record<string, unknown> | null;
  }

  let checks: HealthCheck[] = [];
  let loading = true;
  let overallStatus: CheckStatus = 'unknown';

  // The 8 checks health.rs always runs, in a fixed, sensible display order —
  // `checks` from the API is already in this order, but this also gives each
  // one a translated display name (the raw `name` is a snake_case identifier
  // like "disk_space", not something to show directly).
  const CHECK_LABEL_KEYS: Record<string, string> = {
    authentication: 'health.check_authentication',
    storage: 'health.check_storage',
    memory: 'health.check_memory',
    disk_space: 'health.check_disk_space',
    disk_smart: 'health.check_disk_smart',
    network: 'health.check_network',
    services: 'health.check_services',
    temperatures: 'health.check_temperatures',
  };

  function checkLabel(name: string): string {
    const key = CHECK_LABEL_KEYS[name];
    return key ? translate(key) : name;
  }

  onMount(() => {
    loadHealth();
    // No manual refresh button — revalidate on focus + a slow poll (the
    // checks reflect live system state).
    return onAppVisible(loadHealth, { pollMs: 45_000 });
  });

  async function loadHealth() {
    loading = true;
    const response = await api.healthCheck();
    if (response.success && response.data) {
      checks = response.data.checks || [];
      overallStatus = response.data.overall_status || 'unknown';
    } else {
    }
    loading = false;
  }

  function getStatusColor(status: string): string {
    switch (status) {
      case 'healthy': return 'var(--hv-success)';
      case 'warning': return 'var(--hv-warning)';
      case 'unhealthy': return 'var(--hv-error)';
      default: return 'var(--hv-text-tertiary)';
    }
  }

  function getStatusLabel(status: string): string {
    switch (status) {
      case 'healthy': return $t('health.healthy');
      case 'warning': return $t('health.warning');
      case 'unhealthy': return $t('health.critical');
      default: return $t('health.unknown');
    }
  }
</script>

<PageLayout
  title={$t('health.title')}
  data-testid="health-page"
>
  {#snippet icon()}
    <IconHeart stroke={1.8} />
  {/snippet}

  {#if loading}
    <div data-testid="health-loading" class="flex flex-col items-center gap-3 py-20 text-muted-foreground">
      <Spinner />
      <span>{$t('health.loading')}</span>
    </div>
  {:else if checks.length === 0}
    <EmptyState
      title={$t('health.no_checks_title')}
      description={$t('health.no_checks_desc')}
      data-testid="health-empty"
    >
      {#snippet iconSnippet()}
        <IconHeartbeat size={26} stroke={1.8} />
      {/snippet}
    </EmptyState>
  {:else}
    <!-- Overall status strip. This used to be a badge in the window header;
         it moved here when the header was standardized to icon + title only.
         It is a real reading, not decoration, so it belongs to the page body. -->
    <div
      data-testid="health-overall"
      class="flex items-center gap-2.5 mb-5 px-4 py-3 rounded-xl border border-border bg-card shadow-[var(--hv-shell-shadow-card)]"
    >
      <span
        class="h-2.5 w-2.5 rounded-full flex-none"
        style="background: {getStatusColor(overallStatus)}"
      ></span>
      <span class="text-c-base font-semibold text-foreground">
        {$t('health.overall_status')}
      </span>
      <span
        data-testid="health-badge"
        class="inline-flex items-center px-2.5 py-0.5 rounded-full text-c-base font-semibold"
        style="background: {getStatusColor(overallStatus)}15; color: {getStatusColor(overallStatus)}"
      >
        {getStatusLabel(overallStatus)}
      </span>
    </div>

    <div
      data-testid="health-grid"
      class="grid gap-5 [grid-template-columns:repeat(auto-fill,minmax(min(320px,100%),1fr))]"
    >
      {#each checks as check}
        <article
          data-testid="health-card"
          class="rounded-xl bg-card border border-border shadow-[var(--hv-shell-shadow-card)] p-5 transition-shadow hover:shadow-md"
        >
          <div class="flex items-center gap-3 mb-4">
            <span
              class="grid place-items-center w-11 h-11 rounded-xl flex-none"
              style="background: {getStatusColor(check.status)}15; color: {getStatusColor(check.status)}"
            >
              <IconActivity size={20} />
            </span>
            <div class="min-w-0">
              <div data-testid="health-card-name" class="text-c-md font-semibold text-foreground truncate">
                {checkLabel(check.name)}
              </div>
              <span data-testid="health-card-status" class="text-c-sm font-medium" style="color: {getStatusColor(check.status)}">
                {getStatusLabel(check.status)}
              </span>
            </div>
          </div>
          <div data-testid="health-card-details" class="text-c-base text-muted-foreground">{check.message}</div>
        </article>
      {/each}
    </div>
  {/if}
</PageLayout>
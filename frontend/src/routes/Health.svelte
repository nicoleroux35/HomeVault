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
  import { Spinner, Button } from '$lib/components/theme';
  import { PageLayout, EmptyState } from '$lib/components/app';
  import { IconHeart, IconActivity, IconHeartbeat } from '@tabler/icons-svelte';
  import SecurityAdvisorCard from '$lib/components/health/SecurityAdvisorCard.svelte';
  import { isAdmin } from '$lib/stores/auth';

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
  // Error branch (item ROADMAP 1.5): `api.healthCheck()` resolves to
  // `{success:false}` on network failure (and the endpoint itself reports
  // failures in-body), so any outcome without `data` surfaces as an explicit
  // error state with a retry — never as the silent "no checks" branch.
  let errorMessage = '';

  interface DiskFinding {
    id: number;
    fingerprint: string;
    serial: string;
    model: string;
    kind: string;
    severity: string;
    message: string;
    first_seen: string;
    last_seen: string;
  }

  let diskFindings: DiskFinding[] = [];
  let diskHealthLoading = false;
  let diskHealthError = '';
  let diskSampling = false;

  interface PredictionTrend {
    attr_id: number;
    slope_per_day: number;
    r_squared: number;
    days_remaining: number | null;
    days_low: number | null;
    days_high: number | null;
  }

  interface DiskPrediction {
    serial: string;
    trends: PredictionTrend[];
  }

  let diskPredictions: DiskPrediction[] = [];
  let predictionError = '';

  // The 9 checks health.rs always runs, in a fixed, sensible display order —
  // `checks` from the API is already in this order, but this also gives each
  // one a translated display name (the raw `name` is a snake_case identifier
  // like "disk_space", not something to show directly).
  const CHECK_LABEL_KEYS: Record<string, string> = {
    authentication: 'health.check_authentication',
    storage: 'health.check_storage',
    memory: 'health.check_memory',
    disk_space: 'health.check_disk_space',
    disk_smart: 'health.check_disk_smart',
    raid_arrays: 'health.check_raid_arrays',
    network: 'health.check_network',
    services: 'health.check_services',
    temperatures: 'health.check_temperatures',
  };

  function checkLabel(name: string): string {
    const key = CHECK_LABEL_KEYS[name];
    return key ? translate(key) : name;
  }

  const PREDICTION_ATTR_KEYS: Record<number, string> = {
    5: 'health.disk_health_attr_5',
    187: 'health.disk_health_attr_187',
    188: 'health.disk_health_attr_188',
    194: 'health.disk_health_attr_194',
    197: 'health.disk_health_attr_197',
    198: 'health.disk_health_attr_198',
  };

  function predictionAttrLabel(attrId: number): string {
    const key = PREDICTION_ATTR_KEYS[attrId];
    return key ? translate(key) : `SMART ${attrId}`;
  }

  function hasPredictions(): boolean {
    return diskPredictions.some((disk) => disk.trends.length > 0);
  }

  onMount(() => {
    loadHealth();
    loadDiskHealth();
    loadPredictions();
    // No manual refresh button — revalidate on focus + a slow poll (the
    // checks reflect live system state).
    return onAppVisible(() => { void loadHealth(); void loadDiskHealth(); void loadPredictions(); }, { pollMs: 45_000 });
  });

  async function loadPredictions() {
    const response = await api.diskHealthPredictions();
    if (response.success && response.data) {
      diskPredictions = response.data as DiskPrediction[];
      predictionError = '';
    } else {
      predictionError = response.error || translate('health.disk_health_error');
    }
  }

  async function loadDiskHealth() {
    diskHealthLoading = true;
    const response = await api.diskHealthFindings();
    if (response.success && response.data) {
      diskFindings = response.data as DiskFinding[];
      diskHealthError = '';
    } else {
      diskHealthError = response.error || translate('health.disk_health_error');
    }
    diskHealthLoading = false;
  }

  async function sampleDiskHealth() {
    diskSampling = true;
    const response = await api.diskHealthSample();
    diskSampling = false;
    if (response.success) {
      await loadDiskHealth();
    } else {
      diskHealthError = response.error || translate('health.disk_health_error');
    }
  }

  async function loadHealth() {
    loading = true;
    const response = await api.healthCheck();
    if (response.success && response.data) {
      checks = response.data.checks || [];
      overallStatus = response.data.overall_status || 'unknown';
      errorMessage = '';
    } else {
      errorMessage = response.error || translate('health.error_desc');
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

  <!-- Security Advisor (QW-10): admin-only, because the report names accounts,
       the firewall state, the certificate and the shares. It sits above the
       per-check grid on purpose — it answers "what should I fix first?",
       which is the question that brought the user here. -->
  {#if $isAdmin}
    <SecurityAdvisorCard />
  {/if}

  {#if loading}
    <div data-testid="health-loading" class="flex flex-col items-center gap-3 py-20 text-muted-foreground">
      <Spinner />
      <span>{$t('health.loading')}</span>
    </div>
  {:else if errorMessage}
    <EmptyState
      title={$t('health.error_title')}
      description={errorMessage}
      data-testid="health-error"
    >
      {#snippet iconSnippet()}
        <IconHeartbeat size={26} stroke={1.8} />
      {/snippet}
      {#snippet action()}
        <!-- Explicit retry (an error-recovery action, allowed under the
             no-manual-refresh rule). Focusable, labelled, 44px hit area. -->
        <Button
          variant="secondary"
          class="mt-2"
          data-testid="health-retry"
          onclick={() => loadHealth()}
        >
          {$t('health.retry')}
        </Button>
      {/snippet}
    </EmptyState>
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

    <div
      data-testid="health-disk-health"
      class="mt-5 rounded-xl bg-card border border-border shadow-[var(--hv-shell-shadow-card)] p-5"
    >
      <div class="flex items-center justify-between gap-3 flex-wrap mb-3">
        <div>
          <h2 class="m-0 text-c-md font-semibold text-foreground">{$t('health.disk_health_title')}</h2>
          <p class="m-0 mt-1 text-c-base text-muted-foreground">{$t('health.disk_health_desc')}</p>
        </div>
        {#if $isAdmin}
          <Button
            variant="secondary"
            size="sm"
            data-testid="health-disk-sample"
            onclick={sampleDiskHealth}
            disabled={diskSampling}
          >
            {diskSampling ? $t('health.disk_health_sampling') : $t('health.disk_health_sample')}
          </Button>
        {/if}
      </div>
      {#if diskHealthLoading}
        <span class="text-c-base text-muted-foreground">{$t('health.loading')}</span>
      {:else if diskHealthError}
        <span data-testid="health-disk-error" class="text-c-base" style="color: var(--hv-error)">{diskHealthError}</span>
      {:else if diskFindings.length === 0}
        <span data-testid="health-disk-empty" class="text-c-base text-muted-foreground">{$t('health.disk_health_empty')}</span>
      {:else}
        <ul class="m-0 p-0 list-none flex flex-col gap-2" data-testid="health-disk-findings">
          {#each diskFindings as finding (finding.fingerprint)}
            <li class="flex items-start gap-2 text-c-base">
              <span
                class="mt-1 h-2 w-2 rounded-full flex-none"
                style="background: {finding.severity === 'critical' || finding.severity === 'error' ? 'var(--hv-error)' : 'var(--hv-warning)'}"
              ></span>
              <span class="text-foreground">{finding.message}</span>
            </li>
          {/each}
        </ul>
      {/if}
    </div>

    <div
      data-testid="health-prediction"
      class="mt-5 rounded-xl bg-card border border-border shadow-[var(--hv-shell-shadow-card)] p-5"
    >
      <div class="mb-3">
        <h2 class="m-0 text-c-md font-semibold text-foreground">{$t('health.disk_health_prediction_title')}</h2>
        <p class="m-0 mt-1 text-c-base text-muted-foreground">{$t('health.disk_health_prediction_desc')}</p>
      </div>
      {#if predictionError}
        <span class="text-c-base" style="color: var(--hv-error)">{predictionError}</span>
      {:else if !hasPredictions()}
        <span class="text-c-base text-muted-foreground">{$t('health.disk_health_prediction_empty')}</span>
      {:else}
        <div class="flex flex-col gap-3">
          {#each diskPredictions as disk (disk.serial)}
            {#if disk.trends.length > 0}
              <div>
                <div class="text-c-base font-semibold text-foreground">{disk.serial}</div>
                <ul class="m-0 mt-1 p-0 list-none flex flex-col gap-1">
                  {#each disk.trends as trend (trend.attr_id)}
                    <li class="text-c-base text-muted-foreground">
                      <span class="text-foreground">{predictionAttrLabel(trend.attr_id)}</span>
                      {#if trend.days_remaining !== null}
                        — {Math.max(0, Math.round(trend.days_remaining))} {$t('health.disk_health_prediction_days')}
                        (r² {trend.r_squared.toFixed(2)})
                      {:else}
                        — {$t('health.disk_health_prediction_stable')}
                      {/if}
                    </li>
                  {/each}
                </ul>
              </div>
            {/if}
          {/each}
        </div>
      {/if}
    </div>
  {/if}
</PageLayout>
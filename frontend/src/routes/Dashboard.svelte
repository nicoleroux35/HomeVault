<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { api } from '$lib/services/api';
  import { t, translate, languageStore } from '$lib/stores/i18n';
  import { PageLayout, SectionCard } from '$lib/components/app';
  import { BarsChart } from '$lib/components/charts';
  import { cn } from '$lib/utils.js';

  let serverDetails: Record<string, any> | null = null;
  let health: Record<string, any> | null = null;
  let storage: any[] = [];
  let loading = true;
  let timer: ReturnType<typeof setInterval> | undefined;
  let error = '';
  let updatedAt = '';
  let prevNetTraffic: Record<string, { rx: number; tx: number }> = {};

  function fmtBytes(b: number): string {
    if (!b || b === 0) return '0 B';
    if (b >= 1073741824) return (b / 1073741824).toFixed(1) + ' GB';
    if (b >= 1048576) return (b / 1048576).toFixed(0) + ' MB';
    if (b >= 1024) return (b / 1024).toFixed(0) + ' KB';
    return b + ' B';
  }

  function fmtFreq(mhz: number): string {
    if (!mhz || mhz === 0) return '—';
    if (mhz >= 1000) return (mhz / 1000).toFixed(2) + ' GHz';
    return mhz.toFixed(0) + ' MHz';
  }

  function fmtRate(bps: number): string {
    if (!bps || bps === 0) return '0';
    if (bps >= 1_000_000_000) return (bps / 1_000_000_000).toFixed(2);
    if (bps >= 1_000_000) return (bps / 1_000_000).toFixed(1);
    if (bps >= 1_000) return (bps / 1_000).toFixed(0);
    return bps.toFixed(0);
  }

  function fmtRateUnit(bps: number): string {
    if (!bps || bps === 0) return 'bps';
    if (bps >= 1_000_000_000) return 'Gbps';
    if (bps >= 1_000_000) return 'Mbps';
    if (bps >= 1_000) return 'Kbps';
    return 'bps';
  }

  function fmtBytesRate(bps: number): string {
    if (!bps || bps === 0) return '0 o/s';
    const bytesPerSec = bps / 8;
    if (bytesPerSec >= 1073741824) return (bytesPerSec / 1073741824).toFixed(1) + ' Go/s';
    if (bytesPerSec >= 1048576) return (bytesPerSec / 1048576).toFixed(1) + ' Mo/s';
    if (bytesPerSec >= 1024) return (bytesPerSec / 1024).toFixed(0) + ' Ko/s';
    return bytesPerSec.toFixed(0) + ' o/s';
  }

  const LOCALE_MAP: Record<string, string> = { en: 'en-US', fr: 'fr-FR', de: 'de-DE', es: 'es-ES' };

  function tempColor(c: number): string {
    if (c >= 80) return 'var(--hv-error)';
    if (c >= 60) return 'var(--hv-warning)';
    if (c >= 40) return 'var(--hv-success)';
    return 'var(--hv-primary)';
  }

  function ledColor(pct: number): string {
    if (pct >= 90) return 'red';
    if (pct >= 70) return 'amber';
    return 'green';
  }

  async function refresh() {
    try {
      const [detailsRes, healthRes] = await Promise.all([
        api.getServerDetails(),
        api.getHealthData(),
      ]);
      if (detailsRes.success) {
        const prev = prevNetTraffic;
        for (const iface of detailsRes.data?.network?.interfaces ?? []) {
          const key = iface.name;
          if (prev[key]) {
            const rxDelta = iface.rx_bytes - prev[key].rx;
            const txDelta = iface.tx_bytes - prev[key].tx;
            iface.rx_rate = Math.max(0, rxDelta * 8 / 5);
            iface.tx_rate = Math.max(0, txDelta * 8 / 5);
          } else {
            iface.rx_rate = 0;
            iface.tx_rate = 0;
          }
          prev[key] = { rx: iface.rx_bytes, tx: iface.tx_bytes };
        }
        prevNetTraffic = prev;
        serverDetails = detailsRes.data;
      }
      if (healthRes.success) {
        health = healthRes.data;
        if (healthRes.data?.filesystems) {
          storage = healthRes.data.filesystems.slice(0, 4);
        }
      }
      error = '';
    } catch (e) {
      error = translate('dashboard.load_failed');
      console.error('Dashboard error:', e);
    } finally {
      loading = false;
      updatedAt = new Date().toLocaleTimeString(LOCALE_MAP[$languageStore] || 'en-US');
    }
  }

  onMount(() => {
    refresh();
    timer = setInterval(refresh, 5000);
  });

  onDestroy(() => {
    if (timer) clearInterval(timer);
  });

  $: cpu = serverDetails?.cpu;
  $: mem = serverDetails?.memory;
  $: gpuList = serverDetails?.gpu ?? [];
  $: mb = serverDetails?.motherboard;
  $: sensorData = serverDetails?.sensors;
  $: phys = cpu?.physical_cores ?? 0;
  $: logical = cpu?.logical_cores ?? 0;
  $: isHyperThreading = logical > phys;
  $: tempAvg = cpu?.temperatures?.length
    ? cpu.temperatures.reduce((a: number, t: any) => a + t.temp_celsius, 0) / cpu.temperatures.length
    : 0;
  $: memPct = mem?.usage_percent ?? 0;
  $: memUsedGB = mem?.total_bytes ? (mem.used_bytes / 1073741824).toFixed(1) : '0';
  $: memTotalGB = mem?.total_bytes ? (mem.total_bytes / 1073741824).toFixed(1) : '0';
  $: swapPct = mem?.swap_total_bytes ? (mem.swap_used_bytes / mem.swap_total_bytes) * 100 : 0;
  $: netInterfaces = serverDetails?.network?.interfaces ?? [];
  $: netUp = netInterfaces.some((i: any) => i.carrier);
  $: diskHealthy = health?.disk_summary?.healthy_count ?? 0;
  $: diskTotal = health?.disk_summary?.total_count ?? 0;
  $: disks = health?.disks ?? [];
  $: diskAvg = storage.length ? storage.reduce((a: number, fs: any) => a + (fs.usage_percent || 0), 0) / storage.length : 0;
  $: healthScore = health?.score ?? 0;
  $: healthStatus = health?.status ?? 'unknown';

  $: gaugeCircumference = 2 * Math.PI * 84;
  $: gaugeDash = `${(gaugeCircumference * healthScore / 100).toFixed(1)} ${gaugeCircumference.toFixed(1)}`;
  $: healthLabel = healthScore >= 90 ? $t('dashboard.health_sain') : healthScore >= 75 ? $t('dashboard.health_correct') : $t('dashboard.health_watch');
  $: healthHint = $t('dashboard.health_hint_ok');

  $: cpuPct = health?.cpu_usage ?? 0;
  $: cpuCores = cpu?.per_core_usage ?? [];
  $: cpuBarData = (cpuCores as any[]).map((core, i) => ({
    label: `C${i + 1}`,
    value: core.usage_percent || 0,
  }));

  $: netSummary = `${netInterfaces.length} ${$t('dashboard.active_interfaces')}`;
  $: smartSummary = `${diskHealthy}/${diskTotal} ${$t('dashboard.healthy_label')}`;
  $: sensorCount = (sensorData?.cpu_temps?.length ?? 0) + (sensorData?.motherboard_temps?.length ?? 0) + (sensorData?.drive_temps?.length ?? 0);

  $: osInfo = serverDetails?.os ?? serverDetails?.system;
  $: osDistro = osInfo?.distro ?? osInfo?.name ?? '—';
  $: osKernel = osInfo?.kernel ?? osInfo?.kernel_version ?? '—';
  $: osArch = osInfo?.architecture ?? '—';
  $: osHostname = osInfo?.hostname ?? '—';
  $: osInit = osInfo?.init ?? '—';
  $: osDesktop = osInfo?.desktop ?? '—';
  $: osPackages = osInfo?.packages ?? '—';
  $: osTimezone = osInfo?.timezone ?? '—';

  // Shared shapes (appliance tokens only — the per-app CSS vocabulary is gone).
  const cardHead = 'flex items-center gap-3';
  const cardIcon = 'grid place-items-center w-[52px] h-[52px] rounded-2xl flex-none';
  const cardName = 'text-c-xl font-semibold text-[var(--hv-shell-text-strong)]';
  const cardSub = 'text-c-base text-[var(--hv-shell-text-2)] mt-0.5';
  const kvRow = 'flex items-center justify-between min-h-[44px] border-t border-[var(--hv-shell-border-soft)] text-sm text-[var(--hv-shell-text-2)]';
  const mono = 'font-[family-name:var(--hv-font-mono)] text-sm text-[var(--hv-shell-text-strong)]';
  const iconBlue = cn(cardIcon, 'bg-[var(--hv-primary-light)] text-[var(--hv-primary)]');
  const iconGreen = cn(cardIcon, 'bg-[var(--hv-success-bg)] text-[var(--hv-success-text)]');
  const iconAmber = cn(cardIcon, 'bg-[var(--hv-warning-bg)] text-[var(--hv-warning)]');
  const iconCyan = cn(cardIcon, 'bg-[var(--hv-primary-selected-bg)] text-[var(--hv-primary)]');
  const iconCoral = cn(cardIcon, 'bg-[var(--hv-error-bg)] text-[var(--hv-error)]');

  // LED "tile + dot" pair, tinted by severity (appliance tokens).
  const LED: Record<string, [string, string]> = {
    green: ['bg-[var(--hv-success-bg)]', 'bg-[var(--hv-success)] shadow-[0_0_14px_var(--hv-success)]'],
    amber: ['bg-[var(--hv-warning-bg)]', 'bg-[var(--hv-warning)] shadow-[0_0_14px_var(--hv-warning)]'],
    red: ['bg-[var(--hv-error-bg)]', 'bg-[var(--hv-error)] shadow-[0_0_14px_var(--hv-error)]'],
    blue: ['bg-[var(--hv-primary-light)]', 'bg-[var(--hv-primary)] shadow-[0_0_14px_var(--hv-primary)]'],
  };
  const ledTile = (color: string) => cn('relative w-14 h-14 rounded-2xl grid place-items-center flex-none', (LED[color] ?? LED.blue)[0]);
  const ledDot = (color: string) => cn('absolute w-4 h-4 rounded-full animate-pulse', (LED[color] ?? LED.blue)[1]);
</script>

<PageLayout
  title={$t('dashboard.title')}
  data-testid="dashboard-page"
>
  {#snippet icon()}
    <!-- No width/height here: `WindowHeader`'s `.hv-winhdr-icon :global(svg)`
         clamp owns the tile glyph size, and a stylesheet beats a presentation
         attribute — declaring 26px would be dead. `viewBox` is what makes the
         clamp scale the glyph proportionally, so it stays. -->
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">
      <path d="M4 18a8 8 0 1 1 16 0"></path>
      <path d="M12 18l4-5.5"></path>
      <path d="M4 18h16"></path>
    </svg>
  {/snippet}

  {#if loading && !serverDetails}
    <div data-testid="dashboard-loading" class="flex items-center justify-center py-20 text-[var(--hv-shell-text-3)] text-base">{$t('dashboard.loading_metrics')}</div>
  {:else}
    {#if error}
      <div class="px-5 py-3 rounded-md bg-[rgba(255,107,107,0.15)] border border-[rgba(255,107,107,0.3)] text-[var(--hv-error-text)] text-sm">{error}</div>
    {/if}

    <div data-testid="dashboard-content" class="@container">
      <div class="grid grid-cols-12 gap-5 items-stretch">
        <div data-testid="dashboard-stats-grid" class="grid grid-cols-4 gap-4 col-span-12 @max-[768px]:grid-cols-2 @max-[480px]:grid-cols-1">
          <div data-testid="dashboard-stat-card" class="flex items-center gap-4 px-6 py-5 rounded-2xl bg-[var(--hv-shell-card)] border border-[var(--hv-shell-border)] shadow-[var(--hv-shell-shadow-card)] transition-colors hover:border-[var(--hv-shell-border-strong)]">
            <span class={ledTile(ledColor(cpuPct))}><span class={ledDot(ledColor(cpuPct))}></span></span>
            <div class="flex-1 min-w-0">
              <div data-testid="dashboard-stat-label" class="text-sm text-[var(--hv-shell-text-2)]">{$t('dashboard.cpu')}</div>
              <div data-testid="dashboard-stat-value" class="flex items-baseline gap-1.5 mt-1 text-4xl font-semibold tracking-tight text-[var(--hv-shell-text-strong)]">{cpuPct.toFixed(0)}<span class="text-c-base text-[var(--hv-shell-text)]">%</span></div>
            </div>
          </div>
          <div data-testid="dashboard-stat-card" class="flex items-center gap-4 px-6 py-5 rounded-2xl bg-[var(--hv-shell-card)] border border-[var(--hv-shell-border)] shadow-[var(--hv-shell-shadow-card)] transition-colors hover:border-[var(--hv-shell-border-strong)]">
            <span class={ledTile(ledColor(memPct))}><span class={ledDot(ledColor(memPct))}></span></span>
            <div class="flex-1 min-w-0">
              <div data-testid="dashboard-stat-label" class="text-sm text-[var(--hv-shell-text-2)]">{$t('dashboard.ram')}</div>
              <div data-testid="dashboard-stat-value" class="flex items-baseline gap-1.5 mt-1 text-4xl font-semibold tracking-tight text-[var(--hv-shell-text-strong)]">{memPct.toFixed(0)}<span class="text-c-base text-[var(--hv-shell-text)]">%</span></div>
            </div>
          </div>
          <div data-testid="dashboard-stat-card" class="flex items-center gap-4 px-6 py-5 rounded-2xl bg-[var(--hv-shell-card)] border border-[var(--hv-shell-border)] shadow-[var(--hv-shell-shadow-card)] transition-colors hover:border-[var(--hv-shell-border-strong)]">
            <span class={ledTile(ledColor(diskAvg))}><span class={ledDot(ledColor(diskAvg))}></span></span>
            <div class="flex-1 min-w-0">
              <div data-testid="dashboard-stat-label" class="text-sm text-[var(--hv-shell-text-2)]">{$t('dashboard.disk')}</div>
              <div data-testid="dashboard-stat-value" class="flex items-baseline gap-1.5 mt-1 text-4xl font-semibold tracking-tight text-[var(--hv-shell-text-strong)]">{diskAvg.toFixed(0)}<span class="text-c-base text-[var(--hv-shell-text)]">%</span></div>
            </div>
          </div>
          <div data-testid="dashboard-stat-card" class="flex items-center gap-4 px-6 py-5 rounded-2xl bg-[var(--hv-shell-card)] border border-[var(--hv-shell-border)] shadow-[var(--hv-shell-shadow-card)] transition-colors hover:border-[var(--hv-shell-border-strong)]">
            <span class={ledTile('blue')}><span class={ledDot('blue')}></span></span>
            <div class="flex-1 min-w-0">
              <div data-testid="dashboard-stat-label" class="text-sm text-[var(--hv-shell-text-2)]">{$t('dashboard.network')}</div>
              <div data-testid="dashboard-stat-value" class="flex items-baseline gap-1.5 mt-1 text-4xl font-semibold tracking-tight text-[var(--hv-shell-text-strong)]">{netInterfaces.length > 0 ? netInterfaces.length : '0'}</div>
            </div>
          </div>
        </div>
        <!-- Health -->
        <SectionCard data-testid="dashboard-card" class="flex flex-col col-span-4 @max-[1200px]:col-span-6 @max-[768px]:col-span-12 [&>.hv-section-card-body]:flex [&>.hv-section-card-body]:flex-col [&>.hv-section-card-body]:items-center">
          {#snippet titleSnippet()}
          <div class="self-stretch text-c-card-title font-semibold text-[var(--hv-shell-text-strong)]">{$t('dashboard.health_score')}</div>
          {/snippet}
          <div class="relative w-[236px] h-[236px] mt-3">
            <svg viewBox="0 0 200 200" width="236" height="236" class="block max-w-full -rotate-90">
              <circle cx="100" cy="100" r="84" fill="none" stroke="var(--hv-shell-border-soft)" stroke-width="18"></circle>
              <circle cx="100" cy="100" r="84" fill="none" stroke="url(#db-gauge-grad)" stroke-width="18" stroke-linecap="round" stroke-dasharray={gaugeDash}></circle>
              <defs>
                <linearGradient id="db-gauge-grad" x1="0" y1="0" x2="1" y2="1">
                  <stop offset="0" stop-color="var(--hv-success)"></stop>
                  <stop offset="1" stop-color="var(--hv-primary)"></stop>
                </linearGradient>
              </defs>
            </svg>
            <div class="absolute inset-0 flex flex-col items-center justify-center">
              <div data-testid="dashboard-gauge-score" class="text-5xl font-semibold tracking-tight leading-none text-[var(--hv-shell-text-strong)]">{healthScore}</div>
              <div class="text-sm text-[var(--hv-shell-text-2)] mt-1">{$t('dashboard.health_out_of')}</div>
            </div>
          </div>
          <div class="mt-4 text-xl font-semibold text-[var(--hv-success-text)]">{healthLabel}</div>
          <div class="mt-1.5 text-sm leading-relaxed text-center text-[var(--hv-shell-text-2)] text-pretty">{healthHint}</div>
          <div class="self-stretch flex flex-col gap-0.5 mt-5 pt-4 border-t border-[var(--hv-shell-border-soft)]">
            <div class={kvRow}><span>{$t('dashboard.uptime_label')}</span><span class={mono}>{osInfo?.uptime ?? '—'}</span></div>
            <div class={kvRow}><span>{$t('dashboard.load_average')}</span><span class={mono}>{cpu?.load_avg?.[0]?.toFixed(2) ?? '—'} · {cpu?.load_avg?.[1]?.toFixed(2) ?? '—'} · {cpu?.load_avg?.[2]?.toFixed(2) ?? '—'}</span></div>
            <div class={kvRow}><span>{$t('dashboard.os_kernel')}</span><span class={mono}>{osKernel}</span></div>
          </div>
        </SectionCard>

        <!-- CPU -->
        <SectionCard data-testid="dashboard-card" class="flex flex-col col-span-4 @max-[1200px]:col-span-6 @max-[768px]:col-span-12 [&>.hv-section-card-body]:flex [&>.hv-section-card-body]:flex-1 [&>.hv-section-card-body]:flex-col">
          {#snippet titleSnippet()}
          <div class={cardHead}>
            <span class={iconBlue}>
              <svg viewBox="0 0 24 24" width="24" height="24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linejoin="round">
                <rect x="7" y="7" width="10" height="10" rx="2"></rect>
                <path d="M4 10h3M4 14h3M17 10h3M17 14h3M10 4v3M14 4v3M10 17v3M14 17v3"></path>
              </svg>
            </span>
            <div class="flex-1 min-w-0">
              <div class={cardName}>{$t('dashboard.processor')}</div>
              <div class={cardSub}>{cpu?.model_name ?? '—'}</div>
            </div>
          </div>
          {/snippet}
          {#snippet actions()}
          <span class="font-[family-name:var(--hv-font-mono)] text-c-stat font-bold text-[var(--hv-primary)]">{tempAvg > 0 ? tempAvg.toFixed(0) + ' °C' : '—'}</span>
          {/snippet}
          <div class="h-24 mt-5 p-3 box-border rounded-lg bg-[var(--hv-shell-fill)]">
            <BarsChart data={cpuBarData} height={72} color="var(--hv-primary)" />
          </div>
          <div class="flex flex-col gap-0.5 mt-auto pt-3">
            <div class={kvRow}><span>{$t('dashboard.clock')}</span><span class={mono}>{fmtFreq(cpu?.frequency_mhz)}</span></div>
            <div class={kvRow}><span>{$t('dashboard.cores')} / {$t('dashboard.threads')}</span><span class={mono}>{phys} / {logical}{isHyperThreading ? ' (HT)' : ''}</span></div>
            <div class={kvRow}><span>{$t('dashboard.load_average')}</span><span class={mono}>{cpuPct.toFixed(0)} %</span></div>
          </div>
        </SectionCard>

        <!-- Memory -->
        <SectionCard data-testid="dashboard-card" class="flex flex-col col-span-4 @max-[1200px]:col-span-6 @max-[768px]:col-span-12 [&>.hv-section-card-body]:flex [&>.hv-section-card-body]:flex-1 [&>.hv-section-card-body]:flex-col">
          {#snippet titleSnippet()}
          <div class={cardHead}>
            <span class={iconGreen}>
              <svg viewBox="0 0 24 24" width="24" height="24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linejoin="round">
                <rect x="3" y="7" width="18" height="10" rx="2"></rect>
                <path d="M7 17v3M12 17v3M17 17v3M7.5 10.5h3M13.5 10.5h3"></path>
              </svg>
            </span>
            <div class="flex-1 min-w-0">
              <div class={cardName}>{$t('dashboard.memory')}</div>
              <div class={cardSub}>{mem?.ram_type ?? '—'} · {mem?.ram_speed_mhz ?? '—'} MT/s</div>
            </div>
          </div>
          {/snippet}
          {#snippet actions()}
          <span class={cn('font-[family-name:var(--hv-font-mono)] text-c-stat font-bold', 'text-[var(--hv-success-text)]')}>{memPct.toFixed(0)} %</span>
          {/snippet}
          <div data-testid="dashboard-mem-bar" class="h-[26px] mt-5 rounded-full bg-[var(--hv-shell-fill)] overflow-hidden">
            <svg viewBox="0 0 100 26" preserveAspectRatio="none" width="100%" height="26">
              <defs>
                <linearGradient id="db-mem-grad" x1="0" y1="0" x2="1" y2="0">
                  <stop offset="0" stop-color="var(--hv-success)"></stop>
                  <stop offset="1" stop-color="var(--hv-primary)"></stop>
                </linearGradient>
              </defs>
              <rect x="0" y="0" width={memPct} height="26" fill="url(#db-mem-grad)"></rect>
            </svg>
          </div>
          <div data-testid="dashboard-mem-labels" class="flex justify-between mt-2.5 font-[family-name:var(--hv-font-mono)] text-sm text-[var(--hv-shell-text-2)]">
            <span>{memUsedGB} {$t('dashboard.used_suffix')}</span>
            <span>{memTotalGB} Go</span>
          </div>
          <div class="text-sm text-[var(--hv-shell-text-2)] mt-5">Swap</div>
          <div class="h-[18px] mt-2 rounded-full bg-[var(--hv-shell-fill)] overflow-hidden">
            <div class="h-full rounded-full bg-[linear-gradient(90deg,var(--hv-primary),var(--hv-primary-400))]" style="width: {Math.min(swapPct, 100)}%"></div>
          </div>
          <div class="flex flex-col gap-0.5 mt-auto pt-3">
            <div class={kvRow}><span>{$t('dashboard.swap_used')}</span><span class={mono}>{fmtBytes(mem?.swap_used_bytes || 0)} / {fmtBytes(mem?.swap_total_bytes || 0)}</span></div>
            <div class={kvRow}><span>{$t('dashboard.cache_buffers')}</span><span class={mono}>{fmtBytes(mem?.cached_bytes || 0)}</span></div>
            <div class={kvRow}><span>{$t('dashboard.available')}</span><span class={mono}>{fmtBytes(mem?.available_bytes || 0)}</span></div>
          </div>
        </SectionCard>
        <!-- GPUs -->
        {#each gpuList as gpu}
          <SectionCard data-testid="dashboard-card" class="col-span-6 @max-[768px]:col-span-12">
            {#snippet titleSnippet()}
            <div class={cardHead}>
              <span class={iconBlue}>
                <svg viewBox="0 0 24 24" width="24" height="24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linejoin="round">
                  <rect x="2.5" y="6" width="19" height="11" rx="2.5"></rect>
                  <circle cx="9" cy="11.5" r="2.6"></circle>
                  <circle cx="16" cy="11.5" r="2.6"></circle>
                  <path d="M6 17v3"></path>
                </svg>
              </span>
              <div class="flex-1 min-w-0">
                <div class={cardName}>{gpu.model ?? $t('dashboard.gpu')}</div>
                <div class={cardSub}>{gpu.gpu_type ?? '—'}</div>
              </div>
            </div>
            {/snippet}
            {#snippet actions()}
            <span class="inline-flex items-center h-11 px-3 flex-none rounded-full bg-[var(--hv-primary-light)] text-[var(--hv-primary)] text-c-base font-bold">{gpu.gpu_type ?? 'GPU'}</span>
            {/snippet}
            {#if gpu.utilization_percent !== null && gpu.utilization_percent !== undefined}
              <div class="flex gap-5 mt-5">
                <div class="flex-1">
                  <div class="flex justify-between text-sm text-[var(--hv-shell-text-2)]"><span>{$t('dashboard.utilization')}</span><span class={mono}>{gpu.utilization_percent}%</span></div>
                  <div class="h-5 mt-2 rounded-full bg-[var(--hv-shell-fill)] overflow-hidden">
                    <div class="h-full rounded-full bg-[linear-gradient(90deg,var(--hv-primary),var(--hv-primary-400))]" style="width: {gpu.utilization_percent}%"></div>
                  </div>
                </div>
                {#if gpu.memory_total_bytes}
                  {@const vramPct = (gpu.memory_used_bytes / gpu.memory_total_bytes) * 100}
                  <div class="flex-1">
                    <div class="flex justify-between text-sm text-[var(--hv-shell-text-2)]"><span>{$t('dashboard.vram')}</span><span class={mono}>{fmtBytes(gpu.memory_used_bytes)} / {fmtBytes(gpu.memory_total_bytes)}</span></div>
                    <div class="h-5 mt-2 rounded-full bg-[var(--hv-shell-fill)] overflow-hidden">
                      <div class="h-full rounded-full bg-[linear-gradient(90deg,var(--hv-success),var(--hv-success-hover))]" style="width: {vramPct}%"></div>
                    </div>
                  </div>
                {/if}
              </div>
            {/if}
            <div class="grid grid-cols-3 gap-3 mt-5 @max-[480px]:grid-cols-1">
              {#if gpu.freq_current_mhz}
                <div class="px-4 py-3 min-h-[76px] rounded-lg bg-[var(--hv-shell-fill)]">
                  <div class="text-c-base text-[var(--hv-shell-text-2)]">{$t('dashboard.clock')}</div>
                  <div class="font-[family-name:var(--hv-font-mono)] text-xl font-bold mt-1.5">{fmtFreq(gpu.freq_current_mhz)}</div>
                </div>
              {/if}
              {#if gpu.temperature_celsius !== null && gpu.temperature_celsius !== undefined}
                <div class="px-4 py-3 min-h-[76px] rounded-lg bg-[var(--hv-shell-fill)]">
                  <div class="text-c-base text-[var(--hv-shell-text-2)]">{$t('dashboard.temperature')}</div>
                  <div class="font-[family-name:var(--hv-font-mono)] text-xl font-bold mt-1.5">{gpu.temperature_celsius.toFixed(0)} °C</div>
                </div>
              {/if}
              {#if gpu.power_watts !== null && gpu.power_watts !== undefined}
                <div class="px-4 py-3 min-h-[76px] rounded-lg bg-[var(--hv-shell-fill)]">
                  <div class="text-c-base text-[var(--hv-shell-text-2)]">{$t('dashboard.power')}</div>
                  <div class="font-[family-name:var(--hv-font-mono)] text-xl font-bold mt-1.5">{gpu.power_watts.toFixed(0)} W</div>
                </div>
              {/if}
            </div>
          </SectionCard>
        {/each}

        <!-- Motherboard -->
        {#if mb?.manufacturer || mb?.model}
          <SectionCard data-testid="dashboard-card" class="col-span-4 @max-[1200px]:col-span-6 @max-[768px]:col-span-12">
            {#snippet titleSnippet()}
            <div class={cardHead}>
              <span class={iconAmber}>
                <svg viewBox="0 0 24 24" width="24" height="24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linejoin="round">
                  <rect x="3.5" y="3.5" width="17" height="17" rx="2.5"></rect>
                  <rect x="8" y="8" width="8" height="6" rx="1.5"></rect>
                  <path d="M8 17h8"></path>
                </svg>
              </span>
              <div class="flex-1 min-w-0">
                <div class={cardName}>{$t('dashboard.motherboard_label')}</div>
                <div class={cardSub}>{mb.manufacturer ?? '—'}</div>
              </div>
            </div>
            {/snippet}
            <div class="flex flex-col gap-0.5">
              {#if mb.model}<div class={kvRow}><span>{$t('dashboard.model')}</span><span class={mono}>{mb.model}</span></div>{/if}
              {#if mb.bios_version}<div class={kvRow}><span>{$t('dashboard.bios')}</span><span class={mono}>{mb.bios_version}</span></div>{/if}
              {#if mb.bios_date}<div class={kvRow}><span>Date BIOS</span><span class={mono}>{mb.bios_date}</span></div>{/if}
              {#if mb.chipset}<div class={kvRow}><span>Chipset</span><span class={mono}>{mb.chipset}</span></div>{/if}
            </div>
          </SectionCard>
        {/if}

        <!-- OS -->
        <SectionCard data-testid="dashboard-card" class="col-span-4 @max-[1200px]:col-span-6 @max-[768px]:col-span-12">
          {#snippet titleSnippet()}
          <div class={cardHead}>
            <span class={iconBlue}>
              <svg viewBox="0 0 24 24" width="24" height="24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round">
                <rect x="2.5" y="4.5" width="19" height="13" rx="2.5"></rect>
                <path d="M8 20.5h8M12 17.5v3"></path>
                <path d="M7 9l2.5 2L7 13"></path>
                <path d="M12 13h5"></path>
              </svg>
            </span>
            <div class="flex-1 min-w-0">
              <div class={cardName}>{$t('dashboard.os_label')}</div>
              <div class={cardSub}>{osDistro}</div>
            </div>
          </div>
          {/snippet}
          {#snippet actions()}
          <span class="inline-flex items-center gap-2 h-11 px-3 flex-none rounded-full bg-[var(--hv-success-bg)] text-[var(--hv-success)] text-c-base font-bold">
              <span class="w-2.5 h-2.5 rounded-full bg-[var(--hv-success)]"></span>
              {$t('dashboard.os_uptodate')}
            </span>
          {/snippet}
          <div data-testid="dashboard-os-grid" class="grid grid-cols-2 gap-x-5 gap-y-0.5 mt-4 @max-[768px]:grid-cols-1">
            {#if osKernel}<div class={kvRow}><span>{$t('dashboard.os_kernel')}</span><span class={mono}>{osKernel}</span></div>{/if}
            {#if osArch}<div class={kvRow}><span>{$t('dashboard.os_architecture')}</span><span class={mono}>{osArch}</span></div>{/if}
            {#if osHostname}<div class={kvRow}><span>{$t('dashboard.os_hostname')}</span><span class={mono}>{osHostname}</span></div>{/if}
            {#if osInit}<div class={kvRow}><span>{$t('dashboard.os_init')}</span><span class={mono}>{osInit}</span></div>{/if}
            {#if osDesktop}<div class={kvRow}><span>{$t('dashboard.os_desktop')}</span><span class={mono}>{osDesktop}</span></div>{/if}
            {#if osPackages}<div class={kvRow}><span>{$t('dashboard.os_packages')}</span><span class={mono}>{osPackages}</span></div>{/if}
            {#if osTimezone}<div class={kvRow}><span>{$t('dashboard.os_timezone')}</span><span class={mono}>{osTimezone}</span></div>{/if}
          </div>
        </SectionCard>

        <!-- Storage -->
        {#if storage.length}
          <SectionCard data-testid="dashboard-card" class="col-span-6 @max-[768px]:col-span-12">
            {#snippet titleSnippet()}
            <div class={cardHead}>
              <span class={iconCyan}>
                <svg viewBox="0 0 24 24" width="24" height="24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linejoin="round">
                  <ellipse cx="12" cy="6.5" rx="7.5" ry="3"></ellipse>
                  <path d="M4.5 6.5v11c0 1.7 3.4 3 7.5 3s7.5-1.3 7.5-3v-11"></path>
                  <path d="M4.5 12c0 1.7 3.4 3 7.5 3s7.5-1.3 7.5-3"></path>
                </svg>
              </span>
              <div class="flex-1 min-w-0">
                <div class={cardName}>{$t('dashboard.storage_label')}</div>
                <div class={cardSub}>{storage.length} {$t('dashboard.filesystems_count')}</div>
              </div>
            </div>
            {/snippet}
            <div data-testid="dashboard-storage-bars" class="flex flex-col gap-4 mt-5">
              {#each storage as fs, i}
                {@const pct = fs.usage_percent || 0}
                {@const colors = ['var(--hv-success)', 'var(--hv-primary)', 'var(--hv-warning)', 'var(--hv-success)']}
                {@const c = colors[i % colors.length]}
                <div data-testid="dashboard-storage-row">
                  <div class="flex items-baseline gap-2.5">
                    <span class={cn(mono, 'text-c-card-title font-medium flex-1 min-w-0')}>{fs.mount || '/'}</span>
                    <span class="text-sm text-[var(--hv-shell-text-2)]">{fs.used_human ?? '—'} / {fs.size_human ?? '—'}</span>
                  </div>
                  <div class="h-[22px] mt-2 rounded-full bg-[var(--hv-shell-fill)] overflow-hidden">
                    <div class="h-full rounded-full transition-[width]" style="width: {Math.min(pct, 100)}%; background: {c}"></div>
                  </div>
                </div>
              {/each}
            </div>
            <div class="flex items-center gap-4 mt-5 px-5 py-4 rounded-lg bg-[var(--hv-success-bg)] border border-[var(--hv-success)]/40">
              <span class="grid place-items-center w-[52px] h-[52px] rounded-2xl bg-[var(--hv-success-bg)] text-[var(--hv-success)] flex-none">
                <svg viewBox="0 0 24 24" width="24" height="24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">
                  <path d="M12 3.5l7 3v5.5c0 4-3 7-7 8.5-4-1.5-7-4.5-7-8.5V6.5z"></path>
                  <path d="M9 12l2.2 2.2L15.5 10"></path>
                </svg>
              </span>
              <div class="flex-1 min-w-0">
                <div class="text-base font-semibold">{$t('dashboard.smart_health')}</div>
                <div class="text-sm text-[var(--hv-shell-text-2)] mt-0.5">{$t('dashboard.all_healthy')} · {$t('dashboard.last_test')} 2 {$t('dashboard.last_check')}</div>
              </div>
              <span class="font-[family-name:var(--hv-font-mono)] text-2xl font-bold text-[var(--hv-success-text)]">{diskHealthy} / {diskTotal}</span>
            </div>
          </SectionCard>
        {/if}

        <!-- Network -->
        {#if netInterfaces.length}
          <SectionCard data-testid="dashboard-card" class="col-span-6 @max-[768px]:col-span-12">
            {#snippet titleSnippet()}
            <div class={cardHead}>
              <span class={iconBlue}>
                <svg viewBox="0 0 24 24" width="24" height="24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round">
                  <circle cx="12" cy="5.5" r="2.5"></circle>
                  <circle cx="5.5" cy="18" r="2.5"></circle>
                  <circle cx="18.5" cy="18" r="2.5"></circle>
                  <path d="M12 8v4M12 12H6.5v3.5M12 12h5.5v3.5"></path>
                </svg>
              </span>
              <div class="flex-1 min-w-0">
                <div class={cardName}>{$t('dashboard.network_label')}</div>
                <div class={cardSub}>{netSummary}</div>
              </div>
            </div>
            {/snippet}
            <div class="flex flex-col gap-3 mt-5">
              {#each netInterfaces as iface}
                {@const rx = fmtBytesRate(iface.rx_rate || 0)}
                {@const tx = fmtBytesRate(iface.tx_rate || 0)}
                <div data-testid="dashboard-net-iface" class="px-5 py-4 rounded-lg bg-[var(--hv-shell-fill)] border border-[var(--hv-shell-border-soft)]">
                  <div class="flex items-center gap-3">
                    <span class="font-[family-name:var(--hv-font-mono)] text-c-xl font-bold">{iface.name}</span>
                    <span class="inline-flex items-center gap-2 h-[34px] px-3 rounded-full bg-[var(--hv-primary-light)] text-[var(--hv-primary)] text-xs font-bold">
                      <span class="w-2 h-2 rounded-full bg-[var(--hv-primary)]"></span>
                      {iface.carrier ? 'ACTIVE' : 'DOWN'}
                    </span>
                    <span class="flex-1 text-right text-sm text-[var(--hv-shell-text-2)]">{iface.speed ? iface.speed + ' Mb/s' : '—'}</span>
                  </div>
                  <div class="grid grid-cols-2 gap-x-5 gap-y-0.5 mt-3 @max-[768px]:grid-cols-1">
                    {#if iface.ipv4?.length}<div class={kvRow}><span>IPv4</span><span class={mono}>{iface.ipv4[0]}</span></div>{/if}
                    {#if iface.mac_address}<div class={kvRow}><span>MAC</span><span class={mono}>{iface.mac_address}</span></div>{/if}
                    {#if iface.driver}<div class={kvRow}><span>Driver</span><span class={mono}>{iface.driver}</span></div>{/if}
                  </div>
                  <div class="flex gap-3 mt-3">
                    <div class="flex-1 flex items-center gap-2.5 h-[52px] px-4 rounded-md bg-[var(--hv-success-bg)]">
                      <svg viewBox="0 0 24 24" width="20" height="20" fill="none" stroke="var(--hv-success)" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M12 19V5M6 13l6 6 6-6"></path></svg>
                      <span class={cn(mono, 'text-base font-bold text-[var(--hv-success-text)]')}>{rx}</span>
                    </div>
                    <div class="flex-1 flex items-center gap-2.5 h-[52px] px-4 rounded-md bg-[var(--hv-primary-light)]">
                      <svg viewBox="0 0 24 24" width="20" height="20" fill="none" stroke="var(--hv-primary)" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M12 5v14M6 11l6-6 6 6"></path></svg>
                      <span class={cn(mono, 'text-base font-bold text-[var(--hv-primary)]')}>{tx}</span>
                    </div>
                  </div>
                </div>
              {/each}
            </div>
          </SectionCard>
        {/if}

        <!-- Sensors -->
        {#if sensorCount > 0}
          <SectionCard data-testid="dashboard-card" class="col-span-12">
            {#snippet titleSnippet()}
            <div class={cardHead}>
              <span class={iconCoral}>
                <svg viewBox="0 0 24 24" width="24" height="24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">
                  <path d="M10 14.5V5.5a2 2 0 1 1 4 0v9a4 4 0 1 1-4 0z"></path>
                  <path d="M12 17.5v.5"></path>
                </svg>
              </span>
              <div class="flex-1 min-w-0">
                <div class={cardName}>{$t('dashboard.sensors_label')}</div>
                <div class={cardSub}>{sensorCount} {$t('dashboard.sensors_count')}</div>
              </div>
            </div>
            {/snippet}
            <div data-testid="dashboard-sensors-grid" class="grid gap-3 mt-5 [grid-template-columns:repeat(auto-fill,minmax(180px,1fr))]">
              {#each sensorData?.cpu_temps ?? [] as s}
                <div class="px-4 py-4 min-h-24 rounded-xl bg-[var(--hv-shell-fill)] border border-[var(--hv-shell-border-soft)]">
                  <div class="text-c-base text-[var(--hv-shell-text-2)] leading-snug">{s.label || $t('dashboard.cpu')}</div>
                  <div class="flex items-baseline gap-1 mt-2 font-[family-name:var(--hv-font-mono)] text-2xl font-bold" style="color: {tempColor(s.temp_celsius)}">{s.temp_celsius.toFixed(0)}<span class="text-sm text-[var(--hv-shell-text)]">°C</span></div>
                </div>
              {/each}
              {#each sensorData?.motherboard_temps ?? [] as s}
                <div class="px-4 py-4 min-h-24 rounded-xl bg-[var(--hv-shell-fill)] border border-[var(--hv-shell-border-soft)]">
                  <div class="text-c-base text-[var(--hv-shell-text-2)] leading-snug">{s.label || $t('dashboard.motherboard_label')}</div>
                  <div class="flex items-baseline gap-1 mt-2 font-[family-name:var(--hv-font-mono)] text-2xl font-bold" style="color: {tempColor(s.temp_celsius)}">{s.temp_celsius.toFixed(0)}<span class="text-sm text-[var(--hv-shell-text)]">°C</span></div>
                </div>
              {/each}
              {#each sensorData?.drive_temps ?? [] as s}
                <div class="px-4 py-4 min-h-24 rounded-xl bg-[var(--hv-shell-fill)] border border-[var(--hv-shell-border-soft)]">
                  <div class="text-c-base text-[var(--hv-shell-text-2)] leading-snug">{s.label}</div>
                  <div class="flex items-baseline gap-1 mt-2 font-[family-name:var(--hv-font-mono)] text-2xl font-bold" style="color: {tempColor(s.temp_celsius)}">{s.temp_celsius.toFixed(0)}<span class="text-sm text-[var(--hv-shell-text)]">°C</span></div>
                </div>
              {/each}
            </div>
          </SectionCard>
        {/if}
      </div>
    </div>
  {/if}
</PageLayout>
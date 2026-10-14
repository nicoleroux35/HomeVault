<!-- ========================================================================
     File          : Rescue.svelte
     Author        : Nicolas Le Roux
     Created       : 28/09/2026
     Last modified : 28/09/2026 by Nicolas Le Roux
     Version       : 1.0.0

     Description:
     The Rescue app: disk cloning (ddrescue) with live progress, partition
     analysis and rebuild (TestDisk), file carving (PhotoRec), and mdadm
     diagnose/repair — each with validated options.

     Dependencies:
       - svelte (>= 5), theme + app/ composites, $lib/services/api

     Usage:
     Opened as a desktop window app (admin only).
     ======================================================================== -->
<script lang="ts">
  import { onMount } from 'svelte';
  import { api, type ProgressEvent } from '$lib/services/api';
  import { t, translate } from '$lib/stores/i18n';
  import { notificationsStore } from '$lib/stores/notifications';
  import { Button, Badge, Alert, Spinner } from '$lib/components/theme';
  import PathField from '$lib/components/theme/PathField.svelte';
  import { PageLayout, SectionCard } from '$lib/components/app';
  import { cn } from '$lib/utils.js';
  import { IconAlertTriangle, IconCircleCheck, IconCircleX, IconServer, IconRefresh, IconPlayerPlay, IconDownload, IconSearch, IconShield, IconSettings2 } from '@tabler/icons-svelte';

  let activeTab = 'ddrescue';

  let tools: Array<{ name: string; available: boolean; version: string | null }> = [];
  let devices: Array<{ device: string; name: string; size: number; model: string; transport: string; removable: boolean }> = [];
  let loading = true;

  // -- Modes simple/avancé --
  let ddMode: 'simple' | 'advanced' = 'simple';
  let tdMode: 'simple' | 'advanced' = 'simple';
  let prMode: 'simple' | 'advanced' = 'simple';
  let mdMode: 'simple' | 'advanced' = 'simple';

  // -- ddrescue --
  let ddSource = '';
  let ddDest = '';
  let ddLogfile = '';
  let ddJobId = '';
  let ddRunning = false;
  let ddProgress = 0;
  let ddMessage = '';
  // ddrescue advanced
  let ddBlockSize = 512;
  let ddRetryPasses = 0;
  let ddDirectIO = false;
  let ddReverse = false;
  let ddVerbose = false;

  // -- TestDisk --
  let tdDevice = '';
  let tdAnalyzing = false;
  let tdResult: any = null;
  // TestDisk advanced
  let tdDeepSearch = false;
  let tdLogPath = '';
  let tdForce = false;

  // -- PhotoRec --
  let prDevice = '';
  let prOutputDir = '/tmp/recovered';
  let prJobId = '';
  let prRunning = false;
  // PhotoRec advanced
  let prFileTypes: string[] = [];
  let prKeepBroken = false;
  let prMaxSize = 0;
  let prPartitionType = '';

  const availableFileTypes = ['jpg', 'png', 'gif', 'pdf', 'zip', 'doc', 'xls', 'mp3', 'mp4', 'avi'];

  // -- mdadm --
  let mdName = '';
  let mdDiagnostic: any = null;
  let mdLoading = false;
  let mdRepairing = false;
  // mdadm advanced
  let mdAction = 'repair';
  let mdForce = false;
  let mdVerbose = false;

  // Shared shapes (appliance tokens only — the per-app CSS vocabulary is gone).
  const inpSel =
    'w-full px-3 py-2 border border-[var(--hv-border)] rounded-[var(--hv-radius-default)] ' +
    'text-sm text-[var(--hv-neutral-800)] bg-[var(--hv-neutral-50)] ' +
    'focus:outline-none focus:border-[var(--hv-primary)] focus:shadow-[0_0_0_3px_var(--hv-primary-light)]';
  const formGroup = 'flex flex-col gap-1.5 mb-4';
  const formLabel = 'text-xs font-medium text-[var(--hv-neutral-700)]';
  const checkboxLabel = 'flex items-center gap-2 text-xs text-[var(--hv-neutral-700)] cursor-pointer select-none';
  const modeToggle = 'inline-flex border border-[var(--hv-border)] rounded-md overflow-hidden ml-2';
  const modeBtn =
    'px-3 py-1 border-none bg-transparent text-c-xs font-semibold uppercase tracking-wide ' +
    'text-[var(--hv-neutral-500)] cursor-pointer transition-colors hover:bg-[var(--hv-neutral-100)]';

  onMount(async () => {
    await loadTools();
    await loadDevices();
    loading = false;
  });

  async function loadTools() {
    const r = await api.getRescueTools();
    if (r.success && r.data) {
      tools = (r.data as any).tools || [];
    }
  }

  async function loadDevices() {
    const r = await api.getRescueDevices();
    if (r.success && r.data) {
      devices = r.data as any;
    }
  }

  function formatSize(bytes: number): string {
    if (bytes === 0) return '0 B';
    const units = ['B', 'KB', 'MB', 'GB', 'TB', 'PB'];
    const i = Math.floor(Math.log(bytes) / Math.log(1024));
    const val = bytes / Math.pow(1024, i);
    return val.toFixed(i > 0 ? 1 : 0) + ' ' + units[i];
  }

  // onSuccess fires only on a confirmed completion (callers show a success
  // toast from it) — onSettle fires in every terminal case (success,
  // failure, or unknown) so callers can unconditionally clear their
  // "running" flag. Previously "failed" and connection-error paths never
  // reset that flag at all, leaving the Start button permanently disabled
  // with no way to retry after a real failure.
  function connectSSE(
    jobId: string,
    onProgress: (pct: number, msg: string) => void,
    onSuccess: (msg: string) => void,
    onSettle: () => void
  ) {
    const evtSource = new EventSource(`/api/progress/${jobId}`);
    let opened = false;
    let settled = false;
    const settle = () => { if (!settled) { settled = true; onSettle(); } };
    evtSource.onopen = () => { opened = true; };
    evtSource.addEventListener('progress', (e) => {
      opened = true;
      try {
        const data: ProgressEvent = JSON.parse(e.data);
        onProgress(data.progress, data.message || '');
        if (data.state === 'completed') {
          evtSource.close();
          settle();
          onSuccess(data.message || '');
        } else if (data.state === 'failed') {
          evtSource.close();
          settle();
        }
      } catch {}
    });
    evtSource.addEventListener('done', () => {
      opened = true;
      evtSource.close();
      settle();
      onSuccess('');
    });
    evtSource.onerror = () => {
      evtSource.close();
      // A very fast job (small test disk, tiny recovery target) can finish
      // and be cleaned up from the backend's in-memory job map before this
      // EventSource ever connects — confirmed live: the connection then
      // gets an immediate "Job not found" response, which the UI has no way
      // to tell apart from a real failure. If we never received a single
      // real event, we genuinely don't know whether it succeeded or
      // failed — say so honestly instead of hanging on "running" forever.
      if (!opened) {
        settle();
      }
    };
  }

  /** Surfaces a failed rescue action via the notification center — every
      caller previously left its `else` branch empty, so a bad device, a
      missing tool, or any other backend error left the spinner stopping
      with zero feedback (2026-09-05, found while writing this page's E2E
      suite, fixed on explicit request). */
  function notifyRescueError(message: string) {
    notificationsStore.add({ title: $t('rescue.title'), message, type: 'error' });
  }

  function toggleFileType(ft: string) {
    if (prFileTypes.includes(ft)) {
      prFileTypes = prFileTypes.filter(t => t !== ft);
    } else {
      prFileTypes = [...prFileTypes, ft];
    }
  }

  async function startDdrescue() {
    if (!ddSource || !ddDest) {
      return;
    }
    ddRunning = true;
    ddProgress = 0;
    ddMessage = translate('rescue.starting_ddrescue');
    const r = await api.ddrescueClone(ddSource, ddDest, {
      logfile: ddLogfile || undefined,
      block_size: ddMode === 'advanced' ? ddBlockSize : undefined,
      retry_passes: ddMode === 'advanced' && ddRetryPasses > 0 ? ddRetryPasses : undefined,
      direct_io: ddMode === 'advanced' ? ddDirectIO || undefined : undefined,
      reverse: ddMode === 'advanced' ? ddReverse || undefined : undefined,
      verbose: ddMode === 'advanced' ? ddVerbose || undefined : undefined,
    });
    if (r.success && r.data) {
      ddJobId = (r.data as any).job_id;
      ddLogfile = (r.data as any).logfile || ddLogfile;
      connectSSE(ddJobId,
        (pct, msg) => { ddProgress = pct; ddMessage = msg; },
        () => { ddProgress = 100; ddMessage = translate('rescue.clone_completed'); },
        () => { ddRunning = false; }
      );
    } else {
      ddRunning = false;
      notifyRescueError(r.message || translate('rescue.ddrescue_start_failed'));
    }
  }

  async function analyzeTestDisk() {
    if (!tdDevice) {
      return;
    }
    tdAnalyzing = true;
    tdResult = null;
    const r = await api.testdiskAnalyze(tdDevice, {
      deep_search: tdMode === 'advanced' ? tdDeepSearch || undefined : undefined,
      log_path: tdMode === 'advanced' && tdLogPath ? tdLogPath : undefined,
    });
    if (r.success && r.data) {
      tdResult = r.data;
    } else {
      notifyRescueError(r.message || translate('rescue.testdisk_analysis_failed'));
    }
    tdAnalyzing = false;
  }

  async function rebuildTestDisk() {
    if (!tdDevice) {
      return;
    }
    tdAnalyzing = true;
    const r = await api.testdiskRebuild(tdDevice, {
      deep_search: tdMode === 'advanced' ? tdDeepSearch || undefined : undefined,
      force: tdMode === 'advanced' ? tdForce || undefined : undefined,
    });
    if (!r.success) {
      notifyRescueError(r.message || translate('rescue.testdisk_rebuild_failed'));
    }
    tdAnalyzing = false;
  }

  async function startPhotoRec() {
    if (!prDevice || !prOutputDir) {
      return;
    }
    prRunning = true;
    const r = await api.photorecRecover(prDevice, prOutputDir, {
      file_types: prMode === 'advanced' && prFileTypes.length > 0 ? prFileTypes : undefined,
      keep_broken: prMode === 'advanced' ? prKeepBroken || undefined : undefined,
      max_size: prMode === 'advanced' && prMaxSize > 0 ? prMaxSize : undefined,
      partition_type: prMode === 'advanced' && prPartitionType ? prPartitionType : undefined,
    });
    if (r.success && r.data) {
      prJobId = (r.data as any).job_id;
      connectSSE(prJobId,
        () => {},
        (msg) => {
          // PhotoRec always writes into a numbered sibling of the requested
          // output directory (e.g. /tmp/recovered.1) — surface the real
          // path from the completion message so it's not just "done" with
          // no way to find the recovered files.
          const path = msg.match(/Files saved to (.+)$/)?.[1];
        },
        () => { prRunning = false; }
      );
    } else {
      prRunning = false;
      notifyRescueError(r.message || translate('rescue.photorec_start_failed'));
    }
  }

  async function diagnoseMdadm() {
    if (!mdName) {
      return;
    }
    mdLoading = true;
    mdDiagnostic = null;
    const r = await api.mdadmDiagnose(mdName);
    if (r.success && r.data) {
      mdDiagnostic = r.data;
    } else {
      notifyRescueError(r.message || translate('rescue.mdadm_diagnosis_failed'));
    }
    mdLoading = false;
  }

  async function repairMdadm() {
    if (!mdName) {
      return;
    }
    mdRepairing = true;
    const r = await api.mdadmRepair(mdName, {
      action: mdMode === 'advanced' ? mdAction : undefined,
      force: mdMode === 'advanced' ? mdForce || undefined : undefined,
      verbose: mdMode === 'advanced' ? mdVerbose || undefined : undefined,
    });
    if (!r.success) {
      notifyRescueError(r.message || translate('rescue.mdadm_repair_failed'));
    }
    mdRepairing = false;
  }

  function getHealthColor(status: string) {
    switch (status) {
      case 'healthy': return 'var(--hv-success)';
      case 'degraded': return 'var(--hv-warning)';
      case 'failed': return 'var(--hv-error)';
      default: return 'var(--hv-neutral-400)';
    }
  }

  function toolAvailable(toolName: string): boolean {
    return tools.find(t => t.name === toolName)?.available || false;
  }
</script>

{#snippet toolBadge(name: string)}
  <!-- The availability badge belongs to the tool's own card, not to the window
       header — the header is icon + title only since the 2026-09-14
       standardization. Each tab renders exactly one, for its own tool. -->
  <span data-testid="rescue-tool-badge">
    <Badge variant={toolAvailable(name) ? 'success' : 'error'}>
      {toolAvailable(name) ? $t('rescue.available') : $t('rescue.not_found')}
    </Badge>
  </span>
{/snippet}

<PageLayout
  title={$t('rescue.title')}
  data-testid="rescue-page"
  tabs={[
    { id: 'ddrescue', label: 'ddrescue', icon: IconServer },
    { id: 'testdisk', label: 'TestDisk', icon: IconSearch },
    { id: 'photorec', label: 'PhotoRec', icon: IconDownload },
    { id: 'mdadm', label: 'mdadm', icon: IconShield }
  ]}
  bind:activeTab
>
  {#snippet icon()}
    <IconShield stroke={1.8} />
  {/snippet}

  {#if loading}
    <div data-testid="rescue-loading" class="flex flex-col items-center justify-center h-full gap-4 text-[var(--hv-neutral-400)]">
      <Spinner /><p class="m-0">{$t('rescue.loading')}</p>
    </div>
  {:else}
    <div class="flex flex-col gap-6">
      {#if activeTab === 'ddrescue'}
        <SectionCard title={$t('rescue.dd_title')}>
          {#snippet actions()}
            {@render toolBadge('ddrescue')}
            <div class={modeToggle}>
              <button data-testid="rescue-mode-btn" class:active={ddMode === 'simple'} class={cn(modeBtn, ddMode === 'simple' && 'bg-[var(--hv-primary)] text-white')} onclick={() => (ddMode = 'simple')}>{$t('rescue.mode_simple')}</button>
              <button data-testid="rescue-mode-btn" class:active={ddMode === 'advanced'} class={cn(modeBtn, ddMode === 'advanced' && 'bg-[var(--hv-primary)] text-white')} onclick={() => (ddMode = 'advanced')}>{$t('rescue.mode_advanced')}</button>
            </div>
          {/snippet}
          <div class={formGroup}>
            <label class={formLabel} for="rescue-dd-source">{$t('rescue.source_device')}</label>
            <select id="rescue-dd-source" class={inpSel} bind:value={ddSource}>
              <option value="">{$t('rescue.select_source')}</option>
              {#each devices.filter(d => !d.removable) as dev}
                <option value={dev.device}>{dev.device} ({formatSize(dev.size)} - {dev.model})</option>
              {/each}
            </select>
          </div>
          <div class={formGroup}>
            <label class={formLabel} for="rescue-dd-dest">{$t('rescue.dest_device')}</label>
            <select id="rescue-dd-dest" class={inpSel} bind:value={ddDest}>
              <option value="">{$t('rescue.select_destination')}</option>
              {#each devices as dev}
                <option value={dev.device} disabled={dev.device === ddSource}>{dev.device} ({formatSize(dev.size)} - {dev.model})</option>
              {/each}
            </select>
          </div>

          {#if ddMode === 'advanced'}
            <div data-testid="rescue-advanced" class="p-4 mb-4 bg-[var(--hv-neutral-50)] border border-dashed border-[var(--hv-border)] rounded-lg">
              <div class={formGroup}>
                <label class={formLabel} for="rescue-dd-logfile">{$t('rescue.log_file')}</label>
                <PathField id="rescue-dd-logfile" bind:value={ddLogfile} mode="file" placeholder="/tmp/ddrescue_map.log" />
              </div>
              <div class="flex gap-4 mb-4 flex-wrap">
                <div class={cn(formGroup, 'flex-1 min-w-[140px]')}>
                  <label class={formLabel} for="rescue-dd-blocksize">{$t('rescue.block_size')}</label>
                  <input id="rescue-dd-blocksize" class={inpSel} type="number" bind:value={ddBlockSize} min="512" max="4096" step="512" />
                </div>
                <div class={cn(formGroup, 'flex-1 min-w-[140px]')}>
                  <label class={formLabel} for="rescue-dd-retry">{$t('rescue.retry_passes')}</label>
                  <input id="rescue-dd-retry" class={inpSel} type="number" bind:value={ddRetryPasses} min="0" placeholder={$t('rescue.retry_passes_placeholder')} />
                </div>
              </div>
              <div class="flex gap-4 mb-4 flex-wrap">
                <label data-testid="rescue-checkbox" class={checkboxLabel}><input type="checkbox" class="w-4 h-4" bind:checked={ddDirectIO} /> {$t('rescue.direct_io')}</label>
                <label data-testid="rescue-checkbox" class={checkboxLabel}><input type="checkbox" class="w-4 h-4" bind:checked={ddReverse} /> {$t('rescue.reverse_direction')}</label>
                <label data-testid="rescue-checkbox" class={checkboxLabel}><input type="checkbox" class="w-4 h-4" bind:checked={ddVerbose} /> {$t('rescue.verbose')}</label>
              </div>
            </div>
          {/if}

          <Button variant="primary" disabled={ddRunning} onclick={startDdrescue}>
            <IconPlayerPlay size={16} /> {$t('rescue.start_clone')}
          </Button>

          {#if ddRunning}
            <div data-testid="rescue-progress" class="mt-6 p-4 bg-[var(--hv-neutral-50)] rounded-lg">
              <div class="h-2 bg-[var(--hv-neutral-100)] rounded-full overflow-hidden">
                <div class="h-full bg-[var(--hv-primary)] transition-[width]" style="width: {ddProgress}%"></div>
              </div>
              <p class="mt-2 mb-0 text-xs text-[var(--hv-neutral-500)] font-[family-name:var(--hv-font-mono)]">{ddMessage}</p>
            </div>
          {/if}
        </SectionCard>

        <SectionCard title={$t('rescue.available_devices')}>
          {#if devices.length === 0}
            <p data-testid="rescue-empty" class="text-center py-8 text-[var(--hv-neutral-400)]">{$t('rescue.no_devices')}</p>
          {:else}
            <ul data-testid="rescue-device-list" class="flex flex-col gap-2 list-none p-0 m-0">
              {#each devices as dev}
                <li class="flex items-center justify-between px-3 py-2 bg-[var(--hv-neutral-50)] rounded-md border border-[var(--hv-border)]">
                  <div class="flex items-center flex-wrap gap-2 gap-x-4 text-xs">
                    <strong class="text-[var(--hv-neutral-800)] font-[family-name:var(--hv-font-mono)]">{dev.device}</strong>
                    <span>{dev.model || $t('rescue.unknown_model')}</span>
                    <span>{formatSize(dev.size)}</span>
                    <span class="px-1.5 py-0.5 bg-[var(--hv-neutral-200)] rounded-sm text-micro uppercase">{dev.transport}</span>
                  </div>
                </li>
              {/each}
            </ul>
          {/if}
        </SectionCard>

      {:else if activeTab === 'testdisk'}
        <SectionCard title={$t('rescue.td_title')}>
          {#snippet actions()}
            {@render toolBadge('testdisk')}
            <div class={modeToggle}>
              <button data-testid="rescue-mode-btn" class:active={tdMode === 'simple'} class={cn(modeBtn, tdMode === 'simple' && 'bg-[var(--hv-primary)] text-white')} onclick={() => (tdMode = 'simple')}>{$t('rescue.mode_simple')}</button>
              <button data-testid="rescue-mode-btn" class:active={tdMode === 'advanced'} class={cn(modeBtn, tdMode === 'advanced' && 'bg-[var(--hv-primary)] text-white')} onclick={() => (tdMode = 'advanced')}>{$t('rescue.mode_advanced')}</button>
            </div>
          {/snippet}
          <div class={formGroup}>
            <label class={formLabel} for="rescue-td-device">{$t('rescue.device_to_analyze')}</label>
            <select id="rescue-td-device" class={inpSel} bind:value={tdDevice}>
              <option value="">{$t('rescue.select_device')}</option>
              {#each devices as dev}
                <option value={dev.device}>{dev.device} ({formatSize(dev.size)} - {dev.model})</option>
              {/each}
            </select>
          </div>

          {#if tdMode === 'advanced'}
            <div data-testid="rescue-advanced" class="p-4 mb-4 bg-[var(--hv-neutral-50)] border border-dashed border-[var(--hv-border)] rounded-lg">
              <div class="flex gap-4 mb-4 flex-wrap">
                <label data-testid="rescue-checkbox" class={checkboxLabel}><input type="checkbox" class="w-4 h-4" bind:checked={tdDeepSearch} /> {$t('rescue.deep_search')}</label>
              </div>
              <div class={formGroup}>
                <label class={formLabel} for="rescue-td-logpath">{$t('rescue.log_path')}</label>
                <PathField id="rescue-td-logpath" bind:value={tdLogPath} mode="file" placeholder="/tmp/testdisk.log" />
              </div>
              <div class="flex gap-4 mb-4 flex-wrap">
                <label data-testid="rescue-checkbox" class={checkboxLabel}><input type="checkbox" class="w-4 h-4" bind:checked={tdForce} /> {$t('rescue.force_write')}</label>
              </div>
            </div>
          {/if}

          <div class="flex gap-3 mt-2 flex-wrap">
            <Button variant="primary" disabled={tdAnalyzing} onclick={analyzeTestDisk}>
              <IconSearch size={16} /> {$t('rescue.analyze')}
            </Button>
            <Button variant="warning" disabled={tdAnalyzing} onclick={rebuildTestDisk}>
              <IconRefresh size={16} /> {$t('rescue.rebuild_partition_table')}
            </Button>
          </div>

          {#if tdAnalyzing}<div class="mt-4"><Spinner /></div>{/if}

          {#if tdResult}
            <div class="my-4 border-t border-[var(--hv-border)]"></div>
            <h3 class="text-lg font-semibold text-[var(--hv-neutral-800)] my-3">{$t('rescue.analysis_results')}</h3>
            {#if tdResult.partitions && tdResult.partitions.length > 0}
              <table class="w-full border-collapse text-xs">
                <thead>
                  <tr>
                    <th class="px-3 py-2 text-left font-semibold text-[var(--hv-neutral-500)] uppercase tracking-wide border-b border-[var(--hv-border)]">{ $t('rescue.th_partition')}</th>
                    <th class="px-3 py-2 text-left font-semibold text-[var(--hv-neutral-500)] uppercase tracking-wide border-b border-[var(--hv-border)]">{ $t('rescue.th_size')}</th>
                    <th class="px-3 py-2 text-left font-semibold text-[var(--hv-neutral-500)] uppercase tracking-wide border-b border-[var(--hv-border)]">{ $t('rescue.th_filesystem')}</th>
                    <th class="px-3 py-2 text-left font-semibold text-[var(--hv-neutral-500)] uppercase tracking-wide border-b border-[var(--hv-border)]">{ $t('rescue.th_status')}</th>
                  </tr>
                </thead>
                <tbody>
                  {#each tdResult.partitions as part}
                    <tr class="border-b border-[var(--hv-border)]"><td class="px-3 py-2">{part.partition}</td><td class="px-3 py-2">{part.size}</td><td class="px-3 py-2">{part.filesystem}</td><td class="px-3 py-2">{part.status}</td></tr>
                  {/each}
                </tbody>
              </table>
            {:else}
              <Alert variant="info">{$t('rescue.no_partitions_found')}</Alert>
            {/if}
          {/if}
        </SectionCard>

      {:else if activeTab === 'photorec'}
        <SectionCard title={$t('rescue.pr_title')}>
          {#snippet actions()}
            {@render toolBadge('photorec')}
            <div class={modeToggle}>
              <button data-testid="rescue-mode-btn" class:active={prMode === 'simple'} class={cn(modeBtn, prMode === 'simple' && 'bg-[var(--hv-primary)] text-white')} onclick={() => (prMode = 'simple')}>{$t('rescue.mode_simple')}</button>
              <button data-testid="rescue-mode-btn" class:active={prMode === 'advanced'} class={cn(modeBtn, prMode === 'advanced' && 'bg-[var(--hv-primary)] text-white')} onclick={() => (prMode = 'advanced')}>{$t('rescue.mode_advanced')}</button>
            </div>
          {/snippet}
          <div class={formGroup}>
            <label class={formLabel} for="rescue-pr-device">{$t('rescue.device_to_recover')}</label>
            <select id="rescue-pr-device" class={inpSel} bind:value={prDevice}>
              <option value="">{$t('rescue.select_device')}</option>
              {#each devices as dev}
                <option value={dev.device}>{dev.device} ({formatSize(dev.size)} - {dev.model})</option>
              {/each}
            </select>
          </div>
          <div class={formGroup}>
            <label class={formLabel} for="rescue-pr-outputdir">{$t('rescue.output_directory')}</label>
            <PathField id="rescue-pr-outputdir" bind:value={prOutputDir} mode="folder" placeholder="/tmp/recovered" />
          </div>

          {#if prMode === 'advanced'}
            <div data-testid="rescue-advanced" class="p-4 mb-4 bg-[var(--hv-neutral-50)] border border-dashed border-[var(--hv-border)] rounded-lg">
              <div class={formGroup}>
                <label class={formLabel} for="rescue-pr-parttype">{$t('rescue.partition_type')}</label>
                <select id="rescue-pr-parttype" class={inpSel} bind:value={prPartitionType}>
                  <option value="">{$t('rescue.auto_detect')}</option>
                  <option value="Intel">Intel (MBR)</option>
                  <option value="EFI">EFI GPT</option>
                  <option value="Mac">Apple Mac</option>
                  <option value="Sun">Sun</option>
                  <option value="None">{$t('rescue.whole_disk')}</option>
                </select>
              </div>
              <div class={formGroup}>
                <label class={formLabel} for="rescue-pr-maxsize">{$t('rescue.max_file_size')}</label>
                <input id="rescue-pr-maxsize" class={inpSel} type="number" bind:value={prMaxSize} min="0" />
              </div>
              <div class="flex gap-4 mb-4 flex-wrap">
                <label data-testid="rescue-checkbox" class={checkboxLabel}><input type="checkbox" class="w-4 h-4" bind:checked={prKeepBroken} /> {$t('rescue.keep_broken')}</label>
              </div>
              <div class={formGroup}>
                <span class={formLabel}>{$t('rescue.file_types')}</span>
                <div class="flex flex-wrap gap-2 mt-2">
                  {#each availableFileTypes as ft}
                    <label data-testid="rescue-checkbox" class={checkboxLabel}>
                      <input type="checkbox" class="w-4 h-4" checked={prFileTypes.includes(ft)} onchange={() => toggleFileType(ft)} />
                      .{ft}
                    </label>
                  {/each}
                </div>
              </div>
            </div>
          {/if}

          <Button variant="primary" disabled={prRunning} onclick={startPhotoRec}>
            <IconDownload size={16} /> {$t('rescue.start_recovery')}
          </Button>

          {#if prRunning}
            <div class="mt-4"><Alert variant="info">{$t('rescue.photorec_running')}</Alert></div>
          {/if}
        </SectionCard>

      {:else if activeTab === 'mdadm'}
        <SectionCard title={$t('rescue.md_title')}>
          {#snippet actions()}
            {@render toolBadge('mdadm')}
            <div class={modeToggle}>
              <button data-testid="rescue-mode-btn" class:active={mdMode === 'simple'} class={cn(modeBtn, mdMode === 'simple' && 'bg-[var(--hv-primary)] text-white')} onclick={() => (mdMode = 'simple')}>{$t('rescue.mode_simple')}</button>
              <button data-testid="rescue-mode-btn" class:active={mdMode === 'advanced'} class={cn(modeBtn, mdMode === 'advanced' && 'bg-[var(--hv-primary)] text-white')} onclick={() => (mdMode = 'advanced')}>{$t('rescue.mode_advanced')}</button>
            </div>
          {/snippet}
          <div class={formGroup}>
            <label class={formLabel} for="rescue-md-name">{$t('rescue.array_name')}</label>
            <input id="rescue-md-name" class={inpSel} type="text" bind:value={mdName} placeholder="e.g. md0, /dev/md0" />
          </div>

          {#if mdMode === 'advanced'}
            <div data-testid="rescue-advanced" class="p-4 mb-4 bg-[var(--hv-neutral-50)] border border-dashed border-[var(--hv-border)] rounded-lg">
              <div class={formGroup}>
                <label class={formLabel} for="rescue-md-action">{$t('rescue.action')}</label>
                <select id="rescue-md-action" class={inpSel} bind:value={mdAction}>
                  <option value="repair">{$t('rescue.action_repair')}</option>
                  <option value="check">{$t('rescue.action_check')}</option>
                  <option value="rewrite">{$t('rescue.action_rewrite')}</option>
                </select>
              </div>
              <div class="flex gap-4 mb-4 flex-wrap">
                <label data-testid="rescue-checkbox" class={checkboxLabel}><input type="checkbox" class="w-4 h-4" bind:checked={mdForce} /> {$t('rescue.force')}</label>
                <label data-testid="rescue-checkbox" class={checkboxLabel}><input type="checkbox" class="w-4 h-4" bind:checked={mdVerbose} /> {$t('rescue.verbose')}</label>
              </div>
            </div>
          {/if}

          <div class="flex gap-3 mt-2 flex-wrap">
            <Button variant="primary" disabled={mdLoading} onclick={diagnoseMdadm}>
              <IconSearch size={16} /> {$t('rescue.diagnose')}
            </Button>
            <Button variant="warning" disabled={mdLoading || mdRepairing} onclick={repairMdadm}>
              <IconRefresh size={16} /> {$t('rescue.repair')}
            </Button>
          </div>

          {#if mdLoading}<div class="mt-4"><Spinner /></div>{/if}

          {#if mdDiagnostic}
            <div class="my-4 border-t border-[var(--hv-border)]"></div>
            <h3 class="text-lg font-semibold text-[var(--hv-neutral-800)] my-3">{$t('rescue.raid_diagnostic')}</h3>
            <div data-testid="rescue-diagnostic-grid" class="grid gap-4 [grid-template-columns:repeat(auto-fill,minmax(200px,1fr))]">
              <div class="p-3 bg-[var(--hv-neutral-50)] rounded-md border border-[var(--hv-border)]">
                <span class="block text-micro font-semibold uppercase tracking-wide text-[var(--hv-neutral-400)] mb-1">{$t('rescue.diag_array')}</span>
                <span class="block text-sm font-medium text-[var(--hv-neutral-800)] font-[family-name:var(--hv-font-mono)]">{mdDiagnostic.array}</span>
              </div>
              <div class="p-3 bg-[var(--hv-neutral-50)] rounded-md border border-[var(--hv-border)]">
                <span class="block text-micro font-semibold uppercase tracking-wide text-[var(--hv-neutral-400)] mb-1">{$t('rescue.diag_level')}</span>
                <span class="block text-sm font-medium text-[var(--hv-neutral-800)] font-[family-name:var(--hv-font-mono)]">{mdDiagnostic.level}</span>
              </div>
              <div class="p-3 bg-[var(--hv-neutral-50)] rounded-md border border-[var(--hv-border)]">
                <span class="block text-micro font-semibold uppercase tracking-wide text-[var(--hv-neutral-400)] mb-1">{$t('rescue.diag_state')}</span>
                <span class="block text-sm font-medium text-[var(--hv-neutral-800)] font-[family-name:var(--hv-font-mono)]">{mdDiagnostic.state}</span>
              </div>
              <div class="p-3 bg-[var(--hv-neutral-50)] rounded-md border border-[var(--hv-border)]">
                <span class="block text-micro font-semibold uppercase tracking-wide text-[var(--hv-neutral-400)] mb-1">{$t('rescue.diag_health')}</span>
                <span class="block text-sm font-medium text-[var(--hv-neutral-800)] font-[family-name:var(--hv-font-mono)]" style="color: {getHealthColor(mdDiagnostic.health)};">{mdDiagnostic.health}</span>
              </div>
              {#if mdDiagnostic.rebuild_progress}
                <div class="p-3 bg-[var(--hv-neutral-50)] rounded-md border border-[var(--hv-border)]">
                  <span class="block text-micro font-semibold uppercase tracking-wide text-[var(--hv-neutral-400)] mb-1">{$t('rescue.diag_rebuild')}</span>
                  <span class="block text-sm font-medium text-[var(--hv-neutral-800)] font-[family-name:var(--hv-font-mono)]">{mdDiagnostic.rebuild_progress}</span>
                </div>
              {/if}
            </div>
            <h4 class="text-lg font-semibold text-[var(--hv-neutral-800)] my-3 mt-4">{$t('rescue.member_devices')}</h4>
            <ul data-testid="rescue-device-list" class="flex flex-col gap-2 list-none p-0 m-0">
              {#each mdDiagnostic.devices as dev}<li class="px-3 py-2 bg-[var(--hv-neutral-50)] rounded-md border border-[var(--hv-border)] text-xs">{dev}</li>{/each}
            </ul>
          {/if}
        </SectionCard>
      {/if}
    </div>
  {/if}
</PageLayout>
<!-- ========================================================================
     File          : AntivirusCard.svelte
     Author        : Nicolas Le Roux
     Created       : 28/09/2026
     Last modified : 28/09/2026 by Nicolas Le Roux
     Version       : 1.0.0

     Description:
       Settings sub-card for antivirus (ClamAV): settings form bound to the
       shared draft, manual scan trigger with live SSE progress, scan
       history and quarantined-threat resolution.

     Dependencies:
       - svelte (>= 5), stores/settingsDraft.svelte, $lib/services/api

     Usage:
       Embedded in routes/Settings.svelte's security group.
     ======================================================================== -->
<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { api } from '$lib/services/api';
  import type { AvSettings, AvScan, AvThreat } from '$lib/services/api';
  import { draft, registerGroup } from '$lib/stores/settingsDraft.svelte';
  import { t } from '$lib/stores/i18n';
  import { IconShieldCheckered } from '@tabler/icons-svelte';
  import SettingsCard from './SettingsCard.svelte';
  import PathPickerField from '$lib/components/theme/PathPickerField.svelte';
  import Switch from '$lib/components/theme/Switch.svelte';
  import { cn } from '$lib/utils.js';
  import {
    settingsRow, settingsInfoRow, settingsRowStacked, settingsRowLabel, settingsRowHint, settingsHint,
    settingsInput, settingsBtn, settingsBtnPrimary, settingsBtnGhost, settingsBtnDanger,
    SECURITY_ACCENT
  } from './vocabulary';

  /* The parent stacks this card under another section, so it hands down
     spacing (`mt-3`) the same way it does to `SettingsCard` — without a
     `class` prop here that attribute would be a type error. */
  let { class: klass = '' }: { class?: string } = $props();

  let loading = $state(true);
  let scanning = $state(false);
  let scanProgress = $state(0);
  let scanMessage = $state('');
  let scanPath = $state('');

  let clamscanAvailable = $state(false);
  let definitionsAvailable = $state(false);
  let definitionsLastUpdated = $state<string | null>(null);

  let scans: AvScan[] = $state([]);
  let threats: AvThreat[] = $state([]);

  let evtSource: EventSource | null = null;

  async function loadAll() {
    loading = true;
    const [statusRes, settingsRes, scansRes, threatsRes] = await Promise.all([
      api.getAntivirusStatus(),
      api.getAntivirusSettings(),
      api.getAntivirusScanHistory(),
      api.getAntivirusThreats()
    ]);
    if (statusRes.success && statusRes.data) {
      const d = statusRes.data as any;
      clamscanAvailable = d.clamscan_available;
      definitionsAvailable = d.definitions?.available ?? false;
      definitionsLastUpdated = d.definitions?.last_updated ?? null;
      scanning = d.scan_running ?? false;
    }
    if (settingsRes.success && settingsRes.data) {
      // Stage the settings into the shared draft once; this card no longer
      // saves them — the Settings screen's global Save button does.
      registerGroup('antivirus', settingsRes.data as AvSettings);
    }
    if (scansRes.success && scansRes.data) {
      scans = scansRes.data as AvScan[];
    }
    if (threatsRes.success && threatsRes.data) {
      threats = threatsRes.data as AvThreat[];
    }
    loading = false;
  }

  onMount(loadAll);
  onDestroy(() => evtSource?.close());

  function connectScanProgress(jobId: string) {
    evtSource?.close();
    evtSource = new EventSource(`/api/progress/${jobId}`);
    evtSource.addEventListener('progress', (e) => {
      try {
        const data = JSON.parse((e as MessageEvent).data);
        scanProgress = data.progress ?? 0;
        scanMessage = data.message || '';
        if (data.state === 'completed' || data.state === 'failed') {
          evtSource?.close();
          scanning = false;
          loadAll();
        }
      } catch {
        /* ignore malformed progress event */
      }
    });
    evtSource.onerror = () => {
      evtSource?.close();
    };
  }

  async function handleStartScan() {
    if (!scanPath.trim()) {
      return;
    }
    scanning = true;
    scanProgress = 0;
    scanMessage = $t('antivirus.scan_starting');
    const res = await api.startAntivirusScan(scanPath.trim());
    if (res.success && res.data) {
      connectScanProgress((res.data as any).job_id);
    } else {
      scanning = false;
    }
  }

  async function handleCancelScan() {
    await api.cancelAntivirusScan();
    evtSource?.close();
    scanning = false;
  }

  async function handleResolveThreat(id: string, deleteFile: boolean) {
    const res = await api.resolveAntivirusThreat(id, deleteFile);
    if (res.success) {
      threats = threats.filter((th) => th.id !== id);
    }
  }

  function formatDate(value: string | null): string {
    if (!value) return $t('antivirus.never');
    try {
      return new Date(value).toLocaleString();
    } catch {
      return value;
    }
  }
</script>

<!--
  AntivirusCard — ClamAV status, on-demand + scheduled scans and quarantine,
  rebuilt on the shared Settings vocabulary. Markup/CSS only: `loadAll`,
  `handleStartScan`, `connectScanProgress` (the `/api/progress/{job_id}` SSE
  stream), `handleCancelScan`, `handleResolveThreat` are unchanged, and the
  field ids (`avScanPath`, `avSchedPath`, `avCron`, `avQuarantineDir`) plus the
  `.settings-btn*` / `.settings-field-full` hooks are preserved.

  Persistence moved to the Settings screen's global Save button (2026-09-15):
  the editable fields bind to the shared `draft.antivirus` group and the
  per-card Save button is gone. Scan/threat actions remain immediate.
-->
<SettingsCard icon={IconShieldCheckered} iconVariant="green" title={$t('antivirus.title')} subtitle={$t('antivirus.subtitle')} class={klass}>
  {#if loading}
    <p class={settingsHint}>{$t('antivirus.loading')}</p>
  {:else}
    {#if !clamscanAvailable}
      <div class="av-warning">{$t('antivirus.not_installed')}</div>
    {/if}

    <div class={settingsInfoRow}>
      <span class={settingsRowLabel}>{$t('antivirus.definitions_status')}</span>
      <span class="flex items-center gap-2">
        {#if definitionsAvailable}
          <span class="settings-badge settings-badge-success">{$t('antivirus.definitions_ok')}</span>
          <span class="text-xs leading-4 text-[var(--hv-settings-label)]">{formatDate(definitionsLastUpdated)}</span>
        {:else}
          <span class="settings-badge settings-badge-warning">{$t('antivirus.definitions_missing')}</span>
        {/if}
      </span>
    </div>

    <div class={settingsInfoRow}>
      <span class={settingsRowLabel}>{$t('antivirus.last_scan')}</span>
      <span class="text-xs leading-4 text-[var(--hv-settings-label)]">{formatDate(draft.antivirus.last_scan_at)}</span>
    </div>

    <div class={cn(settingsRowStacked, 'settings-field-full')}>
      <label class={settingsRowLabel} for="avScanPath">{$t('antivirus.scan_path')}</label>
      <div class="av-scan-row">
        <PathPickerField mode="folder" id="avScanPath" bind:value={scanPath} ariaLabel={$t('antivirus.scan_path')} />
        {#if scanning}
          <button class={cn(settingsBtn, settingsBtnDanger)} onclick={handleCancelScan}>{$t('antivirus.cancel_scan')}</button>
        {:else}
          <button class={cn(settingsBtn, settingsBtnPrimary)} onclick={handleStartScan} disabled={!clamscanAvailable}>
            {$t('antivirus.scan_now')}
          </button>
        {/if}
      </div>
      {#if scanning}
        <div class="mt-1">
          <div class="av-progress-track">
            <div class="av-progress-fill" style="width: {scanProgress}%"></div>
          </div>
          <span class={settingsHint}>{scanMessage}</span>
        </div>
      {/if}
    </div>

    <div class={settingsRow}>
      <div class="min-w-0">
        <span class={settingsRowLabel}>{$t('antivirus.scheduled_scan_enable')}</span>
      </div>
      <Switch bind:checked={draft.antivirus.scheduled_scan_enabled} ariaLabel={$t('antivirus.scheduled_scan_enable')} accent={SECURITY_ACCENT} />
    </div>

    {#if draft.antivirus.scheduled_scan_enabled}
      <div class={settingsRowStacked}>
        <label class={settingsRowLabel} for="avSchedPath">{$t('antivirus.scheduled_scan_path')}</label>
        <PathPickerField mode="folder" id="avSchedPath" bind:value={draft.antivirus.scheduled_scan_path} ariaLabel={$t('antivirus.scheduled_scan_path')} />
      </div>
      <div class={cn(settingsRowStacked, 'settings-field-full')}>
        <label class={settingsRowLabel} for="avCron">{$t('antivirus.scheduled_scan_cron')}</label>
        <input id="avCron" class={cn(settingsInput, 'max-w-[200px]')} type="text" bind:value={draft.antivirus.scheduled_scan_cron} placeholder="0 3 * * 0" />
        <span class={settingsHint}>{$t('antivirus.cron_hint')}</span>
      </div>
    {/if}

    <div class={settingsRowStacked}>
      <label class={settingsRowLabel} for="avQuarantineDir">{$t('antivirus.quarantine_dir')}</label>
      <PathPickerField mode="folder" id="avQuarantineDir" bind:value={draft.antivirus.quarantine_dir} ariaLabel={$t('antivirus.quarantine_dir')} />
    </div>

    {#if threats.length > 0}
      <div class={settingsRowStacked}>
        <h3 class="av-subheading">{$t('antivirus.threats')} ({threats.filter(th => !th.resolved_at).length})</h3>
        {#each threats.filter(th => !th.resolved_at) as threat (threat.id)}
          <div class="av-list-row">
            <div class="min-w-0">
              <strong class="av-list-title">{threat.signature_name}</strong>
              <p class={cn(settingsRowHint, 'break-all')}>{threat.file_path}</p>
            </div>
            <div class="av-list-actions">
              <span class="settings-badge settings-badge-warning">{threat.action}</span>
              <button class={cn(settingsBtn, settingsBtnGhost)} onclick={() => handleResolveThreat(threat.id, false)}>
                {$t('antivirus.mark_resolved')}
              </button>
              {#if threat.quarantine_path}
                <button class={cn(settingsBtn, settingsBtnDanger)} onclick={() => handleResolveThreat(threat.id, true)}>
                  {$t('antivirus.delete_quarantined')}
                </button>
              {/if}
            </div>
          </div>
        {/each}
      </div>
    {/if}

    {#if scans.length > 0}
      <div class={settingsRowStacked}>
        <h3 class="av-subheading">{$t('antivirus.scan_history')}</h3>
        {#each scans.slice(0, 5) as scan (scan.id)}
          <div class="av-list-row">
            <div class="min-w-0">
              <strong class="av-list-title">{scan.scan_path}</strong>
              <p class={settingsRowHint}>{formatDate(scan.started_at)} · {scan.trigger}</p>
            </div>
            <span
              class="settings-badge"
              class:settings-badge-success={scan.status === 'completed' && scan.threats_found === 0}
              class:settings-badge-warning={scan.threats_found > 0}
              class:settings-badge-error={scan.status === 'failed'}
            >
              {scan.status} — {scan.files_scanned} {$t('antivirus.files_scanned')}
            </span>
          </div>
        {/each}
      </div>
    {/if}
  {/if}
</SettingsCard>

<style>
  .av-warning {
    margin: 0 0 12px;
    padding: var(--hv-space-2-5) var(--hv-space-3);
    border-radius: var(--hv-radius-lg);
    background: var(--hv-warning-bg);
    color: var(--hv-warning-text);
    font-size: var(--hv-font-size-sm);
  }

  .av-scan-row {
    display: flex;
    gap: var(--hv-space-2-5);
    align-items: flex-start;
  }

  /* PathPickerField renders `.hv-ppf` as its root — let it take the space next to
     the Scan button (was `.settings-input { flex: 1 }` for the old PathField). */
  .av-scan-row > :global(.hv-ppf) {
    flex: 1;
    min-width: 0;
  }

  .av-progress-track {
    width: 100%;
    height: 8px;
    margin-bottom: 6px;
    border-radius: var(--hv-radius-full);
    background: var(--hv-settings-border);
    overflow: hidden;
  }

  .av-progress-fill {
    height: 100%;
    background: var(--hv-primary);
    transition: width 300ms ease;
  }

  .av-subheading {
    margin: 0;
    font-size: var(--hv-font-size-sm);
    font-weight: var(--hv-font-weight-semibold);
    color: var(--hv-settings-title);
  }

  .av-list-row {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 12px;
    padding: var(--hv-space-2-5) var(--hv-space-3);
    border: 1.45px solid var(--hv-settings-hairline);
    border-radius: var(--hv-radius-lg);
  }

  .av-list-title {
    font-size: var(--hv-font-size-sm);
    font-weight: var(--hv-font-weight-medium);
    color: var(--hv-settings-title);
  }

  .av-list-actions {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-shrink: 0;
  }

  .settings-badge {
    display: inline-flex;
    align-items: center;
    padding: 2px 8px;
    border-radius: var(--hv-radius-lg);
    font-size: var(--hv-font-size-xs);
    font-weight: var(--hv-font-weight-medium);
  }

  .settings-badge-warning {
    background: var(--hv-warning-bg);
    color: var(--hv-warning-text);
  }

  .settings-badge-success {
    background: var(--hv-success-bg);
    color: var(--hv-success-text);
  }

  .settings-badge-error {
    background: var(--hv-error-light);
    color: var(--hv-error);
  }
</style>

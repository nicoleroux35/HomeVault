<!-- ========================================================================
     File          : SnmpCard.svelte
     Author        : Nicolas Le Roux
     Created       : 28/09/2026
     Last modified : 28/09/2026 by Nicolas Le Roux
     Version       : 1.0.0

     Description:
       Settings sub-card for SNMP monitoring: settings form bound to the
       shared draft with its own save registration, availability probe and
       live stats preview.

     Dependencies:
       - svelte (>= 5), stores/settingsDraft.svelte, $lib/services/api,
         ./SettingsCard

     Usage:
       Embedded in routes/Settings.svelte's monitoring group.
     ======================================================================== -->
<script lang="ts">
  import { onMount } from 'svelte';
  import { api } from '$lib/services/api';
  import { draft, registerGroup, onGroupSaved } from '$lib/stores/settingsDraft.svelte';
  import { t } from '$lib/stores/i18n';
  import SettingsCard from './SettingsCard.svelte';
  import { cn } from '$lib/utils.js';
  import {
    settingsRow, settingsRowLast, settingsRowStacked, settingsRowLabel, settingsHint,
    settingsInput, settingsSelect, settingsBtn
  } from './vocabulary';

  /* See AntivirusCard: `SettingsCard` accepts a `class` for stacking
     spacing, and this wrapper has to forward it or the parent's `mt-3`
     is a type error. */
  let { class: klass = '' }: { class?: string } = $props();

  let loading = $state(true);
  let available = $state(false);
  let systemInfo: any = $state(null);
  let interfaces: any[] = $state([]);
  let storage: any[] = $state([]);

  async function loadSettings() {
    loading = true;
    const avail = await api.getSnmpAvailability();
    if (avail.success && avail.data) {
      available = !!(avail.data as any).available;
    }
    const res = await api.getSnmpSettings();
    if (res.success && res.data) {
      const d = res.data as any;
      // Stage community/version into the shared draft once; the Settings
      // screen's global Save button persists them.
      registerGroup('snmp', { community: d.community ?? 'public', version: d.version ?? '2c' });
    }
    if (available) {
      await refreshData();
    }
    loading = false;
  }

  async function refreshData() {
    const info = await api.getSnmpInfo({ community: draft.snmp.community });
    if (info.success && info.data) systemInfo = info.data;
    const ifaces = await api.getSnmpInterfaces({ community: draft.snmp.community });
    if (ifaces.success && ifaces.data) interfaces = (ifaces.data as any[]) ?? [];
    const stor = await api.getSnmpStorage({ community: draft.snmp.community });
    if (stor.success && stor.data) storage = (stor.data as any[]) ?? [];
  }

  onMount(() => {
    loadSettings();
    // After the global Save flushes the SNMP group, re-read the live readings
    // with the freshly saved community string.
    onGroupSaved('snmp', () => {
      refreshData();
    });
  });

  function formatBytes(bytes: number): string {
    if (!bytes) return '0 B';
    const units = ['B', 'KB', 'MB', 'GB', 'TB'];
    const i = Math.min(units.length - 1, Math.floor(Math.log(bytes) / Math.log(1024)));
    return `${(bytes / 1024 ** i).toFixed(i === 0 ? 0 : 1)} ${units[i]}`;
  }
</script>

<!--
  SnmpCard — the SNMP agent settings + live readings, rebuilt on the shared
  Settings vocabulary. Markup/CSS only: `loadSettings` / `refreshData` and
  their `api.*` calls are unchanged, and the community / version field ids
  (`snmp-community`, `snmp-version`) are preserved so their labels keep
  resolving. Persistence moved to the Settings screen's global Save button
  (2026-09-15): the fields bind to the shared `draft.snmp` group and the
  per-card Save button is gone; live data re-reads after a successful save.
-->
<SettingsCard title={$t('settings.snmp_monitoring')} subtitle={$t('settings.snmp_description')} class={klass}>
  {#if loading}
    <p class={settingsHint}>{$t('settings.loading')}</p>
  {:else}
    <div class={cn(settingsRowStacked, 'settings-field-full')}>
      <div class="grid grid-cols-[repeat(auto-fit,minmax(220px,1fr))] gap-4">
        <div class="settings-field flex flex-col gap-1.5">
          <label class={settingsRowLabel} for="snmp-community">{$t('settings.snmp_community')}</label>
          <input id="snmp-community" class={settingsInput} bind:value={draft.snmp.community} />
        </div>
        <div class="settings-field flex flex-col gap-1.5">
          <label class={settingsRowLabel} for="snmp-version">{$t('settings.snmp_version')}</label>
          <select id="snmp-version" class={settingsSelect} bind:value={draft.snmp.version}>
            <option value="1">v1</option>
            <option value="2c">v2c</option>
          </select>
          <p class={settingsHint}>{$t('settings.snmp_version_hint')}</p>
        </div>
      </div>
    </div>

    {#if !available}
      <p class={cn(settingsHint, 'py-3')}>{$t('settings.snmp_not_available')}</p>
    {/if}

    {#if available && systemInfo}
      <div class={settingsRowStacked}>
        <h3 class="snmp-section-title m-0 text-sm! font-semibold! leading-5! text-[var(--hv-settings-title)]!">{$t('snmp.system')}</h3>
        <pre class="snmp-pre m-0 overflow-x-auto rounded-lg bg-[var(--hv-settings-hairline)] p-3 text-xs font-[family-name:var(--hv-font-mono)] text-[var(--hv-settings-row)]">{JSON.stringify(systemInfo, null, 2)}</pre>
      </div>
    {/if}

    {#if available && interfaces.length > 0}
      <div class={settingsRowStacked}>
        <h3 class="snmp-section-title m-0 text-sm! font-semibold! leading-5! text-[var(--hv-settings-title)]!">{$t('snmp.interfaces')}</h3>
        {#each interfaces as iface (iface.name ?? iface.iface)}
          <div class={cn(settingsRow, 'snmp-item')}>
            <span class={settingsRowLabel}>{iface.name ?? iface.iface}</span>
            <span class="snmp-value text-xs font-[family-name:var(--hv-font-mono)] text-[var(--hv-settings-label)]">↓ {formatBytes(iface.bytes_in)} / ↑ {formatBytes(iface.bytes_out)}</span>
          </div>
        {/each}
      </div>
    {/if}

    {#if available && storage.length > 0}
      <div class={settingsRowStacked}>
        <h3 class="snmp-section-title m-0 text-sm! font-semibold! leading-5! text-[var(--hv-settings-title)]!">{$t('snmp.storage')}</h3>
        {#each storage as s (s.fs_index)}
          <div class={cn(settingsRow, 'snmp-item')}>
            <span class={settingsRowLabel}>{s.mount_point || s.device || s.fs_index}</span>
            <span class="snmp-value text-xs font-[family-name:var(--hv-font-mono)] text-[var(--hv-settings-label)]">{formatBytes(s.used)} / {formatBytes(s.total)} ({Math.round(s.usage_percent)}%)</span>
          </div>
        {/each}
      </div>
    {/if}
  {/if}
</SettingsCard>

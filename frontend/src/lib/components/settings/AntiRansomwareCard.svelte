<script lang="ts">
  import { onMount } from 'svelte';
  import { api } from '$lib/services/api';
  import type { RansomwareSettings, ProtectedPath, RansomwareEvent } from '$lib/services/api';
  import { draft, registerGroup } from '$lib/stores/settingsDraft.svelte';
  import { t } from '$lib/stores/i18n';
  import { IconShieldExclamation } from '@tabler/icons-svelte';
  import SettingsCard from './SettingsCard.svelte';
  import PathPickerField from '$lib/components/theme/PathPickerField.svelte';
  import Switch from '$lib/components/theme/Switch.svelte';
  import { cn } from '$lib/utils.js';
  import {
    settingsRow, settingsRowStacked, settingsRowLabel, settingsRowHint, settingsHint,
    settingsInput, settingsSelect, settingsBtn, settingsBtnPrimary, settingsBtnDanger,
    SECURITY_ACCENT
  } from './vocabulary';

  /* See AntivirusCard: `SettingsCard` accepts a `class` for stacking
     spacing, and this wrapper has to forward it or the parent's `mt-3`
     is a type error. */
  let { class: klass = '' }: { class?: string } = $props();

  let loading = $state(true);
  let addingPath = $state(false);
  let newPath = $state('');

  let protectedPaths: ProtectedPath[] = $state([]);
  let events: RansomwareEvent[] = $state([]);

  async function loadAll() {
    loading = true;
    const [settingsRes, pathsRes, eventsRes] = await Promise.all([
      api.getRansomwareSettings(),
      api.listProtectedPaths(),
      api.getRansomwareEvents()
    ]);
    if (settingsRes.success && settingsRes.data) {
      // Stage the settings into the shared draft once; the Settings screen's
      // global Save button persists them.
      registerGroup('ransomware', settingsRes.data as RansomwareSettings);
    }
    if (pathsRes.success && pathsRes.data) protectedPaths = pathsRes.data as ProtectedPath[];
    if (eventsRes.success && eventsRes.data) events = eventsRes.data as RansomwareEvent[];
    loading = false;
  }

  onMount(loadAll);

  async function handleAddPath() {
    if (!newPath.trim()) {
      return;
    }
    addingPath = true;
    const res = await api.addProtectedPath({ path: newPath.trim(), place_honeypot: draft.ransomware.honeypots_enabled });
    addingPath = false;
    if (res.success && res.data) {
      protectedPaths = [...protectedPaths, res.data as ProtectedPath];
      newPath = '';
    }
  }

  async function handleRemovePath(id: string) {
    const res = await api.deleteProtectedPath(id);
    if (res.success) {
      protectedPaths = protectedPaths.filter((p) => p.id !== id);
    }
  }

  function formatDate(value: string): string {
    try {
      return new Date(value).toLocaleString();
    } catch {
      return value;
    }
  }

  function eventLabel(eventType: string): string {
    return $t(`ransomware.event_${eventType}`) || eventType;
  }
</script>

<!--
  AntiRansomwareCard — honeypot-based ransomware protection, rebuilt on the
  shared Settings vocabulary. Markup/CSS only: `loadAll`, `handleAddPath` /
  `handleRemovePath` and their `api.*` calls are unchanged.

  Persistence moved to the Settings screen's global Save button (2026-09-15):
  the editable fields bind to the shared `draft.ransomware` group and the
  per-card Save button is gone. `draft.ransomware.honeypots_enabled` is still
  what `handleAddPath` sends as `place_honeypot`, and the honeypot debounce
  lives server-side (`honeypot_debounce`, 10 s per path) — untouched.
-->
<SettingsCard icon={IconShieldExclamation} iconVariant="red" title={$t('ransomware.title')} subtitle={$t('ransomware.subtitle')} class={klass}>
  {#if loading}
    <p class={settingsHint}>{$t('ransomware.loading')}</p>
  {:else}
    <div class="rw-warning">{$t('ransomware.scope_notice')}</div>

    <div class={settingsRow}>
      <div class="min-w-0">
        <span class={settingsRowLabel}>{$t('ransomware.enable_protection')}</span>
      </div>
      <Switch bind:checked={draft.ransomware.enabled} ariaLabel={$t('ransomware.enable_protection')} accent={SECURITY_ACCENT} />
    </div>

    <div class={settingsRow}>
      <div class="min-w-0">
        <span class={settingsRowLabel}>{$t('ransomware.enable_honeypots')}</span>
      </div>
      <Switch bind:checked={draft.ransomware.honeypots_enabled} ariaLabel={$t('ransomware.enable_honeypots')} accent={SECURITY_ACCENT} />
    </div>

    <div class={cn(settingsRowStacked, 'settings-field-full')}>
      <div class="grid grid-cols-[repeat(auto-fit,minmax(220px,1fr))] gap-4">
        <div class="settings-field flex flex-col gap-1.5">
          <label class={settingsRowLabel} for="rwThresholdEvents">{$t('ransomware.rate_threshold_events')}</label>
          <input id="rwThresholdEvents" class={settingsInput} type="number" min="1" bind:value={draft.ransomware.rate_threshold_events} />
        </div>
        <div class="settings-field flex flex-col gap-1.5">
          <label class={settingsRowLabel} for="rwThresholdSeconds">{$t('ransomware.rate_threshold_seconds')}</label>
          <input id="rwThresholdSeconds" class={settingsInput} type="number" min="1" bind:value={draft.ransomware.rate_threshold_seconds} />
        </div>
      </div>
    </div>

    <div class={cn(settingsRowStacked, 'settings-field-full')}>
      <label class={settingsRowLabel} for="rwAutoResponse">{$t('ransomware.auto_response')}</label>
      <select id="rwAutoResponse" class={cn(settingsSelect, 'max-w-[280px]')} bind:value={draft.ransomware.auto_response}>
        <option value="alert_only">{$t('ransomware.response_alert_only')}</option>
        <option value="disable_share">{$t('ransomware.response_disable_share')}</option>
        <option value="lock_account">{$t('ransomware.response_lock_account')}</option>
      </select>
      <span class={settingsHint}>{$t('ransomware.auto_response_hint')}</span>
    </div>

    <div class={settingsRowStacked}>
      <h3 class="rw-subheading">{$t('ransomware.protected_paths')}</h3>
      <div class="rw-add-row">
        <PathPickerField mode="folder" bind:value={newPath} ariaLabel={$t('ransomware.protected_paths')} />
        <button class={cn(settingsBtn, settingsBtnPrimary)} onclick={handleAddPath} disabled={addingPath}>
          {addingPath ? $t('ransomware.adding') : $t('ransomware.add_path')}
        </button>
      </div>
      {#each protectedPaths as p (p.id)}
        <div class="rw-list-row">
          <div class="min-w-0">
            <strong class="rw-list-title">{p.path}</strong>
            {#if p.honeypot_filename}
              <p class={settingsRowHint}>{$t('ransomware.honeypot_placed')}: {p.honeypot_filename}</p>
            {/if}
          </div>
          <button class={cn(settingsBtn, settingsBtnDanger)} onclick={() => handleRemovePath(p.id)}>
            {$t('ransomware.remove')}
          </button>
        </div>
      {:else}
        <p class={settingsHint}>{$t('ransomware.no_protected_paths')}</p>
      {/each}
    </div>

    {#if events.length > 0}
      <div class={settingsRowStacked}>
        <h3 class="rw-subheading">{$t('ransomware.recent_events')}</h3>
        {#each events.slice(0, 5) as event (event.id)}
          <div class="rw-list-row">
            <div class="min-w-0">
              <strong class="rw-list-title">{eventLabel(event.event_type)}</strong>
              <p class={settingsRowHint}>{formatDate(event.created_at)}{event.suspected_username ? ` · ${event.suspected_username}` : ''}</p>
            </div>
            <span class="settings-badge settings-badge-warning">{event.response_taken}</span>
          </div>
        {/each}
      </div>
    {/if}
  {/if}
</SettingsCard>

<style>
  .rw-warning {
    margin: 0 0 12px;
    padding: var(--hv-space-2-5) var(--hv-space-3);
    border-radius: var(--hv-radius-lg);
    background: var(--hv-warning-bg);
    color: var(--hv-warning-text);
    font-size: var(--hv-font-size-sm);
  }

  .rw-subheading {
    margin: 0;
    font-size: var(--hv-font-size-sm);
    font-weight: var(--hv-font-weight-semibold);
    color: var(--hv-settings-title);
  }

  .rw-add-row {
    display: flex;
    gap: var(--hv-space-2-5);
    align-items: flex-start;
  }

  /* PathPickerField's root is `.hv-ppf` (replaced the old PathField's `.settings-input`). */
  .rw-add-row > :global(.hv-ppf) {
    flex: 1;
    min-width: 0;
  }

  .rw-list-row {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 12px;
    padding: var(--hv-space-2-5) var(--hv-space-3);
    border: 1.45px solid var(--hv-settings-hairline);
    border-radius: var(--hv-radius-lg);
  }

  .rw-list-title {
    font-size: var(--hv-font-size-sm);
    font-weight: var(--hv-font-weight-medium);
    color: var(--hv-settings-title);
    word-break: break-all;
  }

  .settings-badge {
    display: inline-flex;
    align-items: center;
    flex-shrink: 0;
    padding: 2px 8px;
    border-radius: var(--hv-radius-lg);
    font-size: var(--hv-font-size-xs);
    font-weight: var(--hv-font-weight-medium);
  }

  .settings-badge-warning {
    background: var(--hv-warning-bg);
    color: var(--hv-warning-text);
  }
</style>

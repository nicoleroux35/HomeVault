<!-- ========================================================================
     File          : DdnsCard.svelte
     Author        : Nicolas Le Roux
     Created       : 29/10/2026
     Last modified : 29/10/2026 by Nicolas Le Roux
     Version       : 1.0.0

     Description:
       Settings card for dynamic DNS: provider, host name, write-only token,
       custom update URL, the address this host would publish next to the one
       the provider is believed to hold, and the "update now" probe.

     Dependencies:
       - svelte (>= 5), $lib/services/api, $lib/stores/i18n,
         ./SettingsCard, ./vocabulary, theme/Switch

     Usage:
       Embedded in routes/Settings.svelte's `security` category. See
       docs/guides/ddns.md.
     ======================================================================== -->
<script lang="ts">
  import { onMount } from 'svelte';
  import { api } from '$lib/services/api';
  import { t } from '$lib/stores/i18n';
  import { Switch } from '$lib/components/theme';
  import SettingsCard from './SettingsCard.svelte';
  import { cn } from '$lib/utils.js';
  import {
    settingsRow,
    settingsRowLabel,
    settingsRowHint,
    settingsHint,
    settingsInput,
    settingsSelect,
    settingsBtn,
    settingsBtnGhost,
    settingsBtnPrimary,
    SECURITY_ACCENT
  } from './vocabulary';

  let { class: klass = '' }: { class?: string } = $props();

  const securityAccent = SECURITY_ACCENT;

  let loading = $state(true);
  let loadError = $state('');
  let notice = $state<{ kind: 'ok' | 'error'; text: string } | null>(null);
  let busy = $state(false);

  let enabled = $state(false);
  let provider = $state<'duckdns' | 'dynv6' | 'cloudflare' | 'custom'>('duckdns');
  let hostname = $state('');
  let token = $state('');
  let tokenSet = $state(false);
  let customUrl = $state('');

  let status = $state('disabled');
  let lastIp = $state<string | null>(null);
  let lastUpdate = $state<string | null>(null);
  let lastError = $state<string | null>(null);
  let currentIp = $state<string | null>(null);

  const needsToken = $derived(provider !== 'custom');

  function applyPayload(d: any) {
    enabled = !!d.enabled;
    const p = d.provider;
    provider = p === 'dynv6' || p === 'cloudflare' || p === 'custom' ? p : 'duckdns';
    hostname = d.hostname ?? '';
    tokenSet = d.token === 'set';
    token = tokenSet ? 'set' : '';
    customUrl = d.custom_update_url ?? '';
    status = d.status ?? 'disabled';
    lastIp = d.last_ip ?? null;
    lastUpdate = d.last_update ?? null;
    lastError = d.last_error ?? null;
    currentIp = d.current_ip ?? null;
  }

  async function load() {
    loading = true;
    loadError = '';
    const res = await api.getDdnsSettings();
    if (res.success && res.data) applyPayload(res.data);
    else loadError = res.message || $t('settings.ddns_load_error');
    loading = false;
  }

  onMount(load);

  async function save() {
    busy = true;
    notice = null;
    const res = await api.setDdnsSettings({
      enabled,
      provider,
      hostname: hostname.trim(),
      token,
      custom_update_url: customUrl.trim(),
    });
    if (res.success) {
      const d = (res.data as any)?.settings;
      if (d) applyPayload(d);
      notice = { kind: 'ok', text: $t('settings.ddns_saved') };
    } else {
      notice = { kind: 'error', text: res.message || $t('settings.ddns_save_error') };
    }
    busy = false;
  }

  async function updateNow() {
    busy = true;
    notice = null;
    const res = await api.updateDdnsNow();
    if (res.success) {
      notice = { kind: 'ok', text: $t('settings.ddns_update_ok') };
      await load();
    } else {
      // The provider's own reason, verbatim — it is what tells the admin
      // whether the token or the name is wrong.
      notice = { kind: 'error', text: res.message || $t('settings.ddns_update_error') };
      await load();
    }
    busy = false;
  }

  function statusClass(s: string): string {
    if (s === 'up-to-date') return 'ddns-badge ddns-badge-ok';
    if (s === 'unknown') return 'ddns-badge ddns-badge-warn';
    if (s === 'error') return 'ddns-badge ddns-badge-error';
    return 'ddns-badge';
  }
</script>

<SettingsCard
  title={$t('settings.ddns_title')}
  subtitle={$t('settings.ddns_subtitle')}
  class={klass}
>
  {#if loading}
    <p class={settingsHint}>{$t('common.loading')}</p>
  {:else}
    {#if loadError}
      <div class="ddns-alert ddns-alert-error" role="alert">{loadError}</div>
    {/if}
    {#if notice}
      <div
        class={cn('ddns-alert', notice.kind === 'ok' ? 'ddns-alert-ok' : 'ddns-alert-error')}
        role={notice.kind === 'ok' ? 'status' : 'alert'}
        data-testid="ddns-notice"
      >
        {notice.text}
      </div>
    {/if}

    <div class={settingsRow}>
      <div class="min-w-0">
        <span id="ddns-enable-label" class={settingsRowLabel}>{$t('settings.ddns_enable')}</span>
        <p class={settingsRowHint}>{$t('settings.ddns_enable_hint')}</p>
      </div>
      <Switch bind:checked={enabled} ariaLabelledby="ddns-enable-label" accent={securityAccent} />
    </div>

    <div class={settingsRow} data-testid="ddns-status-row">
      <span class={settingsRowLabel}>{$t('settings.ddns_status')}</span>
      <span class={statusClass(status)} data-testid="ddns-status">{status}</span>
    </div>

    {#if lastError}
      <div class="ddns-alert ddns-alert-error" role="alert" data-testid="ddns-last-error">
        {lastError}
      </div>
    {/if}

    <div class={settingsRow}>
      <label class={settingsRowLabel} for="ddns-provider">{$t('settings.ddns_provider')}</label>
      <select id="ddns-provider" class={cn(settingsSelect, 'ddns-select')} bind:value={provider} data-testid="ddns-provider">
        <option value="duckdns">DuckDNS</option>
        <option value="dynv6">dynv6</option>
        <option value="cloudflare">Cloudflare</option>
        <option value="custom">{$t('settings.ddns_provider_custom')}</option>
      </select>
    </div>

    <div class={settingsRow}>
      <label class={settingsRowLabel} for="ddns-hostname">{$t('settings.ddns_hostname')}</label>
      <input
        id="ddns-hostname"
        class={cn(settingsInput, 'ddns-input')}
        type="text"
        bind:value={hostname}
        placeholder="myhome.duckdns.org"
        data-testid="ddns-hostname"
      />
    </div>

    {#if needsToken}
      <div class={settingsRow}>
        <div class="min-w-0">
          <label class={settingsRowLabel} for="ddns-token">{$t('settings.ddns_token')}</label>
          <p class={settingsRowHint}>
            {tokenSet ? $t('settings.ddns_token_kept') : $t('settings.ddns_token_hint')}
          </p>
        </div>
        <input
          id="ddns-token"
          class={cn(settingsInput, 'ddns-input')}
          type="password"
          bind:value={token}
          autocomplete="new-password"
          data-testid="ddns-token"
        />
      </div>
    {:else}
      <div class={settingsRow}>
        <div class="min-w-0">
          <label class={settingsRowLabel} for="ddns-custom-url">{$t('settings.ddns_custom_url')}</label>
          <p class={settingsRowHint}>{$t('settings.ddns_custom_url_hint')}</p>
        </div>
        <input
          id="ddns-custom-url"
          class={cn(settingsInput, 'ddns-input')}
          type="url"
          bind:value={customUrl}
          placeholder="https://ddns.example.com/update?host=home.example.com&ip=&#123;ip&#125;"
          data-testid="ddns-custom-url"
        />
      </div>
    {/if}

    <div class="settings-info-rows flex flex-col">
      <div class="settings-info-row">
        <span class="settings-info-label">{$t('settings.ddns_current_ip')}</span>
        <span class="settings-info-value ddns-mono" data-testid="ddns-current-ip">{currentIp ?? '—'}</span>
      </div>
      <div class="settings-info-row">
        <span class="settings-info-label">{$t('settings.ddns_last_ip')}</span>
        <span class="settings-info-value ddns-mono" data-testid="ddns-last-ip">{lastIp ?? '—'}</span>
      </div>
      <div class="settings-info-row">
        <span class="settings-info-label">{$t('settings.ddns_last_update')}</span>
        <span class="settings-info-value">{lastUpdate ? new Date(lastUpdate).toLocaleString() : '—'}</span>
      </div>
    </div>

    <div class="ddns-actions">
      <button class={cn(settingsBtn, settingsBtnPrimary)} onclick={save} disabled={busy} data-testid="ddns-save">
        {$t('common.save')}
      </button>
      <button
        class={cn(settingsBtn, settingsBtnGhost)}
        onclick={updateNow}
        disabled={busy || !enabled}
        data-testid="ddns-update"
      >
        {$t('settings.ddns_update_now')}
      </button>
      <button class={cn(settingsBtn, settingsBtnGhost)} onclick={load} disabled={busy} data-testid="ddns-refresh">
        {$t('common.refresh')}
      </button>
    </div>
  {/if}
</SettingsCard>

<style>
  .ddns-input {
    max-width: 320px;
  }

  .ddns-select {
    width: auto;
    min-width: 200px;
  }

  .ddns-badge {
    display: inline-flex;
    align-items: center;
    height: 28px;
    padding: 0 12px;
    border-radius: 999px;
    font-size: var(--hv-font-size-xs);
    font-weight: var(--hv-font-weight-semibold);
    background: var(--hv-settings-tile);
    border: 1.45px solid var(--hv-settings-border);
    color: var(--hv-settings-label);
  }

  .ddns-badge-ok {
    background: var(--hv-success-bg);
    border-color: var(--hv-success);
    color: var(--hv-success-text);
  }

  .ddns-badge-warn {
    background: var(--hv-warning-light);
    border-color: var(--hv-warning);
    color: var(--hv-warning);
  }

  .ddns-badge-error {
    background: var(--hv-error-light);
    border-color: var(--hv-error);
    color: var(--hv-error);
  }

  .ddns-alert {
    margin: 0 0 12px;
    padding: var(--hv-space-2-5) var(--hv-space-3);
    border-radius: var(--hv-radius-lg);
    font-size: var(--hv-font-size-sm);
  }

  .ddns-alert-ok {
    background: var(--hv-success-bg);
    color: var(--hv-success-text);
  }

  .ddns-alert-error {
    background: var(--hv-error-light);
    color: var(--hv-error);
  }

  .settings-info-row {
    display: flex;
    justify-content: space-between;
    gap: 16px;
    align-items: baseline;
    padding: 8px 0;
    border-bottom: 1.45px solid var(--hv-settings-hairline);
  }

  .settings-info-row:last-child {
    border-bottom: none;
  }

  .settings-info-label {
    flex-shrink: 0;
    font-size: var(--hv-font-size-sm);
    color: var(--hv-settings-label);
  }

  .settings-info-value {
    text-align: right;
    font-size: var(--hv-font-size-sm);
    font-weight: var(--hv-font-weight-medium);
    color: var(--hv-settings-title);
  }

  .ddns-mono {
    font-family: var(--hv-font-mono);
    font-weight: var(--hv-font-weight-regular);
  }

  .ddns-actions {
    display: flex;
    flex-wrap: wrap;
    gap: 10px;
    padding: 12px 0;
  }
</style>

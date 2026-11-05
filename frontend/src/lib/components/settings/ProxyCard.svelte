<!-- ========================================================================
     File          : ProxyCard.svelte
     Author        : Nicolas Le Roux
     Created       : 05/11/2026
     Last modified : 05/11/2026 by Nicolas Le Roux
     Version       : 1.0.0

     Description:
       Settings card for the outbound HTTP(S) proxy: on/off switch, one proxy
       URL per scheme, an optional bypass list and the effective exemptions
       (loopback is always added by the server).

     Dependencies:
       - svelte (>= 5), $lib/services/api, $lib/stores/i18n,
         ./SettingsCard, ./vocabulary, theme/Switch

     Usage:
       Embedded in routes/Settings.svelte's `security` category. See
       docs/guides/settings.md → "Outbound HTTP proxy".
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
    settingsBtn,
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
  let httpProxy = $state('');
  let httpsProxy = $state('');
  let noProxy = $state('');
  let effectiveNoProxy = $state('');

  function applyPayload(d: any) {
    enabled = !!d.enabled;
    httpProxy = d.http_proxy ?? '';
    httpsProxy = d.https_proxy ?? '';
    noProxy = d.no_proxy ?? '';
    effectiveNoProxy = d.effective_no_proxy ?? '';
  }

  async function load() {
    loading = true;
    loadError = '';
    const res = await api.getProxySettings();
    if (res.success && res.data) applyPayload(res.data);
    else loadError = res.message || $t('settings.proxy_load_error');
    loading = false;
  }

  onMount(load);

  async function save() {
    busy = true;
    notice = null;
    const res = await api.setProxySettings({
      enabled,
      http_proxy: httpProxy.trim(),
      https_proxy: httpsProxy.trim(),
      no_proxy: noProxy.trim()
    });
    if (res.success) {
      const d = (res.data as any)?.settings;
      if (d) applyPayload(d);
      notice = { kind: 'ok', text: $t('settings.proxy_saved') };
    } else {
      notice = { kind: 'error', text: res.message || $t('settings.proxy_save_error') };
    }
    busy = false;
  }
</script>

<SettingsCard
  title={$t('settings.proxy_title')}
  subtitle={$t('settings.proxy_subtitle')}
  class={klass}
>
  {#if loading}
    <p class={settingsHint}>{$t('common.loading')}</p>
  {:else}
    {#if loadError}
      <div class="proxy-alert proxy-alert-error" role="alert">{loadError}</div>
    {/if}
    {#if notice}
      <div
        class={cn('proxy-alert', notice.kind === 'ok' ? 'proxy-alert-ok' : 'proxy-alert-error')}
        role={notice.kind === 'ok' ? 'status' : 'alert'}
        data-testid="proxy-notice"
      >
        {notice.text}
      </div>
    {/if}

    <div class={settingsRow}>
      <div class="min-w-0">
        <span id="proxy-enable-label" class={settingsRowLabel}>{$t('settings.proxy_enable')}</span>
        <p class={settingsRowHint}>{$t('settings.proxy_enable_hint')}</p>
      </div>
      <Switch bind:checked={enabled} ariaLabelledby="proxy-enable-label" accent={securityAccent} />
    </div>

    <div class={settingsRow}>
      <div class="min-w-0">
        <label class={settingsRowLabel} for="proxy-http">{$t('settings.proxy_http')}</label>
        <p class={settingsRowHint}>{$t('settings.proxy_url_hint')}</p>
      </div>
      <input
        id="proxy-http"
        class={cn(settingsInput, 'proxy-input')}
        type="url"
        inputmode="url"
        bind:value={httpProxy}
        placeholder="http://proxy.example.com:3128"
        autocomplete="off"
        data-testid="proxy-http"
      />
    </div>

    <div class={settingsRow}>
      <div class="min-w-0">
        <label class={settingsRowLabel} for="proxy-https">{$t('settings.proxy_https')}</label>
        <p class={settingsRowHint}>{$t('settings.proxy_url_hint')}</p>
      </div>
      <input
        id="proxy-https"
        class={cn(settingsInput, 'proxy-input')}
        type="url"
        inputmode="url"
        bind:value={httpsProxy}
        placeholder="http://proxy.example.com:3128"
        autocomplete="off"
        data-testid="proxy-https"
      />
    </div>

    <div class={settingsRow}>
      <div class="min-w-0">
        <label class={settingsRowLabel} for="proxy-no-proxy">{$t('settings.proxy_no_proxy')}</label>
        <p class={settingsRowHint}>{$t('settings.proxy_no_proxy_hint')}</p>
      </div>
      <input
        id="proxy-no-proxy"
        class={cn(settingsInput, 'proxy-input')}
        type="text"
        bind:value={noProxy}
        placeholder="example.com, 192.168.1.0/24"
        autocomplete="off"
        data-testid="proxy-no-proxy"
      />
    </div>

    <div class={settingsRow} data-testid="proxy-effective-row">
      <span class={settingsRowLabel}>{$t('settings.proxy_effective')}</span>
      <span class="proxy-effective" data-testid="proxy-effective">{effectiveNoProxy || '—'}</span>
    </div>

    <div class="proxy-actions">
      <button class={cn(settingsBtn, settingsBtnPrimary)} onclick={save} disabled={busy} data-testid="proxy-save">
        {$t('common.save')}
      </button>
    </div>
  {/if}
</SettingsCard>

<style>
  .proxy-input {
    max-width: 320px;
  }

  .proxy-effective {
    max-width: 320px;
    text-align: right;
    overflow-wrap: anywhere;
    font-family: var(--hv-font-mono);
    font-size: var(--hv-font-size-sm);
    color: var(--hv-settings-title);
  }

  .proxy-alert {
    margin: 0 0 12px;
    padding: var(--hv-space-2-5) var(--hv-space-3);
    border-radius: var(--hv-radius-lg);
    font-size: var(--hv-font-size-sm);
  }

  .proxy-alert-ok {
    background: var(--hv-success-bg);
    color: var(--hv-success-text);
  }

  .proxy-alert-error {
    background: var(--hv-error-light);
    color: var(--hv-error);
  }

  .proxy-actions {
    display: flex;
    flex-wrap: wrap;
    gap: 10px;
    padding: 12px 0;
  }
</style>

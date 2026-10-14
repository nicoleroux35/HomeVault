<!-- ========================================================================
     File          : TlsCard.svelte
     Author        : Nicolas Le Roux
     Created       : 28/09/2026
     Last modified : 28/09/2026 by Nicolas Le Roux
     Version       : 1.0.0

     Description:
       Settings sub-card for HTTPS/TLS: enable toggle, ports, certificate
       info and regeneration, bound to the shared settings draft (its TLS
       group), with save status tracked via a runes $derived over the
       draft's getter.

     Dependencies:
       - svelte (>= 5), stores/settingsDraft.svelte, $lib/services/api

     Usage:
       Embedded in routes/Settings.svelte's general group.
     ======================================================================== -->
<script lang="ts">
  import { onMount } from 'svelte';
  import { api } from '$lib/services/api';
  import { t } from '$lib/stores/i18n';
  import { draft, registerGroup, tls, setTlsSaveResult, getSaveStatus } from '$lib/stores/settingsDraft.svelte';

  /* Runes `$derived` tracks reads made inside the getter. */
  const saveStatus = $derived(getSaveStatus());
  import { IconLock } from '@tabler/icons-svelte';
  import SettingsCard from './SettingsCard.svelte';
  import Switch from '$lib/components/theme/Switch.svelte';
  import PathPickerField from '$lib/components/theme/PathPickerField.svelte';
  import { cn } from '$lib/utils.js';
  import {
    settingsRow, settingsRowStacked, settingsRowLabel, settingsRowHint, settingsHint,
    settingsInput, settingsBtn, settingsBtnPrimary, settingsBtnGhost, SECURITY_ACCENT
  } from './vocabulary';

  /* This card lives in the `security` category, so its switches fill with the
     category colour when on — the same rule Settings.svelte applies to every
     other row on the screen (`accent={activeCat.accent}`). */
  const securityAccent = SECURITY_ACCENT;

  /* The parent stacks this card as the first section of the Security panel,
     so it hands down the 24px top inset (`mt-6`) the other categories get
     from their spacer. Without a `class` prop that attribute would be a type
     error. */
  let { class: klass = '' }: { class?: string } = $props();

  interface CertInfo {
    subject: string;
    issuer: string;
    sans: string[];
    not_before: string | null;
    not_after: string | null;
    days_remaining: number | null;
    self_signed: boolean;
    error: string | null;
  }

  let loading = $state(true);
  let error = $state('');
  let active = $state(false);
  let certInfo = $state<CertInfo | null>(null);
  let restarting = $state(false);

  async function load() {
    loading = true;
    error = '';
    const res = await api.getTlsSettings();
    if (res.success && res.data) {
      const d = res.data as any;
      // Stage the server values into the shared draft once; the TLS group is
      // flushed by the Settings screen's global Save button, not this card.
      registerGroup('tls', {
        enabled: !!d.enabled,
        https_port: d.effective_https_port ?? 8443,
        http_port: d.http_port ?? 8080,
        mode: d.mode === 'manual' ? 'manual' : 'self-signed',
        cert_path: d.cert_path ?? '',
        key_path: d.key_path ?? '',
        redirect: !!d.redirect,
      });
      active = !!d.active;
      certInfo = d.cert_info ?? null;
    } else {
      error = res.message || $t('settings.tls_load_error');
    }
    loading = false;
  }

  onMount(load);

  async function regenerate() {
    const res = await api.regenerateTlsCert();
    if (res.success) {
      const d = res.data as any;
      if (d?.cert_info) certInfo = d.cert_info;
      if (d?.requires_restart) setTlsSaveResult({ requires_restart: true, cert_info: d.cert_info ?? null });
    } else {
      error = res.message || $t('settings.tls_regen_error');
    }
  }

  async function restart() {
    restarting = true;
    await api.restartServer();
    // The server goes away; the page will reconnect on its own.
    setTimeout(() => (restarting = false), 8000);
  }

  const expiryLabel = $derived(
    certInfo?.not_after
      ? new Date(certInfo.not_after).toLocaleDateString()
      : '—',
  );
</script>

<!--
  TlsCard — the built-in HTTPS/TLS listener (see docs/guides/tls.md), rebuilt
  on the shared Settings vocabulary. Markup/CSS only: `load` / `regenerate` /
  `restart` and their `api.*` calls are unchanged.

  Persistence moved to the Settings screen's global Save button (2026-09-15):
  this card's editable fields bind to the shared `draft.tls` group and the
  per-card Save button is gone. `regenerate` and `restart` remain immediate
  actions (they drive a live cert / a live process). The "restart required"
  banner is surfaced from the store's `tls.saveResult` after a global save.

  Two E2E contracts ride on this card and must not move:
    * the enable switch stays the FIRST toggle in the card and is reached
      through `.settings-toggle-label` + `input[type="checkbox"]`
      (`flipSwitch()`, tests 12.3, 12.4, 12.5, 12.6) — it is the first row. It
      is the shared `theme/Switch`, like every other toggle on the screen;
    * the HTTPS port vs HTTP port clash keeps its inline error message.
-->
<SettingsCard
  icon={IconLock}
  iconVariant="green"
  title={$t('settings.tls_title')}
  subtitle={$t('settings.tls_subtitle')}
  class={klass}
>
  {#if loading}
    <p class={settingsHint}>{$t('common.loading')}</p>
  {:else}
    {#if error}
      <div class="tls-alert tls-alert-error">{error}</div>
    {/if}
    {#if tls.saveResult?.requires_restart}
      <div class="tls-alert tls-alert-warning">
        <span>{$t('settings.tls_restart_required')}</span>
        <button class={cn(settingsBtn, settingsBtnPrimary)} onclick={restart} disabled={restarting}>
          {restarting ? $t('settings.tls_restarting') : $t('settings.tls_restart_now')}
        </button>
      </div>
    {/if}

    <div class={settingsRow}>
      <div class="min-w-0">
        <span id="tls-enable-label" class={settingsRowLabel}>{$t('settings.tls_enable')}</span>
        <p class={settingsRowHint}>{$t('settings.tls_enable_hint')}</p>
      </div>
      <Switch bind:checked={draft.tls.enabled} ariaLabelledby="tls-enable-label" accent={securityAccent} />
    </div>

    {#if draft.tls.enabled}
      <div class={settingsRow}>
        <label class={settingsRowLabel} for="tls-https-port">{$t('settings.tls_port')}</label>
        <div class="flex flex-col items-end gap-1">
          <input
            id="tls-https-port"
            class={cn(settingsInput, 'tls-port-input w-[120px]')}
            type="number"
            min="1"
            max="65535"
            bind:value={draft.tls.https_port}
          />
          {#if draft.tls.https_port === draft.tls.http_port}
            <span class="tls-inline-error">{$t('settings.tls_port_clash')}</span>
          {/if}
        </div>
      </div>

      <div class={settingsRowStacked}>
        <span class={settingsRowLabel}>{$t('settings.tls_mode')}</span>
        <div class="flex flex-wrap items-center gap-4">
          <label class="tls-radio">
            <input type="radio" name="tls-mode" value="self-signed" bind:group={draft.tls.mode} />
            <span>{$t('settings.tls_mode_self')}</span>
          </label>
          <label class="tls-radio">
            <input type="radio" name="tls-mode" value="manual" bind:group={draft.tls.mode} />
            <span>{$t('settings.tls_mode_manual')}</span>
          </label>
        </div>
      </div>

      {#if draft.tls.mode === 'manual'}
        <div class={settingsRowStacked}>
          <label class={settingsRowLabel} for="tls-cert-path">{$t('settings.tls_cert_path')}</label>
          <PathPickerField bind:value={draft.tls.cert_path} mode="file" id="tls-cert-path" ariaLabel={$t('settings.tls_cert_path')} />
        </div>
        <div class={settingsRowStacked}>
          <label class={settingsRowLabel} for="tls-key-path">{$t('settings.tls_key_path')}</label>
          <PathPickerField bind:value={draft.tls.key_path} mode="file" id="tls-key-path" ariaLabel={$t('settings.tls_key_path')} />
        </div>
      {/if}

      <div class={settingsRow}>
        <div class="min-w-0">
          <span id="tls-redirect-label" class={settingsRowLabel}>{$t('settings.tls_redirect')}</span>
          <p class={settingsRowHint}>{$t('settings.tls_redirect_hint')}</p>
        </div>
        <Switch bind:checked={draft.tls.redirect} ariaLabelledby="tls-redirect-label" accent={securityAccent} />
      </div>

      <div class={settingsRowStacked}>
        <h3 class="tls-subheading">{$t('settings.tls_cert_info')}</h3>
        {#if certInfo?.error}
          <p class={settingsHint}>{certInfo.error}</p>
        {:else if certInfo}
          <div class="settings-info-rows flex flex-col">
            <div class="settings-info-row">
              <span class="settings-info-label">{$t('settings.tls_cert_subject')}</span>
              <span class="settings-info-value tls-mono">{certInfo.subject || '—'}</span>
            </div>
            <div class="settings-info-row">
              <span class="settings-info-label">{$t('settings.tls_cert_type')}</span>
              <span class="settings-info-value">
                {certInfo.self_signed ? $t('settings.tls_cert_self_signed') : $t('settings.tls_cert_provided')}
              </span>
            </div>
            <div class="settings-info-row">
              <span class="settings-info-label">{$t('settings.tls_cert_expires')}</span>
              <span class="settings-info-value">
                {expiryLabel}
                {#if certInfo.days_remaining != null}
                  <span class="settings-hint">({$t('settings.tls_cert_days_left', { days: String(certInfo.days_remaining) })})</span>
                {/if}
              </span>
            </div>
            {#if certInfo.sans?.length}
              <div class="settings-info-row">
                <span class="settings-info-label">{$t('settings.tls_cert_san')}</span>
                <span class="settings-info-value tls-mono tls-san">{certInfo.sans.join(', ')}</span>
              </div>
            {/if}
          </div>
        {/if}
      </div>

      {#if draft.tls.mode === 'self-signed'}
        <div class="tls-regen-row flex items-center justify-end py-3">
          <button class={cn(settingsBtn, settingsBtnGhost)} onclick={regenerate} disabled={saveStatus === 'saving'}>
            {$t('settings.tls_regenerate')}
          </button>
        </div>
      {/if}
    {/if}
  {/if}
</SettingsCard>

<style>
  .tls-radio {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    font-size: var(--hv-font-size-sm);
    color: var(--hv-settings-row);
    cursor: pointer;
  }

  .tls-inline-error {
    font-size: var(--hv-font-size-xs);
    color: var(--hv-error);
  }

  .tls-subheading {
    margin: 0;
    font-size: var(--hv-font-size-sm);
    font-weight: var(--hv-font-weight-semibold);
    color: var(--hv-settings-title);
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
    display: flex;
    flex-wrap: wrap;
    justify-content: flex-end;
    gap: 6px;
    text-align: right;
    font-size: var(--hv-font-size-sm);
    font-weight: var(--hv-font-weight-medium);
    color: var(--hv-settings-title);
  }

  .tls-mono {
    font-family: var(--hv-font-mono);
    font-weight: var(--hv-font-weight-regular);
    word-break: break-all;
  }

  .tls-san {
    max-width: 60%;
  }

  .tls-alert {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    margin: 0 0 12px;
    padding: var(--hv-space-2-5) var(--hv-space-3);
    border-radius: var(--hv-radius-lg);
    font-size: var(--hv-font-size-sm);
  }

  .tls-alert-error {
    background: var(--hv-error-light);
    color: var(--hv-error);
  }

  .tls-alert-warning {
    background: var(--hv-warning-light);
    color: var(--hv-warning);
  }
</style>

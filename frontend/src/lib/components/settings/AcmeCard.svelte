<!-- ========================================================================
     File          : AcmeCard.svelte
     Author        : Nicolas Le Roux
     Created       : 29/10/2026
     Last modified : 29/10/2026 by Nicolas Le Roux
     Version       : 1.0.0

     Description:
       Settings card for certificate automation (ACME): the public name, the
       challenge and its DNS publisher, the write-only provider token, the
       derived status badge, the pending manual DNS record, and the three
       immediate actions (issue/renew, confirm the record, probe port 80).

     Dependencies:
       - svelte (>= 5), $lib/services/api, $lib/stores/i18n,
         ./SettingsCard, ./vocabulary, theme/Switch

     Usage:
       Embedded in routes/Settings.svelte's `security` category, below
       TlsCard. See docs/guides/acme.md.
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
    settingsRowStacked,
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

  interface PendingRecord {
    name: string;
    value: string;
  }

  interface CertInfo {
    subject: string;
    issuer: string;
    not_after: string | null;
    days_remaining: number | null;
    self_signed: boolean;
    error: string | null;
  }

  /* The card is a peer of TlsCard and AntivirusCard: it carries its own
     header, so the caller gives it the same `mt-3` stacking spacer. */
  const securityAccent = SECURITY_ACCENT;

  let loading = $state(true);
  let loadError = $state('');
  let notice = $state<{ kind: 'ok' | 'error'; text: string } | null>(null);
  let busy = $state(false);

  /* Draft — saved by the card's own button: these fields drive a background
     run whose state (status, pending record, certificate) is server-owned, so
     staging them in the screen-wide draft would show a status that does not
     exist yet. */
  let enabled = $state(false);
  let domain = $state('');
  let contactEmail = $state('');
  let directoryUrl = $state('');
  let defaultDirectory = $state('');
  let stagingDirectory = $state('');
  let tosAgreed = $state(false);
  let challenge = $state<'http-01' | 'dns-01'>('http-01');
  let provider = $state<'manual' | 'cloudflare'>('manual');
  let dnsToken = $state('');
  let tokenSet = $state(false);
  let propagation = $state(60);

  /* Server state, rendered as-is. */
  let status = $state('disabled');
  let lastError = $state<string | null>(null);
  let lastAttempt = $state<string | null>(null);
  let pending = $state<PendingRecord | null>(null);
  let cert = $state<CertInfo | null>(null);
  let runInProgress = $state(false);
  let automationActive = $state(false);

  const showDnsFields = $derived(challenge === 'dns-01');

  function applyPayload(d: any) {
    enabled = !!d.enabled;
    domain = d.domain ?? '';
    contactEmail = d.contact_email ?? '';
    directoryUrl = d.directory_url ?? '';
    defaultDirectory = d.default_directory ?? '';
    stagingDirectory = d.staging_directory ?? '';
    tosAgreed = !!d.tos_agreed;
    challenge = d.challenge === 'dns-01' ? 'dns-01' : 'http-01';
    provider = d.provider === 'cloudflare' ? 'cloudflare' : 'manual';
    // A token is never echoed: the server answers "set" or "".
    tokenSet = d.dns_api_token === 'set';
    dnsToken = tokenSet ? 'set' : '';
    propagation = Number(d.dns_propagation_seconds ?? 60);
    status = d.status ?? 'disabled';
    lastError = d.last_error ?? null;
    lastAttempt = d.last_attempt ?? null;
    pending = d.pending_dns_record ?? null;
    cert = d.cert ?? null;
    runInProgress = !!d.run_in_progress;
    automationActive = !!d.automation_active;
  }

  async function load() {
    loading = true;
    loadError = '';
    const res = await api.getAcmeSettings();
    if (res.success && res.data) applyPayload(res.data);
    else loadError = res.message || $t('settings.acme_load_error');
    loading = false;
  }

  onMount(load);

  /** Save, then re-read: the server derives `status` from what is on disk. */
  async function save() {
    busy = true;
    notice = null;
    const res = await api.setAcmeSettings({
      enabled,
      domain: domain.trim(),
      contact_email: contactEmail.trim(),
      directory_url: directoryUrl.trim(),
      tos_agreed: tosAgreed,
      challenge,
      provider,
      dns_api_token: dnsToken,
      dns_propagation_seconds: propagation,
    });
    if (res.success) {
      const d = (res.data as any)?.settings;
      if (d) applyPayload(d);
      notice = { kind: 'ok', text: $t('settings.acme_saved') };
    } else {
      notice = { kind: 'error', text: res.message || $t('settings.acme_save_error') };
    }
    busy = false;
  }

  async function issue() {
    busy = true;
    notice = null;
    const res = await api.issueAcmeCertificate();
    if (res.success) {
      notice = { kind: 'ok', text: $t('settings.acme_issue_started') };
      // The run is in the background: poll once so the badge stops saying
      // "idle" while the authority is being contacted.
      setTimeout(load, 1500);
    } else {
      notice = { kind: 'error', text: res.message || $t('settings.acme_issue_error') };
    }
    busy = false;
  }

  async function confirmRecord() {
    busy = true;
    notice = null;
    const res = await api.confirmAcmeDnsRecord();
    if (res.success) {
      notice = { kind: 'ok', text: $t('settings.acme_confirm_started') };
      setTimeout(load, 1500);
    } else {
      notice = { kind: 'error', text: res.message || $t('settings.acme_confirm_error') };
    }
    busy = false;
  }

  async function testChallenge() {
    busy = true;
    notice = null;
    const res = await api.testAcmeChallenge();
    if (res.success && (res.data as any)?.reachable) {
      notice = { kind: 'ok', text: $t('settings.acme_test_ok') };
    } else {
      const reason = (res.data as any)?.error;
      notice = {
        kind: 'error',
        text: reason
          ? $t('settings.acme_test_failed_reason', { reason })
          : $t('settings.acme_test_failed'),
      };
    }
    busy = false;
  }

  function statusClass(s: string): string {
    if (s === 'valid') return 'acme-badge acme-badge-ok';
    if (s === 'expiring' || s === 'pending') return 'acme-badge acme-badge-warn';
    if (s === 'error') return 'acme-badge acme-badge-error';
    return 'acme-badge';
  }

  const expiryLabel = $derived(
    cert?.not_after ? new Date(cert.not_after).toLocaleDateString() : '—',
  );
</script>

<SettingsCard
  title={$t('settings.acme_title')}
  subtitle={$t('settings.acme_subtitle')}
  class={klass}
>
  {#if loading}
    <p class={settingsHint}>{$t('common.loading')}</p>
  {:else}
    {#if loadError}
      <div class="acme-alert acme-alert-error" role="alert">{loadError}</div>
    {/if}
    {#if notice}
      <div
        class={cn('acme-alert', notice.kind === 'ok' ? 'acme-alert-ok' : 'acme-alert-error')}
        role={notice.kind === 'ok' ? 'status' : 'alert'}
        data-testid="acme-notice"
      >
        {notice.text}
      </div>
    {/if}

    <div class={settingsRow}>
      <div class="min-w-0">
        <span id="acme-enable-label" class={settingsRowLabel}>{$t('settings.acme_enable')}</span>
        <p class={settingsRowHint}>{$t('settings.acme_enable_hint')}</p>
      </div>
      <Switch bind:checked={enabled} ariaLabelledby="acme-enable-label" accent={securityAccent} />
    </div>

    <div class={settingsRow} data-testid="acme-status-row">
      <span class={settingsRowLabel}>{$t('settings.acme_status')}</span>
      <span class={statusClass(status)} data-testid="acme-status">{status}</span>
    </div>

    {#if lastError}
      <div class="acme-alert acme-alert-error" role="alert" data-testid="acme-last-error">
        {lastError}
      </div>
    {/if}

    {#if pending}
      <div class="acme-record" data-testid="acme-pending-record">
        <p class="m-0! text-c-label font-semibold text-[var(--hv-settings-title)]">
          {$t('settings.acme_pending_title')}
        </p>
        <p class={settingsHint}>{$t('settings.acme_pending_hint')}</p>
        <dl class="acme-record-list">
          <dt>{$t('settings.acme_pending_name')}</dt>
          <dd>{pending.name}</dd>
          <dt>{$t('settings.acme_pending_value')}</dt>
          <dd>{pending.value}</dd>
        </dl>
      </div>
    {/if}

    <div class={settingsRow}>
      <label class={settingsRowLabel} for="acme-domain">{$t('settings.acme_domain')}</label>
      <input
        id="acme-domain"
        class={cn(settingsInput, 'acme-input')}
        type="text"
        bind:value={domain}
        placeholder="home.example.com"
        data-testid="acme-domain"
      />
    </div>

    <div class={settingsRow}>
      <label class={settingsRowLabel} for="acme-contact">{$t('settings.acme_contact')}</label>
      <input
        id="acme-contact"
        class={cn(settingsInput, 'acme-input')}
        type="email"
        bind:value={contactEmail}
        placeholder="admin@example.com"
        data-testid="acme-contact"
      />
    </div>

    <div class={settingsRow}>
      <label class={settingsRowLabel} for="acme-challenge">{$t('settings.acme_challenge')}</label>
      <select id="acme-challenge" class={cn(settingsSelect, 'acme-select')} bind:value={challenge} data-testid="acme-challenge">
        <option value="http-01">{$t('settings.acme_challenge_http')}</option>
        <option value="dns-01">{$t('settings.acme_challenge_dns')}</option>
      </select>
    </div>

    {#if showDnsFields}
      <div class={settingsRow}>
        <label class={settingsRowLabel} for="acme-provider">{$t('settings.acme_provider')}</label>
        <select id="acme-provider" class={cn(settingsSelect, 'acme-select')} bind:value={provider} data-testid="acme-provider">
          <option value="manual">{$t('settings.acme_provider_manual')}</option>
          <option value="cloudflare">{$t('settings.acme_provider_cloudflare')}</option>
        </select>
      </div>

      {#if provider === 'cloudflare'}
        <div class={settingsRow}>
          <div class="min-w-0">
            <label class={settingsRowLabel} for="acme-token">{$t('settings.acme_token')}</label>
            <p class={settingsRowHint}>
              {tokenSet ? $t('settings.acme_token_kept') : $t('settings.acme_token_hint')}
            </p>
          </div>
          <input
            id="acme-token"
            class={cn(settingsInput, 'acme-input')}
            type="password"
            bind:value={dnsToken}
            autocomplete="new-password"
            data-testid="acme-token"
          />
        </div>
      {/if}

      <div class={settingsRow}>
        <div class="min-w-0">
          <label class={settingsRowLabel} for="acme-propagation">{$t('settings.acme_propagation')}</label>
          <p class={settingsRowHint}>{$t('settings.acme_propagation_hint')}</p>
        </div>
        <input
          id="acme-propagation"
          class={cn(settingsInput, 'acme-port-input')}
          type="number"
          min="30"
          max="3600"
          bind:value={propagation}
        />
      </div>
    {/if}

    <div class={settingsRowStacked}>
      <label class={settingsRowLabel} for="acme-directory">{$t('settings.acme_directory')}</label>
      <select id="acme-directory" class={settingsSelect} bind:value={directoryUrl}>
        <option value="">{$t('settings.acme_directory_default')}</option>
        {#if stagingDirectory}
          <option value={stagingDirectory}>{$t('settings.acme_directory_staging')}</option>
        {/if}
      </select>
    </div>

    <label class="acme-tos">
      <input type="checkbox" bind:checked={tosAgreed} data-testid="acme-tos" />
      <span>{$t('settings.acme_tos')}</span>
    </label>

    {#if cert && !cert.error}
      <div class={settingsRowStacked}>
        <h3 class="acme-subheading">{$t('settings.acme_cert_title')}</h3>
        <div class="settings-info-rows flex flex-col">
          <div class="settings-info-row">
            <span class="settings-info-label">{$t('settings.tls_cert_subject')}</span>
            <span class="settings-info-value acme-mono">{cert.subject || '—'}</span>
          </div>
          <div class="settings-info-row">
            <span class="settings-info-label">{$t('settings.tls_cert_type')}</span>
            <span class="settings-info-value">
              {cert.self_signed ? $t('settings.tls_cert_self_signed') : $t('settings.tls_cert_provided')}
            </span>
          </div>
          <div class="settings-info-row">
            <span class="settings-info-label">{$t('settings.tls_cert_expires')}</span>
            <span class="settings-info-value">
              {expiryLabel}
              {#if cert.days_remaining != null}
                <span class={settingsHint}>
                  ({$t('settings.tls_cert_days_left', { days: String(cert.days_remaining) })})
                </span>
              {/if}
            </span>
          </div>
        </div>
      </div>
    {/if}

    <div class="acme-actions">
      <button class={cn(settingsBtn, settingsBtnPrimary)} onclick={save} disabled={busy} data-testid="acme-save">
        {$t('common.save')}
      </button>
      <button
        class={cn(settingsBtn, settingsBtnGhost)}
        onclick={issue}
        disabled={busy || !enabled || runInProgress}
        data-testid="acme-issue"
      >
        {$t('settings.acme_issue')}
      </button>
      {#if pending}
        <button class={cn(settingsBtn, settingsBtnGhost)} onclick={confirmRecord} disabled={busy} data-testid="acme-confirm">
          {$t('settings.acme_confirm')}
        </button>
      {/if}
      {#if challenge === 'http-01'}
        <button class={cn(settingsBtn, settingsBtnGhost)} onclick={testChallenge} disabled={busy || !domain.trim()} data-testid="acme-test">
          {$t('settings.acme_test')}
        </button>
      {/if}
      <button class={cn(settingsBtn, settingsBtnGhost)} onclick={load} disabled={busy} data-testid="acme-refresh">
        {$t('common.refresh')}
      </button>
    </div>

    {#if automationActive}
      <p class={settingsHint} data-testid="acme-active-hint">{$t('settings.acme_active_hint')}</p>
    {/if}
  {/if}
</SettingsCard>

<style>
  .acme-input {
    max-width: 320px;
  }

  .acme-select {
    width: auto;
    min-width: 200px;
  }

  .acme-port-input {
    width: 120px;
  }

  .acme-badge {
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

  .acme-badge-ok {
    background: var(--hv-success-bg);
    border-color: var(--hv-success);
    color: var(--hv-success-text);
  }

  .acme-badge-warn {
    background: var(--hv-warning-light);
    border-color: var(--hv-warning);
    color: var(--hv-warning);
  }

  .acme-badge-error {
    background: var(--hv-error-light);
    border-color: var(--hv-error);
    color: var(--hv-error);
  }

  .acme-alert {
    margin: 0 0 12px;
    padding: var(--hv-space-2-5) var(--hv-space-3);
    border-radius: var(--hv-radius-lg);
    font-size: var(--hv-font-size-sm);
  }

  .acme-alert-ok {
    background: var(--hv-success-bg);
    color: var(--hv-success-text);
  }

  .acme-alert-error {
    background: var(--hv-error-light);
    color: var(--hv-error);
  }

  .acme-record {
    margin: 0 0 12px;
    padding: var(--hv-space-2-5) var(--hv-space-3);
    border-radius: var(--hv-radius-lg);
    background: var(--hv-warning-light);
    color: var(--hv-warning);
    font-size: var(--hv-font-size-sm);
  }

  .acme-record-list {
    display: grid;
    grid-template-columns: auto 1fr;
    gap: 4px 12px;
    margin: 8px 0 0;
    font-family: var(--hv-font-mono);
    font-size: var(--hv-font-size-xs);
    word-break: break-all;
  }

  .acme-record-list dt {
    color: var(--hv-settings-label);
  }

  .acme-record-list dd {
    margin: 0;
  }

  .acme-tos {
    display: flex;
    align-items: flex-start;
    gap: 8px;
    padding: 12px 0;
    font-size: var(--hv-font-size-sm);
    color: var(--hv-settings-row);
    cursor: pointer;
  }

  .acme-subheading {
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

  .acme-mono {
    font-family: var(--hv-font-mono);
    font-weight: var(--hv-font-weight-regular);
    word-break: break-all;
  }

  .acme-actions {
    display: flex;
    flex-wrap: wrap;
    gap: 10px;
    padding: 12px 0;
  }
</style>

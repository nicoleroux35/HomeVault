<!-- ========================================================================
     File          : EmailOAuthCard.svelte
     Author        : Nicolas Le Roux
     Created       : 09/11/2026 by Nicolas Le Roux
     Version       : 1.0.0

     Description:
       Settings card for SMTP OAuth2 (Gmail / Microsoft 365, P1-16): provider,
       client registration, the exact callback URL, connect/disconnect and the
       live connection state. It never shows the client secret or a token back.

     Dependencies:
       - svelte (>= 5), $lib/services/api, $lib/stores/i18n,
         $lib/components/app (ConfirmDialog), $lib/utils/confirmDialog.svelte,
         ./SettingsCard, ./vocabulary

     Usage:
       Embedded in routes/Settings.svelte's `notifications` category, under the
       SMTP card. See docs/guides/email-oauth.md.
     ======================================================================== -->
<script lang="ts">
  import { onMount } from 'svelte';
  import { api } from '$lib/services/api';
  import { t } from '$lib/stores/i18n';
  import { ConfirmDialog } from '$lib/components/app';
  import { Alert } from '$lib/components/theme';
  import { createConfirmState } from '$lib/utils/confirmDialog.svelte';
  import SettingsCard from './SettingsCard.svelte';
  import { cn } from '$lib/utils.js';
  import {
    settingsRow,
    settingsRowLabel,
    settingsRowHint,
    settingsRowStacked,
    settingsHint,
    settingsInput,
    settingsSelect,
    settingsBtn,
    settingsBtnGhost,
    settingsBtnPrimary,
    settingsCardFooter
  } from './vocabulary';

  let { class: klass = '' }: { class?: string } = $props();

  const dlg = createConfirmState();

  type Provider = 'google' | 'microsoft';

  let loading = $state(true);
  let loadError = $state('');
  let busy = $state(false);
  let notice = $state<{ kind: 'ok' | 'error'; text: string } | null>(null);

  let provider = $state<Provider>('google');
  let clientId = $state('');
  /** Write-only: the server never sends it back, and blank keeps it. */
  let clientSecret = $state('');
  let redirectUrl = $state('');

  let configured = $state(false);
  let connected = $state(false);
  let connectedAt = $state<string | null>(null);
  let lastError = $state<string | null>(null);
  let smtpHost = $state('smtp.gmail.com');
  let smtpPort = $state(587);

  /** Set after Start; also the path when a pop-up blocker intervenes. */
  let authorizationUrl = $state('');
  let pollTimer: ReturnType<typeof setInterval> | null = null;

  function applyPayload(o: any) {
    provider = o.provider === 'microsoft' ? 'microsoft' : 'google';
    clientId = o.client_id ?? '';
    redirectUrl = o.redirect_url ?? '';
    configured = !!o.configured;
    connected = !!o.connected;
    connectedAt = o.connected_at ?? null;
    lastError = o.last_error ?? null;
    smtpHost = o.smtp_host ?? (provider === 'microsoft' ? 'smtp.office365.com' : 'smtp.gmail.com');
    smtpPort = typeof o.smtp_port === 'number' ? o.smtp_port : 587;
  }

  async function load() {
    const res = await api.getEmailOAuth();
    if (res.success && res.data) {
      applyPayload((res.data as any).oauth);
      loadError = '';
    } else {
      loadError = res.message || $t('settings.email_oauth_load_error');
    }
    loading = false;
  }

  function stopPolling() {
    if (pollTimer) {
      clearInterval(pollTimer);
      pollTimer = null;
    }
  }

  onMount(() => {
    void load();
    return stopPolling;
  });

  async function save(): Promise<boolean> {
    busy = true;
    notice = null;
    const res = await api.setEmailOAuth({
      provider,
      client_id: clientId.trim(),
      client_secret: clientSecret.trim() || undefined,
      redirect_url: redirectUrl.trim()
    });
    busy = false;
    if (res.success && res.data) {
      applyPayload((res.data as any).oauth);
      // The secret is never echoed; clearing the field means "keep the stored
      // one", so there is nothing to redisplay.
      clientSecret = '';
      notice = { kind: 'ok', text: $t('settings.email_oauth_saved') };
      return true;
    }
    notice = {
      kind: 'error',
      text: res.message || $t('settings.email_oauth_save_error')
    };
    return false;
  }

  async function connect() {
    // Save first: the flow uses the stored registration, and an admin who
    // typed a new client ID expects it to be the one used.
    if (!(await save())) return;
    busy = true;
    notice = null;
    const res = await api.startEmailOAuth();
    busy = false;
    if (!res.success || !res.data) {
      notice = {
        kind: 'error',
        text: res.message || $t('settings.email_oauth_start_error')
      };
      return;
    }
    authorizationUrl = (res.data as any).authorization_url ?? '';
    window.open(authorizationUrl, '_blank', 'noopener');
    notice = { kind: 'ok', text: $t('settings.email_oauth_waiting') };
    // Poll until the callback has stored the token (or two minutes pass), so
    // the card flips to Connected without the admin reloading the page.
    stopPolling();
    let attempts = 0;
    pollTimer = setInterval(async () => {
      attempts += 1;
      await load();
      if (connected || attempts >= 40) {
        stopPolling();
        if (connected) {
          notice = { kind: 'ok', text: $t('settings.email_oauth_connected_notice') };
        }
      }
    }, 3000);
  }

  async function disconnect() {
    const confirmed = await dlg.ask($t('settings.email_oauth_disconnect_confirm'));
    if (!confirmed) return;
    busy = true;
    notice = null;
    stopPolling();
    const res = await api.disconnectEmailOAuth();
    busy = false;
    if (res.success && res.data) {
      applyPayload((res.data as any).oauth);
      authorizationUrl = '';
      clientSecret = '';
      notice = { kind: 'ok', text: $t('settings.email_oauth_disconnected') };
    } else {
      notice = {
        kind: 'error',
        text: res.message || $t('settings.email_oauth_disconnect_error')
      };
    }
  }

  function formatDate(value: string | null): string {
    if (!value) return '';
    const date = new Date(value);
    return isNaN(date.getTime()) ? value : date.toLocaleString();
  }
</script>

<SettingsCard
  title={$t('settings.email_oauth_title')}
  subtitle={$t('settings.email_oauth_subtitle')}
  class={cn('email-oauth-card', klass)}
  data-testid="settings-email-oauth-card"
>
  {#if loading}
    <p class={settingsHint}>{$t('common.loading')}</p>
  {:else}
    {#if loadError}
      <div data-testid="email-oauth-load-error">
        <Alert variant="error">{loadError}</Alert>
      </div>
    {/if}
    {#if notice}
      <div data-testid="email-oauth-notice">
        <Alert variant={notice.kind === 'ok' ? 'success' : 'error'}>{notice.text}</Alert>
      </div>
    {/if}

    <div class={settingsRow} data-testid="email-oauth-status-row">
      <span class={settingsRowLabel}>{$t('settings.email_oauth_status')}</span>
      <span
        class={cn(
          'inline-flex items-center rounded-full px-2.5 py-0.5 text-xs font-semibold',
          connected
            ? 'bg-[var(--hv-success-bg)] text-[var(--hv-success)]'
            : configured
              ? 'bg-[var(--hv-warning-bg)] text-[var(--hv-warning)]'
              : 'bg-[var(--hv-shell-border-soft)] text-[var(--hv-text-secondary)]'
        )}
        data-testid="email-oauth-status"
        data-state={connected ? 'connected' : configured ? 'ready' : 'unconfigured'}
      >
        {connected
          ? $t('settings.email_oauth_connected')
          : configured
            ? $t('settings.email_oauth_ready')
            : $t('settings.email_oauth_unconfigured')}
      </span>
    </div>

    {#if connected && connectedAt}
      <p class={settingsRowHint} data-testid="email-oauth-connected-at">
        {$t('settings.email_oauth_connected_since', { date: formatDate(connectedAt) })}
      </p>
    {/if}

    {#if lastError}
      <div data-testid="email-oauth-last-error">
        <Alert variant="error">{lastError}</Alert>
      </div>
    {/if}

    <div class={settingsRow}>
      <label class={settingsRowLabel} for="emailOauthProvider">
        {$t('settings.email_oauth_provider')}
      </label>
      <select
        id="emailOauthProvider"
        class={cn(settingsSelect, 'max-w-[280px]')}
        bind:value={provider}
        data-testid="email-oauth-provider"
      >
        <option value="google">{$t('settings.email_oauth_provider_google')}</option>
        <option value="microsoft">{$t('settings.email_oauth_provider_microsoft')}</option>
      </select>
    </div>

    <div class={settingsRowStacked}>
      <label class={settingsRowLabel} for="emailOauthClientId">
        {$t('settings.email_oauth_client_id')}
      </label>
      <input
        id="emailOauthClientId"
        class={cn(settingsInput, 'max-w-[420px]')}
        type="text"
        bind:value={clientId}
        autocomplete="off"
        spellcheck="false"
        aria-describedby="emailOauthClientIdHint"
        data-testid="email-oauth-client-id"
      />
      <p class={settingsRowHint} id="emailOauthClientIdHint">
        {$t('settings.email_oauth_client_id_hint')}
      </p>
    </div>

    <div class={settingsRowStacked}>
      <label class={settingsRowLabel} for="emailOauthClientSecret">
        {$t('settings.email_oauth_client_secret')}
      </label>
      <input
        id="emailOauthClientSecret"
        class={cn(settingsInput, 'max-w-[420px]')}
        type="password"
        bind:value={clientSecret}
        autocomplete="new-password"
        placeholder={configured
          ? $t('settings.email_oauth_secret_keep')
          : $t('settings.email_oauth_secret_placeholder')}
        aria-describedby="emailOauthClientSecretHint"
        data-testid="email-oauth-client-secret"
      />
      <p class={settingsRowHint} id="emailOauthClientSecretHint">
        {$t('settings.email_oauth_client_secret_hint')}
      </p>
    </div>

    <div class={settingsRowStacked}>
      <label class={settingsRowLabel} for="emailOauthRedirect">
        {$t('settings.email_oauth_redirect')}
      </label>
      <input
        id="emailOauthRedirect"
        class={cn(settingsInput, 'max-w-[420px]')}
        type="url"
        bind:value={redirectUrl}
        spellcheck="false"
        aria-describedby="emailOauthRedirectHint"
        data-testid="email-oauth-redirect"
      />
      <p class={settingsRowHint} id="emailOauthRedirectHint">
        {$t('settings.email_oauth_redirect_hint')}
      </p>
    </div>

    <p class={settingsHint} data-testid="email-oauth-provider-hint">
      {$t('settings.email_oauth_provider_hint', {
        host: smtpHost,
        port: String(smtpPort)
      })}
    </p>

    {#if authorizationUrl && !connected}
      <p class={settingsHint}>
        <a
          href={authorizationUrl}
          target="_blank"
          rel="noopener noreferrer"
          class="underline text-[var(--hv-primary)]"
          data-testid="email-oauth-authorize-link"
        >
          {$t('settings.email_oauth_open_authorization')}
        </a>
      </p>
    {/if}

    <div class={settingsCardFooter}>
      <button
        class={cn(settingsBtn, settingsBtnPrimary)}
        onclick={connect}
        disabled={busy}
        data-testid="email-oauth-connect"
      >
        {connected ? $t('settings.email_oauth_reconnect') : $t('settings.email_oauth_connect')}
      </button>
      <button
        class={cn(settingsBtn, settingsBtnGhost)}
        onclick={save}
        disabled={busy}
        data-testid="email-oauth-save"
      >
        {$t('common.save')}
      </button>
      {#if connected || configured}
        <button
          class={cn(settingsBtn, settingsBtnGhost)}
          onclick={disconnect}
          disabled={busy}
          data-testid="email-oauth-disconnect"
        >
          {$t('settings.email_oauth_disconnect')}
        </button>
      {/if}
    </div>
  {/if}
</SettingsCard>

<ConfirmDialog
  {dlg}
  onconfirm={() => dlg.settle(true)}
  oncancel={() => dlg.settle(false)}
/>

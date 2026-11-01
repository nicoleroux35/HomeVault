<!-- ========================================================================
     File          : SyslogCard.svelte
     Author        : Nicolas Le Roux
     Created       : 31/10/2026
     Last modified : 31/10/2026 by Nicolas Le Roux
     Version       : 1.0.0

     Description:
       Settings card for remote syslog forwarding: enable, destination
       (protocol/host/port), RFC 5424 or 3164, facility, the pinned CA for
       TLS, live queue counters, and the "send a test message" probe.

     Dependencies:
       - svelte (>= 5), $lib/services/api, $lib/stores/i18n,
         ./SettingsCard, ./vocabulary, theme/Switch

     Usage:
       Embedded in routes/Settings.svelte's `monitoring` category. See
       docs/guides/journal.md.
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
    settingsBtnPrimary
  } from './vocabulary';

  let { class: klass = '' }: { class?: string } = $props();

  let loading = $state(true);
  let loadError = $state('');
  let notice = $state<{ kind: 'ok' | 'error'; text: string } | null>(null);
  let busy = $state(false);

  let enabled = $state(false);
  let protocol = $state<'udp' | 'tcp' | 'tls'>('udp');
  let host = $state('');
  let port = $state(514);
  let format = $state<'rfc5424' | 'rfc3164'>('rfc5424');
  let facility = $state(1);
  let caPem = $state('');
  let includeAppEvents = $state(true);

  let status = $state('disabled');
  let lastSentAt = $state<string | null>(null);
  let lastError = $state<string | null>(null);
  let queue = $state({ pending: 0, enqueued: 0, dropped: 0, sent: 0, failed: 0 });

  const needsCa = $derived(protocol === 'tls');

  function applyPayload(d: any) {
    enabled = !!d.enabled;
    const p = d.protocol;
    protocol = p === 'tcp' || p === 'tls' ? p : 'udp';
    host = d.host ?? '';
    port = typeof d.port === 'number' ? d.port : 514;
    format = d.format === 'rfc3164' ? 'rfc3164' : 'rfc5424';
    facility = typeof d.facility === 'number' ? d.facility : 1;
    caPem = d.ca_pem ?? '';
    includeAppEvents = d.include_app_events !== false;
    status = d.status ?? 'disabled';
    lastSentAt = d.last_sent_at ?? null;
    lastError = d.last_error ?? null;
    const q = d.queue ?? {};
    queue = {
      pending: q.pending ?? 0,
      enqueued: q.enqueued ?? 0,
      dropped: q.dropped ?? 0,
      sent: q.sent ?? 0,
      failed: q.failed ?? 0
    };
  }

  async function load() {
    loading = true;
    loadError = '';
    const res = await api.getSyslogSettings();
    if (res.success && res.data) applyPayload(res.data);
    else loadError = res.message || $t('settings.syslog_load_error');
    loading = false;
  }

  onMount(load);

  async function save() {
    busy = true;
    notice = null;
    const res = await api.setSyslogSettings({
      enabled,
      protocol,
      host: host.trim(),
      port: Number(port),
      format,
      facility: Number(facility),
      ca_pem: caPem,
      include_app_events: includeAppEvents
    });
    if (res.success) {
      const d = (res.data as any)?.settings;
      if (d) applyPayload(d);
      notice = { kind: 'ok', text: $t('settings.syslog_saved') };
    } else {
      // The server names the field that is wrong; a generic "save failed"
      // would leave the admin guessing which of five fields to fix.
      notice = { kind: 'error', text: res.message || $t('settings.syslog_save_error') };
    }
    busy = false;
  }

  async function sendTest() {
    busy = true;
    notice = null;
    const res = await api.testSyslog();
    if (res.success) {
      notice = { kind: 'ok', text: $t('settings.syslog_test_ok') };
    } else {
      notice = { kind: 'error', text: res.message || $t('settings.syslog_test_error') };
    }
    // The failure (or the success) is now part of the server-side status.
    await load();
    busy = false;
  }

  function stateLabel(value: string): string {
    switch (value) {
      case 'idle':
        return $t('settings.syslog_state_idle');
      case 'forwarding':
        return $t('settings.syslog_state_forwarding');
      case 'degraded':
        return $t('settings.syslog_state_degraded');
      default:
        return $t('settings.syslog_state_disabled');
    }
  }
</script>

<SettingsCard
  title={$t('settings.syslog_title')}
  subtitle={$t('settings.syslog_subtitle')}
  class={cn('syslog-card', klass)}
>
  {#if loading}
    <p class={settingsHint}>{$t('common.loading')}</p>
  {:else}
    {#if loadError}
      <div class="syslog-alert syslog-alert-error" role="alert">{loadError}</div>
    {/if}
    {#if notice}
      <div
        class={cn('syslog-alert', notice.kind === 'ok' ? 'syslog-alert-ok' : 'syslog-alert-error')}
        role={notice.kind === 'ok' ? 'status' : 'alert'}
        data-testid="syslog-notice"
      >
        {notice.text}
      </div>
    {/if}

    <div class={settingsRow}>
      <div class="min-w-0">
        <span id="syslog-enable-label" class={settingsRowLabel}>{$t('settings.syslog_enable')}</span>
        <p class={settingsRowHint}>{$t('settings.syslog_enable_hint')}</p>
      </div>
      <Switch
        bind:checked={enabled}
        ariaLabelledby="syslog-enable-label"
        accent="var(--hv-settings-cat-monitoring)"
      />
    </div>

    <div class={settingsRow} data-testid="syslog-status-row">
      <span class={settingsRowLabel}>{$t('settings.syslog_status')}</span>
      <span
        class={cn('syslog-badge', `syslog-badge-${status}`)}
        data-testid="syslog-status"
        data-state={status}
      >
        {stateLabel(status)}
      </span>
    </div>

    {#if lastError}
      <div class="syslog-alert syslog-alert-error" role="alert" data-testid="syslog-last-error">
        {lastError}
      </div>
    {/if}

    <div class={settingsRow}>
      <label class={settingsRowLabel} for="syslog-protocol">{$t('settings.syslog_protocol')}</label>
      <select
        id="syslog-protocol"
        class={cn(settingsSelect, 'syslog-select')}
        bind:value={protocol}
        data-testid="syslog-protocol"
      >
        <option value="udp">UDP</option>
        <option value="tcp">TCP</option>
        <option value="tls">TLS</option>
      </select>
    </div>

    <div class={settingsRow}>
      <div class="min-w-0">
        <label class={settingsRowLabel} for="syslog-host">{$t('settings.syslog_host')}</label>
        <p class={settingsRowHint}>{$t('settings.syslog_host_hint')}</p>
      </div>
      <input
        id="syslog-host"
        class={cn(settingsInput, 'syslog-input')}
        type="text"
        bind:value={host}
        placeholder="192.168.1.20"
        data-testid="syslog-host"
      />
    </div>

    <div class={settingsRow}>
      <label class={settingsRowLabel} for="syslog-port">{$t('settings.syslog_port')}</label>
      <input
        id="syslog-port"
        class={cn(settingsInput, 'syslog-input')}
        type="number"
        min="1"
        max="65535"
        bind:value={port}
        data-testid="syslog-port"
      />
    </div>

    <div class={settingsRow}>
      <label class={settingsRowLabel} for="syslog-format">{$t('settings.syslog_format')}</label>
      <select
        id="syslog-format"
        class={cn(settingsSelect, 'syslog-select')}
        bind:value={format}
        data-testid="syslog-format"
      >
        <option value="rfc5424">RFC 5424</option>
        <option value="rfc3164">RFC 3164</option>
      </select>
    </div>

    <div class={settingsRow}>
      <div class="min-w-0">
        <label class={settingsRowLabel} for="syslog-facility">{$t('settings.syslog_facility')}</label>
        <p class={settingsRowHint}>{$t('settings.syslog_facility_hint')}</p>
      </div>
      <input
        id="syslog-facility"
        class={cn(settingsInput, 'syslog-input')}
        type="number"
        min="0"
        max="23"
        bind:value={facility}
        data-testid="syslog-facility"
      />
    </div>

    {#if needsCa}
      <div class="settings-row-stacked">
        <label class={settingsRowLabel} for="syslog-ca">{$t('settings.syslog_ca')}</label>
        <p class={settingsRowHint}>{$t('settings.syslog_ca_hint')}</p>
        <textarea
          id="syslog-ca"
          class="syslog-ca mt-1 min-h-24 w-full font-mono text-xs p-2 rounded-lg border border-[var(--hv-settings-border)] bg-[var(--hv-settings-tile)] text-[var(--hv-settings-title)]"
          rows="5"
          spellcheck="false"
          bind:value={caPem}
          data-testid="syslog-ca"
        ></textarea>
      </div>
    {/if}

    <div class={settingsRow}>
      <div class="min-w-0">
        <span id="syslog-include-app-label" class={settingsRowLabel}>
          {$t('settings.syslog_include_app')}
        </span>
        <p class={settingsRowHint}>{$t('settings.syslog_include_app_hint')}</p>
      </div>
      <Switch
        bind:checked={includeAppEvents}
        ariaLabelledby="syslog-include-app-label"
        accent="var(--hv-settings-cat-monitoring)"
      />
    </div>

    <div class="settings-info-rows flex flex-col" data-testid="syslog-queue">
      <div class="settings-info-row">
        <span class="settings-info-label">{$t('settings.syslog_pending')}</span>
        <span class="settings-info-value" data-testid="syslog-pending">{queue.pending}</span>
      </div>
      <div class="settings-info-row">
        <span class="settings-info-label">{$t('settings.syslog_sent')}</span>
        <span class="settings-info-value" data-testid="syslog-sent">{queue.sent}</span>
      </div>
      <div class="settings-info-row">
        <span class="settings-info-label">{$t('settings.syslog_dropped')}</span>
        <span class="settings-info-value" data-testid="syslog-dropped">
          {queue.dropped}{#if queue.failed > 0} · {$t('settings.syslog_failed', { count: String(queue.failed) })}{/if}
        </span>
      </div>
      <div class="settings-info-row">
        <span class="settings-info-label">{$t('settings.syslog_last_sent')}</span>
        <span class="settings-info-value" data-testid="syslog-last-sent">
          {lastSentAt ? new Date(lastSentAt).toLocaleString() : '—'}
        </span>
      </div>
    </div>

    <div class="syslog-actions">
      <button
        class={cn(settingsBtn, settingsBtnPrimary)}
        onclick={save}
        disabled={busy}
        data-testid="syslog-save"
      >
        {$t('common.save')}
      </button>
      <button
        class={cn(settingsBtn, settingsBtnGhost)}
        onclick={sendTest}
        disabled={busy || !enabled}
        data-testid="syslog-test"
      >
        {$t('settings.syslog_test')}
      </button>
      <button
        class={cn(settingsBtn, settingsBtnGhost)}
        onclick={load}
        disabled={busy}
        data-testid="syslog-refresh"
      >
        {$t('common.refresh')}
      </button>
    </div>
  {/if}
</SettingsCard>

<style>
  .syslog-input {
    max-width: 200px;
  }

  .syslog-select {
    width: auto;
    min-width: 160px;
  }

  /* The status is a word and a pill: colour alone never carries the meaning
     (WCAG 1.4.1) — `degraded` also has its own label. */
  .syslog-badge {
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

  .syslog-badge-forwarding {
    background: var(--hv-success-bg);
    border-color: var(--hv-success);
    color: var(--hv-success-text);
  }

  .syslog-badge-idle {
    background: var(--hv-warning-light);
    border-color: var(--hv-warning);
    color: var(--hv-warning);
  }

  .syslog-badge-degraded {
    background: var(--hv-error-light);
    border-color: var(--hv-error);
    color: var(--hv-error);
  }

  .syslog-alert {
    margin: 0 0 12px;
    padding: var(--hv-space-2-5) var(--hv-space-3);
    border-radius: var(--hv-radius-lg);
    font-size: var(--hv-font-size-sm);
  }

  .syslog-alert-ok {
    background: var(--hv-success-bg);
    color: var(--hv-success-text);
  }

  .syslog-alert-error {
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

  .syslog-actions {
    display: flex;
    flex-wrap: wrap;
    gap: 10px;
    padding: 12px 0;
  }
</style>

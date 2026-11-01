<!-- ========================================================================
     File          : AutoBlockCard.svelte
     Author        : Nicolas Le Roux
     Created       : 31/10/2026
     Last modified : 31/10/2026 by Nicolas Le Roux
     Version       : 1.0.0

     Description:
       Settings card for the login auto-block: the policy (threshold, window,
       ban duration, optional firewall rule), the live list of blocked
       addresses with a one-click Pardon, and the DoS request-rate row.

     Dependencies:
       - svelte (>= 5), $lib/services/api, $lib/stores/i18n,
         ./SettingsCard, ./vocabulary, theme/Switch

     Usage:
       Embedded in routes/Settings.svelte's `security` category. See
       docs/guides/security.md.
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
    settingsBtnGhost,
    settingsBtnPrimary,
    SECURITY_ACCENT
  } from './vocabulary';

  interface Ban {
    ip: string;
    reason: string;
    failures: number;
    created_at: string;
    expires_at: string;
    firewall_rule: boolean;
    active: boolean;
    remaining_minutes: number;
  }

  let { class: klass = '' }: { class?: string } = $props();

  let loading = $state(true);
  let loadError = $state('');
  let notice = $state<{ kind: 'ok' | 'error'; text: string } | null>(null);
  let busy = $state(false);

  let enabled = $state(false);
  let threshold = $state(5);
  let windowSeconds = $state(300);
  let banSeconds = $state(1800);
  let applyFirewall = $state(false);
  let dosEnabled = $state(true);
  let dosRpm = $state(600);

  let bans = $state<Ban[]>([]);
  let active = $state(0);
  let firewallRules = $state(0);
  let firewallAvailable = $state(false);
  let limits = $state({
    min_threshold: 1,
    max_threshold: 100,
    max_window_seconds: 86400,
    max_ban_seconds: 31536000,
    max_dos_requests_per_minute: 1000000
  });

  function applyState(d: any) {
    const s = d.settings ?? {};
    enabled = !!s.enabled;
    threshold = typeof s.threshold === 'number' ? s.threshold : 5;
    windowSeconds = typeof s.window_seconds === 'number' ? s.window_seconds : 300;
    banSeconds = typeof s.ban_seconds === 'number' ? s.ban_seconds : 1800;
    applyFirewall = !!s.apply_firewall;
    dosEnabled = s.dos_enabled !== false;
    dosRpm = typeof s.dos_requests_per_minute === 'number' ? s.dos_requests_per_minute : 600;
    bans = Array.isArray(d.bans) ? d.bans : [];
    active = typeof d.active === 'number' ? d.active : 0;
    firewallRules = typeof d.firewall_rules === 'number' ? d.firewall_rules : 0;
    firewallAvailable = !!d.firewall_available;
    if (d.limits) limits = d.limits;
  }

  async function load() {
    loading = true;
    loadError = '';
    const res = await api.getAutoBlock();
    if (res.success && res.data) applyState(res.data);
    else loadError = res.message || $t('settings.autoblock_load_error');
    loading = false;
  }

  onMount(load);

  async function save() {
    busy = true;
    notice = null;
    const res = await api.setAutoBlock({
      enabled,
      threshold: Number(threshold),
      window_seconds: Number(windowSeconds),
      ban_seconds: Number(banSeconds),
      apply_firewall: applyFirewall,
      dos_enabled: dosEnabled,
      dos_requests_per_minute: Number(dosRpm)
    });
    if (res.success) {
      const d = (res.data as any)?.state;
      if (d) applyState(d);
      notice = { kind: 'ok', text: $t('settings.autoblock_saved') };
    } else {
      // The server names the field that cannot work; repeating it verbatim is
      // the only way an admin can fix it without guessing.
      notice = { kind: 'error', text: res.message || $t('settings.autoblock_save_error') };
    }
    busy = false;
  }

  async function pardon(ban: Ban) {
    busy = true;
    notice = null;
    const res = await api.pardonAutoBlock(ban.ip);
    if (res.success) {
      const d = (res.data as any)?.state;
      if (d) applyState(d);
      notice = { kind: 'ok', text: $t('settings.autoblock_pardoned', { ip: ban.ip }) };
    } else {
      notice = { kind: 'error', text: res.message || $t('settings.autoblock_pardon_error') };
    }
    busy = false;
  }

  async function clearAll() {
    busy = true;
    notice = null;
    const res = await api.clearAutoBlockBans();
    if (res.success) {
      const d = (res.data as any)?.state;
      if (d) applyState(d);
      notice = { kind: 'ok', text: $t('settings.autoblock_cleared') };
    } else {
      notice = { kind: 'error', text: res.message || $t('settings.autoblock_pardon_error') };
    }
    busy = false;
  }

  function minutes(seconds: number): string {
    const m = Math.round(seconds / 60);
    if (m >= 60 && m % 60 === 0) return `${m / 60} h`;
    return `${m} min`;
  }
</script>

<SettingsCard
  title={$t('settings.autoblock_title')}
  subtitle={$t('settings.autoblock_subtitle')}
  class={cn('autoblock-card', klass)}
>
  {#if loading}
    <p class={settingsHint}>{$t('common.loading')}</p>
  {:else}
    {#if loadError}
      <div class="autoblock-alert autoblock-alert-error" role="alert">{loadError}</div>
    {/if}
    {#if notice}
      <div
        class={cn('autoblock-alert', notice.kind === 'ok' ? 'autoblock-alert-ok' : 'autoblock-alert-error')}
        role={notice.kind === 'ok' ? 'status' : 'alert'}
        data-testid="autoblock-notice"
      >
        {notice.text}
      </div>
    {/if}

    <div class={settingsRow}>
      <div class="min-w-0">
        <span id="autoblock-enable-label" class={settingsRowLabel}>{$t('settings.autoblock_enable')}</span>
        <p class={settingsRowHint}>{$t('settings.autoblock_enable_hint')}</p>
      </div>
      <Switch bind:checked={enabled} ariaLabelledby="autoblock-enable-label" accent={SECURITY_ACCENT} />
    </div>

    <div class={settingsRow}>
      <label class={settingsRowLabel} for="autoblock-threshold">
        {$t('settings.autoblock_threshold')}
      </label>
      <input
        id="autoblock-threshold"
        class={cn(settingsInput, 'autoblock-input')}
        type="number"
        min={limits.min_threshold}
        max={limits.max_threshold}
        bind:value={threshold}
        data-testid="autoblock-threshold"
      />
    </div>

    <div class={settingsRow}>
      <div class="min-w-0">
        <label class={settingsRowLabel} for="autoblock-window">{$t('settings.autoblock_window')}</label>
        <p class={settingsRowHint}>{$t('settings.autoblock_window_hint')}</p>
      </div>
      <input
        id="autoblock-window"
        class={cn(settingsInput, 'autoblock-input')}
        type="number"
        min="1"
        max={limits.max_window_seconds}
        step="60"
        bind:value={windowSeconds}
        data-testid="autoblock-window"
      />
    </div>

    <div class={settingsRow}>
      <div class="min-w-0">
        <label class={settingsRowLabel} for="autoblock-ban">{$t('settings.autoblock_ban')}</label>
        <p class={settingsRowHint}>
          {$t('settings.autoblock_ban_hint', { duration: minutes(Number(banSeconds)) })}
        </p>
      </div>
      <input
        id="autoblock-ban"
        class={cn(settingsInput, 'autoblock-input')}
        type="number"
        min="1"
        max={limits.max_ban_seconds}
        step="60"
        bind:value={banSeconds}
        data-testid="autoblock-ban"
      />
    </div>

    <div class={settingsRow}>
      <div class="min-w-0">
        <span id="autoblock-firewall-label" class={settingsRowLabel}>
          {$t('settings.autoblock_firewall')}
        </span>
        <p class={settingsRowHint}>
          {firewallAvailable
            ? $t('settings.autoblock_firewall_hint')
            : $t('settings.autoblock_firewall_unavailable')}
        </p>
      </div>
      <Switch
        bind:checked={applyFirewall}
        ariaLabelledby="autoblock-firewall-label"
        accent={SECURITY_ACCENT}
      />
    </div>

    <div class={settingsRow}>
      <div class="min-w-0">
        <span id="autoblock-dos-label" class={settingsRowLabel}>{$t('settings.autoblock_dos')}</span>
        <p class={settingsRowHint}>{$t('settings.autoblock_dos_hint')}</p>
      </div>
      <Switch bind:checked={dosEnabled} ariaLabelledby="autoblock-dos-label" accent={SECURITY_ACCENT} />
    </div>

    {#if dosEnabled}
      <div class={settingsRow}>
        <div class="min-w-0">
          <label class={settingsRowLabel} for="autoblock-dos-rpm">
            {$t('settings.autoblock_dos_rpm')}
          </label>
          <p class={settingsRowHint}>{$t('settings.autoblock_dos_rpm_hint')}</p>
        </div>
        <input
          id="autoblock-dos-rpm"
          class={cn(settingsInput, 'autoblock-input')}
          type="number"
          min="1"
          max={limits.max_dos_requests_per_minute}
          step="10"
          bind:value={dosRpm}
          data-testid="autoblock-dos-rpm"
        />
      </div>
    {/if}

    <div class="flex flex-wrap gap-2 py-3">
      <button
        class={cn(settingsBtn, settingsBtnPrimary)}
        onclick={save}
        disabled={busy}
        data-testid="autoblock-save"
      >
        {$t('common.save')}
      </button>
      <button
        class={cn(settingsBtn, settingsBtnGhost)}
        onclick={load}
        disabled={busy}
        data-testid="autoblock-refresh"
      >
        {$t('common.refresh')}
      </button>
    </div>

    <div class={settingsRow}>
      <div class="min-w-0">
        <span class={settingsRowLabel}>{$t('settings.autoblock_list')}</span>
        <p class={settingsRowHint}>
          {$t('settings.autoblock_list_hint', {
            active: String(active),
            rules: String(firewallRules)
          })}
        </p>
      </div>
      {#if bans.length > 0}
        <button
          class={cn(settingsBtn, settingsBtnGhost)}
          onclick={clearAll}
          disabled={busy}
          data-testid="autoblock-clear"
        >
          {$t('settings.autoblock_clear_all')}
        </button>
      {/if}
    </div>

    {#if bans.length === 0}
      <p class={settingsHint} data-testid="autoblock-empty">{$t('settings.autoblock_empty')}</p>
    {:else}
      <ul class="m-0 p-0 list-none" data-testid="autoblock-bans">
        {#each bans as ban (ban.ip)}
          <li class="autoblock-row" data-testid="autoblock-ban-row" data-ip={ban.ip}>
            <span class="min-w-0">
              <span class="block text-sm font-medium text-[var(--hv-settings-title)] autoblock-mono">
                {ban.ip}
              </span>
              <span class="block text-xs text-[var(--hv-settings-label)]">
                {ban.active
                  ? $t('settings.autoblock_remaining', { minutes: String(ban.remaining_minutes) })
                  : $t('settings.autoblock_expired')}
                · {ban.reason}{#if ban.firewall_rule}
                  · {$t('settings.autoblock_rule_installed')}{/if}
              </span>
            </span>
            <button
              class={cn(settingsBtn, settingsBtnGhost)}
              disabled={busy}
              onclick={() => pardon(ban)}
              aria-label={$t('settings.autoblock_pardon_ip', { ip: ban.ip })}
              data-testid="autoblock-pardon"
            >
              {$t('settings.autoblock_pardon')}
            </button>
          </li>
        {/each}
      </ul>
    {/if}

    <p class={settingsHint}>{$t('settings.autoblock_hint')}</p>
  {/if}
</SettingsCard>

<style>
  .autoblock-input {
    max-width: 160px;
  }

  .autoblock-mono {
    font-family: var(--hv-font-mono);
  }

  .autoblock-alert {
    margin: 0 0 12px;
    padding: var(--hv-space-2-5) var(--hv-space-3);
    border-radius: var(--hv-radius-lg);
    font-size: var(--hv-font-size-sm);
  }

  .autoblock-alert-ok {
    background: var(--hv-success-bg);
    color: var(--hv-success-text);
  }

  .autoblock-alert-error {
    background: var(--hv-error-light);
    color: var(--hv-error);
  }

  .autoblock-row {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 16px;
    padding: 8px 0;
    border-bottom: 1.45px solid var(--hv-settings-hairline);
  }

  .autoblock-row:last-child {
    border-bottom: none;
  }
</style>

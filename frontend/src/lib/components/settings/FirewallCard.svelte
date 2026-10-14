<!-- ========================================================================
     File          : FirewallCard.svelte
     Author        : Nicolas Le Roux
     Created       : 28/09/2026
     Last modified : 28/09/2026 by Nicolas Le Roux
     Version       : 1.0.0

     Description:
       Settings sub-card for the firewall (ufw): status, rule list with
       mandatory-rule marking, add/delete rules, and the confirm-guarded
       enable/disable (self-lockout protection is server-side).

     Dependencies:
       - svelte (>= 5), $lib/services/api, ./SettingsCard

     Usage:
       Embedded in routes/Settings.svelte's security group.
     ======================================================================== -->
<script lang="ts">
  import { onMount } from 'svelte';
  import { api } from '$lib/services/api';
  import type { FirewallRule } from '$lib/services/api';
  import { t } from '$lib/stores/i18n';
  import { IconFlame, IconX } from '@tabler/icons-svelte';
  import SettingsCard from './SettingsCard.svelte';
  import Switch from '$lib/components/theme/Switch.svelte';
  import { cn } from '$lib/utils.js';
  import { activateOnEnterOrSpace } from '$lib/utils/activateOnEnterOrSpace';
  import {
    settingsRow, settingsRowStacked, settingsRowLabel, settingsHint,
    settingsInput, settingsSelect, settingsBtn, settingsBtnPrimary, settingsBtnGhost, settingsBtnDanger
  } from './vocabulary';

  /* See AntivirusCard: `SettingsCard` accepts a `class` for stacking
     spacing, and this wrapper has to forward it or the parent's `mt-3`
     is a type error. */
  let { class: klass = '' }: { class?: string } = $props();

  let loading = $state(true);
  let available = $state(false);
  let enabled = $state(false);
  let rules: FirewallRule[] = $state([]);
  let toggling = $state(false);

  let showConfirm = $state(false);
  let previewSshPort = $state(22);
  let previewAppPort = $state(8080);
  let confirmChecked = $state(false);

  let addingRule = $state(false);
  let newPort = $state('');
  let newProtocol = $state('tcp');
  let newAction = $state('allow');
  let newSource = $state('');

  async function loadStatus() {
    loading = true;
    const res = await api.getFirewallStatus();
    if (res.success && res.data) {
      const d = res.data as any;
      available = d.available;
      enabled = d.enabled;
      rules = d.rules ?? [];
    }
    loading = false;
  }

  onMount(loadStatus);

  async function openConfirm() {
    const res = await api.getFirewallEnablePreview();
    if (res.success && res.data) {
      const d = res.data as any;
      previewSshPort = d.ssh_port;
      previewAppPort = d.app_port;
    }
    confirmChecked = false;
    showConfirm = true;
  }

  async function handleConfirmEnable() {
    toggling = true;
    const res = await api.enableFirewall();
    toggling = false;
    showConfirm = false;
    if (res.success) {
      loadStatus();
    } else {
    }
  }

  async function handleDisable() {
    toggling = true;
    const res = await api.disableFirewall();
    toggling = false;
    if (res.success) {
      loadStatus();
    } else {
    }
  }

  async function handleAddRule() {
    if (!newPort.trim()) {
      return;
    }
    addingRule = true;
    const res = await api.addFirewallRule({
      port: newPort.trim(),
      protocol: newProtocol,
      action: newAction,
      source: newSource.trim() || undefined
    });
    addingRule = false;
    if (res.success) {
      newPort = '';
      newSource = '';
      loadStatus();
    } else {
    }
  }

  async function handleDeleteRule(rule: FirewallRule) {
    const res = await api.deleteFirewallRule({
      port: rule.port,
      protocol: rule.protocol,
      action: rule.action.split(' ')[0].toLowerCase()
    });
    if (res.success) {
      loadStatus();
    } else {
    }
  }
</script>

<!--
  FirewallCard — UFW status, rule list and the guarded enable flow, rebuilt on
  the shared Settings vocabulary. Markup/CSS only: every `api.*` call is
  unchanged and still goes through the server's `ufw_command()` wrapper.

  The enable path keeps its mandatory confirmation modal and the
  `confirmChecked` gate — `enableFirewall()` is only ever reachable through
  `handleConfirmEnable` with the checkbox ticked (the server also requires
  `{"confirmed":true}`, which `api.enableFirewall()` sends).
-->
<SettingsCard icon={IconFlame} iconVariant="orange" title={$t('firewall.title')} subtitle={$t('firewall.subtitle')} class={klass}>
  {#if loading}
    <p class={settingsHint}>{$t('firewall.loading')}</p>
  {:else if !available}
    <div class="fw-warning">{$t('firewall.not_installed')}</div>
  {:else}
    <div class={settingsRow}>
      <span class={settingsRowLabel}>{$t('firewall.status')}</span>
      <span class="flex items-center gap-2">
        {#if enabled}
          <span class="settings-badge settings-badge-success">{$t('firewall.status_active')}</span>
        {:else}
          <span class="settings-badge settings-badge-warning">{$t('firewall.status_inactive')}</span>
        {/if}
      </span>
    </div>

    <div class="flex items-center justify-end py-3">
      {#if enabled}
        <button class={cn(settingsBtn, settingsBtnDanger)} onclick={handleDisable} disabled={toggling}>
          {$t('firewall.disable')}
        </button>
      {:else}
        <button class={cn(settingsBtn, settingsBtnPrimary)} onclick={openConfirm} disabled={toggling}>
          {$t('firewall.enable')}
        </button>
      {/if}
    </div>

    <div class={settingsRowStacked}>
      <h3 class="fw-subheading">{$t('firewall.rules')}</h3>
      <div class="fw-add-row">
        <input class={cn(settingsInput, 'fw-port-input w-[90px] flex-none')} type="text" bind:value={newPort} placeholder="443" />
        <select class={cn(settingsSelect, 'fw-proto-select w-[90px] flex-none')} bind:value={newProtocol}>
          <option value="tcp">TCP</option>
          <option value="udp">UDP</option>
          <option value="any">{$t('firewall.any_protocol')}</option>
        </select>
        <select class={cn(settingsSelect, 'fw-action-select w-[100px] flex-none')} bind:value={newAction}>
          <option value="allow">{$t('firewall.allow')}</option>
          <option value="deny">{$t('firewall.deny')}</option>
        </select>
        <input class={cn(settingsInput, 'fw-source-input flex-1 min-w-[140px]')} type="text" bind:value={newSource} placeholder={$t('firewall.source_optional')} />
        <button class={cn(settingsBtn, settingsBtnPrimary)} onclick={handleAddRule} disabled={addingRule}>
          {$t('firewall.add_rule')}
        </button>
      </div>

      {#each rules as rule (rule.number)}
        <div class="fw-list-row">
          <div class="fw-list-main">
            <span class="fw-rule-port">{rule.port}{rule.protocol !== 'any' ? `/${rule.protocol}` : ''}</span>
            <span
              class="settings-badge"
              class:settings-badge-success={rule.action.startsWith('ALLOW')}
              class:settings-badge-error={!rule.action.startsWith('ALLOW')}
            >
              {rule.action}
            </span>
            <span class={settingsHint}>{rule.source || $t('firewall.anywhere')}</span>
            {#if rule.is_mandatory}
              <span class="settings-badge settings-badge-required">{$t('firewall.required')}</span>
            {/if}
          </div>
          {#if !rule.is_mandatory}
            <button class={cn(settingsBtn, settingsBtnDanger)} onclick={() => handleDeleteRule(rule)}>
              {$t('firewall.remove')}
            </button>
          {/if}
        </div>
      {:else}
        <p class={settingsHint}>{$t('firewall.no_rules')}</p>
      {/each}
    </div>
  {/if}
</SettingsCard>

{#if showConfirm}
  <div class="settings-modal-overlay" role="button" tabindex="0" onclick={() => (showConfirm = false)} onkeydown={(e) => activateOnEnterOrSpace(e, () => (showConfirm = false))}>
    <div class="settings-modal" role="presentation" onclick={(e) => e.stopPropagation()}>
      <div class="settings-modal-header">
        <h3>{$t('firewall.confirm_enable_title')}</h3>
        <button class="settings-modal-close hv-ic" onclick={() => (showConfirm = false)} aria-label={$t('common.close')}>
          <IconX size={20} />
        </button>
      </div>
      <div class="settings-modal-body">
        <p>{$t('firewall.confirm_enable_warning')}</p>
        <ul class="fw-preview-list">
          <li>{$t('firewall.confirm_ssh_rule', { port: String(previewSshPort) })}</li>
          <li>{$t('firewall.confirm_app_rule', { port: String(previewAppPort) })}</li>
        </ul>
        <div class="mt-3">
          <Switch bind:checked={confirmChecked} label={$t('firewall.confirm_checkbox_label')} />
        </div>
      </div>
      <div class="fw-modal-footer">
        <button class={cn(settingsBtn, settingsBtnGhost)} onclick={() => (showConfirm = false)}>{$t('firewall.cancel')}</button>
        <button class={cn(settingsBtn, settingsBtnPrimary)} onclick={handleConfirmEnable} disabled={!confirmChecked || toggling}>
          {$t('firewall.confirm_enable')}
        </button>
      </div>
    </div>
  </div>
{/if}

<style>
  .fw-subheading {
    margin: 0;
    font-size: var(--hv-font-size-sm);
    font-weight: var(--hv-font-weight-semibold);
    color: var(--hv-settings-title);
  }

  .fw-add-row {
    display: flex;
    gap: 8px;
    flex-wrap: wrap;
    align-items: center;
  }

  .fw-list-row {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 12px;
    padding: var(--hv-space-2-5) var(--hv-space-3);
    border: 1.45px solid var(--hv-settings-hairline);
    border-radius: var(--hv-radius-lg);
  }

  .fw-list-main {
    display: flex;
    align-items: center;
    gap: var(--hv-space-2-5);
    flex-wrap: wrap;
  }

  .fw-rule-port {
    font-family: var(--hv-font-mono);
    font-weight: var(--hv-font-weight-semibold);
    color: var(--hv-settings-title);
  }

  .fw-warning {
    padding: var(--hv-space-2-5) var(--hv-space-3);
    border-radius: var(--hv-radius-lg);
    background: var(--hv-warning-bg);
    color: var(--hv-warning-text);
    font-size: var(--hv-font-size-sm);
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

  .settings-badge-warning { background: var(--hv-warning-bg); color: var(--hv-warning-text); }
  .settings-badge-success { background: var(--hv-success-bg); color: var(--hv-success-text); }
  .settings-badge-error { background: var(--hv-error-light); color: var(--hv-error); }
  .settings-badge-required { background: var(--hv-settings-border); color: var(--hv-settings-row); }

  .settings-modal-overlay {
    position: fixed;
    inset: 0;
    z-index: var(--hv-z-modal);
    display: flex;
    align-items: center;
    justify-content: center;
    background: var(--hv-modal-overlay);
  }

  .settings-modal {
    display: flex;
    flex-direction: column;
    width: 90%;
    max-width: 520px;
    overflow: hidden;
    border-radius: var(--hv-radius-xl);
    background: var(--hv-settings-tile);
    box-shadow: var(--hv-modal-shadow);
  }

  .settings-modal-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 16px 20px;
    border-bottom: 1.45px solid var(--hv-settings-border);
    background: var(--hv-settings-hairline);
  }

  .settings-modal-header h3 {
    margin: 0;
    font-size: var(--hv-font-size-md);
    font-weight: var(--hv-font-weight-semibold);
    color: var(--hv-settings-title);
  }

  .settings-modal-close {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 32px;
    height: 32px;
    border: none;
    border-radius: var(--hv-radius-lg);
    background: transparent;
    color: var(--hv-settings-label);
    cursor: pointer;
  }

  .settings-modal-body {
    padding: 20px;
    font-size: var(--hv-font-size-sm);
    color: var(--hv-settings-row);
  }

  .fw-preview-list {
    margin: 12px 0;
    padding-left: 20px;
  }

  .fw-preview-list li {
    margin-bottom: 6px;
  }

  .fw-modal-footer {
    display: flex;
    justify-content: flex-end;
    gap: var(--hv-space-2-5);
    padding: var(--hv-space-3) var(--hv-space-5);
    border-top: 1.45px solid var(--hv-settings-border);
  }
</style>

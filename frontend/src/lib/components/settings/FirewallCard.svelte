<!-- ========================================================================
     File          : FirewallCard.svelte
     Author        : Nicolas Le Roux
     Created       : 28/09/2026
     Last modified : 21/10/2026 by Nicolas Le Roux
     Version       : 1.1.0

     Description:
       Settings sub-card for the firewall (ufw): status, default policy, rule
       list with mandatory-rule marking, add/delete rules, the confirm-guarded
       enable/disable and the recent-change log. Every destructive action
       opens a focus-trapped confirmation dialog that names its consequence;
       the server re-checks all of it (self-lockout protection is server-side).

     Dependencies:
       - svelte (>= 5), $lib/services/api, $lib/utils/focusTrap, ./SettingsCard

     Usage:
       Embedded in routes/Settings.svelte's security group.
     ======================================================================== -->
<script lang="ts">
  import { onMount } from 'svelte';
  import { api } from '$lib/services/api';
  import type {
    FirewallRule,
    FirewallDefaultPolicy,
    FirewallHistoryEntry,
  } from '$lib/services/api';
  import { t } from '$lib/stores/i18n';
  import { IconX } from '@tabler/icons-svelte';
  import SettingsCard from './SettingsCard.svelte';
  import Switch from '$lib/components/theme/Switch.svelte';
  import { cn } from '$lib/utils.js';
  import { focusTrap } from '$lib/utils/focusTrap';
  import {
    settingsRow, settingsRowStacked, settingsRowLabel, settingsHint,
    settingsInput, settingsSelect, settingsBtn, settingsBtnPrimary, settingsBtnGhost, settingsBtnDanger
  } from './vocabulary';

  /* See AntivirusCard: `SettingsCard` accepts a `class` for stacking
     spacing, and this wrapper has to forward it or the parent's `mt-3`
     is a type error. */
  let { class: klass = '' }: { class?: string } = $props();

  /** One dialog at a time, driven by a single focus-trapped overlay. */
  type ModalKind = 'enable' | 'disable' | 'delete' | 'policy' | null;

  let loading = $state(true);
  let available = $state(false);
  let enabled = $state(false);
  let rules = $state<FirewallRule[]>([]);
  let defaultPolicy = $state<FirewallDefaultPolicy>({
    incoming: 'deny',
    outgoing: 'allow',
    routed: 'disabled'
  });
  let history = $state<FirewallHistoryEntry[]>([]);
  let toggling = $state(false);

  let modal = $state<ModalKind>(null);
  let confirmChecked = $state(false);
  let previewSshPorts = $state<number[]>([22]);
  let previewAppPort = $state(8080);
  let previewWarnings = $state<string[]>([]);
  let deleteTarget = $state<FirewallRule | null>(null);

  let policyDirection = $state<'incoming' | 'outgoing'>('incoming');
  let policyValue = $state<'allow' | 'deny' | 'reject'>('deny');

  let addingRule = $state(false);
  let newPort = $state('');
  let newProtocol = $state('tcp');
  let newAction = $state('allow');
  let newSource = $state('');

  /** Visible feedback for every action (rule 5: no silent failure). */
  let feedback = $state<{ kind: 'success' | 'error'; text: string } | null>(null);

  function report(kind: 'success' | 'error', text: string) {
    feedback = { kind, text };
  }

  function errorMessage(res: { message?: string } | null | undefined): string {
    return res?.message?.trim() || $t('firewall.action_failed');
  }

  async function loadStatus() {
    loading = true;
    const res = await api.getFirewallStatus();
    if (res.success && res.data) {
      const d = res.data as any;
      available = d.available;
      enabled = d.enabled;
      rules = d.rules ?? [];
      if (d.default_policy) {
        defaultPolicy = d.default_policy;
      }
    }
    loading = false;
    if (available) {
      await loadHistory();
    }
  }

  async function loadHistory() {
    const res = await api.getFirewallHistory(10);
    if (res.success && res.data) {
      history = (res.data as any).entries ?? [];
    }
  }

  onMount(loadStatus);

  /** The rule as the admin sees it: port/protocol, or "any port". */
  function ruleLabel(rule: FirewallRule): string {
    if (!rule.port) {
      return $t('firewall.any_port');
    }
    return rule.protocol !== 'any' ? `${rule.port}/${rule.protocol}` : rule.port;
  }

  function closeModal() {
    modal = null;
    deleteTarget = null;
    confirmChecked = false;
  }

  async function openEnable() {
    const res = await api.getFirewallEnablePreview();
    if (res.success && res.data) {
      const d = res.data as any;
      previewSshPorts = d.ssh_ports ?? [d.ssh_port ?? 22];
      previewAppPort = d.app_port ?? 8080;
      previewWarnings = d.warnings ?? [];
    } else {
      previewSshPorts = [22];
      previewAppPort = 8080;
      previewWarnings = [];
    }
    confirmChecked = false;
    modal = 'enable';
  }

  async function handleEnable() {
    toggling = true;
    const res = await api.enableFirewall();
    toggling = false;
    closeModal();
    if (res.success) {
      report('success', $t('firewall.enabled'));
      await loadStatus();
    } else {
      report('error', errorMessage(res));
    }
  }

  async function handleDisable() {
    toggling = true;
    const res = await api.disableFirewall();
    toggling = false;
    closeModal();
    if (res.success) {
      report('success', $t('firewall.disabled'));
      await loadStatus();
    } else {
      report('error', errorMessage(res));
    }
  }

  async function handleAddRule() {
    if (!newPort.trim()) {
      report('error', $t('firewall.port_required'));
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
      // ufw skips a rule that already exists and exits 0 — say so instead of
      // claiming a change that did not happen.
      const alreadyExists = (res.data as any)?.already_exists === true;
      report('success', alreadyExists ? $t('firewall.rule_exists') : $t('firewall.rule_added'));
      newPort = '';
      newSource = '';
      await loadStatus();
    } else {
      report('error', errorMessage(res));
    }
  }

  function openDelete(rule: FirewallRule) {
    deleteTarget = rule;
    modal = 'delete';
  }

  async function handleDeleteRule() {
    if (!deleteTarget) {
      return;
    }
    toggling = true;
    // Delete by the rule's own live spec: the server looks it up and refuses
    // if it is gone, so a stale page can never remove a different rule.
    const res = await api.deleteFirewallRule({ spec: deleteTarget.spec });
    toggling = false;
    closeModal();
    if (res.success) {
      report('success', $t('firewall.rule_deleted'));
      await loadStatus();
    } else {
      report('error', errorMessage(res));
    }
  }

  function openPolicy() {
    policyDirection = 'incoming';
    policyValue = (defaultPolicy.incoming as any) ?? 'deny';
    modal = 'policy';
  }

  async function handleSavePolicy() {
    toggling = true;
    const res = await api.setFirewallDefaultPolicy({
      direction: policyDirection,
      policy: policyValue
    });
    toggling = false;
    closeModal();
    if (res.success) {
      report('success', $t('firewall.default_policy_saved'));
      await loadStatus();
    } else {
      report('error', errorMessage(res));
    }
  }

  function historyLabel(action: string): string {
    const known = [
      'enabled',
      'disabled',
      'rule_added',
      'rule_deleted',
      'default_policy_changed'
    ];
    return known.includes(action)
      ? $t(`firewall.history_action_${action}`)
      : action;
  }
</script>

<!--
  FirewallCard — UFW status, default policy, rule list, guarded enable/disable
  and the change log, built on the shared Settings vocabulary. Every mutation
  goes through the server's validated `crate::firewall` helpers.

  Destructive actions are confirmed in a focus-trapped dialog (`use:focusTrap`
  moves focus in, cycles Tab, closes on Escape and restores focus to the
  trigger). The dialogs explain the consequence rather than asking "are you
  sure": disabling opens every blocked port, removing a rule stops it applying.
  The server enforces the same gates (a `{"confirmed":true}` body, a 404 for a
  rule that no longer exists, a 403 for a mandatory rule).
-->
<SettingsCard title={$t('firewall.title')} subtitle={$t('firewall.subtitle')} class={klass}>
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

    <div class={settingsRow}>
      <span class={settingsRowLabel}>{$t('firewall.default_policy')}</span>
      <span class="fw-policy">
        <span class="fw-policy-value">
          {$t('firewall.default_policy_summary', {
            incoming: defaultPolicy.incoming,
            outgoing: defaultPolicy.outgoing
          })}
        </span>
        <button class={cn(settingsBtn, settingsBtnGhost)} onclick={openPolicy}>
          {$t('firewall.change')}
        </button>
      </span>
    </div>
    <p class={settingsHint}>{$t('firewall.default_policy_hint')}</p>

    <div class="flex items-center justify-end py-3">
      {#if enabled}
        <button class={cn(settingsBtn, settingsBtnDanger)} onclick={() => (modal = 'disable')} disabled={toggling}>
          {$t('firewall.disable')}
        </button>
      {:else}
        <button class={cn(settingsBtn, settingsBtnPrimary)} onclick={openEnable} disabled={toggling}>
          {$t('firewall.enable')}
        </button>
      {/if}
    </div>

    <div class={settingsRowStacked}>
      <h3 class="fw-subheading">{$t('firewall.rules')}</h3>
      <div class="fw-add-row">
        <label class="sr-only" for="fw-new-port">{$t('firewall.port_label')}</label>
        <input
          id="fw-new-port"
          class={cn(settingsInput, 'fw-port-input w-[110px] flex-none')}
          type="text"
          bind:value={newPort}
          placeholder={$t('firewall.port_placeholder')}
        />
        <label class="sr-only" for="fw-new-protocol">{$t('firewall.protocol_label')}</label>
        <select id="fw-new-protocol" class={cn(settingsSelect, 'fw-proto-select w-[90px] flex-none')} bind:value={newProtocol}>
          <option value="tcp">TCP</option>
          <option value="udp">UDP</option>
          <option value="any">{$t('firewall.any_protocol')}</option>
        </select>
        <label class="sr-only" for="fw-new-action">{$t('firewall.action_label')}</label>
        <select id="fw-new-action" class={cn(settingsSelect, 'fw-action-select w-[100px] flex-none')} bind:value={newAction}>
          <option value="allow">{$t('firewall.allow')}</option>
          <option value="deny">{$t('firewall.deny')}</option>
          <option value="reject">{$t('firewall.policy_reject')}</option>
          <option value="limit">{$t('firewall.limit')}</option>
        </select>
        <label class="sr-only" for="fw-new-source">{$t('firewall.source_label')}</label>
        <input
          id="fw-new-source"
          class={cn(settingsInput, 'fw-source-input flex-1 min-w-[140px]')}
          type="text"
          bind:value={newSource}
          placeholder={$t('firewall.source_optional')}
        />
        <button class={cn(settingsBtn, settingsBtnPrimary)} onclick={handleAddRule} disabled={addingRule}>
          {$t('firewall.add_rule')}
        </button>
      </div>

      {#each rules as rule (rule.spec)}
        <div class="fw-list-row">
          <div class="fw-list-main">
            <span class="fw-rule-port">{ruleLabel(rule)}</span>
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
            <button
              class={cn(settingsBtn, settingsBtnDanger)}
              onclick={() => openDelete(rule)}
              aria-label={$t('firewall.remove_rule_aria', { rule: ruleLabel(rule) })}
            >
              {$t('firewall.remove')}
            </button>
          {/if}
        </div>
      {:else}
        <p class={settingsHint}>{$t('firewall.no_rules')}</p>
      {/each}
    </div>

    {#if history.length > 0}
      <details class="fw-history">
        <summary>{$t('firewall.history')}</summary>
        <ul class="fw-history-list">
          {#each history as entry (entry.id)}
            <li>
              <span class="fw-history-action">{historyLabel(entry.action)}</span>
              {#if entry.rule_description}
                <span class="fw-history-detail">{entry.rule_description}</span>
              {/if}
              <span class={settingsHint}>{entry.performed_by} · {entry.created_at}</span>
            </li>
          {/each}
        </ul>
      </details>
    {/if}

    {#if feedback}
      <p
        class="fw-feedback"
        class:fw-feedback-error={feedback.kind === 'error'}
        role="status"
        aria-live="polite"
      >
        {feedback.text}
      </p>
    {/if}
  {/if}
</SettingsCard>

{#if modal}
  <div class="settings-modal-overlay">
    <div
      class="settings-modal"
      role="dialog"
      aria-modal="true"
      aria-labelledby="fw-modal-title"
      tabindex="-1"
      use:focusTrap={{ escape: closeModal }}
    >
      <div class="settings-modal-header">
        <h3 id="fw-modal-title">
          {#if modal === 'enable'}
            {$t('firewall.confirm_enable_title')}
          {:else if modal === 'disable'}
            {$t('firewall.confirm_disable_title')}
          {:else if modal === 'delete'}
            {$t('firewall.confirm_delete_title')}
          {:else}
            {$t('firewall.default_policy')}
          {/if}
        </h3>
        <button class="settings-modal-close hv-ic" onclick={closeModal} aria-label={$t('common.close')}>
          <IconX size={20} />
        </button>
      </div>

      <div class="settings-modal-body">
        {#if modal === 'enable'}
          <p>{$t('firewall.confirm_enable_warning')}</p>
          <ul class="fw-preview-list">
            {#each previewSshPorts as port (port)}
              <li>{$t('firewall.confirm_ssh_rule', { port: String(port) })}</li>
            {/each}
            <li>{$t('firewall.confirm_app_rule', { port: String(previewAppPort) })}</li>
          </ul>
          {#each previewWarnings as warning}
            <p class="fw-modal-warning">{warning}</p>
          {/each}
          <div class="mt-3">
            <Switch bind:checked={confirmChecked} label={$t('firewall.confirm_checkbox_label')} />
          </div>
        {:else if modal === 'disable'}
          <p>{$t('firewall.confirm_disable_warning')}</p>
        {:else if modal === 'delete'}
          <p>{$t('firewall.confirm_delete_warning', { rule: deleteTarget ? ruleLabel(deleteTarget) : '' })}</p>
        {:else}
          <p>{$t('firewall.default_policy_hint')}</p>
          <div class="fw-policy-form">
            <label for="fw-policy-direction">{$t('firewall.direction_label')}</label>
            <select id="fw-policy-direction" class={settingsSelect} bind:value={policyDirection}>
              <option value="incoming">{$t('firewall.direction_incoming')}</option>
              <option value="outgoing">{$t('firewall.direction_outgoing')}</option>
            </select>
            <label for="fw-policy-value">{$t('firewall.policy_label')}</label>
            <select id="fw-policy-value" class={settingsSelect} bind:value={policyValue}>
              <option value="deny">{$t('firewall.policy_deny')}</option>
              <option value="reject">{$t('firewall.policy_reject')}</option>
              <option value="allow">{$t('firewall.policy_allow')}</option>
            </select>
          </div>
          {#if policyValue === 'allow'}
            <p class="fw-modal-warning">{$t('firewall.policy_allow_warning')}</p>
          {/if}
        {/if}
      </div>

      <div class="fw-modal-footer">
        <button class={cn(settingsBtn, settingsBtnGhost)} onclick={closeModal}>
          {$t('firewall.cancel')}
        </button>
        {#if modal === 'enable'}
          <button class={cn(settingsBtn, settingsBtnPrimary)} onclick={handleEnable} disabled={!confirmChecked || toggling}>
            {$t('firewall.confirm_enable')}
          </button>
        {:else if modal === 'disable'}
          <button class={cn(settingsBtn, settingsBtnDanger)} onclick={handleDisable} disabled={toggling}>
            {$t('firewall.confirm_disable')}
          </button>
        {:else if modal === 'delete'}
          <button class={cn(settingsBtn, settingsBtnDanger)} onclick={handleDeleteRule} disabled={toggling}>
            {$t('firewall.confirm_delete')}
          </button>
        {:else}
          <button class={cn(settingsBtn, settingsBtnPrimary)} onclick={handleSavePolicy} disabled={toggling}>
            {$t('firewall.save')}
          </button>
        {/if}
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

  .fw-policy {
    display: flex;
    align-items: center;
    gap: var(--hv-space-2-5);
    flex-wrap: wrap;
    justify-content: flex-end;
  }

  .fw-policy-value {
    font-family: var(--hv-font-mono);
    font-size: var(--hv-font-size-sm);
    color: var(--hv-settings-row);
  }

  .fw-policy-form {
    display: grid;
    grid-template-columns: auto 1fr;
    gap: 8px 12px;
    align-items: center;
    margin-top: 12px;
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

  .fw-warning,
  .fw-feedback {
    padding: var(--hv-space-2-5) var(--hv-space-3);
    border-radius: var(--hv-radius-lg);
    background: var(--hv-warning-bg);
    color: var(--hv-warning-text);
    font-size: var(--hv-font-size-sm);
  }

  .fw-feedback {
    background: var(--hv-success-bg);
    color: var(--hv-success-text);
  }

  .fw-feedback-error {
    background: var(--hv-error-light);
    color: var(--hv-error);
  }

  .fw-modal-warning {
    margin: 12px 0 0;
    padding: var(--hv-space-2-5) var(--hv-space-3);
    border-radius: var(--hv-radius-lg);
    background: var(--hv-warning-bg);
    color: var(--hv-warning-text);
  }

  .fw-history {
    margin-top: var(--hv-space-3);
    font-size: var(--hv-font-size-sm);
    color: var(--hv-settings-row);
  }

  .fw-history summary {
    cursor: pointer;
    font-weight: var(--hv-font-weight-medium);
    color: var(--hv-settings-title);
  }

  .fw-history-list {
    margin: 8px 0 0;
    padding-left: 18px;
  }

  .fw-history-list li {
    margin-bottom: 4px;
  }

  .fw-history-action {
    font-weight: var(--hv-font-weight-medium);
  }

  .fw-history-detail {
    font-family: var(--hv-font-mono);
    margin-left: 6px;
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
    /* A circle, like every other icon-only control on this screen (the AI
       refresh button is a 44px circle). `.hv-ic` on the same element already
       gives it the 44px hit area, so the visible box can stay compact. */
    border-radius: var(--hv-radius-full);
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

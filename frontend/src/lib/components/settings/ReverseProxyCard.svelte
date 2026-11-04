<!-- ========================================================================
     File          : ReverseProxyCard.svelte
     Author        : Nicolas Le Roux
     Created       : 06/11/2026 by Nicolas Le Roux
     Version       : 1.0.0

     Description:
       The generic reverse proxy (phase C2): a list of host → upstream rules,
       with the listener's state above it. Each rule says what name it answers,
       which service it reaches, whether the upstream speaks HTTPS, and whether
       a HomeVault sign-in is required before forwarding.

       Design rules kept here (CLAUDE.md, AGENTS.md):
       - the listener's state is the first thing shown, including a bind
         failure with the reason — a rule that silently goes nowhere is worse
         than no rule;
       - the session gate is on by default and the hint says plainly what
         turning it off means;
       - every control is a 44px target, labelled, and keyboard reachable;
       - the server's own refusal sentence is shown, never a generic failure;
       - delete is a named confirmation, not a silent click.

     Dependencies:
       - svelte (>= 5), $lib/services/api-phase4, $lib/stores/i18n,
         $lib/utils/notify, ./SettingsCard, ./vocabulary, theme/Modal

     Usage:
       Embedded in routes/Settings.svelte's security category, after the local
       DNS card. See docs/guides/reverse-proxy.md.
     ======================================================================== -->
<script lang="ts">
  import { onMount } from 'svelte';
  import { translate } from '$lib/stores/i18n';
  import { notifyError, notifySuccess } from '$lib/utils/notify';
  import {
    createProxyRule,
    deleteProxyRule,
    listProxyRules,
    setProxyRuleEnabled,
    updateProxyRule
  } from '$lib/services/api-phase4';
  import type { ProxyListener, ProxyRule, ProxyRuleInput } from '$lib/types/phase4';
  import SettingsCard from './SettingsCard.svelte';
  import Modal from '$lib/components/theme/Modal.svelte';
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
    settingsBtnDanger
  } from './vocabulary';

  let { class: klass = '' }: { class?: string } = $props();

  let loading = $state(true);
  let saving = $state(false);
  let loadError = $state('');
  let rules = $state<ProxyRule[]>([]);
  let listener = $state<ProxyListener>({ listening: false, port: 80, error: null });
  let maxRules = $state(100);

  let editorOpen = $state(false);
  let editingId = $state<number | null>(null);
  let formError = $state('');
  let form = $state<ProxyRuleInput>(emptyForm());

  let deleteTarget = $state<ProxyRule | null>(null);

  function emptyForm(): ProxyRuleInput {
    return {
      host: '',
      path: '',
      upstream: '',
      tls: false,
      auth_required: true,
      enabled: true
    };
  }

  onMount(load);

  async function load() {
    loading = true;
    loadError = '';
    try {
      const res = await listProxyRules();
      rules = res.rules ?? [];
      listener = res.listener ?? { listening: false, port: 80, error: null };
      maxRules = res.max_rules ?? 100;
    } catch (e) {
      loadError = e instanceof Error ? e.message : translate('settings.reverse_proxy_load_failed');
    } finally {
      loading = false;
    }
  }

  function openCreate() {
    editingId = null;
    form = emptyForm();
    formError = '';
    editorOpen = true;
  }

  function openEdit(rule: ProxyRule) {
    editingId = rule.id;
    form = {
      host: rule.host,
      path: rule.path,
      upstream: rule.upstream,
      tls: rule.tls,
      auth_required: rule.auth_required,
      enabled: rule.enabled
    };
    formError = '';
    editorOpen = true;
  }

  async function submit() {
    if (saving) return;
    saving = true;
    formError = '';
    try {
      if (editingId === null) {
        await createProxyRule({ ...form });
      } else {
        await updateProxyRule(editingId, { ...form });
      }
      editorOpen = false;
      notifySuccess(
        editingId === null
          ? 'settings.reverse_proxy_created'
          : 'settings.reverse_proxy_saved'
      );
      await load();
    } catch (e) {
      formError = e instanceof Error ? e.message : translate('settings.reverse_proxy_save_failed');
    } finally {
      saving = false;
    }
  }

  async function toggleEnabled(rule: ProxyRule) {
    try {
      await setProxyRuleEnabled(rule, !rule.enabled);
      await load();
    } catch (e) {
      notifyError(
        e instanceof Error ? { success: false, message: e.message } : null,
        'settings.reverse_proxy_save_failed'
      );
    }
  }

  async function confirmDelete() {
    const target = deleteTarget;
    if (!target) return;
    deleteTarget = null;
    try {
      await deleteProxyRule(target.id);
      notifySuccess('settings.reverse_proxy_deleted');
      await load();
    } catch (e) {
      notifyError(
        e instanceof Error ? { success: false, message: e.message } : null,
        'settings.reverse_proxy_delete_failed'
      );
    }
  }

  function displayHost(rule: ProxyRule): string {
    return rule.path ? `${rule.host}${rule.path}` : rule.host;
  }
</script>

<SettingsCard
  title={translate('settings.reverse_proxy_title')}
  subtitle={translate('settings.reverse_proxy_subtitle')}
  class={cn('reverse-proxy-card', klass)}
  data-testid="reverse-proxy-card"
>
  {#if loading}
    <p class={settingsHint}>{translate('settings.loading')}</p>
  {:else}
    {#if loadError}
      <div class="proxy-alert proxy-alert-error" role="alert" data-testid="reverse-proxy-error">
        {loadError}
      </div>
    {/if}

    <!-- The listener's own state, first: a rule that cannot be reached is
         worse than no rule at all. -->
    <div
      class="proxy-status"
      class:proxy-status-error={!listener.listening}
      role="status"
      data-testid="reverse-proxy-listener-status"
    >
      {#if listener.listening}
        <span class="proxy-dot" aria-hidden="true"></span>
        {translate('settings.reverse_proxy_listening', { port: String(listener.port) })}
      {:else}
        <span class="proxy-dot proxy-dot-off" aria-hidden="true"></span>
        {translate('settings.reverse_proxy_not_listening', { port: String(listener.port) })}
        {#if listener.error}
          <span class="proxy-status-reason">{listener.error}</span>
        {/if}
      {/if}
    </div>
    <p class={settingsHint}>{translate('settings.reverse_proxy_port_hint')}</p>

    {#if rules.length === 0}
      <p class={settingsHint} data-testid="reverse-proxy-empty">
        {translate('settings.reverse_proxy_empty')}
      </p>
    {:else}
      <ul class="proxy-list" data-testid="reverse-proxy-rules">
        {#each rules as rule (rule.id)}
          <li class="proxy-item" data-testid="reverse-proxy-rule">
            <div class="proxy-item-main">
              <span class="proxy-host" data-testid="reverse-proxy-rule-host">{displayHost(rule)}</span>
              <span class="proxy-arrow" aria-hidden="true">→</span>
              <span class="proxy-upstream" data-testid="reverse-proxy-rule-upstream">
                {rule.tls ? 'https://' : 'http://'}{rule.upstream}
              </span>
              <span class="proxy-badges">
                {#if rule.auth_required}
                  <span class="proxy-badge proxy-badge-on">
                    {translate('settings.reverse_proxy_gate_on')}
                  </span>
                {:else}
                  <span class="proxy-badge proxy-badge-off">
                    {translate('settings.reverse_proxy_gate_off')}
                  </span>
                {/if}
                {#if !rule.enabled}
                  <span class="proxy-badge proxy-badge-off">
                    {translate('settings.reverse_proxy_disabled')}
                  </span>
                {/if}
              </span>
            </div>
            <div class="proxy-actions">
              <button
                type="button"
                class={settingsBtnGhost}
                onclick={() => toggleEnabled(rule)}
                aria-label={translate(
                  rule.enabled
                    ? 'settings.reverse_proxy_disable'
                    : 'settings.reverse_proxy_enable',
                  { host: displayHost(rule) }
                )}
                data-testid="reverse-proxy-toggle"
              >
                {translate(
                  rule.enabled
                    ? 'settings.reverse_proxy_disable'
                    : 'settings.reverse_proxy_enable',
                  { host: displayHost(rule) }
                )}
              </button>
              <button
                type="button"
                class={settingsBtnGhost}
                onclick={() => openEdit(rule)}
                aria-label={translate('settings.reverse_proxy_edit', { host: displayHost(rule) })}
                data-testid="reverse-proxy-edit"
              >
                {translate('settings.reverse_proxy_edit_short')}
              </button>
              <button
                type="button"
                class={settingsBtnDanger}
                onclick={() => (deleteTarget = rule)}
                aria-label={translate('settings.reverse_proxy_delete', { host: displayHost(rule) })}
                data-testid="reverse-proxy-delete"
              >
                {translate('settings.reverse_proxy_delete_short')}
              </button>
            </div>
          </li>
        {/each}
      </ul>
    {/if}

    <div class={settingsRow}>
      <div class="min-w-0">
        <p class={settingsRowHint}>
          {translate('settings.reverse_proxy_count', {
            count: String(rules.length),
            max: String(maxRules)
          })}
        </p>
      </div>
      <button
        type="button"
        class={settingsBtnPrimary}
        onclick={openCreate}
        disabled={rules.length >= maxRules}
        data-testid="reverse-proxy-add"
      >
        {translate('settings.reverse_proxy_add')}
      </button>
    </div>

    <p class={settingsHint}>{translate('settings.reverse_proxy_dns_hint')}</p>
  {/if}
</SettingsCard>

<Modal
  open={editorOpen}
  title={editingId === null
    ? translate('settings.reverse_proxy_add')
    : translate('settings.reverse_proxy_edit_title')}
  size="md"
  onclose={() => (editorOpen = false)}
>
  <div class="proxy-form">
    {#if formError}
      <div class="proxy-alert proxy-alert-error" role="alert" data-testid="reverse-proxy-form-error">
        {formError}
      </div>
    {/if}

    <div class="proxy-field">
      <label class={settingsRowLabel} for="proxy-host">
        {translate('settings.reverse_proxy_host')}
      </label>
      <p class={settingsRowHint} id="proxy-host-hint">
        {translate('settings.reverse_proxy_host_hint')}
      </p>
      <input
        id="proxy-host"
        class={settingsInput}
        bind:value={form.host}
        placeholder="homeassistant.home.lan"
        autocomplete="off"
        autocapitalize="none"
        spellcheck="false"
        aria-describedby="proxy-host-hint"
        data-testid="reverse-proxy-host"
      />
    </div>

    <div class="proxy-field">
      <label class={settingsRowLabel} for="proxy-path">
        {translate('settings.reverse_proxy_path')}
      </label>
      <p class={settingsRowHint} id="proxy-path-hint">
        {translate('settings.reverse_proxy_path_hint')}
      </p>
      <input
        id="proxy-path"
        class={settingsInput}
        bind:value={form.path}
        placeholder="/"
        autocomplete="off"
        autocapitalize="none"
        spellcheck="false"
        aria-describedby="proxy-path-hint"
        data-testid="reverse-proxy-path"
      />
    </div>

    <div class="proxy-field">
      <label class={settingsRowLabel} for="proxy-upstream">
        {translate('settings.reverse_proxy_upstream')}
      </label>
      <p class={settingsRowHint} id="proxy-upstream-hint">
        {translate('settings.reverse_proxy_upstream_hint')}
      </p>
      <input
        id="proxy-upstream"
        class={settingsInput}
        bind:value={form.upstream}
        placeholder="192.168.1.50:8123"
        autocomplete="off"
        autocapitalize="none"
        spellcheck="false"
        aria-describedby="proxy-upstream-hint"
        data-testid="reverse-proxy-upstream"
      />
    </div>

    <label class="proxy-check-row" for="proxy-tls">
      <input id="proxy-tls" type="checkbox" bind:checked={form.tls} data-testid="reverse-proxy-tls" />
      <span>
        <span class={settingsRowLabel}>{translate('settings.reverse_proxy_tls')}</span>
        <span class={settingsRowHint}>{translate('settings.reverse_proxy_tls_hint')}</span>
      </span>
    </label>

    <label class="proxy-check-row" for="proxy-auth">
      <input
        id="proxy-auth"
        type="checkbox"
        bind:checked={form.auth_required}
        data-testid="reverse-proxy-auth"
      />
      <span>
        <span class={settingsRowLabel}>{translate('settings.reverse_proxy_auth')}</span>
        <span class={settingsRowHint}>{translate('settings.reverse_proxy_auth_hint')}</span>
      </span>
    </label>

    <label class="proxy-check-row" for="proxy-enabled">
      <input
        id="proxy-enabled"
        type="checkbox"
        bind:checked={form.enabled}
        data-testid="reverse-proxy-enabled"
      />
      <span>
        <span class={settingsRowLabel}>{translate('settings.reverse_proxy_enabled')}</span>
        <span class={settingsRowHint}>{translate('settings.reverse_proxy_enabled_hint')}</span>
      </span>
    </label>

    <div class="proxy-form-actions">
      <button type="button" class={settingsBtn} onclick={() => (editorOpen = false)}>
        {translate('common.cancel')}
      </button>
      <button
        type="button"
        class={settingsBtnPrimary}
        onclick={submit}
        disabled={saving}
        data-testid="reverse-proxy-save"
      >
        {translate('settings.reverse_proxy_save')}
      </button>
    </div>
  </div>
</Modal>

<Modal
  open={deleteTarget !== null}
  title={translate('settings.reverse_proxy_delete_title')}
  size="sm"
  onclose={() => (deleteTarget = null)}
>
  <div class="proxy-form">
    <p>
      {translate('settings.reverse_proxy_delete_confirm', {
        host: deleteTarget ? displayHost(deleteTarget) : ''
      })}
    </p>
    <div class="proxy-form-actions">
      <button type="button" class={settingsBtn} onclick={() => (deleteTarget = null)}>
        {translate('common.cancel')}
      </button>
      <button
        type="button"
        class={settingsBtnDanger}
        onclick={confirmDelete}
        data-testid="reverse-proxy-delete-confirm"
      >
        {translate('settings.reverse_proxy_delete_short')}
      </button>
    </div>
  </div>
</Modal>

<style>
  .proxy-alert {
    border: 1px solid var(--hv-shell-border);
    border-radius: 8px;
    padding: 10px 12px;
    margin-bottom: 12px;
    font-size: 0.8125rem;
    line-height: 1.25rem;
    color: var(--hv-shell-text);
    background: var(--hv-shell-border-soft);
  }

  .proxy-alert-error {
    border-color: var(--hv-danger, #b42318);
  }

  .proxy-status {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 8px;
    font-size: 0.8125rem;
    line-height: 1.25rem;
    color: var(--hv-settings-label);
    margin-bottom: 4px;
  }

  .proxy-status-error {
    color: var(--hv-danger, #b42318);
  }

  .proxy-status-reason {
    flex-basis: 100%;
    font-size: 0.75rem;
    overflow-wrap: anywhere;
  }

  .proxy-dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: var(--hv-success, #067647);
    flex: none;
  }

  .proxy-dot-off {
    background: var(--hv-danger, #b42318);
  }

  .proxy-list {
    list-style: none;
    margin: 10px 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .proxy-item {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    padding: 8px 10px;
    border: 1px solid var(--hv-shell-border);
    border-radius: 8px;
  }

  .proxy-item-main {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px;
    min-width: 0;
  }

  .proxy-host,
  .proxy-upstream {
    font-family: var(--hv-font-mono, ui-monospace, monospace);
    font-size: 0.8125rem;
    overflow-wrap: anywhere;
  }

  .proxy-arrow {
    color: var(--hv-settings-label);
  }

  .proxy-badges {
    display: flex;
    flex-wrap: wrap;
    gap: 4px;
  }

  .proxy-badge {
    border-radius: 999px;
    padding: 1px 8px;
    font-size: 0.6875rem;
    line-height: 1.25rem;
    border: 1px solid var(--hv-shell-border);
  }

  .proxy-badge-on {
    color: var(--hv-success, #067647);
    border-color: currentColor;
  }

  .proxy-badge-off {
    color: var(--hv-danger, #b42318);
    border-color: currentColor;
  }

  .proxy-actions {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
  }

  .proxy-form {
    display: flex;
    flex-direction: column;
    gap: 14px;
  }

  .proxy-field {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  .proxy-check-row {
    display: flex;
    align-items: flex-start;
    gap: 10px;
    cursor: pointer;
  }

  .proxy-check-row input {
    width: 20px;
    height: 20px;
    margin-top: 2px;
    flex: none;
  }

  .proxy-form-actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
    margin-top: 6px;
  }

  @media (max-width: 640px) {
    .proxy-item {
      align-items: stretch;
    }

    .proxy-actions {
      justify-content: flex-end;
    }
  }
</style>

<!-- ========================================================================
     File          : McpCard.svelte
     Author        : Nicolas Le Roux
     Created       : 19/10/2026
     Last modified : 19/10/2026 by Nicolas Le Roux
     Version       : 1.0.0

     Description:
       Settings card for the external AI-agent (MCP) server: enable
       kill-switch, agent endpoint, global permissions, and token
       mint/revoke with copy-to-clipboard of the one-time plaintext token.
       Admin-only — the card probes `GET /api/mcp/status`, which rejects a
       non-admin session, and renders nothing at all when that probe fails.

     Dependencies:
       - svelte (>= 5), $lib/services/api, $lib/stores/i18n,
         ./SettingsCard, ./vocabulary, theme/Switch

     Usage:
       Embedded in routes/Settings.svelte's `ai` (AI Assistant) category.
     ======================================================================== -->
<script lang="ts">
  import { onMount } from 'svelte';
  import { api } from '$lib/services/api';
  import { t, translate } from '$lib/stores/i18n';
  import { Switch } from '$lib/components/theme';
  import { IconPlus, IconTrash, IconCopy, IconCheck } from '@tabler/icons-svelte';
  import SettingsCard from './SettingsCard.svelte';
  import { cn } from '$lib/utils.js';
  import {
    settingsRow,
    settingsRowStacked,
    settingsRowLabel,
    settingsRowHint,
    settingsHint,
    settingsInput,
    settingsBtn,
    settingsBtnGhost,
    settingsBtnPrimary
  } from './vocabulary';

  /* In-card section heading (permissions / tokens). Not `secLabel` from the
     vocabulary: that one carries the panel-level `pt-6` rhythm, which would
     leave a 24px hole inside the card. */
  const sectionLabel =
    'm-0 text-c-xs font-bold uppercase tracking-[var(--hv-letter-spacing-wider)] text-[var(--hv-settings-label)]';

  /* `SettingsCard` accepts a `class` for stacking spacing; forward it or the
     caller's `mt-3` becomes a type error (same pattern as SnmpCard). */
  let { class: klass = '' }: { class?: string } = $props();

  /** The AI Assistant category accent, so the card matches its rail group. */
  const AI_ACCENT = 'var(--hv-settings-cat-ai)';

  interface McpToken {
    id: string;
    name: string;
    prefix: string;
    created_by: string;
    created_at: string;
    last_used_at: string | null;
  }

  let loading = $state(true);
  let loadError = $state('');
  /* False until `GET /mcp/status` answers 200 — i.e. until the session is
     known to be an admin. Non-admins get the same 403 the endpoint always
     returns, so the card never advertises a gateway they cannot manage. */
  let isAdmin = $state(false);
  let enabled = $state(false);
  let saving = $state(false);
  let notice = $state<{ kind: 'ok' | 'error'; text: string } | null>(null);

  // Permission draft: scope fields are free text ("all" or comma/space
  // separated ids/paths), matching the ScopeList wire shape.
  let apps = $state('');
  let appStore = $state(false);
  let files = $state('');
  let manageSystem = $state(false);
  let vms = $state('');
  let rag = $state(false);

  let tokens = $state<McpToken[]>([]);
  let newTokenName = $state('');
  let creating = $state(false);
  let justCreated = $state('');
  let copied = $state(false);
  let revokingId = $state<string | null>(null);

  function scopeToText(scope: unknown): string {
    if (scope === 'all') return 'all';
    if (Array.isArray(scope)) return scope.join(', ');
    return '';
  }

  function textToScope(text: string): string | string[] {
    const clean = text.trim();
    if (clean === '' || clean.toLowerCase() === 'none') return [];
    if (clean.toLowerCase() === 'all') return 'all';
    return clean.split(/[\s,]+/).filter(Boolean);
  }

  async function load() {
    loading = true;
    loadError = '';
    try {
      const res: any = await api.mcpStatus();
      if (!res?.success) {
        // Not an admin (403) or the endpoint is unreachable: the card simply
        // does not exist for this session.
        isAdmin = false;
        loadError = res?.message || res?.error || translate('mcp.load_failed');
        return;
      }
      isAdmin = true;
      const status = res.data?.status ?? {};
      const perms = status.permissions ?? {};
      enabled = !!status.enabled;
      apps = scopeToText(perms.apps);
      appStore = !!perms.app_store;
      files = scopeToText(perms.files);
      manageSystem = !!perms.manage_system;
      vms = scopeToText(perms.vms);
      rag = !!perms.rag;
      tokens = res.data?.tokens ?? [];
    } catch (e) {
      isAdmin = false;
      loadError = e instanceof Error ? e.message : translate('mcp.load_failed');
    } finally {
      loading = false;
    }
  }

  onMount(load);

  async function toggleEnabled(next: boolean) {
    saving = true;
    notice = null;
    const res: any = await api.mcpSetEnabled(next);
    saving = false;
    if (res?.success) {
      enabled = next;
      notice = { kind: 'ok', text: translate(next ? 'mcp.enabled_now' : 'mcp.disabled_now') };
    } else {
      notice = { kind: 'error', text: res?.message || res?.error || translate('mcp.save_failed') };
    }
  }

  async function savePermissions() {
    saving = true;
    notice = null;
    const res: any = await api.mcpSetPermissions({
      apps: textToScope(apps),
      app_store: appStore,
      files: textToScope(files),
      manage_system: manageSystem,
      vms: textToScope(vms),
      rag,
    });
    saving = false;
    if (res?.success) {
      notice = { kind: 'ok', text: translate('mcp.perms_saved') };
    } else {
      notice = { kind: 'error', text: res?.message || res?.error || translate('mcp.save_failed') };
    }
  }

  async function createToken() {
    const name = newTokenName.trim();
    if (!name) return;
    creating = true;
    notice = null;
    const res: any = await api.mcpCreateToken(name);
    creating = false;
    if (res?.success && res.data?.token) {
      justCreated = res.data.token;
      newTokenName = '';
      await load();
    } else {
      notice = { kind: 'error', text: res?.message || res?.error || translate('mcp.token_failed') };
    }
  }

  async function revokeToken(id: string) {
    revokingId = id;
    const res: any = await api.mcpRevokeToken(id);
    revokingId = null;
    if (res?.success) {
      tokens = tokens.filter((tok) => tok.id !== id);
      notice = { kind: 'ok', text: translate('mcp.token_revoked') };
    } else {
      notice = { kind: 'error', text: res?.message || res?.error || translate('mcp.revoke_failed') };
    }
  }

  async function copyCreated() {
    try {
      await navigator.clipboard.writeText(justCreated);
      copied = true;
      setTimeout(() => (copied = false), 1500);
    } catch {
      // Clipboard unavailable — the value stays visible for manual copy
    }
  }

  const endpoint = `${location.protocol}//${location.host}/mcp`;
</script>

<!--
  McpCard — the external-agent (MCP) gateway configuration, on the Settings
  vocabulary. It used to live as a side panel inside the Chat app
  (`components/chat/McpPanel.svelte`, deleted 2026-10-19): the gateway is a
  server-wide, admin-only configuration, so it belongs next to the AI model
  card in Settings → AI Assistant, not in the chat window.

  Deliberately NOT staged in the shared settings draft (`settingsDraft`):
  the enable switch is a kill-switch and the permissions decide what an
  external agent may touch, so both take effect immediately instead of
  waiting for the rail-footer Save — a deferred save would leave the gateway
  open after the operator switched it off. Tokens are one-time secrets and
  are immediate for the same reason. Every write reports its own outcome in
  the `role="status"` line at the bottom of the card.
-->
{#if isAdmin}
  <SettingsCard
    title={$t('mcp.title')}
    subtitle={$t('mcp.desc')}
    class={klass}
  >
    {#if loading}
      <p class={cn(settingsHint, 'py-3')} role="status">{$t('common.loading')}</p>
    {:else if loadError}
      <p class="m-0 py-3 text-sm text-[var(--hv-error)]" role="alert">{loadError}</p>
    {:else}
      <!-- Kill-switch + agent endpoint -->
      <div class={settingsRow}>
        <div class="min-w-0">
          <span class={settingsRowLabel} id="mcpEnabledLabel">{$t('mcp.enabled')}</span>
          <p class={settingsRowHint}>
            {$t('mcp.endpoint')}:
            <span data-testid="mcp-endpoint" class="font-[family-name:var(--hv-font-mono)] break-all">{endpoint}</span>
          </p>
        </div>
        <Switch
          checked={enabled}
          disabled={saving}
          accent={AI_ACCENT}
          ariaLabelledby="mcpEnabledLabel"
          data-testid="mcp-enabled"
          onchange={toggleEnabled}
        />
      </div>

      <!-- Global permissions -->
      <div class={settingsRowStacked}>
        <span class={sectionLabel} data-testid="mcp-section-permissions">{$t('mcp.permissions')}</span>
        <div class="flex flex-col gap-1.5">
          <label class={settingsRowLabel} for="mcp-perm-apps">{$t('mcp.perm_apps')}</label>
          <input
            id="mcp-perm-apps"
            class={cn(settingsInput, 'max-w-[420px]')}
            type="text"
            placeholder="all"
            disabled={saving}
            bind:value={apps}
          />
          <p class={settingsRowHint}>{$t('mcp.perm_apps_hint')}</p>
        </div>
        <div class={settingsRow}>
          <span class={settingsRowLabel} id="mcpAppStoreLabel">{$t('mcp.perm_app_store')}</span>
          <Switch
            checked={appStore}
            disabled={saving}
            accent={AI_ACCENT}
            ariaLabelledby="mcpAppStoreLabel"
            onchange={(v) => (appStore = v)}
          />
        </div>
        <div class="flex flex-col gap-1.5">
          <label class={settingsRowLabel} for="mcp-perm-files">{$t('mcp.perm_files')}</label>
          <input
            id="mcp-perm-files"
            class={cn(settingsInput, 'max-w-[420px]')}
            type="text"
            placeholder="/data/photos"
            disabled={saving}
            bind:value={files}
          />
          <p class={settingsRowHint}>{$t('mcp.perm_files_hint')}</p>
        </div>
        <div class="flex flex-col gap-1.5">
          <label class={settingsRowLabel} for="mcp-perm-vms">{$t('mcp.perm_vms')}</label>
          <input
            id="mcp-perm-vms"
            class={cn(settingsInput, 'max-w-[420px]')}
            type="text"
            placeholder="all"
            disabled={saving}
            bind:value={vms}
          />
        </div>
        <div class={settingsRow}>
          <span class={settingsRowLabel} id="mcpRagLabel">{$t('mcp.perm_rag')}</span>
          <Switch
            checked={rag}
            disabled={saving}
            accent={AI_ACCENT}
            ariaLabelledby="mcpRagLabel"
            onchange={(v) => (rag = v)}
          />
        </div>
        <div class={settingsRow}>
          <div class="min-w-0">
            <span class={settingsRowLabel} id="mcpRebootLabel">{$t('mcp.perm_reboot')}</span>
            {#if manageSystem}
              <p class={cn(settingsRowHint, 'text-[var(--hv-warning)]')}>{$t('mcp.perm_reboot_warn')}</p>
            {/if}
          </div>
          <Switch
            checked={manageSystem}
            disabled={saving}
            accent={AI_ACCENT}
            ariaLabelledby="mcpRebootLabel"
            onchange={(v) => (manageSystem = v)}
          />
        </div>
        <div class="flex flex-wrap items-center gap-2 pt-1">
          <button class={cn(settingsBtn, settingsBtnPrimary)} disabled={saving} onclick={savePermissions}>
            {$t('common.save')}
          </button>
        </div>
      </div>

      <!-- Tokens -->
      <div class={settingsRowStacked}>
        <span class={sectionLabel} data-testid="mcp-section-tokens">{$t('mcp.tokens')}</span>

        {#if justCreated}
          <div class="p-3 rounded-xl bg-[var(--hv-warning-bg)] border border-[var(--hv-warning)] flex flex-col gap-2">
            <span class="text-xs font-semibold text-[var(--hv-warning)]">{$t('mcp.token_once')}</span>
            <span class="px-2.5 py-2 rounded-lg bg-[var(--hv-settings-tile)] font-[family-name:var(--hv-font-mono)] text-xs break-all text-[var(--hv-settings-title)]">{justCreated}</span>
            <button class={cn(settingsBtn, settingsBtnGhost, 'self-start')} onclick={copyCreated}>
              {#if copied}<IconCheck size={16} stroke={2} /> {$t('chat.copied')}{:else}<IconCopy size={16} stroke={2} /> {$t('chat.copy_message')}{/if}
            </button>
          </div>
        {/if}

        <form
          class="flex flex-col gap-2 @min-[480px]:flex-row @min-[480px]:items-end"
          onsubmit={(e) => { e.preventDefault(); createToken(); }}
        >
          <div class="flex-1 min-w-0 flex flex-col gap-1.5">
            <label class={settingsRowLabel} for="mcp-token-name">{$t('mcp.token_name')}</label>
            <input
              id="mcp-token-name"
              class={settingsInput}
              type="text"
              placeholder={$t('mcp.token_name')}
              disabled={creating}
              bind:value={newTokenName}
            />
          </div>
          <button
            type="submit"
            class={cn(settingsBtn, settingsBtnGhost)}
            disabled={creating || !newTokenName.trim()}
            aria-label={$t('mcp.token_create')}
          >
            <IconPlus size={16} stroke={2} />
            {$t('mcp.token_create')}
          </button>
        </form>

        {#if tokens.length === 0}
          <p class={settingsHint}>{$t('mcp.tokens_empty')}</p>
        {:else}
          <ul class="m-0 p-0 list-none flex flex-col gap-2">
            {#each tokens as tok (tok.id)}
              <li data-testid="mcp-token-row" class="flex items-center gap-3 px-3 py-2 rounded-xl bg-[var(--hv-settings-canvas)] border-[1.45px] border-[var(--hv-settings-border)]">
                <div class="flex-1 min-w-0">
                  <span class="block text-c-label font-semibold text-[var(--hv-settings-title)] truncate">{tok.name}</span>
                  <span class="block font-[family-name:var(--hv-font-mono)] text-xs text-[var(--hv-settings-label)] truncate">{tok.prefix}…{#if tok.last_used_at} · {$t('mcp.token_last_used', { when: tok.last_used_at })}{/if}</span>
                </div>
                <button
                  class={cn(settingsBtn, settingsBtnGhost, 'hv-ic px-3')}
                  disabled={revokingId === tok.id}
                  onclick={() => revokeToken(tok.id)}
                  aria-label={$t('mcp.token_revoke')}
                >
                  <IconTrash size={16} stroke={2} />
                </button>
              </li>
            {/each}
          </ul>
        {/if}
      </div>

      {#if notice}
        <p class={cn('m-0 py-3 text-xs font-semibold', notice.kind === 'ok' ? 'text-[var(--hv-success)]' : 'text-[var(--hv-error)]')} role="status">{notice.text}</p>
      {/if}
    {/if}
  </SettingsCard>
{/if}

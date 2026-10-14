<!-- ========================================================================
     File          : McpPanel.svelte
     Author        : Nicolas Le Roux
     Created       : 13/10/2026
     Last modified : 13/10/2026 by Nicolas Le Roux
     Version       : 1.0.0

     Description:
       In-app management panel for the external AI-agent (MCP) server:
       enable kill-switch, global permissions, and token mint/revoke with
       copy-to-clipboard of the one-time plaintext token.

     Dependencies:
       - svelte (>= 5), theme primitives, stores/i18n, $lib/services/api

     Usage:
       Embedded in the Chat app's sidebar.
     ======================================================================== -->
<script lang="ts">
  import { onMount } from 'svelte';
  import { api } from '$lib/services/api';
  import { t, translate } from '$lib/stores/i18n';
  import { Button, Input, Switch, Spinner } from '$lib/components/theme';
  import { IconX, IconPlus, IconTrash, IconCopy, IconCheck, IconCpu } from '@tabler/icons-svelte';
  import { cn } from '$lib/utils.js';

  interface Props {
    onclose: () => void;
  }
  let { onclose }: Props = $props();

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
        loadError = res?.message || res?.error || translate('mcp.load_failed');
        return;
      }
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

<aside data-testid="mcp-panel" class="flex flex-col flex-none w-[320px] min-h-0 border-l border-[var(--hv-shell-border)] bg-[var(--hv-shell-fill)]" aria-label={$t('mcp.title')}>
  <div class="flex items-center gap-2 px-4 py-3 border-b border-[var(--hv-shell-border)]">
    <span class="grid place-items-center w-8 h-8 rounded-lg bg-[var(--hv-primary-light)] text-[var(--hv-primary)]"><IconCpu size={20} stroke={1.9} /></span>
    <span class="flex-1 text-sm font-bold text-[var(--hv-shell-text-strong)]">{$t('mcp.title')}</span>
    <button class="hv-ic grid place-items-center w-8 h-8 rounded-lg text-[var(--hv-shell-text-2)] hover:bg-[var(--hv-shell-border-soft)]" onclick={onclose} aria-label={$t('common.close')}><IconX size={18} /></button>
  </div>

  <div class="flex-1 min-h-0 overflow-y-auto px-4 py-3 flex flex-col gap-3">
    {#if loading}
      <div class="flex items-center gap-2 text-sm text-[var(--hv-shell-text-2)]" role="status"><Spinner /> {$t('common.loading')}</div>
    {:else if loadError}
      <p class="m-0 text-sm text-[var(--hv-error)]">{loadError}</p>
    {:else}
      <p class="m-0 text-xs text-[var(--hv-shell-text-2)]">{$t('mcp.desc')}</p>
      <p class="m-0 px-2.5 py-2 rounded-lg bg-[var(--hv-shell-border-soft)] font-[family-name:var(--hv-font-mono)] text-xs text-[var(--hv-shell-text-strong)] break-all" title={$t('mcp.endpoint')}>{endpoint}</p>

      <div class="flex items-center gap-2">
        <Switch checked={enabled} disabled={saving} onchange={toggleEnabled} ariaLabel={$t('mcp.enabled')} data-testid="mcp-enabled" />
        <span class="text-sm font-semibold text-[var(--hv-shell-text-strong)]">{$t('mcp.enabled')}</span>
      </div>

      <div class="flex flex-col gap-2.5">
        <span class="text-xs font-bold uppercase tracking-wide text-[var(--hv-shell-text-3)]">{$t('mcp.permissions')}</span>
        <label class="flex flex-col gap-1 text-xs font-semibold text-[var(--hv-shell-text-2)]">{$t('mcp.perm_apps')}
          <Input bind:value={apps} placeholder="all" disabled={saving} />
          <span class="font-normal">{$t('mcp.perm_apps_hint')}</span>
        </label>
        <label class="flex items-center gap-2 text-sm text-[var(--hv-shell-text-strong)] cursor-pointer">
          <Switch checked={appStore} disabled={saving} onchange={(v) => (appStore = v)} ariaLabel={$t('mcp.perm_app_store')} /> {$t('mcp.perm_app_store')}
        </label>
        <label class="flex flex-col gap-1 text-xs font-semibold text-[var(--hv-shell-text-2)]">{$t('mcp.perm_files')}
          <Input bind:value={files} placeholder="/data/photos" disabled={saving} />
          <span class="font-normal">{$t('mcp.perm_files_hint')}</span>
        </label>
        <label class="flex flex-col gap-1 text-xs font-semibold text-[var(--hv-shell-text-2)]">{$t('mcp.perm_vms')}
          <Input bind:value={vms} placeholder="all" disabled={saving} />
        </label>
        <label class="flex items-center gap-2 text-sm text-[var(--hv-shell-text-strong)] cursor-pointer">
          <Switch checked={rag} disabled={saving} onchange={(v) => (rag = v)} ariaLabel={$t('mcp.perm_rag')} /> {$t('mcp.perm_rag')}
        </label>
        <label class="flex items-center gap-2 text-sm text-[var(--hv-shell-text-strong)] cursor-pointer">
          <Switch checked={manageSystem} disabled={saving} onchange={(v) => (manageSystem = v)} ariaLabel={$t('mcp.perm_reboot')} /> {$t('mcp.perm_reboot')}
        </label>
        {#if manageSystem}
          <p class="m-0 text-xs text-[var(--hv-warning)]">{$t('mcp.perm_reboot_warn')}</p>
        {/if}
        <Button variant="primary" disabled={saving} onclick={savePermissions}>{$t('common.save')}</Button>
      </div>

      <div class="flex flex-col gap-2">
        <span class="text-xs font-bold uppercase tracking-wide text-[var(--hv-shell-text-3)]">{$t('mcp.tokens')}</span>
        {#if justCreated}
          <div class="p-2.5 rounded-xl bg-[var(--hv-warning-bg)] border border-[var(--hv-warning)] flex flex-col gap-1.5">
            <span class="text-xs font-semibold text-[var(--hv-warning)]">{$t('mcp.token_once')}</span>
            <span class="px-2 py-1.5 rounded-lg bg-[var(--hv-shell-card)] font-[family-name:var(--hv-font-mono)] text-xs break-all text-[var(--hv-shell-text-strong)]">{justCreated}</span>
            <Button variant="secondary" onclick={copyCreated}>{#if copied}<IconCheck size={15} stroke={2} /> {$t('chat.copied')}{:else}<IconCopy size={15} stroke={2} /> {$t('chat.copy_message')}{/if}</Button>
          </div>
        {/if}
        <form class="flex gap-2" onsubmit={(e) => { e.preventDefault(); createToken(); }}>
          <label class="flex-1 min-w-0 flex flex-col gap-1 text-xs font-semibold text-[var(--hv-shell-text-2)]">{$t('mcp.token_name')}
            <Input bind:value={newTokenName} placeholder={$t('mcp.token_name')} disabled={creating} />
          </label>
          <button type="submit" class="hv-ic self-end grid place-items-center w-11 h-11 flex-none rounded-xl bg-[var(--hv-shell-border-soft)] border border-[var(--hv-shell-border)] text-[var(--hv-shell-text-strong)] cursor-pointer disabled:opacity-40" disabled={creating || !newTokenName.trim()} aria-label={$t('mcp.token_create')}><IconPlus size={20} stroke={2} /></button>
        </form>
        {#if tokens.length === 0}
          <p class="m-0 text-xs text-[var(--hv-shell-text-2)]">{$t('mcp.tokens_empty')}</p>
        {:else}
          {#each tokens as tok (tok.id)}
            <div data-testid="mcp-token-row" class="flex items-center gap-2 px-2.5 py-2 rounded-xl bg-[var(--hv-shell-card)] border border-[var(--hv-shell-border)]">
              <div class="flex-1 min-w-0">
                <span class="block text-sm font-semibold text-[var(--hv-shell-text-strong)] truncate">{tok.name}</span>
                <span class="block font-[family-name:var(--hv-font-mono)] text-c-xs text-[var(--hv-shell-text-2)] truncate">{tok.prefix}…{#if tok.last_used_at} · {$t('mcp.token_last_used', { when: tok.last_used_at })}{/if}</span>
              </div>
              <button class={cn('hv-ic grid place-items-center w-8 h-8 rounded-lg text-[var(--hv-shell-text-2)] hover:text-[var(--hv-error)]')} disabled={revokingId === tok.id} onclick={() => revokeToken(tok.id)} aria-label={$t('mcp.token_revoke')}><IconTrash size={16} /></button>
            </div>
          {/each}
        {/if}
      </div>

      {#if notice}
        <p class={cn('m-0 text-xs font-semibold', notice.kind === 'ok' ? 'text-[var(--hv-success)]' : 'text-[var(--hv-error)]')} role="status">{notice.text}</p>
      {/if}
    {/if}
  </div>
</aside>

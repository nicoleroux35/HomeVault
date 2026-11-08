<!-- ========================================================================
     File          : ConnectedClientsCard.svelte
     Author        : Nicolas Le Roux
     Created       : 31/10/2026
     Last modified : 31/10/2026 by Nicolas Le Roux
     Version       : 1.0.0

     Description:
       Settings card listing who is connected over SMB / NFS / SFTP / FTP,
       with the provenance of each source, what they hold open, and a
       "Disconnect" action for the sessions that can be dropped.

     Dependencies:
       - svelte (>= 5), $lib/services/api, $lib/stores/i18n,
         ./SettingsCard, ./vocabulary

     Usage:
       Embedded in routes/Settings.svelte's `security` category. See
       docs/guides/security.md.
     ======================================================================== -->
<script lang="ts">
  import { onMount } from 'svelte';
  import { api } from '$lib/services/api';
  import { t } from '$lib/stores/i18n';
  import { IconPlugConnected, IconPlugOff, IconRefresh } from '@tabler/icons-svelte';
  import SettingsCard from './SettingsCard.svelte';
  import { cn } from '$lib/utils.js';
  import {
    settingsRow,
    settingsRowLabel,
    settingsRowHint,
    settingsHint,
    settingsBtn,
    settingsBtnGhost
  } from './vocabulary';

  interface Client {
    id: string;
    protocol: string;
    username: string;
    client_ip: string;
    client_host: string | null;
    target: string;
    opened_files: number;
    kickable: boolean;
    detail: string;
  }

  interface Source {
    source: string;
    available: boolean;
    error: string | null;
    count: number;
  }

  let { class: klass = '' }: { class?: string } = $props();

  let loading = $state(true);
  let loadError = $state('');
  let notice = $state<{ kind: 'ok' | 'error'; text: string } | null>(null);
  let busy = $state(false);
  let clients = $state<Client[]>([]);
  let sources = $state<Source[]>([]);
  let kickable = $state(0);
  let checkedAt = $state<string | null>(null);
  // The row awaiting confirmation — disconnecting someone is not reversible
  // from the screen, so it takes a second click that names the client.
  let pendingKick = $state<string | null>(null);

  async function load() {
    loading = true;
    loadError = '';
    const res = await api.getConnectedClients();
    if (res.success && res.data) {
      const d = res.data as any;
      clients = Array.isArray(d.clients) ? d.clients : [];
      sources = Array.isArray(d.sources) ? d.sources : [];
      kickable = typeof d.kickable === 'number' ? d.kickable : 0;
      checkedAt = d.checked_at ?? null;
    } else {
      loadError = res.message || $t('settings.clients_load_error');
    }
    loading = false;
  }

  onMount(load);

  async function kick(client: Client) {
    if (pendingKick !== client.id) {
      pendingKick = client.id;
      notice = null;
      return;
    }
    pendingKick = null;
    busy = true;
    notice = null;
    const res = await api.kickConnectedClient(client.id);
    if (res.success) {
      const detail = (res.data as any)?.detail;
      notice = {
        kind: 'ok',
        text: detail || $t('settings.clients_kicked', { ip: client.client_ip })
      };
      await load();
    } else {
      // The server's own reason: "no such client any more", "a NFS export
      // cannot be dropped", or the tool's failure.
      notice = { kind: 'error', text: res.message || $t('settings.clients_kick_error') };
    }
    busy = false;
  }

  function sourceLabel(source: string): string {
    switch (source) {
      case 'smb':
        return 'SMB';
      case 'nfs':
        return 'NFS';
      case 'sessions':
        return $t('settings.clients_source_sessions');
      default:
        return source;
    }
  }
</script>

<SettingsCard
  title={$t('settings.clients_title')}
  subtitle={$t('settings.clients_subtitle')}
  class={klass}
>
  {#if loading}
    <p class={settingsHint}>{$t('common.loading')}</p>
  {:else}
    {#if loadError}
      <div class="clients-alert clients-alert-error" role="alert">{loadError}</div>
    {/if}
    {#if notice}
      <div
        class={cn('clients-alert', notice.kind === 'ok' ? 'clients-alert-ok' : 'clients-alert-error')}
        role={notice.kind === 'ok' ? 'status' : 'alert'}
        data-testid="clients-notice"
      >
        {notice.text}
      </div>
    {/if}

    <div class={settingsRow}>
      <div class="min-w-0">
        <span class={settingsRowLabel}>{$t('settings.clients_sources')}</span>
        <p class={settingsRowHint}>
          {$t('settings.clients_sources_hint', {
            clients: String(clients.length),
            kickable: String(kickable)
          })}
        </p>
      </div>
      <button
        class={cn(settingsBtn, settingsBtnGhost)}
        onclick={load}
        disabled={busy}
        data-testid="clients-refresh"
      >
        <IconRefresh size={16} /> {$t('common.refresh')}
      </button>
    </div>

    <ul class="m-0 p-0 list-none mb-3" data-testid="clients-sources">
      {#each sources as source (source.source)}
        <li class="clients-source" data-testid="clients-source" data-source={source.source}>
          <span
            class={cn('clients-dot', source.available ? 'clients-dot-ok' : 'clients-dot-off')}
            aria-hidden="true"
          ></span>
          <span class="text-xs font-semibold">{sourceLabel(source.source)}</span>
          <span class="text-xs text-[var(--hv-settings-label)]">
            {#if source.available}
              {$t('settings.clients_source_count', { count: String(source.count) })}
            {:else}
              {$t('settings.clients_source_unavailable')}{#if source.error}
                — {source.error}{/if}
            {/if}
          </span>
        </li>
      {/each}
    </ul>

    {#if clients.length === 0}
      <p class={settingsHint} data-testid="clients-empty">{$t('settings.clients_empty')}</p>
    {:else}
      <ul class="m-0 p-0 list-none" data-testid="clients-list">
        {#each clients as client (client.id)}
          <li class="clients-row" data-testid="clients-row" data-client-id={client.id}>
            <span class="min-w-0">
              <span class="block text-sm font-medium text-[var(--hv-settings-title)]">
                <span class="clients-tag">{client.protocol.toUpperCase()}</span>
                <span class="clients-mono">{client.client_ip}</span>
                {#if client.client_host}<span class="text-[var(--hv-settings-label)]">
                    ({client.client_host})</span
                  >{/if}
              </span>
              <span class="block text-xs text-[var(--hv-settings-label)]">
                {client.username} · {client.target}
                {#if client.opened_files > 0}
                  · {$t('settings.clients_open_files', { count: String(client.opened_files) })}
                {/if}
                · {client.detail}
              </span>
            </span>
            {#if client.kickable}
              <button
                class={cn(
                  settingsBtn,
                  settingsBtnGhost,
                  pendingKick === client.id && 'clients-confirm'
                )}
                disabled={busy}
                onclick={() => kick(client)}
                aria-label={$t('settings.clients_kick_ip', { ip: client.client_ip })}
                data-testid="clients-kick"
              >
                {#if pendingKick === client.id}
                  {$t('settings.clients_kick_confirm')}
                {:else}
                  <IconPlugOff size={16} /> {$t('settings.clients_kick')}
                {/if}
              </button>
            {:else}
              <span
                class="text-xs text-[var(--hv-settings-label)]"
                data-testid="clients-not-kickable"
                title={$t('settings.clients_not_kickable_hint')}
              >
                <IconPlugConnected size={16} /> {$t('settings.clients_not_kickable')}
              </span>
            {/if}
          </li>
        {/each}
      </ul>
    {/if}

    {#if checkedAt}
      <p class={settingsHint}>
        {$t('settings.clients_checked_at', { time: new Date(checkedAt).toLocaleTimeString() })}
      </p>
    {/if}
  {/if}
</SettingsCard>

<style>
  .clients-mono {
    font-family: var(--hv-font-mono);
  }

  .clients-tag {
    display: inline-block;
    margin-right: 6px;
    padding: 0 6px;
    border-radius: var(--hv-radius-sm);
    background: var(--hv-settings-tile);
    border: 1px solid var(--hv-settings-border);
    font-size: var(--hv-font-size-xs);
    font-weight: var(--hv-font-weight-semibold);
    letter-spacing: var(--hv-letter-spacing-wide);
  }

  .clients-source {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 2px 0;
  }

  .clients-dot {
    width: 8px;
    height: 8px;
    border-radius: 999px;
    flex: none;
  }

  .clients-dot-ok {
    background: var(--hv-success);
  }

  .clients-dot-off {
    background: var(--hv-settings-border-strong);
  }

  .clients-row {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 16px;
    padding: 8px 0;
    border-bottom: 1.45px solid var(--hv-settings-hairline);
  }

  .clients-row:last-child {
    border-bottom: none;
  }

  /* The second click of a kick: same button, marked by text AND outline so the
     state is never carried by colour alone. */
  .clients-confirm {
    outline: 1.45px solid var(--hv-error);
    color: var(--hv-error);
  }

  .clients-alert {
    margin: 0 0 12px;
    padding: var(--hv-space-2-5) var(--hv-space-3);
    border-radius: var(--hv-radius-lg);
    font-size: var(--hv-font-size-sm);
  }

  .clients-alert-ok {
    background: var(--hv-success-bg);
    color: var(--hv-success-text);
  }

  .clients-alert-error {
    background: var(--hv-error-light);
    color: var(--hv-error);
  }
</style>

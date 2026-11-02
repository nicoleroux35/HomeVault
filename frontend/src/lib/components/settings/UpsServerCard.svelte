<!-- ========================================================================
     File          : UpsServerCard.svelte
     Author        : Nicolas Le Roux
     Created       : 02/11/2026
     Last modified : 02/11/2026 by Nicolas Le Roux
     Version       : 1.0.0

     Description:
       Settings card for UPS *server* mode: the enable switch, the bind
       interface/port, the per-machine allowlist, the orderly-shutdown policy
       and the daemon state.

       Rewritten on the shared settings vocabulary and the design tokens. The
       previous version carried hardcoded English strings, native `alert()`
       and CSS variables that do not exist in the token set.

       The shutdown-policy section now says out loud which stages this build
       cannot perform: a stage reported `skipped` is shown as "not available",
       never as done (the server answers `skipped: true`, see
       `src/ups_server.rs`).

     Dependencies:
       - svelte (>= 5), $lib/services/api-phase4, $lib/stores/i18n,
         ./SettingsCard, ./vocabulary

     Usage:
       Embedded in routes/Settings.svelte's hardware category. See
       docs/guides/ups-server.md.
     ======================================================================== -->
<script lang="ts">
  import { onMount } from 'svelte';
  import { t } from '$lib/stores/i18n';
  import {
    getUpsServerConfig,
    updateUpsServerConfig,
    startUpsServer,
    stopUpsServer
  } from '$lib/services/api-phase4';
  import type { UpsServerConfig, UpsClient } from '$lib/types/phase4';
  import SettingsCard from './SettingsCard.svelte';
  import { cn } from '$lib/utils.js';
  import {
    settingsHint,
    settingsRow,
    settingsRowLabel,
    settingsRowHint,
    settingsInput,
    settingsBtn,
    settingsBtnPrimary,
    settingsBtnDanger
  } from './vocabulary';

  let { class: klass = '' }: { class?: string } = $props();

  let loading = $state(true);
  let saving = $state(false);
  let busy = $state(false);
  let loadError = $state('');
  let notice = $state<{ kind: 'ok' | 'error'; text: string } | null>(null);

  let config = $state<UpsServerConfig | null>(null);
  let available = $state(false);
  let running = $state(false);

  let showAddClient = $state(false);
  let newClient = $state<Partial<UpsClient>>({
    name: '',
    address: '',
    username: '',
    master: false
  });

  onMount(load);

  async function load() {
    loading = true;
    loadError = '';
    try {
      const response = await getUpsServerConfig();
      config = response.config;
      available = !!response.available;
      running = !!response.state.running;
    } catch (e) {
      loadError = e instanceof Error ? e.message : $t('settings.ups_server_load_failed');
    } finally {
      loading = false;
    }
  }

  async function handleSave() {
    if (!config) return;
    saving = true;
    notice = null;
    try {
      await updateUpsServerConfig(config);
      await load();
      notice = { kind: 'ok', text: $t('settings.ups_server_saved') };
    } catch (e) {
      notice = {
        kind: 'error',
        text: e instanceof Error ? e.message : $t('settings.ups_server_save_failed')
      };
    } finally {
      saving = false;
    }
  }

  async function handleStart() {
    busy = true;
    notice = null;
    try {
      await startUpsServer();
      await load();
      notice = { kind: 'ok', text: $t('settings.ups_server_saved') };
    } catch (e) {
      notice = {
        kind: 'error',
        text: e instanceof Error ? e.message : $t('settings.ups_server_load_failed')
      };
    } finally {
      busy = false;
    }
  }

  async function handleStop() {
    busy = true;
    notice = null;
    try {
      await stopUpsServer();
      await load();
      notice = { kind: 'ok', text: $t('settings.ups_server_saved') };
    } catch (e) {
      notice = {
        kind: 'error',
        text: e instanceof Error ? e.message : $t('settings.ups_server_load_failed')
      };
    } finally {
      busy = false;
    }
  }

  function addClient() {
    if (!config) return;
    if (!newClient.name || !newClient.address || !newClient.username) {
      notice = { kind: 'error', text: $t('settings.ups_server_clients_hint') };
      return;
    }

    const client: UpsClient = {
      name: newClient.name,
      address: newClient.address,
      username: newClient.username,
      // The server seals the password itself (`Argon2`), so the client never
      // sends one. The field stays in the wire type because the response
      // carries a hash.
      password_hash: '',
      master: newClient.master ?? false
    };

    config.allowed_clients = [...config.allowed_clients, client];
    newClient = { name: '', address: '', username: '', master: false };
    showAddClient = false;
  }

  function removeClient(index: number) {
    if (!config) return;
    config.allowed_clients = config.allowed_clients.filter((_, i) => i !== index);
  }
</script>

<SettingsCard
  title={$t('settings.ups_server_title')}
  subtitle={$t('settings.ups_server_subtitle')}
  class={cn('ups-server-card', klass)}
  data-testid="ups-server-card"
>
  {#if loading}
    <p class={settingsHint}>{$t('common.loading')}</p>
  {:else}
    {#if !available}
      <div class="ups-alert ups-alert-warn" role="status" data-testid="ups-server-unavailable">
        <p>{$t('settings.ups_server_unavailable')}</p>
        <p class={settingsRowHint}>{$t('settings.ups_server_unavailable_hint')}</p>
      </div>
    {/if}

    {#if loadError}
      <div class="ups-alert ups-alert-error" role="alert">{loadError}</div>
    {/if}
    {#if notice}
      <div
        class={cn('ups-alert', notice.kind === 'ok' ? 'ups-alert-ok' : 'ups-alert-error')}
        role={notice.kind === 'ok' ? 'status' : 'alert'}
        data-testid="ups-server-notice"
      >
        {notice.text}
      </div>
    {/if}

    {#if config}
      <div class={settingsRow}>
        <div class="min-w-0">
          <span class={settingsRowLabel} id="ups-server-enable-label">
            {$t('settings.ups_server_enable')}
          </span>
          <p class={settingsRowHint}>{$t('settings.ups_server_enable_hint')}</p>
        </div>
        <input
          type="checkbox"
          class="ups-check"
          bind:checked={config.enabled}
          aria-labelledby="ups-server-enable-label"
          data-testid="ups-server-enable"
        />
      </div>

      {#if config.enabled}
        <h4 class="ups-subhead">{$t('settings.ups_server_network_title')}</h4>
        <div class={settingsRow}>
          <div class="min-w-0">
            <label class={settingsRowLabel} for="ups-bind-interface">
              {$t('settings.ups_server_bind_interface')}
            </label>
            <p class={settingsRowHint}>{$t('settings.ups_server_bind_interface_hint')}</p>
          </div>
          <input
            id="ups-bind-interface"
            type="text"
            class={settingsInput}
            bind:value={config.bind_interface}
            placeholder="eth0"
            data-testid="ups-bind-interface"
          />
        </div>
        <div class={settingsRow}>
          <div class="min-w-0">
            <label class={settingsRowLabel} for="ups-port">{$t('settings.ups_server_port')}</label>
            <p class={settingsRowHint}>{$t('settings.ups_server_port_hint')}</p>
          </div>
          <input
            id="ups-port"
            type="number"
            class={settingsInput}
            min="1024"
            max="65535"
            bind:value={config.port}
            data-testid="ups-port"
          />
        </div>
        <div class={settingsRow}>
          <label class={settingsRowLabel} for="ups-max-clients">
            {$t('settings.ups_server_max_clients')}
          </label>
          <input
            id="ups-max-clients"
            type="number"
            class={settingsInput}
            min="1"
            max="100"
            bind:value={config.max_clients}
            data-testid="ups-max-clients"
          />
        </div>

        <h4 class="ups-subhead">{$t('settings.ups_server_clients_title')}</h4>
        <p class={settingsRowHint}>{$t('settings.ups_server_clients_hint')}</p>

        {#if config.allowed_clients.length === 0}
          <p class={settingsHint} data-testid="ups-no-clients">
            {$t('settings.ups_server_clients_empty')}
          </p>
        {:else}
          <table class="ups-table" data-testid="ups-clients">
            <thead>
              <tr>
                <th scope="col">{$t('settings.ups_server_client_name')}</th>
                <th scope="col">{$t('settings.ups_server_client_address')}</th>
                <th scope="col">{$t('settings.ups_server_client_username')}</th>
                <th scope="col">{$t('settings.ups_server_master')}</th>
                <th scope="col">{$t('common.actions')}</th>
              </tr>
            </thead>
            <tbody>
              {#each config.allowed_clients as client, i (client.name + client.address)}
                <tr>
                  <td>{client.name}</td>
                  <td>{client.address}</td>
                  <td>{client.username}</td>
                  <td>{client.master ? $t('common.yes') : $t('common.no')}</td>
                  <td>
                    <button
                      type="button"
                      class={settingsBtnDanger}
                      onclick={() => removeClient(i)}
                    >
                      {$t('common.remove')}
                    </button>
                  </td>
                </tr>
              {/each}
            </tbody>
          </table>
        {/if}

        {#if !showAddClient}
          <button
            type="button"
            class={settingsBtn}
            onclick={() => (showAddClient = true)}
            data-testid="ups-add-client"
          >
            {$t('settings.ups_server_add_client')}
          </button>
        {:else}
          <div class="ups-add-client">
            <div class={settingsRow}>
              <label class={settingsRowLabel} for="ups-new-name">
                {$t('settings.ups_server_client_name')}
              </label>
              <input
                id="ups-new-name"
                type="text"
                class={settingsInput}
                bind:value={newClient.name}
                placeholder="client1"
                data-testid="ups-new-name"
              />
            </div>
            <div class={settingsRow}>
              <label class={settingsRowLabel} for="ups-new-address">
                {$t('settings.ups_server_client_address')}
              </label>
              <input
                id="ups-new-address"
                type="text"
                class={settingsInput}
                bind:value={newClient.address}
                placeholder="192.168.1.100"
                data-testid="ups-new-address"
              />
            </div>
            <div class={settingsRow}>
              <label class={settingsRowLabel} for="ups-new-username">
                {$t('settings.ups_server_client_username')}
              </label>
              <input
                id="ups-new-username"
                type="text"
                class={settingsInput}
                bind:value={newClient.username}
                placeholder="monitor"
                data-testid="ups-new-username"
              />
            </div>
            <div class={settingsRow}>
              <span class={settingsRowLabel} id="ups-new-master-label">
                {$t('settings.ups_server_client_master')}
              </span>
              <input
                type="checkbox"
                class="ups-check"
                bind:checked={newClient.master}
                aria-labelledby="ups-new-master-label"
                data-testid="ups-new-master"
              />
            </div>
            <div class="ups-actions">
              <button
                type="button"
                class={settingsBtnPrimary}
                onclick={addClient}
                data-testid="ups-confirm-client"
              >
                {$t('settings.ups_server_add_client')}
              </button>
              <button
                type="button"
                class={settingsBtn}
                onclick={() => (showAddClient = false)}
              >
                {$t('common.cancel')}
              </button>
            </div>
          </div>
        {/if}

        <h4 class="ups-subhead">{$t('settings.ups_server_shutdown_title')}</h4>
        <p class={settingsRowHint}>{$t('settings.ups_server_shutdown_hint')}</p>

        <div class={settingsRow}>
          <span class={settingsRowLabel} id="ups-stop-containers-label">
            {$t('settings.ups_server_stop_containers')}
          </span>
          <input
            type="checkbox"
            class="ups-check"
            bind:checked={config.shutdown_policy.stop_containers}
            aria-labelledby="ups-stop-containers-label"
            data-testid="ups-stop-containers"
          />
        </div>
        <div class={settingsRow}>
          <span class={settingsRowLabel} id="ups-shutdown-vms-label">
            {$t('settings.ups_server_shutdown_vms')}
          </span>
          <input
            type="checkbox"
            class="ups-check"
            bind:checked={config.shutdown_policy.shutdown_vms}
            aria-labelledby="ups-shutdown-vms-label"
            data-testid="ups-shutdown-vms"
          />
        </div>
        <div class={settingsRow}>
          <span class={settingsRowLabel} id="ups-flush-restic-label">
            {$t('settings.ups_server_flush_restic')}
          </span>
          <input
            type="checkbox"
            class="ups-check"
            bind:checked={config.shutdown_policy.flush_restic}
            aria-labelledby="ups-flush-restic-label"
            data-testid="ups-flush-restic"
          />
        </div>
        <div class={settingsRow}>
          <span class={settingsRowLabel} id="ups-stop-shares-label">
            {$t('settings.ups_server_stop_shares')}
          </span>
          <input
            type="checkbox"
            class="ups-check"
            bind:checked={config.shutdown_policy.stop_shares}
            aria-labelledby="ups-stop-shares-label"
            data-testid="ups-stop-shares"
          />
        </div>

        <!-- Two of the five stages have no implementation in this build. The
             card says so here rather than letting a "done" report imply
             otherwise. -->
        <p class={settingsRowHint} data-testid="ups-shutdown-warning">
          {$t('settings.ups_server_shutdown_warning')}
        </p>
      {/if}

      <h4 class="ups-subhead">{$t('settings.ups_server_status_title')}</h4>
      <div class={settingsRow} data-testid="ups-server-status-row">
        <span class={settingsRowLabel}>{$t('settings.ups_server_status_label')}</span>
        <span
          class={cn('ups-badge', running ? 'ups-badge-on' : 'ups-badge-off')}
          data-testid="ups-server-status"
          data-state={running ? 'running' : 'stopped'}
        >
          {running ? $t('settings.ups_server_running') : $t('settings.ups_server_stopped')}
        </span>
      </div>

      <div class="ups-actions">
        <button
          type="button"
          class={settingsBtnPrimary}
          onclick={handleSave}
          disabled={saving}
          data-testid="ups-server-save"
        >
          {saving ? $t('common.saving') : $t('common.save')}
        </button>
        {#if running}
          <button
            type="button"
            class={settingsBtnDanger}
            onclick={handleStop}
            disabled={busy}
            data-testid="ups-server-stop"
          >
            {$t('settings.ups_server_stop')}
          </button>
        {:else}
          <button
            type="button"
            class={settingsBtn}
            onclick={handleStart}
            disabled={busy}
            data-testid="ups-server-start"
          >
            {$t('settings.ups_server_start')}
          </button>
        {/if}
      </div>
    {/if}
  {/if}
</SettingsCard>

<style>
  .ups-alert {
    border: 1px solid var(--hv-shell-border);
    border-radius: 8px;
    padding: 10px 12px;
    margin-bottom: 12px;
    font-size: 0.8125rem;
    line-height: 1.25rem;
  }

  .ups-alert-error {
    background: var(--hv-error-bg);
    border-color: var(--hv-error);
    color: var(--hv-error);
  }

  .ups-alert-ok {
    background: var(--hv-success-bg);
    border-color: var(--hv-success);
    color: var(--hv-success);
  }

  .ups-alert-warn {
    background: var(--hv-shell-border-soft);
    border-color: var(--hv-shell-border-strong);
    color: var(--hv-shell-text);
  }

  .ups-check {
    height: 1rem;
    width: 1rem;
    flex: none;
    accent-color: var(--hv-primary);
  }

  .ups-subhead {
    margin: 16px 0 8px;
    font-size: 0.8125rem;
    font-weight: 600;
    color: var(--hv-settings-title);
  }

  .ups-table {
    width: 100%;
    border-collapse: collapse;
    margin-bottom: 12px;
    font-size: 0.8125rem;
  }

  .ups-table th,
  .ups-table td {
    padding: 6px 10px;
    text-align: left;
    border-bottom: 1px solid var(--hv-shell-border);
  }

  .ups-table th {
    font-weight: 600;
    color: var(--hv-shell-text-3);
    font-size: 0.75rem;
  }

  .ups-table td {
    color: var(--hv-shell-text);
  }

  .ups-add-client {
    border: 1px solid var(--hv-shell-border);
    border-radius: 8px;
    padding: 12px;
    margin-bottom: 12px;
  }

  .ups-badge {
    font-size: 0.6875rem;
    padding: 2px 10px;
    border-radius: 999px;
    font-weight: 600;
  }

  .ups-badge-on {
    background: var(--hv-success-bg);
    color: var(--hv-success);
  }

  .ups-badge-off {
    background: var(--hv-shell-border-soft);
    color: var(--hv-shell-text-2);
  }

  .ups-actions {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
    margin-top: 16px;
  }
</style>

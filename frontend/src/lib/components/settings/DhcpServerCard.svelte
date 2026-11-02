<!-- ========================================================================
     File          : DhcpServerCard.svelte
     Author        : Nicolas Le Roux
     Created       : 02/11/2026
     Last modified : 02/11/2026 by Nicolas Le Roux
     Version       : 1.0.0

     Description:
       Settings card for the DHCP/TFTP/PXE server: the two switches, the
       address range, fixed addresses, the boot file, the live server state
       and the active lease table.

       Rewritten on the shared settings vocabulary, the design tokens and the
       shared `Modal` (which traps focus). The previous version carried
       hardcoded English strings, native `alert()`, a hand-rolled modal with no
       focus trap, and CSS variables that do not exist in the token set
       (`--card-bg`, `--primary-color`), so it rendered unstyled.

     Dependencies:
       - svelte (>= 5), $lib/services/api-phase4, $lib/stores/i18n,
         ./SettingsCard, ./vocabulary, $lib/components/theme/Modal

     Usage:
       Embedded in routes/Settings.svelte's network category. See
       docs/guides/dhcp.md.
     ======================================================================== -->
<script lang="ts">
  import { onMount } from 'svelte';
  import { t } from '$lib/stores/i18n';
  import {
    getDhcpServerConfig,
    updateDhcpServerConfig,
    startDhcpServer,
    stopDhcpServer,
    getDhcpLeases
  } from '$lib/services/api-phase4';
  import type { DhcpServerConfig, DhcpLease } from '$lib/types/phase4';
  import SettingsCard from './SettingsCard.svelte';
  import Modal from '$lib/components/theme/Modal.svelte';
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

  let config = $state<DhcpServerConfig | null>(null);
  let leases = $state<DhcpLease[]>([]);
  let available = $state(false);
  let running = $state(false);

  let confirmOpen = $state(false);
  let newReservationMac = $state('');
  let newReservationIp = $state('');

  onMount(load);

  async function load() {
    loading = true;
    loadError = '';
    try {
      const response = await getDhcpServerConfig();
      config = response.config;
      available = !!response.available;
      running = !!response.state.running;
      leases = running ? await getDhcpLeases() : [];
    } catch (e) {
      loadError = e instanceof Error ? e.message : $t('settings.dhcp_load_failed');
    } finally {
      loading = false;
    }
  }

  async function handleSave() {
    if (!config) return;

    // Turning either server on is the destructive-ish action: it is the one
    // that can hand out wrong addresses on a live LAN, so it is confirmed and
    // nothing else is.
    if ((config.dhcp_enabled || config.tftp_enabled) && !config.confirmed) {
      confirmOpen = true;
      return;
    }

    saving = true;
    notice = null;
    try {
      await updateDhcpServerConfig(config);
      await load();
      notice = { kind: 'ok', text: $t('settings.dhcp_saved') };
    } catch (e) {
      notice = {
        kind: 'error',
        text: e instanceof Error ? e.message : $t('settings.dhcp_save_failed')
      };
    } finally {
      saving = false;
    }
  }

  async function confirmAndSave() {
    if (!config) return;
    config.confirmed = true;
    confirmOpen = false;
    await handleSave();
  }

  async function handleStart() {
    busy = true;
    notice = null;
    try {
      await startDhcpServer();
      await load();
      notice = { kind: 'ok', text: $t('settings.dhcp_saved') };
    } catch (e) {
      notice = {
        kind: 'error',
        text: e instanceof Error ? e.message : $t('settings.dhcp_start_failed')
      };
    } finally {
      busy = false;
    }
  }

  async function handleStop() {
    busy = true;
    notice = null;
    try {
      await stopDhcpServer();
      await load();
      notice = { kind: 'ok', text: $t('settings.dhcp_saved') };
    } catch (e) {
      notice = {
        kind: 'error',
        text: e instanceof Error ? e.message : $t('settings.dhcp_stop_failed')
      };
    } finally {
      busy = false;
    }
  }

  function addReservation() {
    if (!config) return;
    const mac = newReservationMac.trim();
    const ip = newReservationIp.trim();
    if (!mac || !ip) {
      notice = { kind: 'error', text: $t('settings.dhcp_reservations_hint') };
      return;
    }
    config.reservations = { ...config.reservations, [mac]: ip };
    newReservationMac = '';
    newReservationIp = '';
  }

  function removeReservation(mac: string) {
    if (!config) return;
    const { [mac]: _removed, ...rest } = config.reservations;
    config.reservations = rest;
  }

  function formatExpiry(timestamp: number): string {
    if (!timestamp) return $t('settings.dhcp_expires');
    return new Date(timestamp * 1000).toLocaleString();
  }
</script>

<SettingsCard
  title={$t('settings.dhcp_title')}
  subtitle={$t('settings.dhcp_subtitle')}
  class={cn('dhcp-card', klass)}
  data-testid="dhcp-card"
>
  {#if loading}
    <p class={settingsHint}>{$t('common.loading')}</p>
  {:else}
    {#if !available}
      <div class="dhcp-alert dhcp-alert-warn" role="status" data-testid="dhcp-unavailable">
        <p>{$t('settings.dhcp_unavailable')}</p>
        <p class={settingsRowHint}>{$t('settings.dhcp_unavailable_hint')}</p>
      </div>
    {/if}

    {#if loadError}
      <div class="dhcp-alert dhcp-alert-error" role="alert">{loadError}</div>
    {/if}
    {#if notice}
      <div
        class={cn('dhcp-alert', notice.kind === 'ok' ? 'dhcp-alert-ok' : 'dhcp-alert-error')}
        role={notice.kind === 'ok' ? 'status' : 'alert'}
        data-testid="dhcp-notice"
      >
        {notice.text}
      </div>
    {/if}

    {#if config}
      <div class={settingsRow}>
        <div class="min-w-0">
          <span class={settingsRowLabel} id="dhcp-enable-label">{$t('settings.dhcp_enable_dhcp')}</span>
          <p class={settingsRowHint}>{$t('settings.dhcp_enable_dhcp_hint')}</p>
        </div>
        <input
          type="checkbox"
          class="dhcp-check"
          bind:checked={config.dhcp_enabled}
          aria-labelledby="dhcp-enable-label"
          data-testid="dhcp-enable"
        />
      </div>

      <div class={settingsRow}>
        <div class="min-w-0">
          <span class={settingsRowLabel} id="dhcp-tftp-label">{$t('settings.dhcp_enable_tftp')}</span>
          <p class={settingsRowHint}>{$t('settings.dhcp_enable_tftp_hint')}</p>
        </div>
        <input
          type="checkbox"
          class="dhcp-check"
          bind:checked={config.tftp_enabled}
          aria-labelledby="dhcp-tftp-label"
          data-testid="dhcp-tftp"
        />
      </div>

      {#if config.dhcp_enabled || config.tftp_enabled}
        <div class={settingsRow}>
          <div class="min-w-0">
            <label class={settingsRowLabel} for="dhcp-interface">{$t('settings.dhcp_interface')}</label>
            <p class={settingsRowHint}>{$t('settings.dhcp_interface_hint')}</p>
          </div>
          <input
            id="dhcp-interface"
            type="text"
            class={settingsInput}
            bind:value={config.interface}
            placeholder="eth0"
            data-testid="dhcp-interface"
          />
        </div>
      {/if}

      {#if config.dhcp_enabled}
        <h4 class="dhcp-subhead">{$t('settings.dhcp_dhcp_title')}</h4>

        <div class={settingsRow}>
          <label class={settingsRowLabel} for="dhcp-range-start">{$t('settings.dhcp_range_start')}</label>
          <input
            id="dhcp-range-start"
            type="text"
            class={settingsInput}
            bind:value={config.dhcp_range_start}
            placeholder="192.168.1.100"
            data-testid="dhcp-range-start"
          />
        </div>
        <div class={settingsRow}>
          <label class={settingsRowLabel} for="dhcp-range-end">{$t('settings.dhcp_range_end')}</label>
          <input
            id="dhcp-range-end"
            type="text"
            class={settingsInput}
            bind:value={config.dhcp_range_end}
            placeholder="192.168.1.200"
            data-testid="dhcp-range-end"
          />
        </div>
        <div class={settingsRow}>
          <label class={settingsRowLabel} for="dhcp-netmask">{$t('settings.dhcp_netmask')}</label>
          <input
            id="dhcp-netmask"
            type="text"
            class={settingsInput}
            bind:value={config.netmask}
            placeholder="255.255.255.0"
            data-testid="dhcp-netmask"
          />
        </div>
        <div class={settingsRow}>
          <div class="min-w-0">
            <label class={settingsRowLabel} for="dhcp-gateway">{$t('settings.dhcp_gateway')}</label>
            <p class={settingsRowHint}>{$t('settings.dhcp_gateway_hint')}</p>
          </div>
          <input
            id="dhcp-gateway"
            type="text"
            class={settingsInput}
            bind:value={config.gateway}
            placeholder="192.168.1.1"
            data-testid="dhcp-gateway"
          />
        </div>
        <div class={settingsRow}>
          <div class="min-w-0">
            <label class={settingsRowLabel} for="dhcp-lease">{$t('settings.dhcp_lease_time')}</label>
            <p class={settingsRowHint}>{$t('settings.dhcp_lease_time_hint')}</p>
          </div>
          <input
            id="dhcp-lease"
            type="number"
            class={settingsInput}
            min="60"
            max="2592000"
            bind:value={config.lease_time_secs}
            data-testid="dhcp-lease"
          />
        </div>

        <h4 class="dhcp-subhead">{$t('settings.dhcp_reservations_title')}</h4>
        <p class={settingsRowHint}>{$t('settings.dhcp_reservations_hint')}</p>

        {#if Object.keys(config.reservations).length === 0}
          <p class={settingsHint} data-testid="dhcp-no-reservations">
            {$t('settings.dhcp_reservations_empty')}
          </p>
        {:else}
          <table class="dhcp-table" data-testid="dhcp-reservations">
            <thead>
              <tr>
                <th scope="col">{$t('settings.dhcp_mac')}</th>
                <th scope="col">{$t('settings.dhcp_ip')}</th>
                <th scope="col">{$t('common.actions')}</th>
              </tr>
            </thead>
            <tbody>
              {#each Object.entries(config.reservations) as [mac, ip] (mac)}
                <tr>
                  <td>{mac}</td>
                  <td>{ip}</td>
                  <td>
                    <button
                      type="button"
                      class={settingsBtnDanger}
                      onclick={() => removeReservation(mac)}
                    >
                      {$t('common.remove')}
                    </button>
                  </td>
                </tr>
              {/each}
            </tbody>
          </table>
        {/if}

        <div class="dhcp-reservation-add">
          <label class="sr-only" for="dhcp-new-mac">{$t('settings.dhcp_mac')}</label>
          <input
            id="dhcp-new-mac"
            type="text"
            class={settingsInput}
            bind:value={newReservationMac}
            placeholder="aa:bb:cc:dd:ee:ff"
            data-testid="dhcp-new-mac"
          />
          <label class="sr-only" for="dhcp-new-ip">{$t('settings.dhcp_ip')}</label>
          <input
            id="dhcp-new-ip"
            type="text"
            class={settingsInput}
            bind:value={newReservationIp}
            placeholder="192.168.1.50"
            data-testid="dhcp-new-ip"
          />
          <button type="button" class={settingsBtn} onclick={addReservation}>
            {$t('settings.dhcp_add_reservation')}
          </button>
        </div>
      {/if}

      {#if config.tftp_enabled}
        <h4 class="dhcp-subhead">{$t('settings.dhcp_tftp_title')}</h4>
        <div class={settingsRow}>
          <div class="min-w-0">
            <label class={settingsRowLabel} for="dhcp-tftp-root">{$t('settings.dhcp_tftp_root')}</label>
            <p class={settingsRowHint}>{$t('settings.dhcp_tftp_root_hint')}</p>
          </div>
          <input
            id="dhcp-tftp-root"
            type="text"
            class={settingsInput}
            bind:value={config.tftp_root}
            placeholder="/srv/tftp"
            data-testid="dhcp-tftp-root"
          />
        </div>
        <div class={settingsRow}>
          <div class="min-w-0">
            <label class={settingsRowLabel} for="dhcp-boot-file">{$t('settings.dhcp_tftp_boot_file')}</label>
            <p class={settingsRowHint}>{$t('settings.dhcp_tftp_boot_file_hint')}</p>
          </div>
          <input
            id="dhcp-boot-file"
            type="text"
            class={settingsInput}
            bind:value={config.pxe_boot_file}
            placeholder="pxelinux.0"
            data-testid="dhcp-boot-file"
          />
        </div>
      {/if}

      <h4 class="dhcp-subhead">{$t('settings.dhcp_status_title')}</h4>
      <div class={settingsRow} data-testid="dhcp-status-row">
        <span class={settingsRowLabel}>{$t('settings.dhcp_status_label')}</span>
        <span
          class={cn('dhcp-badge', running ? 'dhcp-badge-on' : 'dhcp-badge-off')}
          data-testid="dhcp-status"
          data-state={running ? 'running' : 'stopped'}
        >
          {running ? $t('settings.dhcp_running') : $t('settings.dhcp_stopped')}
        </span>
      </div>

      {#if running && config.dhcp_enabled && leases.length > 0}
        <h4 class="dhcp-subhead">{$t('settings.dhcp_leases_title')}</h4>
        <table class="dhcp-table" data-testid="dhcp-leases">
          <thead>
            <tr>
              <th scope="col">{$t('settings.dhcp_ip')}</th>
              <th scope="col">{$t('settings.dhcp_mac')}</th>
              <th scope="col">{$t('settings.dhcp_hostname')}</th>
              <th scope="col">{$t('settings.dhcp_expires')}</th>
            </tr>
          </thead>
          <tbody>
            {#each leases as lease (lease.ip + lease.mac)}
              <tr>
                <td>{lease.ip}</td>
                <td>{lease.mac}</td>
                <td>{lease.hostname ?? '—'}</td>
                <td>{formatExpiry(lease.expires_at)}</td>
              </tr>
            {/each}
          </tbody>
        </table>
      {:else if running && config.dhcp_enabled}
        <p class={settingsHint} data-testid="dhcp-no-leases">
          {$t('settings.dhcp_leases_empty')}
        </p>
      {/if}

      <div class="dhcp-actions">
        <button
          type="button"
          class={settingsBtnPrimary}
          onclick={handleSave}
          disabled={saving}
          data-testid="dhcp-save"
        >
          {saving ? $t('common.saving') : $t('common.save')}
        </button>
        {#if running}
          <button
            type="button"
            class={settingsBtnDanger}
            onclick={handleStop}
            disabled={busy}
            data-testid="dhcp-stop"
          >
            {$t('settings.dhcp_stop')}
          </button>
        {:else}
          <button
            type="button"
            class={settingsBtn}
            onclick={handleStart}
            disabled={busy}
            data-testid="dhcp-start"
          >
            {$t('settings.dhcp_start')}
          </button>
        {/if}
      </div>
    {/if}
  {/if}
</SettingsCard>

<!-- Turning the server on is confirmed; nothing else is. The shared Modal
     traps focus and restores it to the trigger, which the hand-rolled overlay
     this replaced did not. -->
<Modal open={confirmOpen} title={$t('settings.dhcp_confirm_title')} size="sm" onclose={() => (confirmOpen = false)}>
  <div class="dhcp-confirm">
    <p>{$t('settings.dhcp_confirm_only_if')}</p>
    <ul>
      <li>{$t('settings.dhcp_confirm_isolated')}</li>
      <li>{$t('settings.dhcp_confirm_admin')}</li>
    </ul>
    <div class="dhcp-actions">
      <button type="button" class={settingsBtn} onclick={() => (confirmOpen = false)}>
        {$t('common.cancel')}
      </button>
      <button
        type="button"
        class={settingsBtnDanger}
        onclick={confirmAndSave}
        data-testid="dhcp-confirm-enable"
      >
        {$t('settings.dhcp_confirm_understand')}
      </button>
    </div>
  </div>
</Modal>

<style>
  .dhcp-alert {
    border: 1px solid var(--hv-shell-border);
    border-radius: 8px;
    padding: 10px 12px;
    margin-bottom: 12px;
    font-size: 0.8125rem;
    line-height: 1.25rem;
  }

  .dhcp-alert-error {
    background: var(--hv-error-bg);
    border-color: var(--hv-error);
    color: var(--hv-error);
  }

  .dhcp-alert-ok {
    background: var(--hv-success-bg);
    border-color: var(--hv-success);
    color: var(--hv-success);
  }

  .dhcp-alert-warn {
    background: var(--hv-shell-border-soft);
    border-color: var(--hv-warning, var(--hv-shell-border-strong));
    color: var(--hv-shell-text);
  }

  /* A bare checkbox inherits `width: 100%` from base.css (design-system
     pitfall 6); pin the box so it does not stretch across its label. */
  .dhcp-check {
    height: 1rem;
    width: 1rem;
    flex: none;
    accent-color: var(--hv-primary);
  }

  .dhcp-subhead {
    margin: 16px 0 8px;
    font-size: 0.8125rem;
    font-weight: 600;
    color: var(--hv-settings-title);
  }

  .dhcp-table {
    width: 100%;
    border-collapse: collapse;
    margin-bottom: 12px;
    font-size: 0.8125rem;
  }

  .dhcp-table th,
  .dhcp-table td {
    padding: 6px 10px;
    text-align: left;
    border-bottom: 1px solid var(--hv-shell-border);
  }

  .dhcp-table th {
    font-weight: 600;
    color: var(--hv-shell-text-3);
    font-size: 0.75rem;
  }

  .dhcp-table td {
    color: var(--hv-shell-text);
    font-family: var(--hv-font-mono);
  }

  .dhcp-reservation-add {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
    align-items: center;
    margin-bottom: 8px;
  }

  .dhcp-badge {
    font-size: 0.6875rem;
    padding: 2px 10px;
    border-radius: 999px;
    font-weight: 600;
  }

  .dhcp-badge-on {
    background: var(--hv-success-bg);
    color: var(--hv-success);
  }

  .dhcp-badge-off {
    background: var(--hv-shell-border-soft);
    color: var(--hv-shell-text-2);
  }

  .dhcp-actions {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
    margin-top: 16px;
  }

  .dhcp-confirm ul {
    margin: 8px 0 16px;
    padding-left: 18px;
    font-size: 0.8125rem;
    color: var(--hv-shell-text-2);
  }

  .sr-only {
    position: absolute;
    width: 1px;
    height: 1px;
    padding: 0;
    margin: -1px;
    overflow: hidden;
    clip: rect(0, 0, 0, 0);
    white-space: nowrap;
    border: 0;
  }
</style>

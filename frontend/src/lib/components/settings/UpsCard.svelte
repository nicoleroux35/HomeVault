<!-- ========================================================================
     File          : UpsCard.svelte
     Author        : Nicolas Le Roux
     Created       : 28/09/2026
     Last modified : 28/09/2026 by Nicolas Le Roux
     Version       : 1.0.0

     Description:
       Settings sub-card for UPS monitoring: live device status and the
       admin-managed device configuration list (add/remove hosts to poll).

     Dependencies:
       - svelte (>= 5), $lib/services/api, ./SettingsCard

     Usage:
       Embedded in routes/Settings.svelte's monitoring group.
     ======================================================================== -->
<script lang="ts">
  import { onMount } from 'svelte';
  import { api } from '$lib/services/api';
  import { t } from '$lib/stores/i18n';
  import { IconServer, IconPlus, IconTrash } from '@tabler/icons-svelte';
  import SettingsCard from './SettingsCard.svelte';
  import { cn } from '$lib/utils.js';
  import {
    settingsRowStacked, settingsRowLabel, settingsRowHint, settingsHint,
    settingsInput, settingsSelect, settingsBtn, settingsBtnPrimary, settingsBtnDanger
  } from './vocabulary';

  /* The parent stacks this card as the first section of the Monitoring panel,
     so it hands down the 24px top inset (`mt-6`) the other categories get
     from their spacer. Without a `class` prop that attribute would be a type
     error. */
  let { class: klass = '' }: { class?: string } = $props();

  /* The drivers HomeVault exposes in the add form. Kept in lockstep with
     `ALLOWED_UPS_DRIVERS` in `src/ups.rs` — the server rejects anything else. */
  const DRIVERS = [
    'usbhid-ups',
    'blazer_ser',
    'blazer_usb',
    'nutdrv_qx',
    'apcsmart',
    'snmp-ups',
    'dummy-ups',
  ] as const;

  const DRIVER_LABEL_KEY: Record<string, string> = {
    'usbhid-ups': 'settings.ups_driver_usbhid',
    'blazer_ser': 'settings.ups_driver_blazer_ser',
    'blazer_usb': 'settings.ups_driver_blazer_usb',
    'nutdrv_qx': 'settings.ups_driver_nutdrv_qx',
    'apcsmart': 'settings.ups_driver_apcsmart',
    'snmp-ups': 'settings.ups_driver_snmp',
    'dummy-ups': 'settings.ups_driver_dummy',
  };

  let loading = $state(true);
  let available = $state(false);
  let upsList: any[] = $state([]);
  let configs: any[] = $state([]);

  let formName = $state('');
  let formDriver = $state<string>('blazer_ser');
  let formPort = $state('');
  let formDesc = $state('');
  let saving = $state(false);
  let removing = $state('');
  let message = $state('');
  let error = $state('');

  const isSerial = $derived(
    ['blazer_ser', 'blazer_usb', 'nutdrv_qx', 'apcsmart'].includes(formDriver),
  );
  const isNetwork = $derived(formDriver === 'snmp-ups');

  async function loadStatus() {
    loading = true;
    const avail = await api.getUpsAvailability();
    if (avail.success && avail.data) {
      available = !!(avail.data as any).available;
    }
    if (available) {
      const [listRes, configRes] = await Promise.all([
        api.getUpsList(),
        api.getUpsConfig(),
      ]);
      if (listRes.success && listRes.data) upsList = (listRes.data as any[]) ?? [];
      if (configRes.success && configRes.data) configs = (configRes.data as any[]) ?? [];
    }
    loading = false;
  }

  async function saveConfig() {
    saving = true;
    message = '';
    error = '';
    try {
      const res = await api.saveUpsConfig({
        name: formName.trim(),
        driver: formDriver,
        port: formPort.trim(),
        desc: formDesc.trim(),
      });
      if (res.success) {
        message = $t('settings.ups_saved');
        formName = '';
        formPort = '';
        formDesc = '';
        await loadStatus();
      } else {
        error = res.error || $t('settings.ups_save_failed');
      }
    } catch {
      error = $t('settings.ups_save_failed');
    }
    saving = false;
  }

  async function removeConfig(name: string) {
    if (!confirm($t('settings.ups_remove_confirm', { name }))) return;
    removing = name;
    message = '';
    error = '';
    try {
      const res = await api.deleteUpsConfig(name);
      if (res.success) {
        message = $t('settings.ups_removed');
        await loadStatus();
      } else {
        error = res.error || $t('settings.ups_remove_failed');
      }
    } catch {
      error = $t('settings.ups_remove_failed');
    }
    removing = '';
  }

  onMount(loadStatus);
</script>

<!--
  UpsCard — the UPS (NUT) monitor + device configuration.

  A USB (HID) UPS is detected automatically by the `usbhid-ups` `port = auto`
  section the installer writes; a serial or network UPS cannot be guessed, so
  it is added here as a dedicated `[section]` in `/etc/nut/ups.conf` (driver +
  port/address). The server validates and reloads NUT.
-->
<SettingsCard title={$t('settings.ups_monitoring')} subtitle={$t('settings.ups_description')} class={klass}>
  {#if loading}
    <p class={settingsHint}>{$t('settings.loading')}</p>
  {:else if !available}
    <p class={settingsHint}>{$t('settings.ups_not_available')}</p>
  {:else}
    <!-- Live status of every connected UPS -->
    {#if upsList.length === 0}
      <p class={settingsHint}>{$t('settings.no_ups_devices')}</p>
    {:else}
      {#each upsList as ups (ups.name)}
        <div class={settingsRowStacked}>
          <div class="flex items-center gap-2 min-w-0">
            <IconServer size={16} class="flex-none text-[var(--hv-settings-label)]" />
            <span class={settingsRowLabel}>{ups.name}</span>
            <span class="ups-model text-xs leading-4 text-[var(--hv-settings-label)]">{ups.model}</span>
          </div>
          <div class="ups-item-stats grid grid-cols-[repeat(auto-fit,minmax(100px,1fr))] gap-3">
            <div class="ups-stat flex flex-col gap-0.5">
              <span class="ups-stat-label text-t-subheadline uppercase tracking-[0.03em] text-[var(--hv-settings-label)]">{$t('ups.battery_charge')}</span>
              <span class="ups-stat-value text-sm font-medium text-[var(--hv-settings-title)]">{Math.round(ups.battery_charge_percent ?? 0)}%</span>
            </div>
            <div class="ups-stat flex flex-col gap-0.5">
              <span class="ups-stat-label text-t-subheadline uppercase tracking-[0.03em] text-[var(--hv-settings-label)]">{$t('ups.load')}</span>
              <span class="ups-stat-value text-sm font-medium text-[var(--hv-settings-title)]">{Math.round(ups.load_percent ?? 0)}%</span>
            </div>
            <div class="ups-stat flex flex-col gap-0.5">
              <span class="ups-stat-label text-t-subheadline uppercase tracking-[0.03em] text-[var(--hv-settings-label)]">{$t('ups.runtime')}</span>
              <span class="ups-stat-value text-sm font-medium text-[var(--hv-settings-title)]">{ups.runtime_remaining ?? 0}s</span>
            </div>
            <div class="ups-stat flex flex-col gap-0.5">
              <span class="ups-stat-label text-t-subheadline uppercase tracking-[0.03em] text-[var(--hv-settings-label)]">{$t('ups.status')}</span>
              <span
                class="ups-stat-value text-sm font-medium"
                class:ups-online={ups.is_online}
                class:ups-offline={!ups.is_online}
              >
                {ups.is_online ? $t('ups.online') : $t('ups.offline')}
              </span>
            </div>
          </div>
        </div>
      {/each}
    {/if}

    <!-- Configured device sections (`ups.conf`) -->
    <div class={settingsRowStacked} data-testid="ups-config-list">
      <div class="min-w-0">
        <span class={settingsRowLabel}>{$t('settings.ups_config_title')}</span>
        <p class={settingsRowHint}>{$t('settings.ups_config_hint')}</p>
      </div>
      {#if configs.length === 0}
        <p class={settingsHint}>{$t('settings.ups_config_empty')}</p>
      {:else}
        {#each configs as c (c.name)}
          <div class="ups-config-row flex items-center justify-between gap-3 py-1.5">
            <div class="min-w-0">
              <strong class="block text-sm font-medium text-[var(--hv-settings-title)]">{c.name}</strong>
              <span class="text-xs text-[var(--hv-settings-label)]">{c.driver} · {c.port}{c.desc ? ` · ${c.desc}` : ''}</span>
            </div>
            <button
              class={cn(settingsBtn, settingsBtnDanger, 'flex-none')}
              onclick={() => removeConfig(c.name)}
              disabled={removing === c.name}
              data-testid="ups-config-remove"
            >
              <IconTrash size={16} />
              {$t('settings.ups_remove')}
            </button>
          </div>
        {/each}
      {/if}
    </div>

    <!-- Add a serial / network UPS -->
    <div class={settingsRowStacked} data-testid="ups-config-form">
      <span class={settingsRowLabel}>{$t('settings.ups_add_title')}</span>
      <div class="grid grid-cols-[repeat(auto-fit,minmax(200px,1fr))] gap-4">
        <div class="flex flex-col gap-1.5">
          <label class={settingsRowLabel} for="ups-name">{$t('settings.ups_name')}</label>
          <input id="ups-name" class={settingsInput} type="text" bind:value={formName} placeholder="rack-ups" />
        </div>
        <div class="flex flex-col gap-1.5">
          <label class={settingsRowLabel} for="ups-driver">{$t('settings.ups_driver')}</label>
          <select id="ups-driver" class={settingsSelect} bind:value={formDriver}>
            {#each DRIVERS as d}<option value={d}>{$t(DRIVER_LABEL_KEY[d])}</option>{/each}
          </select>
        </div>
        <div class="flex flex-col gap-1.5">
          <label class={settingsRowLabel} for="ups-port">{$t('settings.ups_port')}</label>
          <input
            id="ups-port"
            class={settingsInput}
            type="text"
            bind:value={formPort}
            placeholder={isNetwork ? '192.168.1.10' : isSerial ? '/dev/ttyS0' : 'auto'}
          />
          <span class={settingsHint}>
            {isNetwork ? $t('settings.ups_port_hint_network') : isSerial ? $t('settings.ups_port_hint_serial') : $t('settings.ups_port_hint_usb')}
          </span>
        </div>
        <div class="flex flex-col gap-1.5">
          <label class={settingsRowLabel} for="ups-desc">{$t('settings.ups_desc')}</label>
          <input id="ups-desc" class={settingsInput} type="text" bind:value={formDesc} placeholder={$t('settings.ups_desc_placeholder')} />
        </div>
      </div>
      <div class="flex items-center gap-3">
        <button
          class={cn(settingsBtn, settingsBtnPrimary)}
          onclick={saveConfig}
          disabled={saving || !formName.trim() || !formPort.trim()}
          data-testid="ups-config-save"
        >
          <IconPlus size={16} />
          {saving ? $t('settings.ups_saving') : $t('settings.ups_add')}
        </button>
        {#if message}<span class="text-xs text-[var(--hv-success)]">{message}</span>{/if}
        {#if error}<span class="text-xs text-[var(--hv-error)]">{error}</span>{/if}
      </div>
    </div>
  {/if}
</SettingsCard>

<style>
  .ups-online { color: var(--hv-success); }
  .ups-offline { color: var(--hv-error); }
</style>

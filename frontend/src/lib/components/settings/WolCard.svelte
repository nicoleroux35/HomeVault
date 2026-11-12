<!-- ========================================================================
     File          : WolCard.svelte
     Author        : Nicolas Le Roux
     Created       : 31/10/2026
     Last modified : 31/10/2026 by Nicolas Le Roux
     Version       : 1.0.0

     Description:
       Settings card for Wake-on-LAN: saved hosts, the interface evidence a
       magic packet would use, add/edit/delete, and a "wake now" button whose
       answer is honestly "sent, not confirmed".

     Dependencies:
       - svelte (>= 5), $lib/services/api, $lib/stores/i18n,
         ./SettingsCard, ./vocabulary

     Usage:
       Embedded in routes/Settings.svelte's `system` category. See
       docs/guides/power.md.
     ======================================================================== -->
<script lang="ts">
  import { onMount } from 'svelte';
  import { api } from '$lib/services/api';
  import { t } from '$lib/stores/i18n';
  import { IconBolt, IconTrash, IconDeviceDesktop } from '$lib/icons/lucide';
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
    settingsBtnPrimary
  } from './vocabulary';

  interface WolHost {
    id: number;
    label: string;
    mac: string;
    broadcast: string | null;
    created_at: string;
    updated_at: string;
  }

  interface WolIface {
    name: string;
    address: string;
    prefix: number;
    broadcast: string;
    up: boolean;
  }

  let { class: klass = '' }: { class?: string } = $props();

  let loading = $state(true);
  let loadError = $state('');
  let notice = $state<{ kind: 'ok' | 'error'; text: string } | null>(null);
  let busy = $state(false);

  let hosts = $state<WolHost[]>([]);
  let interfaces = $state<WolIface[]>([]);
  let maxHosts = $state(64);

  // Add/edit form. `editingId` null = the form adds a new host.
  let editingId = $state<number | null>(null);
  let label = $state('');
  let mac = $state('');
  let broadcast = $state('');

  async function load() {
    loading = true;
    loadError = '';
    const res = await api.getWolHosts();
    if (res.success && res.data) {
      const d = res.data as any;
      hosts = Array.isArray(d.hosts) ? d.hosts : [];
      interfaces = Array.isArray(d.interfaces) ? d.interfaces : [];
      maxHosts = typeof d.max_hosts === 'number' ? d.max_hosts : 64;
    } else {
      loadError = res.message || $t('settings.wol_load_error');
    }
    loading = false;
  }

  onMount(load);

  function resetForm() {
    editingId = null;
    label = '';
    mac = '';
    broadcast = '';
  }

  function edit(host: WolHost) {
    editingId = host.id;
    label = host.label;
    mac = host.mac;
    broadcast = host.broadcast ?? '';
    notice = null;
  }

  async function save() {
    busy = true;
    notice = null;
    const payload = {
      label: label.trim(),
      mac: mac.trim(),
      broadcast: broadcast.trim()
    };
    const res =
      editingId === null
        ? await api.createWolHost(payload)
        : await api.updateWolHost(editingId, payload);
    if (res.success) {
      notice = { kind: 'ok', text: $t('settings.wol_saved') };
      resetForm();
      await load();
    } else {
      // The server names the problem (MAC, name, duplicate) — repeating it is
      // the only way the admin can fix it without guessing.
      notice = { kind: 'error', text: res.message || $t('settings.wol_save_error') };
    }
    busy = false;
  }

  async function remove(host: WolHost) {
    busy = true;
    notice = null;
    const res = await api.deleteWolHost(host.id);
    if (res.success) {
      notice = { kind: 'ok', text: $t('settings.wol_deleted', { label: host.label }) };
      if (editingId === host.id) resetForm();
      await load();
    } else {
      notice = { kind: 'error', text: res.message || $t('settings.wol_delete_error') };
    }
    busy = false;
  }

  async function wake(host: WolHost) {
    busy = true;
    notice = null;
    const res = await api.wakeWolHost(host.id);
    if (res.success && res.data) {
      // The server's sentence, verbatim: "sent, not confirmed".
      notice = { kind: 'ok', text: (res.data as any).message || $t('settings.wol_sent') };
    } else {
      notice = { kind: 'error', text: res.message || $t('settings.wol_wake_error') };
    }
    busy = false;
  }

  const upInterfaces = $derived(interfaces.filter((i) => i.up));
</script>

<SettingsCard
  title={$t('settings.wol_title')}
  subtitle={$t('settings.wol_subtitle')}
  class={klass}
>
  {#if loading}
    <p class={settingsHint}>{$t('common.loading')}</p>
  {:else}
    {#if loadError}
      <div class="wol-alert wol-alert-error" role="alert">{loadError}</div>
    {/if}
    {#if notice}
      <div
        class={cn('wol-alert', notice.kind === 'ok' ? 'wol-alert-ok' : 'wol-alert-error')}
        role={notice.kind === 'ok' ? 'status' : 'alert'}
        data-testid="wol-notice"
      >
        {notice.text}
      </div>
    {/if}

    <div class={settingsRow} data-testid="wol-interfaces-row">
      <div class="min-w-0">
        <span class={settingsRowLabel}>{$t('settings.wol_interfaces')}</span>
        <p class={settingsRowHint}>{$t('settings.wol_interfaces_hint')}</p>
      </div>
      <span class="wol-iface" data-testid="wol-interface-list">
        {#if upInterfaces.length === 0}
          {$t('settings.wol_no_interface')}
        {:else}
          {#each upInterfaces as iface (iface.name)}
            <span class="wol-iface-row" data-testid="wol-interface">
              <IconDeviceDesktop size={13} />
              <span class="wol-mono">{iface.name}</span>
              <span class="wol-mono">{iface.address}/{iface.prefix}</span>
              <span class="wol-mono">→ {iface.broadcast}</span>
            </span>
          {/each}
        {/if}
      </span>
    </div>

    <ul class="m-0 p-0 list-none" data-testid="wol-host-list">
      {#if hosts.length === 0}
        <li class="settings-info-row" data-testid="wol-host-list-empty">
          <span class={settingsHint}>{$t('settings.wol_empty')}</span>
        </li>
      {/if}
      {#each hosts as host (host.id)}
        <li class="settings-info-row" data-testid="wol-host" data-host-id={host.id}>
          <span class="flex items-center gap-2 min-w-0">
            <IconDeviceDesktop size={16} />
            <span class="min-w-0">
              <span class="block text-sm font-medium text-[var(--hv-settings-title)] truncate">
                {host.label}
              </span>
              <span class="block text-xs wol-mono text-[var(--hv-settings-label)]">
                {host.mac}{#if host.broadcast}
                  · → {host.broadcast}{/if}
              </span>
            </span>
          </span>
          <span class="flex items-center gap-1">
            <button
              class={cn(settingsBtn, settingsBtnGhost)}
              disabled={busy}
              onclick={() => wake(host)}
              aria-label={$t('settings.wol_wake_host', { label: host.label })}
              data-testid="wol-wake"
            >
              <IconBolt size={16} /> {$t('settings.wol_wake')}
            </button>
            <button
              class={cn(settingsBtn, settingsBtnGhost)}
              disabled={busy}
              onclick={() => edit(host)}
              data-testid="wol-edit"
            >
              {$t('common.edit')}
            </button>
            <button
              class="hv-ic grid place-items-center w-8 h-8 rounded-lg border-none bg-transparent text-[var(--hv-shell-text-2)] cursor-pointer hover:text-[var(--hv-error)] disabled:opacity-40"
              disabled={busy}
              onclick={() => remove(host)}
              aria-label={$t('settings.wol_delete_host', { label: host.label })}
              data-testid="wol-delete"
            >
              <IconTrash size={16} />
            </button>
          </span>
        </li>
      {/each}
    </ul>

    {#if hosts.length === 0}
      <p class={settingsHint} data-testid="wol-empty">{$t('settings.wol_empty')}</p>
    {/if}

    <div class={settingsRow} data-testid="wol-form-row">
      <div class="min-w-0">
        <!-- A heading, not a label: pointing a second <label> at #wol-label
             would concatenate this text into the field's accessible name
             ("Add a machine Name") and give one control two labels. -->
        <span id="wol-form-heading" class={settingsRowLabel}>
          {editingId === null ? $t('settings.wol_add') : $t('settings.wol_edit')}
        </span>
        <p class={settingsRowHint}>
          {editingId === null
            ? $t('settings.wol_add_hint', { max: String(maxHosts) })
            : $t('settings.wol_edit_hint')}
        </p>
      </div>
      <div class="flex flex-col gap-2 w-full max-w-[360px]">
        <label class="text-xs font-semibold text-[var(--hv-settings-label)]" for="wol-label">
          {$t('settings.wol_label')}
          <input
            id="wol-label"
            class={cn(settingsInput, 'mt-1')}
            type="text"
            maxlength="64"
            bind:value={label}
            placeholder={$t('settings.wol_label_placeholder')}
            data-testid="wol-label"
          />
        </label>
        <label class="text-xs font-semibold text-[var(--hv-settings-label)]" for="wol-mac">
          {$t('settings.wol_mac')}
          <input
            id="wol-mac"
            class={cn(settingsInput, 'mt-1 wol-mono')}
            type="text"
            bind:value={mac}
            placeholder="AA:BB:CC:DD:EE:FF"
            spellcheck="false"
            data-testid="wol-mac"
          />
        </label>
        <label class="text-xs font-semibold text-[var(--hv-settings-label)]" for="wol-broadcast">
          {$t('settings.wol_broadcast')}
          <input
            id="wol-broadcast"
            class={cn(settingsInput, 'mt-1 wol-mono')}
            type="text"
            bind:value={broadcast}
            placeholder={$t('settings.wol_broadcast_placeholder')}
            spellcheck="false"
            data-testid="wol-broadcast"
          />
        </label>
        <p class={settingsHint}>{$t('settings.wol_broadcast_hint')}</p>
        <div class="flex flex-wrap gap-2">
          <button
            class={cn(settingsBtn, settingsBtnPrimary)}
            disabled={busy}
            onclick={save}
            data-testid="wol-save"
          >
            {editingId === null ? $t('settings.wol_add') : $t('common.save')}
          </button>
          {#if editingId !== null}
            <button
              class={cn(settingsBtn, settingsBtnGhost)}
              disabled={busy}
              onclick={resetForm}
              data-testid="wol-cancel"
            >
              {$t('common.cancel')}
            </button>
          {/if}
          <button
            class={cn(settingsBtn, settingsBtnGhost)}
            disabled={busy}
            onclick={load}
            data-testid="wol-refresh"
          >
            {$t('common.refresh')}
          </button>
        </div>
      </div>
    </div>

    <p class={settingsHint}>{$t('settings.wol_hint')}</p>
  {/if}
</SettingsCard>

<style>
  .wol-mono {
    font-family: var(--hv-font-mono);
  }

  .wol-iface {
    display: flex;
    flex-direction: column;
    gap: 4px;
    align-items: flex-end;
    font-size: var(--hv-font-size-xs);
    color: var(--hv-settings-label);
  }

  .wol-iface-row {
    display: flex;
    align-items: center;
    gap: 6px;
  }

  .wol-alert {
    margin: 0 0 12px;
    padding: var(--hv-space-2-5) var(--hv-space-3);
    border-radius: var(--hv-radius-lg);
    font-size: var(--hv-font-size-sm);
  }

  .wol-alert-ok {
    background: var(--hv-success-bg);
    color: var(--hv-success-text);
  }

  .wol-alert-error {
    background: var(--hv-error-light);
    color: var(--hv-error);
  }

  .settings-info-row {
    display: flex;
    justify-content: space-between;
    gap: 16px;
    align-items: center;
    padding: 8px 0;
    border-bottom: 1.45px solid var(--hv-settings-hairline);
  }

  .settings-info-row:last-child {
    border-bottom: none;
  }
</style>

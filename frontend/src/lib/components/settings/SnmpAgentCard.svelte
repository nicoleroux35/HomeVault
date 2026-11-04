<!-- ========================================================================
     File          : SnmpAgentCard.svelte
     Author        : Nicolas Le Roux
     Created       : 04/11/2026 by Nicolas Le Roux
     Version       : 1.0.0

     Description:
       The appliance's *own* SNMP agent: a switch to serve SNMP at all, the
       scope (this machine only, or the LAN), a generated read-only community
       and the two identity strings a monitoring tool displays.

     Dependencies:
       - svelte (>= 5), $lib/services/api, ./SettingsCard, ./vocabulary

     Usage:
       Embedded in routes/Settings.svelte's monitoring group, next to the SNMP
       client card.
     ======================================================================== -->
<script lang="ts">
  import { onMount } from 'svelte';
  import { api } from '$lib/services/api';
  import { translate } from '$lib/stores/i18n';
  import { notifyError, notifySuccess } from '$lib/utils/notify';
  import SettingsCard from './SettingsCard.svelte';
  import { cn } from '$lib/utils.js';
  import {
    settingsRow, settingsRowStacked, settingsRowLabel, settingsHint,
    settingsInput, settingsSelect, settingsBtn
  } from './vocabulary';

  let { class: klass = '' }: { class?: string } = $props();

  let loading = $state(true);
  let saving = $state(false);
  /** `503` from the API: `snmpd` is not installed at all. */
  let unavailable = $state(false);
  let enabled = $state(false);
  let scope = $state<'loopback' | 'lan'>('loopback');
  let lanAddress = $state('');
  let location = $state('');
  let contact = $state('');
  let community = $state('');
  let hasCommunity = $state(false);
  let serving = $state(false);
  let candidates = $state<string[]>([]);

  async function load() {
    loading = true;
    const res = await api.getSnmpAgent();
    unavailable = !res.success;
    if (res.success && res.data) {
      const d = res.data as any;
      enabled = !!d.enabled;
      scope = d.scope === 'lan' ? 'lan' : 'loopback';
      lanAddress = d.lan_address || '';
      location = d.location || '';
      contact = d.contact || '';
      hasCommunity = !!d.has_community;
      serving = !!d.serving;
      candidates = (d.lan_candidates as string[]) || [];
      if (!lanAddress && candidates.length > 0) lanAddress = candidates[0];
    }
    loading = false;
  }

  async function save() {
    saving = true;
    const res = await api.updateSnmpAgent({
      enabled,
      scope,
      lan_address: lanAddress || undefined,
      // Only send the community when the admin typed one: an empty field means
      // "keep the current one".
      community: community.trim() ? community.trim() : undefined,
      location,
      contact,
    });
    saving = false;
    if (!res.success) {
      notifyError(res, 'settings.snmp_agent_failed');
      return;
    }
    community = '';
    await load();
    notifySuccess('settings.snmp_agent_saved');
  }

  onMount(load);
</script>

<SettingsCard
  title={translate('settings.snmp_agent')}
  subtitle={translate('settings.snmp_agent_description')}
  class={klass}
>
  {#if loading}
    <p class={settingsHint}>{translate('settings.loading')}</p>
  {:else if unavailable}
    <p class={cn(settingsHint, 'py-3')} data-testid="snmp-agent-unavailable">
      {translate('settings.snmp_agent_unavailable')}
    </p>
  {:else}
    <div class={cn(settingsRow, 'settings-field-full')}>
      <label class={settingsRowLabel} for="snmp-agent-enabled">{translate('settings.snmp_agent_enable')}</label>
      <input id="snmp-agent-enabled" data-testid="snmp-agent-enabled" type="checkbox" bind:checked={enabled} />
    </div>
    <p class={settingsHint}>{translate('settings.snmp_agent_enable_hint')}</p>

    {#if enabled}
      <div class={cn(settingsRowStacked, 'settings-field-full mt-3')}>
        <div class="grid grid-cols-[repeat(auto-fit,minmax(220px,1fr))] gap-4">
          <div class="settings-field flex flex-col gap-1.5">
            <label class={settingsRowLabel} for="snmp-agent-scope">{translate('settings.snmp_agent_scope')}</label>
            <select id="snmp-agent-scope" data-testid="snmp-agent-scope" class={settingsSelect} bind:value={scope}>
              <option value="loopback">{translate('settings.snmp_agent_scope_loopback')}</option>
              <option value="lan">{translate('settings.snmp_agent_scope_lan')}</option>
            </select>
            <p class={settingsHint}>{translate('settings.snmp_agent_scope_hint')}</p>
          </div>

          {#if scope === 'lan'}
            <div class="settings-field flex flex-col gap-1.5">
              <label class={settingsRowLabel} for="snmp-agent-address">{translate('settings.snmp_agent_address')}</label>
              {#if candidates.length > 0}
                <select id="snmp-agent-address" data-testid="snmp-agent-address" class={settingsSelect} bind:value={lanAddress}>
                  {#each candidates as address (address)}<option value={address}>{address}</option>{/each}
                </select>
              {:else}
                <input id="snmp-agent-address" class={settingsInput} bind:value={lanAddress} placeholder="192.168.1.10" />
              {/if}
              <p class={settingsHint}>{translate('settings.snmp_agent_address_hint')}</p>
            </div>
          {/if}

          <div class="settings-field flex flex-col gap-1.5">
            <label class={settingsRowLabel} for="snmp-agent-community">{translate('settings.snmp_agent_community')}</label>
            <input
              id="snmp-agent-community"
              data-testid="snmp-agent-community"
              class={settingsInput}
              bind:value={community}
              autocomplete="new-password"
              placeholder={hasCommunity ? translate('settings.snmp_agent_community_keep') : translate('settings.snmp_agent_community_generate')}
            />
            <p class={settingsHint}>{translate('settings.snmp_agent_community_hint')}</p>
          </div>

          <div class="settings-field flex flex-col gap-1.5">
            <label class={settingsRowLabel} for="snmp-agent-location">{translate('settings.snmp_agent_location')}</label>
            <input id="snmp-agent-location" class={settingsInput} bind:value={location} maxlength="128" />
          </div>

          <div class="settings-field flex flex-col gap-1.5">
            <label class={settingsRowLabel} for="snmp-agent-contact">{translate('settings.snmp_agent_contact')}</label>
            <input id="snmp-agent-contact" class={settingsInput} bind:value={contact} maxlength="128" />
            <p class={settingsHint}>{translate('settings.snmp_agent_contact_hint')}</p>
          </div>
        </div>
      </div>

      <div class={settingsRow}>
        <span class={settingsRowLabel}>{translate('settings.snmp_agent_state')}</span>
        <span data-testid="snmp-agent-state" class="text-xs font-semibold">
          {serving ? translate('settings.snmp_agent_running') : translate('settings.snmp_agent_stopped')}
        </span>
      </div>
    {/if}

    <div class={cn(settingsRow, 'mt-3')}>
      <span class={settingsHint}>{translate('settings.snmp_agent_save_hint')}</span>
      <button type="button" class={settingsBtn} data-testid="snmp-agent-save" onclick={save} disabled={saving}>
        {saving ? translate('settings.saving') : translate('common.save')}
      </button>
    </div>
  {/if}
</SettingsCard>

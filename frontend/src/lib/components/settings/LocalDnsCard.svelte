<!-- ========================================================================
     File          : LocalDnsCard.svelte
     Author        : Nicolas Le Roux
     Created       : 05/11/2026 by Nicolas Le Roux
     Version       : 1.0.0

     Description:
       The appliance's own DNS server (phase C1). One switch, the interface it
       answers on, its upstream forwarders, the local names it answers, the
       domains it answers instead of forwarding, and the switch that tells the
       LAN's DHCP clients to resolve here.

       It is the *same* `dnsmasq` as the DHCP card below it, which is why the
       card says so: turning DHCP off does not stop the daemon while this is
       on, and stopping the daemon from the DHCP card is refused while this is
       on. One daemon, one truth.

       Design rules kept here (CLAUDE.md, AGENTS.md):
       - off by default, and turning it on is the one confirmed action — a
         wrong record breaks name resolution for every device on the LAN;
       - every control is a 44px target, labelled, and reachable by keyboard;
       - the two lists are edited row by row (add / remove) so nothing depends
         on typing a comma-separated string correctly;
       - a refusal shows the server's own sentence (which names the rule it
         broke), never a generic "failed".

     Dependencies:
       - svelte (>= 5), $lib/services/api-phase4, $lib/stores/i18n,
         $lib/utils/notify, ./SettingsCard, ./vocabulary,
         $lib/components/theme/Modal

     Usage:
       Embedded in routes/Settings.svelte's security category, right after the
       DHCP server card. See docs/guides/local-dns.md.
     ======================================================================== -->
<script lang="ts">
  import { onMount } from 'svelte';
  import { translate } from '$lib/stores/i18n';
  import { notifyError, notifySuccess } from '$lib/utils/notify';
  import { getLocalDnsConfig, updateLocalDnsConfig } from '$lib/services/api-phase4';
  import type { DnsRecord, DnsRecordKind, LocalDnsConfig } from '$lib/types/phase4';
  import SettingsCard from './SettingsCard.svelte';
  import Modal from '$lib/components/theme/Modal.svelte';
  import { cn } from '$lib/utils.js';
  import {
    settingsRow,
    settingsRowStacked,
    settingsRowLabel,
    settingsRowHint,
    settingsHint,
    settingsInput,
    settingsSelect,
    settingsBtn,
    settingsBtnGhost,
    settingsBtnPrimary,
    settingsBtnDanger
  } from './vocabulary';

  let { class: klass = '' }: { class?: string } = $props();

  let loading = $state(true);
  let saving = $state(false);
  /** `dnsmasq` is not installed: the card names the package instead of
   *  offering a form that cannot work. */
  let unavailable = $state(false);
  /** Shown when the shared daemon is on for DHCP but this half is off. */
  let dhcpEnabled = $state(false);
  let running = $state(false);
  let serving = $state(false);
  let available = $state(false);

  let config = $state<LocalDnsConfig>({
    enabled: false,
    interface: '',
    serve_dhcp_clients: false,
    forwarders: [],
    records: [],
    overrides: [],
    confirmed: false
  });
  let interfaces = $state<string[]>([]);
  let systemForwarders = $state<string[]>([]);

  // New-row drafts. Kept out of `config` so a half-typed row is never saved.
  let newForwarder = $state('');
  let newName = $state('');
  let newNameKind = $state<DnsRecordKind>('a');
  let newNameValue = $state('');
  let newDomain = $state('');
  let newDomainAddress = $state('');

  let confirmOpen = $state(false);
  let loadError = $state('');

  onMount(load);

  async function load() {
    loading = true;
    loadError = '';
    try {
      const res = await getLocalDnsConfig();
      config = {
        ...res.config,
        forwarders: [...(res.config.forwarders ?? [])],
        records: [...(res.config.records ?? [])],
        overrides: [...(res.config.overrides ?? [])]
      };
      available = !!res.available;
      unavailable = !res.available;
      running = !!res.state.running;
      serving = !!res.state.serving;
      dhcpEnabled = !!res.dhcp_enabled;
      interfaces = res.interfaces ?? [];
      systemForwarders = res.system_forwarders ?? [];
      if (!config.interface && interfaces.length > 0) {
        config.interface = interfaces[0];
      }
    } catch (e) {
      loadError = e instanceof Error ? e.message : translate('settings.local_dns_load_failed');
    } finally {
      loading = false;
    }
  }

  /** Turning the server on is the one confirmed action. Everything else saves
   *  straight away: it cannot break the LAN by itself, and a second
   *  confirmation for editing a name would be noise. */
  async function save() {
    if (config.enabled && !config.confirmed) {
      confirmOpen = true;
      return;
    }
    await persist();
  }

  async function confirmAndSave() {
    config.confirmed = true;
    confirmOpen = false;
    await persist();
  }

  async function persist() {
    saving = true;
    try {
      const res = await updateLocalDnsConfig(config);
      config = {
        ...res.config,
        forwarders: [...(res.config.forwarders ?? [])],
        records: [...(res.config.records ?? [])],
        overrides: [...(res.config.overrides ?? [])]
      };
      serving = !!res.state.serving;
      running = !!res.state.running;
      notifySuccess('settings.local_dns_saved');
      await load();
    } catch (e) {
      notifyError(
        e instanceof Error ? { success: false, message: e.message } : null,
        'settings.local_dns_save_failed'
      );
    } finally {
      saving = false;
    }
  }

  function addForwarder(value: string) {
    const address = value.trim();
    if (!address || config.forwarders.includes(address)) {
      newForwarder = '';
      return;
    }
    config.forwarders = [...config.forwarders, address];
    // An edited list invalidates the confirmation the last save carried.
    config.confirmed = config.enabled ? false : config.confirmed;
    newForwarder = '';
  }

  function removeForwarder(address: string) {
    config.forwarders = config.forwarders.filter((f) => f !== address);
    config.confirmed = config.enabled ? false : config.confirmed;
  }

  function addRecord() {
    const name = newName.trim();
    const value = newNameValue.trim();
    if (!name || !value) return;
    const record: DnsRecord = { name, kind: newNameKind, value };
    config.records = [...config.records, record];
    config.confirmed = config.enabled ? false : config.confirmed;
    newName = '';
    newNameValue = '';
  }

  function removeRecord(index: number) {
    config.records = config.records.filter((_, i) => i !== index);
    config.confirmed = config.enabled ? false : config.confirmed;
  }

  function addOverride() {
    const domain = newDomain.trim();
    const address = newDomainAddress.trim();
    if (!domain || !address) return;
    config.overrides = [...config.overrides, { domain, address }];
    config.confirmed = config.enabled ? false : config.confirmed;
    newDomain = '';
    newDomainAddress = '';
  }

  function removeOverride(index: number) {
    config.overrides = config.overrides.filter((_, i) => i !== index);
    config.confirmed = config.enabled ? false : config.confirmed;
  }
</script>

<SettingsCard
  title={translate('settings.local_dns_title')}
  subtitle={translate('settings.local_dns_subtitle')}
  class={cn('local-dns-card', klass)}
  data-testid="local-dns-card"
>
  {#if loading}
    <p class={settingsHint}>{translate('settings.loading')}</p>
  {:else}
    {#if unavailable}
      <div class="local-dns-alert" role="status" data-testid="local-dns-unavailable">
        <p>{translate('settings.local_dns_unavailable')}</p>
      </div>
    {/if}

    {#if loadError}
      <div class="local-dns-alert local-dns-alert-error" role="alert" data-testid="local-dns-error">
        {loadError}
      </div>
    {/if}

    <div class={settingsRow}>
      <div class="min-w-0">
        <span class={settingsRowLabel} id="local-dns-enable-label">
          {translate('settings.local_dns_enable')}
        </span>
        <p class={settingsRowHint}>{translate('settings.local_dns_enable_hint')}</p>
      </div>
      <input
        id="local-dns-enabled"
        type="checkbox"
        class="local-dns-check"
        bind:checked={config.enabled}
        aria-labelledby="local-dns-enable-label"
        disabled={unavailable}
        data-testid="local-dns-enabled"
      />
    </div>

    {#if config.enabled}
      <div class={settingsRow}>
        <div class="min-w-0">
          <label class={settingsRowLabel} for="local-dns-interface">
            {translate('settings.local_dns_interface')}
          </label>
          <p class={settingsRowHint}>{translate('settings.local_dns_interface_hint')}</p>
        </div>
        {#if interfaces.length > 0}
          <select
            id="local-dns-interface"
            class={settingsSelect}
            bind:value={config.interface}
            data-testid="local-dns-interface"
          >
            {#each interfaces as iface (iface)}
              <option value={iface}>{iface}</option>
            {/each}
          </select>
        {:else}
          <input
            id="local-dns-interface"
            class={settingsInput}
            bind:value={config.interface}
            placeholder="eth0"
            data-testid="local-dns-interface"
          />
        {/if}
      </div>

      <!-- Upstream forwarders -->
      <h4 class="local-dns-subhead">{translate('settings.local_dns_forwarders_title')}</h4>
      <p class={settingsHint}>{translate('settings.local_dns_forwarders_hint')}</p>

      {#if config.forwarders.length === 0}
        <p class={settingsHint} data-testid="local-dns-no-forwarders">
          {translate('settings.local_dns_forwarders_empty')}
        </p>
      {:else}
        <ul class="local-dns-list" data-testid="local-dns-forwarders">
          {#each config.forwarders as forwarder (forwarder)}
            <li class="local-dns-item">
              <span class="local-dns-mono">{forwarder}</span>
              <button
                type="button"
                class={settingsBtnGhost}
                onclick={() => removeForwarder(forwarder)}
                aria-label={translate('settings.local_dns_remove_forwarder', { address: forwarder })}
                data-testid={`local-dns-remove-forwarder-${forwarder}`}
              >
                {translate('common.remove')}
              </button>
            </li>
          {/each}
        </ul>
      {/if}

      <div class="local-dns-add">
        <label class="local-dns-sr-only" for="local-dns-new-forwarder">
          {translate('settings.local_dns_forwarder_address')}
        </label>
        <input
          id="local-dns-new-forwarder"
          class={settingsInput}
          bind:value={newForwarder}
          placeholder="192.168.1.1"
          data-testid="local-dns-new-forwarder"
          onkeydown={(e) => {
            if (e.key === 'Enter') {
              e.preventDefault();
              addForwarder(newForwarder);
            }
          }}
        />
        <button
          type="button"
          class={settingsBtn}
          onclick={() => addForwarder(newForwarder)}
          data-testid="local-dns-add-forwarder"
        >
          {translate('settings.local_dns_add_forwarder')}
        </button>
      </div>
      {#if systemForwarders.length > 0 && config.forwarders.length === 0}
        <p class={settingsHint}>
          {translate('settings.local_dns_forwarders_suggested')}
        </p>
        <div class="local-dns-add">
          {#each systemForwarders as suggested (suggested)}
            <button
              type="button"
              class={settingsBtnGhost}
              onclick={() => addForwarder(suggested)}
              data-testid={`local-dns-suggest-${suggested}`}
            >
              {suggested}
            </button>
          {/each}
        </div>
      {/if}

      <!-- Local names -->
      <h4 class="local-dns-subhead">{translate('settings.local_dns_records_title')}</h4>
      <p class={settingsHint}>{translate('settings.local_dns_records_hint')}</p>

      {#if config.records.length === 0}
        <p class={settingsHint} data-testid="local-dns-no-records">
          {translate('settings.local_dns_records_empty')}
        </p>
      {:else}
        <table class="local-dns-table" data-testid="local-dns-records">
          <thead>
            <tr>
              <th scope="col">{translate('settings.local_dns_name')}</th>
              <th scope="col">{translate('settings.local_dns_kind')}</th>
              <th scope="col">{translate('settings.local_dns_value')}</th>
              <th scope="col">{translate('common.actions')}</th>
            </tr>
          </thead>
          <tbody>
            {#each config.records as record, index (record.name)}
              <tr>
                <td>{record.name}</td>
                <td>
                  {record.kind === 'cname'
                    ? translate('settings.local_dns_kind_cname')
                    : translate('settings.local_dns_kind_a')}
                </td>
                <td>{record.value}</td>
                <td>
                  <button
                    type="button"
                    class={settingsBtnGhost}
                    onclick={() => removeRecord(index)}
                    aria-label={translate('settings.local_dns_remove_record', { name: record.name })}
                    data-testid={`local-dns-remove-record-${record.name}`}
                  >
                    {translate('common.remove')}
                  </button>
                </td>
              </tr>
            {/each}
          </tbody>
        </table>
      {/if}

      <div class="local-dns-add local-dns-add-record">
        <label class="local-dns-sr-only" for="local-dns-new-name">
          {translate('settings.local_dns_name')}
        </label>
        <input
          id="local-dns-new-name"
          class={settingsInput}
          bind:value={newName}
          placeholder="nas.lan"
          data-testid="local-dns-new-name"
        />
        <label class="local-dns-sr-only" for="local-dns-new-kind">
          {translate('settings.local_dns_kind')}
        </label>
        <select
          id="local-dns-new-kind"
          class={settingsSelect}
          bind:value={newNameKind}
          data-testid="local-dns-new-kind"
        >
          <option value="a">{translate('settings.local_dns_kind_a')}</option>
          <option value="cname">{translate('settings.local_dns_kind_cname')}</option>
        </select>
        <label class="local-dns-sr-only" for="local-dns-new-value">
          {translate('settings.local_dns_value')}
        </label>
        <input
          id="local-dns-new-value"
          class={settingsInput}
          bind:value={newNameValue}
          placeholder={newNameKind === 'cname' ? 'nas.lan' : '192.168.1.5'}
          data-testid="local-dns-new-value"
        />
        <button
          type="button"
          class={settingsBtn}
          onclick={addRecord}
          data-testid="local-dns-add-record"
        >
          {translate('settings.local_dns_add_record')}
        </button>
      </div>

      <!-- Answered domains -->
      <h4 class="local-dns-subhead">{translate('settings.local_dns_overrides_title')}</h4>
      <p class={settingsHint}>{translate('settings.local_dns_overrides_hint')}</p>

      {#if config.overrides.length === 0}
        <p class={settingsHint} data-testid="local-dns-no-overrides">
          {translate('settings.local_dns_overrides_empty')}
        </p>
      {:else}
        <table class="local-dns-table" data-testid="local-dns-overrides">
          <thead>
            <tr>
              <th scope="col">{translate('settings.local_dns_domain')}</th>
              <th scope="col">{translate('settings.local_dns_value')}</th>
              <th scope="col">{translate('common.actions')}</th>
            </tr>
          </thead>
          <tbody>
            {#each config.overrides as override, index (override.domain)}
              <tr>
                <td>{override.domain}</td>
                <td>{override.address}</td>
                <td>
                  <button
                    type="button"
                    class={settingsBtnGhost}
                    onclick={() => removeOverride(index)}
                    aria-label={translate('settings.local_dns_remove_override', {
                      domain: override.domain
                    })}
                    data-testid={`local-dns-remove-override-${override.domain}`}
                  >
                    {translate('common.remove')}
                  </button>
                </td>
              </tr>
            {/each}
          </tbody>
        </table>
      {/if}

      <div class="local-dns-add">
        <label class="local-dns-sr-only" for="local-dns-new-domain">
          {translate('settings.local_dns_domain')}
        </label>
        <input
          id="local-dns-new-domain"
          class={settingsInput}
          bind:value={newDomain}
          placeholder=".corp.lan"
          data-testid="local-dns-new-domain"
        />
        <label class="local-dns-sr-only" for="local-dns-new-domain-address">
          {translate('settings.local_dns_value')}
        </label>
        <input
          id="local-dns-new-domain-address"
          class={settingsInput}
          bind:value={newDomainAddress}
          placeholder="10.0.0.1"
          data-testid="local-dns-new-domain-address"
        />
        <button
          type="button"
          class={settingsBtn}
          onclick={addOverride}
          data-testid="local-dns-add-override"
        >
          {translate('settings.local_dns_add_override')}
        </button>
      </div>

      <!-- Tell the LAN's DHCP clients to resolve here -->
      <div class={settingsRowStacked}>
        <div class="flex items-center gap-3">
          <input
            id="local-dns-serve-dhcp"
            type="checkbox"
            class="local-dns-check"
            bind:checked={config.serve_dhcp_clients}
            disabled={!config.enabled}
            data-testid="local-dns-serve-dhcp"
          />
          <label class={settingsRowLabel} for="local-dns-serve-dhcp">
            {translate('settings.local_dns_serve_dhcp')}
          </label>
        </div>
        <p class={settingsHint}>
          {dhcpEnabled
            ? translate('settings.local_dns_serve_dhcp_hint')
            : translate('settings.local_dns_serve_dhcp_needs_dhcp')}
        </p>
      </div>

      <div class={cn(settingsRow, 'local-dns-status-row')}>
        <span class={settingsRowLabel}>{translate('settings.local_dns_state')}</span>
        <span
          class={cn('local-dns-badge', serving ? 'local-dns-badge-on' : 'local-dns-badge-off')}
          data-testid="local-dns-state"
          data-state={serving ? 'serving' : running ? 'shared' : 'stopped'}
        >
          {serving
            ? translate('settings.local_dns_serving')
            : running
              ? translate('settings.local_dns_not_enabled_running')
              : translate('settings.local_dns_stopped')}
        </span>
      </div>
      <p class={settingsHint}>{translate('settings.local_dns_shared_daemon')}</p>
    {/if}

    {#if !unavailable}
      <div class={cn(settingsRow, 'local-dns-actions')}>
        <span class={settingsHint}>{translate('settings.local_dns_save_hint')}</span>
        <button
          type="button"
          class={settingsBtnPrimary}
          onclick={save}
          disabled={saving}
          data-testid="local-dns-save"
        >
          {saving ? translate('settings.saving') : translate('common.save')}
        </button>
      </div>
    {/if}
  {/if}
</SettingsCard>

<!-- Turning the DNS server on is confirmed: a wrong record here breaks name
     resolution for every device on the LAN. The shared Modal traps focus and
     restores it to the trigger on close. -->
<Modal
  open={confirmOpen}
  title={translate('settings.local_dns_confirm_title')}
  size="sm"
  onclose={() => (confirmOpen = false)}
>
  <div class="local-dns-confirm">
    <p>{translate('settings.local_dns_confirm_only_if')}</p>
    <ul>
      <li>{translate('settings.local_dns_confirm_forwarder')}</li>
      <li>{translate('settings.local_dns_confirm_clients')}</li>
      <li>{translate('settings.local_dns_confirm_records')}</li>
    </ul>
    <div class="local-dns-actions">
      <button type="button" class={settingsBtn} onclick={() => (confirmOpen = false)}>
        {translate('common.cancel')}
      </button>
      <button
        type="button"
        class={settingsBtnDanger}
        onclick={confirmAndSave}
        data-testid="local-dns-confirm-enable"
      >
        {translate('settings.local_dns_confirm_understand')}
      </button>
    </div>
  </div>
</Modal>

<style>
  .local-dns-alert {
    border: 1px solid var(--hv-shell-border);
    border-radius: 8px;
    padding: 10px 12px;
    margin-bottom: 12px;
    font-size: 0.8125rem;
    line-height: 1.25rem;
    color: var(--hv-shell-text);
    background: var(--hv-shell-border-soft);
  }

  .local-dns-alert-error {
    background: var(--hv-error-bg);
    border-color: var(--hv-error);
    color: var(--hv-error);
  }

  /* A bare checkbox inherits `width: 100%` from base.css (design-system
     pitfall 6); pin the box so it does not stretch across its label. */
  .local-dns-check {
    height: 1rem;
    width: 1rem;
    flex: none;
    accent-color: var(--hv-primary);
  }

  .local-dns-subhead {
    margin: 16px 0 8px;
    font-size: 0.8125rem;
    font-weight: 600;
    color: var(--hv-settings-title);
  }

  .local-dns-list {
    display: flex;
    flex-direction: column;
    gap: 8px;
    margin: 0 0 12px;
    padding: 0;
    list-style: none;
  }

  .local-dns-item {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    padding: 6px 10px;
    border: 1px solid var(--hv-shell-border);
    border-radius: 8px;
  }

  .local-dns-mono {
    font-family: var(--hv-font-mono);
    font-size: 0.8125rem;
    color: var(--hv-shell-text);
  }

  .local-dns-table {
    width: 100%;
    border-collapse: collapse;
    margin-bottom: 12px;
    font-size: 0.8125rem;
  }

  .local-dns-table th,
  .local-dns-table td {
    padding: 6px 10px;
    text-align: left;
    border-bottom: 1px solid var(--hv-shell-border);
  }

  .local-dns-table th {
    font-weight: 600;
    color: var(--hv-shell-text-3);
    font-size: 0.75rem;
  }

  .local-dns-table td {
    color: var(--hv-shell-text);
    font-family: var(--hv-font-mono);
  }

  .local-dns-add {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
    align-items: center;
    margin-bottom: 8px;
  }

  .local-dns-add-record {
    align-items: stretch;
  }

  .local-dns-badge {
    font-size: 0.6875rem;
    padding: 2px 10px;
    border-radius: 999px;
    font-weight: 600;
  }

  .local-dns-badge-on {
    background: var(--hv-success-bg);
    color: var(--hv-success);
  }

  .local-dns-badge-off {
    background: var(--hv-shell-border-soft);
    color: var(--hv-shell-text-2);
  }

  .local-dns-actions {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
    align-items: center;
    justify-content: space-between;
    margin-top: 16px;
  }

  .local-dns-confirm ul {
    margin: 8px 0 16px;
    padding-left: 18px;
    font-size: 0.8125rem;
    color: var(--hv-shell-text-2);
  }

  .local-dns-sr-only {
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

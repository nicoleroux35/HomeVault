<!-- ========================================================================
     File          : UpnpCard.svelte
     Author        : Nicolas Le Roux
     Created       : 06/11/2026 by Nicolas Le Roux
     Version       : 1.0.0

     Description:
       Router port forwarding through UPnP-IGD (phase C3). Read-only
       discovery first: the external address, the gateway and the mappings the
       router already has. Adding a mapping is a named, confirmed action; the
       same list removes it again, so every change is reversible from here.

       Design rules kept here (CLAUDE.md, AGENTS.md):
       - the capability is the first thing shown: no `upnpc`, no dead form;
       - a discovery failure says why, instead of an empty table that looks
         like "no mapping";
       - the confirmation names the consequence (a port open on the router);
       - every control is a 44px target, labelled and keyboard reachable;
       - the server's own refusal sentence is shown verbatim.

     Dependencies:
       - svelte (>= 5), $lib/services/api-phase4, $lib/stores/i18n,
         $lib/utils/notify, ./SettingsCard, ./vocabulary, theme/Modal

     Usage:
       Embedded in routes/Settings.svelte's security category, after the
       reverse-proxy card. See docs/guides/upnp.md.
     ======================================================================== -->
<script lang="ts">
  import { onMount } from 'svelte';
  import { translate } from '$lib/stores/i18n';
  import { notifyError, notifySuccess } from '$lib/utils/notify';
  import { addUpnpMapping, getUpnpState, removeUpnpMapping } from '$lib/services/api-phase4';
  import type { UpnpMapping, UpnpProtocol } from '$lib/types/phase4';
  import SettingsCard from './SettingsCard.svelte';
  import Modal from '$lib/components/theme/Modal.svelte';
  import { cn } from '$lib/utils.js';
  import {
    settingsRowLabel,
    settingsRowHint,
    settingsHint,
    settingsInput,
    settingsSelect,
    settingsBtn,
    settingsBtnPrimary,
    settingsBtnDanger
  } from './vocabulary';

  let { class: klass = '' }: { class?: string } = $props();

  let loading = $state(true);
  let saving = $state(false);
  let loadError = $state('');
  let available = $state(false);
  let discoveryError = $state('');
  let gateway = $state<string | null>(null);
  let externalIp = $state<string | null>(null);
  let localAddress = $state<string | null>(null);
  let mappings = $state<UpnpMapping[]>([]);
  let suggestedHost = $state('');

  let editorOpen = $state(false);
  let formError = $state('');
  let form = $state({
    protocol: 'tcp' as UpnpProtocol,
    external_port: '8080',
    internal_host: '',
    internal_port: '80',
    description: '',
    lease_seconds: 3600,
    confirmed: false
  });

  let removeTarget = $state<UpnpMapping | null>(null);
  let removing = $state(false);

  onMount(load);

  async function load() {
    loading = true;
    loadError = '';
    try {
      const res = await getUpnpState();
      available = !!res.available;
      discoveryError = res.error ?? '';
      gateway = res.gateway ?? null;
      externalIp = res.external_ip ?? null;
      localAddress = res.local_address ?? null;
      mappings = res.mappings ?? [];
      suggestedHost = res.suggested_host ?? localAddress ?? '';
    } catch (e) {
      loadError = e instanceof Error ? e.message : translate('settings.upnp_load_failed');
    } finally {
      loading = false;
    }
  }

  function openAdd() {
    form = {
      protocol: 'tcp',
      external_port: '8080',
      internal_host: suggestedHost,
      internal_port: '80',
      description: '',
      lease_seconds: 3600,
      confirmed: false
    };
    formError = '';
    editorOpen = true;
  }

  function portValue(raw: string): number {
    const value = Number(raw);
    return Number.isInteger(value) && value >= 1 && value <= 65535 ? value : 0;
  }

  async function submit() {
    if (saving) return;
    const externalPort = portValue(form.external_port);
    const internalPort = portValue(form.internal_port);
    if (!externalPort || !internalPort) {
      formError = translate('settings.upnp_port_invalid');
      return;
    }
    if (!form.internal_host.trim()) {
      formError = translate('settings.upnp_host_required');
      return;
    }
    if (!form.confirmed) {
      formError = translate('settings.upnp_confirm_required');
      return;
    }

    saving = true;
    formError = '';
    try {
      const res = await addUpnpMapping({
        protocol: form.protocol,
        external_port: externalPort,
        internal_host: form.internal_host.trim(),
        internal_port: internalPort,
        description: form.description.trim(),
        lease_seconds: form.lease_seconds,
        confirmed: true
      });
      applySnapshot(res.snapshot);
      if (res.error) {
        discoveryError = res.error;
      }
      editorOpen = false;
      notifySuccess('settings.upnp_added');
      await load();
    } catch (e) {
      formError = e instanceof Error ? e.message : translate('settings.upnp_save_failed');
    } finally {
      saving = false;
    }
  }

  async function confirmRemove() {
    const target = removeTarget;
    if (!target || removing) return;
    removing = true;
    try {
      const res = await removeUpnpMapping(target.protocol, target.external_port);
      applySnapshot(res.snapshot);
      if (res.error) {
        discoveryError = res.error;
      }
      removeTarget = null;
      notifySuccess('settings.upnp_removed');
      await load();
    } catch (e) {
      notifyError(
        e instanceof Error ? { success: false, message: e.message } : null,
        'settings.upnp_remove_failed'
      );
    } finally {
      removing = false;
    }
  }

  function applySnapshot(snapshot: { mappings?: UpnpMapping[]; external_ip?: string | null; gateway?: string | null; local_address?: string | null } | null) {
    if (!snapshot) return;
    mappings = snapshot.mappings ?? [];
    externalIp = snapshot.external_ip ?? null;
    gateway = snapshot.gateway ?? null;
    localAddress = snapshot.local_address ?? null;
  }

  function leaseLabel(seconds: number): string {
    if (seconds === 0) return translate('settings.upnp_lease_permanent_short');
    if (seconds >= 86400) return `${Math.round(seconds / 86400)} d`;
    if (seconds >= 3600) return `${Math.round(seconds / 3600)} h`;
    return `${seconds} s`;
  }

  function mappingKey(mapping: UpnpMapping): string {
    return `${mapping.protocol.toUpperCase()} ${mapping.external_port}`;
  }
</script>

<SettingsCard
  title={translate('settings.upnp_title')}
  subtitle={translate('settings.upnp_subtitle')}
  class={cn('upnp-card', klass)}
  data-testid="upnp-card"
>
  {#if loading}
    <p class={settingsHint}>{translate('settings.loading')}</p>
  {:else}
    {#if loadError}
      <div class="upnp-alert upnp-alert-error" role="alert" data-testid="upnp-error">
        {loadError}
      </div>
    {:else if !available}
      <div class="upnp-alert" role="status" data-testid="upnp-unavailable">
        {translate('settings.upnp_unavailable')}
      </div>
    {:else}
      {#if discoveryError}
        <div class="upnp-alert upnp-alert-error" role="alert" data-testid="upnp-discovery-error">
          {translate('settings.upnp_discovery_failed')}
          <span class="upnp-reason">{discoveryError}</span>
        </div>
      {/if}

      <dl class="upnp-facts">
        <div class="upnp-fact">
          <dt>{translate('settings.upnp_external_ip')}</dt>
          <dd data-testid="upnp-external-ip">
            {externalIp ?? translate('settings.upnp_unknown')}
          </dd>
        </div>
        <div class="upnp-fact">
          <dt>{translate('settings.upnp_gateway')}</dt>
          <dd data-testid="upnp-gateway">{gateway ?? translate('settings.upnp_unknown')}</dd>
        </div>
        <div class="upnp-fact">
          <dt>{translate('settings.upnp_local')}</dt>
          <dd data-testid="upnp-local">{localAddress ?? translate('settings.upnp_unknown')}</dd>
        </div>
      </dl>

      <h4 class="upnp-subhead">{translate('settings.upnp_mappings_title')}</h4>
      {#if mappings.length === 0}
        <p class={settingsHint} data-testid="upnp-empty">
          {translate('settings.upnp_mappings_empty')}
        </p>
      {:else}
        <ul class="upnp-list" data-testid="upnp-mappings">
          {#each mappings as mapping (mappingKey(mapping))}
            <li class="upnp-item" data-testid="upnp-mapping">
              <div class="upnp-item-main">
                <span class="upnp-badge">{mapping.protocol.toUpperCase()}</span>
                <span class="upnp-ports" data-testid="upnp-mapping-key">
                  {mapping.external_port}
                </span>
                <span class="upnp-arrow" aria-hidden="true">→</span>
                <span class="upnp-ports" data-testid="upnp-mapping-target">
                  {mapping.internal_host}:{mapping.internal_port}
                </span>
                <span class="upnp-description">{mapping.description}</span>
                <span class="upnp-lease">{leaseLabel(mapping.lease_seconds)}</span>
              </div>
              <button
                type="button"
                class={settingsBtnDanger}
                onclick={() => (removeTarget = mapping)}
                aria-label={translate('settings.upnp_remove_aria', {
                  mapping: `${mapping.protocol.toUpperCase()} ${mapping.external_port}`
                })}
                data-testid="upnp-remove"
              >
                {translate('settings.upnp_remove_short')}
              </button>
            </li>
          {/each}
        </ul>
      {/if}

      <div class="upnp-actions">
        <button
          type="button"
          class={settingsBtnPrimary}
          onclick={openAdd}
          data-testid="upnp-add"
        >
          {translate('settings.upnp_add')}
        </button>
      </div>
      <p class={settingsHint}>{translate('settings.upnp_hint')}</p>
    {/if}
  {/if}
</SettingsCard>

<Modal
  open={editorOpen}
  title={translate('settings.upnp_add_title')}
  size="md"
  onclose={() => (editorOpen = false)}
>
  <div class="upnp-form">
    {#if formError}
      <div class="upnp-alert upnp-alert-error" role="alert" data-testid="upnp-form-error">
        {formError}
      </div>
    {/if}

    <div class="upnp-field">
      <label class={settingsRowLabel} for="upnp-protocol">
        {translate('settings.upnp_protocol')}
      </label>
      <select
        id="upnp-protocol"
        class={settingsSelect}
        bind:value={form.protocol}
        data-testid="upnp-protocol"
      >
        <option value="tcp">TCP</option>
        <option value="udp">UDP</option>
      </select>
    </div>

    <div class="upnp-field">
      <label class={settingsRowLabel} for="upnp-external-port">
        {translate('settings.upnp_external_port')}
      </label>
      <p class={settingsRowHint} id="upnp-external-port-hint">
        {translate('settings.upnp_external_port_hint')}
      </p>
      <input
        id="upnp-external-port"
        class={settingsInput}
        type="number"
        min="1"
        max="65535"
        bind:value={form.external_port}
        aria-describedby="upnp-external-port-hint"
        data-testid="upnp-external-port"
      />
    </div>

    <div class="upnp-field">
      <label class={settingsRowLabel} for="upnp-internal-host">
        {translate('settings.upnp_internal_host')}
      </label>
      <p class={settingsRowHint} id="upnp-internal-host-hint">
        {translate('settings.upnp_internal_host_hint')}
      </p>
      <input
        id="upnp-internal-host"
        class={settingsInput}
        bind:value={form.internal_host}
        placeholder="192.168.1.50"
        autocomplete="off"
        autocapitalize="none"
        spellcheck="false"
        aria-describedby="upnp-internal-host-hint"
        data-testid="upnp-internal-host"
      />
    </div>

    <div class="upnp-field">
      <label class={settingsRowLabel} for="upnp-internal-port">
        {translate('settings.upnp_internal_port')}
      </label>
      <input
        id="upnp-internal-port"
        class={settingsInput}
        type="number"
        min="1"
        max="65535"
        bind:value={form.internal_port}
        data-testid="upnp-internal-port"
      />
    </div>

    <div class="upnp-field">
      <label class={settingsRowLabel} for="upnp-description">
        {translate('settings.upnp_description')}
      </label>
      <input
        id="upnp-description"
        class={settingsInput}
        bind:value={form.description}
        maxlength="48"
        autocomplete="off"
        data-testid="upnp-description"
      />
    </div>

    <div class="upnp-field">
      <label class={settingsRowLabel} for="upnp-lease">
        {translate('settings.upnp_lease')}
      </label>
      <select
        id="upnp-lease"
        class={settingsSelect}
        bind:value={form.lease_seconds}
        data-testid="upnp-lease"
      >
        <option value={3600}>{translate('settings.upnp_lease_hour')}</option>
        <option value={21600}>{translate('settings.upnp_lease_6h')}</option>
        <option value={86400}>{translate('settings.upnp_lease_day')}</option>
        <option value={604800}>{translate('settings.upnp_lease_week')}</option>
        <option value={0}>{translate('settings.upnp_lease_permanent')}</option>
      </select>
      {#if form.lease_seconds === 0}
        <p class={settingsRowHint}>{translate('settings.upnp_lease_permanent_hint')}</p>
      {/if}
    </div>

    <label class="upnp-check-row" for="upnp-confirm">
      <input
        id="upnp-confirm"
        type="checkbox"
        bind:checked={form.confirmed}
        data-testid="upnp-confirm"
      />
      <span>
        <span class={settingsRowLabel}>{translate('settings.upnp_confirm')}</span>
        <span class={settingsRowHint}>{translate('settings.upnp_confirm_hint')}</span>
      </span>
    </label>

    <div class="upnp-form-actions">
      <button type="button" class={settingsBtn} onclick={() => (editorOpen = false)}>
        {translate('common.cancel')}
      </button>
      <button
        type="button"
        class={settingsBtnPrimary}
        onclick={submit}
        disabled={saving || !form.confirmed}
        data-testid="upnp-save"
      >
        {translate('settings.upnp_save')}
      </button>
    </div>
  </div>
</Modal>

<Modal
  open={removeTarget !== null}
  title={translate('settings.upnp_remove_title')}
  size="sm"
  onclose={() => (removeTarget = null)}
>
  <div class="upnp-form">
    <p>
      {translate('settings.upnp_remove_confirm', {
        mapping: removeTarget ? mappingKey(removeTarget) : '',
        target: removeTarget
          ? `${removeTarget.internal_host}:${removeTarget.internal_port}`
          : ''
      })}
    </p>
    <div class="upnp-form-actions">
      <button type="button" class={settingsBtn} onclick={() => (removeTarget = null)}>
        {translate('common.cancel')}
      </button>
      <button
        type="button"
        class={settingsBtnDanger}
        onclick={confirmRemove}
        disabled={removing}
        data-testid="upnp-remove-confirm"
      >
        {translate('settings.upnp_remove_short')}
      </button>
    </div>
  </div>
</Modal>

<style>
  .upnp-alert {
    border: 1px solid var(--hv-shell-border);
    border-radius: 8px;
    padding: 10px 12px;
    margin-bottom: 12px;
    font-size: 0.8125rem;
    line-height: 1.25rem;
    color: var(--hv-shell-text);
    background: var(--hv-shell-border-soft);
  }

  .upnp-alert-error {
    border-color: var(--hv-danger, #b42318);
  }

  .upnp-reason {
    display: block;
    margin-top: 4px;
    overflow-wrap: anywhere;
  }

  .upnp-facts {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(180px, 1fr));
    gap: 8px;
    margin: 8px 0 14px;
  }

  .upnp-fact dt {
    font-size: 0.75rem;
    color: var(--hv-settings-label);
  }

  .upnp-fact dd {
    margin: 2px 0 0;
    font-family: var(--hv-font-mono, ui-monospace, monospace);
    font-size: 0.8125rem;
    overflow-wrap: anywhere;
  }

  .upnp-subhead {
    font-size: 0.8125rem;
    font-weight: 600;
    color: var(--hv-settings-title);
    margin: 10px 0 6px;
  }

  .upnp-list {
    list-style: none;
    margin: 0 0 10px;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .upnp-item {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    padding: 8px 10px;
    border: 1px solid var(--hv-shell-border);
    border-radius: 8px;
  }

  .upnp-item-main {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px;
    min-width: 0;
  }

  .upnp-badge {
    border: 1px solid var(--hv-shell-border);
    border-radius: 999px;
    padding: 0 8px;
    font-size: 0.6875rem;
    line-height: 1.25rem;
  }

  .upnp-ports {
    font-family: var(--hv-font-mono, ui-monospace, monospace);
    font-size: 0.8125rem;
  }

  .upnp-arrow {
    color: var(--hv-settings-label);
  }

  .upnp-description,
  .upnp-lease {
    font-size: 0.75rem;
    color: var(--hv-settings-label);
    overflow-wrap: anywhere;
  }

  .upnp-actions {
    display: flex;
    justify-content: flex-end;
  }

  .upnp-form {
    display: flex;
    flex-direction: column;
    gap: 14px;
  }

  .upnp-field {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  .upnp-check-row {
    display: flex;
    align-items: flex-start;
    gap: 10px;
    cursor: pointer;
  }

  .upnp-check-row input {
    width: 20px;
    height: 20px;
    margin-top: 2px;
    flex: none;
  }

  .upnp-form-actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
    margin-top: 6px;
  }
</style>

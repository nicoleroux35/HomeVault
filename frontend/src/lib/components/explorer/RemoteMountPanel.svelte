<!-- ========================================================================
     File          : RemoteMountPanel.svelte
     Author        : Nicolas Le Roux
     Created       : 09/11/2026 by Nicolas Le Roux
     Version       : 1.0.0

     Description:
       The Explorer sidebar's "Network locations" section: remote SMB/CIFS and
       NFS shares mounted under /mnt/homevault-remote, with mount/unmount and
       an "Add" dialog.

       Only an administrator sees it: a network location carries a credential
       and runs root-owned `mount`, and the server answers 403 to anyone else,
       so rendering it for a member would be a button that always fails.

       The panel is honest about what it cannot do: when neither `mount.cifs`
       nor `mount.nfs` is installed the list is replaced by a sentence naming
       the missing tool and the Add action is withheld — never a button that
       always fails (docs/API.md, capability contract).

     Dependencies:
       - svelte (>= 5), $lib/services/api, $lib/stores/i18n,
         $lib/components/theme/{Button, Modal}

     Usage:
       Rendered by routes/Explorer.svelte inside its sidebar (admins only).
       See docs/guides/remote-mounts.md.
     ======================================================================== -->
<script lang="ts">
  import { onMount } from 'svelte';
  import { api } from '$lib/services/api';
  import { t } from '$lib/stores/i18n';
  import { Button, Modal } from '$lib/components/theme';
  import type { RemoteMountKind, RemoteMountRequest, RemoteMountView } from '$lib/types/explorer';
  import {
    IconCloud,
    IconNetwork,
    IconPlugConnected,
    IconPlus,
    IconTrash,
    IconPlugOff
  } from '$lib/icons/lucide';

  let {
    isAdmin = false,
    askConfirm,
    onmounted,
    onunmounted
  }: {
    /** Only an administrator may manage network locations. */
    isAdmin?: boolean;
    /** The app's own accessible confirm dialog (never `window.confirm`). */
    askConfirm?: (message: string) => Promise<boolean>;
    onmounted?: (mountPoint: string) => void;
    onunmounted?: (mountPoint: string) => void;
  } = $props();

  let loading = $state(true);
  let busy = $state<string | null>(null);
  let notice = $state<{ kind: 'ok' | 'error'; text: string } | null>(null);
  let available = $state(true);
  let kinds = $state<string[]>([]);
  let mounts = $state<RemoteMountView[]>([]);

  // Add/edit dialog. A password is only ever sent when it was typed, so an
  // edit that changes an option never rewrites the stored credential.
  let dialogOpen = $state(false);
  let editingName = $state<string | null>(null);
  let form = $state({
    name: '',
    kind: 'cifs' as RemoteMountKind,
    server: '',
    source: '',
    username: '',
    password: '',
    domain: '',
    read_only: true,
    enabled: true,
    mount_now: true
  });
  let saving = $state(false);
  let dialogError = $state('');

  onMount(() => {
    if (isAdmin) void load();
  });

  async function load() {
    loading = true;
    const res = await api.listRemoteMounts();
    if (res.success && res.data) {
      available = !!res.data.available;
      kinds = res.data.kinds ?? [];
      mounts = res.data.mounts ?? [];
    } else {
      // A member hitting the 403 gets nothing rendered, not an error card.
      notice = null;
    }
    loading = false;
  }

  function openAdd() {
    editingName = null;
    form = {
      name: '',
      kind: (kinds.includes('cifs') ? 'cifs' : 'nfs') as RemoteMountKind,
      server: '',
      source: '',
      username: '',
      password: '',
      domain: '',
      read_only: true,
      enabled: true,
      mount_now: true
    };
    dialogError = '';
    dialogOpen = true;
  }

  function openEdit(mount: RemoteMountView) {
    editingName = mount.name;
    form = {
      name: mount.name,
      kind: mount.kind,
      server: mount.server,
      source: mount.source,
      username: mount.username ?? '',
      password: '',
      domain: mount.domain ?? '',
      read_only: mount.read_only,
      enabled: mount.enabled,
      mount_now: false
    };
    dialogError = '';
    dialogOpen = true;
  }

  async function save() {
    saving = true;
    dialogError = '';
    const payload: RemoteMountRequest = {
      kind: form.kind,
      server: form.server.trim(),
      source: form.source.trim(),
      username: form.username.trim() || undefined,
      domain: form.domain.trim() || undefined,
      options: [],
      read_only: form.read_only,
      enabled: form.enabled,
      mount_now: form.mount_now
    };
    if (form.password.trim()) payload.password = form.password;
    if (!editingName) payload.name = form.name.trim();

    const res = editingName
      ? await api.updateRemoteMount(editingName, payload)
      : await api.createRemoteMount(payload);
    saving = false;
    if (res.success) {
      const view = res.data as RemoteMountView | undefined;
      dialogOpen = false;
      notice = { kind: 'ok', text: $t('remote_mounts.saved') };
      await load();
      if (view?.mounted) onmounted?.(view.target);
    } else {
      // The server names the rule it refused on ("Use lowercase letters…");
      // surfacing it verbatim is what makes the refusal actionable.
      dialogError = res.message || res.error || $t('remote_mounts.save_failed');
    }
  }

  async function toggle(mount: RemoteMountView) {
    busy = mount.name;
    notice = null;
    const res = mount.mounted
      ? await api.unmountRemoteMount(mount.name)
      : await api.mountRemoteMount(mount.name);
    if (res.success) {
      notice = {
        kind: 'ok',
        text: mount.mounted
          ? $t('remote_mounts.disconnected', { name: mount.name })
          : $t('remote_mounts.connected', { name: mount.name })
      };
      // Leave the folder before it disappears, so the Explorer does not sit
      // on a path that no longer exists.
      if (mount.mounted) onunmounted?.(mount.target);
      else onmounted?.(mount.target);
    } else {
      notice = {
        kind: 'error',
        text: res.message || res.error || $t('remote_mounts.action_failed')
      };
    }
    await load();
    busy = null;
  }

  async function remove(mount: RemoteMountView) {
    // Removing a location is easy to redo, so it asks once through the app's
    // own dialog and names what disappears — never `window.confirm`.
    if (askConfirm) {
      const confirmed = await askConfirm(
        $t('remote_mounts.remove_confirm', { name: mount.name })
      );
      if (!confirmed) return;
    }
    busy = mount.name;
    notice = null;
    const res = await api.deleteRemoteMount(mount.name);
    if (res.success) {
      if (mount.mounted) onunmounted?.(mount.target);
      notice = { kind: 'ok', text: $t('remote_mounts.removed', { name: mount.name }) };
    } else {
      notice = {
        kind: 'error',
        text: res.message || res.error || $t('remote_mounts.action_failed')
      };
    }
    await load();
    busy = null;
  }

  async function reconnectAll() {
    busy = '*';
    notice = null;
    const res = await api.reconcileRemoteMounts();
    if (res.success && res.data) {
      const failed = res.data.failed ?? [];
      notice = failed.length
        ? { kind: 'error', text: $t('remote_mounts.reconnect_partial', { count: String(failed.length) }) }
        : { kind: 'ok', text: $t('remote_mounts.reconnected') };
      mounts = res.data.mounts ?? mounts;
    } else {
      notice = {
        kind: 'error',
        text: res.message || res.error || $t('remote_mounts.action_failed')
      };
    }
    busy = null;
  }
</script>

{#if isAdmin}
  <div class="net-panel" data-testid="remote-mount-panel">
    <div class="net-head">
      <span class="net-title">{$t('remote_mounts.title')}</span>
      <div class="flex items-center gap-1">
        <button
          class="hv-ic"
          type="button"
          onclick={load}
          aria-label={$t('common.refresh')}
          data-testid="remote-mount-refresh"
        >
          <IconPlugConnected size={14} />
        </button>
        {#if available}
          <button
            class="hv-ic"
            type="button"
            onclick={openAdd}
            aria-label={$t('remote_mounts.add')}
            data-testid="remote-mount-add"
          >
            <IconPlus size={14} />
          </button>
        {/if}
      </div>
    </div>

    {#if notice}
      <div
        class={notice.kind === 'ok' ? 'net-notice net-notice-ok' : 'net-notice net-notice-error'}
        role={notice.kind === 'ok' ? 'status' : 'alert'}
        data-testid="remote-mount-notice"
      >
        {notice.text}
      </div>
    {/if}

    {#if loading}
      <p class="net-hint">{$t('common.loading')}</p>
    {:else if !available}
      <!-- `503` semantics made visible: name the missing helper, offer nothing. -->
      <p class="net-hint" data-testid="remote-mount-unavailable">
        {$t('remote_mounts.unavailable')}
      </p>
    {:else if mounts.length === 0}
      <p class="net-hint" data-testid="remote-mount-none">{$t('remote_mounts.none')}</p>
    {:else}
      <ul class="net-list" data-testid="remote-mount-list">
        {#each mounts as mount (mount.name)}
          <li class="net-item">
            <div class="net-item-top">
              <span class="net-name" title={mount.target}>
                {#if mount.kind === 'nfs'}<IconNetwork size={14} />{:else}<IconCloud size={14} />{/if}
                {mount.name}
              </span>
              <span
                class={mount.mounted ? 'net-state net-state-on' : 'net-state'}
                data-testid={`remote-mount-state-${mount.name}`}
              >
                {mount.mounted ? $t('remote_mounts.connected_short') : $t('remote_mounts.disconnected_short')}
              </span>
            </div>
            <span class="net-source">{mount.server}:{mount.source}{mount.read_only ? ' · ro' : ' · rw'}</span>
            {#if mount.last_error}
              <span class="net-error" data-testid={`remote-mount-error-${mount.name}`}>{mount.last_error}</span>
            {/if}
            <div class="net-actions">
              <button
                class="hv-ic"
                type="button"
                onclick={() => toggle(mount)}
                disabled={busy === mount.name}
                data-testid={`remote-mount-toggle-${mount.name}`}
                aria-label={mount.mounted
                  ? $t('remote_mounts.disconnect_named', { name: mount.name })
                  : $t('remote_mounts.connect_named', { name: mount.name })}
              >
                {#if mount.mounted}<IconPlugOff size={14} />{:else}<IconPlugConnected size={14} />{/if}
              </button>
              <button
                class="hv-ic"
                type="button"
                onclick={() => openEdit(mount)}
                aria-label={$t('remote_mounts.edit_named', { name: mount.name })}
                data-testid={`remote-mount-edit-${mount.name}`}
              >
                {$t('common.edit')}
              </button>
              <button
                class="hv-ic is-danger"
                type="button"
                onclick={() => remove(mount)}
                disabled={busy === mount.name}
                aria-label={$t('remote_mounts.remove_named', { name: mount.name })}
                data-testid={`remote-mount-remove-${mount.name}`}
              >
                <IconTrash size={14} />
              </button>
            </div>
          </li>
        {/each}
      </ul>
      <button class="net-link" type="button" onclick={reconnectAll} disabled={busy === '*'}>
        {$t('remote_mounts.reconnect_all')}
      </button>
    {/if}
  </div>
{/if}

<Modal
  open={dialogOpen}
  title={editingName ? $t('remote_mounts.edit_title') : $t('remote_mounts.add_title')}
  size="md"
  onclose={() => (dialogOpen = false)}
>
  {#snippet children()}
    <div class="grid gap-3">
      {#if !editingName}
        <label class="net-field">
          <span>{$t('remote_mounts.name')}</span>
          <input
            type="text"
            bind:value={form.name}
            placeholder="photos-nas"
            maxlength="32"
            data-testid="remote-mount-name"
          />
          <small>{$t('remote_mounts.name_hint')}</small>
        </label>
      {/if}

      <label class="net-field">
        <span>{$t('remote_mounts.protocol')}</span>
        <select bind:value={form.kind} data-testid="remote-mount-kind">
          {#if kinds.includes('cifs') || kinds.length === 0}
            <option value="cifs">{$t('remote_mounts.protocol_cifs')}</option>
          {/if}
          {#if kinds.includes('nfs') || kinds.length === 0}
            <option value="nfs">{$t('remote_mounts.protocol_nfs')}</option>
          {/if}
        </select>
      </label>

      <label class="net-field">
        <span>{$t('remote_mounts.server')}</span>
        <input
          type="text"
          bind:value={form.server}
          placeholder="nas.example.com"
          data-testid="remote-mount-server"
        />
        <small>{$t('remote_mounts.server_hint')}</small>
      </label>

      <label class="net-field">
        <span>{form.kind === 'nfs' ? $t('remote_mounts.export') : $t('remote_mounts.share')}</span>
        <input
          type="text"
          bind:value={form.source}
          placeholder={form.kind === 'nfs' ? '/volume1/media' : 'photos'}
          data-testid="remote-mount-source"
        />
      </label>

      {#if form.kind === 'cifs'}
        <label class="net-field">
          <span>{$t('remote_mounts.username')}</span>
          <input type="text" bind:value={form.username} autocomplete="username" data-testid="remote-mount-username" />
        </label>
        <label class="net-field">
          <span>{$t('remote_mounts.password')}</span>
          <input
            type="password"
            bind:value={form.password}
            autocomplete="new-password"
            placeholder={editingName ? $t('remote_mounts.password_keep') : ''}
            data-testid="remote-mount-password"
          />
        </label>
        <label class="net-field">
          <span>{$t('remote_mounts.domain')}</span>
          <input type="text" bind:value={form.domain} placeholder="WORKGROUP" data-testid="remote-mount-domain" />
        </label>
      {/if}

      <label class="net-check">
        <input type="checkbox" bind:checked={form.read_only} data-testid="remote-mount-readonly" />
        <span>{$t('remote_mounts.read_only')}</span>
      </label>
      <label class="net-check">
        <input type="checkbox" bind:checked={form.enabled} data-testid="remote-mount-enabled" />
        <span>{$t('remote_mounts.enabled')}</span>
      </label>
      <label class="net-check">
        <input type="checkbox" bind:checked={form.mount_now} data-testid="remote-mount-now" />
        <span>{$t('remote_mounts.mount_now')}</span>
      </label>

      {#if dialogError}
        <p class="net-error" role="alert" data-testid="remote-mount-dialog-error">{dialogError}</p>
      {/if}
    </div>
  {/snippet}
  {#snippet footer()}
    <div class="flex gap-2.5 justify-end">
      <Button variant="ghost" onclick={() => (dialogOpen = false)} disabled={saving}>
        {$t('common.cancel')}
      </Button>
      <Button variant="primary" onclick={save} disabled={saving} data-testid="remote-mount-save">
        {saving ? $t('common.saving') : $t('common.save')}
      </Button>
    </div>
  {/snippet}
</Modal>

<style>
  .net-panel {
    display: flex;
    flex-direction: column;
    gap: 8px;
    padding: 8px;
  }

  .net-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
  }

  .net-title {
    font-size: 0.75rem;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    color: var(--hv-shell-text-3);
  }

  .net-hint {
    margin: 0;
    font-size: 0.75rem;
    color: var(--hv-shell-text-2);
  }

  .net-notice {
    border: 1px solid var(--hv-shell-border);
    border-radius: 6px;
    padding: 6px 8px;
    font-size: 0.75rem;
    line-height: 1rem;
  }

  .net-notice-error {
    background: var(--hv-error-bg);
    border-color: var(--hv-error);
    color: var(--hv-error);
  }

  .net-notice-ok {
    background: var(--hv-success-bg);
    border-color: var(--hv-success);
    color: var(--hv-success);
  }

  .net-list {
    list-style: none;
    margin: 0;
    padding: 0;
    display: grid;
    gap: 6px;
  }

  .net-item {
    display: flex;
    flex-direction: column;
    gap: 3px;
    border: 1px solid var(--hv-shell-border);
    border-radius: 6px;
    padding: 6px 8px;
  }

  .net-item-top {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    gap: 8px;
  }

  .net-name {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    font-size: 0.8125rem;
    font-weight: 600;
    color: var(--hv-shell-text);
    overflow-wrap: anywhere;
  }

  .net-state {
    font-size: 0.6875rem;
    font-weight: 700;
    color: var(--hv-shell-text-3);
    flex: none;
  }

  .net-state-on {
    color: var(--hv-success);
  }

  .net-source {
    font-size: 0.6875rem;
    color: var(--hv-shell-text-2);
    font-family: var(--hv-font-mono);
    overflow-wrap: anywhere;
  }

  .net-error {
    margin: 0;
    font-size: 0.6875rem;
    color: var(--hv-error);
    overflow-wrap: anywhere;
  }

  .net-actions {
    display: flex;
    gap: 4px;
    margin-top: 2px;
  }

  .net-link {
    align-self: flex-start;
    border: none;
    background: none;
    padding: 4px 2px;
    font-size: 0.75rem;
    color: var(--hv-primary);
    cursor: pointer;
    text-decoration: underline;
  }

  .net-link:disabled {
    opacity: 0.6;
    cursor: default;
  }

  .net-field {
    display: flex;
    flex-direction: column;
    gap: 4px;
    font-size: 0.8125rem;
    color: var(--hv-shell-text);
  }

  .net-field small {
    font-size: 0.6875rem;
    color: var(--hv-shell-text-2);
  }

  .net-field input,
  .net-field select {
    width: 100%;
    min-height: 44px;
    padding: 0 12px;
    border: 1px solid var(--hv-shell-border-strong);
    border-radius: 8px;
    background: var(--hv-shell-border-soft);
    color: var(--hv-shell-text);
    font-size: 0.875rem;
  }

  .net-field input:focus-visible,
  .net-field select:focus-visible {
    outline: none;
    box-shadow: var(--hv-focus-ring);
  }

  .net-check {
    display: flex;
    align-items: center;
    gap: 8px;
    min-height: 32px;
    font-size: 0.8125rem;
    color: var(--hv-shell-text);
  }
</style>

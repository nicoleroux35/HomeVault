<!-- ========================================================================
     File          : FileRequestModal.svelte
     Author        : Nicolas Le Roux
     Created       : 08/11/2026 by Nicolas Le Roux
     Version       : 1.0.0

     Description:
       Explorer "request files" modal: mint a public upload link for a folder
       (optional password, expiry and upload cap), copy it, and revoke links
       already minted for that folder.

     Dependencies:
       - svelte (>= 5), theme/Modal, theme/Button, theme/Badge,
         $lib/services/api

     Usage:
       Opened from a folder row in the Explorer.
     ======================================================================== -->
<script lang="ts">
  import { Modal, Button, Badge } from '$lib/components/theme';
  import { api } from '$lib/services/api';
  import { t } from '$lib/stores/i18n';
  import {
    IconCheck,
    IconCopy,
    IconInbox,
    IconLink,
    IconTrash,
  } from '@tabler/icons-svelte';

  interface FileRequest {
    id: number;
    token: string;
    username: string;
    path: string;
    message: string;
    expires_at: string | null;
    created_at: string;
    request_path: string;
    protected: boolean;
    max_uploads: number | null;
    upload_count: number;
    exhausted: boolean;
    expired: boolean;
  }

  let {
    open = false,
    onclose,
    initialPath = '',
  }: {
    open?: boolean;
    onclose?: () => void;
    initialPath?: string;
  } = $props();

  const requests = $state<FileRequest[]>([]);
  let message = $state('');
  let password = $state('');
  let expiresDays = $state<number>(30);
  let limitMode = $state<'none' | 'custom'>('none');
  let maxUploads = $state<number>(10);
  let creating = $state(false);
  let error = $state('');
  let copiedToken = $state('');
  let loaded = false;

  $effect(() => {
    if (open && !loaded) {
      loaded = true;
      refresh();
    }
    if (!open) {
      loaded = false;
      error = '';
    }
  });

  const selectClass =
    'w-full h-11 px-3 border border-[var(--hv-shell-border-strong)] rounded-lg bg-[var(--hv-shell-border-soft)] text-sm outline-none focus:border-[var(--hv-primary)]';

  async function refresh() {
    error = '';
    const r = await api.listFileRequests();
    if (r.success && Array.isArray(r.data)) {
      requests.length = 0;
      requests.push(...(r.data as FileRequest[]));
    } else {
      error = r.message || r.error || $t('explorer.file_request_load_failed');
    }
  }

  async function createRequest() {
    if (!initialPath || creating) return;
    creating = true;
    error = '';
    const r = await api.createFileRequest(
      initialPath,
      message.trim(),
      expiresDays,
      password || undefined,
      limitMode === 'none' ? undefined : maxUploads,
    );
    creating = false;
    if (r.success && r.data) {
      requests.unshift(r.data as FileRequest);
      message = '';
      password = '';
      expiresDays = 30;
      limitMode = 'none';
      maxUploads = 10;
    } else {
      error = r.message || r.error || $t('explorer.file_request_create_failed');
    }
  }

  async function revoke(token: string) {
    const r = await api.revokeFileRequest(token);
    if (r.success) {
      const idx = requests.findIndex((item) => item.token === token);
      if (idx >= 0) requests.splice(idx, 1);
    } else {
      error = r.message || r.error || $t('explorer.file_request_revoke_failed');
    }
  }

  async function copyLink(token: string, requestPath: string) {
    try {
      await navigator.clipboard.writeText(api.getFileRequestUrl(requestPath));
      copiedToken = token;
      setTimeout(() => (copiedToken = ''), 1600);
    } catch {
      /* clipboard unavailable — the URL is still visible on screen */
    }
  }

  const activeRequest = $derived(
    requests.find((item) => item.path === initialPath && !item.expired && !item.exhausted),
  );
</script>

<Modal open={open} title={$t('explorer.file_request_title')} size="lg" onclose={onclose}>
  {#if error}
    <div class="mb-3 px-3 py-2 rounded-lg bg-[var(--hv-error-bg)] text-[var(--hv-error)] text-sm">
      {error}
    </div>
  {/if}

  <p class="m-0 mb-3 text-sm text-[var(--hv-shell-text-2)] break-all">{initialPath}</p>

  {#if activeRequest}
    <div data-testid="file-request-active" class="rounded-xl border border-[var(--hv-shell-border)] bg-[var(--hv-shell-fill)] p-4">
      <div class="flex items-center justify-between gap-2 flex-wrap">
        <span class="flex items-center gap-1.5 text-xs font-bold uppercase tracking-wide text-[var(--hv-shell-text-2)]">
          <IconLink size={14} />
          {$t('explorer.file_request_active')}
          {#if activeRequest.protected}<Badge>{$t('explorer.share_link_protected')}</Badge>{/if}
          {#if activeRequest.max_uploads !== null}
            <Badge>{activeRequest.upload_count}/{activeRequest.max_uploads}</Badge>
          {/if}
        </span>
      </div>
      <p class="m-0 mt-3 text-sm font-mono text-[var(--hv-shell-text-strong)] break-all select-all">{api.getFileRequestUrl(activeRequest.request_path)}</p>
      <div class="mt-2 flex gap-2 flex-wrap">
        <Button size="sm" variant="secondary" onclick={() => copyLink(activeRequest.token, activeRequest.request_path)}>
          {#if copiedToken === activeRequest.token}
            <IconCheck size={14} /> {$t('explorer.share_link_copied')}
          {:else}
            <IconCopy size={14} /> {$t('common.copy')}
          {/if}
        </Button>
        <Button size="sm" variant="danger" onclick={() => revoke(activeRequest.token)}>
          <IconTrash size={14} /> {$t('explorer.share_link_revoke')}
        </Button>
      </div>
    </div>
  {:else}
    <div class="grid gap-3 @min-[420px]:grid-cols-2">
      <div>
        <label class="block text-c-label font-semibold text-[var(--hv-shell-text-strong)] mb-1" for="request-expiry">
          {$t('explorer.share_link_expiry')}
        </label>
        <select id="request-expiry" class={selectClass} bind:value={expiresDays}>
          <option value="1">{$t('explorer.share_link_expiry_1d')}</option>
          <option value="7">{$t('explorer.share_link_expiry_7d')}</option>
          <option value="30">{$t('explorer.share_link_expiry_30d')}</option>
          <option value="0">{$t('explorer.share_link_expiry_never')}</option>
        </select>
      </div>
      <div>
        <label class="block text-c-label font-semibold text-[var(--hv-shell-text-strong)] mb-1" for="request-limit">
          {$t('explorer.file_request_limit')}
        </label>
        <select id="request-limit" class={selectClass} bind:value={limitMode}>
          <option value="none">{$t('explorer.share_link_limit_none')}</option>
          <option value="custom">{$t('explorer.share_link_limit_custom')}</option>
        </select>
      </div>
    </div>
    {#if limitMode === 'custom'}
      <label class="block mt-3 text-c-label font-semibold text-[var(--hv-shell-text-strong)]" for="request-max">
        {$t('explorer.file_request_max')}
        <input id="request-max" class="mt-1 w-full h-11 px-3 border border-[var(--hv-shell-border-strong)] rounded-lg bg-[var(--hv-shell-card)] text-[var(--hv-shell-text-strong)]" type="number" min="1" max="100000" bind:value={maxUploads} />
      </label>
    {/if}
    <label class="block mt-3 text-c-label font-semibold text-[var(--hv-shell-text-strong)]" for="request-message">
      {$t('explorer.file_request_message')}
      <input id="request-message" class="mt-1 w-full h-11 px-3 border border-[var(--hv-shell-border-strong)] rounded-lg bg-[var(--hv-shell-card)] text-[var(--hv-shell-text-strong)]" type="text" maxlength="500" bind:value={message} />
    </label>
    <label class="block mt-3 text-c-label font-semibold text-[var(--hv-shell-text-strong)]" for="request-password">
      {$t('explorer.share_link_password')}
      <input id="request-password" class="mt-1 w-full h-11 px-3 border border-[var(--hv-shell-border-strong)] rounded-lg bg-[var(--hv-shell-card)] text-[var(--hv-shell-text-strong)]" type="password" autocomplete="new-password" minlength="8" maxlength="256" bind:value={password} />
    </label>
    <div class="mt-4 flex justify-end">
      <Button variant="primary" disabled={creating} onclick={createRequest}>
        <IconInbox size={16} /> {$t('explorer.file_request_create')}
      </Button>
    </div>
  {/if}

  {#if requests.length > 0}
    <h3 class="mt-6 mb-2 text-xs font-bold uppercase tracking-wide text-[var(--hv-shell-text-2)]">
      {$t('explorer.file_request_list')}
    </h3>
    <ul class="m-0 p-0 list-none max-h-56 overflow-y-auto flex flex-col gap-2" data-testid="file-request-list">
      {#each requests as request (request.id)}
        <li class="flex items-center gap-2 px-3 py-2 rounded-lg border border-[var(--hv-shell-border)] bg-[var(--hv-shell-border-soft)]">
          <span class="flex-1 min-w-0">
            <span class="block text-c-sm text-[var(--hv-shell-text-strong)] truncate">{request.path.split('/').filter(Boolean).pop() || request.path}</span>
            <span class="block text-micro text-[var(--hv-shell-text-2)]">
              {request.expired
                ? $t('explorer.share_link_expired')
                : request.exhausted
                  ? $t('explorer.share_link_exhausted')
                  : request.max_uploads !== null
                    ? `${request.upload_count}/${request.max_uploads}`
                    : request.expires_at
                      ? new Date(request.expires_at).toLocaleDateString()
                      : $t('explorer.share_link_no_expiry')}
            </span>
          </span>
          <button
            class="hv-ic grid place-items-center w-8 h-8 rounded-lg border-none bg-transparent text-[var(--hv-shell-text-2)] cursor-pointer hover:text-[var(--hv-shell-text-strong)]"
            title={$t('common.copy')}
            aria-label={$t('common.copy')}
            onclick={() => copyLink(request.token, request.request_path)}
          >
            {#if copiedToken === request.token}<IconCheck size={15} />{:else}<IconCopy size={15} />{/if}
          </button>
          <button
            class="hv-ic grid place-items-center w-8 h-8 rounded-lg border-none bg-transparent text-[var(--hv-shell-text-2)] cursor-pointer hover:text-[var(--hv-error)]"
            title={$t('explorer.share_link_revoke')}
            aria-label={$t('explorer.share_link_revoke')}
            onclick={() => revoke(request.token)}
          >
            <IconTrash size={15} />
          </button>
        </li>
      {/each}
    </ul>
  {/if}

  {#snippet footer()}
    <div class="flex gap-2.5 justify-end">
      <Button variant="ghost" onclick={onclose}>{$t('common.close')}</Button>
    </div>
  {/snippet}
</Modal>

<!-- ========================================================================
     File          : ShareLinkModal.svelte
     Author        : Nicolas Le Roux
     Created       : 28/09/2026
     Last modified : 28/09/2026 by Nicolas Le Roux
     Version       : 1.0.0

     Description:
       Explorer "share by link" modal: mint a revocable download link to a
       file (anyone with the link or restricted to signed-in sessions),
       display it as copyable text and QR code, and revoke existing links.

     Dependencies:
       - svelte (>= 5), theme/Modal, $lib/services/api

     Usage:
       Opened from the Explorer's file actions.
     ======================================================================== -->
<script lang="ts">
  /**
   * Explorer "share by link" modal.
   *
   * Lets the user mint a revocable download link to a file (anyone with the
   * link, or restricted to signed-in HomeVault sessions), see it as a QR
   * code, and manage every link they've created. Mirrors the share dialog
   * of the arozos reference OS (link + QR + permission mode + revoke),
   * scoped to HomeVault's privacy posture: links default to a 30-day
   * expiry and revoking cuts access immediately.
   */
  import QRCode from 'qrcode';
  import { Modal, Button, Badge } from '$lib/components/theme';
  import { api } from '$lib/services/api';
  import { t } from '$lib/stores/i18n';
  import { cn } from '$lib/utils.js';
  import {
    IconCopy,
    IconCheck,
    IconTrash,
    IconLink,
    IconLock,
    IconWorld,
    IconClock,
  } from '@tabler/icons-svelte';

  interface ShareLink {
    id: number;
    token: string;
    username: string;
    path: string;
    mode: 'anyone' | 'signed_in';
    expires_at: string | null;
    created_at: string;
    share_path: string;
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

const links = $state<ShareLink[]>([]);
  let mode = $state<'anyone' | 'signed_in'>('anyone');
  let expiresDays = $state<number>(30);
  let creating = $state(false);
  let error = $state('');
  let copiedToken = $state('');
  let qrUrl = $state('');
  let qrDataUrl = $state('');
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
    const r = await api.listShareLinks();
    if (r.success && Array.isArray(r.data)) {
      links.length = 0;
      links.push(...(r.data as ShareLink[]));
      const first = links.find((l) => !l.expired);
      if (first) showQr(first);
    } else {
      error = r.message || r.error || $t('explorer.share_link_load_failed');
    }
  }

  async function createLink() {
    if (!initialPath || creating) return;
    creating = true;
    error = '';
    const r = await api.createShareLink(initialPath, mode, expiresDays);
    creating = false;
    if (r.success && r.data) {
      const created = r.data as ShareLink;
      links.unshift(created);
      showQr(created);
      mode = 'anyone';
      expiresDays = 30;
    } else {
      error = r.message || r.error || $t('explorer.share_link_create_failed');
    }
  }

  async function revoke(token: string) {
    const r = await api.revokeShareLink(token);
    if (r.success) {
      const idx = links.findIndex((l) => l.token === token);
      if (idx >= 0) links.splice(idx, 1);
      if (qrUrl && qrUrl.includes(token)) {
        qrUrl = '';
        qrDataUrl = '';
      }
    } else {
      error = r.message || r.error || $t('explorer.share_link_revoke_failed');
    }
  }

  async function copyLink(token: string, sharePath: string) {
    const url = api.getShareLinkUrl(sharePath);
    try {
      await navigator.clipboard.writeText(url);
      copiedToken = token;
      setTimeout(() => (copiedToken = ''), 1600);
    } catch {
      /* clipboard unavailable — the URL is still visible on screen */
    }
  }

  async function showQr(link: ShareLink) {
    qrUrl = link.share_path;
    const url = api.getShareLinkUrl(link.share_path);
    try {
      qrDataUrl = await QRCode.toDataURL(url, { width: 220, margin: 1 });
    } catch {
      qrDataUrl = '';
    }
  }

  const active = $derived(links.find((l) => l.share_path === qrUrl));
  const fileLink = $derived(links.find((l) => !l.expired && l.path === initialPath));
  const visibleLinks = $derived(links);
</script>

<Modal open={open} title={$t('explorer.share_link_title')} size="lg" onclose={onclose}>
  {#if error}
    <div class="mb-3 px-3 py-2 rounded-lg bg-[var(--hv-error-bg)] text-[var(--hv-error)] text-sm">
      {error}
    </div>
  {/if}

  {#if initialPath && !fileLink}
    <p class="m-0 mb-3 text-sm text-[var(--hv-shell-text-2)] break-all">{initialPath}</p>
    <div class="grid gap-3 @min-[420px]:grid-cols-2">
      <div>
        <label class="block text-c-label font-semibold text-[var(--hv-shell-text-strong)] mb-1" for="share-mode">
          {$t('explorer.share_link_mode')}
        </label>
        <select id="share-mode" class={selectClass} bind:value={mode}>
          <option value="anyone">{$t('explorer.share_link_mode_anyone')}</option>
          <option value="signed_in">{$t('explorer.share_link_mode_signed_in')}</option>
        </select>
      </div>
      <div>
        <label class="block text-c-label font-semibold text-[var(--hv-shell-text-strong)] mb-1" for="share-expiry">
          {$t('explorer.share_link_expiry')}
        </label>
        <select id="share-expiry" class={selectClass} bind:value={expiresDays}>
          <option value="1">{$t('explorer.share_link_expiry_1d')}</option>
          <option value="7">{$t('explorer.share_link_expiry_7d')}</option>
          <option value="30">{$t('explorer.share_link_expiry_30d')}</option>
          <option value="0">{$t('explorer.share_link_expiry_never')}</option>
        </select>
      </div>
    </div>
    {#if mode === 'anyone'}
      <p class="mt-2 text-xs text-[var(--hv-shell-text-2)]">{$t('explorer.share_link_anyone_note')}</p>
    {:else}
      <p class="mt-2 text-xs text-[var(--hv-shell-text-2)]">{$t('explorer.share_link_signed_in_note')}</p>
    {/if}
    <div class="mt-4 flex justify-end">
      <Button variant="primary" disabled={creating} onclick={createLink}>
        {$t('explorer.share_link_create')}
      </Button>
    </div>
  {/if}

  {#if fileLink}
    <div data-testid="share-link-active" class="rounded-xl border border-[var(--hv-shell-border)] bg-[var(--hv-shell-fill)] p-4">
      <div class="flex items-center justify-between gap-2 flex-wrap">
        <span class="flex items-center gap-1.5 text-xs font-bold uppercase tracking-wide text-[var(--hv-shell-text-2)]">
          <IconLink size={14} />
          {fileLink.mode === 'anyone' ? $t('explorer.share_link_mode_anyone') : $t('explorer.share_link_mode_signed_in')}
          {#if fileLink.expired}
            <Badge variant="warning">{$t('explorer.share_link_expired')}</Badge>
          {/if}
        </span>
        <span class="flex items-center gap-1 text-xs text-[var(--hv-shell-text-2)]">
          <IconClock size={13} />
          {fileLink.expires_at ? new Date(fileLink.expires_at).toLocaleString() : $t('explorer.share_link_no_expiry')}
        </span>
      </div>
      <div class="mt-3 flex items-center gap-3 flex-wrap">
        {#if qrDataUrl && qrUrl === fileLink.share_path}
          <img data-testid="share-link-qr" src={qrDataUrl} alt="QR" class="w-[120px] h-[120px] rounded-lg bg-white p-1.5" />
        {/if}
        <div class="flex-1 min-w-0">
          <p class="m-0 text-xs text-[var(--hv-shell-text-2)] mb-1">{$t('explorer.share_link_url')}</p>
          <p class="m-0 text-sm font-mono text-[var(--hv-shell-text-strong)] break-all select-all">{api.getShareLinkUrl(fileLink.share_path)}</p>
          <div class="mt-2 flex gap-2 flex-wrap">
            <Button size="sm" variant="secondary" disabled={fileLink.expired} onclick={() => copyLink(fileLink.token, fileLink.share_path)}>
              {#if copiedToken === fileLink.token}
                <IconCheck size={14} /> {$t('explorer.share_link_copied')}
              {:else}
                <IconCopy size={14} /> {$t('common.copy')}
              {/if}
            </Button>
            <Button size="sm" variant="danger" onclick={() => revoke(fileLink.token)}>
              <IconTrash size={14} /> {$t('explorer.share_link_revoke')}
            </Button>
          </div>
        </div>
      </div>
      <p class="m-0 mt-3 text-xs text-[var(--hv-shell-text-2)]">
        {fileLink.mode === 'anyone' ? $t('explorer.share_link_anyone_note') : $t('explorer.share_link_signed_in_note')}
      </p>
    </div>
  {/if}

  {#if visibleLinks.length > 0}
    <h3 class="mt-6 mb-2 text-xs font-bold uppercase tracking-wide text-[var(--hv-shell-text-2)]">
      {$t('explorer.share_link_list')}
    </h3>
    <ul class="m-0 p-0 list-none max-h-56 overflow-y-auto flex flex-col gap-2" data-testid="share-link-list">
      {#each visibleLinks as link (link.id)}
        <li class:opacity-50={link.expired} class="flex items-center gap-2 px-3 py-2 rounded-lg border border-[var(--hv-shell-border)] bg-[var(--hv-shell-border-soft)]">
          <span class={cn('shrink-0', link.mode === 'anyone' ? 'text-[var(--hv-primary)]' : 'text-[var(--hv-warning)]')}>
            {#if link.mode === 'anyone'}<IconWorld size={15} />{:else}<IconLock size={15} />{/if}
          </span>
          <span class="flex-1 min-w-0">
            <span class="block text-c-sm text-[var(--hv-shell-text-strong)] truncate">{link.path.split('/').filter(Boolean).pop() || link.path}</span>
            <span class="block text-micro text-[var(--hv-shell-text-2)]">
              {link.expired
                ? $t('explorer.share_link_expired')
                : link.expires_at
                  ? new Date(link.expires_at).toLocaleDateString()
                  : $t('explorer.share_link_no_expiry')}
            </span>
          </span>
          <button
            class="hv-ic grid place-items-center w-8 h-8 rounded-lg border-none bg-transparent text-[var(--hv-shell-text-2)] cursor-pointer hover:text-[var(--hv-shell-text-strong)] disabled:opacity-40"
            title={$t('common.copy')}
            aria-label={$t('common.copy')}
            disabled={link.expired}
            onclick={() => copyLink(link.token, link.share_path)}
          >
            {#if copiedToken === link.token}<IconCheck size={15} />{:else}<IconCopy size={15} />{/if}
          </button>
          <button
            class="hv-ic grid place-items-center w-8 h-8 rounded-lg border-none bg-transparent text-[var(--hv-shell-text-2)] cursor-pointer hover:text-[var(--hv-error)]"
            title={$t('explorer.share_link_revoke')}
            aria-label={$t('explorer.share_link_revoke')}
            onclick={() => revoke(link.token)}
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
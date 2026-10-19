<!-- ========================================================================
     File          : ShortcutAddDialog.svelte
     Author        : Nicolas Le Roux
     Created       : 13/10/2026
     Last modified : 13/10/2026 by Nicolas Le Roux
     Version       : 1.0.0

     Description:
       The "add a web shortcut" dialog. Asks for a URL whose metadata
       (title/description/favicon) the server fetches SSRF-guarded; the
       result is stored per-account by shortcutsStore.

     Dependencies:
       - svelte (>= 5), theme/Modal, stores/shortcuts, $lib/services/api

     Usage:
       Mounted by the desktop shell; opened from the launcher's last tile.
     ======================================================================== -->
<script lang="ts">
  /**
   * Add-a-shortcut dialog.
   *
   * The trigger lives in the launcher (`HvLaunchpad`, last tile of the grid)
   * rather than on the desktop itself, so this dialog is owned by the shell
   * (`Desktop.svelte`) and driven by its `open` prop.
   *
   * The metadata fetch is server-side (`api.fetchShortcutMetadata` →
   * SSRF-guarded handler); this component only collects the URL and reports
   * a failure inline. `theme/Modal` supplies the focus trap and focus
   * restore, so the dialog needs no bespoke accessibility plumbing.
   */
  import { t, translate } from '$lib/stores/i18n';
  import { shortcutsStore } from '$lib/stores/shortcuts';
  import { api } from '$lib/services/api';
  import { Modal } from '$lib/components/theme';
  import { IconPlus, IconWorld, IconLoader2 } from '@tabler/icons-svelte';

  let {
    open = false,
    onclose = () => {},
  }: {
    open?: boolean;
    onclose?: () => void;
  } = $props();

  let url = $state('');
  let busy = $state(false);
  let error = $state('');

  async function addShortcut() {
    const trimmed = url.trim();
    if (!trimmed || busy) return;
    busy = true;
    error = '';
    try {
      const res = await api.fetchShortcutMetadata(trimmed);
      if (!res.success || !res.data) {
        error = res.message || res.error || translate('shortcuts.fetch_failed');
        return;
      }
      const d = res.data as {
        url: string;
        title: string | null;
        description: string | null;
        icon_url: string | null;
        site_name: string | null;
      };
      const ok = shortcutsStore.add({
        url: d.url || trimmed,
        title: d.title || d.site_name || trimmed,
        description: d.description || '',
        iconUrl: d.icon_url,
        siteName: d.site_name || '',
      });
      if (!ok) {
        error = translate('shortcuts.duplicate_or_full');
        return;
      }
      url = '';
      onclose();
    } catch {
      error = translate('shortcuts.fetch_failed');
    } finally {
      busy = false;
    }
  }

  function closeAdd() {
    error = '';
    onclose();
  }
</script>

<Modal open={open} title={$t('shortcuts.add')} size="md" onclose={closeAdd}>
  {#snippet icon()}<IconWorld size={22} stroke={1.75} />{/snippet}
  {#snippet children()}
    <p class="m-0 mb-3 text-sm text-[var(--hv-shell-text)]">{$t('shortcuts.add_hint')}</p>
    <label class="flex flex-col gap-1 text-c-base text-[var(--hv-shell-text-2)]">
      {$t('shortcuts.url_label')}
      <input
        data-testid="shortcuts-url"
        class="h-10 px-3 rounded-md border border-[var(--hv-shell-border)] bg-[var(--hv-shell-card)] text-c-base text-[var(--hv-shell-text-strong)]"
        placeholder="https://example.com"
        bind:value={url}
        onkeydown={(e) => { if (e.key === 'Enter') { e.preventDefault(); addShortcut(); } }}
        autocomplete="off"
      />
    </label>
    {#if error}
      <p data-testid="shortcuts-error" role="alert" class="mt-2 mb-0 text-c-base text-[var(--hv-error-text)]">{error}</p>
    {/if}
  {/snippet}
  {#snippet footer()}
    <button type="button" class="hv-shortcut-btn hv-shortcut-btn-ghost" onclick={closeAdd}>{$t('common.cancel')}</button>
    <button
      type="button"
      data-testid="shortcuts-submit"
      class="hv-shortcut-btn hv-shortcut-btn-primary"
      disabled={busy || !url.trim()}
      onclick={addShortcut}
    >
      {#if busy}<IconLoader2 size={16} stroke={2} class="hv-spin" />{/if}
      {$t('shortcuts.add')}
    </button>
  {/snippet}
</Modal>

<style>
  .hv-shortcut-btn {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    height: 38px;
    padding: 0 16px;
    border: 0;
    border-radius: var(--hv-radius-md);
    font-weight: 600;
    cursor: pointer;
  }
  .hv-shortcut-btn-ghost {
    background: var(--hv-shell-fill);
    color: var(--hv-shell-text-strong);
  }
  .hv-shortcut-btn-primary {
    background: var(--hv-primary);
    color: #fff;
  }
  .hv-shortcut-btn:focus-visible {
    outline: none;
    box-shadow: var(--hv-focus-ring);
  }
  .hv-shortcut-btn:disabled {
    opacity: 0.6;
    cursor: not-allowed;
  }
  :global(.hv-spin) {
    animation: hv-spin 1s linear infinite;
  }
  @keyframes hv-spin {
    to {
      transform: rotate(360deg);
    }
  }
</style>

<!-- ========================================================================
     File          : DesktopShortcuts.svelte
     Author        : Nicolas Le Roux
     Created       : 13/10/2026
     Last modified : 13/10/2026 by Nicolas Le Roux
     Version       : 1.0.0

     Description:
       Pinned web-shortcut tiles under the app grid. Adding one asks for a
       URL whose metadata (title/description/favicon) the server fetches
       SSRF-guarded; the list is stored per-account by shortcutsStore.

     Dependencies:
       - svelte (>= 5), stores/shortcuts, $lib/services/api

     Usage:
       Mounted by the desktop shell.
     ======================================================================== -->
<script lang="ts">
  /**
   * Pinned web shortcuts on the desktop.
   *
   * A compact row of tiles under the app grid. Adding one opens a dialog that
   * asks for a URL; the server fetches the page's title/description/favicon
   * (`api.fetchShortcutMetadata` → SSRF-guarded handler) and the result is
   * stored per-account by `shortcutsStore`.
   *
   * Accessibility: every tile is a real `<button>` (opens the URL in a new
   * tab) and carries a visible-on-hover/focus-within remove button, so the
   * removal path is not hover-only. The remove button has an `aria-label` from
   * an i18n key.
   */
  import { t, translate } from '$lib/stores/i18n';
  import { shortcutsStore, MAX_SHORTCUTS, type Shortcut } from '$lib/stores/shortcuts';
  import { api } from '$lib/services/api';
  import { Modal } from '$lib/components/theme';
  import { IconPlus, IconTrash, IconWorld, IconLoader2 } from '@tabler/icons-svelte';
  import { cn } from '$lib/utils.js';

  let showAdd = false;
  let url = '';
  let busy = false;
  let error = '';

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
      showAdd = false;
    } catch {
      error = translate('shortcuts.fetch_failed');
    } finally {
      busy = false;
    }
  }

  function openShortcut(s: Shortcut) {
    window.open(s.url, '_blank', 'noopener,noreferrer');
  }

  function closeAdd() {
    showAdd = false;
    error = '';
  }

  function initial(s: Shortcut): string {
    return (s.title || s.siteName || s.url).trim().charAt(0).toUpperCase() || '?';
  }
</script>

<div data-testid="desktop-shortcuts" class="hv-shortcuts" aria-label={$t('shortcuts.title')}>
  {#each $shortcutsStore as s (s.id)}
    <div class="hv-shortcut" data-testid="desktop-shortcut">
      <button
        type="button"
        class="hv-shortcut-open"
        title={s.description || s.url}
        aria-label={$t('shortcuts.open', { title: s.title })}
        onclick={() => openShortcut(s)}
      >
        {#if s.iconUrl}
          <img
            class="hv-shortcut-icon"
            src={s.iconUrl}
            alt=""
            loading="lazy"
            referrerpolicy="no-referrer"
            onerror={(e) => ((e.currentTarget as HTMLImageElement).style.display = 'none')}
          />
          <span class="hv-shortcut-fallback" aria-hidden="true">{initial(s)}</span>
        {:else}
          <span class="hv-shortcut-fallback" aria-hidden="true">{initial(s)}</span>
        {/if}
        <span class="hv-shortcut-label">{s.title}</span>
      </button>
      <button
        type="button"
        class="hv-shortcut-remove"
        data-testid="desktop-shortcut-remove"
        aria-label={$t('shortcuts.remove', { title: s.title })}
        onclick={() => shortcutsStore.remove(s.id)}
      >
        <IconTrash size={14} stroke={2} />
      </button>
    </div>
  {/each}

  {#if $shortcutsStore.length < MAX_SHORTCUTS}
    <button
      type="button"
      class="hv-shortcut hv-shortcut-add"
      data-testid="desktop-shortcut-add"
      aria-label={$t('shortcuts.add')}
      onclick={() => (showAdd = true)}
    >
      <span class="hv-shortcut-add-icon"><IconPlus size={20} stroke={2} /></span>
      <span class="hv-shortcut-label">{$t('shortcuts.add_short')}</span>
    </button>
  {/if}
</div>

<Modal open={showAdd} title={$t('shortcuts.add')} size="md" onclose={closeAdd}>
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
  .hv-shortcuts {
    position: absolute;
    top: 120px;
    left: 20px;
    z-index: 1;
    display: flex;
    flex-direction: column;
    gap: 8px;
    max-width: 120px;
  }
  .hv-shortcut {
    position: relative;
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 6px 8px;
    border: 0;
    border-radius: 10px;
    background: transparent;
    color: #fff;
    cursor: pointer;
    text-align: left;
  }
  .hv-shortcut:hover,
  .hv-shortcut:focus-within {
    background: rgba(0, 0, 0, 0.35);
  }
  .hv-shortcut-open {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 4px;
    width: 100%;
    min-height: 44px;
    border: 0;
    background: none;
    color: inherit;
    cursor: pointer;
    font: inherit;
  }
  .hv-shortcut-open:focus-visible {
    outline: none;
    box-shadow: var(--hv-focus-ring);
    border-radius: 8px;
  }
  .hv-shortcut-icon {
    width: 32px;
    height: 32px;
    border-radius: 8px;
    object-fit: cover;
    background: #fff;
  }
  .hv-shortcut-fallback {
    display: grid;
    place-items: center;
    width: 32px;
    height: 32px;
    border-radius: 8px;
    background: var(--hv-primary);
    color: #fff;
    font-weight: 700;
  }
  .hv-shortcut-icon + .hv-shortcut-fallback {
    display: none;
  }
  .hv-shortcut-label {
    max-width: 100px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-size: 11px;
    text-shadow: 0 1px 3px rgba(0, 0, 0, 0.8);
  }
  .hv-shortcut-remove {
    position: absolute;
    top: -4px;
    right: -4px;
    display: none;
    place-items: center;
    width: 22px;
    height: 22px;
    border: 0;
    border-radius: 50%;
    background: var(--hv-error);
    color: #fff;
    cursor: pointer;
  }
  .hv-shortcut:hover .hv-shortcut-remove,
  .hv-shortcut:focus-within .hv-shortcut-remove {
    display: grid;
  }
  .hv-shortcut-remove:focus-visible {
    outline: none;
    box-shadow: var(--hv-focus-ring);
  }
  .hv-shortcut-add {
    flex-direction: column;
    align-items: center;
    gap: 4px;
    color: rgba(255, 255, 255, 0.85);
  }
  .hv-shortcut-add-icon {
    display: grid;
    place-items: center;
    width: 32px;
    height: 32px;
    border-radius: 8px;
    border: 1.5px dashed rgba(255, 255, 255, 0.6);
  }
  .hv-shortcut-btn {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    height: 38px;
    padding: 0 16px;
    border: 0;
    border-radius: 8px;
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

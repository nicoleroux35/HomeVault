<!-- ========================================================================
     File          : DesktopShortcuts.svelte
     Author        : Nicolas Le Roux
     Created       : 13/10/2026
     Last modified : 13/10/2026 by Nicolas Le Roux
     Version       : 1.0.0

     Description:
       Pinned web-shortcut tiles on the desktop. The list is stored
       per-account by shortcutsStore; the add dialog lives in
       ShortcutAddDialog.svelte and is opened from the launcher's last tile.

     Dependencies:
       - svelte (>= 5), stores/shortcuts

     Usage:
       Mounted by the desktop shell.
     ======================================================================== -->
<script lang="ts">
  /**
   * Pinned web shortcuts on the desktop.
   *
   * A compact column of tiles on the desktop. There is deliberately NO "add"
   * tile here: adding a shortcut is a launcher action (`HvLaunchpad`, last
   * tile after every app), so the desktop stays a canvas of the things the
   * user pinned rather than a place to configure them.
   *
   * Accessibility: every tile is a real `<button>` (opens the URL in a new
   * tab) and carries a visible-on-hover/focus-within remove button, so the
   * removal path is not hover-only. The remove button has an `aria-label` from
   * an i18n key.
   */
  import { t } from '$lib/stores/i18n';
  import { shortcutsStore, type Shortcut } from '$lib/stores/shortcuts';
  import { IconTrash } from '@tabler/icons-svelte';

  function openShortcut(s: Shortcut) {
    window.open(s.url, '_blank', 'noopener,noreferrer');
  }

  function initial(s: Shortcut): string {
    return (s.title || s.siteName || s.url).trim().charAt(0).toUpperCase() || '?';
  }
</script>

{#if $shortcutsStore.length > 0}
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
</div>
{/if}

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
    border-radius: var(--hv-radius-lg);
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
    border-radius: var(--hv-radius-md);
  }
  .hv-shortcut-icon {
    width: 32px;
    height: 32px;
    border-radius: var(--hv-radius-md);
    object-fit: cover;
    background: #fff;
  }
  .hv-shortcut-fallback {
    display: grid;
    place-items: center;
    width: 32px;
    height: 32px;
    border-radius: var(--hv-radius-md);
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
</style>

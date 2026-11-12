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
  import { IconTrash } from '$lib/icons/lucide';

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
        <IconTrash size={14} strokeWidth={1.8} />
      </button>
    </div>
  {/each}
</div>
{/if}

<style>
  .hv-shortcuts {
    /* v2 desktop shortcuts: a right-hand column of white squircle tiles with
       the label in ink under each one (the old white-on-wallpaper column only
       worked over a dark photo). */
    position: absolute;
    top: 120px;
    right: 24px;
    left: auto;
    z-index: 1;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 16px;
    max-width: 120px;
  }
  .hv-shortcut {
    position: relative;
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 0;
    border: 0;
    border-radius: 16px;
    background: transparent;
    color: var(--hv-shell-text-strong);
    cursor: pointer;
    text-align: left;
  }
  .hv-shortcut-open {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 6px;
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
    width: 60px;
    height: 60px;
    border-radius: 16px;
    object-fit: cover;
    background: #fff;
    border: 1px solid var(--hv-shell-border);
    box-shadow: var(--hv-shadow-md);
  }
  .hv-shortcut-fallback {
    display: grid;
    place-items: center;
    width: 60px;
    height: 60px;
    border-radius: 16px;
    background: #fff;
    border: 1px solid var(--hv-shell-border);
    box-shadow: var(--hv-shadow-md);
    color: var(--hv-primary);
    font-size: 20px;
    font-weight: 700;
  }
  .hv-shortcut-icon + .hv-shortcut-fallback {
    display: none;
  }
  .hv-shortcut-open:hover .hv-shortcut-icon,
  .hv-shortcut-open:hover .hv-shortcut-fallback {
    box-shadow: var(--hv-shadow-lg);
  }
  .hv-shortcut-label {
    max-width: 104px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-size: var(--hv-text-xs);
    font-weight: var(--hv-font-weight-medium);
    color: var(--hv-shell-text-2);
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

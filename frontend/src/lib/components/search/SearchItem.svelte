<!-- ========================================================================
     File          : SearchItem.svelte
     Author        : Nicolas Le Roux
     Created       : 28/09/2026
     Last modified : 28/09/2026 by Nicolas Le Roux
     Version       : 1.0.0

     Description:
       One row of the search overlay results: icon, type badge, title and
       description, selected/highlighted state, 44px keyboard target.

     Dependencies:
       - svelte (>= 5), theme/Icon, stores/i18n

     Usage:
       Rendered by SearchResults.
     ======================================================================== -->
<script lang="ts">
  import type { SearchableItem } from '$lib/stores/search';
  import Icon from '$lib/components/theme/Icon.svelte';
  import { IconTag } from '$lib/icons/lucide';
  import { t } from '$lib/stores/i18n';

  let {
    item,
    selected = false,
    onTag,
    tagLabel,
    onclick
  }: {
    item: SearchableItem;
    selected?: boolean;
    onclick?: (e: MouseEvent) => void;
    onTag?: () => void;
    tagLabel?: string;
  } = $props();

  function getTypeColor(type: string): string {
    switch (type) {
      case 'app': return 'var(--hv-primary)';
      case 'action': return 'var(--hv-success)';
      case 'setting': return 'var(--hv-warning)';
      default: return 'var(--hv-text-tertiary)';
    }
  }

  /** Accent for this row — an explicit item colour wins, else the type default. */
  const accent = $derived(item.color || getTypeColor(item.type));
  /** 12% tint of the accent, valid whether the accent is a hex or a var(). */
  const accentTint = $derived(`color-mix(in srgb, ${accent} 12%, transparent)`);
</script>

<!-- `role="option"` sits on the row, which is *not* the button: the row hosts
     the open action and (for an active tag filter) a per-row tag action, and a
     control inside a control is invalid. The row keeps the listbox semantics,
     the button keeps the activation. -->
<div
  class="search-item"
  class:selected
  role="option"
  aria-selected={selected}
  tabindex="-1"
  onclick={onclick}
  onkeydown={(event) => { if (event.key === 'Enter' || event.key === ' ') { event.preventDefault(); onclick?.(event as unknown as MouseEvent); } }}
>
  <div class="item-icon" style="background: {accentTint};">
    <Icon name={item.icon} size={24} style="color: {accent};" />
  </div>
  <div class="item-content">
    <div class="item-title">{item.title}</div>
    <div class="item-description">{item.description}</div>
  </div>
  {#if onTag}
    <button type="button" class="item-action-button" aria-label={$t('explorer.add_tag_for_file', { filename: tagLabel || '' })} onclick={(event) => { event.stopPropagation(); onTag(); }}>
      <IconTag size={16} aria-hidden="true" />
    </button>
  {/if}
  <div class="item-type" style="background: {accentTint}; color: {getTypeColor(item.type)};">
    {$t(`search.type_${item.type}`)}
  </div>
</div>

<style>
  .search-item {
    width: 100%;
    display: flex;
    align-items: center;
    gap: var(--hv-space-3);
    padding: var(--hv-space-3) 20px;
    border: none;
    border-left: 3px solid transparent;
    background: transparent;
    transition: background 150ms, border-color 150ms;
  }

  .search-item:hover {
    background: var(--hv-shell-border-soft);
  }

  .search-item.selected {
    background: var(--hv-primary-selected-bg);
    border-left-color: var(--hv-primary);
  }

  .item-icon {
    width: 40px;
    height: 40px;
    border-radius: var(--hv-radius-md);
    display: flex;
    align-items: center;
    justify-content: center;
    flex-shrink: 0;
  }

  .search-item {
    cursor: pointer;
  }

  .item-content {
    min-height: var(--hv-touch-target-min);
    /* Centred so the 44px target is symmetric around the two text lines. */
    display: flex;
    flex-direction: column;
    justify-content: center;
    flex: 1;
    min-width: 0;
  }

  .item-title {
    font-size: var(--hv-font-size-base);
    font-weight: var(--hv-font-weight-semibold);
    color: var(--hv-shell-text-strong);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .item-description {
    font-size: var(--hv-font-size-sm);
    color: var(--hv-shell-text-2);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .item-action-button {
    width: var(--hv-touch-target-min);
    height: var(--hv-touch-target-min);
    flex: 0 0 var(--hv-touch-target-min);
    display: grid;
    place-items: center;
    border: 0;
    border-radius: var(--hv-radius-control);
    background: transparent;
    color: var(--hv-shell-text-2);
    cursor: pointer;
  }

  .search-item:focus-visible,
  .item-action-button:focus-visible {
    outline: none;
    box-shadow: var(--hv-focus-ring);
  }

  .item-type {
    padding: 3px 8px;
    border-radius: var(--hv-radius-sm);
    font-size: var(--hv-font-size-micro);
    font-weight: var(--hv-font-weight-bold);
    text-transform: uppercase;
    letter-spacing: 0.5px;
    flex-shrink: 0;
  }
</style>

<script lang="ts">
  import type { SearchableItem } from '$lib/stores/search';
  import Icon from '$lib/components/theme/Icon.svelte';
  import { t } from '$lib/stores/i18n';

  let {
    item,
    selected = false,
    ...rest
  }: {
    item: SearchableItem;
    selected?: boolean;
    onclick?: (e: MouseEvent) => void;
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

<button class="search-item" class:selected {...rest} role="option" aria-selected={selected}>
  <div class="item-icon" style="background: {accentTint};">
    <Icon name={item.icon} size={24} style="color: {accent};" />
  </div>
  <div class="item-content">
    <div class="item-title">{item.title}</div>
    <div class="item-description">{item.description}</div>
  </div>
  <div class="item-type" style="background: {accentTint}; color: {getTypeColor(item.type)};">
    {$t(`search.type_${item.type}`)}
  </div>
</button>

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
    cursor: pointer;
    transition: background 150ms, border-color 150ms;
    text-align: left;
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

  .item-content {
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

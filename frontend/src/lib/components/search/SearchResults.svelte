<!-- ========================================================================
     File          : SearchResults.svelte
     Author        : Nicolas Le Roux
     Created       : 28/09/2026
     Last modified : 28/09/2026 by Nicolas Le Roux
     Version       : 1.0.0

     Description:
       Result list of the search overlay: renders SearchItem rows for the
       store's current results and a friendly empty state when nothing
       matches.

     Dependencies:
       - svelte (>= 5), stores/search, stores/i18n

     Usage:
       Inside SearchBar.
     ======================================================================== -->
<script lang="ts">
  import { searchStore, closeSearch } from '$lib/stores/search';
  import SearchItem from './SearchItem.svelte';
  import { IconSearchOff } from '@tabler/icons-svelte';
  import { t } from '$lib/stores/i18n';
</script>

{#if $searchStore.results.length > 0}
  <div class="results-list" role="listbox" aria-label={$t('search.title')}>
    {#each $searchStore.results as item, index (item.id)}
      <SearchItem
        {item}
        selected={index === $searchStore.selectedIndex}
        onclick={() => { item.action(); closeSearch(); }}
      />
    {/each}
  </div>
{:else if $searchStore.query}
  <div class="no-results">
    <IconSearchOff size={48} stroke={2} />
    <p>{$t('search.no_results', { query: $searchStore.query })}</p>
  </div>
{:else}
  <div class="empty-state">
    <p class="hint-text">{$t('search.hint')}</p>
    <div class="shortcuts">
      <div class="shortcut-item">
        <kbd>{$t('search.key_up')}</kbd><kbd>{$t('search.key_down')}</kbd>
        <span>{$t('search.navigate')}</span>
      </div>
      <div class="shortcut-item">
        <kbd>{$t('search.key_enter')}</kbd>
        <span>{$t('search.select')}</span>
      </div>
      <div class="shortcut-item">
        <kbd>{$t('search.key_esc')}</kbd>
        <span>{$t('search.close')}</span>
      </div>
    </div>
  </div>
{/if}

<style>
  .results-list {
    max-height: 400px;
    overflow-y: auto;
  }

  .no-results, .empty-state {
    padding: var(--hv-space-10) 20px;
    text-align: center;
  }

  .no-results {
    color: var(--hv-shell-text-2);
  }

  .no-results :global(svg) {
    color: var(--hv-shell-text-3);
    margin-bottom: var(--hv-space-3);
  }

  .hint-text {
    font-size: var(--hv-font-size-base);
    color: var(--hv-shell-text-2);
    margin: 0 0 20px 0;
  }

  .shortcuts {
    display: flex;
    justify-content: center;
    gap: var(--hv-space-5);
  }

  .shortcut-item {
    display: flex;
    align-items: center;
    gap: var(--hv-space-1-5);
    font-size: var(--hv-font-size-sm);
    color: var(--hv-shell-text-3);
  }

  kbd {
    min-width: 24px;
    padding: var(--hv-space-1) 6px;
    background: var(--hv-shell-fill);
    border: 1px solid var(--hv-shell-border);
    border-radius: var(--hv-radius-sm);
    font-size: var(--hv-font-size-xs);
    font-weight: var(--hv-font-weight-semibold);
    color: var(--hv-shell-text-2);
    text-align: center;
    font-family: inherit;
  }
</style>

<!-- ========================================================================
     File          : SearchBar.svelte
     Author        : Nicolas Le Roux
     Created       : 28/09/2026
     Last modified : 28/09/2026 by Nicolas Le Roux
     Version       : 1.0.0

     Description:
       Global search overlay (Cmd/Ctrl-K): input bound to the search store,
       full keyboard navigation (arrows, Enter, Escape) against the scored
       app/setting/action index.

     Dependencies:
       - svelte (>= 5), stores/search, stores/i18n

     Usage:
       Opened by the desktop shell's search hotkey.
     ======================================================================== -->
<script lang="ts">
  import { onDestroy, tick } from 'svelte';
  import { searchStore, closeSearch, updateQuery, selectNext, selectPrev, executeSelected, type SearchableItem } from '$lib/stores/search';
  import SearchResults from './SearchResults.svelte';
  import { IconSearch } from '@tabler/icons-svelte';
  import { t } from '$lib/stores/i18n';

  export let searchIndex: SearchableItem[];

  let inputRef: HTMLInputElement;

  // Focus the field as soon as the overlay has been rendered. `tick()` (not a
  // `setTimeout(…, 50)`) is the point: the old delay left a window in which the
  // overlay was on screen with the focus still on `document.body`, and Escape
  // pressed during it went nowhere — the keydown only reached the overlay's own
  // handler when focus was already inside it (E2E `6.4`, deterministic on
  // Chromium, invisible on Firefox because its round trip outran the timer).
  $: if ($searchStore.isOpen) {
    tick().then(() => inputRef?.focus());
  }

  // Escape closes the overlay from anywhere while it is open, not only from
  // inside it: it is a modal dialog (`aria-modal`), so the key must work even
  // if focus has not landed yet or was moved to the scrim by a click.
  function onWindowKeydown(e: KeyboardEvent) {
    if (e.key !== 'Escape' || !$searchStore.isOpen) return;
    e.preventDefault();
    closeSearch();
  }

  // The field's own handler keeps the list navigation keys; Escape is handled
  // once, at window level, so the two can never close twice or race.
  function handleKeydown(e: KeyboardEvent) {
    if (e.key === 'ArrowDown') {
      e.preventDefault();
      selectNext();
    } else if (e.key === 'ArrowUp') {
      e.preventDefault();
      selectPrev();
    } else if (e.key === 'Enter') {
      e.preventDefault();
      executeSelected();
    }
  }

  function handleInput(e: Event) {
    const target = e.target as HTMLInputElement;
    updateQuery(target.value, searchIndex);
  }

  // Lock body scroll while the overlay is open; always release on teardown.
  $: if (typeof document !== 'undefined') {
    document.body.style.overflow = $searchStore.isOpen ? 'hidden' : '';
  }

  onDestroy(() => {
    if (typeof document !== 'undefined') document.body.style.overflow = '';
  });
</script>

<!-- Window-level Escape: the overlay is a modal dialog, so the key must close
     it whatever holds focus (see `onWindowKeydown`). It lives outside the
     `{#if}` because `<svelte:window>` cannot sit inside a block; the handler
     no-ops while the overlay is closed. -->
<svelte:window onkeydown={onWindowKeydown} />

{#if $searchStore.isOpen}
  <!-- `svelte-ignore` is deliberate: the click handler is the scrim's
       "click outside to dismiss", and the keyboard path is the window-level
       Escape above — which the linter cannot see from here. Same pattern as
       the launcher's overlay. -->
  <!-- svelte-ignore a11y_click_events_have_key_events -->
  <div
    class="search-overlay"
    role="dialog"
    tabindex="-1"
    aria-modal="true"
    aria-label={$t('search.title')}
    onclick={closeSearch}
  >
    <div class="search-container" role="presentation" onclick={(e)=>{e.stopPropagation();}}>
      <div class="search-input-wrapper">
        <IconSearch size={24} stroke={2} class="search-icon" />
        <input
          bind:this={inputRef}
          type="text"
          placeholder={$t('search.placeholder')}
          aria-label={$t('search.placeholder')}
          value={$searchStore.query}
          oninput={handleInput}
          onkeydown={handleKeydown}
          class="search-input"
          autocomplete="off"
          spellcheck="false"
        />
        <kbd class="search-hint">ESC</kbd>
      </div>

      <SearchResults />
    </div>
  </div>
{/if}

<style>
  .search-overlay {
    position: fixed;
    inset: 0;
    background: var(--hv-modal-overlay);
    z-index: var(--hv-z-launcher);
    display: flex;
    align-items: flex-start;
    justify-content: center;
    padding-top: 100px;
    animation: fadeIn 150ms ease-out;
  }

  @keyframes fadeIn {
    from {
      opacity: 0;
    }
    to {
      opacity: 1;
    }
  }

  .search-container {
    width: 600px;
    max-width: 90vw;
    background: var(--hv-shell-glass);
    border: 1px solid var(--hv-shell-border);
    border-radius: var(--hv-radius-2xl);
    box-shadow: var(--hv-shell-shadow-card);
    overflow: hidden;
    animation: slideIn 200ms ease-out;
  }

  @keyframes slideIn {
    from {
      transform: translateY(-20px);
      opacity: 0;
    }
    to {
      transform: translateY(0);
      opacity: 1;
    }
  }

  .search-input-wrapper {
    display: flex;
    align-items: center;
    gap: var(--hv-space-3);
    padding: var(--hv-space-4) 20px;
    border-bottom: 1px solid var(--hv-shell-border-soft);
    color: var(--hv-shell-text-2);
  }

  .search-input {
    flex: 1;
    border: none;
    outline: none;
    font-size: var(--hv-font-size-lg);
    color: var(--hv-shell-text-strong);
    background: transparent;
  }

  .search-input::placeholder {
    color: var(--hv-shell-text-3);
  }

  .search-hint {
    padding: var(--hv-space-1) 8px;
    background: var(--hv-shell-fill);
    border: 1px solid var(--hv-shell-border);
    border-radius: var(--hv-radius-sm);
    font-size: var(--hv-font-size-xs);
    font-weight: var(--hv-font-weight-semibold);
    color: var(--hv-shell-text-2);
    flex-shrink: 0;
  }
</style>

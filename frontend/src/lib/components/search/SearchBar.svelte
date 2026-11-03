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
  import { get } from 'svelte/store';
  import { searchStore, closeSearch, updateQuery, selectNext, selectPrev, executeSelected, setFilesLoading, type SearchableItem } from '$lib/stores/search';
  import { mergeFileHits } from '$lib/stores/search';
  import { queryFiles, openHitInExplorer } from '$lib/stores/fileProvider';
  import { api } from '$lib/services/api';
  import { focusTrap } from '$lib/utils/focusTrap';
  import SearchResults from './SearchResults.svelte';
  import { IconSearch } from '@tabler/icons-svelte';
  import { t } from '$lib/stores/i18n';
  import type { FileHit } from '$lib/stores/fileProvider';

  let { searchIndex }: { searchIndex: SearchableItem[] } = $props();

  let inputRef: HTMLInputElement | undefined = $state();
  let selectedTag = $state('');
  let availableTags = $state<string[]>([]);
  let tagRequestGeneration = 0;

  // The tag filter offers the caller's own tags, refreshed each time the
  // overlay opens so a tag added in the Explorer is immediately selectable.
  // Tags never widen the search: the server still confines the answer to the
  // roots this session may browse.
  $effect(() => {
    const open = $searchStore.isOpen;
    const generation = ++tagRequestGeneration;
    if (!open) {
      selectedTag = '';
      availableTags = [];
      return;
    }
    api.getExplorerTags().then((response) => {
      if (generation !== tagRequestGeneration || !get(searchStore).isOpen) return;
      if (response.success && Array.isArray(response.data)) {
        availableTags = [...new Set(response.data.map((entry: { tag: string }) => entry.tag))].sort();
      }
    }).catch(() => {
      if (generation === tagRequestGeneration) availableTags = [];
    });
  });

  /**
   * Re-run the query with the current tag filter. One path for the keystroke
   * and the filter change, so a file hit can never be shown under a filter it
   * was not selected by.
   */
  function runFileQuery(query: string) {
    const localResults = get(searchStore).results;
    queryFiles(
      query,
      (forQuery, items) => {
        const state = get(searchStore);
        if (state.query !== forQuery) return;
        mergeFileHits(items, { ...state, results: localResults }, forQuery);
      },
      (forQuery, note) => {
        if (get(searchStore).query === forQuery) fileNote = note;
      },
      (loading, forQuery) => {
        setFilesLoading(loading, forQuery);
        if (!loading) filesReady(forQuery);
      },
      openHitInExplorer,
      selectedTag || undefined,
      selectedTag ? addTagToHit : undefined
    );
  }

  /**
   * The per-row action, offered only while a tag filter is active: apply that
   * same tag to the hit. It is the "this one belongs with the others" gesture
   * — tagging from scratch stays in the Explorer sidebar, so the overlay never
   * has to invent a label.
   */
  async function addTagToHit(hit: FileHit) {
    if (!selectedTag) return;
    const response = await api.addExplorerTag(hit.path, selectedTag);
    fileNote = get(t)(response.success ? 'explorer.tag_added' : 'explorer.tag_add_failed');
  }


  // Focus the field as soon as the overlay has been rendered. `tick()` (not a
  // `setTimeout(…, 50)`) is the point: the old delay left a window in which the
  // overlay was on screen with the focus still on `document.body`, and Escape
  // pressed during it went nowhere — the keydown only reached the overlay's own
  // handler when focus was already inside it (E2E `6.4`, deterministic on
  // Chromium, invisible on Firefox because its round trip outran the timer).
  $effect(() => {
    if ($searchStore.isOpen) tick().then(() => inputRef?.focus());
  });

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

  // Every keystroke runs the local index synchronously (apps, settings,
  // actions — it answers instantly) and hands the query to the cross-root file
  // provider, which debounces and merges its own hits in when they land
  // (QW-12). The provider drops a superseded answer itself, and `mergeFileHits`
  // checks the query again so a response can never land under a newer query.
  function handleInput(e: Event) {
    const target = e.target as HTMLInputElement;
    const query = target.value;
    updateQuery(query, searchIndex);
    runFileQuery(query);
  }

  /** The coverage note for the current query ("2 folders could not be read"). */
  let fileNote = $state('');
  /** `true` once the file provider answered for the current query. */
  let filesAnswered = $state(false);

  function filesReady(forQuery: string) {
    if (get(searchStore).query === forQuery) filesAnswered = true;
  }

  // A new overlay session starts with no provider state.
  $effect(() => {
    if (!$searchStore.isOpen && (fileNote || filesAnswered)) {
      fileNote = '';
      filesAnswered = false;
    }
  });

  // Lock body scroll while the overlay is open; always release on teardown.
  $effect(() => {
    if (typeof document === 'undefined') return;
    document.body.style.overflow = $searchStore.isOpen ? 'hidden' : '';
  });

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
    use:focusTrap={{ escape: closeSearch }}
    onclick={closeSearch}
  >
    <div class="search-container" role="presentation" onclick={(e)=>{e.stopPropagation();}}>
      <div class="search-input-wrapper">
        <IconSearch size={24} stroke={2} class="search-icon" />
        <input
          bind:this={inputRef}
          data-focus-return
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

      <!-- The filter is offered before the first keystroke: choosing the tag
           first and then typing is the natural order, and hiding it until two
           characters made that impossible. -->
      {#if availableTags.length > 0 || selectedTag}
        <label class="search-tag-filter">
          <span>{$t('explorer.filter_by_tag')}</span>
          <select
            value={selectedTag}
            aria-label={$t('explorer.filter_by_tag')}
            onchange={(event) => {
              selectedTag = event.currentTarget.value;
              runFileQuery(inputRef?.value ?? $searchStore.query);
            }}
          >
            <option value="">{$t('explorer.all_tags')}</option>
            {#each availableTags as tag (tag)}<option value={tag}>{tag}</option>{/each}
          </select>
        </label>
      {/if}
      <SearchResults {fileNote} />
      <!-- The file provider is still walking the roots: say so instead of
           letting "no results" imply the file does not exist (QW-12). -->
      {#if $searchStore.filesLoading}
        <div class="files-status" data-testid="search-files-loading" role="status">
          {$t('search.files_searching')}
        </div>
      {/if}
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

  .search-tag-filter {
    display: flex;
    align-items: center;
    gap: var(--hv-space-2);
    padding: var(--hv-space-2) 20px;
    color: var(--hv-shell-text-2);
    font-size: var(--hv-text-body);
  }

  .search-tag-filter select {
    min-height: var(--hv-touch-target-min);
    max-width: 220px;
    border: 1px solid var(--hv-shell-border);
    border-radius: var(--hv-radius-control);
    padding: 0 var(--hv-space-3);
    color: var(--hv-shell-text-1);
    background: var(--hv-shell-card);
  }

  .search-tag-filter select:focus-visible {
    outline: none;
    box-shadow: var(--hv-focus-ring);
  }

  .files-status {
    padding: var(--hv-space-2) 20px var(--hv-space-3);
    border-top: 1px solid var(--hv-shell-border-soft);
    font-size: var(--hv-font-size-sm);
    color: var(--hv-shell-text-3);
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

/* =============================================================================
 * File          : search.ts
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 28/09/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   State of the global search overlay plus its scoring engine: the index
 *   (apps, settings, actions) is filtered case-insensitively on title,
 *   description and keywords, scored (exact/prefix title weight, keyword
 *   and type bonuses), capped at 8 results, and navigable by keyboard.
 *
 * Dependencies:
 *   - svelte (>= 5)
 *
 * Usage:
 *   searchStore + openSearch/updateQuery/selectNext/executeSelected from
 *   the search overlay components.
 * =============================================================================
 */

import { writable, get } from 'svelte/store';

/** An item the user can find via the search overlay. */
export interface SearchableItem {
  id: string;
  type: 'app' | 'setting' | 'action' | 'file';
  title: string;
  description: string;
  icon: string;
  color: string;
  keywords: string[];
  action: () => void;
}

interface SearchState {
  isOpen: boolean;
  query: string;
  results: SearchableItem[];
  selectedIndex: number;
  /**
   * `true` while the cross-root file provider is still answering. The overlay
   * shows a row for it instead of "no results" — a file search that hides its
   * own latency reads as "the file does not exist".
   */
  filesLoading: boolean;
  /**
   * What the file provider could not cover, in plain words: the roots whose
   * walk ran out of budget, and the roots that could not be read. Empty when
   * the answer is complete.
   */
  filesNote: string;
}

const initialState: SearchState = {
  isOpen: false,
  query: '',
  results: [],
  selectedIndex: 0,
  filesLoading: false,
  filesNote: ''
};

export const searchStore = writable<SearchState>(initialState);

/** Open the search overlay and reset all search state. */
export function openSearch() {
  searchStore.update(s => ({
    ...s,
    isOpen: true,
    query: '',
    results: [],
    selectedIndex: 0,
    filesLoading: false,
    filesNote: ''
  }));
}

/** Close the search overlay and clear state. */
export function closeSearch() {
  searchStore.update(s => ({
    ...s,
    isOpen: false,
    query: '',
    results: [],
    selectedIndex: 0,
    filesLoading: false,
    filesNote: ''
  }));
}

/** Run a search against the provided index and store the results. */
export function updateQuery(query: string, searchIndex: SearchableItem[]) {
  const results = search(query, searchIndex);
  searchStore.update(s => ({ ...s, query, results, selectedIndex: 0 }));
}

/**
 * Merge the cross-root file hits into whatever the local index already found.
 *
 * The local index (apps, settings, actions) always comes first: it is what the
 * user asked for 99 % of the time, and it answers instantly, so a file hit
 * that had to walk the disk must never push "Settings" off the list. File hits
 * then appear in the provider's own order (exact name → prefix → shortest),
 * deduplicated against the local results by id.
 *
 * `onlyIfQuery` guards against a stale answer: an HTTP response that lands
 * after the user typed something else must be dropped, not merged into the
 * new query's results.
 */
export function mergeFileHits(
  fileHits: SearchableItem[],
  state: SearchState,
  onlyIfQuery?: string
) {
  searchStore.update(s => {
    if (!s.isOpen) return s;
    if (onlyIfQuery !== undefined && s.query !== onlyIfQuery) return s;
    const seen = new Set(state.results.map(item => item.id));
    const local = state.results.filter(item => item.type !== 'file');
    const files = fileHits.filter(item => !seen.has(item.id));
    return {
      ...s,
      results: [...local, ...files],
      filesLoading: false,
      filesNote: state.filesNote
    };
  });
}

/** Mark the file provider as running (or done) for the current query. */
export function setFilesLoading(loading: boolean, query?: string) {
  searchStore.update(s => {
    if (query !== undefined && s.query !== query) return s;
    return { ...s, filesLoading: loading };
  });
}

/** Move selection to the next result (wraps at end). */
export function selectNext() {
  searchStore.update(s => ({
    ...s,
    selectedIndex: Math.min(s.selectedIndex + 1, s.results.length - 1)
  }));
}

/** Move selection to the previous result (clamped at 0). */
export function selectPrev() {
  searchStore.update(s => ({
    ...s,
    selectedIndex: Math.max(s.selectedIndex - 1, 0)
  }));
}

/** Run the action of the currently selected result, then close search. */
export function executeSelected() {
  const state = get(searchStore);
  const item = state.results[state.selectedIndex];
  if (item) {
    item.action();
    closeSearch();
  }
}

/**
 * Filter the full search index by title, description, and keywords (case-insensitive).
 * Results are scored and sorted by relevance, capped at 8 items.
 */
function search(query: string, searchIndex: SearchableItem[]): SearchableItem[] {
  if (!query) return [];

  const lowerQuery = query.toLowerCase();

  return searchIndex
    .filter(item =>
      item.title.toLowerCase().includes(lowerQuery) ||
      item.description.toLowerCase().includes(lowerQuery) ||
      item.keywords.some(k => k.includes(lowerQuery))
    )
    .sort((a, b) => {
      const scoreA = calculateScore(a, lowerQuery);
      const scoreB = calculateScore(b, lowerQuery);
      return scoreB - scoreA;
    })
    .slice(0, 8);
}

/**
 * Score an item against the query:
 * - Exact title match gets the highest weight (100).
 * - Title prefix match (50) and substring match (20) are next.
 * - Keyword and type bonuses help surface direct hits.
 */
function calculateScore(item: SearchableItem, query: string): number {
  let score = 0;

  if (item.title.toLowerCase() === query) score += 100;
  else if (item.title.toLowerCase().startsWith(query)) score += 50;
  else if (item.title.toLowerCase().includes(query)) score += 20;

  if (item.keywords.some(k => k === query)) score += 10;

  if (item.type === 'app') score += 5;
  else if (item.type === 'action') score += 3;

  return score;
}

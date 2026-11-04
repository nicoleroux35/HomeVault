/* =============================================================================
 * File          : fileProvider.ts
 * Author        : Nicolas Le Roux
 * Created       : 31/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   The cross-root file provider behind the desktop's ⌘K overlay (QW-12):
 *   debounces keystrokes, drops stale answers, turns the server's hits into
 *   searchable items that open the file's folder in the Explorer, and reports
 *   what the search could not cover.
 *
 * Dependencies:
 *   - $lib/services/api, $lib/stores/search, $lib/stores/explorerIntent
 *
 * Usage:
 *   The desktop shell calls `queryFiles(query)` from its search input;
 *   `openHitInExplorer()` is the action each file result runs.
 * =============================================================================
 */

import { api } from '$lib/services/api';
import { translate } from '$lib/stores/i18n';
import type { SearchableItem } from '$lib/stores/search';
import { explorerIntent } from '$lib/stores/explorerIntent';
import type { ContentHit, ContentSearchResponse } from '$lib/types/phase4';

/** Shortest query that starts a disk walk — mirrors `MIN_QUERY_LEN`. */
export const MIN_FILE_QUERY = 2;

/** How many file hits the overlay shows. Eight is the overlay's own cap for
 *  the local index; files get the same room, no more. */
const FILE_HITS = 8;

/** Content matches are a supplement, not a second list: four at most. */
const CONTENT_HITS = 4;

/** Keystroke debounce. A walk per keypress would fan out sixteen roots on
 *  every character; 220 ms is below the threshold where a fast typist waits. */
const DEBOUNCE_MS = 220;

/** One hit as `GET /api/search/files` answers it (`SearchHit` in Rust). */
export interface FileHit {
  name: string;
  path: string;
  is_directory: boolean;
  size_bytes: number;
  modified: string;
  permissions: string;
  owner: string;
  filesystem?: string | null;
  root: string;
  root_kind: string;
}

/** The whole report, including what could not be covered. */
export interface FileSearchReport {
  query: string;
  hits: FileHit[];
  roots: { path: string; kind: string; name: string }[];
  timed_out: string[];
  unreadable: string[];
  truncated: boolean;
  elapsed_ms: number;
  tag?: string | null;
}

let timer: ReturnType<typeof setTimeout> | null = null;
/** Increments on every query so a late answer for an older one is dropped. */
let generation = 0;

/** Human sentence for what the search could not cover, or `''`. */
export function coverageNote(report: FileSearchReport): string {
  const missing = [...report.timed_out, ...report.unreadable];
  if (missing.length === 0) {
    return report.truncated ? translate('search.files_truncated') : '';
  }
  return translate('search.files_partial', { count: String(missing.length) });
}

/** One file hit → the overlay's item shape. `path` travels in the item id and
 *  in the closure only: never in the URL. */
export function toSearchItem(hit: FileHit, onOpen: (hit: FileHit) => void, onTag?: (hit: FileHit) => void): SearchableItem {
  const folder = hit.path.replace(/\/+$/, '').split('/').slice(0, -1).join('/') || '/';
  return {
    id: `file-${hit.path}`,
    type: 'file',
    title: hit.name,
    // The folder, so two identically named files in different shares are
    // distinguishable at a glance — which is the whole point of a cross-root
    // search.
    description: `${translate(`explorer.root_${hit.root_kind}`) || hit.root} · ${folder}`,
    icon: hit.is_directory ? 'folder' : 'file',
    color: 'var(--hv-text-secondary)',
    keywords: [hit.name.toLowerCase(), hit.root.toLowerCase()],
    action: () => onOpen(hit),
    ...(onTag ? { tag: () => onTag(hit) } : {})
  };
}

/**
 * Ask the server for files matching `query`, then hand the items to `onResults`
 * and the coverage note to `onNote`. Debounced, and the answer of a superseded
 * query is dropped (the generation guard) — a slow walk over a network share
 * must never overwrite the results of what the user typed next.
 */
export function queryFiles(
  query: string,
  onResults: (query: string, items: SearchableItem[]) => void,
  onNote: (query: string, note: string) => void,
  onLoading: (loading: boolean, query: string) => void,
  onOpen: (hit: { path: string }) => void,
  tag?: string,
  onTag?: (hit: FileHit) => void
): void {
  if (timer) clearTimeout(timer);
  const trimmed = query.trim();
  if (trimmed.length < MIN_FILE_QUERY) {
    generation += 1;
    onLoading(false, query);
    onResults(query, []);
    onNote(query, '');
    return;
  }
  onLoading(true, query);
  const mine = ++generation;
  timer = setTimeout(async () => {
    // The name walk and the content index are independent: a slow content
    // answer must not delay the files, and a disabled index must not turn
    // into an error note.
    const [filesResponse, contentResponse] = await Promise.all([
      api.searchAllFiles(trimmed, FILE_HITS, false, tag),
      api.searchContent(trimmed, CONTENT_HITS).catch(() => null)
    ]);
    if (mine !== generation) return; // superseded

    const items: SearchableItem[] = [];
    const seen = new Set<string>();
    let note = '';

    if (filesResponse.success && filesResponse.data) {
      const report = filesResponse.data as unknown as FileSearchReport;
      for (const hit of report.hits || []) {
        items.push(toSearchItem(hit, onOpen, onTag));
        seen.add(hit.path);
      }
      note = coverageNote(report);
    } else {
      // A failed provider is not "no files": say so, and keep the local
      // results the overlay already has.
      note = translate('search.files_unavailable');
    }

    const contentReport =
      contentResponse && contentResponse.success
        ? (contentResponse.data as unknown as ContentSearchResponse)
        : null;
    if (contentReport?.available) {
      for (const hit of contentReport.hits || []) {
        if (seen.has(hit.path)) continue;
        items.push(toContentItem(hit, onOpen));
        seen.add(hit.path);
        if (items.length >= FILE_HITS + CONTENT_HITS) break;
      }
    }

    onResults(query, items);
    onNote(query, note);
  }, DEBOUNCE_MS);
}

/**
 * A content match → the overlay's item shape. The snippet is the point: two
 * files with the same name are told apart by the sentence that matched.
 */
export function toContentItem(
  hit: ContentHit,
  onOpen: (hit: { path: string }) => void
): SearchableItem {
  const folder = hit.path.replace(/\/+$/, '').split('/').slice(0, -1).join('/') || '/';
  const rootLabel = translate(`explorer.root_${hit.root_kind}`) || hit.root_name || hit.root;
  const snippet = (hit.snippet || '').replace(/\s+/g, ' ').trim();
  return {
    id: `content-${hit.path}`,
    type: 'file',
    title: hit.name,
    description: snippet ? `${rootLabel} · ${snippet}` : `${rootLabel} · ${folder}`,
    icon: 'file',
    color: 'var(--hv-text-secondary)',
    keywords: [hit.name.toLowerCase(), hit.root.toLowerCase(), snippet.toLowerCase()],
    action: () => onOpen({ path: hit.path })
  };
}

/**
 * Open a hit's **folder** in the Explorer, with the item selected.
 *
 * The overlay hands the intent to the desktop (which owns every window) through
 * the one-shot store the Explorer's own "share this folder" handoff uses — the
 * same convention, one more consumer.
 */
export function openHitInExplorer(hit: { path: string }): void {
  const folder =
    hit.path.replace(/\/+$/, '').split('/').slice(0, -1).join('/') || '/';
  explorerIntent.set({ path: folder, select: hit.path });
  if (typeof window !== 'undefined') {
    window.dispatchEvent(new CustomEvent('hv-open-app', { detail: { id: 'explorer' } }));
  }
}

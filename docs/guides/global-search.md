# Global file search (⌘K)

**Purpose:** find a file by name across every folder you are allowed to read,
without knowing which share it lives in.

The desktop search overlay (`Ctrl+K` / `⌘K`) used to answer "which app or
setting is called this" and nothing else. The Explorer's own search is scoped to
the folder you already opened. This provider closes the gap: one query fanned out
over every root your Explorer sidebar lists.

## Backend

| Route | Auth | Answers |
|---|---|---|
| `GET /api/search/files?q=<name>&limit=<n>&files_only=<bool>` | session | `{ query, hits[], roots[], timed_out[], unreadable[], truncated, elapsed_ms }` |

`hits[]` is the Explorer's `FileInfo` plus `root` (the root that produced the
hit) and `root_kind` (`home`, `users`, `shares`, `media`, `storage`,
`virtualdisk`, …), so a row can say *where* the file is.

Implementation: `src/global_search.rs` (validation, root dedupe, merge/rank,
confinement — all pure and unit-tested) and `src/handlers/search.rs` (the
fan-out).

## Rules

- **The roots are the Explorer's.** `handlers::search` calls
  `explorer::explorer_roots_for_session`, the exact function
  `handle_explorer_roots` delegates to. One definition, two callers: a folder
  the sidebar would not show can never produce a hit.
- **Admins are bounded too.** The Explorer lets an admin resolve paths outside
  the configured roots; the search does not. An admin's ⌘K never becomes a
  `find /`.
- **A query names a file, never a path.** 2–128 **characters** (counted as
  characters, so an accented name is not refused), no `/`, `\` or NUL, trimmed
  and lowercased. A refusal is a `400` naming the rule — never a `200` with an
  empty list, which would read as "no such file".
- **Bounded end to end.** At most 16 roots after dedupe, 1.5 s per root, 4 s in
  total, at most 100 hits whatever `limit` says.
- **The answer says when it is incomplete.** `timed_out` / `unreadable` name the
  roots that did not answer; `truncated` says the list was cut. The overlay
  renders that under the results (`search.files_partial` /
  `search.files_truncated` / `search.files_unavailable`).
- **Name search only.** No content index is built, and the docs say so rather
  than implying otherwise.

## Frontend

- `frontend/src/lib/stores/fileProvider.ts` — the 220 ms debounce, the
  generation counter that drops a superseded answer, the hit → row mapping (the
  folder is in the description, so `photo.jpg` in two shares is
  distinguishable), and the open action.
- `frontend/src/lib/stores/explorerIntent.ts` — one-shot "open this folder,
  select this entry", the same handoff convention `explorerShareIntent` uses for
  the Shares app.
- `SearchBar.svelte` / `SearchResults.svelte` — run the provider on every
  keystroke, render "Looking for matching files…" while it works, and put the
  coverage note under the results.
- `Explorer.svelte` — `openExplorerIntent()` consumes the intent after the first
  listing and selects the entry when it is still there.

### Ordering: navigation first, files after

`mergeFileHits` (`frontend/src/lib/stores/search.ts`) puts every
**navigation** row — `app`, `setting`, `action` — before any `file` row, then
caps the merged list at `MAX_RESULTS` (8).

This is a correctness rule, not a preference. Before it, the merge appended
files after the local index *as it stood at the time the async answer landed*,
which was not the same as the index for the current query: typing `Settings`
showed `settings.png`, `Settings.java` and a `settings.gradle.kts` from the
host's own source tree in the eight visible rows, and "Open Settings" was not
reachable with the arrow keys at all. The overlay's job is "where do I go?";
a disk walk is the long tail of that answer.

Files remain fully searchable — they simply sit after everything that
navigates. Capping also clamps `selectedIndex`, so a shorter merged list can
never leave the selection pointing past the end.

## Invariants

- A file outside the allowed roots is never returned, for any role.
- A one-character query never reaches the network.
- The path of a result never appears in the URL.
- **A navigation row is never pushed off the list by a file hit.** Navigation
  types sort before `file`, and the merged list stops at `MAX_RESULTS`.
- A partial answer never looks complete: if any root did not answer, the note
  says how many.
- The tests: `src/global_search.rs` (17 unit), `tests/global_search_integration.rs`
  (13, incl. the ACL and admin-confinement cases), `tests/e2e/desktop/desktop.spec.ts`
  §6b (6).

## Related

- [Explorer](./explorer.md) — the per-folder search this one sits above
- [Desktop](./desktop.md) — the ⌘K overlay itself

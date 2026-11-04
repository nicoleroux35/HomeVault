# D1 — Content index: design note

Status: implemented (2026-11-06). Guide:
[`docs/guides/content-index.md`](../guides/content-index.md).

This note records the decisions the tasklist required before the first line of
code: opt-in per root, explicit exclusions, ACL-filtered results, bounded
index size, retention, and a "delete my index" action.

---

## 1. What the item is

Full-text search over the **contents** of text files in the folders an
administrator chooses. The existing global search is a **name** search
(`docs/API.md`: "Name search only (no content index)"); this adds the missing
half without turning the appliance into a search cluster.

Non-goals for this version:

- no PDF/Office/image text extraction (no `pdftotext`, no OCR, no new daemon);
- no per-user index: there is **one appliance-wide index**, filtered for each
  caller at query time;
- no remote/cloud index, no vector search, no embeddings.

## 2. Storage — SQLite FTS5, not a search server

A new migration creates:

- `content_index_docs` — one row per indexed file: `path` (unique), `root`,
  `name`, `body`, `size_bytes`, `modified` (unix seconds), `last_seen`,
  `indexed_at`;
- `content_index_fts` — FTS5 external-content table over `name` and `body`,
  `unicode61 remove_diacritics 2`, kept in sync by insert/update/delete
  triggers.

FTS5 already ships in the bundled SQLite; a Meilisearch/Elasticsearch process
would contradict the 1 GB RAM target. `bm25()` ranks, with the file name
weighted above the body.

## 3. Opt-in per root

Nothing is indexed by default (`enabled: false`). An administrator:

1. turns the feature on and confirms it (reading file contents is a privacy
   decision, so the switch alone is not enough);
2. ticks one or more **roots** from the appliance's own Explorer roots
   (shares, pools, mounted disks — never a hand-typed path);
3. saves.

A root must be one of the candidate roots or a descendant of one. `/`, `/etc`,
`/var` and anything outside the Explorer roots are refused. Removing a root
removes its documents immediately.

## 4. Explicit exclusions

- **Names/globs**: `.git`, `node_modules`, `.cache`, `#recycle`, `@eaDir`,
  `$RECYCLE.BIN`, `lost+found` by default; `*` is supported (`*.bak`).
- **Extensions**: an ignore list (media, archives, images, PDF, Office) on top
  of a **text allowlist**. A file that is not on the allowlist is never read,
  so a misconfigured exclusion cannot turn the worker into a binary parser.
- Symlinks are skipped (no loops, no escape from the root), hidden entries are
  skipped unless the admin removes the name rule, and every canonical path is
  checked to remain under its canonical root.
- Files above **1 MiB**, empty files, files containing NUL in the first 8 KiB
  and invalid UTF-8 are skipped.

## 5. ACL-filtered results

The index is written by the appliance, but **no query returns a path the
caller could not open**. The search handler:

1. gets the caller's roots from `explorer_roots_for_session` — the same list
   the Explorer sidebar uses;
2. asks FTS5 for a bounded candidate set;
3. keeps only hits whose path is under one of those roots (boundary-aware:
   `/srv/share` matches `/srv/share/a` but not `/srv/share2`);
4. only then builds the snippet.

The snippet is therefore never produced for a document the caller may not
read. A share revoked five minutes ago disappears on the next query even if the
index still holds its text.

## 6. Bounds and retention

| Bound | Value |
|---|---|
| Roots | 16 |
| Documents | 50 000 |
| Stored text | 256 MiB |
| File size | 1 MiB |
| Walk depth | 32 |
| Query tokens | 8 |
| Results | 50 |

The worker stops adding when a cap is reached and reports `truncated`, so the
screen never claims a complete index when it is not. Each run:

- touches every still-indexed file (`last_seen`), re-reads only when size or
  mtime changed;
- after a root has been walked completely, deletes its documents that were not
  seen (file deleted, renamed or excluded);
- deletes documents whose `last_seen` is older than `retention_days`
  (default 30, 1–3650) — the safety net for a root that stayed unreadable.

Runs are sequential (one at a time), every 30 minutes when enabled, and on
demand with **Rebuild now**.

## 7. "Delete my index"

`DELETE /api/search/index?confirm=true` (admin) deletes every document and its
FTS rows in one transaction and turns the feature off, so the worker cannot
re-create them behind the user's back. Removing one root does the same for
that root. The action is named and confirmed in the card.

## 8. Query safety

The query is tokenised into at most eight alphanumeric/underscore/hyphen
tokens, each wrapped in a quoted FTS5 literal and joined with `AND`. FTS5
operators (`OR`, `NOT`, `NEAR`, `*`, `-`, `:`) never reach the parser, so a
query can neither error nor widen the result set. The existing
`validate_query` length rules apply.

## 9. Failure answers

| Situation | Answer |
|---|---|
| Feature off | `GET /search/content` returns `{ available: false, hits: [] }` (200) |
| No candidate root / removed root | `400` naming the path |
| Root not under an Explorer root | `400` |
| Empty/too-short/path-shaped query | `400` naming the rule (shared with the name search) |
| Worker running when another run is asked | `202` `{ started: false }` |
| Unreadable file/directory | skipped, counted in the run report; never fails the run |
| Index cap reached | `truncated: true` in the state and in the search answer |

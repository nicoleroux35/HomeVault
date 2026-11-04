# Content search (full-text index)

The global search in ⌘K finds files by **name**; this feature adds a second,
optional half: text inside the files. It is one SQLite **FTS5** index for the
whole appliance, built only from the folders an administrator opts in to, and
every answer is filtered by the caller's own Explorer access.

Settings → System → **Content search**
(`GET|PUT /api/search/index`, `POST /api/search/index/run`,
`DELETE /api/search/index`, `GET /api/search/content`).

The design note is
[`docs/analysis/d1-content-index-design.md`](../analysis/d1-content-index-design.md).

---

## Turning it on

1. Open **Settings → System → Content search**.
2. Switch it on: a named confirmation explains that HomeVault will read the
   chosen folders.
3. Tick one or more **folders** from the appliance's own Explorer roots
   (shares, pools, mounted disks). Each one is a real path; nothing is typed.
4. Adjust the exclusions and retention if needed, then **Save**.

Off by default. Turning it off stops future runs but keeps the existing
documents until they are deleted or aged out; **Delete the index** removes them
and turns the feature off in one action.

## What is read

Only files that pass **all** of these:

- on the text allowlist (text, Markdown, source code, configuration, JSON,
  YAML, CSV, HTML/XML/CSS, SQL, shell, …);
- not on the extension ignore list (media, archives, images, PDF, Office);
- not under an excluded name or `*`-pattern (`.git`, `node_modules`, `*.bak`,
  … by default);
- not hidden (any path component starting with `.`, so `.ssh` and `.env` are
  never indexed);
- at most **1 MiB**, non-empty, valid UTF-8 and free of NUL bytes;
- reached without following a symlink, inside a canonicalised root, at depth
  ≤ 32.

PDFs, Office documents, images and archives are **not** extracted. That would
need an external parser or OCR; the feature deliberately does not add one.

## How results are protected

The index is written by the appliance, but a query never returns a path the
caller could not open:

1. `/api/search/content` resolves the caller's roots with
   `explorer_roots_for_session` — the same list the Explorer sidebar shows;
2. FTS5 returns a bounded candidate set;
3. hits are filtered by path boundary (`/srv/share` matches
   `/srv/share/a`, never `/srv/share2`);
4. only then is the snippet built.

Revoking a share takes effect on the next query, even if its text is still in
the index. Search is available to every signed-in user; configuration,
rebuild and deletion are admin-only.

## Limits and retention

| Limit | Value |
|---|---|
| Roots | 16 |
| Documents | 50 000 |
| Stored text | 256 MiB |
| File size | 1 MiB |
| Query tokens | 8 |
| Results per query | 50 |

When a cap is reached the screen says **truncated** instead of pretending the
index is complete. Each run:

- re-reads a file only when its size or mtime changed;
- after walking a root completely, removes documents there that were not seen
  (deleted, renamed or newly excluded);
- drops documents not seen for `retention_days` (default 30, range 1–3650).

Runs happen every 30 minutes when enabled, and on demand with **Rebuild now**.
One run at a time.

## Query syntax

Plain words. Up to eight tokens are quoted and `AND`ed; FTS5 operators in the
input (`OR`, `NOT`, `NEAR`, `*`, `-`, `:`) are treated as text, so a query can
neither fail nor widen the result set. Queries shorter than two characters are
refused, exactly like the name search.

## Failure answers

| Situation | Answer |
|---|---|
| Feature off | `GET /search/content` → `{ available: false, hits: [] }` (200); the overlay simply shows name hits |
| Root not under an Explorer root | `400` naming the path |
| Enabling without `confirmed: true` | `400` |
| Empty/short/path-shaped query | `400` naming the rule |
| A run is already in progress | `202` with `{ started: false }`; the card shows "Indexing now…" |
| A folder becomes unreadable | reported in the status line; the run continues and does not delete that root's documents |
| Database error | `500` with the SQLite sentence; the rest of the appliance is unaffected |

## Tests and verification

| Level | File |
|---|---|
| Unit | `src/content_index/mod.rs` — config/root/retention limits, glob exclusions, text allowlist, safe FTS expression, ACL boundary, hit cap |
| Unit | `src/storage/database.rs` — the migration parser keeps `CREATE TRIGGER … BEGIN … ; … END;` together |
| Integration | `tests/content_index_integration.rs` — FTS insert/update/delete triggers, retention/prune, admin gate, config refusals, delete-my-index, ACL filtering, operator-only queries |
| E2E | `tests/e2e/settings/content-index.spec.ts` — off by default, confirmation, save payload, refusal, status/rebuild, named delete, responsive |

**Never exercised live:** a multi-terabyte share, a root that disappears
mid-run, and a database at the 50 000-document cap. The walker's per-file rules
are unit-tested and the FTS schema/triggers are exercised on every integration
run.

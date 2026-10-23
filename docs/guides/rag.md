# RAG Knowledge Base

**Purpose:** Retrieval-Augmented Generation system that enriches Chat AI responses
with relevant context from the app catalog, feature documentation, and i18n
translations. Uses Ollama embeddings (`nomic-embed-text`) with cosine similarity
search in SQLite.

## Architecture

```
User query → embed via Ollama /api/embed → cosine similarity search
→ top-K chunks → inject into system prompt → Ollama /api/chat
```

Three layers:
1. **Chunking** (`src/rag.rs`) — split documents into ~400-word chunks with 80-word overlap
2. **Embedding** (`src/rag.rs`) — batch embed via Ollama `nomic-embed-text` model
3. **Indexing** (`src/rag_indexers.rs`) — collect documents from 3 sources, embed, store

## Backend

Core module: `src/rag.rs` — chunking, embedding, vector math, SQLite storage, search.
Indexers: `src/rag_indexers.rs` — catalog, features, i18n document collectors.
Handlers: `src/handlers/rag.rs` — API endpoints for status, indexing, search, documents.
Migration: `sqlx/migrations/033_rag_knowledge_base.sql` — `rag_documents`, `rag_chunks`, `rag_index_status` tables.

Router entries in `src/web/mod.rs` under `/api/rag`.

| Endpoint | Auth | Notes |
|----------|------|-------|
| `GET /rag/status` | auth | index status (last indexed, doc/chunk count, the embedding model the index needs, `idle`/`indexing`/`error` state) |
| `POST /rag/index` | admin | triggers full reindex + reconcile in background (`tokio::spawn`); refused (`success:false`) while one is already running |
| `POST /rag/search` | admin | semantic search test — returns top-K results + context string; an embedding error is `success:false`, never an empty result |
| `GET /rag/documents` | admin | list all indexed documents; a DB error is `success:false`, never an empty list |

## Integration with Chat

`src/handlers/chat.rs` calls `system_prompt_with_rag()` instead of `system_prompt()`.
The function embeds the user's query, searches for similar chunks, and appends the
context block to the system prompt. Falls back to plain prompt on embedding failure
**and logs it** — an empty index and a broken one are told apart in the logs.

## Document Sources

| Source | ID prefix | Content |
|--------|-----------|---------|
| App catalog | `catalog:<app_id>` | Manifest fields: name, description (4 langs), category, developer, website, port, GPU, arch |
| Feature docs | `feature:<name>` | **50** feature docs — **one per guide in `docs/guides/`**, minus `design-system.md` and `development.md` (developer-only, deliberately excluded). The ids are not always the guide's filename (`backup` ← `backup-restic.md`, `virtual-machines` ← `vm.md`, `setup` ← `setup-wizard.md`, `cloudsync` ← `cloud-sync.md`, `scheduled-tasks`/`metrics`/`power` match their file names). Full list, and the id → guide mapping that is asserted in both directions: `src/rag_indexers.rs::tests::FEATURE_GUIDES`. |
| i18n translations | `i18n:<lang>:<section>` | All TOML keys grouped by section, per language (en/fr/de/es), read from `i18n::embedded_translation_sources()` — the bundles compiled into the binary |

## Frontend

- `frontend/src/lib/services/api.ts`: `ragStatus`, `ragIndex`, `ragSearch`, `ragDocuments`.
- **Settings → AI Assistant → Knowledge Base** (`frontend/src/lib/components/settings/KnowledgeBaseCard.svelte`):
  the index status (documents, chunks, state, last indexed), the embedding-model
  gate, the install action and the reindex button. It polls `GET /rag/status`
  every 2 s while `status == "indexing"`, so progress is visible; the reindex
  itself also appears in the desktop activity taskbar as a `rag_index` job.
  The status is readable by any session; the two actions are admin-only and are
  simply absent for a non-admin.
- **Chat** (`frontend/src/routes/Chat.svelte`) shows a notice in its empty state
  when the index has zero documents, with an "index now" action for an admin —
  the honest version of "I don't know", which the assistant otherwise gives for
  shipped features whenever the index was never built.

## Invariants

- **Reindex reconciles, it does not only upsert.** After each source is
  indexed, `index_all` prunes every document of that source the run did not
  produce (`rag::prune_source`). Before 2026-10-22 `clear_source` had zero
  callers, so a deleted or renamed guide / app / i18n section stayed
  searchable — and in every chat answer — forever.
- **An embedding failure fails the run.** `upsert_document_with_chunks` embeds
  before writing anything and returns the error; `index_all` records the
  `error` status and returns `Err`. A chunk is never stored with a `NULL`
  embedding (which would be present in the table but invisible to search).
- **One reindex at a time.** `POST /rag/index` claims a process-wide
  `ReindexGuard`; a second request while one is in flight answers
  `{"success":false, "error":"A reindex is already running"}`.
- Search distinguishes an error from an empty result: the search handler
  returns `success:false` with the reason, and `GET /rag/documents` no longer
  turns a DB error into an empty list.
- `GET /rag/status` tells the truth about *when*: `last_indexed_at` only
  advances when a run completes (`status = idle`), and `total_documents` /
  `total_chunks` are only replaced by a completed run. A `indexing` or `error`
  state therefore keeps the counts of the index that is still serving answers,
  instead of reporting "last indexed: now" with zero documents after a failed
  run.
- Embedding model is hardcoded to `nomic-embed-text` — the admin must pull it via
  Settings → AI Assistant → Knowledge Base before indexing (`GET /rag/status`
  reports the required model even before the first index, so the card can tell
  whether it is installed).
- `chunk_text` splits on word boundaries at ~400 tokens with 80-token overlap.
- `cosine_similarity` returns 0.0 for mismatched lengths or zero norms.
- Embedding vectors are serialized as f32 little-endian BLOBs in SQLite.
- `build_rag_context` caps output at 3000 chars to avoid prompt overflow.
- Index status uses upsert (`INSERT … ON CONFLICT`) — a fresh DB gets a baseline
  row on first index.
- `index_all` is unit-tested (`test_index_all_populates_knowledge_base` in
  `src/rag_indexers.rs`): every source (`catalog`/`feature`/`i18n`) produces
  documents, chunks link to documents, and re-indexing upserts without
  duplicating documents. The indexers take an `EmbeddingProvider`
  (`src/rag.rs`): production passes `OllamaEmbedder`, the tests inject a
  deterministic fake — so reconcile, the fail-loud contract and search are all
  covered without a live Ollama (`rag.rs` unit tests,
  `rag_indexers.rs` unit tests, `tests/rag_integration.rs`).
- **Guide coverage is enforced by a test, not by this prose.**
  `test_every_guide_has_a_feature_doc` (`src/rag_indexers.rs`) enumerates
  `docs/guides/*.md` and requires each one — except the `DEVELOPER_ONLY_GUIDES`
  list, currently just `design-system.md` — to be claimed by exactly one entry
  of `FEATURE_GUIDES`, which is itself asserted to match `feature_docs()`
  id-for-id. Adding a guide without indexing it fails `cargo test`.
  **Why it exists:** the rule used to be prose only, and by 2026-10-12 **25 of
  the 45 indexable guides** (46 files minus `design-system.md`) had never been
  indexed — the assistant answered "I don't know"
  for shipped features (Syncthing, downloader, notifications, the desktop shell,
  LUKS encryption, RAID, the cache tier, widgets…).
- `test_feature_docs_are_unique_and_substantive` rejects a duplicate id, a stub
  body (< 200 chars), and any implementation detail leaking into the text
  (`src/`, `.svelte`, backticks): these entries describe the product to a user,
  not the checkout to a maintainer.
- **Indexers never read app content from disk** (single-binary invariant):
  `index_catalog` uses the embedded `catalog/`, `index_features` is a `vec!`
  of literals, `index_i18n` uses `i18n::embedded_translation_sources()`.
  Reading `src/i18n/*.toml` off disk — which `index_i18n` did until
  2026-09-23 — silently indexed nothing on a deployed host, where the
  process runs from `/opt/homevault` with no source tree. Test it from a
  directory other than the repo root if you ever touch these.
- Background indexing via `tokio::spawn` — the POST returns immediately, status
  polls via GET.
- RAG is young and still evolving. Keep this guide in sync when
  adding sources, changing chunk size, or modifying the embedding model.

## Related

- [AI Assistant (Chat)](./chat.md), [API Contract](../API.md), [Backend Structure](../BACKEND.md)

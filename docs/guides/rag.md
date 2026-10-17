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
| `GET /rag/status` | auth | index status (last indexed, doc/chunk count, embedding model) |
| `POST /rag/index` | admin | triggers full reindex in background (`tokio::spawn`) |
| `POST /rag/search` | admin | semantic search test — returns top-K results + context string |
| `GET /rag/documents` | admin | list all indexed documents |

## Integration with Chat

`src/handlers/chat.rs` calls `system_prompt_with_rag()` instead of `system_prompt()`.
The function embeds the user's query, searches for similar chunks, and appends the
context block to the system prompt. Falls back to plain prompt on embedding failure.

## Document Sources

| Source | ID prefix | Content |
|--------|-----------|---------|
| App catalog | `catalog:<app_id>` | Manifest fields: name, description (4 langs), category, developer, website, port, GPU, arch |
| Feature docs | `feature:<name>` | **50** feature docs — **one per guide in `docs/guides/`**, minus `design-system.md` and `development.md` (developer-only, deliberately excluded). The ids are not always the guide's filename (`backup` ← `backup-restic.md`, `virtual-machines` ← `vm.md`, `setup` ← `setup-wizard.md`, `cloudsync` ← `cloud-sync.md`, `scheduled-tasks`/`metrics`/`power` match their file names). Full list, and the id → guide mapping that is asserted in both directions: `src/rag_indexers.rs::tests::FEATURE_GUIDES`. |
| i18n translations | `i18n:<lang>:<section>` | All TOML keys grouped by section, per language (en/fr/de/es), read from `i18n::embedded_translation_sources()` — the bundles compiled into the binary |

## Frontend

- `frontend/src/lib/services/api.ts`: `ragStatus`, `ragIndex`, `ragSearch`, `ragDocuments`.

## Invariants

- Embedding model is hardcoded to `nomic-embed-text` — the admin must pull it via
  Settings → AI before indexing.
- `chunk_text` splits on word boundaries at ~400 tokens with 80-token overlap.
- `cosine_similarity` returns 0.0 for mismatched lengths or zero norms.
- Embedding vectors are serialized as f32 little-endian BLOBs in SQLite.
- `build_rag_context` caps output at 3000 chars to avoid prompt overflow.
- Index status uses upsert (`INSERT … ON CONFLICT`) — a fresh DB gets a baseline
  row on first index.
- `index_all` is unit-tested (`test_index_all_populates_knowledge_base` in
  `src/rag_indexers.rs`): every source (`catalog`/`feature`/`i18n`) produces
  documents, chunks link to documents, and re-indexing upserts without
  duplicating documents.
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

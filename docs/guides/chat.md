# AI Assistant (Chat)

**Purpose:** Ollama-backed chat and model management (plus the AI model config card
on Settings). Integrated with the RAG knowledge base.

## Backend

Handlers in `src/handlers/chat.rs`; system prompt built per requested language.
When the RAG knowledge base is indexed, `system_prompt_with_rag()` enriches the
prompt with relevant context chunks before sending to Ollama.
Router entries in `src/web/mod.rs` under `/api/chat`.

| Endpoint | Auth | Notes |
|----------|------|-------|
| `POST /chat/send` | auth | chat with local Ollama model (RAG-enriched prompt) |
| `POST /chat/send/stream` | auth | streaming chat (SSE, RAG-enriched prompt) |
| `GET /chat/models` | auth | list installed models |
| `POST /chat/models/pull` | admin | pull a model |
| `GET /chat/models/recommend` | ⚠️ **none** | recommendations — `handle_chat_models_recommend()` takes no `HeaderMap`/session parameter at all, unlike every other handler here. Real, unfixed gap; see Invariants below. |
| `GET /chat/model` | auth | current model |
| `PUT /chat/model` | admin | set current model |

See [RAG Knowledge Base](./rag.md) for the RAG endpoints (`/api/rag/*`).

## Frontend

- `frontend/src/routes/Chat.svelte`.
- Settings AI card: `frontend/src/routes/Settings.svelte`
  (`loadAiModels`, `loadRecommendations`, `handleInstallAndLoad`,
  `handlePullModel`). Since 2026-09-15 the model `<select>` stages
  `draft.ai.model` and the Settings Save button activates it via
  `chatSetModel` (no more auto-apply on `change`); install/pull remain
  immediate actions.
- `frontend/src/routes/Desktop.svelte`: `chat` entry in the `apps` list.
- `frontend/src/lib/services/api.ts`: `chatSend`, `chatSendStream`, `chatListModels`,
  `chatPullModel`, `chatGetModel`, `chatSetModel`, `chatRecommendModels`.

## Invariants

- `handle_chat_send`, `handle_chat_models_list`, `handle_chat_model_get` require a
  session (a past bug had **no** check, allowing unauthenticated chat/model
  enumeration). Do not revert to "no check".
- **Known, unfixed gap found during a 2026-07-19 docs audit:**
  `handle_chat_models_recommend` (`GET /chat/models/recommend`) still has no
  session check at all — it takes zero parameters (`fn
  handle_chat_models_recommend()`), unlike every sibling handler in this file.
  Low severity (it only returns model recommendations derived from detected
  local hardware specs, no user data), but the same bug class documented as
  fixed elsewhere in this file. Not fixed here — reported to the user instead
  of edited directly.
- The system prompt is built per the request `language` (defaults to English); a
  reply to `language: "de"` is German. A past bug hardcoded French.
- `system_prompt_with_rag()` embeds the user's query and searches the RAG knowledge
  base for relevant context. Falls back to plain `system_prompt()` on embedding
  failure — the chat must never break because RAG is unavailable.
- The chat page and Settings AI card must use the `chat.*` / `settings.ai_*`
  translation keys in all four locales.
- `handle_chat_models_pull` streams Ollama's `POST /api/pull` NDJSON
  (`stream: true`), emitting a **determinate** `ollama_pull` `PROGRESS_MANAGER`
  job (`completed`/`total` → `%`) for the desktop [activity taskbar](./activity.md).
  It still answers the HTTP request synchronously with the same
  `{ success, message }` envelope once the pull finishes.
- **Editable since 2026-09-12** — `CLAUDE.md`'s frozen-area registry is gone, so
  `Chat.svelte`, the Settings AI card, the `chat` app entry, the `chat*` api.ts
  methods and `src/handlers/chat.rs` are open to edits. The invariants above
  stay binding: a change keeps the Rust integration + Playwright E2E suites
  green.

## Deployment (Ollama provisioning)

`installer/install.sh` provisions Ollama through its **official installer**
(`https://ollama.com/install.sh` — not an apt package), then enables the
`ollama` unit and polls `http://127.0.0.1:11434/` until the API answers; the
final verification step re-checks all three (binary + active unit + API on
`:11434`), because a present binary with a dead daemon still leaves Chat
offline. **No model is pulled at install time** — models are the user's
choice, downloaded from Settings → AI (`POST /chat/models/pull`), which
recommends ones fitting the host's RAM/GPU. Everything is best-effort:
without Ollama the app shows the "AI offline" state instead of failing the
install. Locked by
`dependencies.rs::tests::test_install_sh_installs_ollama_for_the_chat_app`
(official script + unit enable + `:11434` probe present, `ollama pull`
absent).

## Related

- [RAG Knowledge Base](./rag.md), [Settings](./settings.md), [API Contract](../API.md)

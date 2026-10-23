# AI Assistant (Chat)

**Purpose:** the local AI assistant — an Ollama-backed chat that answers questions
about HomeVault itself (RAG) and manages models from Settings. Nothing leaves the
machine: no provider, no API key, no cloud account. The assistant is also how an
operator asks "how does this feature work?" without leaving the appliance, while
an external agent drives the appliance through the [MCP server](./mcp.md).

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
| `GET /chat/models/recommend` | auth | five models ranked for this host's RAM/GPU (`recommend_models`) |
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
- The external-agent (MCP) gateway is **not** configured here: its card lives
  in Settings → AI Assistant (`frontend/src/lib/components/settings/McpCard.svelte`).
  It used to be a side panel of this screen (`components/chat/McpPanel.svelte`,
  deleted 2026-10-19) — the gateway is server-wide, admin-only configuration,
  so it belongs next to the model card. Full contract in
  [External Agents](./mcp.md).

## Invariants

- `handle_chat_send`, `handle_chat_models_list`, `handle_chat_model_get` **and**
  `handle_chat_models_recommend` require a session (a past bug had **no** check
  on the first three, allowing unauthenticated chat/model enumeration). Do not
  revert to "no check".
  A 2026-07-19 docs audit recorded `GET /chat/models/recommend` as still
  unguarded (it took zero parameters at the time); that note was stale — the
  handler now takes `State` + `HeaderMap` and rejects an empty/unknown session
  with an empty `recommendations` payload before touching the hardware probes
  (`src/handlers/chat.rs`).
- The system prompt is built per the request `language` (defaults to English); a
  reply to `language: "de"` is German. A past bug hardcoded French.
- `system_prompt_with_rag()` embeds the user's query and searches the RAG knowledge
  base for relevant context. Falls back to plain `system_prompt()` on embedding
  failure — the chat must never break because RAG is unavailable — **and logs
  the failure**, so a broken index is not mistaken for an empty one.
- When the knowledge base has zero documents, the Chat empty state shows a
  notice (and, for an admin, an "index now" action) instead of silently
  answering "I don't know" for shipped features; the status card lives in
  Settings → AI Assistant ([RAG](./rag.md)).
- The chat page and Settings AI card must use the `chat.*` / `settings.ai_*`
  translation keys in all four locales.
- Server-side chat messages — the `/chat/*` error strings and the pull
  success message — are **English**; the UI owns the localised copy. They used
  to be French-only (`src/handlers/chat.rs`), which leaked French into a German
  or Spanish session.
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

## Model selection (five models, ranked for the host)

`GET /chat/models/recommend` returns exactly **five** entries from
`MODEL_CATALOG` (`src/handlers/chat.rs`), ranked by `fit_for()`:

| Rank | Rule |
|------|------|
| 1 | GPU fit first — the whole model fits in VRAM (Ollama keeps it resident) |
| 2 | then the largest model that fits in system RAM, then progressively smaller/faster |
| 3 | when fewer than five fit, the list is completed with the next smallest models, flagged `fits: false` and shown with a "needs N GB" chip — never labelled "best choice" |

The catalog only contains names that exist in the public Ollama library: a name
that does not resolve fails at `ollama pull`, in front of the operator. Current
entries span 0.4 GB (`qwen3:0.6b`, 1 GB RAM floor) to 5.2 GB (`qwen3:8b`,
12 GB) so every supported machine gets a meaningful ladder. Locked by
`handlers::chat::tests` (`…_returns_five_real_catalog_entries`,
`…_never_puts_a_model_that_does_not_fit_first`,
`…_always_returns_five_even_on_a_tiny_host`, `…_prefers_vram_fit`,
`catalog_names_are_unique_and_sized`).

**Known gap:** each entry's `description` and `speed` are still French-only
strings carried by `MODEL_CATALOG`; the card renders them as-is, so a non-French
operator reads French copy in the recommendation list. The names, RAM/VRAM
floors and sizes — the actionable parts — are language-neutral.

The embedding model used by the knowledge base is separate
(`nomic-embed-text`, see [RAG](./rag.md)) — it is not part of the five, and the
UI never offers it as a chat model.

## Why Ollama (runtime evaluation)

HomeVault needs a runtime that fits an appliance: one binary, no Python, model
download by name, streaming, and CPU inference on 1 GB. Evaluated 2026-10-19:

| Runtime | Verdict |
|---------|---------|
| **Ollama** | **chosen** — static Go binary (amd64/arm64), official installer + systemd unit, model registry with `pull`, streaming NDJSON, `/v1` compatibility, automatic CUDA/ROCm/Metal split, keeps the model resident |
| llama.cpp `llama-server` | single C++ binary and the only option on **armv7l**, but no registry: the operator must fetch a GGUF + tokenizer by hand, so the Settings card could not offer a list |
| LocalAI | Go, [OI]-compatible and multi-backend, but pulls a much heavier stack and more moving parts than an appliance warrants |
| vLLM | GPU-only, Python, large — incompatible with the 1 GB floor |
| llamafile / LM Studio / Jan | single-file or desktop GUI, no server-side model management |

Consequence in the installer: Ollama is provisioned on `x86_64` and `aarch64`
only. Ollama publishes **no 32-bit ARM build**, so on `armv7l` the install is
skipped with an explicit message instead of leaving a dead daemon that reads as
an app bug (`install_ollama()`'s arch guard, asserted by
`dependencies.rs::tests::test_install_sh_installs_ollama_for_the_chat_app`).
A future runtime swap stays possible without touching the handlers: the client
talks [OI]-compatible JSON, not Ollama-specific structs beyond the pull stream.

## Deployment (Ollama provisioning)

`installer/install.sh` provisions Ollama through its **official installer**
(`https://ollama.com/install.sh` — not an apt package), then enables the
`ollama` unit and polls `http://127.0.0.1:11434/` until the API answers; the
final verification step re-checks all three (binary + active unit + API on
`:11434`), because a present binary with a dead daemon still leaves Chat
offline. **`zstd` is installed first**: Ollama publishes
`ollama-linux-<arch>.tar.zst`, and its installer aborts with "This version
requires zstd for extraction" when the tool is missing — `zstd` is Priority:
optional, so a fresh Debian/Ubuntu does not have it and the whole step used
to degrade silently to "AI offline". **No model is pulled at install time** — models are the user's
choice, downloaded from Settings → AI (`POST /chat/models/pull`), which
recommends ones fitting the host's RAM/GPU. Everything is best-effort:
without Ollama the app shows the "AI offline" state instead of failing the
install. Locked by
`dependencies.rs::tests::test_install_sh_installs_ollama_for_the_chat_app`
(official script + unit enable + `:11434` probe present, `ollama pull`
absent).

## Related

- [RAG Knowledge Base](./rag.md), [Settings](./settings.md), [API Contract](../API.md)

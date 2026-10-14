# External Agents (MCP server)

**Purpose:** Let an external coding agent drive
HomeVault over the Model Context Protocol (Streamable HTTP), with granular
permissions and revocable tokens. A minimal external-agent gateway surface:
one master kill-switch, one global permission set shared by every token,
opaque bearer tokens. The HomeVault-only addition is the `knowledge` family,
which exposes the internal RAG base to agents.

## Backend

- Core (pure logic, unit-tested without a DB): `src/mcp.rs` — `McpPermissions`
  (`apps`/`files`/`vms` take `"all"` or a list, everything else is a bool;
  everything defaults to denied), token mint/parse/verify
  (`hv_mcp_<16 hex id>_<32 hex secret>`, SHA-256 of the secret half stored,
  constant-time compare), `tools_for_permissions()`, and the file-path guard
  (`clean_absolute_path` + `authorize_file_path` with `FILE_DENYLIST`).
- Storage: `src/storage/mcp_repo.rs::McpRepository` (`mcp_tokens` +
  singleton `mcp_settings`, migration `038_mcp_server.sql`). Only the secret
  hash is stored; the full token is returned exactly once at creation.
- HTTP: `src/handlers/mcp.rs`, routes in `src/web/mod.rs`:
  - Management (admin session): `GET /mcp/status` (state + permissions +
    token prefixes), `PUT /mcp/status` (`{enabled}` kill-switch — disabling
    keeps tokens/permissions), `PUT /mcp/permissions` (replaces the global
    set; bad shapes and relative file grants are 400, never widened),
    `POST /mcp/tokens` (`{name}`, cap `MAX_TOKENS = 20`), `DELETE
    /mcp/tokens/{id}` (revoke; unknown ids are 400).
  - Agent endpoint: `POST /mcp` (no session — `Authorization: Bearer
    hv_mcp_…`). JSON-RPC with `initialize` (handshakes `2024-11-05`),
    `notifications/initialized` (202, no body), `ping`, `tools/list`
    (filtered from the permission snapshot), `tools/call`. Disabled server,
    unknown token and revoked token all answer the same 401 with
    `WWW-Authenticate: Bearer`; unknown methods are `-32601`, bad JSON
    `-32700`, and a call to a tool outside the snapshot is a denial
    (`-32003`), not a method error.

## Tools

| Tool | Gate |
|------|------|
| `system.get_info`, `system.get_resources` | always on (`/proc`-based, no new dependency) |
| `catalog.search`, `catalog.get`, `apps.list` | any app access (`apps` scope or `app_store`) |
| `apps.get_status`, `apps.logs`, `apps.start/stop/restart`, `apps.uninstall` | `apps` scope (`all` or bare id / install key) |
| `apps.install` | `app_store` — shares the HTTP core (`docker_catalog::install_catalog_app`, same compat/env/dependency gates) |
| `files.list_directory` | `files` scope (`all` minus the system denylist, or explicit absolute prefixes; symlinks canonicalized and re-checked) |
| `vms.list`, `vms.start/stop/restart` | `vms` scope (`all` or VM names, via `IncusService`) |
| `knowledge.search` | `rag` — embeds the query and searches the RAG chunks (top 5) |
| `system.reboot` | `manage_system` — same delayed `shutdown -r now` shape as the power menu |

File content is never embedded in tool results beyond a directory listing.
Every mutation is event-logged with the calling token id
(`MCP token <id> …`), and `last_used_at` is touched per call.

## Frontend

External-agent management lives in the Chat screen (`frontend/src/routes/Chat.svelte`,
admin-only "External agents" section): enable switch, permission editors
(app allow-list, App Store toggle, file grants, VM allow-list, knowledge-base
and reboot toggles), token list (prefix only) with create (full value shown
once with a copy button) and revoke. API client: `frontend/src/lib/services/api.ts`
(`mcpStatus`, `mcpSetEnabled`, `mcpSetPermissions`, `mcpCreateToken`,
`mcpRevokeToken`).

## Invariants

- Secure by default: fresh installs are disabled with an empty permission
  set; adding a tool family means a gate in `tools_for_permissions()` **and**
  a re-check inside `call_tool` (a stale client must never reach a tool
  through a cached list).
- Secrets never leak: no full token in logs, errors, the event log or the
  token list; verification is constant-time over the hash.
- The file scope never widens silently: `authorize_file_path` denies on
  unknown shapes, `..` escapes and non-absolute input, and the handler
  re-checks after `canonicalize`.
- RAG rule: this guide is indexed (`FeatureDoc` id `mcp` in
  `src/rag_indexers.rs`, row in `FEATURE_GUIDES`).

## Related

- [Chat](./chat.md), [Knowledge Base](./rag.md), [Docker](./docker.md),
  [VM](./vm.md)

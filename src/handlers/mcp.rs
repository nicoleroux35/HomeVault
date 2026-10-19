/* =============================================================================
 * File          : mcp.rs
 * Author        : Nicolas Le Roux
 * Created       : 13/10/2026
 * Last modified : 13/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   HTTP surface of the external AI-agent (MCP) server: the admin
 *   management API under /api/mcp (master kill-switch, global permissions,
 *   token mint/revoke — tokens stored hashed) and the agent endpoint
 *   POST /mcp, which speaks MCP Streamable HTTP authenticated by a bearer
 *   token instead of a session. The tool set served to an agent is
 *   filtered by the permission snapshot, and every mutation is
 *   event-logged with the calling token id.
 *
 * Dependencies:
 *   - axum (>= 0.8)
 *   - serde (>= 1.0)
 *   - serde_json (>= 1.0)
 *
 * Usage:
 *   Routes /api/mcp (management, admin) and /mcp (agent endpoint, bearer
 *   token) in src/web/mod.rs; see docs/guides/mcp.md.
 * =============================================================================
 */

//! HTTP surface of the external AI-agent (MCP) server.
//!
//! Two halves: the **management API** under `/api/mcp/*` (admin session,
//! `x-session-token`) to enable the server, set the global permissions and
//! mint/revoke tokens; and the **agent endpoint** `POST /mcp` (no session —
//! `Authorization: Bearer hv_mcp_…`), which speaks MCP Streamable HTTP
//! (`initialize`, `tools/list`, `tools/call`) with the tool set filtered
//! from the permission snapshot. File content is never embedded in tool
//! results beyond a directory listing; every mutation is event-logged with
//! the calling token id.

use crate::handlers::admin_guard::require_admin;
use crate::handlers::docker_catalog;
use crate::handlers::AppState;
use crate::mcp;
use crate::mcp::{McpPermissions, PROTOCOL_VERSION, TOKEN_PREFIX};
use crate::storage::{AppStoreRepository, McpRepository};
use crate::utils::ResponseHelper;
use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

// ── Management API ────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
struct McpStatusView {
    enabled: bool,
    permissions: McpPermissions,
    endpoint: &'static str,
    protocol: &'static str,
}

/// `GET /api/mcp/status` — kill-switch state, global permissions, tokens
/// (hashes only). Admin-only: the token list is sensitive.
pub async fn handle_mcp_status(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    if let Err(response) = require_admin(&state, &headers).await {
        return response;
    }
    let repo = McpRepository::new(state.db.pool());
    let (enabled, permissions_json) = repo
        .get_settings()
        .await
        .unwrap_or((false, "{}".to_string()));
    let tokens = repo.list().await.unwrap_or_default();
    ResponseHelper::json_success(serde_json::json!({
        "status": McpStatusView {
            enabled,
            permissions: McpPermissions::parse(&permissions_json),
            endpoint: "/mcp",
            protocol: "Streamable HTTP",
        },
        "tokens": tokens,
    }))
}

#[derive(Debug, Deserialize)]
pub struct McpEnabledRequest {
    pub enabled: bool,
}

/// `PUT /api/mcp/status` — flip the master kill-switch. Disabling keeps
/// tokens and permissions survive; every live agent call
/// then gets a 401 until re-enabled.
pub async fn handle_mcp_set_enabled(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<McpEnabledRequest>,
) -> Response {
    let admin = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(response) => return response,
    };
    let repo = McpRepository::new(state.db.pool());
    if let Err(e) = repo.set_enabled(req.enabled).await {
        return ResponseHelper::json_error(format!("Failed to update MCP state: {}", e));
    }
    crate::events::log_event(format!(
        "Admin '{}' {} the external agent (MCP) server",
        admin,
        if req.enabled { "enabled" } else { "disabled" }
    ));
    ResponseHelper::json_success(serde_json::json!({ "enabled": req.enabled }))
}

/// `PUT /api/mcp/permissions` — replace the global permission set. The body
/// is the `McpPermissions` shape (`apps`/`files`/`vms` take `"all"` or a
/// list; everything defaults to denied). Unknown shapes are a 400, never
/// silently widened.
pub async fn handle_mcp_set_permissions(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(value): Json<serde_json::Value>,
) -> Response {
    let admin = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(response) => return response,
    };
    let permissions: McpPermissions = match serde_json::from_value(value.clone()) {
        Ok(p) => p,
        Err(e) => {
            return ResponseHelper::json_error(format!("Invalid permissions: {}", e));
        }
    };
    // Grants must be absolute paths; anything else is a typo worth a 400.
    if let mcp::ScopeList::Ids(grants) = &permissions.files {
        for grant in grants {
            if let Err(e) = mcp::clean_absolute_path(grant) {
                return ResponseHelper::json_error(format!("Invalid file grant {grant:?}: {e}"));
            }
        }
    }
    let json = serde_json::to_string(&permissions).unwrap_or_else(|_| "{}".to_string());
    let repo = McpRepository::new(state.db.pool());
    if let Err(e) = repo.set_permissions(&json).await {
        return ResponseHelper::json_error(format!("Failed to save permissions: {}", e));
    }
    crate::events::log_event(format!(
        "Admin '{}' updated the external agent (MCP) permissions",
        admin
    ));
    ResponseHelper::json_success(permissions)
}

#[derive(Debug, Deserialize)]
pub struct McpCreateTokenRequest {
    pub name: String,
}

/// `POST /api/mcp/tokens` — mint a token. The full value is returned
/// exactly once (`token`); afterwards only the prefix identifies it.
pub async fn handle_mcp_create_token(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<McpCreateTokenRequest>,
) -> Response {
    let admin = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(response) => return response,
    };
    let name = req.name.trim().to_string();
    if name.is_empty() || name.len() > 64 {
        return ResponseHelper::json_error("Token name must be 1-64 characters".to_string());
    }
    let repo = McpRepository::new(state.db.pool());
    match repo.count().await {
        Ok(n) if n >= mcp::MAX_TOKENS => {
            return ResponseHelper::json_error(format!(
                "Token limit reached ({}) — revoke one first",
                mcp::MAX_TOKENS
            ));
        }
        Err(e) => return ResponseHelper::json_error(format!("Failed to count tokens: {}", e)),
        _ => {}
    }
    let (full, id, hash, prefix) = mcp::mint_token();
    if let Err(e) = repo.insert(&id, &name, &hash, &prefix, &admin).await {
        return ResponseHelper::json_error(format!("Failed to store token: {}", e));
    }
    crate::events::log_event(format!(
        "Admin '{}' issued external agent (MCP) token '{}'",
        admin, name
    ));
    ResponseHelper::json_success(serde_json::json!({
        "id": id,
        "name": name,
        "prefix": prefix,
        "token": full,
        "note": "Copy this token now — it is never shown again.",
    }))
}

/// `DELETE /api/mcp/tokens/{id}` — revoke. Unknown ids are a 400 (same
/// "already gone" wording as other delete endpoints).
pub async fn handle_mcp_revoke_token(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Response {
    let admin = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(response) => return response,
    };
    if id.len() != 16 || !id.chars().all(|c| c.is_ascii_hexdigit()) {
        return ResponseHelper::json_error("Unknown token".to_string());
    }
    let repo = McpRepository::new(state.db.pool());
    match repo.delete(&id).await {
        Ok(true) => {
            crate::events::log_event(format!(
                "Admin '{}' revoked external agent (MCP) token '{}{}…'",
                admin,
                TOKEN_PREFIX,
                &id[..8.min(id.len())]
            ));
            ResponseHelper::json_success("Token revoked".to_string())
        }
        Ok(false) => ResponseHelper::json_error("Unknown token".to_string()),
        Err(e) => ResponseHelper::json_error(format!("Failed to revoke token: {}", e)),
    }
}

// ── Agent endpoint (`POST /mcp`) ──────────────────────────────────────────

// `Response` is axum's canonical error type; boxing it would ripple
// through every call site for no real benefit, so allow the lint here.
#[allow(clippy::result_large_err)]
async fn agent_context(
    state: &Arc<AppState>,
    headers: &HeaderMap,
) -> Result<(McpPermissions, String), Response> {
    let presented = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer "))
        .unwrap_or("");
    // Indistinguishable 401 for disabled/invalid/revoked.
    let unauthorized = || {
        (
            StatusCode::UNAUTHORIZED,
            [(axum::http::header::WWW_AUTHENTICATE, "Bearer")],
            Json(serde_json::json!({
                "jsonrpc": "2.0",
                "id": null,
                "error": { "code": -32001, "message": "Unauthorized" },
            })),
        )
            .into_response()
    };
    let repo = McpRepository::new(state.db.pool());
    let (enabled, permissions_json) = repo.get_settings().await.map_err(|_| unauthorized())?;
    if !enabled {
        return Err(unauthorized());
    }
    let (token_id, _) = mcp::parse_token(presented).ok_or_else(unauthorized)?;
    let stored = repo
        .secret_hash(&token_id)
        .await
        .map_err(|_| unauthorized())?
        .ok_or_else(unauthorized)?;
    let verified = mcp::verify_token(presented, &stored).ok_or_else(unauthorized)?;
    let _ = repo.touch_last_used(&verified).await;
    Ok((McpPermissions::parse(&permissions_json), verified))
}

#[derive(Debug, Deserialize)]
struct JsonRpcRequest {
    #[allow(dead_code)]
    jsonrpc: Option<String>,
    id: Option<serde_json::Value>,
    method: String,
    params: Option<serde_json::Value>,
}

fn rpc_result(id: Option<serde_json::Value>, result: serde_json::Value) -> Response {
    Json(serde_json::json!({ "jsonrpc": "2.0", "id": id, "result": result })).into_response()
}

fn rpc_error(
    id: Option<serde_json::Value>,
    code: i64,
    message: String,
    data: Option<serde_json::Value>,
) -> Response {
    let mut error = serde_json::json!({ "code": code, "message": message });
    if let Some(d) = data {
        error["data"] = d;
    }
    Json(serde_json::json!({ "jsonrpc": "2.0", "id": id, "error": error })).into_response()
}

fn tool_denied(id: Option<serde_json::Value>, tool: &str) -> Response {
    rpc_error(
        id,
        -32003,
        format!("{tool} is outside this token's permissions (Settings → AI Assistant → External agents)"),
        None,
    )
}

/// `POST /mcp` — MCP Streamable HTTP (single-response): `initialize`,
/// `tools/list`, `tools/call`, `ping`. Bearer-authenticated; see
/// `agent_context` for the 401 shape.
pub async fn handle_mcp_rpc(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: String,
) -> Response {
    let (perms, token_id) = match agent_context(&state, &headers).await {
        Ok(ctx) => ctx,
        Err(response) => return response,
    };
    let req: JsonRpcRequest = match serde_json::from_str(&body) {
        Ok(r) => r,
        Err(_) => return rpc_error(None, -32700, "Parse error".to_string(), None),
    };
    match req.method.as_str() {
        "initialize" => rpc_result(
            req.id,
            serde_json::json!({
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": { "tools": {} },
                "serverInfo": {
                    "name": "homevault",
                    "version": env!("CARGO_PKG_VERSION"),
                    "instructions": "HomeVault appliance agent API. Mutating tools need their permission family; file paths must stay inside the granted scope.",
                },
            }),
        ),
        "notifications/initialized" => StatusCode::ACCEPTED.into_response(),
        "ping" => rpc_result(req.id, serde_json::json!({})),
        "tools/list" => {
            let tools = mcp::tools_for_permissions(&perms);
            rpc_result(req.id, serde_json::json!({ "tools": tools }))
        }
        "tools/call" => {
            let name = req
                .params
                .as_ref()
                .and_then(|p| p.get("name"))
                .and_then(|n| n.as_str())
                .unwrap_or("")
                .to_string();
            let args = req
                .params
                .as_ref()
                .and_then(|p| p.get("arguments"))
                .cloned()
                .unwrap_or(serde_json::json!({}));
            let rpc_id = req.id.clone();
            // The tool must be registered for this snapshot — a stale client
            // calling a tool outside its permissions gets a denial, not a
            // method error.
            if !mcp::tools_for_permissions(&perms)
                .iter()
                .any(|t| t.name == name)
            {
                return tool_denied(rpc_id, &name);
            }
            call_tool(&state, &perms, &token_id, &name, &args)
                .await
                .map(|result| {
                    rpc_result(
                        rpc_id.clone(),
                        serde_json::json!({
                            "content": [{ "type": "text", "text": result }],
                        }),
                    )
                })
                .unwrap_or_else(|e| rpc_error(rpc_id, -32000, e, None))
        }
        _ => rpc_error(
            req.id,
            -32601,
            format!("Unknown method '{}'", req.method),
            None,
        ),
    }
}

fn arg_str(args: &serde_json::Value, key: &str) -> Option<String> {
    args.get(key)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

async fn enabled_store_pairs(state: &Arc<AppState>) -> Vec<(String, String)> {
    AppStoreRepository::new(state.db.pool())
        .list()
        .await
        .unwrap_or_default()
        .into_iter()
        .filter(|s| s.enabled)
        .map(|s| (s.id, s.name))
        .collect()
}

fn resolve_app(
    stores: &[(String, String)],
    perms: &McpPermissions,
    input: &str,
) -> Result<crate::app_catalog::CatalogApp, String> {
    // Accept a namespaced id, a bare id, or an install key.
    let found = crate::app_catalog::find(stores, input).or_else(|| {
        crate::app_catalog::list_apps(stores)
            .into_iter()
            .find(|a| a.install_key() == input || a.bare_id() == input)
    });
    let app = found.ok_or_else(|| format!("Unknown app {input:?}"))?;
    if !perms.can_access_app(app.bare_id(), &app.install_key()) {
        return Err(format!(
            "App '{}' is outside this token's app scope",
            app.bare_id()
        ));
    }
    Ok(app)
}

fn host_name() -> String {
    std::fs::read_to_string("/proc/sys/kernel/hostname")
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| "homevault".to_string())
}

fn read_meminfo() -> (Option<u64>, Option<u64>) {
    let text = std::fs::read_to_string("/proc/meminfo").unwrap_or_default();
    let mut total = None;
    let mut available = None;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("MemTotal:") {
            total = rest.split_whitespace().next().and_then(|v| v.parse().ok());
        } else if let Some(rest) = line.strip_prefix("MemAvailable:") {
            available = rest.split_whitespace().next().and_then(|v| v.parse().ok());
        }
    }
    (total, available)
}

async fn call_tool(
    state: &Arc<AppState>,
    perms: &McpPermissions,
    token_id: &str,
    name: &str,
    args: &serde_json::Value,
) -> Result<String, String> {
    let actor = format!("MCP token {token_id}");
    match name {
        "system.get_info" => Ok(serde_json::to_string(&serde_json::json!({
            "name": host_name(),
            "homevault_version": env!("CARGO_PKG_VERSION"),
            "arch": std::env::consts::ARCH,
        }))
        .unwrap_or_default()),
        "system.get_resources" => {
            let (mem_total_kb, mem_available_kb) = read_meminfo();
            let load = std::fs::read_to_string("/proc/loadavg").unwrap_or_default();
            let uptime = std::fs::read_to_string("/proc/uptime")
                .unwrap_or_default()
                .split_whitespace()
                .next()
                .and_then(|v| v.parse::<f64>().ok())
                .unwrap_or(0.0);
            Ok(serde_json::to_string(&serde_json::json!({
                "mem_total_kb": mem_total_kb,
                "mem_available_kb": mem_available_kb,
                "loadavg": load.trim(),
                "uptime_seconds": uptime as u64,
            }))
            .unwrap_or_default())
        }
        "catalog.search" => {
            let query = arg_str(args, "query").unwrap_or_default().to_lowercase();
            let stores = enabled_store_pairs(state).await;
            let apps = tokio::task::spawn_blocking(move || crate::app_catalog::list_apps(&stores))
                .await
                .map_err(|e| format!("Task join error: {e}"))?;
            let hits: Vec<_> = apps
                .iter()
                .filter(|a| {
                    query.is_empty()
                        || a.manifest.name.to_lowercase().contains(&query)
                        || a.manifest.description.to_lowercase().contains(&query)
                        || a.manifest.category.to_lowercase().contains(&query)
                })
                .map(|a| {
                    serde_json::json!({
                        "id": a.namespaced_id(),
                        "name": a.manifest.name,
                        "category": a.manifest.category,
                        "version": a.manifest.version,
                        "description": a.manifest.description,
                        "store": a.store_name,
                    })
                })
                .collect();
            serde_json::to_string(&hits).map_err(|e| e.to_string())
        }
        "catalog.get" => {
            let id = arg_str(args, "id").ok_or("missing argument 'id'")?;
            let stores = enabled_store_pairs(state).await;
            let hit = tokio::task::spawn_blocking(move || crate::app_catalog::find(&stores, &id))
                .await
                .map_err(|e| format!("Task join error: {e}"))?
                .ok_or_else(|| "Unknown catalog app".to_string())?;
            let installed = crate::compose::ComposeManager::get_project(&hit.install_key()).is_ok();
            serde_json::to_string(&serde_json::json!({
                "id": hit.namespaced_id(),
                "name": hit.manifest.name,
                "version": hit.manifest.version,
                "category": hit.manifest.category,
                "description": hit.manifest.description,
                "developer": hit.manifest.developer,
                "website": hit.manifest.website,
                "release_notes": hit.manifest.release_notes,
                "dependencies": hit.manifest.dependencies,
                "implements": hit.manifest.implements,
                "installed": installed,
            }))
            .map_err(|e| e.to_string())
        }
        "apps.list" => {
            let stores = enabled_store_pairs(state).await;
            let snapshot = perms.clone();
            let rows = tokio::task::spawn_blocking(move || {
                crate::app_catalog::list_apps(&stores)
                    .into_iter()
                    .filter(|a| {
                        crate::compose::ComposeManager::get_project(&a.install_key()).is_ok()
                            && snapshot.can_access_app(a.bare_id(), &a.install_key())
                    })
                    .map(|a| {
                        let running = crate::compose::ComposeManager::get_project(&a.install_key())
                            .map(|p| p.status == "running")
                            .unwrap_or(false);
                        serde_json::json!({
                            "id": a.namespaced_id(),
                            "name": a.manifest.name,
                            "running": running,
                        })
                    })
                    .collect::<Vec<_>>()
            })
            .await
            .map_err(|e| format!("Task join error: {e}"))?;
            serde_json::to_string(&rows).map_err(|e| e.to_string())
        }
        "apps.get_status" | "apps.logs" | "apps.start" | "apps.stop" | "apps.restart"
        | "apps.uninstall" => {
            let input = arg_str(args, "app").ok_or("missing argument 'app'")?;
            let stores = enabled_store_pairs(state).await;
            let owned_perms = perms.clone();
            let app =
                tokio::task::spawn_blocking(move || resolve_app(&stores, &owned_perms, &input))
                    .await
                    .map_err(|e| format!("Task join error: {e}"))??;
            let key = app.install_key();
            let label = app.manifest.name.clone();
            match name {
                "apps.get_status" => {
                    let running = crate::compose::ComposeManager::get_project(&key)
                        .map(|p| p.status == "running")
                        .unwrap_or(false);
                    let installed = crate::compose::ComposeManager::get_project(&key).is_ok();
                    Ok(format!(
                        "{{\"app\": \"{label}\", \"installed\": {installed}, \"running\": {running}}}"
                    ))
                }
                "apps.logs" => {
                    if crate::compose::ComposeManager::get_project(&key).is_err() {
                        return Err(format!("{label} is not installed"));
                    }
                    let tail = args
                        .get("tail")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(200)
                        .clamp(1, 2000);
                    let out = tokio::task::spawn_blocking(move || {
                        std::process::Command::new("docker")
                            .args([
                                "compose",
                                "-p",
                                &key,
                                "logs",
                                "--no-color",
                                "--tail",
                                &tail.to_string(),
                            ])
                            .output()
                            .map_err(|e| format!("failed to run docker compose logs: {e}"))
                            .and_then(|o| {
                                if o.status.success() {
                                    Ok(String::from_utf8_lossy(&o.stdout).to_string())
                                } else {
                                    Err(format!(
                                        "docker compose logs: {}",
                                        String::from_utf8_lossy(&o.stderr).trim()
                                    ))
                                }
                            })
                    })
                    .await
                    .map_err(|e| format!("Task join error: {e}"))??;
                    Ok(out)
                }
                "apps.start" | "apps.stop" | "apps.restart" => {
                    let action = name.trim_start_matches("apps.").to_string();
                    let action_for_task = action.clone();
                    let result =
                        tokio::task::spawn_blocking(move || match action_for_task.as_str() {
                            "start" => crate::compose::ComposeManager::start_project(&key),
                            "stop" => crate::compose::ComposeManager::stop_project(&key),
                            _ => crate::compose::ComposeManager::restart_project(&key),
                        })
                        .await
                        .map_err(|e| format!("Task join error: {e}"))?;
                    match result {
                        Ok(()) => {
                            crate::events::log_event(format!(
                                "{} ran apps.{} on '{label}'",
                                actor, action
                            ));
                            Ok(format!("{label} {action}ed"))
                        }
                        Err(e) => Err(format!("Failed to {action} {label}: {e}")),
                    }
                }
                _ => {
                    // apps.uninstall shares the HTTP core (marker cleanup,
                    // proxy stop, event log) via the actor label.
                    docker_catalog::uninstall_catalog_app(state, &app.namespaced_id(), &actor).await
                }
            }
        }
        "apps.install" => {
            let id = arg_str(args, "id").ok_or("missing argument 'id'")?;
            let port = args.get("port").and_then(|v| v.as_u64()).map(|p| p as u16);
            let alternatives: std::collections::HashMap<String, String> = args
                .get("alternatives")
                .and_then(|v| v.as_object())
                .map(|o| {
                    o.iter()
                        .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                        .collect()
                })
                .unwrap_or_default();
            let environment: std::collections::HashMap<String, String> = args
                .get("environment")
                .and_then(|v| v.as_object())
                .map(|o| {
                    o.iter()
                        .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                        .collect()
                })
                .unwrap_or_default();
            let req = docker_catalog::InstallCatalogAppRequest {
                port,
                alternatives,
                environment,
            };
            docker_catalog::install_catalog_app(state, &id, &req, &actor).await
        }
        "files.list_directory" => {
            let requested = arg_str(args, "path").ok_or("missing argument 'path'")?;
            let cleaned = mcp::authorize_file_path(perms, &requested)?;
            let scoped = cleaned.clone();
            let owned_perms = perms.clone();
            let entries = tokio::task::spawn_blocking(move || {
                // Resolve symlinks and re-check containment: a link inside
                // the scope must not smuggle the listing outside it.
                let canonical = std::fs::canonicalize(&scoped)
                    .map_err(|e| format!("cannot read {scoped}: {e}"))?;
                let canonical_str = canonical.to_string_lossy().to_string();
                mcp::authorize_file_path(&owned_perms, &canonical_str)?;
                let mut rows = Vec::new();
                for entry in std::fs::read_dir(&canonical)
                    .map_err(|e| format!("cannot read {scoped}: {e}"))?
                {
                    let entry = entry.map_err(|e| e.to_string())?;
                    let file_type = entry.file_type().map_err(|e| e.to_string())?;
                    rows.push(serde_json::json!({
                        "name": entry.file_name().to_string_lossy(),
                        "is_dir": file_type.is_dir(),
                        "is_symlink": file_type.is_symlink(),
                    }));
                }
                rows.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
                Ok::<_, String>(rows)
            })
            .await
            .map_err(|e| format!("Task join error: {e}"))??;
            serde_json::to_string(&entries).map_err(|e| e.to_string())
        }
        "vms.list" => {
            let snapshot = perms.clone();
            let rows = crate::incus::IncusService::list_instances()
                .await
                .map_err(|e| format!("Failed to list VMs: {e}"))?
                .into_iter()
                .filter(|i| snapshot.can_access_vm(&i.name))
                .map(|i| {
                    serde_json::json!({
                        "name": i.name,
                        "status": i.status,
                        "type": i.instance_type,
                    })
                })
                .collect::<Vec<_>>();
            serde_json::to_string(&rows).map_err(|e| e.to_string())
        }
        "vms.start" | "vms.stop" | "vms.restart" => {
            let vm = arg_str(args, "name").ok_or("missing argument 'name'")?;
            if !perms.can_access_vm(&vm) {
                return Err(format!("VM '{vm}' is outside this token's VM scope"));
            }
            let action = name.trim_start_matches("vms.");
            let op = crate::incus::IncusService::set_state(&vm, action, false, Some(30))
                .await
                .map_err(|e| format!("Failed to {action} VM '{vm}': {e}"))?;
            let _ = crate::incus::IncusService::wait_for_operation(&op, 60).await;
            crate::events::log_event(format!("{} ran vms.{action} on '{vm}'", actor));
            Ok(format!("VM '{vm}' {action}ed"))
        }
        "knowledge.search" => {
            let query = arg_str(args, "query").ok_or("missing argument 'query'")?;
            if query.trim().is_empty() {
                return Err("query must not be empty".to_string());
            }
            let embeddings = crate::rag::generate_embeddings(&[query])
                .await
                .map_err(|e| format!("Embedding failed: {e}"))?;
            let embedding = embeddings.into_iter().next().ok_or("Embedding failed")?;
            let hits = crate::rag::search_similar(state.db.pool().as_ref(), &embedding, 5)
                .await
                .map_err(|e| format!("Search failed: {e}"))?;
            let passages: Vec<_> = hits
                .iter()
                .map(|h| {
                    serde_json::json!({
                        "title": h.title,
                        "passage": h.content,
                        "score": h.score,
                    })
                })
                .collect();
            serde_json::to_string(&passages).map_err(|e| e.to_string())
        }
        "system.reboot" => {
            crate::events::log_event(format!("{} initiated a system reboot", actor));
            let actor_clone = actor.clone();
            tokio::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                tracing::info!("Executing system reboot (requested by {})", actor_clone);
                let _ = tokio::process::Command::new("shutdown")
                    .args(["-r", "now"])
                    .output()
                    .await;
            });
            Ok("Server restart initiated".to_string())
        }
        _ => Err(format!("Unknown tool '{name}'")),
    }
}

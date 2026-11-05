/* =============================================================================
 * File          : openapi.rs
 * Author        : Nicolas Le Roux
 * Created       : 05/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   OpenAPI 3 document generated from the actual Axum route table. The route
 *   inventory is parsed from the same sources `create_app` is assembled from,
 *   so a route cannot be added, renamed or removed without showing up (or
 *   disappearing) in `/api/openapi.json`.
 *
 * Dependencies:
 *   - serde_json (>= 1.0)
 *
 * Usage:
 *   handlers/openapi.rs serves `GET /api/openapi.json` (admin only);
 *   `spec()` returns the cached document. See docs/API.md.
 * =============================================================================
 */

//! Generated OpenAPI 3 description of the JSON HTTP API.
//!
//! The document is intentionally built from the **source of the router** rather
//! than a second hand-maintained table: `src/web/mod.rs` plus the sub-routers
//! nested under `/api/network`, `/api/vpn`, `/api/rescue`, `/api/terminal` and
//! `/api/vm`. A route added to the router appears here on the next build; one
//! removed disappears. That is the only way a machine-readable contract stays
//! true in a tree with a few hundred endpoints.
//!
//! Only JSON endpoints are published. Transports that carry a one-shot ticket,
//! a WebSocket, an SSE stream or a binary body are excluded (see
//! [`EXCLUDED_TRANSPORTS`]) and listed in the document's
//! `x-homevault-excluded` extension instead of being silently dropped.
//!
//! The endpoint itself is administrator-only, so the document is not a public
//! map of the appliance.

use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::OnceLock;

use crate::api_contract::CONTRACT_VERSION;

/// The OpenAPI minor version emitted by this module.
pub const OPENAPI_VERSION: &str = "3.0.3";

/// Methods the router registers; used by the parser and the CSRF decision.
const METHODS: &[&str] = &["get", "post", "put", "delete", "patch", "head", "options"];

/// Public endpoints: they must not inherit the global session requirement.
///
/// This list mirrors the `public` rows of `docs/API.md`. Authentication flows
/// are public by necessity (a locked-out user has no session yet); the setup
/// wizard is public but additionally confined to the local network by
/// `middleware::setup_guard`.
const PUBLIC_ROUTES: &[(&str, &str)] = &[
    ("POST", "/api/auth/login"),
    ("POST", "/api/auth/2fa/verify"),
    ("POST", "/api/auth/verify-recovery-code"),
    ("POST", "/api/auth/reset-password-recovery"),
    ("POST", "/api/auth/recover"),
    ("POST", "/api/auth/recover/verify"),
    ("GET", "/api/auth/setup-status"),
    ("GET", "/api/auth/check-username"),
    ("POST", "/api/auth/setup"),
    ("POST", "/api/auth/setup/verify-totp"),
    ("GET", "/api/auth/setup/progress"),
    ("PUT", "/api/auth/setup/progress"),
    ("GET", "/api/auth/methods"),
    ("POST", "/api/auth/oidc/start"),
    ("GET", "/api/auth/oidc/callback"),
    ("POST", "/api/auth/oidc/complete"),
    ("POST", "/api/auth/saml/start"),
    ("POST", "/api/auth/saml/acs"),
    ("POST", "/api/auth/saml/complete"),
    ("GET", "/api/auth/saml/metadata"),
    ("POST", "/api/passkeys/login/begin"),
    ("POST", "/api/passkeys/login/finish"),
    ("GET", "/api/settings/public"),
    ("GET", "/api/translations"),
    ("GET", "/api/meta"),
    ("GET", "/api/health/live"),
    ("GET", "/api/health/ready"),
];

/// Transport endpoints deliberately kept out of the JSON contract.
///
/// The reasons are part of the document (`x-homevault-excluded`) so a reader
/// can tell an omission from a missing endpoint.
const EXCLUDED_TRANSPORTS: &[(&str, &str)] = &[
    ("/api/terminal/", "native PTY over WebSocket"),
    ("/api/vm/console/ws", "interactive VM console WebSocket"),
    ("/api/vm/screen/ws", "VM screen WebSocket"),
    ("/api/events", "server-sent events stream"),
    ("/api/progress/", "server-sent events stream"),
    ("/api/explorer/watch", "server-sent events stream"),
    ("/api/chat/send/stream", "server-sent events stream"),
    ("/api/explorer/download", "binary file stream"),
    ("/api/explorer/thumbnail", "binary image stream"),
    ("/api/explorer/upload", "binary upload stream"),
    ("/api/explorer/upload/", "binary upload stream"),
    ("/api/explorer/preview", "binary preview stream"),
    ("/api/avatars/preview", "binary SVG preview"),
    ("/api/users/{username}/avatar.svg", "binary SVG avatar"),
];

/// One operation discovered in the router sources.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct ApiRoute {
    method: String,
    path: String,
    handler: String,
}

/// The cached OpenAPI document.
pub fn spec() -> &'static Value {
    static SPEC: OnceLock<Value> = OnceLock::new();
    SPEC.get_or_init(build_spec)
}

/// Build the document. Public for tests; production goes through [`spec`].
pub fn build_spec() -> Value {
    let routes = route_inventory();

    let mut paths: BTreeMap<String, Value> = BTreeMap::new();
    let mut tags: BTreeSet<String> = BTreeSet::new();

    for route in &routes {
        if exclusion_reason(&route.path).is_some() {
            continue;
        }
        let path = normalize_path(&route.path);
        let method = route.method.to_ascii_lowercase();
        let operation = build_operation(&route.method, &path);
        let entry = paths
            .entry(path.clone())
            .or_insert_with(|| Value::Object(Map::new()));
        if let Value::Object(map) = entry {
            map.insert(method, operation);
        }
        tags.insert(tag_for_path(&path).to_string());
    }

    let excluded: Vec<Value> = routes
        .iter()
        .filter_map(|route| {
            exclusion_reason(&route.path).map(|reason| {
                json!({
                    "method": route.method,
                    "path": route.path,
                    "reason": reason,
                })
            })
        })
        .collect();

    let mut components = Map::new();
    components.insert("securitySchemes".into(), security_schemes());
    components.insert("parameters".into(), parameter_components());
    components.insert("responses".into(), response_components());

    json!({
        "openapi": OPENAPI_VERSION,
        "info": {
            "title": "HomeVault API",
            "version": crate::api_contract::server_version(),
            "description": info_description(),
        },
        "servers": [
            { "url": "/", "description": "Same origin as the web interface" }
        ],
        "tags": tags.into_iter().map(|tag| json!({ "name": tag })).collect::<Vec<_>>(),
        "paths": paths,
        "components": Value::Object(components),
        "security": [{ "sessionToken": [] }],
        "x-homevault-contract-version": CONTRACT_VERSION,
        "x-homevault-generated-from": "src/web/mod.rs + nested routers",
        "x-homevault-excluded": excluded,
    })
}

fn info_description() -> String {
    format!(
        "Machine-readable OpenAPI {OPENAPI_VERSION} description of the HomeVault \
JSON API, generated from the Axum route table. The document is served to \
administrators only.\n\n\
Authentication: send `X-Session-Token` on every authenticated request and \
`X-CSRF-Token` on state-changing requests (`POST`, `PUT`, `PATCH`, `DELETE`) \
once signed in. The optional `X-HomeVault-Contract` header must equal the \
server's contract version when present; a mismatch is answered with `426`.\n\n\
Status contract: `400` is a malformed or refused request, `401` a missing or \
expired session, `403` a valid non-admin session on an admin operation, `426` \
a client/server contract mismatch, `501` a feature this build does not \
implement, and `503` a missing host capability (the message names the missing \
tool).\n\n\
Contract version: {CONTRACT_VERSION}."
    )
}

fn security_schemes() -> Value {
    json!({
        "sessionToken": {
            "type": "apiKey",
            "in": "header",
            "name": "X-Session-Token",
            "description": "Opaque session token returned by the sign-in endpoints. Required for every operation whose `x-homevault-auth` is `session` (the global default)."
        },
        "csrfToken": {
            "type": "apiKey",
            "in": "header",
            "name": "X-CSRF-Token",
            "description": "Session-tied CSRF token, required on state-changing requests once a session token is present."
        }
    })
}

fn parameter_components() -> Value {
    json!({
        "ContractVersion": {
            "name": "X-HomeVault-Contract",
            "in": "header",
            "required": false,
            "schema": { "type": "string" },
            "description": "Client contract version. A present value that differs from the server's is answered with 426 Upgrade Required; a missing value is accepted for legacy clients."
        },
        "CsrfTokenParameter": {
            "name": "X-CSRF-Token",
            "in": "header",
            "required": false,
            "schema": { "type": "string" },
            "description": "Session-tied CSRF token. Required on POST/PUT/PATCH/DELETE once the request carries a session token."
        }
    })
}

fn response_components() -> Value {
    json!({
        "Success": {
            "description": "Success. Most handlers answer `{ \"success\": true, \"data\": ... }`; a few older endpoints use `message` instead of `data`. The shape is documented per area in docs/API.md."
        },
        "BadRequest": {
            "description": "400 — the request was malformed or a value was refused. The message names the field and the rule it broke."
        },
        "Unauthorized": {
            "description": "401 — the `X-Session-Token` header is absent, empty, unknown or expired."
        },
        "Forbidden": {
            "description": "403 — the session is valid but the account is not an administrator, or the setup wizard is no longer reachable."
        },
        "UpgradeRequired": {
            "description": "426 — the client sent an `X-HomeVault-Contract` value that differs from the server's. Reload the web interface."
        },
        "InternalServerError": {
            "description": "500 — an unexpected server-side failure. No secret is included in the message."
        },
        "NotImplemented": {
            "description": "501 — this build does not implement the feature. The message says what to use instead."
        },
        "Unavailable": {
            "description": "503 — a host capability is missing (a tool, kernel module or device). The message names the missing piece."
        }
    })
}

fn build_operation(method: &str, path: &str) -> Value {
    let public = is_public(method, path);
    let upper = method.to_ascii_uppercase();
    let mut parameters = path_parameters(path);
    parameters.push(json!({ "$ref": "#/components/parameters/ContractVersion" }));
    if is_mutating(method) && !public {
        parameters.push(json!({ "$ref": "#/components/parameters/CsrfTokenParameter" }));
    }

    let mut responses = Map::new();
    responses.insert(
        "200".into(),
        json!({ "$ref": "#/components/responses/Success" }),
    );
    responses.insert(
        "400".into(),
        json!({ "$ref": "#/components/responses/BadRequest" }),
    );
    if !public {
        responses.insert(
            "401".into(),
            json!({ "$ref": "#/components/responses/Unauthorized" }),
        );
        responses.insert(
            "403".into(),
            json!({ "$ref": "#/components/responses/Forbidden" }),
        );
    }
    for (code, name) in [
        ("426", "UpgradeRequired"),
        ("500", "InternalServerError"),
        ("501", "NotImplemented"),
        ("503", "Unavailable"),
    ] {
        responses.insert(
            code.into(),
            json!({ "$ref": format!("#/components/responses/{name}") }),
        );
    }

    let mut operation = Map::new();
    operation.insert("summary".into(), json!(format!("{upper} {path}")));
    operation.insert("operationId".into(), json!(operation_id(method, path)));
    operation.insert("tags".into(), json!([tag_for_path(path)]));
    operation.insert("parameters".into(), Value::Array(parameters));
    operation.insert("responses".into(), Value::Object(responses));
    operation.insert(
        "x-homevault-auth".into(),
        json!(if public { "public" } else { "session" }),
    );
    // The global requirement covers every non-public operation; the override
    // is what makes an authentication flow correctly machine-readable.
    if public {
        operation.insert("security".into(), json!([]));
    }
    Value::Object(operation)
}

fn is_mutating(method: &str) -> bool {
    !matches!(
        method.to_ascii_lowercase().as_str(),
        "get" | "head" | "options"
    )
}

fn is_public(method: &str, path: &str) -> bool {
    PUBLIC_ROUTES
        .iter()
        .any(|(m, p)| m.eq_ignore_ascii_case(method) && *p == path)
}

fn exclusion_reason(path: &str) -> Option<&'static str> {
    for (prefix, reason) in EXCLUDED_TRANSPORTS {
        if prefix.ends_with('/') {
            if path.starts_with(prefix) {
                return Some(reason);
            }
        } else if path == *prefix {
            return Some(reason);
        }
    }
    // One-shot capability tickets minted by a session and spent by a stream.
    if path.ends_with("/ticket") || path.contains("/ticket/") {
        return Some("one-shot capability ticket");
    }
    if path.contains("/logs/stream") {
        return Some("server-sent events stream");
    }
    None
}

/// Axum spells a catch-all parameter `{*name}`; OpenAPI spells it `{name}`.
fn normalize_path(path: &str) -> String {
    path.replace("{*", "{")
}

fn tag_for_path(path: &str) -> &str {
    path.strip_prefix("/api/")
        .and_then(|rest| rest.split('/').next())
        .filter(|tag| !tag.is_empty())
        .unwrap_or("root")
}

fn operation_id(method: &str, path: &str) -> String {
    let mut id = method.to_ascii_lowercase();
    for c in path.chars() {
        if c.is_ascii_alphanumeric() {
            id.push(c.to_ascii_lowercase());
        } else if !id.ends_with('_') {
            id.push('_');
        }
    }
    id.trim_end_matches('_').to_string()
}

fn path_parameters(path: &str) -> Vec<Value> {
    let mut names: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut in_param = false;
    for c in path.chars() {
        match c {
            '{' => {
                current.clear();
                in_param = true;
            }
            '}' if in_param => {
                let name = current.trim_start_matches('*').trim();
                if !name.is_empty() && !names.iter().any(|n| n == name) {
                    names.push(name.to_string());
                }
                in_param = false;
            }
            _ if in_param => current.push(c),
            _ => {}
        }
    }
    names
        .into_iter()
        .map(|name| {
            json!({
                "name": name,
                "in": "path",
                "required": true,
                "schema": { "type": "string" }
            })
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Route-table parser
// ---------------------------------------------------------------------------

fn route_inventory() -> Vec<ApiRoute> {
    let mut routes = Vec::new();
    routes.extend(parse_routes(include_str!("web/mod.rs"), ""));
    routes.extend(parse_routes(
        include_str!("handlers/network.rs"),
        "/api/network",
    ));
    routes.extend(parse_routes(include_str!("handlers/vpn.rs"), "/api/vpn"));
    routes.extend(parse_routes(
        include_str!("handlers/rescue.rs"),
        "/api/rescue",
    ));
    routes.extend(parse_routes(
        include_str!("handlers/terminal_pty.rs"),
        "/api/terminal",
    ));
    routes.extend(parse_routes(include_str!("handlers/vm.rs"), "/api/vm"));
    routes.sort();
    routes.dedup();
    routes
}

/// Parse every `.route("…", method(handler)…)` call in one router source.
///
/// Comments are blanked first so a commented-out route is not published; the
/// second argument may chain methods (`get(a).post(b)`) and may qualify them
/// (`axum::routing::delete(c)`).
fn parse_routes(source: &str, base: &str) -> Vec<ApiRoute> {
    let code = strip_comments(source);
    let bytes = code.as_bytes();
    let mut routes = Vec::new();
    let mut i = 0usize;
    while i + 6 <= bytes.len() {
        if bytes[i] == b'"' {
            i = skip_string(bytes, i);
            continue;
        }
        if &bytes[i..i + 6] == b".route" {
            let mut open = i + 6;
            while open < bytes.len() && bytes[open].is_ascii_whitespace() {
                open += 1;
            }
            if open < bytes.len() && bytes[open] == b'(' {
                if let Some(args) = balanced_body(&code, open) {
                    if let Some((path, methods)) = parse_route_args(args, base) {
                        for (method, handler) in methods {
                            routes.push(ApiRoute {
                                method,
                                path: path.clone(),
                                handler,
                            });
                        }
                    }
                }
            }
        }
        i += 1;
    }
    routes.sort();
    routes.dedup();
    routes
}

fn parse_route_args(args: &str, base: &str) -> Option<(String, Vec<(String, String)>)> {
    let parts = split_top_level(args);
    if parts.len() < 2 {
        return None;
    }
    let raw_path = path_literal(parts[0])?;
    if !raw_path.starts_with('/') {
        return None;
    }
    let path = if raw_path.starts_with("/api/") || raw_path == "/api" {
        raw_path
    } else {
        format!("{base}{raw_path}")
    };
    if !path.starts_with("/api/") {
        return None;
    }

    let mut methods = BTreeSet::new();
    for (method, handler) in method_calls(parts[1]) {
        methods.insert((method, handler));
    }
    if methods.is_empty() {
        return None;
    }
    Some((path, methods.into_iter().collect()))
}

fn path_literal(arg: &str) -> Option<String> {
    let bytes = arg.as_bytes();
    let mut i = 0;
    while i < bytes.len() && bytes[i].is_ascii_whitespace() {
        i += 1;
    }
    if i >= bytes.len() || bytes[i] != b'"' {
        return None;
    }
    let start = i + 1;
    i = start;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 2,
            b'"' => return Some(arg[start..i].to_string()),
            _ => i += 1,
        }
    }
    None
}

fn method_calls(expr: &str) -> Vec<(String, String)> {
    let bytes = expr.as_bytes();
    let mut found = Vec::new();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b'"' {
            i = skip_string(bytes, i);
            continue;
        }
        if !is_ident_start(bytes[i]) {
            i += 1;
            continue;
        }
        let start = i;
        i += 1;
        while i < bytes.len() && (is_ident_continue(bytes[i]) || bytes[i] == b':') {
            i += 1;
        }
        let full = &expr[start..i];
        let last = full.rsplit("::").next().unwrap_or(full);
        let mut open = i;
        while open < bytes.len() && bytes[open].is_ascii_whitespace() {
            open += 1;
        }
        if open < bytes.len() && bytes[open] == b'(' && METHODS.contains(&last) {
            if let Some(body) = balanced_body(expr, open) {
                if let Some(handler) = first_ident_path(body) {
                    found.push((last.to_ascii_uppercase(), handler));
                }
            }
        }
    }
    found
}

fn first_ident_path(body: &str) -> Option<String> {
    let bytes = body.as_bytes();
    let mut i = 0;
    while i < bytes.len() && bytes[i].is_ascii_whitespace() {
        i += 1;
    }
    if i >= bytes.len() || !is_ident_start(bytes[i]) {
        return None;
    }
    let start = i;
    i += 1;
    while i < bytes.len() && (is_ident_continue(bytes[i]) || bytes[i] == b':') {
        i += 1;
    }
    body[start..i]
        .rsplit("::")
        .next()
        .map(|name| name.to_string())
}

fn is_ident_start(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || byte == b'_'
}

fn is_ident_continue(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

/// Return the text between `open` (a `(`) and its matching `)`, skipping
/// string literals so parentheses inside a message do not unbalance it.
fn balanced_body(code: &str, open: usize) -> Option<&str> {
    let bytes = code.as_bytes();
    if open >= bytes.len() || bytes[open] != b'(' {
        return None;
    }
    let mut depth = 0usize;
    let mut i = open;
    while i < bytes.len() {
        match bytes[i] {
            b'"' => i = skip_string(bytes, i),
            b'(' => {
                depth += 1;
                i += 1;
            }
            b')' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Some(&code[open + 1..i]);
                }
                i += 1;
            }
            _ => i += 1,
        }
    }
    None
}

fn skip_string(bytes: &[u8], mut i: usize) -> usize {
    if i >= bytes.len() || bytes[i] != b'"' {
        return i + 1;
    }
    i += 1;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 2,
            b'"' => return i + 1,
            _ => i += 1,
        }
    }
    i
}

fn split_top_level(args: &str) -> Vec<&str> {
    let bytes = args.as_bytes();
    let mut parts = Vec::new();
    let mut depth = 0i32;
    let mut last = 0usize;
    let mut i = 0usize;
    while i < bytes.len() {
        match bytes[i] {
            b'"' => i = skip_string(bytes, i),
            b'(' | b'[' | b'{' => {
                depth += 1;
                i += 1;
            }
            b')' | b']' | b'}' => {
                depth -= 1;
                i += 1;
            }
            b',' if depth == 0 => {
                parts.push(&args[last..i]);
                last = i + 1;
                i += 1;
            }
            _ => i += 1,
        }
    }
    parts.push(&args[last..]);
    parts
}

/// Blank Rust comments while preserving string literals and byte offsets.
///
/// The router sources contain a lot of explanatory comments inside the
/// argument lists; leaving them in place would make the method scan see prose.
fn strip_comments(source: &str) -> String {
    let bytes = source.as_bytes();
    let mut out = bytes.to_vec();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b'"' {
            i = skip_string(bytes, i);
            continue;
        }
        if bytes[i] == b'/' && i + 1 < bytes.len() {
            if bytes[i + 1] == b'/' {
                while i < bytes.len() && bytes[i] != b'\n' {
                    out[i] = b' ';
                    i += 1;
                }
                continue;
            }
            if bytes[i + 1] == b'*' {
                let mut depth = 1usize;
                out[i] = b' ';
                out[i + 1] = b' ';
                i += 2;
                while i < bytes.len() && depth > 0 {
                    if bytes[i] == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'*' {
                        depth += 1;
                        out[i] = b' ';
                        out[i + 1] = b' ';
                        i += 2;
                    } else if bytes[i] == b'*' && i + 1 < bytes.len() && bytes[i + 1] == b'/' {
                        depth -= 1;
                        out[i] = b' ';
                        out[i + 1] = b' ';
                        i += 2;
                    } else {
                        if bytes[i] != b'\n' {
                            out[i] = b' ';
                        }
                        i += 1;
                    }
                }
                continue;
            }
        }
        i += 1;
    }
    String::from_utf8(out).unwrap_or_else(|e| String::from_utf8_lossy(&e.into_bytes()).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parser_reads_method_chains_and_nested_routers() {
        let source = r#"
            let r = Router::new()
                // .route("/api/commented", get(handle_commented))
                .route("/api/items/{id}", get(handle_get).post(handle_update))
                .route(
                    "/api/items",
                    axum::routing::delete(handle_delete),
                );
            let spa = Router::new().route("/manifest.json", get(served));
        "#;
        let routes = parse_routes(source, "");
        let ops: Vec<(&str, &str)> = routes
            .iter()
            .map(|r| (r.method.as_str(), r.path.as_str()))
            .collect();
        assert_eq!(
            ops,
            vec![
                ("DELETE", "/api/items"),
                ("GET", "/api/items/{id}"),
                ("POST", "/api/items/{id}"),
            ]
        );
        assert!(!ops.iter().any(|(_, p)| p.contains("commented")));
        assert!(!ops.iter().any(|(_, p)| *p == "/manifest.json"));
    }

    #[test]
    fn test_parser_applies_a_nested_prefix() {
        let routes = parse_routes(
            r#"Router::new().route("/interfaces", get(handle_interfaces))"#,
            "/api/network",
        );
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].path, "/api/network/interfaces");
        assert_eq!(routes[0].handler, "handle_interfaces");
    }

    #[test]
    fn test_spec_covers_the_real_router_and_nested_subrouters() {
        let spec = build_spec();
        let paths = spec["paths"].as_object().expect("paths object");
        assert!(paths.contains_key("/api/meta"));
        assert!(paths.contains_key("/api/settings/general"));
        assert!(paths.contains_key("/api/network/interfaces"));
        assert!(paths.contains_key("/api/vpn/connections"));
        assert!(paths.contains_key("/api/rescue/tools"));
        assert!(paths.contains_key("/api/openapi.json"));
        assert!(
            paths.contains_key("/api/settings/config-backup/archives/{id}"),
            "Axum catch-alls must be spelled as OpenAPI path parameters"
        );
        assert!(!paths.contains_key("/api/settings/config-backup/archives/{*id}"));
    }

    #[test]
    fn test_spec_declares_openapi_3_and_the_contract_version() {
        let spec = build_spec();
        assert_eq!(spec["openapi"], OPENAPI_VERSION);
        assert_eq!(spec["x-homevault-contract-version"], CONTRACT_VERSION);
        assert_eq!(
            spec["components"]["securitySchemes"]["sessionToken"]["name"],
            "X-Session-Token"
        );
        assert_eq!(
            spec["components"]["securitySchemes"]["csrfToken"]["name"],
            "X-CSRF-Token"
        );
    }

    #[test]
    fn test_public_routes_override_the_global_session_requirement() {
        let spec = build_spec();
        let login = &spec["paths"]["/api/auth/login"]["post"];
        assert_eq!(login["security"], json!([]));
        assert_eq!(login["x-homevault-auth"], "public");
        let read = &spec["paths"]["/api/settings/general"]["get"];
        assert!(read.get("security").is_none());
        assert_eq!(read["x-homevault-auth"], "session");
        assert_eq!(
            spec["paths"]["/api/auth/login"]["post"]["responses"]
                .get("401")
                .is_none(),
            true,
            "a public operation must not advertise a session refusal"
        );
    }

    #[test]
    fn test_every_declared_public_route_is_present_and_public() {
        let spec = build_spec();
        for (method, path) in PUBLIC_ROUTES {
            let operation = spec["paths"]
                .get(*path)
                .and_then(|operations| operations.get(method.to_ascii_lowercase().as_str()))
                .unwrap_or_else(|| panic!("public route {method} {path} is missing"));
            assert_eq!(
                operation["x-homevault-auth"], "public",
                "{method} {path} must be marked public"
            );
            assert_eq!(
                operation["security"],
                json!([]),
                "{method} {path} must override the global session requirement"
            );
            if is_mutating(method) {
                assert!(
                    !operation["parameters"]
                        .to_string()
                        .contains("CsrfTokenParameter"),
                    "{method} {path} has no session yet and must not ask for a CSRF token"
                );
            }
        }
    }

    #[test]
    fn test_mutating_operations_advertise_csrf_and_reads_do_not() {
        let spec = build_spec();
        let put = &spec["paths"]["/api/settings/general"]["put"]["parameters"];
        assert!(put
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["$ref"] == "#/components/parameters/CsrfTokenParameter"));
        let get = &spec["paths"]["/api/settings/general"]["get"]["parameters"];
        assert!(!get
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["$ref"] == "#/components/parameters/CsrfTokenParameter"));
    }

    #[test]
    fn test_every_path_parameter_is_declared_required() {
        let spec = build_spec();
        for (path, operations) in spec["paths"].as_object().expect("paths") {
            let expected = path_parameters(path);
            for operation in operations.as_object().expect("operations").values() {
                let declared = operation["parameters"].as_array().expect("parameters");
                for param in &expected {
                    assert!(
                        declared.iter().any(|p| p["name"] == param["name"]
                            && p["in"] == "path"
                            && p["required"] == true),
                        "{path} does not declare path parameter {}",
                        param["name"]
                    );
                }
            }
        }
    }

    #[test]
    fn test_spec_excludes_ticket_websocket_and_binary_transports() {
        let spec = build_spec();
        let paths = spec["paths"].as_object().expect("paths");
        for forbidden in [
            "/api/terminal/ws",
            "/api/vm/console/ws",
            "/api/explorer/upload",
            "/api/explorer/upload/session",
            "/api/explorer/upload/chunk",
            "/api/explorer/upload/complete",
            "/api/explorer/download",
            "/api/explorer/watch",
            "/api/explorer/download-ticket",
            "/api/notifications/events/ticket",
            "/api/docker/containers/{name}/logs/ticket",
        ] {
            assert!(
                !paths.contains_key(forbidden),
                "{forbidden} must stay out of the JSON contract"
            );
        }
        let excluded = spec["x-homevault-excluded"].as_array().expect("excluded");
        assert!(
            !excluded.is_empty(),
            "exclusions must be visible, not silent"
        );
        assert!(excluded.iter().any(|e| e["path"] == "/api/terminal/ws"));
    }

    #[test]
    fn test_spec_serialization_is_stable() {
        let first = serde_json::to_string(&build_spec()).expect("serialize");
        let second = serde_json::to_string(&build_spec()).expect("serialize");
        assert_eq!(first, second);
        assert!(first.contains("\"openapi\":\"3.0.3\""));
    }
}

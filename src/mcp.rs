/* =============================================================================
 * File          : mcp.rs
 * Author        : Nicolas Le Roux
 * Created       : 13/10/2026
 * Last modified : 13/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   External AI-agent server (Model Context Protocol, Streamable HTTP):
 *   lets an outside agent call a filtered tool set (file listing, docker
 *   status, health, ...) authenticated by hashed bearer tokens with
 *   per-token permission snapshots. Every call is event-logged with the
 *   token id.
 *
 * Dependencies:
 *   - serde (>= 1.0), serde_json (>= 1.0)
 *
 * Usage:
 *   handlers/mcp.rs routes POST /mcp here; tokens/permissions live in the
 *   MCP repository (see docs/guides/mcp.md).
 * =============================================================================
 */

//! External AI-agent server (Model Context Protocol, Streamable HTTP).
//!
//! HomeVault ships an internal Chat/RAG assistant, but nothing an external
//! agent (Claude Code, opencode, …) can call. This module is the shareable
//! half of that gap, modelled on Umbrel's `umbrelMcp` module:
//!
//! - one master kill-switch (`enabled`) plus one **global** permission set
//!   shared by every token (per-token scoping is only revocation),
//! - opaque `hv_mcp_<id>_<secret>` bearer tokens; only the SHA-256 of the
//!   secret half is stored,
//! - a JSON-RPC `POST /mcp` endpoint speaking MCP Streamable HTTP
//!   (`initialize`, `tools/list`, `tools/call`), where the registered tools
//!   are filtered per request from the permission snapshot.
//!
//! The I/O dispatch lives in `crate::handlers::mcp`; this file holds the
//! pure core (permissions model, token minting/parsing, tool catalog,
//! permission gates, file-path guard) so it is unit-testable without a DB.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

/// Prefix of every issued token. The full token is
/// `hv_mcp_<16 hex id>_<32 hex secret>`.
pub const TOKEN_PREFIX: &str = "hv_mcp_";
/// Hard cap on live tokens (Umbrel parity: 20).
pub const MAX_TOKENS: i64 = 20;
/// MCP protocol version we handshake.
pub const PROTOCOL_VERSION: &str = "2024-11-05";

/// `"all"`, `"none"`, or an explicit list of ids/paths. Untouched unknown
/// shapes deserialize as `None` (deny) rather than failing the whole
/// permissions row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ScopeList {
    All(String),
    Ids(Vec<String>),
}

impl Default for ScopeList {
    fn default() -> Self {
        ScopeList::Ids(Vec::new())
    }
}

impl ScopeList {
    pub fn is_all(&self) -> bool {
        matches!(self, ScopeList::All(s) if s == "all")
    }

    pub fn contains_id(&self, id: &str) -> bool {
        match self {
            ScopeList::All(s) if s == "all" => true,
            ScopeList::Ids(ids) => ids.iter().any(|i| i == id),
            _ => false,
        }
    }

    pub fn is_empty_scope(&self) -> bool {
        match self {
            ScopeList::All(s) if s == "all" => false,
            ScopeList::Ids(ids) => ids.is_empty(),
            _ => true,
        }
    }
}

/// Global permission set shared by every token. Everything defaults to
/// denied (secure by default); the admin opts in per family from the UI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct McpPermissions {
    /// Which catalog apps / compose projects the agent may see and control:
    /// `"all"`, a list of bare app ids / install keys, or empty (= none).
    #[serde(default)]
    pub apps: ScopeList,
    /// Whether the agent may search the catalog and install apps.
    #[serde(default)]
    pub app_store: bool,
    /// Which host directories the agent may list: `"all"` (minus the
    /// system denylist) or explicit absolute directory prefixes.
    #[serde(default)]
    pub files: ScopeList,
    /// Whether the agent may reboot the appliance.
    #[serde(default)]
    pub manage_system: bool,
    /// Which virtual machines the agent may see and power-cycle: `"all"`,
    /// a list of VM names, or empty (= none).
    #[serde(default)]
    pub vms: ScopeList,
    /// Whether the agent may query the internal RAG knowledge base.
    #[serde(default)]
    pub rag: bool,
}

impl McpPermissions {
    pub fn default_json() -> String {
        serde_json::to_string(&McpPermissions::default()).unwrap_or_else(|_| "{}".to_string())
    }

    pub fn parse(json: &str) -> Self {
        serde_json::from_str(json).unwrap_or_default()
    }

    /// An app is addressable when the scope is `all` or lists its bare id
    /// or its install key (`<store>-<id>` for community stores).
    pub fn can_access_app(&self, bare_id: &str, install_key: &str) -> bool {
        self.apps.contains_id(bare_id) || self.apps.contains_id(install_key)
    }

    pub fn has_any_app_access(&self) -> bool {
        !self.apps.is_empty_scope() || self.app_store
    }

    pub fn can_access_vm(&self, name: &str) -> bool {
        self.vms.contains_id(name)
    }

    pub fn has_any_vm_access(&self) -> bool {
        !self.vms.is_empty_scope()
    }
}

/// Absolute path prefixes the `files: "all"` scope may never touch.
pub const FILE_DENYLIST: &[&str] = &[
    "/proc", "/sys", "/dev", "/etc", "/root", "/boot", "/run", "/var/run",
];

/// Lexically clean `requested` (no filesystem access) and check it against
/// the file scope. Returns the cleaned absolute path or a denial reason.
/// Callers re-verify symlinks with `canonicalize` at read time.
pub fn authorize_file_path(perms: &McpPermissions, requested: &str) -> Result<String, String> {
    let cleaned = clean_absolute_path(requested)?;
    if perms.files.is_all() {
        for denied in FILE_DENYLIST {
            if cleaned == *denied || cleaned.starts_with(&format!("{denied}/")) {
                return Err(format!("path {cleaned} is outside the agent file scope"));
            }
        }
        return Ok(cleaned);
    }
    if let ScopeList::Ids(grants) = &perms.files {
        for grant in grants {
            let g = grant.trim_end_matches('/');
            if g.is_empty() {
                continue;
            }
            if cleaned == g || cleaned.starts_with(&format!("{g}/")) {
                // Grants themselves must not smuggle the denylist in.
                for denied in FILE_DENYLIST {
                    if g == *denied || g.starts_with(&format!("{denied}/")) {
                        return Err(format!("grant {g} covers a system directory"));
                    }
                }
                return Ok(cleaned);
            }
        }
    }
    Err(format!("path {cleaned} is outside the agent file scope"))
}

/// Lexical absolute-path cleaning: must be absolute, no empty segments, and
/// `..` may never climb above `/`. Returns the normalized path.
pub fn clean_absolute_path(requested: &str) -> Result<String, String> {
    if requested.is_empty() {
        return Err("path must not be empty".to_string());
    }
    if !requested.starts_with('/') {
        return Err("path must be absolute".to_string());
    }
    let mut parts: Vec<&str> = Vec::new();
    for seg in requested.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                if parts.pop().is_none() {
                    return Err("path escapes the filesystem root".to_string());
                }
            }
            s if s.contains('\0') => return Err("path contains NUL".to_string()),
            s => parts.push(s),
        }
    }
    Ok(format!("/{}", parts.join("/")))
}

// ── Tokens ────────────────────────────────────────────────────────────────

fn hex_bytes(n: usize) -> String {
    let mut buf = vec![0u8; n];
    rand::Rng::fill_bytes(&mut rand::rng(), &mut buf);
    buf.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn sha256_hex(secret_hex: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(secret_hex.as_bytes());
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn constant_time_eq(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.bytes().zip(b.bytes()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// Mint a fresh `(full_token, id, secret_hash, prefix)`. The full token is
/// shown to the admin exactly once; only the hash is stored.
pub fn mint_token() -> (String, String, String, String) {
    let id = hex_bytes(8);
    let secret = hex_bytes(16);
    let full = format!("{TOKEN_PREFIX}{id}_{secret}");
    let hash = sha256_hex(&secret);
    let prefix = format!("{TOKEN_PREFIX}{id}");
    (full, id, hash, prefix)
}

/// Split a presented `Authorization: Bearer …` value into `(id, secret)`.
pub fn parse_token(presented: &str) -> Option<(String, String)> {
    let rest = presented.strip_prefix(TOKEN_PREFIX)?;
    let (id, secret) = rest.split_once('_')?;
    if id.len() != 16 || secret.len() != 32 {
        return None;
    }
    if !id.chars().all(|c| c.is_ascii_hexdigit()) || !secret.chars().all(|c| c.is_ascii_hexdigit())
    {
        return None;
    }
    Some((id.to_string(), secret.to_string()))
}

/// Verify a presented token against a stored hash (constant-time).
pub fn verify_token(presented: &str, stored_hash: &str) -> Option<String> {
    let (id, secret) = parse_token(presented)?;
    if constant_time_eq(&sha256_hex(&secret), stored_hash) {
        Some(id)
    } else {
        None
    }
}

// ── Tool catalog ──────────────────────────────────────────────────────────

/// One MCP tool descriptor (name, description, JSON-Schema input).
#[derive(Debug, Clone, Serialize)]
pub struct McpTool {
    pub name: &'static str,
    pub description: &'static str,
    pub input_schema: Value,
}

fn schema(props: &[(&str, &str)], required: &[&str]) -> Value {
    let properties: serde_json::Map<String, Value> = props
        .iter()
        .map(|(name, typ)| (name.to_string(), serde_json::json!({ "type": typ })))
        .collect();
    serde_json::json!({
        "type": "object",
        "properties": properties,
        "required": required,
    })
}

/// The tools visible for a permission snapshot. Mirrors Umbrel's families:
/// always-on system info, then `apps`, `files`, `machines` (VMs here),
/// `management` (reboot) and the HomeVault-only `knowledge` family that
/// exposes the internal RAG base to external agents.
pub fn tools_for_permissions(perms: &McpPermissions) -> Vec<McpTool> {
    let mut tools = vec![
        McpTool {
            name: "system.get_info",
            description: "Appliance identity: name, HomeVault version, CPU architecture.",
            input_schema: schema(&[], &[]),
        },
        McpTool {
            name: "system.get_resources",
            description: "Current memory, load average and uptime of the appliance.",
            input_schema: schema(&[], &[]),
        },
    ];
    if perms.has_any_app_access() {
        tools.push(McpTool {
            name: "catalog.search",
            description: "Search the app catalog (built-in plus community stores) by name, description or category.",
            input_schema: schema(&[("query", "string")], &["query"]),
        });
        tools.push(McpTool {
            name: "catalog.get",
            description: "Full manifest of one catalog app plus its install state. Takes the <store>:<id> namespaced id.",
            input_schema: schema(&[("id", "string")], &["id"]),
        });
        tools.push(McpTool {
            name: "apps.list",
            description: "Installed catalog apps with running state.",
            input_schema: schema(&[], &[]),
        });
    }
    if !perms.apps.is_empty_scope() {
        tools.extend([
            McpTool {
                name: "apps.get_status",
                description: "Running state of one installed app (bare id or install key).",
                input_schema: schema(&[("app", "string")], &["app"]),
            },
            McpTool {
                name: "apps.logs",
                description: "Tail an installed app's compose logs. Optional tail line count (default 200, max 2000).",
                input_schema: schema(&[("app", "string"), ("tail", "number")], &["app"]),
            },
            McpTool {
                name: "apps.start",
                description: "Start an installed app's stack.",
                input_schema: schema(&[("app", "string")], &["app"]),
            },
            McpTool {
                name: "apps.stop",
                description: "Stop an installed app's stack.",
                input_schema: schema(&[("app", "string")], &["app"]),
            },
            McpTool {
                name: "apps.restart",
                description: "Restart an installed app's stack.",
                input_schema: schema(&[("app", "string")], &["app"]),
            },
            McpTool {
                name: "apps.uninstall",
                description: "Stop and remove an installed app (its data directory is kept).",
                input_schema: schema(&[("app", "string")], &["app"]),
            },
        ]);
    }
    if perms.app_store {
        tools.push(McpTool {
            name: "apps.install",
            description: "Install a catalog app by <store>:<id>. Optional host port, dependency alternatives (dependency -> <store>:<id>) and environment settings.",
            input_schema: schema(&[("id", "string"), ("port", "number"), ("alternatives", "object"), ("environment", "object")], &["id"]),
        });
    }
    if !perms.files.is_empty_scope() {
        tools.push(McpTool {
            name: "files.list_directory",
            description: "List a directory inside the granted file scope. Symbolic links are resolved and re-checked against the scope.",
            input_schema: schema(&[("path", "string")], &["path"]),
        });
    }
    if perms.has_any_vm_access() {
        tools.extend([
            McpTool {
                name: "vms.list",
                description: "Virtual machines visible in this scope with their power state.",
                input_schema: schema(&[], &[]),
            },
            McpTool {
                name: "vms.start",
                description: "Start a virtual machine by name.",
                input_schema: schema(&[("name", "string")], &["name"]),
            },
            McpTool {
                name: "vms.stop",
                description: "Stop a virtual machine by name.",
                input_schema: schema(&[("name", "string")], &["name"]),
            },
            McpTool {
                name: "vms.restart",
                description: "Restart a virtual machine by name.",
                input_schema: schema(&[("name", "string")], &["name"]),
            },
        ]);
    }
    if perms.rag {
        tools.push(McpTool {
            name: "knowledge.search",
            description: "Semantic search over the appliance knowledge base (app docs, feature guides). Returns matching passages.",
            input_schema: schema(&[("query", "string")], &["query"]),
        });
    }
    if perms.manage_system {
        tools.push(McpTool {
            name: "system.reboot",
            description: "Reboot the appliance gracefully. The call returns immediately; the reboot follows within seconds.",
            input_schema: schema(&[], &[]),
        });
    }
    tools
}

#[cfg(test)]
mod tests {
    use super::*;

    fn perms() -> McpPermissions {
        McpPermissions {
            apps: ScopeList::Ids(vec!["jellyfin".to_string()]),
            app_store: true,
            files: ScopeList::Ids(vec!["/data/photos".to_string()]),
            manage_system: false,
            vms: ScopeList::All("all".to_string()),
            rag: true,
        }
    }

    #[test]
    fn test_scope_list_contains() {
        let all = ScopeList::All("all".to_string());
        assert!(all.contains_id("anything"));
        let ids = ScopeList::Ids(vec!["a".to_string()]);
        assert!(ids.contains_id("a"));
        assert!(!ids.contains_id("b"));
        assert!(!ScopeList::default().contains_id("a"));
    }

    #[test]
    fn test_permissions_parse_defaults_deny() {
        let p = McpPermissions::parse("not json");
        assert_eq!(p, McpPermissions::default());
        let p = McpPermissions::parse(r#"{"app_store":true}"#);
        assert!(p.app_store);
        assert!(!p.rag);
        assert!(p.apps.is_empty_scope());
    }

    #[test]
    fn test_can_access_app_matches_id_or_install_key() {
        let p = perms();
        assert!(p.can_access_app("jellyfin", "jellyfin"));
        assert!(!p.can_access_app("gitea", "gitea"));
        let all = McpPermissions {
            apps: ScopeList::All("all".to_string()),
            ..Default::default()
        };
        assert!(all.can_access_app("store-gitea", "store-gitea"));
    }

    #[test]
    fn test_clean_absolute_path() {
        assert_eq!(clean_absolute_path("/a/b").unwrap(), "/a/b");
        assert_eq!(clean_absolute_path("/a/./b//c").unwrap(), "/a/b/c");
        assert_eq!(clean_absolute_path("/a/../b").unwrap(), "/b");
        assert_eq!(clean_absolute_path("/").unwrap(), "/");
        assert!(clean_absolute_path("relative").is_err());
        assert!(clean_absolute_path("/../escape").is_err());
        assert!(clean_absolute_path("").is_err());
    }

    #[test]
    fn test_authorize_file_path_grants_and_denylist() {
        let p = perms();
        assert_eq!(
            authorize_file_path(&p, "/data/photos/2024").unwrap(),
            "/data/photos/2024"
        );
        assert!(authorize_file_path(&p, "/data/other").is_err());
        assert!(authorize_file_path(&p, "/data/photos/../../etc/passwd").is_err());

        let all = McpPermissions {
            files: ScopeList::All("all".to_string()),
            ..Default::default()
        };
        assert!(authorize_file_path(&all, "/data/x").is_ok());
        assert!(authorize_file_path(&all, "/etc/passwd").is_err());
        assert!(authorize_file_path(&all, "/proc/1/cmdline").is_err());

        let none = McpPermissions::default();
        assert!(authorize_file_path(&none, "/data/x").is_err());
    }

    #[test]
    fn test_token_roundtrip_and_rejections() {
        let (full, id, hash, prefix) = mint_token();
        assert!(full.starts_with(TOKEN_PREFIX));
        assert!(prefix.starts_with(TOKEN_PREFIX));
        let (pid, _) = parse_token(&full).unwrap();
        assert_eq!(pid, id);
        assert_eq!(verify_token(&full, &hash).as_deref(), Some(id.as_str()));
        assert!(verify_token(&full, &sha256_hex("00".repeat(16).as_str())).is_none());
        assert!(parse_token("bogus").is_none());
        assert!(parse_token(&format!("{TOKEN_PREFIX}short_x")).is_none());
        assert!(parse_token(&format!("{TOKEN_PREFIX}{}_zz", "g".repeat(16))).is_none());
    }

    #[test]
    fn test_tools_filtered_by_permissions() {
        let names: Vec<&str> = tools_for_permissions(&perms())
            .iter()
            .map(|t| t.name)
            .collect();
        for always in ["system.get_info", "system.get_resources"] {
            assert!(names.contains(&always), "{always} must always register");
        }
        for gated in [
            "apps.list",
            "apps.uninstall",
            "apps.install",
            "files.list_directory",
            "vms.list",
            "vms.stop",
            "knowledge.search",
            "catalog.search",
        ] {
            assert!(names.contains(&gated), "{gated} must register");
        }
        assert!(!names.contains(&"system.reboot"));

        let none = tools_for_permissions(&McpPermissions::default());
        let names: Vec<&str> = none.iter().map(|t| t.name).collect();
        assert_eq!(names, vec!["system.get_info", "system.get_resources"]);

        let p = McpPermissions {
            manage_system: true,
            ..Default::default()
        };
        let names: Vec<&str> = tools_for_permissions(&p).iter().map(|t| t.name).collect();
        assert!(names.contains(&"system.reboot"));
    }
}

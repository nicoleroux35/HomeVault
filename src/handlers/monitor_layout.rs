/* =============================================================================
 * File          : monitor_layout.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 23/10/2026 by Nicolas Le Roux
 * Version       : 1.1.0
 *
 * Description:
 *   Per-account desktop monitor-panel layout (position and visibility of
 *   the floating Resource/RAID/Weather panels), synced server-side so the
 *   same account sees the same layout from any browser origin (LAN IP,
 *   public IP, ...). Read and write are session-auth and scoped to the
 *   caller's own layout, and the blob is validated and bounded before it is
 *   stored (top-level object, size cap, depth/key/node/string limits) while
 *   the accepted bytes are stored verbatim so a read returns exactly what
 *   was written.
 *
 * Dependencies:
 *   - serde (>= 1.0)
 *
 * Usage:
 *   GET/PUT layout routes in src/web/mod.rs; consumed by the frontend
 *   monitorStore.
 * =============================================================================
 */

//! Per-account desktop monitor layout (position + visibility of the floating
//! Resource/RAID/Weather/... monitor panels), synced server-side so the same
//! account sees the same layout regardless of which browser origin (LAN IP,
//! public IP, ...) it's accessed from - the frontend's `monitorStore` also
//! keeps a localStorage copy per origin for instant, offline-tolerant reads,
//! but this table is the cross-origin source of truth.

use crate::handlers::admin_guard::require_auth;
use crate::handlers::AppState;
use crate::utils::ResponseHelper;
use axum::{extract::State, http::HeaderMap, response::Response, Json as AxumJson};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Debug, Serialize)]
pub struct MonitorLayoutResponse {
    /// Opaque JSON blob (the frontend's own `MonitorPositions` shape); `None`
    /// if this account has never saved a layout server-side yet.
    pub layout_json: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct SetMonitorLayoutRequest {
    pub layout_json: String,
}

/// Largest layout blob accepted, in bytes.
///
/// The real blob is one object of a handful of panel entries
/// (`{"positions":{...},"refreshMs":5000,"savedAt":1699…}`), a few hundred
/// bytes at most; 16 KiB leaves room for new panels while keeping the column
/// (and the client's localStorage mirror) bounded.
pub const MAX_LAYOUT_BYTES: usize = 16 * 1024;
/// Deepest nesting accepted. The shape is `{ positions: { key: { x, y } } }`
/// (three levels); a fourth is already more than any client writes.
pub const MAX_LAYOUT_DEPTH: usize = 4;
/// Total nodes (objects + arrays + scalars) accepted.
pub const MAX_LAYOUT_NODES: usize = 512;
/// Longest object key accepted.
pub const MAX_LAYOUT_KEY_LEN: usize = 64;
/// Longest string value accepted.
pub const MAX_LAYOUT_STRING_LEN: usize = 256;

/// Validate a monitor-layout blob before it is stored.
///
/// The blob is opaque to the backend (the frontend owns `MonitorPositions`),
/// so the check is structural rather than schema-based: it must be a JSON
/// **object**, fit the size cap, and stay within the depth/node/key/string
/// limits. That is enough to reject a payload that would bloat the row, break
/// the reader, or nest deeply enough to be a stack risk on parse.
///
/// The accepted string is stored **verbatim** (never re-serialised): the GET
/// handler must return exactly the bytes the client wrote, and a
/// `serde_json::Value` round trip would reorder keys.
pub fn validate_layout_blob(blob: &str) -> Result<(), String> {
    if blob.trim().is_empty() {
        return Err("Layout payload is empty".to_string());
    }
    if blob.len() > MAX_LAYOUT_BYTES {
        return Err(format!(
            "Layout payload is too large ({} bytes, max {MAX_LAYOUT_BYTES})",
            blob.len()
        ));
    }

    let value: serde_json::Value = serde_json::from_str(blob)
        .map_err(|e| format!("Layout payload is not valid JSON: {e}"))?;
    if !value.is_object() {
        return Err("Layout payload must be a JSON object".to_string());
    }

    let mut nodes = 0usize;
    // The root container is level 0; the accepted maximum is therefore "four
    // nested objects under the root", which is what MAX_LAYOUT_DEPTH now means
    // (`{"a":{"b":{"c":{"d":1}}}}` is accepted, five levels is not).
    check_layout_value(&value, 0, &mut nodes)
}

/// Recursive structural check: depth, node count, key length, string length.
fn check_layout_value(
    value: &serde_json::Value,
    depth: usize,
    nodes: &mut usize,
) -> Result<(), String> {
    *nodes += 1;
    if *nodes > MAX_LAYOUT_NODES {
        return Err(format!(
            "Layout payload has too many values (max {MAX_LAYOUT_NODES})"
        ));
    }
    if depth > MAX_LAYOUT_DEPTH {
        return Err(format!(
            "Layout payload is nested too deeply (max {MAX_LAYOUT_DEPTH} levels)"
        ));
    }

    match value {
        serde_json::Value::Object(map) => {
            for (key, child) in map {
                if key.chars().count() > MAX_LAYOUT_KEY_LEN {
                    return Err(format!(
                        "Layout payload has a key longer than {MAX_LAYOUT_KEY_LEN} characters"
                    ));
                }
                check_layout_value(child, depth + 1, nodes)?;
            }
        }
        serde_json::Value::Array(items) => {
            for child in items {
                check_layout_value(child, depth + 1, nodes)?;
            }
        }
        serde_json::Value::String(s) => {
            if s.chars().count() > MAX_LAYOUT_STRING_LEN {
                return Err(format!(
                    "Layout payload has a value longer than {MAX_LAYOUT_STRING_LEN} characters"
                ));
            }
        }
        serde_json::Value::Null | serde_json::Value::Bool(_) | serde_json::Value::Number(_) => {}
    }
    Ok(())
}

/// Fetch the calling account's own saved monitor layout, if any.
pub async fn handle_get_monitor_layout(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    let username = match require_auth(&state, &headers).await {
        Ok(u) => u,
        Err(resp) => return resp,
    };

    match sqlx::query_scalar::<_, String>(
        "SELECT layout_json FROM user_monitor_layout WHERE username = ?",
    )
    .bind(&username)
    .fetch_optional(state.db.pool().as_ref())
    .await
    {
        Ok(layout_json) => ResponseHelper::json_success(MonitorLayoutResponse { layout_json }),
        Err(e) => ResponseHelper::json_error(format!("Database error: {}", e)),
    }
}

/// Upsert the calling account's own monitor layout.
pub async fn handle_set_monitor_layout(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    AxumJson(payload): AxumJson<SetMonitorLayoutRequest>,
) -> Response {
    let username = match require_auth(&state, &headers).await {
        Ok(u) => u,
        Err(resp) => return resp,
    };

    if let Err(message) = validate_layout_blob(&payload.layout_json) {
        return ResponseHelper::json_error(message);
    }

    let result = sqlx::query(
        "INSERT INTO user_monitor_layout (username, layout_json, updated_at)
         VALUES (?, ?, CURRENT_TIMESTAMP)
         ON CONFLICT(username) DO UPDATE SET layout_json = excluded.layout_json, updated_at = CURRENT_TIMESTAMP",
    )
    .bind(&username)
    .bind(&payload.layout_json)
    .execute(state.db.pool().as_ref())
    .await;

    match result {
        Ok(_) => ResponseHelper::json_success(serde_json::json!({})),
        Err(e) => ResponseHelper::json_error(format!("Database error: {}", e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- validate_layout_blob: structural bounds on the opaque client blob ---

    /// The shape the frontend actually writes (monitorStore `MonitorPositions`).
    const REAL_BLOB: &str = r#"{
        "positions": {
            "resourceMonitor": { "x": 40, "y": 80 },
            "raidMonitor": { "x": 420, "y": 80 }
        },
        "refreshMs": 5000,
        "savedAt": 1729600000000
    }"#;

    #[test]
    fn test_validate_layout_accepts_the_real_client_blob() {
        assert!(validate_layout_blob(REAL_BLOB).is_ok());
    }

    #[test]
    fn test_validate_layout_accepts_a_legacy_blob_without_refresh_or_saved_at() {
        // Layouts stored before `refreshMs`/`savedAt` existed must keep loading.
        let legacy = r#"{"resourceMonitor":{"x":10,"y":20},"raidMonitorVisible":false}"#;
        assert!(validate_layout_blob(legacy).is_ok());
    }

    #[test]
    fn test_validate_layout_accepts_an_empty_object() {
        // A user who never moved a panel still has a valid layout.
        assert!(validate_layout_blob("{}").is_ok());
    }

    #[test]
    fn test_validate_layout_rejects_an_empty_payload() {
        assert!(validate_layout_blob("").is_err());
        assert!(validate_layout_blob("   \n\t ").is_err());
    }

    #[test]
    fn test_validate_layout_rejects_a_payload_over_the_size_cap() {
        let huge = format!(r#"{{"blob":"{}"}}"#, "x".repeat(MAX_LAYOUT_BYTES));
        let err = validate_layout_blob(&huge).unwrap_err();
        assert!(err.contains("too large"), "{err}");
    }

    #[test]
    fn test_validate_layout_rejects_invalid_json() {
        // Truncated / hand-edited localStorage mirrors reach here.
        assert!(validate_layout_blob(r#"{"positions": "#).is_err());
        assert!(validate_layout_blob("not json at all").is_err());
        let err = validate_layout_blob(r#"{"a":"#).unwrap_err();
        assert!(err.contains("not valid JSON"), "{err}");
    }

    #[test]
    fn test_validate_layout_rejects_a_non_object_top_level() {
        // The store reads the blob as an object; an array or a scalar would
        // make `{...defaults, ...parsed}` silently drop everything.
        for payload in ["[]", "[1,2,3]", "\"text\"", "42", "null", "true"] {
            let err = validate_layout_blob(payload).unwrap_err();
            assert!(err.contains("must be a JSON object"), "{payload}: {err}");
        }
    }

    #[test]
    fn test_validate_layout_rejects_a_deeply_nested_payload() {
        // `{"a":{"a":{"a":{"a":{}}}}}` — five levels, past MAX_LAYOUT_DEPTH.
        let deep = r#"{"a":{"b":{"c":{"d":{"e":1}}}}}"#;
        let err = validate_layout_blob(deep).unwrap_err();
        assert!(err.contains("nested too deeply"), "{err}");

        // Four levels (the maximum) is still accepted.
        let ok = r#"{"a":{"b":{"c":{"d":1}}}}"#;
        assert!(validate_layout_blob(ok).is_ok());
    }

    #[test]
    fn test_validate_layout_rejects_too_many_values() {
        let items: Vec<String> = (0..MAX_LAYOUT_NODES + 8)
            .map(|i| format!(r#"{{"k{i}":{i}}}"#))
            .collect();
        let payload = format!(r#"{{"panels":[{}]}}"#, items.join(","));
        let err = validate_layout_blob(&payload).unwrap_err();
        assert!(err.contains("too many values"), "{err}");
    }

    #[test]
    fn test_validate_layout_rejects_an_over_long_key() {
        let key = "k".repeat(MAX_LAYOUT_KEY_LEN + 1);
        let payload = format!(r#"{{"{key}":1}}"#);
        let err = validate_layout_blob(&payload).unwrap_err();
        assert!(err.contains("key longer than"), "{err}");

        let ok_key = "k".repeat(MAX_LAYOUT_KEY_LEN);
        let ok = format!(r#"{{"{ok_key}":1}}"#);
        assert!(validate_layout_blob(&ok).is_ok());
    }

    #[test]
    fn test_validate_layout_rejects_an_over_long_string_value() {
        let value = "v".repeat(MAX_LAYOUT_STRING_LEN + 1);
        let payload = format!(r#"{{"note":"{value}"}}"#);
        let err = validate_layout_blob(&payload).unwrap_err();
        assert!(err.contains("value longer than"), "{err}");

        let ok_value = "v".repeat(MAX_LAYOUT_STRING_LEN);
        let ok = format!(r#"{{"note":"{ok_value}"}}"#);
        assert!(validate_layout_blob(&ok).is_ok());
    }

    #[test]
    fn test_validate_layout_accepts_an_escaped_string_within_the_limit() {
        // The cap counts characters, not JSON escapes: a 40-character value
        // written with `\uXXXX` escapes is still 40 characters.
        let escaped = "\\u00e9".repeat(40);
        let payload = format!(r#"{{"note":"{escaped}"}}"#);
        assert!(validate_layout_blob(&payload).is_ok());
    }

    #[test]
    fn test_validate_layout_is_verbatim_friendly() {
        // Key order and whitespace must survive, because the GET handler
        // returns the stored string byte-for-byte (tests/widgets_integration.rs
        // asserts the round trip). Validation must not normalise it.
        let odd = "{ \"z\": 1, \"a\": [2, 3], \"m\": null }";
        assert!(validate_layout_blob(odd).is_ok());
        // Nothing in the validator rewrites the caller's string: it only reads.
        assert_eq!(odd, "{ \"z\": 1, \"a\": [2, 3], \"m\": null }");
    }
}

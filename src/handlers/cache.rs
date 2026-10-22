/* =============================================================================
 * File          : cache.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 10/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Disk cache-tier endpoints: settings (cache pool, mover cron schedule,
 *   free-space and age thresholds, bandwidth limit), cache shares with
 *   their automatic SMB exports, and manual mover triggering that runs in
 *   the background and reports progress over SSE. Reads are auth-only;
 *   mutations are admin-only and validated (cron shape, pool existence,
 *   value ranges).
 *
 * Dependencies:
 *   - axum (>= 0.8)
 *   - serde (>= 1.0)
 *
 * Usage:
 *   Routes /api/cache in src/web/mod.rs; the mover executes
 *   crate::cache::run_mover, its runs are listed for the history view.
 * =============================================================================
 */

//! HTTP handlers for the disk cache tier.

use crate::cache::{mover_is_running, run_mover, CacheMode, PoolMounts};
use crate::handlers::admin_guard::{require_admin, require_auth};
use crate::handlers::AppState;
use crate::pools::{PoolInfo, PoolService};
use crate::shares::{Share, ShareType, SharesService};
use crate::storage::cache_repo::{CacheRepository, CacheSettings, CacheShare};
use crate::utils::ResponseHelper;
use axum::{extract::State, http::HeaderMap, response::Response};
use std::sync::Arc;

/// The mover's free-space reserve is expressed as a percentage; the UI has
/// always clamped it to 1–95 and the server used to accept `0`, which means
/// "drain the cache to nothing" for `Prefer` shares.
const MIN_FREE_PERCENT_MIN: u32 = 1;
const MIN_FREE_PERCENT_MAX: u32 = 95;

/// Scan the pools off the async runtime.
///
/// `PoolService::scan_pools` shells out to `zpool`, `mdadm`, `btrfs` and `df`;
/// calling it inline from a handler parked a runtime worker for the duration of
/// three subprocesses, and the old `unwrap_or_default()` collapsed a *failed*
/// scan into "the pool you named does not exist" (ROADMAP 2.3).
async fn scan_pools_async() -> Result<Vec<PoolInfo>, String> {
    tokio::task::spawn_blocking(PoolService::scan_pools)
        .await
        .map_err(|e| format!("Pool scan task failed: {}", e))?
        .map_err(|e| format!("Failed to scan pools: {}", e))
}

/// Validate a 5-field cron expression.
fn validate_cron(cron: &str) -> Result<(), String> {
    if cron.split_whitespace().count() == 5 {
        Ok(())
    } else {
        Err("Mover schedule must be a 5-field cron expression (min hour day-of-month month day-of-week)".to_string())
    }
}

/// Validate the `Prefer` free-space reserve (1–95 %).
fn validate_min_free_percent(value: u32) -> Result<(), String> {
    if (MIN_FREE_PERCENT_MIN..=MIN_FREE_PERCENT_MAX).contains(&value) {
        Ok(())
    } else {
        Err(format!(
            "min_free_percent must be between {} and {}",
            MIN_FREE_PERCENT_MIN, MIN_FREE_PERCENT_MAX
        ))
    }
}

/// Validate a cache-share name (1–64 chars, letters/digits/`-`/`_`).
fn validate_share_name(name: &str) -> Result<(), String> {
    if name.is_empty()
        || name.len() > 64
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(
            "Cache share name must be 1-64 characters (letters, digits, - or _)".to_string(),
        );
    }
    Ok(())
}

/// Validate a cache share's target pool against the configured cache pool and
/// the pools that actually exist. Shared by create and update — the update path
/// used to accept any string at all.
fn validate_target_pool(
    target: &str,
    cache_pool: Option<&str>,
    known_pools: &[String],
) -> Result<(), String> {
    if target.trim().is_empty() {
        return Err("Target pool is required".to_string());
    }
    if let Some(cache_pool) = cache_pool {
        if target == cache_pool {
            return Err("Target pool cannot be the same as the cache pool".to_string());
        }
    }
    if !known_pools.iter().any(|p| p == target) {
        return Err(format!("Target pool '{}' does not exist", target));
    }
    Ok(())
}

/// Normalise the `cache_pool` field of an update.
///
/// * `Some(Some(name))` — designate that pool;
/// * `Some(None)` — clear the designation (`null`, `""` or whitespace);
/// * `None` — leave it untouched.
///
/// Clearing used to be impossible: `payload.cache_pool.or(current.cache_pool)`
/// turned `Some("")` into an empty pool name that then failed validation, so
/// only a client that sent `null` could clear it.
fn normalize_pool_field(raw: Option<String>) -> Option<Option<String>> {
    raw.map(|value| {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    })
}

/// Returns the current cache-tier settings and the list of pools eligible to
/// act as the cache or target tier.
#[axum_macros::debug_handler]
pub async fn handle_cache_settings_get(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(resp) = require_auth(&state, &headers).await {
        return resp;
    }

    let repo = CacheRepository::new(state.db.pool());
    match repo.get_settings().await {
        Ok(settings) => ResponseHelper::json_success(settings),
        Err(e) => ResponseHelper::json_error(format!("Failed to load cache settings: {}", e)),
    }
}

/// Body for updating cache-tier settings.
#[derive(serde::Deserialize)]
pub struct CacheSettingsUpdate {
    pub cache_pool: Option<String>,
    pub mover_cron: Option<String>,
    pub mover_enabled: Option<bool>,
    pub min_free_percent: Option<u32>,
    pub mover_age_days: Option<u32>,
    pub mover_excludes: Option<String>,
    pub mover_bwlimit: Option<u32>,
}

/// Updates the cache-tier settings (admin only).
#[axum_macros::debug_handler]
pub async fn handle_cache_settings_update(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    axum::extract::Json(payload): axum::extract::Json<CacheSettingsUpdate>,
) -> Response {
    if let Err(resp) = require_admin(&state, &headers).await {
        return resp;
    }

    let repo = CacheRepository::new(state.db.pool());
    let current = match repo.get_settings().await {
        Ok(s) => s,
        Err(e) => {
            return ResponseHelper::json_error(format!("Failed to load cache settings: {}", e))
        }
    };

    if let Some(ref cron) = payload.mover_cron {
        if let Err(e) = validate_cron(cron) {
            return ResponseHelper::json_error(e);
        }
    }

    // `Some("")` / whitespace clears the designation; a non-empty value must be
    // a pool that exists.
    let cache_pool = match normalize_pool_field(payload.cache_pool.clone()) {
        Some(None) => None,
        Some(Some(pool)) => {
            let pools = match scan_pools_async().await {
                Ok(p) => p,
                Err(e) => return ResponseHelper::json_error(e),
            };
            if !pools.iter().any(|p| p.name == pool) {
                return ResponseHelper::json_error(format!("Pool '{}' does not exist", pool));
            }
            Some(pool)
        }
        None => current.cache_pool.clone(),
    };

    if let Some(mfp) = payload.min_free_percent {
        if let Err(e) = validate_min_free_percent(mfp) {
            return ResponseHelper::json_error(e);
        }
    }

    if let Some(age) = payload.mover_age_days {
        if age > 3650 {
            return ResponseHelper::json_error(
                "mover_age_days must be between 0 and 3650".to_string(),
            );
        }
    }

    if let Some(bw) = payload.mover_bwlimit {
        if bw > 10_000_000 {
            return ResponseHelper::json_error(
                "mover_bwlimit must be between 0 and 10,000,000 KB/s".to_string(),
            );
        }
    }

    let mover_cron = payload.mover_cron.clone().unwrap_or(current.mover_cron);
    let mover_enabled = payload.mover_enabled.unwrap_or(current.mover_enabled);
    let min_free_percent = payload.min_free_percent.unwrap_or(current.min_free_percent);
    let mover_age_days = payload.mover_age_days.unwrap_or(current.mover_age_days);
    let mover_excludes = payload.mover_excludes.clone().or(current.mover_excludes);
    let mover_bwlimit = payload.mover_bwlimit.unwrap_or(current.mover_bwlimit);

    let settings = CacheSettings {
        cache_pool,
        mover_cron,
        mover_enabled,
        min_free_percent,
        mover_last_run_at: current.mover_last_run_at,
        mover_age_days,
        mover_excludes,
        mover_bwlimit,
    };

    match repo.save_settings(&settings).await {
        Ok(_) => {
            ResponseHelper::json_success(serde_json::json!({ "message": "Cache settings saved" }))
        }
        Err(e) => ResponseHelper::json_error(format!("Failed to save cache settings: {}", e)),
    }
}

/// Lists pools usable as cache/target tiers, plus the current cache shares.
#[axum_macros::debug_handler]
pub async fn handle_cache_pools_get(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(resp) = require_auth(&state, &headers).await {
        return resp;
    }

    match scan_pools_async().await {
        Ok(pools) => {
            let pools: Vec<serde_json::Value> = pools
                .into_iter()
                .map(|p| {
                    serde_json::json!({
                        "name": p.name,
                        "pool_type": format!("{:?}", p.pool_type),
                        "mount_point": p.mount_point,
                        "size_bytes": p.size_bytes,
                        "used_bytes": p.used_bytes,
                        "available_bytes": p.available_bytes,
                    })
                })
                .collect();
            ResponseHelper::json_success(pools)
        }
        Err(e) => ResponseHelper::json_error(format!("Failed to list pools: {}", e)),
    }
}

/// Lists cache shares.
#[axum_macros::debug_handler]
pub async fn handle_cache_shares_list(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(resp) = require_auth(&state, &headers).await {
        return resp;
    }

    let repo = CacheRepository::new(state.db.pool());
    match repo.list_shares().await {
        Ok(shares) => ResponseHelper::json_success(shares),
        Err(e) => ResponseHelper::json_error(format!("Failed to list cache shares: {}", e)),
    }
}

/// Body for creating a cache share.
#[derive(serde::Deserialize)]
pub struct CreateCacheShareRequest {
    pub name: String,
    pub cache_mode: String,
    pub target_pool: String,
}

/// Creates a cache share (admin only) and a matching SMB export so the data is
/// accessible over the network.
#[axum_macros::debug_handler]
pub async fn handle_cache_share_create(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    axum::extract::Json(payload): axum::extract::Json<CreateCacheShareRequest>,
) -> Response {
    if let Err(resp) = require_admin(&state, &headers).await {
        return resp;
    }

    let name = payload.name.trim();
    if let Err(e) = validate_share_name(name) {
        return ResponseHelper::json_error(e);
    }
    let Some(mode) = CacheMode::parse(&payload.cache_mode) else {
        return ResponseHelper::json_error(
            "cache_mode must be one of: yes, no, prefer, only".to_string(),
        );
    };

    let repo = CacheRepository::new(state.db.pool());

    if repo
        .get_share(name)
        .await
        .map(|s| s.is_some())
        .unwrap_or(false)
    {
        return ResponseHelper::json_error(format!("Cache share '{}' already exists", name));
    }

    // Validate cache pool is configured.
    let settings = match repo.get_settings().await {
        Ok(s) => s,
        Err(e) => {
            return ResponseHelper::json_error(format!("Failed to load cache settings: {}", e))
        }
    };
    let Some(cache_pool) = settings.cache_pool else {
        return ResponseHelper::json_error(
            "No cache pool configured. Set one in Cache settings first.".to_string(),
        );
    };

    // One pool scan: validates the target and feeds the SMB export path.
    let pools = match scan_pools_async().await {
        Ok(p) => p,
        Err(e) => return ResponseHelper::json_error(e),
    };
    let known: Vec<String> = pools.iter().map(|p| p.name.clone()).collect();
    if let Err(e) = validate_target_pool(&payload.target_pool, Some(&cache_pool), &known) {
        return ResponseHelper::json_error(e);
    }
    let mounts = PoolMounts::from_pools(pools);

    match repo
        .create_share(name, mode.as_str(), &payload.target_pool)
        .await
    {
        Ok(share) => {
            // Best-effort: create an SMB export at the active path.
            create_or_update_smb_export(&share, &mounts, &cache_pool);
            ResponseHelper::json_success(share)
        }
        Err(e) => ResponseHelper::json_error(format!("Failed to create cache share: {}", e)),
    }
}

/// Body for updating a cache share.
#[derive(serde::Deserialize, Default)]
pub struct UpdateCacheShareRequest {
    pub cache_mode: Option<String>,
    pub target_pool: Option<String>,
    pub enabled: Option<bool>,
}

/// Updates a cache share (admin only).
#[axum_macros::debug_handler]
pub async fn handle_cache_share_update(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(name): axum::extract::Path<String>,
    headers: HeaderMap,
    axum::extract::Json(payload): axum::extract::Json<UpdateCacheShareRequest>,
) -> Response {
    if let Err(resp) = require_admin(&state, &headers).await {
        return resp;
    }

    if let Some(ref m) = payload.cache_mode {
        if CacheMode::parse(m).is_none() {
            return ResponseHelper::json_error(
                "cache_mode must be one of: yes, no, prefer, only".to_string(),
            );
        }
    }

    let repo = CacheRepository::new(state.db.pool());

    // A retarget used to be accepted verbatim: `cache_shares.target_pool` could
    // end up naming a pool that does not exist (or the cache pool itself), and
    // the mover then skipped the share on every run.
    let mut scanned: Option<Vec<PoolInfo>> = None;
    if let Some(ref target) = payload.target_pool {
        let pools = match scan_pools_async().await {
            Ok(p) => p,
            Err(e) => return ResponseHelper::json_error(e),
        };
        let known: Vec<String> = pools.iter().map(|p| p.name.clone()).collect();
        let cache_pool = repo.get_settings().await.ok().and_then(|s| s.cache_pool);
        if let Err(e) = validate_target_pool(target, cache_pool.as_deref(), &known) {
            return ResponseHelper::json_error(e);
        }
        scanned = Some(pools);
    }

    match repo
        .update_share(
            &name,
            payload.cache_mode.as_deref(),
            payload.target_pool.as_deref(),
            payload.enabled,
        )
        .await
    {
        Ok(Some(share)) => {
            if payload.cache_mode.is_some() || payload.target_pool.is_some() {
                let settings = repo.get_settings().await.ok();
                if let Some(cp) = settings.and_then(|s| s.cache_pool) {
                    let pools = match scanned {
                        Some(p) => Some(p),
                        None => scan_pools_async().await.ok(),
                    };
                    if let Some(pools) = pools {
                        create_or_update_smb_export(&share, &PoolMounts::from_pools(pools), &cp);
                    }
                }
            }
            ResponseHelper::json_success(share)
        }
        Ok(None) => ResponseHelper::json_error(format!("Cache share '{}' not found", name)),
        Err(e) => ResponseHelper::json_error(format!("Failed to update cache share: {}", e)),
    }
}

/// Deletes a cache share (admin only) and its SMB export.
#[axum_macros::debug_handler]
pub async fn handle_cache_share_delete(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(name): axum::extract::Path<String>,
    headers: HeaderMap,
) -> Response {
    if let Err(resp) = require_admin(&state, &headers).await {
        return resp;
    }

    let repo = CacheRepository::new(state.db.pool());
    match repo.delete_share(&name).await {
        Ok(true) => {
            // Best-effort: remove the SMB export.
            let _ = SharesService::delete_share(&name, ShareType::Smb);
            ResponseHelper::json_success(serde_json::json!({ "message": "Cache share deleted" }))
        }
        Ok(false) => ResponseHelper::json_error(format!("Cache share '{}' not found", name)),
        Err(e) => ResponseHelper::json_error(format!("Failed to delete cache share: {}", e)),
    }
}

/// Triggers the mover immediately (admin only). Runs in the background so the
/// client can follow live progress over SSE via the returned `job_id`.
#[axum_macros::debug_handler]
pub async fn handle_cache_mover_run(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(resp) = require_admin(&state, &headers).await {
        return resp;
    }

    if mover_is_running() {
        return ResponseHelper::json_error("Cache mover is already running".to_string());
    }

    let job_id = crate::sse::ProgressEmitter::cache_mover();
    let job_id_resp = job_id.clone();
    let pool = state.db.pool();
    tokio::spawn(async move {
        let _ = run_mover(pool, Some(job_id)).await;
    });

    ResponseHelper::json_success(serde_json::json!({ "job_id": job_id_resp }))
}

/// Lists recent mover runs.
#[axum_macros::debug_handler]
pub async fn handle_cache_mover_runs(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    if let Err(resp) = require_auth(&state, &headers).await {
        return resp;
    }

    let repo = CacheRepository::new(state.db.pool());
    match repo.list_runs(50).await {
        Ok(runs) => ResponseHelper::json_success(runs),
        Err(e) => ResponseHelper::json_error(format!("Failed to list mover runs: {}", e)),
    }
}

/// Create or update the SMB export for a cache share so its data is reachable
/// over the network. Best-effort: failures are logged, not fatal.
///
/// `mounts` is the pool scan the caller already performed — resolving the two
/// mount points here used to shell out to `zpool`/`mdadm`/`btrfs` twice more.
fn create_or_update_smb_export(share: &CacheShare, mounts: &PoolMounts, cache_pool: &str) {
    let (Some(cache_mount), Some(target_mount)) = (
        mounts.mount_of(cache_pool),
        mounts.mount_of(&share.target_pool),
    ) else {
        return;
    };

    let active = crate::cache::CacheService::active_path(share, cache_mount, target_mount);
    let smb_share = Share {
        name: share.name.clone(),
        share_type: ShareType::Smb,
        path: active,
        enabled: share.enabled,
        comment: format!("HomeVault cache share (mode: {})", share.cache_mode),
        options: vec![],
    };

    // Try update first (in case it already exists), then create.
    if SharesService::update_share(
        &share.name,
        ShareType::Smb,
        crate::shares::ShareUpdate {
            path: Some(smb_share.path.clone()),
            enabled: Some(share.enabled),
            comment: Some(smb_share.comment.clone()),
            options: Some(vec![]),
        },
    )
    .is_err()
    {
        if let Err(e) = SharesService::create_share(&smb_share) {
            tracing::warn!(
                "Failed to create SMB export for cache share '{}': {}",
                share.name,
                e
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- validate_cron ----------------------------------------------

    #[test]
    fn test_validate_cron_accepts_five_fields() {
        for ok in ["0 3 * * *", "*/15 2 * * 1-5", "0 0 1 1 0"] {
            assert!(validate_cron(ok).is_ok(), "{ok:?} must be accepted");
        }
    }

    #[test]
    fn test_validate_cron_rejects_the_wrong_field_count() {
        for bad in ["", "nope", "* * * *", "* * * * * *"] {
            assert!(validate_cron(bad).is_err(), "{bad:?} must be rejected");
        }
    }

    // -- validate_min_free_percent ----------------------------------

    #[test]
    fn test_validate_min_free_percent_accepts_the_ui_range() {
        for ok in [1, 20, 50, 95] {
            assert!(
                validate_min_free_percent(ok).is_ok(),
                "{ok} must be accepted"
            );
        }
    }

    #[test]
    fn test_validate_min_free_percent_rejects_zero_and_over_95() {
        // 0 means "drain the cache to nothing" for `prefer` shares; the UI has
        // always clamped 1-95, the server used to accept anything up to 100.
        for bad in [0, 96, 100, 1000] {
            assert!(
                validate_min_free_percent(bad).is_err(),
                "{bad} must be rejected"
            );
        }
    }

    // -- validate_share_name ----------------------------------------

    #[test]
    fn test_validate_share_name_accepts_reasonable_names() {
        for ok in ["media", "Movies-2026", "a_b", "x"] {
            assert!(validate_share_name(ok).is_ok(), "{ok:?} must be accepted");
        }
    }

    #[test]
    fn test_validate_share_name_rejects_paths_and_junk() {
        for bad in ["", "with space", "../etc", "a/b", "nul\0", "é"] {
            assert!(
                validate_share_name(bad).is_err(),
                "{bad:?} must be rejected"
            );
        }
        assert!(validate_share_name(&"a".repeat(65)).is_err());
        assert!(validate_share_name(&"a".repeat(64)).is_ok());
    }

    // -- validate_target_pool ---------------------------------------

    fn known() -> Vec<String> {
        vec!["cache".into(), "array".into()]
    }

    #[test]
    fn test_validate_target_pool_accepts_an_existing_other_pool() {
        assert!(validate_target_pool("array", Some("cache"), &known()).is_ok());
        assert!(validate_target_pool("array", None, &known()).is_ok());
    }

    #[test]
    fn test_validate_target_pool_rejects_the_cache_pool_itself() {
        let err = validate_target_pool("cache", Some("cache"), &known()).unwrap_err();
        assert!(err.contains("same as the cache pool"), "{err}");
    }

    #[test]
    fn test_validate_target_pool_rejects_an_unknown_or_empty_pool() {
        let err = validate_target_pool("ghost", Some("cache"), &known()).unwrap_err();
        assert!(err.contains("does not exist"), "{err}");
        assert!(validate_target_pool("  ", Some("cache"), &known()).is_err());
    }

    // -- normalize_pool_field ---------------------------------------

    #[test]
    fn test_normalize_pool_field_absent_leaves_the_designation_alone() {
        assert_eq!(normalize_pool_field(None), None);
    }

    #[test]
    fn test_normalize_pool_field_empty_clears_the_designation() {
        // `Some("")` used to become an empty pool name and then fail
        // validation, so only `null` could clear it.
        assert_eq!(normalize_pool_field(Some(String::new())), Some(None));
        assert_eq!(normalize_pool_field(Some("   ".into())), Some(None));
        assert_eq!(normalize_pool_field(Some("\t\n".into())), Some(None));
    }

    #[test]
    fn test_normalize_pool_field_trims_a_real_pool_name() {
        assert_eq!(
            normalize_pool_field(Some("  cache ".into())),
            Some(Some("cache".to_string()))
        );
    }
}

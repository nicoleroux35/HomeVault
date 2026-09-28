//! HTTP handlers for the disk cache tier.

use crate::cache::{mover_is_running, run_mover, CacheMode};
use crate::handlers::admin_guard::{require_admin, require_auth};
use crate::handlers::AppState;
use crate::pools::PoolService;
use crate::shares::{Share, ShareType, SharesService};
use crate::storage::cache_repo::{CacheRepository, CacheShare};
use crate::utils::ResponseHelper;
use axum::{extract::State, http::HeaderMap, response::Response};
use std::sync::Arc;

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
        Err(e) => return ResponseHelper::json_error(format!("Failed to load cache settings: {}", e)),
    };

    if let Some(ref cron) = payload.mover_cron {
        if cron.trim().split_whitespace().count() != 5 {
            return ResponseHelper::json_error(
                "Mover schedule must be a 5-field cron expression (min hour day-of-month month day-of-week)".to_string(),
            );
        }
    }

    if let Some(ref pool) = payload.cache_pool {
        if !pool.is_empty() {
            let pools = PoolService::scan_pools().unwrap_or_default();
            if !pools.iter().any(|p| p.name == *pool) {
                return ResponseHelper::json_error(format!("Pool '{}' does not exist", pool));
            }
        }
    }

    if let Some(mfp) = payload.min_free_percent {
        if mfp > 100 {
            return ResponseHelper::json_error("min_free_percent must be between 0 and 100".to_string());
        }
    }

    if let Some(age) = payload.mover_age_days {
        if age > 3650 {
            return ResponseHelper::json_error("mover_age_days must be between 0 and 3650".to_string());
        }
    }

    if let Some(bw) = payload.mover_bwlimit {
        if bw > 10_000_000 {
            return ResponseHelper::json_error("mover_bwlimit must be between 0 and 10,000,000 KB/s".to_string());
        }
    }

    let cache_pool = payload.cache_pool.clone().or(current.cache_pool);
    let mover_cron = payload.mover_cron.clone().unwrap_or(current.mover_cron);
    let mover_enabled = payload.mover_enabled.unwrap_or(current.mover_enabled);
    let min_free_percent = payload.min_free_percent.unwrap_or(current.min_free_percent);
    let mover_age_days = payload.mover_age_days.unwrap_or(current.mover_age_days);
    let mover_excludes = payload.mover_excludes.clone().or(current.mover_excludes);
    let mover_bwlimit = payload.mover_bwlimit.unwrap_or(current.mover_bwlimit);

    match repo
        .save_settings(
            cache_pool.as_deref(),
            &mover_cron,
            mover_enabled,
            min_free_percent,
            current.mover_last_run_at.as_deref(),
            mover_age_days,
            mover_excludes.as_deref(),
            mover_bwlimit,
        )
        .await
    {
        Ok(_) => ResponseHelper::json_success(serde_json::json!({ "message": "Cache settings saved" })),
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

    match PoolService::scan_pools() {
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
    if name.is_empty() || name.len() > 64 || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return ResponseHelper::json_error(
            "Cache share name must be 1-64 characters (letters, digits, - or _)".to_string(),
        );
    }
    let Some(mode) = CacheMode::parse(&payload.cache_mode) else {
        return ResponseHelper::json_error("cache_mode must be one of: yes, no, prefer, only".to_string());
    };

    let repo = CacheRepository::new(state.db.pool());

    if repo.get_share(name).await.map(|s| s.is_some()).unwrap_or(false) {
        return ResponseHelper::json_error(format!("Cache share '{}' already exists", name));
    }

    // Validate target pool exists.
    let pools = PoolService::scan_pools().unwrap_or_default();
    if !pools.iter().any(|p| p.name == payload.target_pool) {
        return ResponseHelper::json_error(format!("Target pool '{}' does not exist", payload.target_pool));
    }
    // Validate cache pool is configured.
    let settings = match repo.get_settings().await {
        Ok(s) => s,
        Err(e) => return ResponseHelper::json_error(format!("Failed to load cache settings: {}", e)),
    };
    let Some(cache_pool) = settings.cache_pool else {
        return ResponseHelper::json_error("No cache pool configured. Set one in Cache settings first.".to_string());
    };
    if payload.target_pool == cache_pool {
        return ResponseHelper::json_error("Target pool cannot be the same as the cache pool".to_string());
    }

    match repo.create_share(name, mode.as_str(), &payload.target_pool).await {
        Ok(share) => {
            // Best-effort: create an SMB export at the active path.
            create_or_update_smb_export(&share, &cache_pool, &payload.target_pool);
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
            return ResponseHelper::json_error("cache_mode must be one of: yes, no, prefer, only".to_string());
        }
    }

    let repo = CacheRepository::new(state.db.pool());
    match repo
        .update_share(&name, payload.cache_mode.as_deref(), payload.target_pool.as_deref(), payload.enabled)
        .await
    {
        Ok(Some(share)) => {
            if payload.cache_mode.is_some() || payload.target_pool.is_some() {
                let settings = repo.get_settings().await.ok();
                if let Some(s) = settings {
                    if let Some(cp) = s.cache_pool {
                        let tp = share.target_pool.clone();
                        create_or_update_smb_export(&share, &cp, &tp);
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
fn create_or_update_smb_export(share: &CacheShare, cache_pool: &str, target_pool: &str) {
    let cache_mount = CacheServiceMount::mount_of(cache_pool);
    let target_mount = CacheServiceMount::mount_of(target_pool);
    let (cache_mount, target_mount) = match (cache_mount, target_mount) {
        (Some(c), Some(t)) => (c, t),
        _ => return,
    };

    let active = crate::cache::CacheService::active_path(share, &cache_mount, &target_mount);
    let smb_share = Share {
        name: share.name.clone(),
        share_type: ShareType::Smb,
        path: active,
        enabled: share.enabled,
        comment: format!("HomeVault cache share (mode: {})", share.cache_mode),
        options: vec![],
    };

    // Try update first (in case it already exists), then create.
    if SharesService::update_share(&share.name, ShareType::Smb, crate::shares::ShareUpdate {
        path: Some(smb_share.path.clone()),
        enabled: Some(share.enabled),
        comment: Some(smb_share.comment.clone()),
        options: Some(vec![]),
    })
    .is_err()
    {
        if let Err(e) = SharesService::create_share(&smb_share) {
            tracing::warn!("Failed to create SMB export for cache share '{}': {}", share.name, e);
        }
    }
}

/// Tiny helper to resolve a pool name to its mount point.
struct CacheServiceMount;
impl CacheServiceMount {
    fn mount_of(pool_name: &str) -> Option<String> {
        let pools = PoolService::scan_pools().ok()?;
        pools.into_iter().find(|p| p.name == pool_name).and_then(|p| p.mount_point)
    }
}

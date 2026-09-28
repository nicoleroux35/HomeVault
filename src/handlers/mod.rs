use crate::auth::AuthService;
use crate::config::PathGuard;
use crate::middleware::rate_limit::RateLimitState;
use crate::storage::{Database, WidgetRepository};
use std::sync::Arc;

/// Global application state injected into every handler via Axum's `State` extractor.
///
/// Cloning is cheap — all fields are `Arc` or light clones.
#[derive(Clone)]
pub struct AppState {
    pub auth: AuthService,
    pub widgets: WidgetRepository,
    pub db: Database,
    pub path_guard: PathGuard,
    pub admin_rate_limiter: Arc<RateLimitState>,
    pub pty_manager: Arc<terminal_pty::PtyManager>,
    pub downloader: crate::handlers::downloader::Aria2State,
    pub ransomware_watch_tx: tokio::sync::mpsc::UnboundedSender<crate::ransomware_watcher::WatchCommand>,
}

pub type AppStateArc = Arc<AppState>;

pub mod activity;
pub mod auth;
pub mod admin_guard;
pub mod antivirus;
pub mod avatars;
pub mod cache;
pub mod chat;
pub mod dashboard;
pub mod docker;
pub mod docker_catalog;
pub mod downloader;
pub mod compose;
pub mod encryption;
pub mod explorer;
pub mod firewall;
pub mod groups;
pub mod mdns;
pub mod monitor_layout;
pub mod network;
pub mod notifications;
pub mod weather_location;
pub mod permissions;
pub mod transfer_quotas;
pub mod ransomware;
pub mod rescue;
pub mod progress;
pub mod rclone;
pub mod rag;
pub mod restic;
pub mod security;
pub mod settings;
pub mod share_links;
pub mod shares;
pub mod snmp;
pub mod spa;
pub mod syncthing;
pub mod tls;
pub mod storage;
pub mod system;
pub mod terminal_pty;
pub mod ups;
pub mod users;
pub mod users_models;
pub mod user_homes;
pub mod vm;
pub mod vpn;
pub mod widgets;
pub mod journal;

// Selective re-exports to prevent name clashes across handler modules
pub use auth::*;
pub use dashboard::*;
pub use settings::{handle_version_info, *};
pub use snmp::*;
pub use storage::{handle_system_disk, *};
pub use system::*;
pub use ups::*;
pub use users::*;
pub use users_models::*;
pub use user_homes::*;
pub use user_homes::AvailableHomeDisksResponse;

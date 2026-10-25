/* =============================================================================
 * File          : mod.rs
 * Author        : Nicolas Le Roux
 * Created       : 14/10/2026
 * Last modified : 14/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   The built-in WebDAV server: an in-process axum listener started,
 *   stopped and rebuilt on the configured port. It replaces the generated
 *   Python script + systemd unit, so WebDAV now runs inside HomeVault —
 *   no root, no `homevault-webdav.service`, and the share table plus
 *   per-member grants are enforced on every request.
 *
 * Dependencies:
 *   - axum (>= 0.8), tokio (>= 1.52), dav-server (>= 0.11)
 *
 * Usage:
 *   `WEBDAV.reload()` is called at startup and after every config or share
 *   change; `SharesService::start/stop/restart_service(Webdav)` forwards to
 *   `start`/`stop`/`reload` here.
 * ============================================================================= */

//! Built-in WebDAV server manager (in-process listener).

pub mod fs;
pub mod serve;
pub mod vfs;

use crate::handlers::AppState;
use crate::shares::{ShareType, SharesService};
use once_cell::sync::Lazy;
use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::Arc;
use tokio::sync::{oneshot, Mutex, RwLock};

/// Port the server is listening on, or 0 when stopped. Kept as a plain atomic
/// so synchronous callers (mDNS advertisement) can read it without a runtime.
static RUNNING_PORT: AtomicU16 = AtomicU16::new(0);

/// The port the WebDAV server is bound to, or `None` when stopped.
pub fn running_port() -> Option<u16> {
    match RUNNING_PORT.load(Ordering::Relaxed) {
        0 => None,
        p => Some(p),
    }
}

/// A running listener plus the handle used to shut it down.
struct Running {
    port: u16,
    shutdown: Shutdown,
}

/// How to stop a running listener — plain axum uses a oneshot, the TLS
/// listener (axum-server) uses its own graceful-shutdown handle.
enum Shutdown {
    Plain(oneshot::Sender<()>),
    Tls(axum_server::Handle<std::net::SocketAddr>),
}

pub struct WebdavManager {
    inner: RwLock<Option<Running>>,
    /// Serialises start/stop/reload so two concurrent config saves cannot
    /// leave two listeners (or a half-torn-down one) behind.
    gate: Mutex<()>,
}

impl WebdavManager {
    fn new() -> Self {
        Self {
            inner: RwLock::new(None),
            gate: Mutex::new(()),
        }
    }

    /// The port currently bound, if any.
    pub async fn bound_port(&self) -> Option<u16> {
        self.inner.read().await.as_ref().map(|r| r.port)
    }

    /// Starts the listener, or restarts it when already running.
    ///
    /// The config and the share table are re-read from disk every time, so a
    /// call after a share change picks the new state up without a separate
    /// invalidation path. When no enabled WebDAV share exists the server is
    /// stopped and `Ok(0)` is returned — nothing is listening.
    pub async fn reload(self: &Arc<Self>, state: &Arc<AppState>) -> Result<u16, String> {
        let _gate = self.gate.lock().await;
        self.stop_locked().await?;
        self.start_locked(state).await
    }

    /// Starts the listener if it is not running yet. No-op when running.
    pub async fn start(self: &Arc<Self>, state: &Arc<AppState>) -> Result<u16, String> {
        let _gate = self.gate.lock().await;
        if let Some(running) = self.inner.read().await.as_ref() {
            return Ok(running.port);
        }
        self.start_locked(state).await
    }

    /// Stops the listener. Idempotent.
    pub async fn stop(&self) -> Result<(), String> {
        let _gate = self.gate.lock().await;
        self.stop_locked().await
    }

    /// Shared start path — caller must hold `gate`.
    async fn start_locked(self: &Arc<Self>, state: &Arc<AppState>) -> Result<u16, String> {
        let shares = shares_from_disk();
        if shares.is_empty() {
            tracing::info!("WebDAV: no enabled share, server stays stopped");
            return Ok(0);
        }

        let config = SharesService::get_webdav_config().unwrap_or_default();
        if config.port == 0 {
            return Err("WebDAV port 0 is not a usable listen port".to_string());
        }

        let dav = serve::build_handler(shares, &config);
        let ctx = Arc::new(serve::WebdavCtx {
            state: state.clone(),
            dav,
            config: config.clone(),
        });
        let app = serve::router(ctx);
        let addr: std::net::SocketAddr = ([0, 0, 0, 0], config.port).into();

        // TLS is optional and reuses the certificate HomeVault already has
        // (Settings > Security > HTTPS/TLS). When it is required but no
        // certificate is available the server refuses to start rather than
        // silently serving plain HTTP.
        let (port, shutdown) = if config.require_ssl {
            let tls = crate::tls::load_settings();
            if !tls.enabled {
                return Err(
                    "WebDAV requires TLS but HTTPS is not enabled in Settings > Security"
                        .to_string(),
                );
            }
            let rustls_config = crate::tls::build_rustls_config(&tls).await?;
            let handle = axum_server::Handle::new();
            let serve_handle = handle.clone();
            tokio::spawn(async move {
                if let Err(e) = axum_server::bind_rustls(addr, rustls_config)
                    .handle(serve_handle)
                    .serve(app.into_make_service_with_connect_info::<std::net::SocketAddr>())
                    .await
                {
                    tracing::error!("WebDAV TLS listener on :{} stopped: {e}", addr.port());
                }
            });
            (config.port, Shutdown::Tls(handle))
        } else {
            let listener = tokio::net::TcpListener::bind(addr)
                .await
                .map_err(|e| format!("could not bind WebDAV port {}: {e}", config.port))?;
            let bound = listener
                .local_addr()
                .map(|a| a.port())
                .unwrap_or(config.port);
            let (tx, rx) = oneshot::channel::<()>();
            tokio::spawn(async move {
                let server = axum::serve(
                    listener,
                    app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
                )
                .with_graceful_shutdown(async move {
                    let _ = rx.await;
                });
                if let Err(e) = server.await {
                    tracing::error!("WebDAV listener on :{bound} stopped: {e}");
                }
            });
            (bound, Shutdown::Plain(tx))
        };

        *self.inner.write().await = Some(Running { port, shutdown });
        RUNNING_PORT.store(port, Ordering::Relaxed);
        tracing::info!("WebDAV server listening on :{port}");
        Ok(port)
    }

    /// Shared stop path — caller must hold `gate`.
    async fn stop_locked(&self) -> Result<(), String> {
        if let Some(running) = self.inner.write().await.take() {
            match running.shutdown {
                Shutdown::Plain(tx) => {
                    let _ = tx.send(());
                }
                Shutdown::Tls(handle) => {
                    handle.graceful_shutdown(Some(std::time::Duration::from_secs(5)));
                }
            }
            tracing::info!("WebDAV server on :{} stopped", running.port);
        }
        RUNNING_PORT.store(0, Ordering::Relaxed);
        Ok(())
    }
}

/// Global WebDAV manager, mirroring `APP_PROXY`.
pub static WEBDAV: Lazy<Arc<WebdavManager>> = Lazy::new(|| Arc::new(WebdavManager::new()));

/// The WebDAV shares currently defined on disk that should be served.
fn shares_from_disk() -> Vec<fs::WebdavShare> {
    match SharesService::get_shares() {
        Ok(all) => {
            let webdav: Vec<_> = all
                .into_iter()
                .filter(|s| s.share_type == ShareType::Webdav)
                .collect();
            fs::servable_shares(webdav)
        }
        Err(e) => {
            tracing::warn!("WebDAV: could not read the share table: {e}");
            Vec::new()
        }
    }
}

/// Called after a share or config change so the listener reflects it.
///
/// A change while stopped stays stopped: only a *running* server is reloaded,
/// so editing a share never starts a service the admin had turned off.
pub async fn reload_if_running(state: &Arc<AppState>) {
    if WEBDAV.bound_port().await.is_some() {
        if let Err(e) = WEBDAV.reload(state).await {
            tracing::warn!("WebDAV: reload after change failed: {e}");
        }
    }
}

/// Best-effort removal of the legacy generated service, so an upgraded host
/// does not keep a root-owned Python WebDAV server listening alongside the
/// in-process one. Idempotent and silent when the files are absent.
pub fn cleanup_legacy_service() {
    use crate::utils::process::run_timed;
    use std::process::Command;
    use std::time::Duration;

    /// Deadline on one removal step. This runs at startup, before the HTTP
    /// listener is up, so an unbounded `sudo` here delays the whole appliance
    /// (ROADMAP X.9). Four steps, so the worst case is bounded and named.
    const LEGACY_CLEANUP_TIMEOUT: Duration = Duration::from_secs(60);

    const PY: &str = "/usr/local/bin/homevault-webdav.py";
    const UNIT: &str = "/etc/systemd/system/homevault-webdav.service";
    if !std::path::Path::new(UNIT).exists() && !std::path::Path::new(PY).exists() {
        return;
    }

    let _ = run_timed(
        Command::new("sudo").args(["systemctl", "disable", "--now", "homevault-webdav"]),
        LEGACY_CLEANUP_TIMEOUT,
    );
    let _ = run_timed(
        Command::new("sudo").args(["rm", "-f", UNIT]),
        LEGACY_CLEANUP_TIMEOUT,
    );
    let _ = run_timed(
        Command::new("sudo").args(["rm", "-f", PY]),
        LEGACY_CLEANUP_TIMEOUT,
    );
    let _ = run_timed(
        Command::new("sudo").args(["systemctl", "daemon-reload"]),
        LEGACY_CLEANUP_TIMEOUT,
    );
    tracing::info!("WebDAV: removed the legacy generated service");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_webdav_share_type_string_is_the_lookup_key() {
        // `shares_from_disk` filters on `ShareType::Webdav`; the API and the
        // grant table both key on the same lowercase string.
        assert_eq!(ShareType::Webdav.as_str(), "webdav");
    }
}

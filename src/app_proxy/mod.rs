/* =============================================================================
 * File          : mod.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 10/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Per-app reverse proxy for installed catalog apps: one listener per
 *   running app on a dedicated port, forwarding to the container's port,
 *   with the optional app-proxy-scoped session gate. The app's own port
 *   stays open on the LAN — the proxy is an additional consistent entry,
 *   not a firewall.
 *
 * Dependencies:
 *   - tokio (>= 1.52)
 *
 * Usage:
 *   Spawned per installed app at startup; see docs/guides/apps-center.md.
 * =============================================================================
 */

//! Per-app reverse proxy for installed Docker catalog apps. Each running
//! app gets its own listener on a dedicated port
//! that forwards to the container's published port, with an optional
//! app-proxy-scoped session gate (see [`app_auth`]). The app's own published
//! port stays open on the LAN — the proxy is an additional, consistent entry
//! point, not a hard lockdown (deliberate: the user asked for the app to
//! remain reachable directly on LAN and the internet).

pub mod app_auth;
pub mod cookie;
pub mod origin;
mod serve;

use once_cell::sync::Lazy;
use serve::ProxyCtx;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use tokio::sync::{oneshot, RwLock};

/// Ports handed out to app proxies. Chosen to sit well clear of the usual
/// service and container port ranges.
const PROXY_PORT_START: u16 = 47000;
const PROXY_PORT_END: u16 = 47999;

pub static APP_PROXY: Lazy<AppProxyManager> = Lazy::new(AppProxyManager::new);

struct ProxyEntry {
    proxy_port: u16,
    target_port: u16,
    auth_required: Arc<AtomicBool>,
    shutdown: Option<oneshot::Sender<()>>,
}

/// Public view of one running proxy, for the catalog API.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ProxyInfo {
    pub proxy_port: u16,
    pub target_port: u16,
    pub auth_required: bool,
}

pub struct AppProxyManager {
    entries: RwLock<HashMap<String, ProxyEntry>>,
    main_port: OnceLock<u16>,
}

impl AppProxyManager {
    fn new() -> Self {
        Self {
            entries: RwLock::new(HashMap::new()),
            main_port: OnceLock::new(),
        }
    }

    /// Called once at startup with the main port, for building the
    /// `/app-auth` redirect from an unauthenticated proxy request.
    pub fn init(&self, main_port: u16) {
        let _ = self.main_port.set(main_port);
    }

    /// Current proxy for an app, if running.
    pub async fn info(&self, ns_id: &str) -> Option<ProxyInfo> {
        self.entries.read().await.get(ns_id).map(|e| ProxyInfo {
            proxy_port: e.proxy_port,
            target_port: e.target_port,
            auth_required: e.auth_required.load(Ordering::Relaxed),
        })
    }

    /// All running proxies, keyed by namespaced app id.
    pub async fn snapshot(&self) -> HashMap<String, ProxyInfo> {
        self.entries
            .read()
            .await
            .iter()
            .map(|(k, e)| {
                (
                    k.clone(),
                    ProxyInfo {
                        proxy_port: e.proxy_port,
                        target_port: e.target_port,
                        auth_required: e.auth_required.load(Ordering::Relaxed),
                    },
                )
            })
            .collect()
    }

    pub async fn set_auth_required(&self, ns_id: &str, required: bool) -> bool {
        if let Some(e) = self.entries.read().await.get(ns_id) {
            e.auth_required.store(required, Ordering::Relaxed);
            true
        } else {
            false
        }
    }

    /// Stop and drop the proxy for an app, revoking **its** app sessions so a
    /// stopped/removed app can't be reached with a still-valid cookie.
    ///
    /// Scoped to `ns_id` on purpose: revoking every app session here logged
    /// the whole household out of every other proxied app whenever one app was
    /// restarted or uninstalled.
    pub async fn stop(&self, ns_id: &str) {
        if let Some(mut e) = self.entries.write().await.remove(ns_id) {
            if let Some(tx) = e.shutdown.take() {
                let _ = tx.send(());
            }
        }
        app_auth::APP_AUTH.revoke_app(ns_id);
    }

    /// Start (or restart) a proxy for `ns_id` forwarding to
    /// `127.0.0.1:target_port`. Returns the chosen proxy port. If
    /// `preferred_port` is set and still free it is reused (so a restart
    /// keeps a stable URL).
    pub async fn start(
        &self,
        ns_id: &str,
        target_port: u16,
        auth_required: bool,
        preferred_port: Option<u16>,
    ) -> Result<u16, String> {
        let main_port = self.main_port.get().copied().unwrap_or(8080);

        // Drop any existing proxy for this app first.
        self.stop(ns_id).await;

        let in_use: Vec<u16> = self
            .entries
            .read()
            .await
            .values()
            .map(|e| e.proxy_port)
            .collect();

        let port = self
            .pick_port(preferred_port, &in_use)
            .ok_or_else(|| "no free app-proxy port available".to_string())?;

        let listener = tokio::net::TcpListener::bind(("0.0.0.0", port))
            .await
            .map_err(|e| format!("could not bind proxy port {port}: {e}"))?;

        let auth_flag = Arc::new(AtomicBool::new(auth_required));
        let ctx = Arc::new(ProxyCtx {
            ns_id: ns_id.to_string(),
            target_port,
            auth_required: auth_flag.clone(),
            main_port,
        });

        let (tx, mut rx) = oneshot::channel::<()>();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = &mut rx => break,
                    accepted = listener.accept() => {
                        let Ok((stream, _)) = accepted else { continue };
                        let ctx = ctx.clone();
                        tokio::spawn(async move {
                            let io = hyper_util::rt::TokioIo::new(stream);
                            let service = hyper::service::service_fn(move |req| {
                                serve::handle(ctx.clone(), req)
                            });
                            let _ = hyper::server::conn::http1::Builder::new()
                                .serve_connection(io, service)
                                .with_upgrades()
                                .await;
                        });
                    }
                }
            }
        });

        self.entries.write().await.insert(
            ns_id.to_string(),
            ProxyEntry {
                proxy_port: port,
                target_port,
                auth_required: auth_flag,
                shutdown: Some(tx),
            },
        );
        Ok(port)
    }

    fn pick_port(&self, preferred: Option<u16>, in_use: &[u16]) -> Option<u16> {
        let free =
            |p: u16| !in_use.contains(&p) && crate::docker::DockerService::check_port(p).available;
        if let Some(p) = preferred {
            if (PROXY_PORT_START..=PROXY_PORT_END).contains(&p) && free(p) {
                return Some(p);
            }
        }
        (PROXY_PORT_START..=PROXY_PORT_END).find(|&p| free(p))
    }
}

/// One installed app to (re)start a proxy for, from its `.install.toml`.
pub struct RestoreEntry {
    pub ns_id: String,
    pub target_port: u16,
    pub auth_required: bool,
    pub proxy_port: Option<u16>,
}

/// At startup, bring proxies back up for every installed app whose marker
/// records a proxy. Best-effort — a failure for one app is logged, not
/// fatal.
pub async fn restore_all(entries: Vec<RestoreEntry>) {
    for e in entries {
        match APP_PROXY
            .start(&e.ns_id, e.target_port, e.auth_required, e.proxy_port)
            .await
        {
            Ok(p) => tracing::info!("app proxy for {} listening on :{}", e.ns_id, p),
            Err(err) => tracing::warn!("could not restore app proxy for {}: {}", e.ns_id, err),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pick_port_prefers_and_ranges() {
        let mgr = AppProxyManager::new();
        // A preferred port outside the range is ignored.
        let p = mgr.pick_port(Some(80), &[]);
        assert!(p.is_some_and(|p| (PROXY_PORT_START..=PROXY_PORT_END).contains(&p)));
        // An in-use port is skipped.
        let taken = mgr.pick_port(None, &[]).unwrap();
        let next = mgr.pick_port(None, &[taken]).unwrap();
        assert_ne!(taken, next);
    }
}

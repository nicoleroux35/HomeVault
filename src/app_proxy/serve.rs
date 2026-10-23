/* =============================================================================
 * File          : serve.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 21/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   The per-app proxy listener: one hyper HTTP/1 server per installed app
 *   forwarding to the container's published port on loopback. When the app
 *   requires auth, a request without a valid app-proxy session is 302'd to
 *   /app-auth to obtain one.
 *
 * Dependencies:
 *   - hyper (>= 1)
 *   - tokio (>= 1.52)
 *
 * Usage:
 *   Spawned by app_proxy/mod.rs for each app with a running proxy.
 * =============================================================================
 */

//! The per-app proxy: one hyper 1.x HTTP/1 listener per installed catalog
//! app, forwarding to the container's published port on `127.0.0.1`.
//!
//! - Optional auth gate: when the app has `auth_required`, a request with no
//!   valid **app-proxy session** (`hv_app_session`) gets a 302 to the
//!   server-chosen entry `GET /app-auth?app=<ns_id>` on the main origin, which
//!   validates the real API session and mints the app session. The API session
//!   cookie (`hv_session`) is *never* trusted here, and both HomeVault cookies
//!   are stripped from the request before it is forwarded upstream, so an
//!   installed app never sees either credential. The app's own published port
//!   stays open on the LAN, so this gate is a convenience, not a hard boundary
//!   (documented tradeoff).
//! - `Upgrade` requests (WebSockets) are passed through: the proxy relays
//!   the 101 and then splices the two upgraded byte streams.

use crate::app_proxy::app_auth::APP_AUTH;
use crate::app_proxy::cookie::{
    app_session_from_cookie_header, strip_homevault_cookies, SESSION_COOKIE,
};
use http_body_util::{combinators::BoxBody, BodyExt, Empty, Full};
use hyper::body::{Bytes, Incoming};
use hyper::header::{HeaderValue, CONNECTION, COOKIE, HOST, LOCATION, UPGRADE};
use hyper::{Request, Response, StatusCode};
use std::convert::Infallible;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

type BoxedBody = BoxBody<Bytes, Box<dyn std::error::Error + Send + Sync>>;

fn text(status: StatusCode, msg: &str) -> Response<BoxedBody> {
    Response::builder()
        .status(status)
        .header("content-type", "text/plain; charset=utf-8")
        .body(
            Full::new(Bytes::from(msg.to_owned()))
                .map_err(|e| Box::new(e) as _)
                .boxed(),
        )
        .unwrap()
}

/// Shared per-proxy config the connection handler reads on every request.
pub struct ProxyCtx {
    /// Namespaced app id, used for the `/app-auth` redirect and log context.
    pub ns_id: String,
    pub target_port: u16,
    pub auth_required: Arc<AtomicBool>,
    /// The main HomeVault port, for building the app-auth redirect.
    pub main_port: u16,
}

/// Handle one proxied request.
pub async fn handle(
    ctx: Arc<ProxyCtx>,
    mut req: Request<Incoming>,
) -> Result<Response<BoxedBody>, Infallible> {
    // ── Auth gate ──────────────────────────────────────────────────────────
    if ctx.auth_required.load(Ordering::Relaxed) {
        // Only the app-proxy-scoped session is accepted here — never the
        // full API session token. `APP_AUTH.validate` returns the owning
        // username, which we don't otherwise need (the proxy is per-app).
        let authed = req
            .headers()
            .get(COOKIE)
            .and_then(|v| v.to_str().ok())
            .and_then(app_session_from_cookie_header)
            .and_then(|tok| APP_AUTH.validate(tok, &ctx.ns_id));

        if authed.is_none() {
            // Redirect to the server-chosen app-auth entry on the main
            // origin. The host comes from the server's own identity, never
            // from the `Host` header as-is: a forged header (or a proxy in
            // front) must not be able to aim this redirect anywhere else.
            let host = crate::app_proxy::origin::resolve_redirect_host(
                req.headers().get(HOST).and_then(|v| v.to_str().ok()),
            );
            // The redirect must land on the main origin the browser actually
            // holds `hv_session` on. Under an enabled HTTPS listener that
            // cookie is `Secure`, so pointing at the plain-HTTP main port
            // would loop forever — the browser never re-sends the cookie
            // there. Computed per request because TLS can be turned on after
            // this proxy was spawned.
            let tls = crate::tls::tls_active();
            let tls_port = if tls {
                crate::tls::load_settings().effective_https_port()
            } else {
                0
            };
            let location = app_auth_location(ctx.main_port, tls, tls_port, &host, &ctx.ns_id);
            let mut resp = text(StatusCode::FOUND, "Authentication required");
            if let Ok(value) = HeaderValue::from_str(&location) {
                resp.headers_mut().insert(LOCATION, value);
            }
            resp.headers_mut().insert(
                hyper::header::CACHE_CONTROL,
                HeaderValue::from_static("no-store"),
            );
            return Ok(resp);
        }
    }

    // Strip HomeVault credentials before the request reaches the app. We do
    // this unconditionally (auth gate on or off) so `hv_session` can never
    // leak upstream. Other cookies (the app's own) are preserved.
    strip_upstream_cookies(&mut req);

    // ── Build the upstream request ─────────────────────────────────────────
    let path_and_query = req
        .uri()
        .path_and_query()
        .map(|p| p.as_str())
        .unwrap_or("/")
        .to_string();
    let upstream_uri: hyper::Uri =
        match format!("http://127.0.0.1:{}{}", ctx.target_port, path_and_query).parse() {
            Ok(u) => u,
            Err(_) => return Ok(text(StatusCode::BAD_GATEWAY, "bad upstream uri")),
        };

    let is_upgrade = req
        .headers()
        .get(CONNECTION)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.to_ascii_lowercase().contains("upgrade"))
        .unwrap_or(false)
        && req.headers().contains_key(UPGRADE);

    // Connect to the container.
    let stream = match tokio::net::TcpStream::connect(("127.0.0.1", ctx.target_port)).await {
        Ok(s) => s,
        Err(_) => return Ok(text(StatusCode::BAD_GATEWAY, "app is not reachable")),
    };
    let io = hyper_util::rt::TokioIo::new(stream);
    let (mut sender, conn) = match hyper::client::conn::http1::handshake(io).await {
        Ok(pair) => pair,
        Err(_) => return Ok(text(StatusCode::BAD_GATEWAY, "upstream handshake failed")),
    };

    if is_upgrade {
        tokio::spawn(async move {
            if let Err(e) = conn.with_upgrades().await {
                tracing::debug!("app proxy: upstream conn task ended: {e}");
            }
        });

        // Origin-form URI (single known upstream); keep `req` alive for the
        // client-side upgrade. Empty body — an Upgrade handshake has none.
        rewrite_host(&mut req, ctx.target_port);
        let path_only: hyper::Uri = req
            .uri()
            .path_and_query()
            .map(|p| p.as_str())
            .unwrap_or("/")
            .parse()
            .unwrap_or_else(|_| upstream_uri.clone());
        let mut upstream_req = Request::builder()
            .method(req.method().clone())
            .uri(path_only);
        for (k, v) in req.headers().iter() {
            upstream_req = upstream_req.header(k, v);
        }
        let empty_body: BoxedBody = Empty::<Bytes>::new().map_err(|e| Box::new(e) as _).boxed();
        let upstream_req = match upstream_req.body(empty_body) {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!("app proxy: bad upstream upgrade request: {e}");
                return Ok(text(StatusCode::BAD_GATEWAY, "bad upstream request"));
            }
        };

        let upstream_resp = match sender.send_request(upstream_req).await {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!("app proxy: upstream upgrade send failed: {e}");
                return Ok(text(StatusCode::BAD_GATEWAY, "upstream error"));
            }
        };

        if upstream_resp.status() == StatusCode::SWITCHING_PROTOCOLS {
            let status = upstream_resp.status();
            let headers = upstream_resp.headers().clone();
            let upstream_upgraded = hyper::upgrade::on(upstream_resp);
            let client_upgraded = hyper::upgrade::on(&mut req);

            tokio::spawn(async move {
                if let (Ok(a), Ok(b)) = (client_upgraded.await, upstream_upgraded.await) {
                    let mut a = hyper_util::rt::TokioIo::new(a);
                    let mut b = hyper_util::rt::TokioIo::new(b);
                    let _ = tokio::io::copy_bidirectional(&mut a, &mut b).await;
                }
            });

            let mut resp = Response::builder().status(status);
            for (k, v) in headers.iter() {
                resp = resp.header(k, v);
            }
            return Ok(resp
                .body(Empty::<Bytes>::new().map_err(|e| Box::new(e) as _).boxed())
                .unwrap());
        }

        // Not actually an upgrade — relay the plain response.
        return Ok(relay_response(upstream_resp));
    }

    // ── Plain HTTP forward ────────────────────────────────────────────────
    tokio::spawn(async move {
        let _ = conn.await;
    });
    rewrite_host(&mut req, ctx.target_port);
    let (mut parts, body) = req.into_parts();
    parts.uri = upstream_uri;
    let boxed = body
        .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)
        .boxed();
    let upstream_req = Request::from_parts(parts, boxed);

    match sender.send_request(upstream_req).await {
        Ok(resp) => Ok(relay_response(resp)),
        Err(_) => Ok(text(StatusCode::BAD_GATEWAY, "upstream error")),
    }
}

/// Location of the server-chosen `/app-auth` entry on the main origin.
///
/// Pure builder so both TLS states are unit-testable. Plain HTTP targets
/// `main_port`; with TLS enabled the target is the actual HTTPS entry,
/// because `hv_session` is `Secure` there and would never be re-sent over
/// plain HTTP (see the call site). `host` must already be resolved through
/// [`crate::app_proxy::origin`] (never a raw `Host` header). `ns_id` is
/// percent-encoded for the query.
pub fn app_auth_location(
    main_port: u16,
    tls_active: bool,
    tls_https_port: u16,
    host: &str,
    ns_id: &str,
) -> String {
    let (scheme, port) = if tls_active {
        ("https", tls_https_port)
    } else {
        ("http", main_port)
    };
    let encoded = url::form_urlencoded::byte_serialize(ns_id.as_bytes()).collect::<String>();
    format!(
        "{scheme}://{}/app-auth?app={encoded}",
        crate::app_proxy::origin::authority(host, port)
    )
}

fn rewrite_host<B>(req: &mut Request<B>, target_port: u16) {
    if let Ok(v) = HeaderValue::from_str(&format!("127.0.0.1:{target_port}")) {
        req.headers_mut().insert(HOST, v);
    }
}

/// Remove `hv_session` and `hv_app_session` from the request's `Cookie` header
/// before it is forwarded upstream. Other cookies the app itself uses are kept.
/// When nothing is left, the header is dropped entirely.
fn strip_upstream_cookies<B>(req: &mut Request<B>) {
    let Some(raw) = req.headers().get(COOKIE).and_then(|v| v.to_str().ok()) else {
        return;
    };
    // Only rewrite when a HomeVault credential is actually present, so the
    // common case (no cookies) doesn't allocate.
    if !raw.contains(SESSION_COOKIE) {
        return;
    }
    let stripped = strip_homevault_cookies(raw);
    match stripped {
        Some(v) => {
            if let Ok(hv) = HeaderValue::from_str(&v) {
                req.headers_mut().insert(COOKIE, hv);
            } else {
                req.headers_mut().remove(COOKIE);
            }
        }
        None => {
            req.headers_mut().remove(COOKIE);
        }
    }
}

fn relay_response(resp: Response<Incoming>) -> Response<BoxedBody> {
    let (parts, body) = resp.into_parts();
    let boxed = body
        .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)
        .boxed();
    Response::from_parts(parts, boxed)
}

/// An empty boxed body, for completeness where a body is required.
#[allow(dead_code)]
fn empty() -> BoxedBody {
    Empty::<Bytes>::new().map_err(|e| Box::new(e) as _).boxed()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_app_auth_location_http_main_origin() {
        assert_eq!(
            app_auth_location(8080, false, 0, "homevault.lan", "default:joplin"),
            "http://homevault.lan:8080/app-auth?app=default%3Ajoplin"
        );
    }

    #[test]
    fn test_app_auth_location_follows_tls_entry() {
        // TLS active: the redirect must target the HTTPS entry — that is where
        // the `Secure` `hv_session` cookie the browser holds actually travels.
        assert_eq!(
            app_auth_location(8080, true, 8443, "homevault.lan", "default:immich"),
            "https://homevault.lan:8443/app-auth?app=default%3Aimmich"
        );
    }

    #[test]
    fn test_app_auth_location_custom_https_port() {
        assert_eq!(
            app_auth_location(81, true, 9005, "location.host", "home:docmost"),
            "https://location.host:9005/app-auth?app=home%3Adocmost"
        );
    }

    #[test]
    fn test_app_auth_location_encodes_namespaces() {
        assert_eq!(
            app_auth_location(8080, false, 0, "h", "store two:app&1"),
            "http://h:8080/app-auth?app=store+two%3Aapp%261"
        );
    }

    /// An IPv6 main origin stays a valid authority (`[host]:port`), not a
    /// malformed `::1:8080`.
    #[test]
    fn test_app_auth_location_brackets_ipv6_host() {
        assert_eq!(
            app_auth_location(8080, false, 0, "[fd00::1]", "default:joplin"),
            "http://[fd00::1]:8080/app-auth?app=default%3Ajoplin"
        );
    }
}

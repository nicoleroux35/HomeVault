//! The per-app proxy: one hyper 1.x HTTP/1 listener per installed catalog
//! app, forwarding to the container's published port on `127.0.0.1`.
//!
//! - Optional auth gate: when the app has `auth_required`, a request with
//!   no valid `hv_session` cookie gets a 302 to the main HomeVault
//!   `/login`. The app's own published port stays open on the LAN, so this
//!   gate is a convenience, not a hard boundary (documented tradeoff).
//! - `Upgrade` requests (WebSockets) are passed through: the proxy relays
//!   the 101 and then splices the two upgraded byte streams.

use crate::app_proxy::cookie::session_from_cookie_header;
use crate::auth::AuthService;
use http_body_util::{combinators::BoxBody, BodyExt, Empty, Full};
use hyper::body::{Bytes, Incoming};
use hyper::header::{HeaderValue, CONNECTION, HOST, LOCATION, UPGRADE};
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
    pub target_port: u16,
    pub auth_required: Arc<AtomicBool>,
    pub auth: AuthService,
    /// The main HomeVault port, for building the login redirect.
    pub main_port: u16,
}

/// Handle one proxied request.
pub async fn handle(
    ctx: Arc<ProxyCtx>,
    mut req: Request<Incoming>,
) -> Result<Response<BoxedBody>, Infallible> {
    // ── Auth gate ──────────────────────────────────────────────────────────
    if ctx.auth_required.load(Ordering::Relaxed) {
        let authed = req
            .headers()
            .get(hyper::header::COOKIE)
            .and_then(|v| v.to_str().ok())
            .and_then(session_from_cookie_header)
            .map(|tok| tok.to_string());

        let ok = match authed {
            Some(tok) => ctx.auth.get_username_from_token(&tok).await.is_some(),
            None => false,
        };
        if !ok {
            // Redirect to the main UI's login, preserving the host the
            // browser used (just swapping the port).
            let host = req
                .headers()
                .get(HOST)
                .and_then(|v| v.to_str().ok())
                .map(|h| h.split(':').next().unwrap_or(h).to_string())
                .unwrap_or_else(|| "localhost".to_string());
            let location = format!("http://{host}:{}/login", ctx.main_port);
            let mut resp = text(StatusCode::FOUND, "Authentication required");
            resp.headers_mut()
                .insert(LOCATION, HeaderValue::from_str(&location).unwrap());
            return Ok(resp);
        }
    }

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

fn rewrite_host<B>(req: &mut Request<B>, target_port: u16) {
    if let Ok(v) = HeaderValue::from_str(&format!("127.0.0.1:{target_port}")) {
        req.headers_mut().insert(HOST, v);
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

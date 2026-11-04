/* =============================================================================
 * File          : proxy.rs
 * Author        : Nicolas Le Roux
 * Created       : 06/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   One request through the generic reverse proxy: resolve the rule, gate it,
 *   discard every client-supplied forwarding header, dial a private upstream,
 *   relay the response (and a WebSocket upgrade, when there is one).
 *
 *   The gate is the cross-host ticket exchange described in
 *   docs/analysis/c2-reverse-proxy-design.md: the listener validates only the
 *   rule-scoped `hv_app_session`; `GET /proxy-auth` on the dashboard origin is
 *   the only place the real `hv_session` is read, and it hands back a
 *   single-use ticket instead of a cookie.
 *
 * Dependencies:
 *   - hyper (>= 1), hyper-util, http-body-util, tokio-rustls, rustls
 *
 * Usage:
 *   Spawned by `ReverseProxyManager::bind` for every accepted connection.
 * ============================================================================= */

//! The host-routed listener's request path: match, gate, sanitise, forward.

use crate::app_proxy::app_auth::APP_AUTH;
use crate::app_proxy::cookie::{
    app_session_cookie, app_session_from_cookie_header, strip_homevault_cookies,
};
use crate::app_proxy::origin;
use crate::reverse_proxy::ticket::TICKETS;
use crate::reverse_proxy::{ns_id, select_rule, ProxyRule, ReverseProxyManager, Upstream};
use http_body_util::{combinators::BoxBody, BodyExt, Empty, Full};
use hyper::body::{Bytes, Incoming};
use hyper::header::{HeaderValue, CONNECTION, COOKIE, HOST, LOCATION, SET_COOKIE, UPGRADE};
use hyper::{Request, Response, StatusCode};
use once_cell::sync::Lazy;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use super::AUTH_CALLBACK_PATH;

type BoxedBody = BoxBody<Bytes, Box<dyn std::error::Error + Send + Sync>>;

/// Dial timeout. A LAN service that is down should answer `502`, not hold a
/// browser tab open for the kernel's default minute.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Header names a client is never allowed to hand to an upstream. They either
/// assert a client identity (`X-Forwarded-*`, `Forwarded`, `X-Real-IP`) or
/// rewrite the request server-side (`X-Rewrite-URL`, method override). The
/// proxy sets its own values from the socket after removing all of them.
const FORGED_HEADERS: &[&str] = &[
    "forwarded",
    "x-forwarded-for",
    "x-forwarded-host",
    "x-forwarded-proto",
    "x-forwarded-port",
    "x-real-ip",
    "x-original-forwarded-for",
    "x-rewrite-url",
    "x-http-method-override",
    "x-method-override",
];

/// TLS client config for `tls: true` upstreams: the public root store, no
/// client certificate. A self-signed upstream certificate is refused (a
/// per-rule CA pin is deliberately out of scope for this version).
static UPSTREAM_TLS: Lazy<Option<Arc<rustls::ClientConfig>>> = Lazy::new(|| {
    let mut roots = rustls::RootCertStore::empty();
    for der in webpki_root_certs::TLS_SERVER_ROOT_CERTS {
        let _ = roots.add(der.clone());
    }
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .ok()?
        .with_root_certificates(roots)
        .with_no_client_auth();
    Some(Arc::new(config))
});

trait IoStream: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send {}
impl<T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send> IoStream for T {}

/// Handle one connection's request.
pub async fn handle(
    manager: Arc<ReverseProxyManager>,
    mut req: Request<Incoming>,
    peer: IpAddr,
) -> Response<BoxedBody> {
    let host = match req
        .headers()
        .get(HOST)
        .and_then(|value| value.to_str().ok())
        .and_then(origin::normalize_host)
    {
        Some(host) => host,
        None => return text(StatusCode::BAD_REQUEST, "A Host header is required."),
    };

    let path = req.uri().path().to_string();
    let query = req.uri().query().map(str::to_string);
    let path_and_query = req
        .uri()
        .path_and_query()
        .map(|value| value.as_str().to_string())
        .unwrap_or_else(|| "/".to_string());

    let rules = manager.rules_snapshot().await;

    // The ticket callback is served by the listener itself and must be handled
    // before rule selection: it is the one path that does not proxy.
    if path == AUTH_CALLBACK_PATH {
        return redeem_ticket(&rules, &host, query.as_deref());
    }

    let Some(rule) = select_rule(&rules, &host, &path) else {
        return text(StatusCode::NOT_FOUND, &format!("No proxy rule for {host}."));
    };
    let rule = rule.clone();

    if rule.auth_required && !is_authenticated(&req, &rule) {
        let next = match query {
            Some(query) => format!("{path}?{query}"),
            None => path,
        };
        let location = auth_entry_location(
            manager.main_port(),
            crate::tls::tls_active(),
            if crate::tls::tls_active() {
                crate::tls::load_settings().effective_https_port()
            } else {
                0
            },
            &origin::resolve_redirect_host(
                req.headers()
                    .get(HOST)
                    .and_then(|value| value.to_str().ok()),
            ),
            rule.id,
            &safe_next_path(&next),
        );
        return redirect(&location, None);
    }

    // ── Never trust an inbound forwarding header ──────────────────────────
    sanitize_headers(req.headers_mut(), peer, "http");
    strip_cookies(req.headers_mut());

    // ── Resolve and dial the upstream ─────────────────────────────────────
    let upstream = match Upstream::parse(&rule.upstream) {
        Ok(upstream) => upstream,
        Err(e) => return text(StatusCode::BAD_GATEWAY, &e),
    };
    let local = manager.local_ips().await;
    let addresses = match super::resolve_upstream(
        &upstream,
        manager.effective_listen_port().await,
        manager.main_port(),
        &local,
    )
    .await
    {
        Ok(addresses) => addresses,
        Err(e) => return text(StatusCode::BAD_GATEWAY, &e),
    };

    let io = match connect(&addresses, &upstream, rule.tls).await {
        Ok(io) => io,
        Err(e) => return text(StatusCode::BAD_GATEWAY, &e),
    };

    let is_upgrade = req
        .headers()
        .get(CONNECTION)
        .and_then(|value| value.to_str().ok())
        .map(|value| value.to_ascii_lowercase().contains("upgrade"))
        .unwrap_or(false)
        && req.headers().contains_key(UPGRADE);

    let io = hyper_util::rt::TokioIo::new(io);
    let (mut sender, conn) = match hyper::client::conn::http1::handshake(io).await {
        Ok(pair) => pair,
        Err(e) => {
            return text(
                StatusCode::BAD_GATEWAY,
                &format!("The upstream handshake failed: {e}"),
            )
        }
    };

    // Rewrite `Host` to the upstream authority; the rule's host is ours, not
    // the upstream's, and a virtual host behind the upstream needs its own.
    if let Ok(value) = HeaderValue::from_str(&upstream.authority()) {
        req.headers_mut().insert(HOST, value);
    }
    let origin_form: hyper::Uri = path_and_query
        .parse()
        .unwrap_or_else(|_| hyper::Uri::from_static("/"));

    if is_upgrade {
        tokio::spawn(async move {
            if let Err(e) = conn.with_upgrades().await {
                tracing::debug!("reverse proxy: upstream connection ended: {e}");
            }
        });

        let mut upstream_req = Request::builder()
            .method(req.method().clone())
            .uri(origin_form);
        for (name, value) in req.headers().iter() {
            upstream_req = upstream_req.header(name, value);
        }
        let empty: BoxedBody = Empty::<Bytes>::new().map_err(|e| Box::new(e) as _).boxed();
        let upstream_req = match upstream_req.body(empty) {
            Ok(request) => request,
            Err(e) => {
                return text(
                    StatusCode::BAD_GATEWAY,
                    &format!("The upstream request could not be built: {e}"),
                )
            }
        };

        let upstream_resp = match sender.send_request(upstream_req).await {
            Ok(response) => response,
            Err(e) => {
                return text(
                    StatusCode::BAD_GATEWAY,
                    &format!("The upstream did not answer: {e}"),
                )
            }
        };

        if upstream_resp.status() == StatusCode::SWITCHING_PROTOCOLS {
            let status = upstream_resp.status();
            let headers = upstream_resp.headers().clone();
            let upstream_upgraded = hyper::upgrade::on(upstream_resp);
            let client_upgraded = hyper::upgrade::on(&mut req);
            tokio::spawn(async move {
                if let (Ok(client), Ok(upstream)) = (client_upgraded.await, upstream_upgraded.await)
                {
                    let mut client = hyper_util::rt::TokioIo::new(client);
                    let mut upstream = hyper_util::rt::TokioIo::new(upstream);
                    let _ = tokio::io::copy_bidirectional(&mut client, &mut upstream).await;
                }
            });

            let mut response = Response::builder().status(status);
            for (name, value) in headers.iter() {
                response = response.header(name, value);
            }
            return response
                .body(Empty::<Bytes>::new().map_err(|e| Box::new(e) as _).boxed())
                .unwrap_or_else(|_| text(StatusCode::BAD_GATEWAY, "The upstream upgrade failed."));
        }
        return relay(upstream_resp);
    }

    tokio::spawn(async move {
        let _ = conn.await;
    });

    let (mut parts, body) = req.into_parts();
    parts.uri = origin_form;
    let boxed = body
        .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)
        .boxed();
    let upstream_req = Request::from_parts(parts, boxed);

    match sender.send_request(upstream_req).await {
        Ok(response) => relay(response),
        Err(e) => text(
            StatusCode::BAD_GATEWAY,
            &format!("The upstream did not answer: {e}"),
        ),
    }
}

/// Is this request already carrying a live session for `rule`?
fn is_authenticated(req: &Request<Incoming>, rule: &ProxyRule) -> bool {
    req.headers()
        .get(COOKIE)
        .and_then(|value| value.to_str().ok())
        .and_then(app_session_from_cookie_header)
        .and_then(|token| APP_AUTH.validate(token, &rule.ns_id()))
        .is_some()
}

/// `__hv_proxy_auth` — redeem a ticket, mint the rule-scoped session on this
/// host, and send the browser on to the path it originally asked for.
fn redeem_ticket(rules: &[ProxyRule], host: &str, query: Option<&str>) -> Response<BoxedBody> {
    let mut rule_id: Option<i64> = None;
    let mut token: Option<String> = None;

    for (key, value) in url::form_urlencoded::parse(query.unwrap_or("").as_bytes()) {
        match key.as_ref() {
            "rule" => rule_id = value.parse::<i64>().ok(),
            "ticket" => token = Some(value.into_owned()),
            // `next` is carried by the ticket, which `/proxy-auth` validated
            // before minting. A query value here is ignored so the ticket is
            // the only source of the redirect target.
            _ => {}
        }
    }

    let Some(rule_id) = rule_id else {
        return text(StatusCode::BAD_REQUEST, "The rule is missing.");
    };
    let Some(token) = token else {
        return text(StatusCode::BAD_REQUEST, "The ticket is missing.");
    };
    // The callback is only valid on the rule's own host: a ticket cannot be
    // replayed against another rule by rewriting the Host header.
    let Some(rule) = rules
        .iter()
        .find(|rule| rule.id == rule_id && rule.host == host && rule.enabled)
    else {
        return text(StatusCode::NOT_FOUND, "No such proxy rule.");
    };

    let Some(ticket) = TICKETS.redeem(&token, rule.id) else {
        return text(
            StatusCode::UNAUTHORIZED,
            "This sign-in link is invalid, expired or already used.",
        );
    };

    let session = APP_AUTH.mint(&ticket.username, &ns_id(rule.id));
    redirect(
        &safe_next_path(&ticket.next),
        Some(&app_session_cookie(&session)),
    )
}

fn text(status: StatusCode, message: &str) -> Response<BoxedBody> {
    Response::builder()
        .status(status)
        .header("content-type", "text/plain; charset=utf-8")
        .body(
            Full::new(Bytes::from(message.to_owned()))
                .map_err(|e| Box::new(e) as _)
                .boxed(),
        )
        .unwrap_or_else(|_| {
            Response::new(Empty::<Bytes>::new().map_err(|e| Box::new(e) as _).boxed())
        })
}

fn redirect(location: &str, cookie: Option<&str>) -> Response<BoxedBody> {
    let mut response = Response::builder()
        .status(StatusCode::FOUND)
        .header("cache-control", "no-store")
        .header("referrer-policy", "no-referrer");
    if let Ok(value) = HeaderValue::from_str(location) {
        response = response.header(LOCATION, value);
    }
    if let Some(cookie) = cookie {
        if let Ok(value) = HeaderValue::from_str(cookie) {
            response = response.header(SET_COOKIE, value);
        }
    }
    response
        .body(Empty::<Bytes>::new().map_err(|e| Box::new(e) as _).boxed())
        .unwrap_or_else(|_| text(StatusCode::INTERNAL_SERVER_ERROR, "Could not redirect."))
}

/// Remove every client-supplied forwarding header, then set the two the
/// upstream may legitimately use from values the proxy itself observed.
pub fn sanitize_headers(headers: &mut hyper::HeaderMap, peer: IpAddr, scheme: &str) {
    for name in FORGED_HEADERS {
        headers.remove(*name);
    }
    if let Ok(value) = HeaderValue::from_str(&peer.to_string()) {
        headers.insert("x-forwarded-for", value);
    }
    if let Ok(value) = HeaderValue::from_str(scheme) {
        headers.insert("x-forwarded-proto", value);
    }
    if let Ok(value) = HeaderValue::from_str(&peer.to_string()) {
        headers.insert("x-real-ip", value);
    }
}

/// Drop both HomeVault cookies, keeping any cookie the upstream itself uses.
pub fn strip_cookies(headers: &mut hyper::HeaderMap) {
    let Some(raw) = headers.get(COOKIE).and_then(|value| value.to_str().ok()) else {
        return;
    };
    match strip_homevault_cookies(raw) {
        Some(stripped) => {
            if let Ok(value) = HeaderValue::from_str(&stripped) {
                headers.insert(COOKIE, value);
            } else {
                headers.remove(COOKIE);
            }
        }
        None => {
            headers.remove(COOKIE);
        }
    }
}

/// A `next` target is only ever a path on the rule's own origin. Anything
/// absolute, scheme-relative or containing control characters becomes `/`.
pub fn safe_next_path(raw: &str) -> String {
    let raw = raw.trim();
    if raw.is_empty()
        || raw.len() > 2048
        || !raw.starts_with('/')
        || raw.starts_with("//")
        || raw.starts_with("/\\")
        || raw
            .chars()
            .any(|c| c.is_control() || c == '\\' || c == '\n' || c == '\r')
    {
        return "/".to_string();
    }
    raw.to_string()
}

/// The dashboard entry a gated request is sent to. The host is already
/// resolved through `app_proxy::origin` (server state, never a raw header).
pub fn auth_entry_location(
    main_port: u16,
    tls_active: bool,
    https_port: u16,
    host: &str,
    rule_id: i64,
    next: &str,
) -> String {
    let (scheme, port) = if tls_active {
        ("https", https_port)
    } else {
        ("http", main_port)
    };
    let next: String = url::form_urlencoded::byte_serialize(next.as_bytes()).collect();
    format!(
        "{scheme}://{}/proxy-auth?rule={rule_id}&next={next}",
        origin::authority(host, port)
    )
}

/// Where `GET /proxy-auth` sends the browser once it has a ticket.
pub fn ticket_callback_location(
    rule_host: &str,
    listen_port: u16,
    rule_id: i64,
    ticket: &str,
    next: &str,
) -> String {
    let next: String = url::form_urlencoded::byte_serialize(next.as_bytes()).collect();
    format!(
        "http://{}{AUTH_CALLBACK_PATH}?rule={rule_id}&ticket={ticket}&next={next}",
        origin::authority(rule_host, listen_port)
    )
}

async fn connect(
    addresses: &[SocketAddr],
    upstream: &Upstream,
    tls: bool,
) -> Result<Box<dyn IoStream>, String> {
    let mut last_error = String::from("The upstream did not answer.");
    for address in addresses {
        let tcp =
            match tokio::time::timeout(CONNECT_TIMEOUT, tokio::net::TcpStream::connect(address))
                .await
            {
                Ok(Ok(stream)) => stream,
                Ok(Err(e)) => {
                    last_error = format!("The upstream {address} refused the connection: {e}");
                    continue;
                }
                Err(_) => {
                    last_error = format!(
                        "The upstream {address} did not answer within {}s.",
                        CONNECT_TIMEOUT.as_secs()
                    );
                    continue;
                }
            };
        let _ = tcp.set_nodelay(true);
        if !tls {
            return Ok(Box::new(tcp));
        }

        let Some(config) = UPSTREAM_TLS.as_ref() else {
            return Err("TLS could not be configured for the upstream.".to_string());
        };
        let bare = upstream.host.trim_matches(['[', ']']).to_string();
        let server_name = match rustls::pki_types::ServerName::try_from(bare.clone()) {
            Ok(name) => name,
            Err(_) => return Err("The upstream host is not a valid TLS name.".to_string()),
        };
        let connector = tokio_rustls::TlsConnector::from(config.clone());
        match tokio::time::timeout(CONNECT_TIMEOUT, connector.connect(server_name, tcp)).await {
            Ok(Ok(stream)) => return Ok(Box::new(stream)),
            Ok(Err(e)) => {
                last_error = format!("The upstream TLS handshake failed: {e}");
            }
            Err(_) => {
                last_error = format!(
                    "The upstream TLS handshake did not finish within {}s.",
                    CONNECT_TIMEOUT.as_secs()
                );
            }
        }
    }
    Err(last_error)
}

fn relay(response: Response<Incoming>) -> Response<BoxedBody> {
    let (parts, body) = response.into_parts();
    let boxed = body
        .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)
        .boxed();
    Response::from_parts(parts, boxed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use hyper::header::HeaderValue;

    #[test]
    fn test_safe_next_path_keeps_relative_paths() {
        assert_eq!(safe_next_path("/"), "/");
        assert_eq!(safe_next_path("/api?x=1"), "/api?x=1");
    }

    #[test]
    fn test_safe_next_path_refuses_absolute_and_scheme_relative() {
        for bad in [
            "https://evil.example/",
            "//evil.example/",
            "evil",
            "/\\evil",
            "/a\nb",
            "",
        ] {
            assert_eq!(safe_next_path(bad), "/", "{bad}");
        }
    }

    #[test]
    fn test_auth_entry_location_is_server_chosen_and_encoded() {
        assert_eq!(
            auth_entry_location(8080, false, 0, "homevault.lan", 7, "/app?x=1"),
            "http://homevault.lan:8080/proxy-auth?rule=7&next=%2Fapp%3Fx%3D1"
        );
        assert_eq!(
            auth_entry_location(8080, true, 8443, "homevault.lan", 7, "/"),
            "https://homevault.lan:8443/proxy-auth?rule=7&next=%2F"
        );
    }

    #[test]
    fn test_ticket_callback_targets_the_stored_rule_host() {
        assert_eq!(
            ticket_callback_location("ha.home.lan", 80, 4, "hvt_x", "/lovelace"),
            "http://ha.home.lan:80/__hv_proxy_auth?rule=4&ticket=hvt_x&next=%2Flovelace"
        );
    }

    #[test]
    fn test_sanitize_headers_discards_forged_identity() {
        let mut headers = hyper::HeaderMap::new();
        headers.insert("x-forwarded-for", HeaderValue::from_static("203.0.113.9"));
        headers.insert("x-real-ip", HeaderValue::from_static("203.0.113.9"));
        headers.insert("forwarded", HeaderValue::from_static("for=203.0.113.9"));
        headers.insert("x-rewrite-url", HeaderValue::from_static("/admin"));

        sanitize_headers(&mut headers, "192.168.1.20".parse().unwrap(), "http");

        assert_eq!(headers.get("x-forwarded-for").unwrap(), "192.168.1.20");
        assert_eq!(headers.get("x-real-ip").unwrap(), "192.168.1.20");
        assert_eq!(headers.get("x-forwarded-proto").unwrap(), "http");
        assert!(headers.get("forwarded").is_none());
        assert!(headers.get("x-rewrite-url").is_none());
    }

    #[test]
    fn test_strip_cookies_never_leaks_a_homevault_credential() {
        let mut headers = hyper::HeaderMap::new();
        headers.insert(
            COOKIE,
            HeaderValue::from_static("hv_session=api; theme=dark; hv_app_session=app"),
        );
        strip_cookies(&mut headers);
        assert_eq!(headers.get(COOKIE).unwrap(), "theme=dark");

        let mut only_hv = hyper::HeaderMap::new();
        only_hv.insert(COOKIE, HeaderValue::from_static("hv_session=api"));
        strip_cookies(&mut only_hv);
        assert!(only_hv.get(COOKIE).is_none());
    }
}

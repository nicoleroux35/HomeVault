/* =============================================================================
 * File          : app_proxy_integration.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 10/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   App proxy integration tests: in-memory SQLite + assembled Axum router, exercising the area's endpoints end-to-end via oneshot requests.
 *
 * Dependencies:
 *   - none (test crate: axum router + tower oneshot / Playwright)
 *
 * Usage:
 *   Rust: cargo test  |  E2E: npx playwright test
 * =============================================================================
 */

//! Integration test for the per-app reverse proxy (`crate::app_proxy`).
//!
//! Spins a tiny echo backend, starts one proxy in front of it via the global
//! `APP_PROXY`, and exercises the hardened app-auth model:
//!
//! - plain forwarding when no auth is required;
//! - the gate rejecting a request with no app session with a 302 to the
//!   server-chosen `/app-auth` entry (never `/login` and never trusting the
//!   API session cookie);
//! - an app-proxy-scoped session (`hv_app_session`) letting the request
//!   through;
//! - both HomeVault cookies (`hv_session`, `hv_app_session`) being stripped
//!   from the request before it reaches the upstream app, while the app's own
//!   cookies survive.
//!
//! One `#[tokio::test]` on purpose — `APP_PROXY` is a process-global whose
//! main port is set once, so a single sequential flow avoids cross-test
//! interference.
//!
//! Run: cargo test --test app_proxy_integration

use axum::{routing::get, Router};
use homevault::app_proxy::app_auth::APP_AUTH;
use homevault::app_proxy::APP_PROXY;
use homevault::auth::AuthService;
use homevault::storage::Database;

/// A backend that echoes the `Cookie` header it received, so the test can
/// prove which credentials reach the app.
async fn echo_backend() -> u16 {
    let app = Router::new()
        .route("/", get(|| async { "backend root" }))
        .route("/ping", get(|| async { "pong" }))
        .route(
            "/cookies",
            get(|headers: axum::http::HeaderMap| async move {
                headers
                    .get(axum::http::header::COOKIE)
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("")
                    .to_string()
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    port
}

#[tokio::test]
async fn test_app_proxy_end_to_end() {
    std::env::set_var("DATA_DIRECTORY", std::env::temp_dir().join("hv-proxy-test"));

    // Real AuthService on an in-memory DB, with one user + session.
    let db = Database::new("sqlite::memory:").await.unwrap();
    db.run_migrations().await.unwrap();
    let auth = AuthService::new(db.clone());
    let hash = auth.hash_password("pw012345").unwrap();
    sqlx::query(
        "INSERT INTO users (username, password_hash, fullname, email, role_id, locked, created_at, avatar_seed)
         VALUES ('bob', ?, 'B', 'b@e.com', 2, false, datetime('now'), 's')",
    )
    .bind(&hash)
    .execute(&*db.pool())
    .await
    .unwrap();
    let session = auth
        .create_session("bob".to_string(), "127.0.0.1".to_string(), false)
        .await
        .unwrap();
    let api_token = session.token;

    let backend_port = echo_backend().await;

    APP_PROXY.init(8080);
    let proxy_port = APP_PROXY
        .start("test:echo", backend_port, false, None)
        .await
        .expect("proxy starts");
    // Give the listener a beat to come up.
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;

    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let base = format!("http://127.0.0.1:{proxy_port}");

    // 1. Plain forward, no auth required.
    let body = client
        .get(format!("{base}/ping"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert_eq!(body, "pong");

    // 2. Turn the auth gate on — a cookie-less request is redirected to the
    //    server-chosen `/app-auth` entry, never to `/login` directly.
    assert!(APP_PROXY.set_auth_required("test:echo", true).await);
    let resp = client.get(format!("{base}/ping")).send().await.unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::FOUND);
    let location = resp
        .headers()
        .get("location")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert!(location.contains("/app-auth?app=test%3Aecho"), "got: {location}");

    // 3. The full API session cookie must NOT satisfy the gate any more.
    let resp = client
        .get(format!("{base}/ping"))
        .header("cookie", format!("hv_session={api_token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::FOUND);

    // 4. An app-proxy-scoped session lets the request through.
    let app_token = APP_AUTH.mint("bob");
    let resp = client
        .get(format!("{base}/ping"))
        .header("cookie", format!("hv_app_session={app_token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    assert_eq!(resp.text().await.unwrap(), "pong");

    // 5. Both HomeVault credentials are stripped before the app sees them,
    //    while the app's own cookie survives.
    let echoed = client
        .get(format!("{base}/cookies"))
        .header(
            "cookie",
            format!("hv_session={api_token}; hv_app_session={app_token}; app_theme=dark"),
        )
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(!echoed.contains("hv_session"), "leaked API session: {echoed}");
    assert!(!echoed.contains("hv_app_session"), "leaked app session: {echoed}");
    assert!(echoed.contains("app_theme=dark"), "dropped app cookie: {echoed}");

    // 6. A bogus app session is still rejected.
    let resp = client
        .get(format!("{base}/ping"))
        .header("cookie", "hv_app_session=not-a-real-token")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::FOUND);

    APP_PROXY.stop("test:echo").await;
}

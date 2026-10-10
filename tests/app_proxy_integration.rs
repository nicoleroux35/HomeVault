//! Integration test for the per-app reverse proxy (`crate::app_proxy`).
//!
//! Spins a tiny echo backend, starts one proxy in front of it via the
//! global `APP_PROXY`, and exercises: plain forwarding, the auth gate
//! rejecting a cookie-less request with a 302, and letting a request with
//! a valid `hv_session` cookie through.
//!
//! One `#[tokio::test]` on purpose — `APP_PROXY` is a process-global with a
//! `OnceLock` for its auth handle, so a single sequential flow avoids
//! cross-test interference.
//!
//! Run: cargo test --test app_proxy_integration

use axum::{routing::get, Router};
use homevault::app_proxy::APP_PROXY;
use homevault::auth::AuthService;
use homevault::storage::Database;

async fn echo_backend() -> u16 {
    let app = Router::new()
        .route("/", get(|| async { "backend root" }))
        .route("/ping", get(|| async { "pong" }));
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
    let token = session.token;

    let backend_port = echo_backend().await;

    APP_PROXY.init(auth, 8080);
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

    // 2. Turn the auth gate on — a cookie-less request is redirected.
    assert!(APP_PROXY.set_auth_required("test:echo", true).await);
    let resp = client.get(format!("{base}/ping")).send().await.unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::FOUND);
    assert!(resp
        .headers()
        .get("location")
        .unwrap()
        .to_str()
        .unwrap()
        .contains("/login"));

    // 3. With a valid hv_session cookie it goes through.
    let resp = client
        .get(format!("{base}/ping"))
        .header("cookie", format!("hv_session={token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    assert_eq!(resp.text().await.unwrap(), "pong");

    // 4. A bogus cookie is still rejected.
    let resp = client
        .get(format!("{base}/ping"))
        .header("cookie", "hv_session=not-a-real-token")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::FOUND);

    APP_PROXY.stop("test:echo").await;
}

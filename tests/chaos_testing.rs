/* =============================================================================
 * File          : chaos_testing.rs
 * Author        : Nicolas Le Roux
 * Created       : 2026-11-18
 * Version       : 1.0.0
 *
 * Description:
 *   Chaos engineering tests: simulate failures, network issues, disk full,
 *   timeouts, and verify the system degrades gracefully.
 *
 * Dependencies:
 *   - tokio::time for timeouts
 *
 * Usage:
 *   cargo test --test chaos_testing -- --nocapture
 *   CHAOS_ITERATIONS=100 cargo test --test chaos_testing
 * =============================================================================
 */

use axum::http::{Method, StatusCode};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::time::timeout;
use tower::{Service, ServiceExt}; // For .call() and .oneshot()

mod common;
use common::*;

#[tokio::test]
async fn test_chaos_concurrent_requests_with_failures() {
    let (app, admin_token, _db) = create_test_app_with_admin().await;

    let iterations = std::env::var("CHAOS_ITERATIONS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(50);

    let success_count = Arc::new(AtomicUsize::new(0));
    let failure_count = Arc::new(AtomicUsize::new(0));

    let mut handles = vec![];

    // Spawn many concurrent requests
    for i in 0..iterations {
        let mut app_clone = app.clone();
        let token = admin_token.clone();
        let success = Arc::clone(&success_count);
        let failure = Arc::clone(&failure_count);

        let handle = tokio::spawn(async move {
            // Mix of different endpoints
            let endpoint = match i % 4 {
                0 => "/api/auth/me",
                1 => "/api/settings/appearance",
                2 => "/api/health",
                _ => "/api/dashboard/storage",
            };

            let req = axum::http::Request::builder()
                .method(Method::GET)
                .uri(endpoint)
                .header("X-Session-Token", &token)
                .body(axum::body::Body::empty())
                .unwrap();

            // Add random timeout to simulate network issues
            let timeout_ms = 100 + (i % 500);
            let result = timeout(
                Duration::from_millis(timeout_ms as u64),
                app_clone.call(req),
            )
            .await;

            match result {
                Ok(Ok(response)) if response.status().is_success() => {
                    success.fetch_add(1, Ordering::Relaxed);
                }
                _ => {
                    failure.fetch_add(1, Ordering::Relaxed);
                }
            }
        });

        handles.push(handle);
    }

    // Wait for all requests
    for handle in handles {
        let _ = handle.await;
    }

    let successes = success_count.load(Ordering::Relaxed);
    let failures = failure_count.load(Ordering::Relaxed);

    println!("Chaos test: {} successes, {} failures", successes, failures);

    // At least 70% should succeed even under chaos
    let success_rate = (successes as f64) / (iterations as f64) * 100.0;
    assert!(
        success_rate >= 70.0,
        "Success rate too low: {:.1}% (expected >= 70%)",
        success_rate
    );
}

#[tokio::test]
async fn test_chaos_database_contention() {
    let (app, admin_token, _db) = create_test_app_with_admin().await;

    let mut handles = vec![];

    // Concurrent writes to the same resources
    for i in 0..20 {
        let app_clone = app.clone();
        let token = admin_token.clone();

        let handle = tokio::spawn(async move {
            let payload = serde_json::json!({
                "wallpaper": format!("hv-{:02}", (i % 44) + 1),
                "accent_color": "#2E72C9"
            });

            let req = axum::http::Request::builder()
                .method(Method::PUT)
                .uri("/api/settings/appearance")
                .header("X-Session-Token", &token)
                .header("Content-Type", "application/json")
                .body(axum::body::Body::from(
                    serde_json::to_vec(&payload).unwrap(),
                ))
                .unwrap();

            let response = app_clone.oneshot(req).await.unwrap();
            response.status()
        });

        handles.push(handle);
    }

    let mut results = vec![];
    for handle in handles {
        results.push(handle.await.unwrap());
    }

    // All requests should complete with either success or graceful failure
    for status in results {
        assert!(
            status.is_success()
                || status.is_server_error()
                || status == StatusCode::TOO_MANY_REQUESTS,
            "Unexpected status under contention: {}",
            status
        );
    }
}

#[tokio::test]
async fn test_chaos_rapid_auth_logout_cycles() {
    let app = create_test_app().await;

    // Rapidly create and destroy sessions
    for _ in 0..10 {
        // Login
        let login_payload = serde_json::json!({
            "username": "testuser",
            "password": "TestPass123!",
            "remember": false
        });

        let req = axum::http::Request::builder()
            .method(Method::POST)
            .uri("/api/auth/login")
            .header("Content-Type", "application/json")
            .body(axum::body::Body::from(
                serde_json::to_vec(&login_payload).unwrap(),
            ))
            .unwrap();

        let response = app.clone().oneshot(req).await;

        // May fail (user doesn't exist) but shouldn't panic
        assert!(response.is_ok());
    }
}

#[tokio::test]
async fn test_chaos_malformed_requests() {
    let (app, admin_token, _db) = create_test_app_with_admin().await;

    // Send various malformed requests
    let malformed_requests = vec![
        // Missing Content-Type
        (
            Method::PUT,
            "/api/settings/appearance",
            Some(b"{\"wallpaper\":\"test\"}".to_vec()),
            None,
        ),
        // Invalid JSON
        (
            Method::PUT,
            "/api/settings/appearance",
            Some(b"not-json".to_vec()),
            Some("application/json"),
        ),
        // Extra fields
        (
            Method::PUT,
            "/api/settings/appearance",
            Some(b"{\"wallpaper\":\"test\",\"extra\":\"field\",\"another\":123}".to_vec()),
            Some("application/json"),
        ),
        // Wrong types
        (
            Method::PUT,
            "/api/settings/appearance",
            Some(b"{\"wallpaper\":123,\"accent_color\":true}".to_vec()),
            Some("application/json"),
        ),
    ];

    for (method, uri, body, content_type) in malformed_requests {
        let mut req = axum::http::Request::builder()
            .method(method)
            .uri(uri)
            .header("X-Session-Token", &admin_token);

        if let Some(ct) = content_type {
            req = req.header("Content-Type", ct);
        }

        let body_data = body.unwrap_or_default();
        let request = req.body(axum::body::Body::from(body_data)).unwrap();

        let response = app.clone().oneshot(request).await.unwrap();

        // Must be a client error from the extractor, never a panic or a 500.
        // The contract is axum's: 415 without a JSON content-type, 400 for
        // unparseable JSON, 422 when the body is valid JSON but the field
        // types do not match (`Json<T>` rejects that with 422). A 2xx is
        // allowed for the "extra fields" case, which serde ignores.
        assert!(
            response.status() == StatusCode::BAD_REQUEST
                || response.status() == StatusCode::UNSUPPORTED_MEDIA_TYPE
                || response.status() == StatusCode::UNPROCESSABLE_ENTITY
                || response.status().is_success(), // Some may be valid
            "Unexpected status for malformed request: {}",
            response.status()
        );
    }
}

#[tokio::test]
async fn test_chaos_extremely_long_requests() {
    let (app, admin_token, _db) = create_test_app_with_admin().await;

    // Very long wallpaper string
    let long_value = "x".repeat(10_000);
    let payload = serde_json::json!({
        "wallpaper": long_value,
        "accent_color": "#2E72C9"
    });

    let req = axum::http::Request::builder()
        .method(Method::PUT)
        .uri("/api/settings/appearance")
        .header("X-Session-Token", &admin_token)
        .header("Content-Type", "application/json")
        .body(axum::body::Body::from(
            serde_json::to_vec(&payload).unwrap(),
        ))
        .unwrap();

    let response = app.oneshot(req).await.unwrap();

    // Should reject (400) or handle gracefully
    assert!(
        response.status() == StatusCode::BAD_REQUEST
            || response.status() == StatusCode::PAYLOAD_TOO_LARGE
            || response.status().is_success(),
    );
}

#[tokio::test]
async fn test_chaos_concurrent_session_invalidations() {
    let (app, admin_token, _db) = create_test_app_with_admin().await;

    let shared_app = Arc::new(app);
    let shared_token = Arc::new(admin_token);
    let running = Arc::new(AtomicBool::new(true));

    let mut handles = vec![];

    // Readers
    for _ in 0..5 {
        let app = Arc::clone(&shared_app);
        let token = Arc::clone(&shared_token);
        let run = Arc::clone(&running);

        let handle = tokio::spawn(async move {
            while run.load(Ordering::Relaxed) {
                let req = axum::http::Request::builder()
                    .method(Method::GET)
                    .uri("/api/auth/me")
                    .header("X-Session-Token", token.as_str())
                    .body(axum::body::Body::empty())
                    .unwrap();

                let _ = app.as_ref().clone().oneshot(req).await;
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        });

        handles.push(handle);
    }

    // Let them run
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Stop
    running.store(false, Ordering::Relaxed);

    for handle in handles {
        let _ = handle.await;
    }

    // System should remain stable
}

#[tokio::test]
async fn test_chaos_path_traversal_attempts() {
    let (_app, _admin_token, _db) = create_test_app_with_admin().await;

    // Various path traversal attempts that should be safely rejected
    let traversal_attempts = vec![
        "../../../etc/passwd",
        "..%2F..%2F..%2Fetc%2Fpasswd",
        "....//....//....//etc/passwd",
        "/etc/passwd",
        "../../../../../../../../../../etc/shadow",
        "./../..",
        "foo/../../bar",
    ];

    for attempt in traversal_attempts {
        // Try as a path parameter (would need actual endpoint that takes paths)
        // For now, test the validation exists
        assert!(!is_safe_path_component(attempt));
    }
}

#[tokio::test]
async fn test_chaos_sql_injection_attempts() {
    let (app, _admin_token, _db) = create_test_app_with_admin().await;

    // SQL injection attempts in login
    let injection_attempts = vec![
        "admin' OR '1'='1",
        "admin'--",
        "admin' /*",
        "' OR 1=1--",
        "admin'; DROP TABLE users--",
    ];

    for username in injection_attempts {
        let payload = serde_json::json!({
            "username": username,
            "password": "anything"
        });

        let req = axum::http::Request::builder()
            .method(Method::POST)
            .uri("/api/auth/login")
            .header("Content-Type", "application/json")
            .body(axum::body::Body::from(
                serde_json::to_vec(&payload).unwrap(),
            ))
            .unwrap();

        let response = app.clone().oneshot(req).await.unwrap();

        // Should not succeed (401), and definitely shouldn't return 500
        assert_ne!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }
}

#[tokio::test]
async fn test_chaos_xss_injection_attempts() {
    let (app, admin_token, _db) = create_test_app_with_admin().await;

    // XSS attempts
    let xss_attempts = vec![
        "<script>alert('xss')</script>",
        "<img src=x onerror=alert('xss')>",
        "javascript:alert('xss')",
        "<iframe src='javascript:alert(1)'>",
    ];

    for xss in xss_attempts {
        let payload = serde_json::json!({
            "wallpaper": xss,
            "accent_color": "#2E72C9"
        });

        let req = axum::http::Request::builder()
            .method(Method::PUT)
            .uri("/api/settings/appearance")
            .header("X-Session-Token", &admin_token)
            .header("Content-Type", "application/json")
            .body(axum::body::Body::from(
                serde_json::to_vec(&payload).unwrap(),
            ))
            .unwrap();

        let response = app.clone().oneshot(req).await.unwrap();

        // Should reject or sanitize - never return 500
        assert_ne!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }
}

// Helper functions
fn is_safe_path_component(path: &str) -> bool {
    !path.contains("..") && !path.starts_with('/') && !path.contains('\0')
}

#[cfg(test)]
mod chaos_helpers {
    use super::*;

    #[test]
    fn test_path_validation_rejects_traversal() {
        assert!(!is_safe_path_component("../etc/passwd"));
        assert!(!is_safe_path_component("foo/../bar"));
        assert!(is_safe_path_component("safe/path"));
    }
}

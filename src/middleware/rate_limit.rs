use axum::{
    extract::{Request, State},
    http::StatusCode,
    middleware::Next,
    response::Response,
};
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// In-memory rate limiter using a sliding-window counter per key.
#[derive(Clone)]
pub struct RateLimitState {
    trackers: Arc<RwLock<HashMap<String, (u32, Instant)>>>,
    max_requests: u32,
    window_duration: Duration,
}

impl RateLimitState {
    pub fn new(max_requests: u32, window_seconds: u64) -> Self {
        Self {
            trackers: Arc::new(RwLock::new(HashMap::new())),
            max_requests,
            window_duration: Duration::from_secs(window_seconds),
        }
    }

    /// Check whether `key` (IP or username) has exceeded the rate limit.
    pub fn check_rate_limit(&self, key: &str) -> Result<(), StatusCode> {
        let mut trackers = self.trackers.write();
        let now = Instant::now();

        if let Some((count, first_request)) = trackers.get_mut(key) {
            if now.duration_since(*first_request) > self.window_duration {
                *count = 1;
                *first_request = now;
            } else {
                *count += 1;
                if *count > self.max_requests {
                    tracing::warn!(
                        key = %key,
                        count = *count,
                        max = self.max_requests,
                        "Rate limit exceeded"
                    );
                    return Err(StatusCode::TOO_MANY_REQUESTS);
                }
            }
        } else {
            trackers.insert(key.to_string(), (1, now));
        }

        Ok(())
    }

    /// Remove expired entries (should be called periodically).
    pub fn cleanup_expired(&self) {
        let mut trackers = self.trackers.write();
        let now = Instant::now();

        trackers.retain(|_, (_, first_request)| {
            now.duration_since(*first_request) <= self.window_duration
        });
    }
}

/// Axum middleware that rate-limits by client IP.
pub async fn rate_limit_middleware(
    State(state): State<Arc<RateLimitState>>,
    request: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    let ip = extract_ip(&request);

    state.check_rate_limit(&ip)?;

    Ok(next.run(request).await)
}

fn extract_ip(request: &Request) -> String {
    // Prefer X-Forwarded-For, then X-Real-IP, falling back to "unknown".
    let headers = request.headers();

    if let Some(forwarded_for) = headers.get("x-forwarded-for") {
        if let Ok(forwarded_str) = forwarded_for.to_str() {
            if let Some(first_ip) = forwarded_str.split(',').next() {
                return first_ip.trim().to_string();
            }
        }
    }

    if let Some(real_ip) = headers.get("x-real-ip") {
        if let Ok(ip_str) = real_ip.to_str() {
            return ip_str.trim().to_string();
        }
    }

    // Fallback to remote address
    "unknown".to_string()
}

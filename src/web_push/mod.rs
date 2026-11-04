/* =============================================================================
 * File          : mod.rs
 * Author        : Nicolas Le Roux
 * Created       : 06/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Web Push backend (phase D2): VAPID key management, subscription
 *   validation, the sender that turns a persisted notification into one
 *   encrypted push per device, and the per-device cleanup when a push service
 *   reports the subscription gone.
 *
 *   The service worker already had a `push` handler with no backend behind it.
 *   This module supplies the missing half. It is deliberately conservative:
 *   - a subscription exists only after an explicit user action and browser
 *     permission (the screen says so);
 *   - the endpoint is an HTTPS origin on port 443 resolving to a public
 *     address, so a signed-in user cannot turn the sender into an SSRF probe;
 *   - the VAPID private key is sealed with the appliance key and the file is
 *     `0600`; only the public key ever reaches the browser;
 *   - a device is removed when the push service says it is gone (404/410) or
 *     after repeated failures, and the user can always revoke it from the
 *     screen.
 *
 * Dependencies:
 *   - ring, reqwest, serde/serde_json, crate::storage::secret_box,
 *     crate::network (public-address policy)
 *
 * Usage:
 *   handlers/web_push.rs (the API); src/notification_engine.rs calls
 *   `dispatch` after persisting a notification. See docs/guides/web-push.md.
 * ============================================================================= */

//! Web Push: VAPID keys, subscription policy, encryption and delivery.

pub mod crypto;
pub mod repo;

use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Mutex, OnceCell};

use crate::config::paths;
use crate::storage::secret_box;
use repo::PushSubscriptionRepository;

pub use repo::PushSubscription;

/// Scope of the sealed VAPID private key, so the blob cannot be moved into
/// another secret slot.
const VAPID_SCOPE: &str = "web-push:vapid";

/// The `sub` claim. Push services require a contact URI; this one is fixed
/// because HomeVault has no per-instance contact setting and never uses it to
/// deliver anything.
const VAPID_SUBJECT: &str = "mailto:admin@homevault.local";

/// One user's device cap. A home account has a phone, a laptop and a tablet;
/// ten leaves room without letting one account fill the table.
pub const MAX_SUBSCRIPTIONS_PER_USER: i64 = 10;

/// Consecutive failures after which a subscription is dropped. A push service
/// outage is transient; ten notifications over hours is not.
pub const MAX_FAILURES: i64 = 10;

/// The push service keeps an undelivered message for this long.
pub const PUSH_TTL_SECS: u32 = 24 * 60 * 60;

/// A push request that hangs must not hold the sender for a whole poll.
const SEND_TIMEOUT: Duration = Duration::from_secs(10);

/// The payload the service worker expects (see `frontend/public/sw.js`).
#[derive(Debug, Clone, Serialize)]
pub struct PushPayload {
    pub title: String,
    pub body: String,
    pub tag: String,
    pub data: PushPayloadData,
}

#[derive(Debug, Clone, Serialize)]
pub struct PushPayloadData {
    pub url: String,
    pub severity: String,
    pub id: String,
}

impl PushPayload {
    /// Build the payload for a persisted notification.
    pub fn for_notification(title: &str, message: &str, severity: &str, id: &str) -> Self {
        Self {
            title: truncate_utf8(title, 120).to_string(),
            body: truncate_utf8(message, 600).to_string(),
            tag: format!("homevault-{id}"),
            data: PushPayloadData {
                url: "/desktop".to_string(),
                severity: severity.to_string(),
                id: id.to_string(),
            },
        }
    }

    /// The exact plaintext sent to the encryption layer.
    pub fn encode(&self) -> Vec<u8> {
        // `for_notification` bounds the two user-visible strings well below
        // the 4 KiB record, and the rest is a fixed shape.
        serde_json::to_vec(self).unwrap_or_default()
    }
}

/// Public form of the byte-bounded truncation, for the handler's device label.
pub fn truncate_at(value: &str, max_bytes: usize) -> String {
    truncate_utf8(value, max_bytes).to_string()
}

fn truncate_utf8(value: &str, max_bytes: usize) -> &str {
    if value.len() <= max_bytes {
        return value;
    }
    let mut end = max_bytes;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    &value[..end]
}

/// VAPID keys ready to sign.
pub struct VapidKeys {
    pub public_key: String,
    key_pair: ring::signature::EcdsaKeyPair,
}

impl VapidKeys {
    /// The `Authorization` header for one endpoint.
    pub fn authorization(&self, endpoint: &str, now_unix: u64) -> Result<String, String> {
        let audience = crypto::endpoint_audience(endpoint)?;
        let token = crypto::vapid_token(
            &self.key_pair,
            &audience,
            VAPID_SUBJECT,
            now_unix + 12 * 60 * 60,
        )?;
        Ok(format!("vapid t={token}, k={}", self.public_key))
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct StoredKeys {
    public_key: String,
    /// `hv1:` sealed PKCS#8, standard base64.
    private_key: String,
}

fn keys_path() -> PathBuf {
    paths::base_dir().join("data/web_push.json")
}

static KEYS: Lazy<OnceCell<Arc<VapidKeys>>> = Lazy::new(OnceCell::new);
static KEY_LOCK: Lazy<Mutex<()>> = Lazy::new(|| Mutex::new(()));

/// The appliance's VAPID keys, generated once and sealed at rest.
pub async fn keys() -> Result<Arc<VapidKeys>, String> {
    if let Some(keys) = KEYS.get() {
        return Ok(keys.clone());
    }
    let _guard = KEY_LOCK.lock().await;
    if let Some(keys) = KEYS.get() {
        return Ok(keys.clone());
    }
    let keys = Arc::new(load_or_generate().await?);
    let _ = KEYS.set(keys.clone());
    Ok(keys)
}

async fn load_or_generate() -> Result<VapidKeys, String> {
    let path = keys_path();
    if path.exists() {
        let content = tokio::fs::read_to_string(&path)
            .await
            .map_err(|e| format!("Could not read {}: {e}", path.display()))?;
        let stored: StoredKeys = serde_json::from_str(&content)
            .map_err(|e| format!("Could not parse {}: {e}", path.display()))?;
        let sealed = secret_box::open(VAPID_SCOPE, &stored.private_key)?;
        let pkcs8 = crypto::standard_decode(&sealed)?;
        let key_pair = crypto::vapid_key_pair(&pkcs8)?;
        // Trust the key pair over the stored public half: if they disagree,
        // the file was tampered with and every push would be rejected.
        return Ok(VapidKeys {
            public_key: crypto::vapid_public_key(&key_pair),
            key_pair,
        });
    }

    let (pkcs8, public_key) = crypto::generate_vapid_pkcs8()?;
    let sealed = secret_box::seal(VAPID_SCOPE, &crypto::standard_encode(&pkcs8))?;
    let stored = StoredKeys {
        public_key: public_key.clone(),
        private_key: sealed,
    };
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| format!("Could not create {}: {e}", parent.display()))?;
    }
    let content = serde_json::to_string_pretty(&stored)
        .map_err(|e| format!("Could not serialise the VAPID keys: {e}"))?;
    tokio::fs::write(&path, content)
        .await
        .map_err(|e| format!("Could not write {}: {e}", path.display()))?;
    set_private_permissions(&path).await;

    let key_pair = crypto::vapid_key_pair(&pkcs8)?;
    Ok(VapidKeys {
        public_key,
        key_pair,
    })
}

#[cfg(unix)]
async fn set_private_permissions(path: &PathBuf) {
    use std::os::unix::fs::PermissionsExt;
    if let Ok(metadata) = tokio::fs::metadata(path).await {
        let mut permissions = metadata.permissions();
        permissions.set_mode(0o600);
        let _ = tokio::fs::set_permissions(path, permissions).await;
    }
}

#[cfg(not(unix))]
async fn set_private_permissions(_path: &PathBuf) {}

/// Syntax rules for a subscription endpoint. Resolution is separate because
/// it can fail transiently and is exercised by the handler.
pub fn validate_endpoint_syntax(raw: &str) -> Result<url::Url, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.len() > 2048 {
        return Err("The push endpoint is missing or too long.".to_string());
    }
    let url = url::Url::parse(trimmed)
        .map_err(|_| "The push endpoint is not a valid URL.".to_string())?;
    if url.scheme() != "https" {
        return Err("A push endpoint must use HTTPS.".to_string());
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("A push endpoint must not carry credentials.".to_string());
    }
    if url.fragment().is_some() {
        return Err("A push endpoint must not have a fragment.".to_string());
    }
    if url.port().is_some_and(|port| port != 443) {
        return Err("A push endpoint must use port 443.".to_string());
    }
    if url.host_str().is_none() {
        return Err("The push endpoint has no host.".to_string());
    }
    Ok(url)
}

/// Whether an address is a public internet address (the only kind a push
/// service endpoint may resolve to).
fn is_public_address(ip: IpAddr) -> bool {
    !crate::network::is_private_address(ip) && !ip.is_unspecified() && !ip.is_multicast()
}

/// Resolve the endpoint and require **every** address to be public. This is
/// what keeps a signed-in user from aiming the sender at the LAN.
pub async fn resolve_public_endpoint(url: &url::Url) -> Result<(), String> {
    let host = url
        .host_str()
        .ok_or_else(|| "The push endpoint has no host.".to_string())?;

    if let Ok(ip) = host.trim_matches(['[', ']']).parse::<IpAddr>() {
        if !is_public_address(ip) {
            return Err(
                "The push endpoint must be a public internet address, not a local one.".to_string(),
            );
        }
        return Ok(());
    }

    let addresses: Vec<IpAddr> = tokio::net::lookup_host((host, 443))
        .await
        .map_err(|e| format!("The push endpoint name could not be resolved: {e}"))?
        .map(|address| address.ip())
        .collect();
    if addresses.is_empty() {
        return Err("The push endpoint name did not resolve.".to_string());
    }
    if addresses.iter().any(|address| !is_public_address(*address)) {
        return Err(
            "The push endpoint resolves to a local address; it must be a public service."
                .to_string(),
        );
    }
    Ok(())
}

/// Validate a browser subscription as it arrives from the screen.
pub async fn validate_subscription(
    endpoint: &str,
    p256dh: &str,
    auth: &str,
) -> Result<(String, String, String), String> {
    let url = validate_endpoint_syntax(endpoint)?;
    resolve_public_endpoint(&url).await?;

    let public = crypto::b64url_decode(p256dh)?;
    if public.len() != crypto::PUBLIC_KEY_LEN || public[0] != 0x04 {
        return Err(
            "The subscription public key is not a 65-byte uncompressed P-256 point.".to_string(),
        );
    }
    let auth = crypto::b64url_decode(auth)?;
    if auth.len() != crypto::AUTH_SECRET_LEN {
        return Err("The subscription authentication secret must be 16 bytes.".to_string());
    }
    Ok((url.to_string(), p256dh.trim().to_string(), auth_b64(auth)))
}

fn auth_b64(auth: Vec<u8>) -> String {
    crypto::b64url_encode(&auth)
}

enum SendOutcome {
    Delivered,
    /// The push service says this subscription no longer exists.
    Gone,
    /// A transient or configuration failure; count it, never delete a device
    /// because the VAPID key was wrong.
    Failed {
        config: bool,
    },
}

/// Send one notification to every subscribed device. Called in a spawned task
/// by the notification engine, so a slow push service never delays the loop.
pub async fn dispatch(pool: Arc<sqlx::SqlitePool>, payload: PushPayload) {
    let repo = PushSubscriptionRepository::new(pool);
    let subscriptions = match repo.list_all().await {
        Ok(subscriptions) => subscriptions,
        Err(e) => {
            tracing::warn!("Web Push: subscriptions could not be listed: {e}");
            return;
        }
    };
    if subscriptions.is_empty() {
        return;
    }
    let keys = match keys().await {
        Ok(keys) => keys,
        Err(e) => {
            tracing::warn!("Web Push: VAPID keys are unavailable: {e}");
            return;
        }
    };
    let client = match reqwest::Client::builder()
        .timeout(SEND_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .build()
    {
        Ok(client) => client,
        Err(e) => {
            tracing::warn!("Web Push: the HTTP client could not be built: {e}");
            return;
        }
    };
    let plaintext = payload.encode();

    for subscription in subscriptions {
        let outcome = send_one(&client, &keys, &subscription, &plaintext).await;
        match outcome {
            SendOutcome::Delivered => {
                let _ = repo.record_success(&subscription.id).await;
            }
            SendOutcome::Gone => {
                tracing::info!(
                    "Web Push: removing expired subscription {} ({})",
                    subscription.id,
                    subscription.device_label
                );
                let _ = repo.delete(&subscription.id).await;
            }
            SendOutcome::Failed { config } => {
                if config {
                    tracing::warn!(
                        "Web Push: the push service rejected the VAPID credentials; \
                         keeping the device and not counting a delivery failure"
                    );
                    continue;
                }
                match repo.record_failure(&subscription.id).await {
                    Ok(count) if count >= MAX_FAILURES => {
                        tracing::info!(
                            "Web Push: removing subscription {} after {count} failures",
                            subscription.id
                        );
                        let _ = repo.delete(&subscription.id).await;
                    }
                    Ok(_) => {}
                    Err(e) => tracing::warn!("Web Push: failure count could not be recorded: {e}"),
                }
            }
        }
    }
}

async fn send_one(
    client: &reqwest::Client,
    keys: &VapidKeys,
    subscription: &PushSubscription,
    plaintext: &[u8],
) -> SendOutcome {
    let url = match validate_endpoint_syntax(&subscription.endpoint) {
        Ok(url) => url,
        Err(e) => {
            tracing::warn!("Web Push: stored endpoint is invalid: {e}");
            return SendOutcome::Gone;
        }
    };
    if let Err(e) = resolve_public_endpoint(&url).await {
        tracing::warn!("Web Push: stored endpoint is no longer public: {e}");
        return SendOutcome::Gone;
    }

    let public = match crypto::b64url_decode(&subscription.p256dh) {
        Ok(public) => public,
        Err(_) => return SendOutcome::Gone,
    };
    let auth = match crypto::b64url_decode(&subscription.auth) {
        Ok(auth) => auth,
        Err(_) => return SendOutcome::Gone,
    };
    let body = match crypto::encrypt_push(&public, &auth, plaintext) {
        Ok(body) => body,
        Err(e) => {
            tracing::warn!("Web Push: message could not be encrypted: {e}");
            return SendOutcome::Gone;
        }
    };
    let authorization = match keys.authorization(&subscription.endpoint, now_unix()) {
        Ok(value) => value,
        Err(e) => {
            tracing::warn!("Web Push: VAPID authorization could not be built: {e}");
            return SendOutcome::Failed { config: true };
        }
    };

    let response = client
        .post(subscription.endpoint.as_str())
        .header("TTL", PUSH_TTL_SECS.to_string())
        .header("Content-Encoding", "aes128gcm")
        .header("Content-Type", "application/octet-stream")
        .header("Authorization", authorization)
        .body(body)
        .send()
        .await;

    match response {
        Ok(response) if response.status().is_success() => SendOutcome::Delivered,
        Ok(response) if matches!(response.status().as_u16(), 404 | 410) => SendOutcome::Gone,
        Ok(response) if matches!(response.status().as_u16(), 401 | 403) => {
            SendOutcome::Failed { config: true }
        }
        Ok(response) => {
            tracing::debug!(
                "Web Push: {} answered {}",
                subscription.device_label,
                response.status()
            );
            SendOutcome::Failed { config: false }
        }
        Err(e) => {
            tracing::debug!("Web Push: delivery failed: {e}");
            SendOutcome::Failed { config: false }
        }
    }
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

/// The host of an endpoint, for the screen (never the full capability URL).
pub fn endpoint_host(endpoint: &str) -> String {
    url::Url::parse(endpoint)
        .ok()
        .and_then(|url| url.host_str().map(str::to_string))
        .unwrap_or_else(|| "unknown".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_endpoint_syntax_accepts_https_on_443() {
        assert!(validate_endpoint_syntax("https://fcm.googleapis.com/wp/abc?x=1").is_ok());
        assert!(validate_endpoint_syntax("https://push.example:443/x").is_ok());
    }

    #[test]
    fn test_endpoint_syntax_refuses_http_other_ports_and_credentials() {
        for bad in [
            "http://fcm.googleapis.com/wp/abc",
            "https://fcm.googleapis.com:8443/wp/abc",
            "https://user:pass@fcm.googleapis.com/wp/abc",
            "https://fcm.googleapis.com/wp/abc#frag",
            "not a url",
        ] {
            assert!(validate_endpoint_syntax(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn test_public_address_policy() {
        for public in ["8.8.8.8", "1.1.1.1", "2606:4700:4700::1111"] {
            assert!(is_public_address(public.parse().unwrap()), "{public}");
        }
        for local in [
            "127.0.0.1",
            "192.168.1.1",
            "10.0.0.1",
            "::1",
            "fd00::1",
            "0.0.0.0",
            "224.0.0.1",
        ] {
            assert!(!is_public_address(local.parse().unwrap()), "{local}");
        }
    }

    #[test]
    fn test_payload_is_bounded_and_carries_the_service_worker_fields() {
        let long = "é".repeat(1000);
        let payload = PushPayload::for_notification("t", &long, "warning", "id-1");
        assert!(payload.body.len() <= 600);
        assert!(payload.body.is_char_boundary(payload.body.len()));
        let encoded = payload.encode();
        let value: serde_json::Value = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(value["title"], "t");
        assert_eq!(value["data"]["severity"], "warning");
        assert_eq!(value["data"]["url"], "/desktop");
        assert_eq!(value["tag"], "homevault-id-1");
        assert!(encoded.len() < crypto::MAX_PLAINTEXT);
    }

    #[test]
    fn test_truncate_utf8_never_splits_a_code_point() {
        assert_eq!(truncate_utf8("abc", 10), "abc");
        // "é" is two bytes; a one-byte cut must fall back to zero.
        assert_eq!(truncate_utf8("éé", 1), "");
        assert_eq!(truncate_utf8("éé", 3), "é");
    }

    #[test]
    fn test_endpoint_host_never_returns_the_path() {
        assert_eq!(
            endpoint_host("https://fcm.googleapis.com/wp/secret-token"),
            "fcm.googleapis.com"
        );
        assert_eq!(endpoint_host("not a url"), "unknown");
    }
}

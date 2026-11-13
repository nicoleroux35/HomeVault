/* =============================================================================
 * File          : acme_client.rs
 * Author        : Nicolas Le Roux
 * Created       : 29/10/2026
 * Last modified : 29/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   RFC 8555 client used by certificate automation: JWS signing with an ES256
 *   account key, order/authorization/finalize flow, HTTP-01 and DNS-01
 *   challenges, and installation of the issued pair. The network layer is a
 *   trait so the whole flow is exercised against a scripted CA in tests.
 *
 * Dependencies:
 *   - rcgen (>= 0.14, P-256 key, CSR and ECDSA signing)
 *   - reqwest (>= 0.13, HTTP + JSON)
 *   - tokio (>= 1, timers)
 *
 * Usage:
 *   `Issuer::new(Arc::new(ReqwestTransport::new()?))` from the handler and the
 *   renewal worker; see docs/guides/acme.md.
 * =============================================================================
 */

//! The ACME client: everything between "the admin pressed Save" and "a
//! certificate is on disk".
//!
//! Two layers, on purpose:
//!
//! * [`Session`] speaks RFC 8555 — nonces, JWS, orders, authorizations,
//!   finalization — over a [`Transport`].
//! * [`Issuer`] decides *what* to do for the configured settings (which
//!   challenge, where the DNS record goes) and installs the result.
//!
//! [`Transport`] is a trait so the flow can be tested end to end against a
//! scripted certificate authority: no test ever reaches the network, and every
//! step of the protocol is asserted in [`super::tests`].

use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::json;

use super::{
    base64url_nopad, challenge_record_name, describe_problem, dns01_txt_value, is_retryable_status,
    sha256, AcmeError, AcmeSettings, ChallengeType, DnsProvider, PendingDnsRecord,
};

/// How many times a validation or finalization is polled before giving up.
pub const POLL_ATTEMPTS: u32 = 20;

/// Wait between two polls. The CA answers in seconds when the challenge is
/// already in place, so this is short; the propagation wait for DNS-01 is
/// separate and configured by the admin.
pub const POLL_DELAY: Duration = Duration::from_secs(3);

use crate::outbound::BoxFuture;
pub use crate::outbound::{HttpRequest, HttpResponse, ReqwestTransport, Transport};

// --------------------------------------------------------------------------
// Account key, JWK and JWS
// --------------------------------------------------------------------------

/// The ACME account key: a P-256 key pair that signs every request.
///
/// It is not the certificate key. Keeping them separate means a certificate
/// renewal never invalidates the registration.
pub struct AccountKey {
    key: rcgen::KeyPair,
}

impl AccountKey {
    pub fn generate() -> Result<Self, AcmeError> {
        rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256)
            .map(|key| Self { key })
            .map_err(|e| AcmeError::new(format!("cannot generate the account key: {e}"), false))
    }

    pub fn from_pem(pem: &str) -> Result<Self, AcmeError> {
        rcgen::KeyPair::from_pem(pem)
            .map(|key| Self { key })
            .map_err(|e| AcmeError::new(format!("the account key is unreadable: {e}"), false))
    }

    pub fn to_pem(&self) -> String {
        self.key.serialize_pem()
    }

    /// Public JWK as RFC 7517 encodes it. `crv`/`kty`/`x`/`y` is also the exact
    /// subset RFC 7638 hashes for the thumbprint, so both live here.
    pub fn public_jwk(&self) -> serde_json::Value {
        let raw = self.key.public_key_raw();
        // An uncompressed SEC1 point is `0x04 || X || Y`; anything else is
        // already the bare coordinates.
        let (x, y) = if raw.len() == 65 && raw[0] == 0x04 {
            (&raw[1..33], &raw[33..65])
        } else {
            (raw, raw)
        };
        json!({
            "crv": "P-256",
            "kty": "EC",
            "x": base64url_nopad(x),
            "y": base64url_nopad(y),
        })
    }

    /// RFC 7638 thumbprint: the SHA-256 of the JWK with only the required
    /// members, in lexicographic order, no whitespace.
    pub fn thumbprint(&self) -> String {
        let jwk = self.public_jwk();
        let canonical = format!(
            "{{\"crv\":\"{}\",\"kty\":\"{}\",\"x\":\"{}\",\"y\":\"{}\"}}",
            jwk["crv"].as_str().unwrap_or_default(),
            jwk["kty"].as_str().unwrap_or_default(),
            jwk["x"].as_str().unwrap_or_default(),
            jwk["y"].as_str().unwrap_or_default(),
        );
        base64url_nopad(&sha256(canonical.as_bytes()))
    }

    /// ES256 signature of `data`, as the 64-byte `R || S` pair JWS expects.
    pub fn sign_es256(&self, data: &[u8]) -> Result<Vec<u8>, AcmeError> {
        use rcgen::SigningKey;
        let der = self
            .key
            .sign(data)
            .map_err(|e| AcmeError::new(format!("cannot sign the request: {e}"), false))?;
        ecdsa_der_to_raw(&der)
    }

    /// PKCS#10 request for one DNS name, DER-encoded (ACME wants base64url of
    /// this, not PEM).
    pub fn csr_der(&self, domain: &str) -> Result<Vec<u8>, AcmeError> {
        let params = rcgen::CertificateParams::new(vec![domain.to_string()])
            .map_err(|e| AcmeError::new(format!("cannot build the request: {e}"), false))?;
        let csr = params.serialize_request(&self.key).map_err(|e| {
            AcmeError::new(format!("cannot build the certificate request: {e}"), false)
        })?;
        Ok(csr.der().to_vec())
    }
}

/// Convert an ASN.1 `SEQUENCE { INTEGER r, INTEGER s }` ECDSA signature to the
/// fixed-width `R || S` form JWS uses. Written by hand because the only other
/// option is a DER dependency for twenty lines of parsing.
pub fn ecdsa_der_to_raw(der: &[u8]) -> Result<Vec<u8>, AcmeError> {
    const BAD: &str = "the signature from the account key is not DER-encoded ECDSA";
    if der.len() < 8 || der[0] != 0x30 {
        return Err(AcmeError::new(BAD, false));
    }
    let mut i = 1;
    let long_form = der[i] & 0x80 != 0;
    i += if long_form {
        1 + (der[i] & 0x7f) as usize
    } else {
        1
    };
    let mut out = vec![0u8; 64];
    for slot in 0..2 {
        if der.get(i) != Some(&0x02) {
            return Err(AcmeError::new(BAD, false));
        }
        let len = *der.get(i + 1).ok_or_else(|| AcmeError::new(BAD, false))? as usize;
        let body = der
            .get(i + 2..i + 2 + len)
            .ok_or_else(|| AcmeError::new(BAD, false))?;
        // Strip the leading zero DER adds when the high bit is set.
        let trimmed = if body.len() > 32 {
            &body[body.len() - 32..]
        } else {
            body
        };
        let start = slot * 32 + (32 - trimmed.len());
        out[start..start + trimmed.len()].copy_from_slice(trimmed);
        i += 2 + len;
    }
    Ok(out)
}

/// Build a flattened JWS body for one ACME request.
///
/// `payload` is `None` for POST-as-GET (RFC 8555 §6.3), which sends an empty
/// payload string rather than `{}`.
pub fn build_jws(
    key: &AccountKey,
    url: &str,
    nonce: &str,
    kid: Option<&str>,
    payload: Option<&serde_json::Value>,
) -> Result<String, AcmeError> {
    let mut header = serde_json::Map::new();
    header.insert("alg".to_string(), json!("ES256"));
    header.insert("nonce".to_string(), json!(nonce));
    header.insert("url".to_string(), json!(url));
    match kid {
        Some(kid) => header.insert("kid".to_string(), json!(kid)),
        None => header.insert("jwk".to_string(), key.public_jwk()),
    };
    let protected = base64url_nopad(
        &serde_json::to_vec(&serde_json::Value::Object(header))
            .map_err(|e| AcmeError::new(format!("cannot encode the request header: {e}"), false))?,
    );
    let payload = match payload {
        Some(value) => base64url_nopad(value.to_string().as_bytes()),
        None => String::new(),
    };
    let signing_input = format!("{protected}.{payload}");
    let signature = base64url_nopad(&key.sign_es256(signing_input.as_bytes())?);
    Ok(json!({
        "protected": protected,
        "payload": payload,
        "signature": signature,
    })
    .to_string())
}

// --------------------------------------------------------------------------
// Account key on disk
// --------------------------------------------------------------------------

/// Load the account key from `dir`, generating and persisting it (`0600`) on
/// first use.
pub fn load_or_create_account_key_in(dir: &std::path::Path) -> Result<AccountKey, AcmeError> {
    let path = dir.join("account.key");
    if let Ok(pem) = std::fs::read_to_string(&path) {
        if !pem.trim().is_empty() {
            return AccountKey::from_pem(&pem);
        }
    }
    let key = AccountKey::generate()?;
    write_private(&path, key.to_pem().as_bytes())?;
    Ok(key)
}

/// Load (or create) the account key under `data/acme/`.
pub fn load_or_create_account_key() -> Result<AccountKey, AcmeError> {
    load_or_create_account_key_in(&super::acme_dir())
}

/// Write a secret file inside `data/acme/` with the permissions a private key
/// deserves (`0700` directory, `0600` file).
pub fn write_private(path: &std::path::Path, bytes: &[u8]) -> Result<(), AcmeError> {
    let dir = path.parent().unwrap_or(std::path::Path::new("."));
    std::fs::create_dir_all(dir)
        .map_err(|e| AcmeError::new(format!("cannot create {}: {e}", dir.display()), false))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
    }
    std::fs::write(path, bytes)
        .map_err(|e| AcmeError::new(format!("cannot write {}: {e}", path.display()), false))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

// --------------------------------------------------------------------------
// Session (RFC 8555)
// --------------------------------------------------------------------------

/// The three endpoints every ACME server publishes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Directory {
    pub new_nonce: String,
    pub new_account: String,
    pub new_order: String,
}

/// Parse a directory document. Missing members are a hard failure: without
/// `newOrder` there is nothing to do, and a partial directory usually means a
/// captive portal or an HTML error page answered instead of the CA.
pub fn parse_directory(body: &str) -> Result<Directory, AcmeError> {
    let value: serde_json::Value = serde_json::from_str(body).map_err(|_| {
        AcmeError::new(
            "the certificate authority did not answer with an ACME directory",
            true,
        )
    })?;
    let field = |name: &str| {
        value
            .get(name)
            .and_then(|v| v.as_str())
            .filter(|v| v.starts_with("https://"))
            .map(str::to_string)
            .ok_or_else(|| {
                AcmeError::new(
                    format!("the ACME directory has no usable {name} endpoint"),
                    true,
                )
            })
    };
    Ok(Directory {
        new_nonce: field("newNonce")?,
        new_account: field("newAccount")?,
        new_order: field("newOrder")?,
    })
}

/// One ACME account, with the nonce and account URL that every signed request
/// needs.
pub struct Session {
    transport: Arc<dyn Transport>,
    key: AccountKey,
    directory: Directory,
    nonce: Option<String>,
    kid: Option<String>,
    pub poll_attempts: u32,
    pub poll_delay: Duration,
}

impl Session {
    /// Fetch the directory and prepare a session. The account key is generated
    /// here if this is the first run, so a failure to write it surfaces before
    /// the CA is contacted.
    pub async fn open(
        transport: Arc<dyn Transport>,
        key: AccountKey,
        directory_url: &str,
        kid: Option<String>,
    ) -> Result<Self, AcmeError> {
        let response = transport.send(HttpRequest::get(directory_url)).await?;
        if !response.is_success() {
            return Err(AcmeError::new(
                describe_problem(response.status, &response.body),
                is_retryable_status(response.status),
            ));
        }
        Ok(Self {
            transport,
            key,
            directory: parse_directory(&response.body)?,
            nonce: response.header("replay-nonce").map(str::to_string),
            kid,
            poll_attempts: POLL_ATTEMPTS,
            poll_delay: POLL_DELAY,
        })
    }

    /// Register (or find) the account and remember its URL. `onlyReturnExisting`
    /// is deliberately not used: registering twice with the same key returns
    /// the same account, which is exactly what a reinstall needs.
    pub async fn register(
        &mut self,
        contact_email: &str,
        tos_agreed: bool,
    ) -> Result<String, AcmeError> {
        let payload = json!({
            "termsOfServiceAgreed": tos_agreed,
            "contact": [format!("mailto:{contact_email}")],
        });
        let url = self.directory.new_account.clone();
        let response = self.post(&url, Some(&payload)).await?;
        let kid = response
            .header("location")
            .map(str::to_string)
            .or_else(|| self.kid.clone())
            .ok_or_else(|| {
                AcmeError::new(
                    "the certificate authority did not return an account URL",
                    true,
                )
            })?;
        self.kid = Some(kid.clone());
        Ok(kid)
    }

    /// A fresh nonce, from the cache or from the CA's `newNonce` endpoint.
    async fn nonce(&mut self) -> Result<String, AcmeError> {
        if let Some(nonce) = self.nonce.take() {
            return Ok(nonce);
        }
        let url = self.directory.new_nonce.clone();
        let response = self.transport.send(HttpRequest::head(&url)).await?;
        response
            .header("replay-nonce")
            .map(str::to_string)
            .ok_or_else(|| {
                AcmeError::new(
                    "the certificate authority did not return a request nonce",
                    true,
                )
            })
    }

    /// POST a signed body, retrying once on `badNonce` (the only ACME error
    /// that is safe — and required — to retry with the identical payload).
    async fn post(
        &mut self,
        url: &str,
        payload: Option<&serde_json::Value>,
    ) -> Result<HttpResponse, AcmeError> {
        let mut last: Option<HttpResponse> = None;
        for attempt in 0..2 {
            let nonce = self.nonce().await?;
            let body = build_jws(&self.key, url, &nonce, self.kid.as_deref(), payload)?;
            let response = self
                .transport
                .send(HttpRequest::post(url, body, "application/jose+json"))
                .await?;
            self.nonce = response.header("replay-nonce").map(str::to_string);
            if response.is_success() {
                return Ok(response);
            }
            let bad_nonce = response.status == 400 && response.body.contains("badNonce");
            if bad_nonce && attempt == 0 {
                self.nonce = None;
                last = Some(response);
                continue;
            }
            return Err(AcmeError::new(
                describe_problem(response.status, &response.body),
                is_retryable_status(response.status),
            ));
        }
        let _ = last;
        Err(AcmeError::new(
            "the certificate authority kept rejecting the request nonce",
            true,
        ))
    }

    /// POST-as-GET (RFC 8555 §6.3): fetch a resource with a signed empty body.
    pub async fn fetch(&mut self, url: &str) -> Result<HttpResponse, AcmeError> {
        self.post(url, None).await
    }

    /// Create an order for one domain and return the challenge it asks for.
    pub async fn start_order(
        &mut self,
        domain: &str,
        kind: ChallengeType,
    ) -> Result<OrderStart, AcmeError> {
        let payload = json!({
            "identifiers": [{ "type": "dns", "value": domain }],
        });
        let url = self.directory.new_order.clone();
        let response = self.post(&url, Some(&payload)).await?;
        let order_url = response.header("location").map(str::to_string);
        let order = response.json();
        let authz_url = order["authorizations"]
            .as_array()
            .and_then(|a| a.first())
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .ok_or_else(|| {
                AcmeError::new(
                    "the certificate authority returned an order with no authorization",
                    true,
                )
            })?;
        let finalize_url = order["finalize"]
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| {
                AcmeError::new(
                    "the certificate authority returned an order with no finalize endpoint",
                    true,
                )
            })?;

        let authz = self.fetch(&authz_url).await?;
        let authz_json = authz.json();
        let wanted = match kind {
            ChallengeType::Http01 => "http-01",
            ChallengeType::Dns01 => "dns-01",
        };
        let challenge = authz_json["challenges"]
            .as_array()
            .and_then(|list| list.iter().find(|c| c["type"].as_str() == Some(wanted)))
            .cloned()
            .ok_or_else(|| {
                AcmeError::new(
                    format!("the certificate authority did not offer a {wanted} challenge"),
                    false,
                )
            })?;
        let token = challenge["token"]
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| AcmeError::new("the offered challenge has no token", true))?;
        let challenge_url = challenge["url"]
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| AcmeError::new("the offered challenge has no URL", true))?;

        Ok(OrderStart {
            authz_url,
            challenge_url,
            finalize_url,
            order_url,
            token,
        })
    }

    /// Ask the CA to validate the challenge now.
    pub async fn respond(&mut self, challenge_url: &str) -> Result<(), AcmeError> {
        let payload = json!({});
        self.post(challenge_url, Some(&payload)).await?;
        Ok(())
    }

    /// Poll the authorization until it is valid. An `invalid` authorization is
    /// final: the CA will not change its mind, and its reason is the only
    /// useful thing to show the admin.
    pub async fn await_authorization(&mut self, authz_url: &str) -> Result<(), AcmeError> {
        for _ in 0..self.poll_attempts {
            let response = self.fetch(authz_url).await?;
            let authz = response.json();
            match authz["status"].as_str().unwrap_or("") {
                "valid" => return Ok(()),
                "invalid" => {
                    let detail = authz["challenges"]
                        .as_array()
                        .and_then(|list| list.iter().find_map(|c| c["error"]["detail"].as_str()))
                        .unwrap_or("the challenge did not validate");
                    return Err(AcmeError::final_error(detail.to_string()));
                }
                _ => tokio::time::sleep(self.poll_delay).await,
            }
        }
        Err(AcmeError::new(
            "the certificate authority did not finish validating the domain in time",
            true,
        ))
    }

    /// Submit the CSR, wait for the order to be ready, and download the chain.
    pub async fn finalize(
        &mut self,
        finalize_url: &str,
        order_url: Option<&str>,
        domain: &str,
        key: &AccountKey,
    ) -> Result<String, AcmeError> {
        let csr = key.csr_der(domain)?;
        let payload = json!({ "csr": base64url_nopad(&csr) });
        let response = self.post(finalize_url, Some(&payload)).await?;
        let mut order = response.json();
        let mut certificate_url = order["certificate"].as_str().map(str::to_string);

        let mut attempts = 0;
        while certificate_url.is_none() && attempts < self.poll_attempts {
            attempts += 1;
            tokio::time::sleep(self.poll_delay).await;
            let order_url = order_url
                .or_else(|| order["url"].as_str())
                .ok_or_else(|| {
                    AcmeError::new(
                        "the certificate authority did not return an order URL",
                        true,
                    )
                })?
                .to_string();
            let polled = self.fetch(&order_url).await?;
            order = polled.json();
            if order["status"].as_str().unwrap_or("") == "invalid" {
                let detail = order["error"]["detail"]
                    .as_str()
                    .unwrap_or("the certificate authority could not issue the certificate");
                return Err(AcmeError::final_error(detail.to_string()));
            }
            certificate_url = order["certificate"].as_str().map(str::to_string);
        }

        let certificate_url = certificate_url.ok_or_else(|| {
            AcmeError::new(
                "the certificate authority issued the certificate but never offered it for download",
                true,
            )
        })?;
        let response = self.fetch(&certificate_url).await?;
        if !response.body.contains("BEGIN CERTIFICATE") {
            return Err(AcmeError::new(
                "the downloaded certificate is not PEM",
                true,
            ));
        }
        Ok(response.body)
    }
}

/// What [`Session::start_order`] hands back: the URLs the rest of the flow
/// needs, plus the token whose key authorization has to be published.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrderStart {
    pub order_url: Option<String>,
    pub authz_url: String,
    pub challenge_url: String,
    pub finalize_url: String,
    pub token: String,
}

/// A manual DNS-01 validation waiting for the admin, persisted so the flow
/// survives a restart between "here is the record" and "the record exists".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingRun {
    pub domain: String,
    pub authz_url: String,
    pub challenge_url: String,
    pub finalize_url: String,
    #[serde(default)]
    pub order_url: Option<String>,
    pub record: PendingDnsRecord,
}

/// File name of the pending manual run, inside `data/acme/`.
pub const RUN_FILE: &str = "run.json";

/// Persist the pending manual run (no secret: a record name, a TXT value and
/// three CA URLs, all meaningless without the account key).
pub fn save_pending_run_in(dir: &std::path::Path, run: &PendingRun) -> Result<(), String> {
    let body = serde_json::to_string_pretty(run).map_err(|e| e.to_string())?;
    write_private(&dir.join(RUN_FILE), body.as_bytes()).map_err(|e| e.to_string())
}

pub fn save_pending_run(run: &PendingRun) -> Result<(), String> {
    save_pending_run_in(&super::acme_dir(), run)
}

pub fn load_pending_run_in(dir: &std::path::Path) -> Option<PendingRun> {
    let text = std::fs::read_to_string(dir.join(RUN_FILE)).ok()?;
    serde_json::from_str(&text).ok()
}

pub fn load_pending_run() -> Option<PendingRun> {
    load_pending_run_in(&super::acme_dir())
}

pub fn clear_pending_run_in(dir: &std::path::Path) {
    let _ = std::fs::remove_file(dir.join(RUN_FILE));
}

pub fn clear_pending_run() {
    clear_pending_run_in(&super::acme_dir());
}

// --------------------------------------------------------------------------
// DNS providers
// --------------------------------------------------------------------------

/// Publishes and removes the `_acme-challenge` TXT record.
///
/// A provider that cannot publish on its own (the `manual` one) is represented
/// by `None`: the flow stops and asks the admin instead.
pub trait DnsPublisher: Send + Sync {
    fn publish<'a>(&'a self, name: &'a str, value: &'a str)
        -> BoxFuture<'a, Result<(), AcmeError>>;

    fn remove<'a>(&'a self, name: &'a str, value: &'a str) -> BoxFuture<'a, Result<(), AcmeError>>;
}

/// Cloudflare DNS, the one provider HomeVault ships. The token is scoped to
/// "edit DNS for this zone" and stored with the rest of the automation secrets.
pub struct Cloudflare {
    transport: Arc<dyn Transport>,
    token: String,
}

impl Cloudflare {
    pub fn new(transport: Arc<dyn Transport>, token: impl Into<String>) -> Self {
        Self {
            transport,
            token: token.into(),
        }
    }

    fn api(path: &str) -> String {
        format!("https://api.cloudflare.com/client/v4{path}")
    }

    /// Candidate zones for a challenge record, longest first: a delegated
    /// subdomain (`nas.example.com` hosted on its own zone) comes before the
    /// registrable name (`example.com`). The API call decides which one exists
    /// — guessing with a public-suffix list would be wrong for every private
    /// suffix and would add a dependency for it.
    pub fn zone_candidates(name: &str) -> Vec<String> {
        let host = name
            .strip_prefix("_acme-challenge.")
            .unwrap_or(name)
            .to_string();
        let labels: Vec<&str> = host.split('.').collect();
        let mut out = Vec::new();
        for start in 0..labels.len().saturating_sub(1) {
            out.push(labels[start..].join("."));
        }
        out
    }

    /// Ask the API for the first candidate zone the token can see.
    async fn find_zone(&self, name: &str) -> Result<String, AcmeError> {
        for candidate in Self::zone_candidates(name) {
            let response = self
                .transport
                .send(
                    HttpRequest::get(&Self::api(&format!("/zones?name={candidate}")))
                        .with_bearer(&self.token),
                )
                .await?;
            let body = response.json();
            if let Some(id) = body["result"]
                .as_array()
                .and_then(|zones| zones.first())
                .and_then(|zone| zone["id"].as_str())
            {
                return Ok(id.to_string());
            }
        }
        Err(AcmeError::new(
            format!(
                "the DNS token cannot see any zone for {}",
                Self::zone_candidates(name)
                    .first()
                    .cloned()
                    .unwrap_or_else(|| name.to_string())
            ),
            false,
        ))
    }
}

impl DnsPublisher for Cloudflare {
    fn publish<'a>(
        &'a self,
        name: &'a str,
        value: &'a str,
    ) -> BoxFuture<'a, Result<(), AcmeError>> {
        Box::pin(async move {
            let zone_id = self.find_zone(name).await?;
            let payload = json!({
                "type": "TXT",
                "name": name,
                "content": value,
                "ttl": 60,
            });
            let _ = &self.token;
            let response = self
                .transport
                .send(
                    HttpRequest::post(
                        &Self::api(&format!("/zones/{zone_id}/dns_records")),
                        payload.to_string(),
                        "application/json",
                    )
                    .with_bearer(&self.token),
                )
                .await?;
            if !(200..300).contains(&response.status) {
                return Err(AcmeError::new(
                    format!(
                        "the DNS provider refused the record: {}",
                        provider_error(&response.body)
                    ),
                    is_retryable_status(response.status),
                ));
            }
            Ok(())
        })
    }

    fn remove<'a>(&'a self, name: &'a str, value: &'a str) -> BoxFuture<'a, Result<(), AcmeError>> {
        Box::pin(async move {
            let zone_id = match self.find_zone(name).await {
                Ok(id) => id,
                // A record we cannot find is already gone: cleanup must never
                // turn a successful issuance into a failure.
                Err(_) => return Ok(()),
            };
            let records = self
                .transport
                .send(
                    HttpRequest::get(&Self::api(&format!(
                        "/zones/{zone_id}/dns_records?type=TXT&name={name}"
                    )))
                    .with_bearer(&self.token),
                )
                .await?;
            let records = records.json();
            let ids: Vec<String> = records["result"]
                .as_array()
                .map(|list| {
                    list.iter()
                        .filter(|r| r["content"].as_str() == Some(value))
                        .filter_map(|r| r["id"].as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();
            for id in ids {
                let _ = self
                    .transport
                    .send(
                        HttpRequest::delete(&Self::api(&format!(
                            "/zones/{zone_id}/dns_records/{id}"
                        )))
                        .with_bearer(&self.token),
                    )
                    .await;
            }
            Ok(())
        })
    }
}

/// Pull the first human-readable error out of a DNS provider's JSON body.
fn provider_error(body: &str) -> String {
    let value: serde_json::Value = serde_json::from_str(body).unwrap_or(serde_json::Value::Null);
    value["errors"]
        .as_array()
        .and_then(|errors| errors.first())
        .and_then(|e| e["message"].as_str())
        .map(str::to_string)
        .unwrap_or_else(|| "no reason given".to_string())
}

// --------------------------------------------------------------------------
// Issuance
// --------------------------------------------------------------------------

/// Result of one issuance attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IssueOutcome {
    /// A certificate is installed; the expiry is filled in when the file could
    /// be parsed back.
    Installed { not_after: Option<String> },
    /// DNS-01 with the manual provider: the admin has to create this record,
    /// then confirm.
    AwaitingDns { record: PendingDnsRecord },
}

/// Drives one issuance or renewal for a settings snapshot.
pub struct Issuer {
    transport: Arc<dyn Transport>,
    dns: Option<Arc<dyn DnsPublisher>>,
    pub poll_attempts: u32,
    pub poll_delay: Duration,
    /// How long a DNS-01 TXT record is given to propagate.
    pub propagation: Duration,
    /// Where the account key and the issued pair are written. `None` means
    /// `data/acme/`; tests point it at a scratch directory.
    pub work_dir: Option<std::path::PathBuf>,
    /// A publisher injected by a caller that already knows which provider to
    /// use (tests, and the manual-confirm path).
    pub forced_dns: Option<Arc<dyn DnsPublisher>>,
}

impl Issuer {
    pub fn new(transport: Arc<dyn Transport>) -> Self {
        Self {
            transport,
            dns: None,
            poll_attempts: POLL_ATTEMPTS,
            poll_delay: POLL_DELAY,
            propagation: Duration::from_secs(super::DEFAULT_DNS_PROPAGATION_SECONDS),
            work_dir: None,
            forced_dns: None,
        }
    }

    /// Use this publisher instead of the one the settings name.
    pub fn with_publisher(mut self, publisher: Arc<dyn DnsPublisher>) -> Self {
        self.forced_dns = Some(publisher);
        self
    }

    /// Directory holding `account.key`, `run.json` and `tls/`.
    fn dir(&self) -> std::path::PathBuf {
        self.work_dir.clone().unwrap_or_else(super::acme_dir)
    }

    /// Attach the provider that can publish the TXT record on its own.
    pub fn with_dns(mut self, dns: Arc<dyn DnsPublisher>) -> Self {
        self.dns = Some(dns);
        self
    }

    /// Build the provider the settings ask for. `manual` deliberately yields
    /// `None`.
    pub fn provider_for(&self, settings: &AcmeSettings) -> Option<Arc<dyn DnsPublisher>> {
        match (settings.challenge, settings.provider) {
            (ChallengeType::Dns01, DnsProvider::Cloudflare)
                if !settings.dns_api_token.is_empty() =>
            {
                Some(Arc::new(Cloudflare::new(
                    self.transport.clone(),
                    settings.dns_api_token.clone(),
                )))
            }
            _ => None,
        }
    }

    fn session<'a>(
        &'a self,
        key: AccountKey,
        settings: &'a AcmeSettings,
    ) -> BoxFuture<'a, Result<Session, AcmeError>> {
        Box::pin(async move {
            let directory = super::validate_directory_url(&settings.directory_url)
                .map_err(|e| AcmeError::new(e, false))?;
            let mut session = Session::open(
                self.transport.clone(),
                key,
                &directory,
                settings.account_url.clone(),
            )
            .await?;
            session.poll_attempts = self.poll_attempts;
            session.poll_delay = self.poll_delay;
            Ok(session)
        })
    }

    /// Run the whole flow. For DNS-01 + `manual` this stops at the record and
    /// returns [`IssueOutcome::AwaitingDns`]; every other combination finishes
    /// and installs the certificate.
    pub async fn issue(&self, settings: &AcmeSettings) -> Result<IssueOutcome, AcmeError> {
        let key = load_or_create_account_key_in(&self.dir())?;
        let mut session = self.session(key, settings).await?;
        let kid = session
            .register(&settings.contact_email, settings.tos_agreed)
            .await?;
        let _ = kid;

        let start = session
            .start_order(&settings.domain, settings.challenge)
            .await?;
        let key_authorization = format!("{}.{}", start.token, session.key.thumbprint());
        // The certificate gets its own key: reusing the account key would mean
        // a certificate compromise also hands over the ACME registration.
        let cert_key = AccountKey::generate()?;

        match settings.challenge {
            ChallengeType::Http01 => {
                super::register_challenge(&start.token, &key_authorization)
                    .map_err(|e| AcmeError::new(e, false))?;
                let result = async {
                    session.respond(&start.challenge_url).await?;
                    session.await_authorization(&start.authz_url).await
                }
                .await;
                super::remove_challenge(&start.token);
                result?;
            }
            ChallengeType::Dns01 => {
                let name = challenge_record_name(&settings.domain);
                let value = dns01_txt_value(&key_authorization);
                let record = PendingDnsRecord {
                    name: name.clone(),
                    value: value.clone(),
                };
                match self
                    .forced_dns
                    .clone()
                    .or_else(|| self.provider_for(settings))
                {
                    Some(publisher) => {
                        publisher.publish(&name, &value).await?;
                        tokio::time::sleep(self.propagation).await;
                        let result = async {
                            session.respond(&start.challenge_url).await?;
                            session.await_authorization(&start.authz_url).await
                        }
                        .await;
                        let _ = publisher.remove(&name, &value).await;
                        result?;
                    }
                    None => {
                        save_pending_run_in(
                            &self.dir(),
                            &PendingRun {
                                domain: settings.domain.clone(),
                                authz_url: start.authz_url,
                                challenge_url: start.challenge_url,
                                finalize_url: start.finalize_url,
                                order_url: start.order_url,
                                record: record.clone(),
                            },
                        )
                        .map_err(|e| AcmeError::new(e, false))?;
                        return Ok(IssueOutcome::AwaitingDns { record });
                    }
                }
            }
        }

        let pem = session
            .finalize(
                &start.finalize_url,
                start.order_url.as_deref(),
                &settings.domain,
                &cert_key,
            )
            .await?;
        self.install(&pem, &cert_key)
    }

    /// Second half of a manual DNS-01 run: the admin has created the record, so
    /// validate, finalize and install.
    pub async fn finish_manual(
        &self,
        settings: &AcmeSettings,
        run: &PendingRun,
    ) -> Result<IssueOutcome, AcmeError> {
        let key = load_or_create_account_key_in(&self.dir())?;
        let thumbprint = key.thumbprint();
        let mut session = self.session(key, settings).await?;
        session
            .register(&settings.contact_email, settings.tos_agreed)
            .await?;
        session.respond(&run.challenge_url).await?;
        session.await_authorization(&run.authz_url).await?;
        let cert_key = AccountKey::generate()?;
        let pem = session
            .finalize(
                &run.finalize_url,
                run.order_url.as_deref(),
                &run.domain,
                &cert_key,
            )
            .await?;
        let _ = thumbprint;
        clear_pending_run_in(&self.dir());
        self.install(&pem, &cert_key)
    }

    /// Write the pair where the listener expects it and report the expiry. The
    /// key written is the one the CSR was signed with — anything else would
    /// install a certificate whose private key HomeVault does not hold.
    fn install(&self, cert_pem: &str, cert_key: &AccountKey) -> Result<IssueOutcome, AcmeError> {
        let dir = self.dir().join("tls");
        write_private(
            &dir.join(super::ISSUED_KEY_FILE),
            cert_key.to_pem().as_bytes(),
        )?;
        let cert_path = dir.join(super::ISSUED_CERT_FILE);
        write_private(&cert_path, cert_pem.as_bytes())?;
        Ok(IssueOutcome::Installed {
            not_after: crate::tls::cert_not_after(&cert_path),
        })
    }
}

#[cfg(test)]
impl Issuer {
    /// Point the issuer at a scratch tree instead of `data/acme/`.
    pub fn with_dir_for_test(mut self, dir: &std::path::Path) -> Self {
        self.work_dir = Some(dir.to_path_buf());
        self
    }

    /// Skip the real polling and propagation waits, which exist for a CA's
    /// benefit, not a test suite's.
    pub fn with_quick_polling(mut self) -> Self {
        self.poll_delay = Duration::ZERO;
        self.propagation = Duration::ZERO;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::outbound::host_of;
    use base64::Engine as _;
    use std::collections::{BTreeMap, VecDeque};
    use std::sync::Mutex;

    /// A certificate authority that answers a fixed script, in order, and
    /// records every request it was sent. No test in this module touches the
    /// network or a real CA.
    struct Scripted {
        replies: Mutex<VecDeque<HttpResponse>>,
        seen: Mutex<Vec<HttpRequest>>,
    }

    impl Scripted {
        fn new(replies: Vec<HttpResponse>) -> Arc<Self> {
            Arc::new(Self {
                replies: Mutex::new(replies.into()),
                seen: Mutex::new(Vec::new()),
            })
        }

        fn urls(&self) -> Vec<String> {
            self.seen
                .lock()
                .unwrap()
                .iter()
                .map(|r| r.url.clone())
                .collect()
        }

        fn last_bearer(&self) -> Option<String> {
            self.seen
                .lock()
                .unwrap()
                .iter()
                .filter_map(|r| r.bearer.clone())
                .next_back()
        }
    }

    impl Transport for Scripted {
        fn send<'a>(
            &'a self,
            request: HttpRequest,
        ) -> BoxFuture<'a, Result<HttpResponse, AcmeError>> {
            self.seen.lock().unwrap().push(request);
            let next = self.replies.lock().unwrap().pop_front();
            Box::pin(async move {
                next.ok_or_else(|| AcmeError::new("unexpected request in the script", false))
            })
        }
    }

    fn headers(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_ascii_lowercase(), v.to_string()))
            .collect()
    }

    fn json_ok(body: serde_json::Value, nonce: &str) -> HttpResponse {
        HttpResponse {
            status: 200,
            body: body.to_string(),
            headers: headers(&[("replay-nonce", nonce)]),
        }
    }

    fn created(location: &str, body: serde_json::Value, nonce: &str) -> HttpResponse {
        HttpResponse {
            status: 201,
            body: body.to_string(),
            headers: headers(&[("replay-nonce", nonce), ("location", location)]),
        }
    }

    const DIRECTORY: &str = "https://ca.example/directory";

    /// A directory document. Deliberately no `Replay-Nonce`: the session must
    /// then fetch one from `newNonce`, which is what the script expects.
    fn directory_response() -> HttpResponse {
        HttpResponse {
            status: 200,
            body: json!({
                "newNonce": "https://ca.example/new-nonce",
                "newAccount": "https://ca.example/new-account",
                "newOrder": "https://ca.example/new-order",
            })
            .to_string(),
            headers: BTreeMap::new(),
        }
    }

    fn settings(challenge: ChallengeType) -> AcmeSettings {
        AcmeSettings {
            enabled: true,
            domain: "nas.example.com".to_string(),
            contact_email: "admin@example.com".to_string(),
            tos_agreed: true,
            challenge,
            provider: DnsProvider::Manual,
            directory_url: DIRECTORY.to_string(),
            ..Default::default()
        }
    }

    fn b64d(value: &str) -> Vec<u8> {
        base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(value.as_bytes())
            .expect("base64url")
    }

    fn test_pem() -> String {
        rcgen::generate_simple_self_signed(vec!["nas.example.com".to_string()])
            .expect("self-signed for the scripted CA")
            .cert
            .pem()
    }

    /// The twelve requests of a successful HTTP-01 order, in order.
    fn http01_script() -> Vec<HttpResponse> {
        vec![
            directory_response(),
            HttpResponse {
                status: 200,
                body: String::new(),
                headers: headers(&[("replay-nonce", "n1")]),
            },
            created(
                "https://ca.example/acct/1",
                json!({ "status": "valid" }),
                "n2",
            ),
            created(
                "https://ca.example/order/1",
                json!({
                    "status": "pending",
                    "authorizations": ["https://ca.example/authz/1"],
                    "finalize": "https://ca.example/finalize/1",
                }),
                "n3",
            ),
            json_ok(
                json!({
                    "status": "pending",
                    "challenges": [{
                        "type": "http-01",
                        "token": "test-token-http01-ok",
                        "url": "https://ca.example/challenge/1",
                    }],
                }),
                "n4",
            ),
            json_ok(json!({ "status": "processing" }), "n5"),
            json_ok(json!({ "status": "valid" }), "n6"),
            json_ok(json!({ "status": "processing" }), "n7"),
            json_ok(
                json!({
                    "status": "valid",
                    "certificate": "https://ca.example/cert/1",
                }),
                "n8",
            ),
            HttpResponse {
                status: 200,
                body: test_pem(),
                headers: headers(&[("replay-nonce", "n9")]),
            },
        ]
    }

    #[test]
    fn test_parse_directory_rejects_a_non_acme_answer() {
        assert!(parse_directory("<html>router login</html>").is_err());
        let missing = r#"{"newNonce":"https://ca/new-nonce"}"#;
        assert!(parse_directory(missing).is_err());
        let plain_http =
            r#"{"newNonce":"http://ca/n","newAccount":"https://ca/a","newOrder":"https://ca/o"}"#;
        assert!(
            parse_directory(plain_http).is_err(),
            "an http:// endpoint would let a proxy answer for the CA"
        );
        assert!(parse_directory(
            r#"{"newNonce":"https://ca/n","newAccount":"https://ca/a","newOrder":"https://ca/o"}"#
        )
        .is_ok());
    }

    /// Known answer: the key and the values below were produced independently
    /// of this code, so a change in the JWK encoding or the thumbprint
    /// canonicalisation cannot pass silently.
    #[test]
    fn test_known_account_key_thumbprint() {
        let pem = "-----BEGIN PRIVATE KEY-----\n\
MIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQgDybAjsm6iZVDx0NS\n\
6yg6HkWUjxZhxoKN3Cpk8AadukWhRANCAATkEMkrgOAOMwiJtIFC+I4HYaNb7V0Z\n\
wdX9wOkaJXLiDm3RvJeoS2HofTrQ9D15dWvzyFjpVTszgtcQVFeykjJj\n\
-----END PRIVATE KEY-----\n";
        let key = AccountKey::from_pem(pem).expect("fixed key parses");
        let jwk = key.public_jwk();
        assert_eq!(jwk["crv"], "P-256");
        assert_eq!(jwk["kty"], "EC");
        assert_eq!(jwk["x"], "5BDJK4DgDjMIibSBQviOB2GjW-1dGcHV_cDpGiVy4g4");
        assert_eq!(jwk["y"], "bdG8l6hLYeh9OtD0PXl1a_PIWOlVOzOC1xBUV7KSMmM");
        assert_eq!(
            key.thumbprint(),
            "wOnFL9HZK43mzKXXNfXBfaGcy-z8SIDdpim5VWIUuAY"
        );
    }

    #[test]
    fn test_jws_signature_is_a_64_byte_raw_pair() {
        let key = AccountKey::generate().unwrap();
        let body = build_jws(&key, "https://ca/x", "nonce", None, None).unwrap();
        let value: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(value["payload"], "", "POST-as-GET sends an empty payload");
        let sig = value["signature"].as_str().unwrap();
        assert_eq!(b64d(sig).len(), 64);
        let protected = b64d(value["protected"].as_str().unwrap());
        let header: serde_json::Value = serde_json::from_slice(&protected).unwrap();
        assert_eq!(header["alg"], "ES256");
        assert_eq!(header["url"], "https://ca/x");
        assert!(
            header.get("jwk").is_some(),
            "an unregistered account uses jwk"
        );
        assert!(header.get("kid").is_none());

        let body = build_jws(
            &key,
            "https://ca/x",
            "nonce",
            Some("https://ca/acct/1"),
            Some(&json!({"a":1})),
        )
        .unwrap();
        let value: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_ne!(value["payload"], "");
        let protected = b64d(value["protected"].as_str().unwrap());
        let header: serde_json::Value = serde_json::from_slice(&protected).unwrap();
        assert_eq!(header["kid"], "https://ca/acct/1");
        assert!(
            header.get("jwk").is_none(),
            "a registered account must not resend the key"
        );
    }

    #[test]
    fn test_ecdsa_der_to_raw_trims_the_der_padding() {
        // SEQUENCE { INTEGER 0x00 0x80.. (high bit set, padded), INTEGER 0x01 }
        let mut der = vec![0x30, 8, 0x02, 1, 0x01, 0x02, 0x02, 0x00, 0x80, 0x00];
        der[1] = (der.len() - 2) as u8;
        let raw = ecdsa_der_to_raw(&der).unwrap();
        assert_eq!(raw.len(), 64);
        assert_eq!(raw[31], 0x01, "r is right-aligned");
        assert_eq!(raw[62], 0x00);
        assert_eq!(raw[63], 0x80, "s keeps its high bit");
        assert!(ecdsa_der_to_raw(b"not a signature").is_err());
        assert!(ecdsa_der_to_raw(&[0x30, 0x00]).is_err());
    }

    #[tokio::test]
    async fn test_http01_issuance_installs_the_pair_and_clears_the_challenge() {
        let ca = Scripted::new(http01_script());
        let tmp = tempfile::tempdir().unwrap();
        let issuer = Issuer::new(ca.clone())
            .with_dir_for_test(tmp.path())
            .with_quick_polling();

        let outcome = issuer
            .issue(&settings(ChallengeType::Http01))
            .await
            .unwrap();
        assert!(matches!(
            outcome,
            IssueOutcome::Installed { not_after: Some(_) }
        ));

        let cert = tmp.path().join("tls").join("cert.pem");
        let key = tmp.path().join("tls").join("key.pem");
        assert!(cert.exists() && key.exists(), "the pair must be on disk");
        assert!(rcgen::KeyPair::from_pem(&std::fs::read_to_string(&key).unwrap()).is_ok());
        assert!(crate::tls::cert_days_remaining(&cert).unwrap() >= 0);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&key).unwrap().permissions().mode() & 0o777,
                0o600,
                "the issued private key must not be group/world readable"
            );
        }
        assert!(
            super::super::challenge_answer("test-token-http01-ok").is_none(),
            "the answer is dropped after validation"
        );

        let urls = ca.urls();
        assert_eq!(urls[0], DIRECTORY);
        assert_eq!(urls[1], "https://ca.example/new-nonce");
        assert_eq!(urls[2], "https://ca.example/new-account");
        assert_eq!(urls[3], "https://ca.example/new-order");
    }

    #[tokio::test]
    async fn test_manual_dns_returns_the_record_and_persists_the_run() {
        let script = vec![
            directory_response(),
            HttpResponse {
                status: 200,
                body: String::new(),
                headers: headers(&[("replay-nonce", "n1")]),
            },
            created("https://ca.example/acct/1", json!({}), "n2"),
            created(
                "https://ca.example/order/1",
                json!({
                    "authorizations": ["https://ca.example/authz/1"],
                    "finalize": "https://ca.example/finalize/1",
                }),
                "n3",
            ),
            json_ok(
                json!({
                    "status": "pending",
                    "challenges": [{
                        "type": "dns-01",
                        "token": "dns-token",
                        "url": "https://ca.example/challenge/1",
                    }],
                }),
                "n4",
            ),
        ];
        let ca = Scripted::new(script);
        let tmp = tempfile::tempdir().unwrap();
        let issuer = Issuer::new(ca.clone()).with_dir_for_test(tmp.path());

        let mut s = settings(ChallengeType::Dns01);
        s.provider = DnsProvider::Manual;
        let outcome = issuer.issue(&s).await.unwrap();
        let record = match outcome {
            IssueOutcome::AwaitingDns { record } => record,
            other => panic!("expected a pending record, got {other:?}"),
        };
        assert_eq!(record.name, "_acme-challenge.nas.example.com");

        let account = load_or_create_account_key_in(tmp.path()).unwrap();
        let expected = dns01_txt_value(&format!("dns-token.{}", account.thumbprint()));
        assert_eq!(record.value, expected);

        let run = super::load_pending_run_in(tmp.path()).expect("run persisted");
        assert_eq!(run.record, record);
        assert_eq!(run.finalize_url, "https://ca.example/finalize/1");
    }

    /// A refused challenge is final: retrying it cannot succeed, and the UI
    /// must be able to say so instead of showing "pending" forever.
    #[tokio::test]
    async fn test_invalid_authorization_is_a_final_error() {
        let script = vec![
            directory_response(),
            HttpResponse {
                status: 200,
                body: String::new(),
                headers: headers(&[("replay-nonce", "n1")]),
            },
            created("https://ca.example/acct/1", json!({}), "n2"),
            created(
                "https://ca.example/order/1",
                json!({
                    "authorizations": ["https://ca.example/authz/1"],
                    "finalize": "https://ca.example/finalize/1",
                }),
                "n3",
            ),
            json_ok(
                json!({
                    "status": "pending",
                    "challenges": [{
                        "type": "http-01",
                        "token": "test-token",
                        "url": "https://ca.example/challenge/1",
                    }],
                }),
                "n4",
            ),
            json_ok(json!({}), "n5"),
            json_ok(
                json!({
                    "status": "invalid",
                    "challenges": [{
                        "type": "http-01",
                        "error": { "detail": "Invalid response from http://nas.example.com/.well-known/acme-challenge/test-token" },
                    }],
                }),
                "n6",
            ),
        ];
        let ca = Scripted::new(script);
        let tmp = tempfile::tempdir().unwrap();
        let issuer = Issuer::new(ca)
            .with_dir_for_test(tmp.path())
            .with_quick_polling();

        let err = issuer
            .issue(&settings(ChallengeType::Http01))
            .await
            .unwrap_err();
        assert!(!err.retryable, "a refused challenge will not fix itself");
        assert!(err.message.contains("Invalid response"), "{}", err.message);
    }

    #[tokio::test]
    async fn test_an_automated_provider_publishes_then_removes_the_record() {
        struct Recorder {
            calls: Mutex<Vec<String>>,
        }
        impl DnsPublisher for Recorder {
            fn publish<'a>(
                &'a self,
                name: &'a str,
                value: &'a str,
            ) -> BoxFuture<'a, Result<(), AcmeError>> {
                self.calls
                    .lock()
                    .unwrap()
                    .push(format!("publish {name} {value}"));
                Box::pin(async move { Ok(()) })
            }
            fn remove<'a>(
                &'a self,
                name: &'a str,
                value: &'a str,
            ) -> BoxFuture<'a, Result<(), AcmeError>> {
                self.calls
                    .lock()
                    .unwrap()
                    .push(format!("remove {name} {value}"));
                Box::pin(async move { Ok(()) })
            }
        }
        let recorder = Arc::new(Recorder {
            calls: Mutex::new(Vec::new()),
        });

        let script = vec![
            directory_response(),
            HttpResponse {
                status: 200,
                body: String::new(),
                headers: headers(&[("replay-nonce", "n1")]),
            },
            created("https://ca.example/acct/1", json!({}), "n2"),
            created(
                "https://ca.example/order/1",
                json!({
                    "authorizations": ["https://ca.example/authz/1"],
                    "finalize": "https://ca.example/finalize/1",
                }),
                "n3",
            ),
            json_ok(
                json!({
                    "status": "pending",
                    "challenges": [{
                        "type": "dns-01",
                        "token": "dns-token",
                        "url": "https://ca.example/challenge/1",
                    }],
                }),
                "n4",
            ),
            json_ok(json!({}), "n5"),
            json_ok(json!({ "status": "valid" }), "n6"),
            json_ok(json!({ "status": "processing" }), "n7"),
            json_ok(
                json!({ "status": "valid", "certificate": "https://ca.example/cert/1" }),
                "n8",
            ),
            HttpResponse {
                status: 200,
                body: test_pem(),
                headers: headers(&[("replay-nonce", "n9")]),
            },
        ];
        let ca = Scripted::new(script);
        let tmp = tempfile::tempdir().unwrap();
        let issuer = Issuer::new(ca)
            .with_publisher(recorder.clone())
            .with_dir_for_test(tmp.path())
            .with_quick_polling();

        let mut s = settings(ChallengeType::Dns01);
        s.provider = DnsProvider::Cloudflare;
        s.dns_api_token = "token".to_string();
        let outcome = issuer.issue(&s).await.unwrap();
        assert!(matches!(outcome, IssueOutcome::Installed { .. }));

        let calls = recorder.calls.lock().unwrap().clone();
        assert_eq!(calls.len(), 2, "{calls:?}");
        assert!(calls[0].starts_with("publish _acme-challenge.nas.example.com "));
        assert!(calls[1].starts_with("remove _acme-challenge.nas.example.com "));
        assert!(
            super::load_pending_run_in(tmp.path()).is_none(),
            "an automated run leaves nothing pending"
        );
    }

    #[tokio::test]
    async fn test_cloudflare_requests_carry_the_token() {
        let ca = Scripted::new(vec![
            json_ok(json!({ "result": [{ "id": "zone-1" }] }), "n0"),
            json_ok(json!({ "result": { "id": "record-1" } }), "n1"),
        ]);
        let publisher = Cloudflare::new(ca.clone(), "secret-token");
        publisher
            .publish("_acme-challenge.nas.example.com", "value")
            .await
            .unwrap();
        assert_eq!(ca.last_bearer().as_deref(), Some("secret-token"));
        assert!(
            ca.urls()[0].contains("/zones?name=nas.example.com"),
            "the delegated name is tried first: {}",
            ca.urls()[0]
        );
        assert!(ca.urls()[1].contains("/zones/zone-1/dns_records"));
    }

    #[tokio::test]
    async fn test_cloudflare_reports_a_zone_it_cannot_see() {
        let ca = Scripted::new(vec![
            json_ok(json!({ "result": [] }), "n0"),
            json_ok(json!({ "result": [] }), "n1"),
        ]);
        let publisher = Cloudflare::new(ca.clone(), "secret-token");
        let err = publisher
            .publish("_acme-challenge.nas.example.com", "value")
            .await
            .unwrap_err();
        assert!(
            err.message.contains("cannot see any zone"),
            "{}",
            err.message
        );
        assert_eq!(
            ca.urls().len(),
            2,
            "both the delegated name and the parent zone are tried"
        );
    }

    #[test]
    fn test_provider_for_only_builds_an_automated_provider() {
        let issuer = Issuer::new(Scripted::new(vec![]));
        let mut s = settings(ChallengeType::Dns01);
        s.challenge = ChallengeType::Dns01;
        s.provider = DnsProvider::Manual;
        assert!(
            issuer.provider_for(&s).is_none(),
            "the manual provider never publishes"
        );
        s.provider = DnsProvider::Cloudflare;
        s.dns_api_token = String::new();
        assert!(issuer.provider_for(&s).is_none(), "no token, no provider");
        s.dns_api_token = "token".to_string();
        assert!(issuer.provider_for(&s).is_some());
        s.challenge = ChallengeType::Http01;
        assert!(issuer.provider_for(&s).is_none(), "HTTP-01 needs no DNS");
    }

    #[test]
    fn test_host_of_never_echoes_a_token() {
        assert_eq!(
            host_of("https://ca.example/dir?token=secret"),
            Some("ca.example")
        );
        assert_eq!(host_of("not a url"), Some("not a url"));
    }
}

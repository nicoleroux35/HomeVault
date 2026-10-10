//! Built-in HTTPS/TLS support.
//!
//! HomeVault serves plain HTTP on `PORT` (default 8080) as it always has. When
//! TLS is enabled it *additionally* serves the same Axum router over HTTPS on a
//! second listener (`HTTPS_PORT`, default 8443) — see the optional second
//! `axum_server` task in [`crate::web::run`]. This is opt-in: an existing HTTP
//! install is unchanged until an admin turns it on from Settings.
//!
//! Two certificate modes, mirroring OpenMediaVault:
//!
//! * **Self-signed** (default): HomeVault generates its own certificate under
//!   `{DATA_DIRECTORY}/tls/` and regenerates it before it expires. The SAN list
//!   covers `localhost`, the loopback IPs, the OS hostname, `<hostname>.local`,
//!   `homevault.local` and the detected LAN IPv4, so it validates for every
//!   sane way of reaching the box — browsers still show a trust warning, as
//!   with every self-signed appliance.
//! * **Manual**: the admin points HomeVault at their own PEM certificate and
//!   private key (from their own CA, `certbot`, `mkcert`, a reverse proxy…).
//!
//! No ACME/Let's Encrypt in this module yet — that is a possible later phase.
//!
//! Settings are persisted to `{DATA_DIRECTORY}/data/tls.json` (paths only, never
//! key material). Environment variables (`TLS_ENABLED`, `HTTPS_PORT`,
//! `TLS_MODE`, `TLS_CERT_PATH`, `TLS_KEY_PATH`, `TLS_REDIRECT`) provide the
//! bootstrap defaults used until the admin saves the Settings card once.

use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

use serde::{Deserialize, Serialize};

use crate::storage::json::JsonStorage;

/// Default HTTPS listen port when the admin has not chosen one.
pub const DEFAULT_HTTPS_PORT: u16 = 8443;

/// Validity window of a generated self-signed certificate. ~13 months keeps it
/// under the 398-day cap public CAs enforce while still being long enough that
/// regeneration is rare.
const SELF_SIGNED_VALIDITY_DAYS: i64 = 397;

/// Regenerate a self-signed certificate once it has fewer than this many days
/// left, so a running instance never serves an expired cert.
const RENEW_WHEN_DAYS_LEFT: i64 = 30;

/// JSON storage key -> `{DATA_DIRECTORY}/data/tls.json`.
const STORE_KEY: &str = "tls";

/// `true` once [`crate::web::run`] has brought up the HTTPS listener. Read by
/// the security-headers middleware so `Strict-Transport-Security` is only sent
/// when the instance actually answers over TLS (a browser ignores HSTS received
/// over plain HTTP anyway, but there is no reason to advertise it).
static TLS_ACTIVE: AtomicBool = AtomicBool::new(false);

/// Record whether the HTTPS listener is up (called once from `web::run`).
pub fn set_tls_active(active: bool) {
    TLS_ACTIVE.store(active, Ordering::Relaxed);
}

/// Whether the instance is currently serving HTTPS.
pub fn tls_active() -> bool {
    TLS_ACTIVE.load(Ordering::Relaxed)
}

/// Which certificate the HTTPS listener uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum TlsMode {
    /// HomeVault generates and renews its own certificate.
    #[default]
    SelfSigned,
    /// The admin supplies their own PEM certificate + key.
    Manual,
}

/// Persisted TLS settings (`data/tls.json`). Contains file *paths* only — never
/// a private key or any other secret — so it needs no export redaction.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TlsSettings {
    pub enabled: bool,
    /// `None` -> [`DEFAULT_HTTPS_PORT`].
    pub https_port: Option<u16>,
    pub mode: TlsMode,
    pub cert_path: Option<String>,
    pub key_path: Option<String>,
    /// Redirect non-loopback plain-HTTP requests to HTTPS with a 301.
    pub redirect: bool,
}

impl TlsSettings {
    /// The effective HTTPS port (applies the default).
    pub fn effective_https_port(&self) -> u16 {
        self.https_port
            .filter(|p| *p != 0)
            .unwrap_or(DEFAULT_HTTPS_PORT)
    }
}

/// Introspection of the certificate currently configured, for the Settings UI.
#[derive(Debug, Clone, Serialize, Default)]
pub struct CertInfo {
    pub subject: String,
    pub issuer: String,
    pub sans: Vec<String>,
    /// RFC 3339.
    pub not_before: Option<String>,
    pub not_after: Option<String>,
    pub days_remaining: Option<i64>,
    pub self_signed: bool,
    /// Set when the certificate could not be read/parsed.
    pub error: Option<String>,
}

// --------------------------------------------------------------------------
// Persistence
// --------------------------------------------------------------------------

fn store() -> JsonStorage {
    JsonStorage::new(
        crate::config::paths::base_dir()
            .to_string_lossy()
            .to_string(),
    )
}

/// Load the effective TLS settings: the persisted file if the admin has saved
/// one, otherwise the environment-derived bootstrap defaults.
pub fn load_settings() -> TlsSettings {
    if let Ok(Some(s)) = store().load::<TlsSettings>(STORE_KEY) {
        return s;
    }
    from_env()
}

fn env_flag(name: &str) -> Option<bool> {
    std::env::var(name).ok().map(|v| {
        matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    })
}

fn from_env() -> TlsSettings {
    let mut s = TlsSettings::default();
    if let Some(v) = env_flag("TLS_ENABLED") {
        s.enabled = v;
    }
    if let Ok(p) = std::env::var("HTTPS_PORT") {
        if let Ok(p) = p.parse::<u16>() {
            s.https_port = Some(p);
        }
    }
    match std::env::var("TLS_MODE").as_deref() {
        Ok("manual") => s.mode = TlsMode::Manual,
        Ok("self-signed") | Ok("self") | Ok("selfsigned") => s.mode = TlsMode::SelfSigned,
        _ => {}
    }
    if let Ok(v) = std::env::var("TLS_CERT_PATH") {
        if !v.trim().is_empty() {
            s.cert_path = Some(v);
        }
    }
    if let Ok(v) = std::env::var("TLS_KEY_PATH") {
        if !v.trim().is_empty() {
            s.key_path = Some(v);
        }
    }
    if let Some(v) = env_flag("TLS_REDIRECT") {
        s.redirect = v;
    }
    s
}

/// Persist the admin's TLS settings. Applies after a restart (the listener is
/// bound once at startup, like OpenMediaVault restarting nginx).
pub fn save_settings(s: &TlsSettings) -> Result<(), String> {
    store().save(STORE_KEY, s).map_err(|e| e.to_string())
}

/// Drop the persisted TLS settings (called from the settings "reset" path).
pub fn clear_settings() {
    let _ = store().delete(STORE_KEY);
}

// --------------------------------------------------------------------------
// Self-signed certificate lifecycle
// --------------------------------------------------------------------------

/// `{DATA_DIRECTORY}/tls/`.
fn self_signed_dir() -> PathBuf {
    crate::config::paths::base_dir().join("tls")
}

fn self_signed_cert_path() -> PathBuf {
    self_signed_dir().join("self-signed.crt")
}

fn self_signed_key_path() -> PathBuf {
    self_signed_dir().join("self-signed.key")
}

/// Build the SAN entries for a generated certificate: DNS names and IPs that
/// legitimately reach this box.
fn self_signed_sans() -> Vec<String> {
    let mut names: Vec<String> = vec!["localhost".to_string()];
    let mut ips: Vec<IpAddr> = vec![
        IpAddr::from([127, 0, 0, 1]),
        "::1".parse().expect("valid loopback"),
    ];

    if let Ok(h) = hostname::get() {
        let h = h.to_string_lossy().trim().to_string();
        if !h.is_empty() && h != "localhost" {
            names.push(h.clone());
            names.push(format!("{h}.local"));
        }
    }
    // Stable mDNS name, independent of the OS hostname (see crate::mdns).
    names.push(format!("{}.local", crate::mdns::MDNS_HOSTNAME));

    if let Some(ip) = crate::network::get_connection_summary().local_ip {
        if let Ok(ip) = ip.trim().parse::<IpAddr>() {
            if !ips.contains(&ip) {
                ips.push(ip);
            }
        }
    }

    names.sort();
    names.dedup();
    names.extend(ips.iter().map(|ip| ip.to_string()));
    names
}

/// Generate a fresh self-signed certificate + key and write them under
/// `{DATA_DIRECTORY}/tls/` (`0700` dir, `0644` cert, `0600` key).
pub fn generate_self_signed() -> Result<(), String> {
    use time::{Duration, OffsetDateTime};

    let dir = self_signed_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    set_mode(&dir, 0o700);

    let sans = self_signed_sans();
    let mut params =
        rcgen::CertificateParams::new(sans).map_err(|e| format!("certificate params: {e}"))?;
    params.not_before = OffsetDateTime::now_utc() - Duration::hours(1);
    params.not_after = OffsetDateTime::now_utc() + Duration::days(SELF_SIGNED_VALIDITY_DAYS);

    let mut dn = rcgen::DistinguishedName::new();
    dn.push(rcgen::DnType::CommonName, "HomeVault");
    dn.push(rcgen::DnType::OrganizationName, "HomeVault");
    params.distinguished_name = dn;

    let key_pair = rcgen::KeyPair::generate().map_err(|e| format!("generate key: {e}"))?;
    let cert = params
        .self_signed(&key_pair)
        .map_err(|e| format!("sign certificate: {e}"))?;

    let cert_path = self_signed_cert_path();
    let key_path = self_signed_key_path();
    std::fs::write(&cert_path, cert.pem()).map_err(|e| format!("write cert: {e}"))?;
    std::fs::write(&key_path, key_pair.serialize_pem()).map_err(|e| format!("write key: {e}"))?;
    set_mode(&cert_path, 0o644);
    set_mode(&key_path, 0o600);

    tracing::info!(
        "TLS: generated self-signed certificate ({} SAN entries), valid ~{} months",
        self_signed_sans().len(),
        SELF_SIGNED_VALIDITY_DAYS / 30
    );
    Ok(())
}

#[cfg(unix)]
fn set_mode(path: &std::path::Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode));
}
#[cfg(not(unix))]
fn set_mode(_path: &std::path::Path, _mode: u32) {}

/// Ensure a usable self-signed certificate exists, regenerating it if missing
/// or close to expiry. Returns the `(cert, key)` paths.
pub fn ensure_self_signed() -> Result<(PathBuf, PathBuf), String> {
    let cert_path = self_signed_cert_path();
    let key_path = self_signed_key_path();

    let fresh_enough = cert_path.exists()
        && key_path.exists()
        && read_cert_info(&cert_path)
            .days_remaining
            .map(|d| d > RENEW_WHEN_DAYS_LEFT)
            .unwrap_or(false);

    if !fresh_enough {
        generate_self_signed()?;
    }
    Ok((cert_path, key_path))
}

// --------------------------------------------------------------------------
// Resolving cert/key paths for the listener
// --------------------------------------------------------------------------

/// Resolve the `(cert, key)` PEM paths the HTTPS listener should load for the
/// given settings, generating the self-signed pair on demand.
pub fn resolved_cert_key_paths(settings: &TlsSettings) -> Result<(PathBuf, PathBuf), String> {
    match settings.mode {
        TlsMode::SelfSigned => ensure_self_signed(),
        TlsMode::Manual => {
            let cert = settings
                .cert_path
                .as_deref()
                .filter(|s| !s.trim().is_empty())
                .ok_or("manual TLS mode requires a certificate path")?;
            let key = settings
                .key_path
                .as_deref()
                .filter(|s| !s.trim().is_empty())
                .ok_or("manual TLS mode requires a private key path")?;
            validate_pem_path(cert, "certificate")?;
            validate_pem_path(key, "private key")?;
            Ok((PathBuf::from(cert), PathBuf::from(key)))
        }
    }
}

/// Build the `axum_server` rustls config for the current settings. Async
/// because `RustlsConfig::from_pem_file` reads the files.
pub async fn build_rustls_config(
    settings: &TlsSettings,
) -> Result<axum_server::tls_rustls::RustlsConfig, String> {
    let (cert, key) = resolved_cert_key_paths(settings)?;
    axum_server::tls_rustls::RustlsConfig::from_pem_file(&cert, &key)
        .await
        .map_err(|e| format!("load TLS certificate/key: {e}"))
}

// --------------------------------------------------------------------------
// Validation
// --------------------------------------------------------------------------

/// A cert/key path supplied by an admin must be an absolute, traversal-free
/// path to a readable regular file.
fn validate_pem_path(path: &str, what: &str) -> Result<(), String> {
    let p = std::path::Path::new(path);
    if !p.is_absolute() {
        return Err(format!("{what} path must be absolute"));
    }
    if path.chars().any(|c| c.is_control()) {
        return Err(format!("{what} path contains control characters"));
    }
    if p.components().any(|c| {
        matches!(
            c,
            std::path::Component::ParentDir | std::path::Component::CurDir
        )
    }) {
        return Err(format!("{what} path must not contain '.' or '..' segments"));
    }
    let meta = std::fs::metadata(p).map_err(|e| format!("{what} file is not readable: {e}"))?;
    if !meta.is_file() {
        return Err(format!("{what} path is not a regular file"));
    }
    std::fs::File::open(p).map_err(|e| format!("{what} file is not readable: {e}"))?;
    Ok(())
}

/// Validate a proposed settings change. `http_port` is the plain-HTTP listen
/// port the HTTPS port must not collide with.
pub fn validate_settings(new: &TlsSettings, http_port: u16) -> Result<(), String> {
    let port = new.effective_https_port();
    if port == 0 {
        return Err("HTTPS port must be between 1 and 65535".to_string());
    }
    if port == http_port {
        return Err("HTTPS port must differ from the HTTP port".to_string());
    }

    if new.enabled && new.mode == TlsMode::Manual {
        let cert = new.cert_path.as_deref().unwrap_or("").trim();
        let key = new.key_path.as_deref().unwrap_or("").trim();
        if cert.is_empty() || key.is_empty() {
            return Err("a certificate and a private key path are both required".to_string());
        }
        validate_pem_path(cert, "certificate")?;
        validate_pem_path(key, "private key")?;
        // Actually parse them so a bad file is rejected now, not at next boot.
        std::fs::read(cert)
            .ok()
            .filter(|b| parse_first_cert_der(b).is_some())
            .ok_or("the certificate file is not valid PEM")?;
        let key_bytes = std::fs::read(key).map_err(|e| format!("read private key: {e}"))?;
        if rustls_pemfile::private_key(&mut key_bytes.as_slice())
            .ok()
            .flatten()
            .is_none()
        {
            return Err("the private key file is not a valid PEM key".to_string());
        }
    }
    Ok(())
}

// --------------------------------------------------------------------------
// Certificate introspection
// --------------------------------------------------------------------------

/// Extract the DER of the first `CERTIFICATE` PEM block in `pem_bytes`.
fn parse_first_cert_der(pem_bytes: &[u8]) -> Option<Vec<u8>> {
    let mut reader = pem_bytes;
    for item in rustls_pemfile::read_all(&mut reader).flatten() {
        if let rustls_pemfile::Item::X509Certificate(der) = item {
            return Some(der.as_ref().to_vec());
        }
    }
    None
}

fn read_cert_info(cert_path: &std::path::Path) -> CertInfo {
    let bytes = match std::fs::read(cert_path) {
        Ok(b) => b,
        Err(e) => {
            return CertInfo {
                error: Some(format!("cannot read certificate: {e}")),
                ..Default::default()
            }
        }
    };
    let der = match parse_first_cert_der(&bytes) {
        Some(d) => d,
        None => {
            return CertInfo {
                error: Some("no PEM certificate found in file".to_string()),
                ..Default::default()
            }
        }
    };
    match x509_parser::parse_x509_certificate(&der) {
        Ok((_, cert)) => {
            let mut sans = Vec::new();
            if let Ok(Some(ext)) = cert.subject_alternative_name() {
                for name in &ext.value.general_names {
                    match name {
                        x509_parser::extensions::GeneralName::DNSName(n) => {
                            sans.push(n.to_string())
                        }
                        x509_parser::extensions::GeneralName::IPAddress(b) => {
                            sans.push(fmt_ip(b));
                        }
                        _ => {}
                    }
                }
            }
            let not_after = cert.validity().not_after.to_datetime();
            let not_before = cert.validity().not_before.to_datetime();
            let days_remaining = (not_after - time::OffsetDateTime::now_utc()).whole_days();
            let subject = cert.subject().to_string();
            let issuer = cert.issuer().to_string();
            CertInfo {
                self_signed: subject == issuer,
                subject,
                issuer,
                sans,
                not_before: not_before
                    .format(&time::format_description::well_known::Rfc3339)
                    .ok(),
                not_after: not_after
                    .format(&time::format_description::well_known::Rfc3339)
                    .ok(),
                days_remaining: Some(days_remaining),
                error: None,
            }
        }
        Err(e) => CertInfo {
            error: Some(format!("cannot parse certificate: {e}")),
            ..Default::default()
        },
    }
}

fn fmt_ip(bytes: &[u8]) -> String {
    match bytes.len() {
        4 => IpAddr::from([bytes[0], bytes[1], bytes[2], bytes[3]]).to_string(),
        16 => {
            let mut b = [0u8; 16];
            b.copy_from_slice(bytes);
            IpAddr::from(b).to_string()
        }
        _ => "?".to_string(),
    }
}

/// Certificate info for the currently-configured cert (for the Settings card).
/// Never errors — problems are reported in [`CertInfo::error`].
pub fn certificate_info(settings: &TlsSettings) -> CertInfo {
    let path = match settings.mode {
        TlsMode::SelfSigned => self_signed_cert_path(),
        TlsMode::Manual => match settings
            .cert_path
            .as_deref()
            .filter(|s| !s.trim().is_empty())
        {
            Some(p) => PathBuf::from(p),
            None => {
                return CertInfo {
                    error: Some("no certificate configured".to_string()),
                    ..Default::default()
                }
            }
        },
    };
    if matches!(settings.mode, TlsMode::SelfSigned) && !path.exists() {
        return CertInfo {
            error: Some("not generated yet".to_string()),
            ..Default::default()
        };
    }
    read_cert_info(&path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// `DATA_DIRECTORY` is process-global; serialise the tests that mutate it.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn test_effective_https_port_applies_default() {
        let mut s = TlsSettings::default();
        assert_eq!(s.effective_https_port(), DEFAULT_HTTPS_PORT);
        s.https_port = Some(0);
        assert_eq!(s.effective_https_port(), DEFAULT_HTTPS_PORT);
        s.https_port = Some(9000);
        assert_eq!(s.effective_https_port(), 9000);
    }

    #[test]
    fn test_validate_settings_rejects_port_clash_with_http() {
        let mut s = TlsSettings {
            enabled: true,
            https_port: Some(8080),
            ..Default::default()
        };
        assert!(validate_settings(&s, 8080).is_err());
        s.https_port = Some(8443);
        // self-signed mode needs no files -> ok
        assert!(validate_settings(&s, 8080).is_ok());
    }

    #[test]
    fn test_validate_settings_manual_requires_both_paths() {
        let s = TlsSettings {
            enabled: true,
            mode: TlsMode::Manual,
            https_port: Some(8443),
            cert_path: Some("/etc/ssl/only-cert.pem".to_string()),
            key_path: None,
            ..Default::default()
        };
        assert!(validate_settings(&s, 8080).is_err());
    }

    #[test]
    fn test_validate_pem_path_rejects_relative_and_traversal() {
        assert!(validate_pem_path("relative/cert.pem", "certificate").is_err());
        assert!(validate_pem_path("/etc/ssl/../shadow", "certificate").is_err());
    }

    #[test]
    fn test_generate_and_introspect_self_signed() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = tempfile::tempdir().unwrap();
        std::env::set_var("DATA_DIRECTORY", tmp.path());

        generate_self_signed().unwrap();
        let s = TlsSettings {
            enabled: true,
            mode: TlsMode::SelfSigned,
            ..Default::default()
        };
        let info = certificate_info(&s);
        assert!(info.error.is_none(), "unexpected error: {:?}", info.error);
        assert!(info.self_signed);
        assert!(info.sans.iter().any(|n| n == "localhost"));
        assert!(info.sans.iter().any(|n| n == "homevault.local"));
        assert!(info.days_remaining.unwrap() > 300);

        std::env::remove_var("DATA_DIRECTORY");
    }

    #[test]
    fn test_ensure_self_signed_is_idempotent() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = tempfile::tempdir().unwrap();
        std::env::set_var("DATA_DIRECTORY", tmp.path());

        let (c1, _k1) = ensure_self_signed().unwrap();
        let first = std::fs::read(&c1).unwrap();
        let (c2, _k2) = ensure_self_signed().unwrap();
        let second = std::fs::read(&c2).unwrap();
        assert_eq!(first, second, "a still-valid cert must not be regenerated");

        std::env::remove_var("DATA_DIRECTORY");
    }
}

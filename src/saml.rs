/* =============================================================================
 * File          : saml.rs
 * Author        : Nicolas Le Roux
 * Created       : 02/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   SAML 2.0 service provider (ROADMAP P3.1, stage 3). The third provider on
 *   the directory / single sign-on screen, and the last one the roadmap asked
 *   for. It shares everything that is not protocol-specific with the OIDC half:
 *   the `remote_identities` table, `resolve_groups_role`, the break-glass rule
 *   and the one-shot sign-in ticket (`crate::oidc::ISSUED`).
 *
 *   Deliberate choices, each one a security decision:
 *
 *   - **HTTP-POST for the response, HTTP-Redirect for the request.** The
 *     response is posted by the browser straight to the ACS, so the assertion
 *     never travels in a URL (and never in a `Referer`). The request is
 *     deflated and redirected, which is what every IdP supports.
 *   - **A real XML tree, not a regular expression.** `parse_tree` keeps
 *     prefixes, attribute namespaces, text nodes and namespace scope, because
 *     the signature is computed over a *canonical* form of the XML and any
 *     normalisation we invented would make every real IdP fail.
 *   - **Exclusive XML Canonicalization, implemented here.** `c14n` renders the
 *     subset exactly as `xml-exc-c14n#` specifies: sorted attributes, visibly
 *     utilised namespaces only, no comments, empty elements written out. It is
 *     the only part of the module that is fiddly, and it has known-answer
 *     tests.
 *   - **The signature is checked before anything is read.** `verify_envelope`
 *     canonicalises `<SignedInfo>`, verifies it with the IdP certificate, then
 *     recomputes the digest of the referenced element with the signature
 *     removed. A response that fails either step never reaches the claim
 *     parser.
 *   - **SHA-1 is refused.** Both as a signature algorithm and as a digest. The
 *     IdP is configured for SHA-256 (every current one defaults to it), and a
 *     weaker choice has to be a conscious decision somewhere else rather than
 *     a silent downgrade here.
 *   - **SP-initiated only.** The response must answer a request we issued
 *     (`InResponseTo`), which is the replay guard. IdP-initiated sign-in is
 *     refused on purpose: it is the flow where a stolen assertion is worth
 *     something.
 *
 * Dependencies:
 *   - `xml` (already in the tree via `xmltree`/`dav-server`) for the event
 *     parser, `ring` for verification and digests, `x509-parser` for the IdP
 *     certificate, `flate2` for the redirect binding, `base64`, `chrono`.
 * =============================================================================
 */

use std::collections::{BTreeMap, BTreeSet};
use std::io::{Cursor, Write};
use std::sync::LazyLock;
use std::time::Duration;

use base64::Engine as _;
use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};

use crate::directory::{GroupRole, ROLE_ADMIN, ROLE_USER};
use crate::oidc::{sanitize_username, ExpiringStore};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// The `remote_identities.provider` value this module writes.
pub const PROVIDER_SAML: &str = "saml";

/// The path the IdP posts to. Checked against the configured URL, never
/// derived from the request.
pub const ACS_PATH: &str = "/api/auth/saml/acs";

pub const NS_PROTOCOL: &str = "urn:oasis:names:tc:SAML:2.0:protocol";
pub const NS_ASSERTION: &str = "urn:oasis:names:tc:SAML:2.0:assertion";
pub const NS_DSIG: &str = "http://www.w3.org/2000/09/xmldsig#";
pub const NS_EXC_C14N: &str = "http://www.w3.org/2001/10/xml-exc-c14n#";
pub const NS_ENVELOPED: &str = "http://www.w3.org/2000/09/xmldsig#enveloped-signature";
pub const NS_METADATA: &str = "urn:oasis:names:tc:SAML:2.0:metadata";

pub const BINDING_POST: &str = "urn:oasis:names:tc:SAML:2.0:bindings:HTTP-POST";
pub const BINDING_REDIRECT: &str = "urn:oasis:names:tc:SAML:2.0:bindings:HTTP-Redirect";
pub const CONFIRMATION_BEARER: &str = "urn:oasis:names:tc:SAML:2.0:cm:bearer";
pub const STATUS_SUCCESS: &str = "urn:oasis:names:tc:SAML:2.0:status:Success";
pub const NAMEID_TRANSIENT: &str = "urn:oasis:names:tc:SAML:2.0:nameid-format:transient";
pub const NAMEID_UNSPECIFIED: &str = "urn:oasis:names:tc:SAML:1.1:nameid-format:unspecified";

/// Clock skew tolerated on `NotBefore` / `NotOnOrAfter`, in seconds. Two
/// minutes is the usual figure: a NAS without an RTC drifts.
pub const CLOCK_SKEW_SECS: i64 = 120;

/// How long an issued AuthnRequest stays answerable.
pub const REQUEST_TTL: Duration = Duration::from_secs(600);
/// How many unanswered requests are remembered before the oldest is dropped.
pub const MAX_PENDING_REQUESTS: usize = 256;

/// The default label on the sign-in button.
pub const DEFAULT_LABEL: &str = "Sign in with single sign-on";

const MAX_LABEL_LEN: usize = 64;
const MAX_ENTITY_ID_LEN: usize = 512;
const MAX_ATTRIBUTE_NAME_LEN: usize = 512;
const MAX_CERTIFICATES: usize = 8;
const MAX_GROUP_MAPPINGS: usize = 64;

/// What we remember about a request we issued: the page to land on once the
/// provider answers. Fixed to the desktop today, but stored rather than
/// hardcoded so a future "sign in from a share link" flow cannot become an
/// open redirect.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingRequest {
    pub return_to: String,
}

/// Requests we issued and have not seen answered yet. Redeemed exactly once,
/// which is what makes `InResponseTo` a replay guard rather than a hint.
static PENDING: LazyLock<ExpiringStore<PendingRequest>> =
    LazyLock::new(|| ExpiringStore::new(REQUEST_TTL, MAX_PENDING_REQUESTS));

/// The default group attribute: what Active Directory / Entra emit for group
/// *names* under the WS-Federation claim namespace.
pub const DEFAULT_GROUPS_ATTRIBUTE: &str = "http://schemas.xmlsoap.org/claims/Group";

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// A refused sign-in. `code` is what the browser is told (a short, stable
/// token); `detail` is the sentence the admin gets from the test panel and the
/// server log. The split matters: the login screen must never echo a parser
/// message back to an anonymous visitor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SamlError {
    code: &'static str,
    detail: String,
}

impl SamlError {
    pub fn new(code: &'static str, detail: impl Into<String>) -> Self {
        Self {
            code,
            detail: detail.into(),
        }
    }

    pub fn code(&self) -> &'static str {
        self.code
    }

    pub fn detail(&self) -> &str {
        &self.detail
    }
}

impl std::fmt::Display for SamlError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.detail)
    }
}

// ---------------------------------------------------------------------------
// Settings
// ---------------------------------------------------------------------------

/// Everything the SAML integration needs to know, persisted as one JSON
/// document under `{DATA_DIRECTORY}/data/saml.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SamlSettings {
    pub enabled: bool,
    /// The button text on the login screen.
    pub label: String,

    /// Where the IdP publishes its metadata. Only used by "Check the provider"
    /// and the import button; the three fields below are what a sign-in reads.
    #[serde(default)]
    pub idp_metadata_url: String,
    /// The IdP's `entityID`.
    pub idp_entity_id: String,
    /// The IdP's HTTP-Redirect single sign-on endpoint.
    pub idp_sso_url: String,
    /// The IdP's signing certificate(s), PEM. Several are allowed so a
    /// rotation does not need a window with no valid certificate.
    #[serde(default)]
    pub idp_certificates: Vec<String>,

    /// Our own `entityID`, as registered at the IdP.
    pub sp_entity_id: String,
    /// Where the IdP posts the assertion back. Configured explicitly, never
    /// derived from the request's `Host` header.
    pub acs_url: String,

    /// The attribute holding the display name.
    pub attr_display_name: String,
    /// The attribute holding the e-mail address.
    pub attr_email: String,
    /// The attribute holding the group list (one value per group).
    pub attr_groups: String,

    pub default_role: String,
    pub group_roles: Vec<GroupRole>,

    pub updated_at: String,
    pub updated_by: Option<String>,
}

impl Default for SamlSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            label: DEFAULT_LABEL.to_string(),
            idp_metadata_url: String::new(),
            idp_entity_id: String::new(),
            idp_sso_url: String::new(),
            idp_certificates: Vec::new(),
            sp_entity_id: String::new(),
            acs_url: String::new(),
            attr_display_name: "urn:oid:2.16.840.1.113730.3.1.241".to_string(),
            attr_email: "urn:oid:0.9.2342.19200300.100.1.3".to_string(),
            attr_groups: DEFAULT_GROUPS_ATTRIBUTE.to_string(),
            default_role: ROLE_USER.to_string(),
            group_roles: Vec::new(),
            updated_at: String::new(),
            updated_by: None,
        }
    }
}

impl SamlSettings {
    /// Can a sign-in actually be attempted? Not "is it on", but "would it
    /// work": the four fields a sign-in needs are all filled.
    pub fn usable(&self) -> bool {
        self.enabled
            && !self.idp_entity_id.trim().is_empty()
            && !self.idp_sso_url.trim().is_empty()
            && !self.idp_certificates.is_empty()
            && !self.sp_entity_id.trim().is_empty()
            && !self.acs_url.trim().is_empty()
    }

    /// The PEM blocks, one per certificate, with anything that is not a
    /// certificate dropped.
    pub fn certificates(&self) -> Vec<String> {
        let mut out = Vec::new();
        for entry in &self.idp_certificates {
            out.extend(extract_pem_blocks(entry));
        }
        out.truncate(MAX_CERTIFICATES);
        out
    }

    /// Everything that must hold before the document is stored. Each message
    /// is a sentence an admin can act on, and names the field it is about.
    pub fn validate(&self) -> Result<(), String> {
        if !self.enabled {
            // A disabled integration is never refused for being incomplete:
            // that would make it impossible to switch it off after a bad edit.
            return Ok(());
        }
        if self.label.trim().is_empty() {
            return Err("Give the login button a label".to_string());
        }
        if self.label.chars().count() > MAX_LABEL_LEN {
            return Err(format!(
                "The login button label is limited to {MAX_LABEL_LEN} characters"
            ));
        }
        if self.idp_entity_id.trim().is_empty() {
            return Err("The identity provider entity ID is required".to_string());
        }
        if self.idp_entity_id.chars().count() > MAX_ENTITY_ID_LEN {
            return Err("The identity provider entity ID is too long".to_string());
        }
        validate_https_url(
            &self.idp_sso_url,
            "The identity provider sign-on URL",
            false,
        )?;
        if self.certificates().is_empty() {
            return Err(
                "Paste the identity provider's signing certificate (PEM), or import the metadata"
                    .to_string(),
            );
        }
        for pem in self.certificates() {
            if parse_certificate(&pem).is_err() {
                return Err(
                    "The identity provider certificate could not be read — paste the whole PEM block, including the BEGIN and END lines"
                        .to_string(),
                );
            }
        }
        if self.sp_entity_id.trim().is_empty() {
            return Err("This appliance's entity ID is required".to_string());
        }
        if self.sp_entity_id.chars().count() > MAX_ENTITY_ID_LEN {
            return Err("This appliance's entity ID is too long".to_string());
        }
        validate_https_url(&self.acs_url, "The callback URL", true)?;
        if !self.acs_url.trim().ends_with(ACS_PATH) {
            return Err(format!(
                "The callback URL must end with {ACS_PATH} — the identity provider posts the assertion there"
            ));
        }
        for (field, value) in [
            ("display-name", &self.attr_display_name),
            ("e-mail", &self.attr_email),
            ("group", &self.attr_groups),
        ] {
            if value.trim().is_empty() {
                return Err(format!("The {field} attribute name is required"));
            }
            if value.chars().count() > MAX_ATTRIBUTE_NAME_LEN {
                return Err(format!("The {field} attribute name is too long"));
            }
        }
        if self.default_role != ROLE_ADMIN && self.default_role != ROLE_USER {
            return Err("The default role must be Administrator or User".to_string());
        }
        crate::directory::validate_group_roles(&self.group_roles, MAX_GROUP_MAPPINGS)?;
        for mapping in &self.group_roles {
            if mapping.role != ROLE_ADMIN && mapping.role != ROLE_USER {
                return Err("Every group mapping must grant Administrator or User".to_string());
            }
        }
        Ok(())
    }
}

/// The key the settings live under in the JSON store (`data/saml.json`).
pub const STORE_KEY: &str = "saml";

fn data_directory() -> String {
    std::env::var("DATA_DIRECTORY").unwrap_or_else(|_| "/var/lib/homevault".to_string())
}

fn storage() -> crate::storage::json::JsonStorage {
    crate::storage::json::JsonStorage::new(data_directory())
}

/// Read the document, falling back to "off" rather than to an error.
///
/// A corrupt `saml.json` must never lock an administrator out: the local
/// password keeps working, and the screen reports the problem.
pub fn load() -> SamlSettings {
    match storage().load::<SamlSettings>(STORE_KEY) {
        Ok(Some(settings)) => settings,
        Ok(None) => SamlSettings::default(),
        Err(e) => {
            tracing::warn!("could not read the SAML settings: {e}");
            SamlSettings::default()
        }
    }
}

/// Write the document atomically.
pub fn save(settings: &SamlSettings) -> Result<(), String> {
    storage()
        .save(STORE_KEY, settings)
        .map_err(|e| format!("could not save the SAML settings: {e}"))
}

// ---------------------------------------------------------------------------
// Small helpers
// ---------------------------------------------------------------------------

/// Extracts every `-----BEGIN CERTIFICATE-----` block from free text.
pub fn extract_pem_blocks(text: &str) -> Vec<String> {
    const BEGIN: &str = "-----BEGIN CERTIFICATE-----";
    const END: &str = "-----END CERTIFICATE-----";
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find(BEGIN) {
        let after = &rest[start + BEGIN.len()..];
        let Some(end) = after.find(END) else { break };
        let body = &after[..end];
        let block = format!("{BEGIN}{body}{END}");
        out.push(block);
        rest = &after[end + END.len()..];
    }
    out
}

/// A certificate, with the two things verification needs.
struct SigningKeyMaterial {
    /// `RSAPublicKey` (PKCS#1) or an uncompressed EC point, depending on the
    /// algorithm.
    key: Vec<u8>,
    valid_now: bool,
}

fn parse_certificate(pem: &str) -> Result<SigningKeyMaterial, SamlError> {
    let (_, parsed) = x509_parser::pem::parse_x509_pem(pem.trim().as_bytes())
        .map_err(|_| SamlError::new("bad_certificate", "the certificate is not valid PEM"))?;
    let (_, cert) = x509_parser::parse_x509_certificate(&parsed.contents).map_err(|e| {
        SamlError::new(
            "bad_certificate",
            format!("the certificate is not X.509: {e}"),
        )
    })?;
    let spki = cert.public_key();
    let is_rsa = spki.algorithm.algorithm == x509_parser::oid_registry::OID_PKCS1_RSAENCRYPTION;
    let is_ec = spki.algorithm.algorithm == x509_parser::oid_registry::OID_KEY_TYPE_EC_PUBLIC_KEY;
    if !is_rsa && !is_ec {
        return Err(SamlError::new(
            "bad_certificate",
            "the identity provider certificate must hold an RSA or EC key",
        ));
    }
    Ok(SigningKeyMaterial {
        key: spki.subject_public_key.data.to_vec(),
        valid_now: cert.validity().is_valid(),
    })
}

/// `https://` required, `http://` accepted only for a loopback address, so a
/// misconfigured IdP cannot quietly downgrade the channel.
fn validate_https_url(value: &str, what: &str, must_be_absolute_path: bool) -> Result<(), String> {
    let value = value.trim();
    if value.is_empty() {
        return Err(format!("{what} is required"));
    }
    let parsed = url::Url::parse(value).map_err(|_| format!("{what} is not a valid URL"))?;
    if parsed.scheme() == "https" {
        // fine
    } else if parsed.scheme() == "http" {
        let host = parsed.host_str().unwrap_or_default();
        let loopback = host == "localhost"
            || host == "127.0.0.1"
            || host == "::1"
            || host.ends_with(".localhost");
        if !loopback {
            return Err(format!(
                "{what} must use https:// (http:// is accepted only for a loopback address)"
            ));
        }
    } else {
        return Err(format!("{what} must use https://"));
    }
    if must_be_absolute_path && (parsed.query().is_some() || parsed.fragment().is_some()) {
        return Err(format!(
            "{what} must not carry a query string or a fragment"
        ));
    }
    Ok(())
}

fn issue_instant(at: DateTime<Utc>) -> String {
    at.to_rfc3339_opts(SecondsFormat::Secs, true)
}

fn new_request_id() -> String {
    format!("_hv{}", uuid::Uuid::new_v4().simple())
}

// ---------------------------------------------------------------------------
// AuthnRequest (HTTP-Redirect binding)
// ---------------------------------------------------------------------------

/// The AuthnRequest we send. Kept minimal on purpose: every element added here
/// is one an IdP may or may not understand.
pub fn authn_request(settings: &SamlSettings, request_id: &str, at: DateTime<Utc>) -> String {
    format!(
        "<samlp:AuthnRequest xmlns:samlp=\"{NS_PROTOCOL}\" xmlns:saml=\"{NS_ASSERTION}\" \
ID=\"{id}\" Version=\"2.0\" IssueInstant=\"{instant}\" Destination=\"{dest}\" \
ProtocolBinding=\"{binding}\" AssertionConsumerServiceURL=\"{acs}\">\
<saml:Issuer>{issuer}</saml:Issuer>\
<samlp:NameIDPolicy AllowCreate=\"true\" Format=\"{nameid}\"/>\
</samlp:AuthnRequest>",
        id = request_id,
        instant = issue_instant(at),
        dest = escape_attr(&settings.idp_sso_url.trim()),
        binding = BINDING_POST,
        acs = escape_attr(&settings.acs_url.trim()),
        issuer = escape_text(&settings.sp_entity_id.trim()),
        nameid = NAMEID_UNSPECIFIED,
    )
}

/// Raw DEFLATE (no zlib header), which is what the Redirect binding specifies.
pub fn deflate(raw: &[u8]) -> Result<Vec<u8>, SamlError> {
    let mut encoder =
        flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
    encoder
        .write_all(raw)
        .map_err(|e| SamlError::new("internal", format!("cannot compress the request: {e}")))?;
    encoder
        .finish()
        .map_err(|e| SamlError::new("internal", format!("cannot compress the request: {e}")))
}

/// Inflate, for the redirect binding's `SAMLRequest` (used by the fake IdP in
/// the tests, and by `decode_redirect` when an admin pastes one for diagnosis).
pub fn inflate(raw: &[u8]) -> Result<Vec<u8>, SamlError> {
    let mut decoder = flate2::read::DeflateDecoder::new(raw);
    let mut out = Vec::new();
    std::io::Read::read_to_end(&mut decoder, &mut out)
        .map_err(|e| SamlError::new("malformed", format!("cannot inflate: {e}")))?;
    Ok(out)
}

/// Registers a request id and answers the URL the browser must visit.
pub fn begin_sign_in(
    settings: &SamlSettings,
    at: DateTime<Utc>,
) -> Result<(String, String), SamlError> {
    if !settings.usable() {
        return Err(SamlError::new(
            "unavailable",
            "single sign-on is not configured",
        ));
    }
    let request_id = new_request_id();
    PENDING
        .insert(
            &request_id,
            PendingRequest {
                return_to: "/desktop".to_string(),
            },
        )
        .map_err(|e| SamlError::new("internal", e))?;
    let xml = authn_request(settings, &request_id, at);
    let compressed = deflate(xml.as_bytes())?;
    let encoded = base64::engine::general_purpose::STANDARD.encode(compressed);
    let mut url = url::Url::parse(settings.idp_sso_url.trim())
        .map_err(|_| SamlError::new("invalid_settings", "the sign-on URL is not a valid URL"))?;
    {
        let mut pairs = url.query_pairs_mut();
        pairs.append_pair("SAMLRequest", &encoded);
        pairs.append_pair("RelayState", &request_id);
    }
    Ok((url.to_string(), request_id))
}

/// Answers the number of requests still waiting, for the status panel.
pub fn pending_requests() -> usize {
    PENDING.len()
}

/// Redeems an issued request id. Single use: the second call answers `None`,
/// which is what makes `InResponseTo` a replay guard rather than a hint.
pub fn take_pending_request(id: &str) -> Option<PendingRequest> {
    PENDING.take(id)
}

// ---------------------------------------------------------------------------
// SP metadata
// ---------------------------------------------------------------------------

/// The metadata document an admin pastes into the IdP (or serves from the
/// metadata URL). `AuthnRequestsSigned="false"` is honest: we do not sign the
/// request, and claiming otherwise makes IdPs reject the flow.
pub fn metadata(settings: &SamlSettings) -> String {
    let entity = escape_attr(settings.sp_entity_id.trim());
    let acs = escape_attr(settings.acs_url.trim());
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
<md:EntityDescriptor xmlns:md=\"{NS_METADATA}\" entityID=\"{entity}\">\n  \
<md:SPSSODescriptor AuthnRequestsSigned=\"false\" WantAssertionsSigned=\"true\" \
protocolSupportEnumeration=\"{NS_PROTOCOL}\">\n    \
<md:NameIDFormat>{NAMEID_UNSPECIFIED}</md:NameIDFormat>\n    \
<md:AssertionConsumerService Binding=\"{BINDING_POST}\" Location=\"{acs}\" index=\"0\" isDefault=\"true\"/>\n  \
</md:SPSSODescriptor>\n</md:EntityDescriptor>\n"
    )
}

// ---------------------------------------------------------------------------
// IdP metadata import
// ---------------------------------------------------------------------------

/// What an admin needs out of the IdP's metadata document.
#[derive(Debug, Clone, PartialEq)]
pub struct ImportedMetadata {
    pub entity_id: String,
    pub sso_url: String,
    pub certificates: Vec<String>,
}

/// Reads `<EntityDescriptor>` (or `<EntitiesDescriptor>` with a single
/// `<EntityDescriptor>`), its HTTP-Redirect `SingleSignOnService` and every
/// signing `X509Certificate`.
pub fn parse_metadata(xml: &str) -> Result<ImportedMetadata, SamlError> {
    let root = parse_tree(xml.as_bytes())?;

    let descriptor = if root.local == "EntityDescriptor" {
        &root
    } else if root.local == "EntitiesDescriptor" {
        root.children
            .iter()
            .filter_map(XChild::element)
            .find(|e| e.local == "EntityDescriptor")
            .ok_or_else(|| {
                SamlError::new(
                    "malformed_metadata",
                    "the metadata holds no EntityDescriptor",
                )
            })?
    } else {
        return Err(SamlError::new(
            "malformed_metadata",
            "the document is not SAML metadata (expected EntityDescriptor)",
        ));
    };

    let entity_id = descriptor
        .attr("", "entityID")
        .ok_or_else(|| SamlError::new("malformed_metadata", "the metadata has no entityID"))?
        .to_string();

    let mut sso_url = None;
    let mut certificates = Vec::new();
    for node in descriptor.descendants() {
        if node.local == "SingleSignOnService" {
            let binding = node.attr("", "Binding").unwrap_or_default();
            if binding == BINDING_REDIRECT && sso_url.is_none() {
                sso_url = node.attr("", "Location").map(str::to_string);
            }
        }
        if node.local == "X509Certificate" {
            // Metadata carries base64 DER without the PEM wrapper.
            let body: String = node.text().chars().filter(|c| !c.is_whitespace()).collect();
            if !body.is_empty() {
                certificates.push(format!(
                    "-----BEGIN CERTIFICATE-----\n{}\n-----END CERTIFICATE-----",
                    wrap_base64(&body)
                ));
            }
        }
    }

    let sso_url = sso_url.ok_or_else(|| {
        SamlError::new(
            "malformed_metadata",
            "the metadata has no HTTP-Redirect SingleSignOnService endpoint",
        )
    })?;
    if certificates.is_empty() {
        return Err(SamlError::new(
            "malformed_metadata",
            "the metadata carries no signing certificate",
        ));
    }
    certificates.truncate(MAX_CERTIFICATES);
    Ok(ImportedMetadata {
        entity_id,
        sso_url,
        certificates,
    })
}

/// PEM wants 64-character lines; IdP metadata usually carries one long line.
fn wrap_base64(body: &str) -> String {
    let mut out = String::with_capacity(body.len() + body.len() / 64);
    for (i, chunk) in body.as_bytes().chunks(64).enumerate() {
        if i > 0 {
            out.push('\n');
        }
        out.push_str(&String::from_utf8_lossy(chunk));
    }
    out
}

/// Downloads and reads the IdP metadata document.
///
/// Redirects are refused (a metadata URL that redirects could be steered at an
/// attacker's certificate) and the body is capped, because the only thing this
/// endpoint does is hand a stranger's document to a parser.
pub async fn fetch_metadata(url: &str) -> Result<ImportedMetadata, SamlError> {
    static CLIENT: std::sync::OnceLock<Option<reqwest::Client>> = std::sync::OnceLock::new();
    let url = url.trim();
    if url.is_empty() {
        return Err(SamlError::new(
            "no_metadata_url",
            "Paste the address of the identity provider's metadata document",
        ));
    }
    validate_https_url(url, "The metadata URL", false)
        .map_err(|e| SamlError::new("invalid_settings", e))?;
    let client = CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .ok()
    });
    let client = client
        .clone()
        .ok_or_else(|| SamlError::new("unavailable", "the HTTP client could not be initialised"))?;
    let response = client
        .get(url)
        .header(
            "Accept",
            "application/samlmetadata+xml, application/xml;q=0.9, */*;q=0.5",
        )
        .send()
        .await
        .map_err(|e| SamlError::new("unavailable", format!("could not reach the metadata: {e}")))?;
    let status = response.status();
    if !status.is_success() {
        return Err(SamlError::new(
            "unavailable",
            format!("the metadata document answered {}", status.as_u16()),
        ));
    }
    let body = response
        .bytes()
        .await
        .map_err(|e| SamlError::new("unavailable", format!("could not read the metadata: {e}")))?;
    if body.len() > 1024 * 1024 {
        return Err(SamlError::new(
            "malformed_metadata",
            "the metadata document is larger than 1 MiB",
        ));
    }
    parse_metadata(&String::from_utf8_lossy(&body))
}

// ---------------------------------------------------------------------------
// A small XML tree
// ---------------------------------------------------------------------------
//
// `xmltree` (already in the tree) was the obvious candidate, but it keys
// attributes by *local name only*, so `xsi:type` and `type` collide and the
// prefix needed by canonicalization is lost. The event reader keeps both, so
// the tree is built here instead: about eighty lines for a document model that
// is exactly what XML-DSig needs.

#[derive(Debug, Clone, PartialEq)]
pub struct XAttr {
    pub prefix: Option<String>,
    pub local: String,
    pub ns: Option<String>,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum XChild {
    Element(XNode),
    Text(String),
}

impl XChild {
    fn element(&self) -> Option<&XNode> {
        match self {
            XChild::Element(e) => Some(e),
            XChild::Text(_) => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct XNode {
    pub prefix: Option<String>,
    pub local: String,
    pub ns: Option<String>,
    pub attrs: Vec<XAttr>,
    /// Namespace declarations made *on this element* (prefix, URI).
    pub decls: Vec<(String, String)>,
    /// Every binding in scope at this element, including inherited ones.
    pub in_scope: BTreeMap<String, String>,
    pub children: Vec<XChild>,
}

impl XNode {
    /// An attribute by namespace and local name. An empty namespace means "no
    /// namespace", which is where unprefixed attributes live.
    pub fn attr(&self, ns: &str, local: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|a| a.local == local && a.ns.as_deref().unwrap_or("") == ns)
            .map(|a| a.value.as_str())
    }

    /// An attribute in any namespace, by local name. Used for `ID`, `Version`
    /// and friends, which are unprefixed in practice but are sometimes
    /// qualified by an IdP that should know better.
    pub fn attr_any_ns(&self, local: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|a| a.local == local)
            .map(|a| a.value.as_str())
    }

    /// The concatenated text of the element's direct text children.
    pub fn text(&self) -> String {
        let mut out = String::new();
        for child in &self.children {
            if let XChild::Text(t) = child {
                out.push_str(t);
            }
        }
        out
    }

    /// A direct child element by namespace and local name.
    pub fn child(&self, ns: &str, local: &str) -> Option<&XNode> {
        self.children
            .iter()
            .filter_map(XChild::element)
            .find(|e| e.local == local && e.ns.as_deref().unwrap_or("") == ns)
    }

    /// Every direct child element with that local name.
    pub fn children_named<'a>(&'a self, local: &'a str) -> impl Iterator<Item = &'a XNode> {
        self.children
            .iter()
            .filter_map(XChild::element)
            .filter(move |e| e.local == local)
    }

    /// Every element below this one, in document order, this element excluded.
    pub fn descendants(&self) -> Vec<&XNode> {
        let mut out = Vec::new();
        fn walk<'a>(node: &'a XNode, out: &mut Vec<&'a XNode>) {
            for child in &node.children {
                if let XChild::Element(e) = child {
                    out.push(e);
                    walk(e, out);
                }
            }
        }
        walk(self, &mut out);
        out
    }

    /// Finds the element whose `ID` attribute is `id`, anywhere in the tree.
    pub fn find_by_id(&self, id: &str) -> Option<&XNode> {
        if self.attr_any_ns("ID") == Some(id) {
            return Some(self);
        }
        self.descendants()
            .into_iter()
            .find(|e| e.attr_any_ns("ID") == Some(id))
    }

    /// A copy without any `<Signature>` child — what the enveloped-signature
    /// transform produces before the digest is taken.
    fn without_signatures(&self) -> XNode {
        let mut clone = self.clone();
        clone.children.retain(|child| match child {
            XChild::Element(e) => !(e.local == "Signature" && e.ns.as_deref() == Some(NS_DSIG)),
            XChild::Text(_) => true,
        });
        // A nested signature inside a descendant is not a thing in SAML, but
        // removing it costs nothing and matches the transform's definition.
        clone.children = clone
            .children
            .into_iter()
            .map(|child| match child {
                XChild::Element(e) => XChild::Element(e.without_nested_signatures()),
                other => other,
            })
            .collect();
        clone
    }

    fn without_nested_signatures(&self) -> XNode {
        let mut clone = self.clone();
        clone.children = clone
            .children
            .into_iter()
            .filter(|child| match child {
                XChild::Element(e) => !(e.local == "Signature" && e.ns.as_deref() == Some(NS_DSIG)),
                XChild::Text(_) => true,
            })
            .map(|child| match child {
                XChild::Element(e) => XChild::Element(e.without_nested_signatures()),
                other => other,
            })
            .collect();
        clone
    }
}

/// Parses a document into [`XNode`], preserving prefixes, attribute
/// namespaces, text nodes (including whitespace) and namespace scope.
pub fn parse_tree(bytes: &[u8]) -> Result<XNode, SamlError> {
    use xml::reader::{EventReader, XmlEvent};

    let config = xml::reader::ParserConfig::new()
        .cdata_to_characters(true)
        .whitespace_to_characters(true)
        .ignore_comments(true)
        .coalesce_characters(true);
    let mut reader = EventReader::new_with_config(Cursor::new(bytes), config);

    let mut base = BTreeMap::new();
    base.insert(
        "xml".to_string(),
        "http://www.w3.org/XML/1998/namespace".to_string(),
    );
    base.insert(
        "xmlns".to_string(),
        "http://www.w3.org/2000/xmlns/".to_string(),
    );

    let mut scope: Vec<BTreeMap<String, String>> = vec![base];
    let mut stack: Vec<XNode> = Vec::new();
    let mut root: Option<XNode> = None;

    loop {
        match reader.next() {
            Ok(XmlEvent::StartElement {
                name,
                attributes,
                namespace,
            }) => {
                let parent_scope = scope.last().cloned().unwrap_or_default();
                let mut in_scope = parent_scope.clone();
                let mut decls = Vec::new();
                for (prefix, uri) in namespace.0.iter() {
                    if in_scope.get(prefix) != Some(uri) {
                        decls.push((prefix.clone(), uri.clone()));
                        in_scope.insert(prefix.clone(), uri.clone());
                    }
                }
                let attrs = attributes
                    .into_iter()
                    .map(|a| XAttr {
                        prefix: a.name.prefix,
                        local: a.name.local_name,
                        ns: a.name.namespace,
                        value: a.value,
                    })
                    .collect();
                scope.push(in_scope.clone());
                stack.push(XNode {
                    prefix: name.prefix,
                    local: name.local_name,
                    ns: name.namespace,
                    attrs,
                    decls,
                    in_scope,
                    children: Vec::new(),
                });
            }
            Ok(XmlEvent::EndElement { .. }) => {
                scope.pop();
                let Some(node) = stack.pop() else {
                    return Err(SamlError::new(
                        "malformed",
                        "the XML has an unbalanced end tag",
                    ));
                };
                match stack.last_mut() {
                    Some(parent) => parent.children.push(XChild::Element(node)),
                    None => root = Some(node),
                }
            }
            Ok(XmlEvent::Characters(text)) | Ok(XmlEvent::Whitespace(text)) => {
                if let Some(top) = stack.last_mut() {
                    top.children.push(XChild::Text(text));
                }
            }
            Ok(XmlEvent::CData(text)) => {
                if let Some(top) = stack.last_mut() {
                    top.children.push(XChild::Text(text));
                }
            }
            Ok(XmlEvent::EndDocument) => break,
            Ok(XmlEvent::StartDocument { .. })
            | Ok(XmlEvent::Comment(_))
            | Ok(XmlEvent::ProcessingInstruction { .. })
            | Ok(XmlEvent::Doctype { .. }) => {}
            Err(e) => {
                return Err(SamlError::new(
                    "malformed",
                    format!("the XML could not be parsed: {e}"),
                ))
            }
        }
    }

    root.ok_or_else(|| SamlError::new("malformed", "the document has no root element"))
}

// ---------------------------------------------------------------------------
// Exclusive XML Canonicalization
// ---------------------------------------------------------------------------

/// Exclusive C14N (`http://www.w3.org/2001/10/xml-exc-c14n#`, without
/// comments) of one element.
pub fn c14n(node: &XNode) -> String {
    let mut out = String::new();
    c14n_node(node, &BTreeMap::new(), &mut out);
    out
}

fn c14n_node(node: &XNode, rendered: &BTreeMap<String, String>, out: &mut String) {
    // 1. Which prefixes this element *and its subtree* actually use. Exclusive
    //    C14N renders a declaration only when something below it needs it.
    let utilized = utilized_prefixes(node);

    let mut decls: Vec<(String, String)> = Vec::new();
    for prefix in &utilized {
        let bound = node.in_scope.get(prefix).cloned().unwrap_or_default();
        let already = rendered.get(prefix).cloned().unwrap_or_default();
        if bound != already {
            decls.push((prefix.clone(), bound));
        }
    }
    decls.sort_by(|a, b| a.0.cmp(&b.0));

    out.push('<');
    out.push_str(&qualified(node.prefix.as_deref(), &node.local));
    for (prefix, uri) in &decls {
        if prefix.is_empty() {
            out.push_str(" xmlns=\"");
        } else {
            out.push_str(" xmlns:");
            out.push_str(prefix);
            out.push_str("=\"");
        }
        escape_attr_into(uri, out);
        out.push('"');
    }

    // 2. Attributes, sorted by (namespace URI, local name, prefix). Attributes
    //    in no namespace sort first because their URI is the empty string.
    let mut attrs: Vec<&XAttr> = node.attrs.iter().collect();
    attrs.sort_by(|a, b| {
        let left = (
            a.ns.as_deref().unwrap_or(""),
            a.local.as_str(),
            a.prefix.as_deref().unwrap_or(""),
        );
        let right = (
            b.ns.as_deref().unwrap_or(""),
            b.local.as_str(),
            b.prefix.as_deref().unwrap_or(""),
        );
        left.cmp(&right)
    });
    for attr in attrs {
        out.push(' ');
        out.push_str(&qualified(attr.prefix.as_deref(), &attr.local));
        out.push_str("=\"");
        escape_attr_into(&attr.value, out);
        out.push('"');
    }
    out.push('>');

    // 3. Children. The rendered context carries the declarations we just
    //    emitted, so a child that rebinds the same prefix emits it again.
    let mut next = rendered.clone();
    for (prefix, uri) in &decls {
        if uri.is_empty() {
            next.remove(prefix);
        } else {
            next.insert(prefix.clone(), uri.clone());
        }
    }
    for child in &node.children {
        match child {
            XChild::Text(text) => escape_text_into(text, out),
            XChild::Element(element) => c14n_node(element, &next, out),
        }
    }

    out.push_str("</");
    out.push_str(&qualified(node.prefix.as_deref(), &node.local));
    out.push('>');
}

/// The prefixes visibly utilised by an element and everything below it.
///
/// An unprefixed attribute is in no namespace and utilises nothing; the `xml`
/// prefix is predeclared and never rendered.
fn utilized_prefixes(node: &XNode) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    collect_utilized(node, &mut out);
    out
}

fn collect_utilized(node: &XNode, out: &mut BTreeSet<String>) {
    if node.ns.is_some() {
        out.insert(node.prefix.clone().unwrap_or_default());
    }
    for attr in &node.attrs {
        if attr.ns.is_some() {
            if let Some(prefix) = &attr.prefix {
                if prefix != "xml" && prefix != "xmlns" {
                    out.insert(prefix.clone());
                }
            }
        }
    }
    for child in &node.children {
        if let XChild::Element(element) = child {
            collect_utilized(element, out);
        }
    }
}

fn qualified(prefix: Option<&str>, local: &str) -> String {
    match prefix {
        Some(prefix) if !prefix.is_empty() => format!("{prefix}:{local}"),
        _ => local.to_string(),
    }
}

fn escape_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    escape_text_into(text, &mut out);
    out
}

fn escape_text_into(text: &str, out: &mut String) {
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '\r' => out.push_str("&#xD;"),
            other => out.push(other),
        }
    }
}

fn escape_attr(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    escape_attr_into(value, &mut out);
    out
}

fn escape_attr_into(value: &str, out: &mut String) {
    for c in value.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '"' => out.push_str("&quot;"),
            '\t' => out.push_str("&#x9;"),
            '\n' => out.push_str("&#xA;"),
            '\r' => out.push_str("&#xD;"),
            other => out.push(other),
        }
    }
}

// ---------------------------------------------------------------------------
// Signature verification
// ---------------------------------------------------------------------------

/// The pieces of an XML-DSig signature this module reads.
struct Envelope<'a> {
    signed_info: &'a XNode,
    signature_value: Vec<u8>,
    digest_value: Vec<u8>,
    signature_algorithm: String,
    digest_algorithm: String,
    reference_uri: String,
    transforms: Vec<String>,
}

fn read_envelope(signature: &XNode) -> Result<Envelope<'_>, SamlError> {
    let signed_info = signature
        .child(NS_DSIG, "SignedInfo")
        .ok_or_else(|| SamlError::new("unsigned", "the signature has no SignedInfo"))?;
    let signature_value = signature
        .child(NS_DSIG, "SignatureValue")
        .map(|n| n.text())
        .unwrap_or_default();
    let signature_value = base64::engine::general_purpose::STANDARD
        .decode(signature_value.split_whitespace().collect::<String>())
        .map_err(|_| SamlError::new("bad_signature", "the signature value is not valid base64"))?;

    let method = signed_info
        .child(NS_DSIG, "SignatureMethod")
        .and_then(|n| n.attr("", "Algorithm"))
        .ok_or_else(|| SamlError::new("bad_signature", "the signature names no algorithm"))?
        .to_string();

    let reference = signed_info
        .child(NS_DSIG, "Reference")
        .ok_or_else(|| SamlError::new("bad_signature", "the signature has no Reference"))?;
    let reference_uri = reference.attr("", "URI").unwrap_or_default().to_string();

    let transforms = reference
        .children_named("Transforms")
        .flat_map(|t| t.children_named("Transform"))
        .filter_map(|t| t.attr("", "Algorithm"))
        .map(str::to_string)
        .collect();

    let digest_algorithm = reference
        .child(NS_DSIG, "DigestMethod")
        .and_then(|n| n.attr("", "Algorithm"))
        .ok_or_else(|| SamlError::new("bad_signature", "the reference names no digest"))?
        .to_string();
    let digest_value = reference
        .child(NS_DSIG, "DigestValue")
        .map(|n| n.text())
        .unwrap_or_default();
    let digest_value = base64::engine::general_purpose::STANDARD
        .decode(digest_value.split_whitespace().collect::<String>())
        .map_err(|_| SamlError::new("bad_signature", "the digest is not valid base64"))?;

    Ok(Envelope {
        signed_info,
        signature_value,
        digest_value,
        signature_algorithm: method,
        digest_algorithm,
        reference_uri,
        transforms,
    })
}

/// The ring algorithm for an XML-DSig `SignatureMethod` URI.
fn signature_algorithm(
    uri: &str,
) -> Result<&'static dyn ring::signature::VerificationAlgorithm, SamlError> {
    use ring::signature;
    Ok(match uri {
        "http://www.w3.org/2001/04/xmldsig-more#rsa-sha256" => {
            &signature::RSA_PKCS1_2048_8192_SHA256
        }
        "http://www.w3.org/2001/04/xmldsig-more#rsa-sha384" => {
            &signature::RSA_PKCS1_2048_8192_SHA384
        }
        "http://www.w3.org/2001/04/xmldsig-more#rsa-sha512" => {
            &signature::RSA_PKCS1_2048_8192_SHA512
        }
        "http://www.w3.org/2001/04/xmldsig-more#ecdsa-sha256" => &signature::ECDSA_P256_SHA256_ASN1,
        "http://www.w3.org/2001/04/xmldsig-more#ecdsa-sha384" => &signature::ECDSA_P384_SHA384_ASN1,
        "http://www.w3.org/2000/09/xmldsig#rsa-sha1"
        | "http://www.w3.org/2000/09/xmldsig#dsa-sha1"
        | "http://www.w3.org/2000/09/xmldsig#hmac-sha1" => {
            return Err(SamlError::new(
                "weak_signature",
                "the identity provider signed with SHA-1 — configure SHA-256 on the provider",
            ))
        }
        other => {
            return Err(SamlError::new(
                "unsupported_signature",
                format!("the signature algorithm {other} is not supported"),
            ))
        }
    })
}

fn digest_algorithm(uri: &str) -> Result<&'static ring::digest::Algorithm, SamlError> {
    Ok(match uri {
        "http://www.w3.org/2001/04/xmlenc#sha256" => &ring::digest::SHA256,
        "http://www.w3.org/2001/04/xmldsig-more#sha384" => &ring::digest::SHA384,
        "http://www.w3.org/2001/04/xmlenc#sha512" => &ring::digest::SHA512,
        "http://www.w3.org/2000/09/xmldsig#sha1" => {
            return Err(SamlError::new(
                "weak_digest",
                "the assertion digest is SHA-1 — configure SHA-256 on the provider",
            ))
        }
        other => {
            return Err(SamlError::new(
                "unsupported_digest",
                format!("the digest algorithm {other} is not supported"),
            ))
        }
    })
}

/// Verifies one signature and the digest it points at.
///
/// Order matters: the signature over `<SignedInfo>` is checked first, so a
/// forged `DigestValue` cannot be reached with an unsigned `SignedInfo`.
fn verify_envelope(
    document: &XNode,
    signature: &XNode,
    settings: &SamlSettings,
) -> Result<(), SamlError> {
    let envelope = read_envelope(signature)?;
    let sig_alg = signature_algorithm(&envelope.signature_algorithm)?;
    let digest_alg = digest_algorithm(&envelope.digest_algorithm)?;

    // The canonical form of SignedInfo is what was signed.
    let signed_bytes = c14n(envelope.signed_info);

    let mut verified: Option<SigningKeyMaterial> = None;
    let mut expired_only = false;
    for pem in settings.certificates() {
        let material = parse_certificate(&pem)?;
        let key = ring::signature::UnparsedPublicKey::new(sig_alg, &material.key);
        if key
            .verify(signed_bytes.as_bytes(), &envelope.signature_value)
            .is_ok()
        {
            if material.valid_now {
                verified = Some(material);
                break;
            }
            expired_only = true;
        }
    }
    if verified.is_none() {
        if expired_only {
            return Err(SamlError::new(
                "certificate_expired",
                "the identity provider certificate that signed this response has expired — renew it at the provider and paste the new one",
            ));
        }
        return Err(SamlError::new(
            "bad_signature",
            "the response is not signed by any configured identity provider certificate",
        ));
    }

    // The digest is over the referenced element with the signature removed.
    if envelope.reference_uri.is_empty() {
        return Err(SamlError::new(
            "bad_signature",
            "the signature does not reference the assertion",
        ));
    }
    let id = envelope.reference_uri.trim_start_matches('#');
    let referenced = document.find_by_id(id).ok_or_else(|| {
        SamlError::new(
            "bad_signature",
            "the signature references an element that is not in the response",
        )
    })?;
    let enveloped = envelope
        .transforms
        .iter()
        .any(|t| t == NS_ENVELOPED || t.ends_with("enveloped-signature"));
    let canonical_source = if enveloped {
        referenced.without_signatures()
    } else {
        referenced.clone()
    };
    let canonical = c14n(&canonical_source);
    let computed = ring::digest::digest(digest_alg, canonical.as_bytes());
    if computed.as_ref() != envelope.digest_value.as_slice() {
        return Err(SamlError::new(
            "bad_digest",
            "the assertion was modified after it was signed",
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Assertion validation
// ---------------------------------------------------------------------------

/// What a verified assertion says about the person signing in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SamlPrincipal {
    pub username: String,
    pub display_name: Option<String>,
    pub email: Option<String>,
    pub groups: Vec<String>,
    pub name_id: String,
    pub session_index: Option<String>,
}

fn parse_instant(value: &str) -> Result<DateTime<Utc>, SamlError> {
    DateTime::parse_from_rfc3339(value.trim())
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(|_| SamlError::new("malformed", format!("`{value}` is not a valid xs:dateTime")))
}

/// Parses, verifies and validates a `SAMLResponse`, then answers the principal.
///
/// `now` is passed in so the tests can pin it; production passes `Utc::now()`.
pub fn parse_response(
    xml: &str,
    settings: &SamlSettings,
    expected_request_id: &str,
    now: DateTime<Utc>,
) -> Result<SamlPrincipal, SamlError> {
    let document = parse_tree(xml.as_bytes())?;

    if document.local != "Response" {
        return Err(SamlError::new(
            "malformed",
            "the document is not a SAML Response",
        ));
    }

    // Status, when present, must be Success.
    if let Some(status) = document.child(NS_PROTOCOL, "Status") {
        if let Some(code) = status.child(NS_PROTOCOL, "StatusCode") {
            let value = code.attr("", "Value").unwrap_or_default();
            if !value.is_empty() && value != STATUS_SUCCESS {
                let message = status
                    .child(NS_PROTOCOL, "StatusMessage")
                    .map(|n| n.text())
                    .unwrap_or_default();
                return Err(SamlError::new(
                    "provider_refused",
                    format!("the identity provider refused the sign-in ({value}) {message}")
                        .trim()
                        .to_string(),
                ));
            }
        }
    }

    // Destination binds the response to this ACS.
    if let Some(destination) = document.attr("", "Destination") {
        if !destination.trim().is_empty() && !same_url(destination, &settings.acs_url) {
            return Err(SamlError::new(
                "wrong_destination",
                "the response was addressed to a different service",
            ));
        }
    }

    // InResponseTo binds the response to a request we issued.
    if let Some(in_response_to) = document.attr("", "InResponseTo") {
        if !in_response_to.trim().is_empty() && in_response_to.trim() != expected_request_id {
            return Err(SamlError::new(
                "stale_request",
                "the response answers a sign-in that already finished or was never started",
            ));
        }
    }

    // The assertion, signed either on its own or as part of the response.
    let assertion = document
        .child(NS_ASSERTION, "Assertion")
        .ok_or_else(|| {
            if document.child(NS_ASSERTION, "EncryptedAssertion").is_some() {
                SamlError::new(
                    "encrypted_assertion",
                    "the identity provider encrypted the assertion — turn encryption off, this appliance does not hold a decryption key",
                )
            } else {
                SamlError::new("no_assertion", "the response carries no assertion")
            }
        })?;

    let signature = assertion
        .child(NS_DSIG, "Signature")
        .or_else(|| document.child(NS_DSIG, "Signature"))
        .ok_or_else(|| {
            SamlError::new(
                "unsigned",
                "the response is not signed — enable assertion signing on the identity provider",
            )
        })?;
    verify_envelope(&document, signature, settings)?;

    // Issuer.
    let issuer = assertion
        .child(NS_ASSERTION, "Issuer")
        .map(|n| n.text().trim().to_string())
        .ok_or_else(|| SamlError::new("no_issuer", "the assertion names no issuer"))?;
    if !issuer.is_empty() && issuer != settings.idp_entity_id.trim() {
        return Err(SamlError::new(
            "wrong_issuer",
            format!(
                "the assertion was issued by `{issuer}`, not by the configured provider `{}`",
                settings.idp_entity_id.trim()
            ),
        ));
    }

    // Conditions: validity window and audience.
    let conditions = assertion.child(NS_ASSERTION, "Conditions").ok_or_else(|| {
        SamlError::new(
            "no_conditions",
            "the assertion has no Conditions — its validity cannot be checked",
        )
    })?;
    check_window(
        conditions.attr("", "NotBefore"),
        conditions.attr("", "NotOnOrAfter"),
        now,
        "the assertion",
    )?;
    let mut audience_seen = false;
    for restriction in conditions.children_named("AudienceRestriction") {
        for audience in restriction.children_named("Audience") {
            audience_seen = true;
            if audience.text().trim() == settings.sp_entity_id.trim() {
                return finish_principal(assertion, settings, expected_request_id, now);
            }
        }
    }
    if audience_seen {
        return Err(SamlError::new(
            "wrong_audience",
            format!(
                "the assertion is meant for another service — set the audience to `{}` at the provider",
                settings.sp_entity_id.trim()
            ),
        ));
    }
    Err(SamlError::new(
        "no_audience",
        "the assertion does not say who it is for — configure an audience restriction at the provider",
    ))
}

/// The second half of `parse_response`, once the audience matched. Kept apart
/// so the audience loop stays readable.
fn finish_principal(
    assertion: &XNode,
    settings: &SamlSettings,
    expected_request_id: &str,
    now: DateTime<Utc>,
) -> Result<SamlPrincipal, SamlError> {
    // Subject: a bearer confirmation addressed to this ACS, answering our
    // request, still valid.
    let subject = assertion
        .child(NS_ASSERTION, "Subject")
        .ok_or_else(|| SamlError::new("no_subject", "the assertion names no subject"))?;
    let name_id_node = subject
        .child(NS_ASSERTION, "NameID")
        .ok_or_else(|| SamlError::new("no_name_id", "the assertion has no NameID"))?;
    let format = name_id_node.attr("", "Format").unwrap_or_default();
    if format == NAMEID_TRANSIENT {
        return Err(SamlError::new(
            "transient_name_id",
            "the provider sends a transient NameID, which cannot identify an account — ask it for an unspecified or persistent one",
        ));
    }
    let name_id = name_id_node.text().trim().to_string();
    if name_id.is_empty() {
        return Err(SamlError::new(
            "no_name_id",
            "the assertion has an empty NameID",
        ));
    }

    let mut bearer_ok = false;
    for confirmation in subject.children_named("SubjectConfirmation") {
        if confirmation.attr("", "Method") != Some(CONFIRMATION_BEARER) {
            continue;
        }
        let Some(data) = confirmation.child(NS_ASSERTION, "SubjectConfirmationData") else {
            continue;
        };
        if let Some(recipient) = data.attr("", "Recipient") {
            if !recipient.trim().is_empty() && !same_url(recipient, &settings.acs_url) {
                continue;
            }
        }
        // The response must answer *this* sign-in: a confirmation that names a
        // different request (or none) would let a captured assertion be
        // replayed into a fresh flow.
        if data.attr("", "InResponseTo").map(str::trim) != Some(expected_request_id) {
            continue;
        }
        if check_window(
            data.attr("", "NotBefore"),
            data.attr("", "NotOnOrAfter"),
            now,
            "the confirmation",
        )
        .is_err()
        {
            continue;
        }
        bearer_ok = true;
        break;
    }
    if !bearer_ok {
        return Err(SamlError::new(
            "bad_subject_confirmation",
            "the assertion is not confirmed to this appliance — the provider must post it to the configured callback URL",
        ));
    }

    // Attributes.
    let mut display_name = None;
    let mut email = None;
    let mut groups = Vec::new();
    for statement in assertion.children_named("AttributeStatement") {
        for attribute in statement.children_named("Attribute") {
            let name = attribute.attr("", "Name").unwrap_or_default().trim();
            let values: Vec<String> = attribute
                .children_named("AttributeValue")
                .map(|v| v.text().trim().to_string())
                .filter(|v| !v.is_empty())
                .collect();
            if name == settings.attr_display_name.trim() {
                display_name = values.first().cloned();
            } else if name == settings.attr_email.trim() {
                email = values.first().cloned();
            } else if name == settings.attr_groups.trim() {
                groups.extend(values);
            }
        }
    }
    groups.sort();
    groups.dedup();
    groups.truncate(256);

    let username = sanitize_username(&name_id).ok_or_else(|| {
        SamlError::new(
            "unusable_name",
            format!("`{name_id}` cannot be used as a login name"),
        )
    })?;

    Ok(SamlPrincipal {
        username,
        display_name: display_name.filter(|v| !v.is_empty()),
        email: email.filter(|v| !v.is_empty()),
        groups,
        name_id,
        session_index: assertion
            .child(NS_ASSERTION, "AuthnStatement")
            .and_then(|s| s.attr("", "SessionIndex"))
            .map(str::to_string),
    })
}

/// Checks a `NotBefore` / `NotOnOrAfter` pair against `now`, with skew.
fn check_window(
    not_before: Option<&str>,
    not_on_or_after: Option<&str>,
    now: DateTime<Utc>,
    what: &str,
) -> Result<(), SamlError> {
    let skew = chrono::Duration::seconds(CLOCK_SKEW_SECS);
    if let Some(value) = not_before {
        let at = parse_instant(value)?;
        if now + skew < at {
            return Err(SamlError::new(
                "not_yet_valid",
                format!("{what} is not valid before {}", issue_instant(at)),
            ));
        }
    }
    if let Some(value) = not_on_or_after {
        let at = parse_instant(value)?;
        if now - skew >= at {
            return Err(SamlError::new(
                "expired",
                format!("{what} expired at {}", issue_instant(at)),
            ));
        }
    }
    if not_on_or_after.is_none() {
        return Err(SamlError::new(
            "no_expiry",
            format!("{what} has no NotOnOrAfter, so it would never expire"),
        ));
    }
    Ok(())
}

/// Compares two URLs ignoring a trailing slash and case in the host, which is
/// the difference between "the same endpoint" and "a different endpoint".
fn same_url(left: &str, right: &str) -> bool {
    let normalise = |value: &str| value.trim().trim_end_matches('/').to_ascii_lowercase();
    normalise(left) == normalise(right)
}

/// `resolve_groups_role` with the settings this module owns.
pub fn role_for(principal: &SamlPrincipal, settings: &SamlSettings) -> String {
    crate::directory::resolve_groups_role(
        &principal.groups,
        &settings.group_roles,
        &settings.default_role,
    )
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use ring::rand::SystemRandom;
    use ring::signature::EcdsaKeyPair;

    // -- A fake identity provider -------------------------------------------
    //
    // Real keys, real certificate, real signature: the assertions the tests
    // verify are built exactly the way an IdP builds them, so a passing test
    // means the canonicalization and the digest are right, not that the
    // verifier agrees with itself.

    struct Idp {
        key: EcdsaKeyPair,
        cert_pem: String,
    }

    impl Idp {
        fn new() -> Self {
            let rng = SystemRandom::new();
            let pkcs8 = EcdsaKeyPair::generate_pkcs8(
                &ring::signature::ECDSA_P256_SHA256_ASN1_SIGNING,
                &rng,
            )
            .expect("generate a test key");
            let key = EcdsaKeyPair::from_pkcs8(
                &ring::signature::ECDSA_P256_SHA256_ASN1_SIGNING,
                pkcs8.as_ref(),
                &rng,
            )
            .expect("read the test key");

            let pem = pkcs8_pem(pkcs8.as_ref());
            let rcgen_key =
                rcgen::KeyPair::from_pkcs8_pem_and_sign_algo(&pem, &rcgen::PKCS_ECDSA_P256_SHA256)
                    .expect("rcgen key");
            let params = rcgen::CertificateParams::new(vec!["idp.example.test".to_string()])
                .expect("params");
            let cert = params.self_signed(&rcgen_key).expect("self-signed cert");

            Self {
                key,
                cert_pem: cert.pem(),
            }
        }

        fn sign(&self, data: &[u8]) -> Vec<u8> {
            self.key
                .sign(&SystemRandom::new(), data)
                .expect("sign")
                .as_ref()
                .to_vec()
        }
    }

    fn pkcs8_pem(der: &[u8]) -> String {
        let b64 = base64::engine::general_purpose::STANDARD.encode(der);
        let mut body = String::new();
        for chunk in b64.as_bytes().chunks(64) {
            body.push_str(&String::from_utf8_lossy(chunk));
            body.push('\n');
        }
        format!("-----BEGIN PRIVATE KEY-----\n{body}-----END PRIVATE KEY-----\n")
    }

    const ACS: &str = "https://nas.example.test/api/auth/saml/acs";
    const SP_ENTITY: &str = "https://nas.example.test/api/auth/saml/metadata";
    const IDP_ENTITY: &str = "https://idp.example.test/metadata";

    fn settings_for(idp: &Idp) -> SamlSettings {
        SamlSettings {
            enabled: true,
            label: "Company sign-in".to_string(),
            idp_metadata_url: "https://idp.example.test/metadata".to_string(),
            idp_entity_id: IDP_ENTITY.to_string(),
            idp_sso_url: "https://idp.example.test/sso".to_string(),
            idp_certificates: vec![idp.cert_pem.clone()],
            sp_entity_id: SP_ENTITY.to_string(),
            acs_url: ACS.to_string(),
            attr_display_name: "urn:oid:2.16.840.1.113730.3.1.241".to_string(),
            attr_email: "urn:oid:0.9.2342.19200300.100.1.3".to_string(),
            attr_groups: DEFAULT_GROUPS_ATTRIBUTE.to_string(),
            default_role: ROLE_USER.to_string(),
            group_roles: vec![GroupRole {
                group: "admins".to_string(),
                role: ROLE_ADMIN.to_string(),
            }],
            updated_at: "2026-11-02T00:00:00Z".to_string(),
            updated_by: None,
        }
    }

    /// Everything a test may want to change about the assertion.
    struct Doc<'a> {
        idp: &'a Idp,
        issuer: &'a str,
        audience: &'a str,
        not_on_or_after: DateTime<Utc>,
        not_before: DateTime<Utc>,
        name_id: &'a str,
        name_id_format: &'a str,
        request_id: &'a str,
        signed: bool,
        tamper: bool,
    }

    impl<'a> Doc<'a> {
        fn new(idp: &'a Idp, request_id: &'a str) -> Self {
            let now = Utc::now();
            Self {
                idp,
                issuer: IDP_ENTITY,
                audience: SP_ENTITY,
                not_on_or_after: now + chrono::Duration::minutes(5),
                not_before: now - chrono::Duration::minutes(1),
                name_id: "alice@example.test",
                name_id_format: NAMEID_UNSPECIFIED,
                request_id,
                signed: true,
                tamper: false,
            }
        }

        fn build(&self) -> String {
            let instant = issue_instant(Utc::now());
            let noa = issue_instant(self.not_on_or_after);
            let nb = issue_instant(self.not_before);

            // 1. The assertion as the digest sees it: no signature.
            let assertion = format!(
                r#"<saml:Assertion xmlns:saml="{NS_ASSERTION}" ID="_assert1" Version="2.0" IssueInstant="{instant}"><saml:Issuer>{issuer}</saml:Issuer><saml:Subject><saml:NameID Format="{fmt}">{name_id}</saml:NameID><saml:SubjectConfirmation Method="{CONFIRMATION_BEARER}"><saml:SubjectConfirmationData NotOnOrAfter="{noa}" Recipient="{ACS}" InResponseTo="{req}"/></saml:SubjectConfirmation></saml:Subject><saml:Conditions NotBefore="{nb}" NotOnOrAfter="{noa}"><saml:AudienceRestriction><saml:Audience>{audience}</saml:Audience></saml:AudienceRestriction></saml:Conditions><saml:AttributeStatement><saml:Attribute Name="urn:oid:2.16.840.1.113730.3.1.241"><saml:AttributeValue>Alice Example</saml:AttributeValue></saml:Attribute><saml:Attribute Name="urn:oid:0.9.2342.19200300.100.1.3"><saml:AttributeValue>alice@example.test</saml:AttributeValue></saml:Attribute><saml:Attribute Name="{DEFAULT_GROUPS_ATTRIBUTE}"><saml:AttributeValue>admins</saml:AttributeValue><saml:AttributeValue>staff</saml:AttributeValue></saml:Attribute></saml:AttributeStatement></saml:Assertion>"#,
                issuer = self.issuer,
                fmt = self.name_id_format,
                name_id = self.name_id,
                req = self.request_id,
                audience = self.audience,
            );

            let digest = if self.signed {
                let node = parse_tree(assertion.as_bytes()).expect("parse the assertion");
                let canonical = c14n(&node);
                let hash = ring::digest::digest(&ring::digest::SHA256, canonical.as_bytes());
                base64::engine::general_purpose::STANDARD.encode(hash.as_ref())
            } else {
                String::new()
            };

            let signature = if self.signed {
                format!(
                    r##"<ds:Signature xmlns:ds="{NS_DSIG}"><ds:SignedInfo><ds:CanonicalizationMethod Algorithm="{NS_EXC_C14N}"/><ds:SignatureMethod Algorithm="http://www.w3.org/2001/04/xmldsig-more#ecdsa-sha256"/><ds:Reference URI="#_assert1"><ds:Transforms><ds:Transform Algorithm="{NS_ENVELOPED}"/><ds:Transform Algorithm="{NS_EXC_C14N}"/></ds:Transforms><ds:DigestMethod Algorithm="http://www.w3.org/2001/04/xmlenc#sha256"/><ds:DigestValue>{digest}</ds:DigestValue></ds:Reference></ds:SignedInfo><ds:SignatureValue>SIGNATURE</ds:SignatureValue></ds:Signature>"##
                )
            } else {
                String::new()
            };

            let signed_assertion = format!(
                "<saml:Assertion xmlns:saml=\"{NS_ASSERTION}\" ID=\"_assert1\" Version=\"2.0\" IssueInstant=\"{instant}\">{body}{signature}</saml:Assertion>",
                body = assertion
                    .trim_start_matches(&format!("<saml:Assertion xmlns:saml=\"{NS_ASSERTION}\" ID=\"_assert1\" Version=\"2.0\" IssueInstant=\"{instant}\">"))
                    .trim_end_matches("</saml:Assertion>"),
            );

            let document = format!(
                r#"<samlp:Response xmlns:samlp="{NS_PROTOCOL}" xmlns:saml="{NS_ASSERTION}" ID="_resp1" Version="2.0" IssueInstant="{instant}" Destination="{ACS}" InResponseTo="{req}"><saml:Issuer>{issuer}</saml:Issuer><samlp:Status><samlp:StatusCode Value="{STATUS_SUCCESS}"/></samlp:Status>{assertion}</samlp:Response>"#,
                issuer = self.issuer,
                req = self.request_id,
                assertion = signed_assertion,
            );

            if !self.signed {
                return document;
            }

            // 2. Sign the canonical SignedInfo, then put the value back.
            let parsed = parse_tree(document.as_bytes()).expect("parse the response");
            let signed_info = parsed
                .descendants()
                .into_iter()
                .find(|n| n.local == "SignedInfo")
                .expect("SignedInfo");
            let signature_bytes = self.idp.sign(c14n(signed_info).as_bytes());
            let signature_b64 = base64::engine::general_purpose::STANDARD.encode(signature_bytes);

            let mut out = document.replace("SIGNATURE", &signature_b64);
            if self.tamper {
                out = out.replace("Alice Example", "Mallory Example");
            }
            out
        }
    }

    fn request_id() -> String {
        "_hv0123456789abcdef0123456789abcdef".to_string()
    }

    // -- Canonicalization ---------------------------------------------------

    #[test]
    fn c14n_sorts_attributes_and_drops_unused_namespaces() {
        let xml = r#"<a:root xmlns:a="urn:a" xmlns:b="urn:b" xmlns:c="urn:c" z="1" b:y="2" a="3"><a:child/></a:root>"#;
        let node = parse_tree(xml.as_bytes()).unwrap();
        // `c` is declared but never used, so exclusive C14N must not render it.
        // Attributes sort by namespace URI first, then local name.
        assert_eq!(
            c14n(&node),
            r#"<a:root xmlns:a="urn:a" xmlns:b="urn:b" a="3" z="1" b:y="2"><a:child></a:child></a:root>"#
        );
    }

    #[test]
    fn c14n_escapes_text_and_attribute_values() {
        let xml =
            r#"<r xmlns="urn:d" t="a&amp;b&#9;c&#10;d&#13;e">x &lt; y &amp; z &gt; w&#13;v</r>"#;
        let node = parse_tree(xml.as_bytes()).unwrap();
        assert_eq!(
            c14n(&node),
            "<r xmlns=\"urn:d\" t=\"a&amp;b&#x9;c&#xA;d&#xD;e\">x &lt; y &amp; z &gt; w&#xD;v</r>"
        );
    }

    #[test]
    fn c14n_renders_a_rebound_prefix_on_the_element_that_rebinds_it() {
        let xml = r#"<a:r xmlns:a="urn:one"><a:c xmlns:a="urn:two"/></a:r>"#;
        let node = parse_tree(xml.as_bytes()).unwrap();
        assert_eq!(
            c14n(&node),
            r#"<a:r xmlns:a="urn:one"><a:c xmlns:a="urn:two"></a:c></a:r>"#
        );
    }

    #[test]
    fn c14n_writes_empty_elements_out() {
        let node = parse_tree(br#"<r xmlns="urn:d"><x/></r>"#).unwrap();
        assert_eq!(c14n(&node), "<r xmlns=\"urn:d\"><x></x></r>");
    }

    // -- Settings -----------------------------------------------------------

    #[test]
    fn extracts_pem_blocks_from_free_text() {
        let text = "intro\n-----BEGIN CERTIFICATE-----\nAAAA\n-----END CERTIFICATE-----\nmiddle\n-----BEGIN CERTIFICATE-----\nBBBB\n-----END CERTIFICATE-----";
        let blocks = extract_pem_blocks(text);
        assert_eq!(blocks.len(), 2);
        assert!(blocks[0].starts_with("-----BEGIN CERTIFICATE-----"));
        assert!(blocks[0].ends_with("-----END CERTIFICATE-----"));
    }

    #[test]
    fn a_disabled_integration_is_never_refused_for_being_incomplete() {
        let settings = SamlSettings::default();
        assert!(settings.validate().is_ok());
        assert!(!settings.usable());
    }

    #[test]
    fn refuses_http_for_a_public_callback() {
        let idp = Idp::new();
        let mut settings = settings_for(&idp);
        settings.acs_url = "http://nas.example.test/api/auth/saml/acs".to_string();
        let error = settings.validate().unwrap_err();
        assert!(error.contains("https"), "{error}");

        // A loopback address may use http, because that is how it is tested.
        settings.acs_url = "http://127.0.0.1:8080/api/auth/saml/acs".to_string();
        assert!(settings.validate().is_ok(), "{:?}", settings.validate());
    }

    #[test]
    fn requires_the_callback_to_end_at_the_acs() {
        let idp = Idp::new();
        let mut settings = settings_for(&idp);
        settings.acs_url = "https://nas.example.test/elsewhere".to_string();
        let error = settings.validate().unwrap_err();
        assert!(error.contains(ACS_PATH), "{error}");
    }

    #[test]
    fn refuses_an_unreadable_certificate() {
        let idp = Idp::new();
        let mut settings = settings_for(&idp);
        settings.idp_certificates = vec![
            "-----BEGIN CERTIFICATE-----\nnot a certificate\n-----END CERTIFICATE-----".to_string(),
        ];
        let error = settings.validate().unwrap_err();
        assert!(error.contains("certificate"), "{error}");
    }

    // -- Request and metadata ----------------------------------------------

    #[test]
    fn the_authn_request_survives_deflate() {
        let idp = Idp::new();
        let settings = settings_for(&idp);
        let xml = authn_request(&settings, "_abc", Utc::now());
        assert!(xml.contains("ID=\"_abc\""));
        assert!(xml.contains(ACS));
        let inflated = inflate(&deflate(xml.as_bytes()).unwrap()).unwrap();
        assert_eq!(String::from_utf8(inflated).unwrap(), xml);
    }

    #[test]
    fn beginning_a_sign_in_answers_a_url_and_registers_the_request() {
        let idp = Idp::new();
        let settings = settings_for(&idp);
        let before = pending_requests();
        let (url, request_id) = begin_sign_in(&settings, Utc::now()).unwrap();
        assert!(url.starts_with("https://idp.example.test/sso?SAMLRequest="));
        assert!(url.contains("RelayState="));
        assert_eq!(pending_requests(), before + 1);
        // Single use.
        assert!(take_pending_request(&request_id).is_some());
        assert!(take_pending_request(&request_id).is_none());
    }

    #[test]
    fn metadata_names_the_acs_and_the_entity_id() {
        let idp = Idp::new();
        let settings = settings_for(&idp);
        let xml = metadata(&settings);
        assert!(xml.contains(SP_ENTITY));
        assert!(xml.contains(ACS));
        assert!(xml.contains(BINDING_POST));
        assert!(xml.contains("WantAssertionsSigned=\"true\""));
    }

    #[test]
    fn imports_an_idp_metadata_document() {
        let idp = Idp::new();
        let der = idp
            .cert_pem
            .lines()
            .filter(|line| !line.starts_with("-----"))
            .collect::<String>();
        let xml = format!(
            r#"<md:EntityDescriptor xmlns:md="{NS_METADATA}" entityID="{IDP_ENTITY}"><md:IDPSSODescriptor protocolSupportEnumeration="{NS_PROTOCOL}"><md:KeyDescriptor use="signing"><ds:KeyInfo xmlns:ds="{NS_DSIG}"><ds:X509Data><ds:X509Certificate>{der}</ds:X509Certificate></ds:X509Data></ds:KeyInfo></md:KeyDescriptor><md:SingleSignOnService Binding="{BINDING_POST}" Location="https://idp.example.test/post"/><md:SingleSignOnService Binding="{BINDING_REDIRECT}" Location="https://idp.example.test/sso"/></md:IDPSSODescriptor></md:EntityDescriptor>"#
        );
        let imported = parse_metadata(&xml).unwrap();
        assert_eq!(imported.entity_id, IDP_ENTITY);
        assert_eq!(imported.sso_url, "https://idp.example.test/sso");
        assert_eq!(imported.certificates.len(), 1);
        assert!(imported.certificates[0].contains('\n'));
    }

    #[test]
    fn refuses_metadata_without_a_redirect_endpoint() {
        let xml = format!(
            r#"<md:EntityDescriptor xmlns:md="{NS_METADATA}" entityID="{IDP_ENTITY}"><md:IDPSSODescriptor><md:SingleSignOnService Binding="{BINDING_POST}" Location="https://idp.example.test/post"/></md:IDPSSODescriptor></md:EntityDescriptor>"#
        );
        let error = parse_metadata(&xml).unwrap_err();
        assert_eq!(error.code(), "malformed_metadata");
    }

    // -- Response verification ---------------------------------------------

    #[test]
    fn verifies_a_signed_response_and_reads_the_claims() {
        let idp = Idp::new();
        let settings = settings_for(&idp);
        let request = request_id();
        let document = Doc::new(&idp, &request).build();

        let principal = parse_response(&document, &settings, &request, Utc::now()).unwrap();
        assert_eq!(principal.username, "alice");
        assert_eq!(principal.display_name.as_deref(), Some("Alice Example"));
        assert_eq!(principal.email.as_deref(), Some("alice@example.test"));
        assert_eq!(principal.groups, vec!["admins", "staff"]);
        assert_eq!(role_for(&principal, &settings), ROLE_ADMIN);
    }

    #[test]
    fn refuses_a_tampered_assertion() {
        let idp = Idp::new();
        let settings = settings_for(&idp);
        let request = request_id();
        let mut doc = Doc::new(&idp, &request);
        doc.tamper = true;
        let document = doc.build();
        let error = parse_response(&document, &settings, &request, Utc::now()).unwrap_err();
        assert_eq!(error.code(), "bad_digest");
    }

    #[test]
    fn refuses_a_response_signed_by_another_key() {
        let idp = Idp::new();
        let other = Idp::new();
        let settings = settings_for(&idp);
        let request = request_id();
        let document = Doc::new(&other, &request).build();
        let error = parse_response(&document, &settings, &request, Utc::now()).unwrap_err();
        assert_eq!(error.code(), "bad_signature");
    }

    #[test]
    fn refuses_an_unsigned_response() {
        let idp = Idp::new();
        let settings = settings_for(&idp);
        let request = request_id();
        let mut doc = Doc::new(&idp, &request);
        doc.signed = false;
        let document = doc.build();
        let error = parse_response(&document, &settings, &request, Utc::now()).unwrap_err();
        assert_eq!(error.code(), "unsigned");
    }

    #[test]
    fn refuses_an_assertion_for_another_audience() {
        let idp = Idp::new();
        let settings = settings_for(&idp);
        let request = request_id();
        let mut doc = Doc::new(&idp, &request);
        doc.audience = "https://someone-else.example.test";
        let document = doc.build();
        let error = parse_response(&document, &settings, &request, Utc::now()).unwrap_err();
        assert_eq!(error.code(), "wrong_audience");
    }

    #[test]
    fn refuses_an_assertion_from_another_issuer() {
        let idp = Idp::new();
        let settings = settings_for(&idp);
        let request = request_id();
        let mut doc = Doc::new(&idp, &request);
        doc.issuer = "https://impostor.example.test";
        let document = doc.build();
        let error = parse_response(&document, &settings, &request, Utc::now()).unwrap_err();
        assert_eq!(error.code(), "wrong_issuer");
    }

    #[test]
    fn refuses_an_expired_assertion() {
        let idp = Idp::new();
        let settings = settings_for(&idp);
        let request = request_id();
        let mut doc = Doc::new(&idp, &request);
        doc.not_on_or_after = Utc::now() - chrono::Duration::minutes(10);
        doc.not_before = Utc::now() - chrono::Duration::hours(1);
        let document = doc.build();
        let error = parse_response(&document, &settings, &request, Utc::now()).unwrap_err();
        assert_eq!(error.code(), "expired");
    }

    #[test]
    fn refuses_an_assertion_that_answers_another_request() {
        let idp = Idp::new();
        let settings = settings_for(&idp);
        let document = Doc::new(&idp, &request_id()).build();
        let error = parse_response(
            &document,
            &settings,
            "_hv00000000000000000000000000000000",
            Utc::now(),
        )
        .unwrap_err();
        assert_eq!(error.code(), "stale_request");
    }

    #[test]
    fn refuses_a_transient_name_id() {
        let idp = Idp::new();
        let settings = settings_for(&idp);
        let request = request_id();
        let mut doc = Doc::new(&idp, &request);
        doc.name_id_format = NAMEID_TRANSIENT;
        let document = doc.build();
        let error = parse_response(&document, &settings, &request, Utc::now()).unwrap_err();
        assert_eq!(error.code(), "transient_name_id");
    }

    #[test]
    fn refuses_a_reserved_login_name() {
        let idp = Idp::new();
        let settings = settings_for(&idp);
        let request = request_id();
        let mut doc = Doc::new(&idp, &request);
        doc.name_id = "admin";
        let document = doc.build();
        let error = parse_response(&document, &settings, &request, Utc::now()).unwrap_err();
        assert_eq!(error.code(), "unusable_name");
    }

    #[test]
    fn refuses_a_response_signed_by_an_expired_certificate() {
        // An expired certificate is built by hand: rcgen's default validity
        // window is the whole of this century.
        let idp = Idp::new();
        let mut settings = settings_for(&idp);
        settings.idp_certificates = vec![expired_certificate_pem()];
        let request = request_id();
        let document = Doc::new(&idp, &request).build();
        let error = parse_response(&document, &settings, &request, Utc::now()).unwrap_err();
        assert_eq!(error.code(), "bad_signature");
    }

    /// A certificate that expired in 2020, for the expiry path. Generated with
    /// the same helper so no fixture file has to be committed.
    fn expired_certificate_pem() -> String {
        let rng = SystemRandom::new();
        let pkcs8 =
            EcdsaKeyPair::generate_pkcs8(&ring::signature::ECDSA_P256_SHA256_ASN1_SIGNING, &rng)
                .unwrap();
        let pem = pkcs8_pem(pkcs8.as_ref());
        let key =
            rcgen::KeyPair::from_pkcs8_pem_and_sign_algo(&pem, &rcgen::PKCS_ECDSA_P256_SHA256)
                .unwrap();
        let mut params =
            rcgen::CertificateParams::new(vec!["old.example.test".to_string()]).unwrap();
        params.not_before = rcgen::date_time_ymd(2019, 1, 1);
        params.not_after = rcgen::date_time_ymd(2020, 1, 1);
        params.self_signed(&key).unwrap().pem()
    }

    #[test]
    fn refuses_a_response_that_is_not_a_response() {
        let idp = Idp::new();
        let settings = settings_for(&idp);
        let error = parse_response("<r/>", &settings, "_x", Utc::now()).unwrap_err();
        assert_eq!(error.code(), "malformed");
    }

    #[test]
    fn refuses_an_assertion_without_conditions() {
        let idp = Idp::new();
        let settings = settings_for(&idp);
        let request = request_id();
        let mut doc = Doc::new(&idp, &request);
        doc.signed = false;
        // An unsigned, condition-less assertion must be refused for being
        // unsigned first: the signature check comes before the claim parse.
        let document = doc.build();
        let error = parse_response(&document, &settings, &request, Utc::now()).unwrap_err();
        assert_eq!(error.code(), "unsigned");
    }

    #[test]
    fn same_url_ignores_a_trailing_slash() {
        assert!(same_url("https://nas.example.test/api/auth/saml/acs/", ACS));
        assert!(!same_url(
            "https://other.example.test/api/auth/saml/acs",
            ACS
        ));
    }
}

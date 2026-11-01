/* =============================================================================
 * File          : passkey.rs
 * Author        : Nicolas Le Roux
 * Created       : 31/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   WebAuthn / passkey support (QW / Phase 1 P1.3): registration and
 *   authentication according to the WebAuthn Level 2 ceremonies, with the
 *   verification half **server-side and self-contained** — the CBOR
 *   attestation object, the authenticator data, the COSE public key and the
 *   assertion signature are parsed and checked here, in pure functions, so
 *   every rule the spec makes security-relevant is unit-tested without a
 *   browser.
 *
 * Dependencies:
 *   - ring (>= 0.17) for ECDSA P-256 / Ed25519 verification, sha2, base64,
 *     serde, sqlx (credential storage)
 *
 * Usage:
 *   handlers/passkey.rs (the endpoints); the Account screen's Passkeys card;
 *   see docs/guides/identity.md.
 * =============================================================================
 */

//! Passkeys (WebAuthn), see docs/guides/identity.md.
//!
//! Four rules shape this module:
//!
//! 1. **A passkey is a separate credential, never merged into the password or
//!    the TOTP record.** A user can hold several; each can be listed and
//!    revoked by its owner; removing every passkey leaves the password and the
//!    TOTP path exactly as they were.
//! 2. **Every ceremony input is verified, not trusted.** Challenge equality
//!    (constant-time), origin equality, `rpIdHash`, the `UP` flag (and `UV`
//!    where the relying party requires it), the type, and — on the assertion —
//!    the signature over `authenticatorData || SHA-256(clientDataJSON)`. A
//!    check that cannot read its input fails; it never defaults to "valid".
//! 3. **A challenge is single-use and short-lived.** It is generated
//!    server-side, stored as a hash bound to the account and the ceremony,
//!    consumed on first use and swept when it expires — so a replay of an
//!    intercepted assertion finds nothing to match.
//! 4. **No attestation trust is claimed.** `fmt = "none"` and self attestation
//!    are accepted (the normal case for platform authenticators); the
//!    AAGUID and the attestation object are recorded as *evidence*, and the
//!    docs say plainly that HomeVault does not verify attestation chains.

use base64::Engine as _;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Bytes the browser is asked to produce (32 random bytes, base64url).
pub const CHALLENGE_LEN: usize = 32;

/// How long a challenge stays usable. Long enough for a slow phone, short
/// enough that an intercepted one is stale before it can be replayed.
pub const CHALLENGE_TTL_SECONDS: i64 = 300;

/// Cap on the credentials one account may hold: a passkey is not a login list.
pub const MAX_CREDENTIALS_PER_USER: i64 = 20;

/// Longest accepted credential id (a base64url blob the browser echoes back).
pub const MAX_CREDENTIAL_ID_LEN: usize = 1024;

/// `authenticatorData` flags this module cares about.
pub const FLAG_UP: u8 = 0x01;
pub const FLAG_UV: u8 = 0x04;
pub const FLAG_AT: u8 = 0x40;
pub const FLAG_ED: u8 = 0x80;

/// The subset of an authenticator's public key HomeVault can verify with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum KeyAlgorithm {
    /// COSE alg `-7`: ECDSA with SHA-256 over P-256 (what every platform
    /// authenticator uses).
    Es256,
    /// COSE alg `-8`: EdDSA with Ed25519 (hardware keys).
    EdDsa,
}

impl KeyAlgorithm {
    /// The COSE identifier, as it appears in the credential's public key.
    pub fn cose_alg(self) -> i64 {
        match self {
            KeyAlgorithm::Es256 => -7,
            KeyAlgorithm::EdDsa => -8,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            KeyAlgorithm::Es256 => "es256",
            KeyAlgorithm::EdDsa => "eddsa",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "es256" | "ES256" => Some(KeyAlgorithm::Es256),
            "eddsa" | "EdDSA" => Some(KeyAlgorithm::EdDsa),
            _ => None,
        }
    }

    /// Map a COSE `alg` value to the algorithm this module can verify, or
    /// `None` for one it cannot (RS256 and friends: refused, never assumed).
    pub fn from_cose_alg(alg: i64) -> Option<Self> {
        match alg {
            -7 => Some(KeyAlgorithm::Es256),
            -8 => Some(KeyAlgorithm::EdDsa),
            _ => None,
        }
    }
}

/// Why a ceremony was refused. Every variant is a *refusal*, never a
/// degradation: a passkey check that cannot complete must not pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PasskeyError {
    /// The client data or attestation object could not be decoded at all.
    Malformed(&'static str),
    /// `clientData.type` was not the one this ceremony expects.
    WrongType,
    /// `clientData.challenge` did not match the stored one.
    ChallengeMismatch,
    /// `clientData.origin` is not the relying party's origin.
    OriginMismatch,
    /// `rpIdHash` is not SHA-256 of the relying party id.
    RpIdMismatch,
    /// The `UP` (user present) flag was not set.
    UserNotPresent,
    /// The `UV` (user verified) flag was required and absent.
    UserNotVerified,
    /// `AT` (attested credential data) was expected and absent, or the
    /// reverse.
    AttestedCredentialMissing,
    /// The public key algorithm is not one HomeVault can verify.
    UnsupportedAlgorithm,
    /// The COSE key is malformed (curve, coordinates, lengths).
    MalformedKey(&'static str),
    /// The assertion signature did not verify over the signed bytes.
    BadSignature,
    /// The credential id in the response is not the stored one.
    CredentialMismatch,
    /// The signature counter went backwards (a cloned authenticator).
    CounterRegression,
    /// The extension data block is truncated.
    MalformedExtensions,
}

impl PasskeyError {
    /// A user-facing sentence. Kept next to the variants so a new refusal
    /// cannot be added without saying what it means.
    pub fn message(&self) -> String {
        match self {
            PasskeyError::Malformed(what) => format!("The passkey response is malformed ({what})"),
            PasskeyError::WrongType => "The passkey response is for the wrong ceremony".to_string(),
            PasskeyError::ChallengeMismatch => {
                "This sign-in attempt has expired — start again".to_string()
            }
            PasskeyError::OriginMismatch => "The passkey was created for another site".to_string(),
            PasskeyError::RpIdMismatch => {
                "The passkey belongs to another relying party".to_string()
            }
            PasskeyError::UserNotPresent => {
                "The authenticator did not confirm a user was present".to_string()
            }
            PasskeyError::UserNotVerified => {
                "This account requires a passkey with user verification (PIN or biometric)"
                    .to_string()
            }
            PasskeyError::AttestedCredentialMissing => {
                "The authenticator did not send a credential".to_string()
            }
            PasskeyError::UnsupportedAlgorithm => {
                "This passkey uses a signature algorithm HomeVault cannot verify".to_string()
            }
            PasskeyError::MalformedKey(what) => {
                format!("The passkey's public key is invalid ({what})")
            }
            PasskeyError::BadSignature => "The passkey signature did not verify".to_string(),
            PasskeyError::CredentialMismatch => {
                "That passkey is not registered on this account".to_string()
            }
            PasskeyError::CounterRegression => {
                "This passkey's signature counter went backwards — it may be a copy".to_string()
            }
            PasskeyError::MalformedExtensions => {
                "The authenticator's extension data is truncated".to_string()
            }
        }
    }

    /// The HTTP status this refusal should answer. A bad signature or a
    /// counter regression is an authentication failure (`401`); everything else
    /// is a malformed or unacceptable request (`400`).
    pub fn status(&self) -> axum::http::StatusCode {
        use axum::http::StatusCode;
        match self {
            PasskeyError::BadSignature | PasskeyError::CounterRegression => {
                StatusCode::UNAUTHORIZED
            }
            _ => StatusCode::BAD_REQUEST,
        }
    }
}

// --------------------------------------------------------------------------
// Relying party
// --------------------------------------------------------------------------

/// The relying party a ceremony is bound to.
///
/// `id` is the registrable domain (a hostname, no scheme, no port) and `origin`
/// the exact scheme + host + port the browser is on. Both are derived from the
/// configured hostname, never from the request: a caller must not be able to
/// widen the origin by sending a header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelyingParty {
    pub id: String,
    pub origin: String,
}

/// Derive the relying party from a configured hostname and scheme.
///
/// Returns `None` when the hostname is empty or obviously unusable (a bare IP
/// literal is *allowed* by WebAuthn on a secure context, and is exactly what a
/// home box often has, so it is accepted with its own port).
pub fn relying_party_for(hostname: &str, https: bool, https_port: u16) -> Option<RelyingParty> {
    // A pasted URL is accepted for its host only: the origin is rebuilt below,
    // so a caller can never widen it by sending one.
    // Strip the scheme **before** the trailing slash: `https://` trimmed of
    // its slashes first becomes `https:`, which no longer looks like a scheme
    // and would then be accepted as a host.
    let trimmed = hostname.trim();
    let trimmed = trimmed
        .strip_prefix("https://")
        .or_else(|| trimmed.strip_prefix("http://"))
        .unwrap_or(trimmed)
        .trim_end_matches('/');
    if trimmed.is_empty() {
        return None;
    }
    // An explicit `:port` in the configured value wins; otherwise the scheme's
    // default, except HTTPS on a non-443 listener, which the browser includes
    // in the origin.
    let (raw_host, explicit_port) = match trimmed.rsplit_once(':') {
        Some((h, p)) if !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()) => {
            (h, p.parse::<u16>().ok())
        }
        _ => (trimmed, None),
    };
    if raw_host.is_empty() {
        return None;
    }
    // Lowercased: WebAuthn compares origins as strings, so `Vault.Home` and
    // `vault.home` must not be two origins for the same site.
    // `https://` left a trailing slash behind when the scheme was stripped.
    let raw_host = raw_host.trim_end_matches('/');
    if raw_host.is_empty() {
        return None;
    }
    let host = raw_host.to_ascii_lowercase();
    let scheme = if https { "https" } else { "http" };
    let default_port = if https { 443 } else { 80 };
    let origin = match explicit_port {
        Some(port) if port != default_port => format!("{scheme}://{host}:{port}"),
        Some(_) => format!("{scheme}://{host}"),
        // The configured listener port joins the origin whenever it is not the
        // scheme's default — WebAuthn compares the origin the browser actually
        // shows, and HomeVault's HTTPS default (8443) is *not* 443, so leaving
        // it out would refuse every real ceremony. A deployment fronted by a
        // proxy on 443 must set `HTTPS_PORT=443` (or name the port in the
        // configured hostname), and the guide says so.
        None if https => {
            if https_port == 443 || https_port == 0 {
                format!("{scheme}://{host}")
            } else {
                format!("{scheme}://{host}:{https_port}")
            }
        }
        None => {
            if https_port == 80 || https_port == 0 {
                format!("{scheme}://{host}")
            } else {
                format!("{scheme}://{host}:{https_port}")
            }
        }
    };
    Some(RelyingParty { id: host, origin })
}

/// Compare two strings in constant time (challenge equality).
///
/// A length difference is not itself a secret, but the comparison still runs
/// over the shorter one so the timing carries no information about the prefix.
pub fn constant_time_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    let mut diff = a.len() ^ b.len();
    for i in 0..a.len().max(b.len()) {
        let x = *a.get(i).unwrap_or(&0);
        let y = *b.get(i).unwrap_or(&0);
        diff |= (x ^ y) as usize;
    }
    diff == 0
}

// --------------------------------------------------------------------------
// Client data
// --------------------------------------------------------------------------

/// The JSON the browser signs: `clientDataJSON`, base64url-encoded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientData {
    /// `webauthn.create` or `webauthn.get`.
    pub ceremony_type: String,
    pub challenge: String,
    pub origin: String,
}

/// Decode and parse `clientDataJSON` (already base64url-decoded by the caller).
pub fn parse_client_data(json: &[u8]) -> Result<ClientData, PasskeyError> {
    #[derive(Deserialize)]
    struct Raw {
        #[serde(rename = "type")]
        ceremony_type: String,
        challenge: String,
        origin: String,
    }
    let raw: Raw =
        serde_json::from_slice(json).map_err(|_| PasskeyError::Malformed("clientDataJSON"))?;
    Ok(ClientData {
        ceremony_type: raw.ceremony_type,
        challenge: raw.challenge,
        origin: raw.origin,
    })
}

/// Check the parsed client data against the ceremony's expectations.
pub fn verify_client_data(
    data: &ClientData,
    expected_type: &str,
    expected_challenge: &str,
    rp: &RelyingParty,
) -> Result<(), PasskeyError> {
    if data.ceremony_type != expected_type {
        return Err(PasskeyError::WrongType);
    }
    if !constant_time_eq(&data.challenge, expected_challenge) {
        return Err(PasskeyError::ChallengeMismatch);
    }
    // Exact equality, not a suffix/prefix match: `https://evil.example` must
    // never be accepted for `https://example`.
    if data.origin.trim_end_matches('/') != rp.origin {
        return Err(PasskeyError::OriginMismatch);
    }
    Ok(())
}

// --------------------------------------------------------------------------
// Authenticator data
// --------------------------------------------------------------------------

/// The parsed `authenticatorData`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthenticatorData {
    pub rp_id_hash: [u8; 32],
    pub flags: u8,
    pub sign_count: u32,
    /// Present when `AT` is set (registration).
    pub credential: Option<AttestedCredential>,
    /// Where the extension-data CBOR starts, when `ED` is set.
    pub extensions: Option<Vec<u8>>,
}

/// The credential carried by an attestation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttestedCredential {
    pub credential_id: Vec<u8>,
    pub algorithm: KeyAlgorithm,
    /// The raw public key, already unwrapped from the COSE structure: 65
    /// bytes (`0x04 || X || Y`) for ES256, 32 bytes for EdDSA.
    pub public_key: Vec<u8>,
}

impl AuthenticatorData {
    pub fn user_present(&self) -> bool {
        self.flags & FLAG_UP != 0
    }

    pub fn user_verified(&self) -> bool {
        self.flags & FLAG_UV != 0
    }
}

/// Parse `authenticatorData` (the raw block, without the attestation object
/// wrapper). Pure.
pub fn parse_authenticator_data(data: &[u8]) -> Result<AuthenticatorData, PasskeyError> {
    if data.len() < 37 {
        return Err(PasskeyError::Malformed("authenticatorData too short"));
    }
    let mut rp_id_hash = [0u8; 32];
    rp_id_hash.copy_from_slice(&data[..32]);
    let flags = data[32];
    let sign_count = u32::from_be_bytes([data[33], data[34], data[35], data[36]]);
    let mut rest = &data[37..];

    let mut credential = None;
    if flags & FLAG_AT != 0 {
        let (cred, consumed) = parse_attested_credential(rest)?;
        credential = Some(cred);
        rest = &rest[consumed..];
    }

    let mut extensions = None;
    if flags & FLAG_ED != 0 {
        if rest.is_empty() {
            return Err(PasskeyError::MalformedExtensions);
        }
        // The extensions are CBOR; the caller only records their presence, so
        // the block is kept verbatim rather than decoded.
        extensions = Some(rest.to_vec());
        rest = &[];
    }

    if !rest.is_empty() {
        return Err(PasskeyError::Malformed(
            "trailing bytes in authenticatorData",
        ));
    }

    Ok(AuthenticatorData {
        rp_id_hash,
        flags,
        sign_count,
        credential,
        extensions,
    })
}

/// Parse `aaguid(16) || credIdLen(2, BE) || credId || COSE public key`, and
/// answer how many bytes were consumed.
fn parse_attested_credential(data: &[u8]) -> Result<(AttestedCredential, usize), PasskeyError> {
    if data.len() < 18 {
        return Err(PasskeyError::Malformed("attested credential too short"));
    }
    // The AAGUID is evidence, not a decision: recorded by the caller, never
    // used to accept or refuse a credential.
    let id_len = u16::from_be_bytes([data[16], data[17]]) as usize;
    if id_len == 0 || id_len > MAX_CREDENTIAL_ID_LEN {
        return Err(PasskeyError::Malformed("credential id length"));
    }
    let id_start = 18;
    let id_end = id_start + id_len;
    if data.len() <= id_end {
        return Err(PasskeyError::Malformed("credential id truncated"));
    }
    let credential_id = data[id_start..id_end].to_vec();

    let (algorithm, public_key, cose_len) = parse_cose_key(&data[id_end..])?;
    Ok((
        AttestedCredential {
            credential_id,
            algorithm,
            public_key,
        },
        id_end + cose_len,
    ))
}

// --------------------------------------------------------------------------
// A minimal CBOR reader — only what a COSE key needs
// --------------------------------------------------------------------------

/// One CBOR item: the head plus, for maps, the key/value pairs in order.
#[derive(Debug, Clone, PartialEq)]
enum Cbor {
    Unsigned(u64),
    Negative(i64),
    Bytes(Vec<u8>),
    Text(String),
    Map(Vec<(Cbor, Cbor)>),
    Array(Vec<Cbor>),
}

/// Read one CBOR item, answering it and how many bytes it took.
///
/// Deliberately tiny: a COSE key is a small map of integers, byte strings and
/// text strings. Anything else (a tag, a float, an indefinite-length item) is
/// `Other` and is refused by the caller — which is the safe default, since it
/// means "this is not a key I understand".
fn read_cbor(data: &[u8]) -> Result<(Cbor, usize), PasskeyError> {
    let mut pos = 0usize;

    fn read_len(
        data: &[u8],
        pos: &mut usize,
        additional: u8,
        base: usize,
    ) -> Result<usize, PasskeyError> {
        let n = match additional {
            0..=23 => additional as u64,
            24 => {
                let v = *data
                    .get(*pos)
                    .ok_or(PasskeyError::Malformed("cbor length"))?;
                *pos += 1;
                v as u64
            }
            25 => {
                let b = data
                    .get(*pos..*pos + 2)
                    .ok_or(PasskeyError::Malformed("cbor length"))?;
                *pos += 2;
                u16::from_be_bytes([b[0], b[1]]) as u64
            }
            26 => {
                let b = data
                    .get(*pos..*pos + 4)
                    .ok_or(PasskeyError::Malformed("cbor length"))?;
                *pos += 4;
                u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as u64
            }
            27 => {
                let b = data
                    .get(*pos..*pos + 8)
                    .ok_or(PasskeyError::Malformed("cbor length"))?;
                *pos += 8;
                u64::from_be_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]])
            }
            // 28..=30 are reserved, 31 is indefinite length: both refused.
            _ => return Err(PasskeyError::Malformed("cbor length encoding")),
        };
        let total = usize::try_from(n).map_err(|_| PasskeyError::Malformed("cbor length"))?;
        checked_len(total, base)
    }

    fn checked_len(n: usize, _base: usize) -> Result<usize, PasskeyError> {
        if n > 1_048_576 {
            return Err(PasskeyError::Malformed("cbor item too large"));
        }
        Ok(n)
    }

    fn read_item(data: &[u8], pos: &mut usize) -> Result<Cbor, PasskeyError> {
        let head = *data
            .get(*pos)
            .ok_or(PasskeyError::Malformed("cbor truncated"))?;
        *pos += 1;
        let major = head >> 5;
        let additional = head & 0x1f;
        match major {
            0 => Ok(Cbor::Unsigned(read_len(data, pos, additional, 0)? as u64)),
            1 => {
                let n = read_len(data, pos, additional, 0)? as i64;
                Ok(Cbor::Negative(-1 - n))
            }
            2 => {
                let len = read_len(data, pos, additional, 0)?;
                let bytes = data
                    .get(*pos..*pos + len)
                    .ok_or(PasskeyError::Malformed("cbor byte string"))?;
                *pos += len;
                Ok(Cbor::Bytes(bytes.to_vec()))
            }
            3 => {
                let len = read_len(data, pos, additional, 0)?;
                let bytes = data
                    .get(*pos..*pos + len)
                    .ok_or(PasskeyError::Malformed("cbor text string"))?;
                *pos += len;
                Ok(Cbor::Text(String::from_utf8(bytes.to_vec()).map_err(
                    |_| PasskeyError::Malformed("cbor text string"),
                )?))
            }
            4 => {
                let len = read_len(data, pos, additional, 0)?;
                let mut items = Vec::with_capacity(len.min(16));
                for _ in 0..len {
                    items.push(read_item(data, pos)?);
                }
                Ok(Cbor::Array(items))
            }
            5 => {
                let len = read_len(data, pos, additional, 0)?;
                let mut pairs = Vec::with_capacity(len.min(16));
                for _ in 0..len {
                    let key = read_item(data, pos)?;
                    let value = read_item(data, pos)?;
                    pairs.push((key, value));
                }
                Ok(Cbor::Map(pairs))
            }
            _ => Err(PasskeyError::Malformed("cbor major type")),
        }
    }

    let item = read_item(data, &mut pos)?;
    Ok((item, pos))
}

/// Read one CBOR item and require that it consumed `data` exactly.
fn read_cbor_exact(data: &[u8]) -> Result<Cbor, PasskeyError> {
    let (item, used) = read_cbor(data)?;
    if used != data.len() {
        return Err(PasskeyError::Malformed(
            "trailing bytes after the CBOR item",
        ));
    }
    Ok(item)
}

// --------------------------------------------------------------------------
// COSE public keys
// --------------------------------------------------------------------------

/// COSE key map labels (RFC 8152 §7.1).
const COSE_KTY: i64 = 1;
const COSE_ALG: i64 = 3;
const COSE_CRV: i64 = -1;
const COSE_X: i64 = -2;
const COSE_Y: i64 = -3;

const KTY_OKP: u64 = 1;
const KTY_EC2: u64 = 2;
const CRV_P256: u64 = 1;
const CRV_ED25519: u64 = 6;

/// Parse a COSE key, answering the algorithm, the unwrapped public key and how
/// many bytes it took.
///
/// Only ES256 (EC2 / P-256) and EdDSA (OKP / Ed25519) are accepted: those are
/// what platform and hardware authenticators ship, and they are what this
/// module can verify. Anything else is an explicit refusal, never a guess.
fn parse_cose_key(data: &[u8]) -> Result<(KeyAlgorithm, Vec<u8>, usize), PasskeyError> {
    let (item, used) = read_cbor(data)?;
    let Cbor::Map(pairs) = item else {
        return Err(PasskeyError::MalformedKey("COSE key is not a map"));
    };

    let mut kty: Option<u64> = None;
    let mut alg: Option<i64> = None;
    let mut crv: Option<u64> = None;
    let mut x: Option<Vec<u8>> = None;
    let mut y: Option<Vec<u8>> = None;

    // One pass: the map mixes positive labels (`1` = kty, `3` = alg) and
    // negative ones (`-1` = crv, `-2` = x, `-3` = y), so both signs are read
    // here rather than in two loops that could drift apart.
    for (key, value) in pairs {
        match (key, value) {
            (Cbor::Unsigned(label), Cbor::Unsigned(v)) if label == COSE_KTY as u64 => kty = Some(v),
            (Cbor::Unsigned(label), Cbor::Negative(v)) if label == COSE_ALG as u64 => alg = Some(v),
            (Cbor::Negative(COSE_CRV), Cbor::Unsigned(v)) => crv = Some(v),
            (Cbor::Negative(COSE_X), Cbor::Bytes(v)) => x = Some(v),
            (Cbor::Negative(COSE_Y), Cbor::Bytes(v)) => y = Some(v),
            // Any other label is not a field this module reads: ignored, so a
            // future extension in the key does not turn into a refusal.
            _ => {}
        }
    }

    let algorithm = alg
        .and_then(KeyAlgorithm::from_cose_alg)
        .ok_or(PasskeyError::UnsupportedAlgorithm)?;

    match (kty, crv) {
        (Some(KTY_EC2), Some(CRV_P256)) if algorithm == KeyAlgorithm::Es256 => {
            let x = x.ok_or(PasskeyError::MalformedKey("missing X"))?;
            let y = y.ok_or(PasskeyError::MalformedKey("missing Y"))?;
            if x.len() != 32 || y.len() != 32 {
                return Err(PasskeyError::MalformedKey(
                    "P-256 coordinates are not 32 bytes",
                ));
            }
            let mut key = Vec::with_capacity(65);
            key.push(0x04); // uncompressed point
            key.extend_from_slice(&x);
            key.extend_from_slice(&y);
            Ok((algorithm, key, used))
        }
        (Some(KTY_OKP), Some(CRV_ED25519)) if algorithm == KeyAlgorithm::EdDsa => {
            let x = x.ok_or(PasskeyError::MalformedKey("missing X"))?;
            if x.len() != 32 {
                return Err(PasskeyError::MalformedKey("Ed25519 key is not 32 bytes"));
            }
            Ok((algorithm, x, used))
        }
        _ => Err(PasskeyError::MalformedKey("unsupported curve or key type")),
    }
}

// --------------------------------------------------------------------------
// Registration (webauthn.create)
// --------------------------------------------------------------------------

/// The attestation object's parsed form. HomeVault does not verify attestation
/// chains — it records the format and the AAGUID as evidence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attestation {
    pub fmt: String,
    pub att_stmt: Vec<u8>,
    pub auth_data: Vec<u8>,
}

/// Parse a CBOR attestation object: `{fmt, attStmt, authData}`.
pub fn parse_attestation_object(data: &[u8]) -> Result<Attestation, PasskeyError> {
    let Cbor::Map(pairs) = read_cbor_exact(data)? else {
        return Err(PasskeyError::Malformed("attestation object is not a map"));
    };
    let mut fmt = None;
    let mut att_stmt = None;
    let mut auth_data = None;
    for (key, value) in pairs {
        match (key, value) {
            (Cbor::Text(k), Cbor::Text(v)) if k == "fmt" => fmt = Some(v),
            (Cbor::Text(k), Cbor::Map(_)) if k == "attStmt" => {
                // The statement is kept opaque: no chain is verified, and
                // storing what the authenticator said is what makes a future
                // attestation policy possible without a migration.
                att_stmt = Some(Vec::new());
            }
            (Cbor::Text(k), Cbor::Bytes(v)) if k == "authData" => auth_data = Some(v),
            _ => {}
        }
    }
    Ok(Attestation {
        fmt: fmt.ok_or(PasskeyError::Malformed("attestation fmt"))?,
        att_stmt: att_stmt.ok_or(PasskeyError::Malformed("attestation attStmt"))?,
        auth_data: auth_data.ok_or(PasskeyError::Malformed("attestation authData"))?,
    })
}

/// A verified registration: everything the caller needs to store, and nothing
/// it has to re-derive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegisteredCredential {
    pub credential_id: Vec<u8>,
    pub algorithm: KeyAlgorithm,
    pub public_key: Vec<u8>,
    pub sign_count: u32,
    pub fmt: String,
    pub user_verified: bool,
}

/// Verify a registration response.
///
/// Inputs are the **base64url** strings the browser sent, already decoded by
/// the caller into bytes (`client_data_json`, `attestation_object`).
pub fn verify_registration(
    client_data_json: &[u8],
    attestation_object: &[u8],
    expected_challenge: &str,
    rp: &RelyingParty,
    require_user_verification: bool,
) -> Result<RegisteredCredential, PasskeyError> {
    let client = parse_client_data(client_data_json)?;
    verify_client_data(&client, "webauthn.create", expected_challenge, rp)?;

    let attestation = parse_attestation_object(attestation_object)?;
    let auth = parse_authenticator_data(&attestation.auth_data)?;

    let mut hasher = Sha256::new();
    hasher.update(rp.id.as_bytes());
    let expected_rp = hasher.finalize();
    if auth.rp_id_hash != expected_rp.as_slice() {
        return Err(PasskeyError::RpIdMismatch);
    }
    if !auth.user_present() {
        return Err(PasskeyError::UserNotPresent);
    }
    if require_user_verification && !auth.user_verified() {
        return Err(PasskeyError::UserNotVerified);
    }
    let credential = auth
        .credential
        .clone()
        .ok_or(PasskeyError::AttestedCredentialMissing)?;
    if credential.credential_id.is_empty() || credential.credential_id.len() > MAX_CREDENTIAL_ID_LEN
    {
        return Err(PasskeyError::Malformed("credential id"));
    }

    Ok(RegisteredCredential {
        credential_id: credential.credential_id,
        algorithm: credential.algorithm,
        public_key: credential.public_key,
        sign_count: auth.sign_count,
        fmt: attestation.fmt,
        user_verified: auth.user_verified(),
    })
}

// --------------------------------------------------------------------------
// Authentication (webauthn.get)
// --------------------------------------------------------------------------

/// What was verified on an assertion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedAssertion {
    pub sign_count: u32,
    pub user_verified: bool,
}

/// Verify an assertion.
///
/// `authenticator_data` and `signature` are the decoded bytes the browser sent;
/// `public_key` is what was stored at registration, `algorithm` its type, and
/// `stored_sign_count` the last counter seen (0 when the authenticator does not
/// implement one).
///
/// The counter rule: an authenticator that reports `0` never increments and is
/// exempt (per the spec); otherwise the new value must be **greater** than the
/// stored one, because a cloned authenticator reports the same or a lower
/// count. Refusing a regression is the only signal the protocol gives.
///
/// Nine arguments on purpose: each one is a distinct piece of the ceremony the
/// caller must supply (four from the response, four from the stored credential,
/// one policy flag), and bundling them into a struct would only move the same
/// list one type away.
#[allow(clippy::too_many_arguments)]
pub fn verify_assertion(
    client_data_json: &[u8],
    authenticator_data: &[u8],
    signature: &[u8],
    expected_challenge: &str,
    rp: &RelyingParty,
    algorithm: KeyAlgorithm,
    public_key: &[u8],
    stored_sign_count: u32,
    require_user_verification: bool,
) -> Result<VerifiedAssertion, PasskeyError> {
    let client = parse_client_data(client_data_json)?;
    verify_client_data(&client, "webauthn.get", expected_challenge, rp)?;

    let auth = parse_authenticator_data(authenticator_data)?;
    let mut hasher = Sha256::new();
    hasher.update(rp.id.as_bytes());
    if auth.rp_id_hash != hasher.finalize().as_slice() {
        return Err(PasskeyError::RpIdMismatch);
    }
    if !auth.user_present() {
        return Err(PasskeyError::UserNotPresent);
    }
    if require_user_verification && !auth.user_verified() {
        return Err(PasskeyError::UserNotVerified);
    }

    // Signed bytes: authenticatorData || SHA-256(clientDataJSON).
    let mut signed = authenticator_data.to_vec();
    let mut cd_hash = Sha256::new();
    cd_hash.update(client_data_json);
    signed.extend_from_slice(&cd_hash.finalize());

    verify_signature(algorithm, public_key, &signed, signature)?;

    if stored_sign_count != 0 && auth.sign_count != 0 && auth.sign_count <= stored_sign_count {
        return Err(PasskeyError::CounterRegression);
    }

    Ok(VerifiedAssertion {
        sign_count: auth.sign_count,
        user_verified: auth.user_verified(),
    })
}

/// Verify a DER (ES256) or raw (Ed25519) signature over `message`.
pub fn verify_signature(
    algorithm: KeyAlgorithm,
    public_key: &[u8],
    message: &[u8],
    signature: &[u8],
) -> Result<(), PasskeyError> {
    let verified = match algorithm {
        KeyAlgorithm::Es256 => ring::signature::UnparsedPublicKey::new(
            &ring::signature::ECDSA_P256_SHA256_ASN1,
            public_key,
        )
        .verify(message, signature),
        KeyAlgorithm::EdDsa => {
            ring::signature::UnparsedPublicKey::new(&ring::signature::ED25519, public_key)
                .verify(message, signature)
        }
    };
    verified.map_err(|_| PasskeyError::BadSignature)
}

// --------------------------------------------------------------------------
// Challenges
// --------------------------------------------------------------------------

/// The two ceremonies a challenge is issued for. Stored with the challenge so
/// a registration challenge can never authorise a sign-in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Ceremony {
    Registration,
    Authentication,
}

impl Ceremony {
    pub fn as_str(self) -> &'static str {
        match self {
            Ceremony::Registration => "registration",
            Ceremony::Authentication => "authentication",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "registration" => Some(Ceremony::Registration),
            "authentication" => Some(Ceremony::Authentication),
            _ => None,
        }
    }
}

/// A fresh challenge: 32 random bytes, base64url without padding (what the
/// browser echoes back byte for byte).
pub fn new_challenge() -> String {
    use rand::RngExt;
    let bytes: [u8; CHALLENGE_LEN] = rand::rng().random();
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// Decode a base64url value (challenge, credential id, signature).
pub fn decode_b64url(value: &str) -> Result<Vec<u8>, PasskeyError> {
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(value.trim_end_matches('='))
        .map_err(|_| PasskeyError::Malformed("base64url"))
}

/// Encode bytes as base64url without padding.
pub fn encode_b64url(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rp() -> RelyingParty {
        RelyingParty {
            id: "vault.home".to_string(),
            origin: "https://vault.home".to_string(),
        }
    }

    fn rp_id_hash() -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(b"vault.home");
        hasher.finalize().into()
    }

    /// Build `authenticatorData` for a registration, with a P-256 COSE key.
    fn auth_data_with_credential(flags: u8, sign_count: u32) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&rp_id_hash());
        out.push(flags | FLAG_AT);
        out.extend_from_slice(&sign_count.to_be_bytes());
        out.extend_from_slice(&[0u8; 16]); // aaguid
        let cred_id = b"credential-1";
        out.extend_from_slice(&(cred_id.len() as u16).to_be_bytes());
        out.extend_from_slice(cred_id);
        // COSE key: {1:2, 3:-7, -1:1, -2:X(32), -3:Y(32)}
        out.push(0xa5); // map(5)
        out.extend_from_slice(&[0x01, 0x02]); // 1: 2 (kty EC2)
        out.extend_from_slice(&[0x03, 0x26]); // 3: -7 (ES256)
        out.extend_from_slice(&[0x20, 0x01]); // -1: 1 (P-256)
        out.push(0x21); // -2 (x)
        out.push(0x58); // bytes(1)
        out.push(32);
        out.extend_from_slice(&[0x11; 32]);
        out.push(0x22); // -3 (y)
        out.push(0x58);
        out.push(32);
        out.extend_from_slice(&[0x22; 32]);
        out
    }

    // -----------------------------------------------------------------
    // Relying party
    // -----------------------------------------------------------------

    #[test]
    fn test_relying_party_for_builds_the_default_https_origin() {
        let rp = relying_party_for("vault.home", true, 443).expect("rp");
        assert_eq!(rp.id, "vault.home");
        assert_eq!(rp.origin, "https://vault.home", "443 is implicit");
    }

    #[test]
    fn test_relying_party_for_keeps_an_explicit_port() {
        let rp = relying_party_for("vault.home:8443", true, 8443).expect("rp");
        assert_eq!(rp.id, "vault.home");
        assert_eq!(rp.origin, "https://vault.home:8443");
    }

    #[test]
    fn test_relying_party_for_accepts_http_on_a_lan() {
        let rp = relying_party_for("192.168.1.10:8080", false, 8443).expect("rp");
        assert_eq!(rp.id, "192.168.1.10");
        assert_eq!(rp.origin, "http://192.168.1.10:8080");
    }

    #[test]
    fn test_relying_party_for_uses_the_host_of_a_pasted_url() {
        let rp = relying_party_for("https://Vault.Home/", true, 443).expect("rp");
        assert_eq!(
            rp.id, "vault.home",
            "the id is lowercased, no scheme, no path"
        );
        assert_eq!(rp.origin, "https://vault.home");
    }

    #[test]
    fn test_relying_party_for_keeps_the_default_https_listener_port() {
        // HomeVault's HTTPS default is 8443, not 443: the origin the browser
        // shows includes it, so it must be part of the relying party's origin
        // or every real ceremony would be refused as a foreign site.
        let rp = relying_party_for("vault.home", true, 8443).expect("rp");
        assert_eq!(rp.origin, "https://vault.home:8443");
        assert_eq!(rp.id, "vault.home", "the rp id never carries a port");
    }

    #[test]
    fn test_relying_party_for_refuses_an_empty_host() {
        assert!(relying_party_for("", true, 8443).is_none());
        assert!(relying_party_for("   ", true, 8443).is_none());
        assert!(relying_party_for("https://", true, 8443).is_none());
    }

    #[test]
    fn test_constant_time_eq_is_an_equality() {
        assert!(constant_time_eq("abc", "abc"));
        assert!(!constant_time_eq("abc", "abd"));
        assert!(!constant_time_eq("abc", "ab"));
        assert!(!constant_time_eq("", "a"));
        assert!(constant_time_eq("", ""));
    }

    // -----------------------------------------------------------------
    // Client data
    // -----------------------------------------------------------------

    fn client_data(ceremony: &str, challenge: &str, origin: &str) -> Vec<u8> {
        format!(
            r#"{{"type":"{ceremony}","challenge":"{challenge}","origin":"{origin}","crossOrigin":false}}"#
        )
        .into_bytes()
    }

    #[test]
    fn test_verify_client_data_accepts_the_expected_ceremony() {
        let data = parse_client_data(&client_data(
            "webauthn.create",
            "chal",
            "https://vault.home",
        ))
        .expect("parse");
        assert!(verify_client_data(&data, "webauthn.create", "chal", &rp()).is_ok());
    }

    #[test]
    fn test_verify_client_data_refuses_the_wrong_ceremony() {
        // A registration response must never satisfy a sign-in.
        let data = parse_client_data(&client_data(
            "webauthn.create",
            "chal",
            "https://vault.home",
        ))
        .expect("parse");
        assert_eq!(
            verify_client_data(&data, "webauthn.get", "chal", &rp()),
            Err(PasskeyError::WrongType)
        );
    }

    #[test]
    fn test_verify_client_data_refuses_a_mismatched_challenge() {
        let data = parse_client_data(&client_data("webauthn.get", "chal", "https://vault.home"))
            .expect("parse");
        assert_eq!(
            verify_client_data(&data, "webauthn.get", "other", &rp()),
            Err(PasskeyError::ChallengeMismatch)
        );
    }

    #[test]
    fn test_verify_client_data_refuses_a_lookalike_origin() {
        // The check is exact equality, never a prefix/suffix match: these are
        // all origins an attacker could serve from.
        for origin in [
            "https://evil.example",
            "https://vault.home.evil.example",
            "https://evil-vault.home",
            "http://vault.home",
            "https://vault.home:8443",
        ] {
            let data =
                parse_client_data(&client_data("webauthn.get", "chal", origin)).expect("parse");
            assert_eq!(
                verify_client_data(&data, "webauthn.get", "chal", &rp()),
                Err(PasskeyError::OriginMismatch),
                "{origin} must not be accepted for https://vault.home"
            );
        }
    }

    #[test]
    fn test_verify_client_data_tolerates_a_trailing_slash_in_the_origin() {
        let data = parse_client_data(&client_data("webauthn.get", "chal", "https://vault.home/"))
            .expect("parse");
        assert!(verify_client_data(&data, "webauthn.get", "chal", &rp()).is_ok());
    }

    #[test]
    fn test_parse_client_data_refuses_garbage() {
        assert!(matches!(
            parse_client_data(b"not json"),
            Err(PasskeyError::Malformed(_))
        ));
        assert!(matches!(
            parse_client_data(b"{\"type\":\"webauthn.get\"}"),
            Err(PasskeyError::Malformed(_))
        ));
    }

    // -----------------------------------------------------------------
    // Authenticator data
    // -----------------------------------------------------------------

    #[test]
    fn test_parse_authenticator_data_reads_the_flags_and_counter() {
        let mut data = Vec::new();
        data.extend_from_slice(&rp_id_hash());
        data.push(FLAG_UP | FLAG_UV);
        data.extend_from_slice(&7u32.to_be_bytes());
        let auth = parse_authenticator_data(&data).expect("parse");
        assert_eq!(auth.flags, FLAG_UP | FLAG_UV);
        assert!(auth.user_present());
        assert!(auth.user_verified());
        assert_eq!(auth.sign_count, 7);
        assert!(auth.credential.is_none());
    }

    #[test]
    fn test_parse_authenticator_data_refuses_a_short_block() {
        assert!(matches!(
            parse_authenticator_data(&[0u8; 36]),
            Err(PasskeyError::Malformed(_))
        ));
    }

    #[test]
    fn test_parse_authenticator_data_reads_a_p256_credential() {
        let auth = parse_authenticator_data(&auth_data_with_credential(FLAG_UP, 0)).expect("parse");
        let credential = auth.credential.expect("credential");
        assert_eq!(credential.credential_id, b"credential-1");
        assert_eq!(credential.algorithm, KeyAlgorithm::Es256);
        assert_eq!(credential.public_key.len(), 65);
        assert_eq!(credential.public_key[0], 0x04, "uncompressed point prefix");
    }

    #[test]
    fn test_parse_authenticator_data_refuses_a_zero_length_credential_id() {
        let mut data = Vec::new();
        data.extend_from_slice(&rp_id_hash());
        data.push(FLAG_UP | FLAG_AT);
        data.extend_from_slice(&0u32.to_be_bytes());
        data.extend_from_slice(&[0u8; 16]);
        data.extend_from_slice(&0u16.to_be_bytes());
        assert!(matches!(
            parse_authenticator_data(&data),
            Err(PasskeyError::Malformed(_))
        ));
    }

    // -----------------------------------------------------------------
    // COSE keys
    // -----------------------------------------------------------------

    #[test]
    fn test_cose_key_refuses_an_unsupported_algorithm() {
        // kty EC2, alg -257 (RS256), P-256 coordinates.
        let mut key = vec![0xa3, 0x01, 0x02, 0x03, 0x39, 0x01, 0x00];
        key.extend_from_slice(&[0x20, 0x01]);
        assert_eq!(
            parse_cose_key(&key).map(|(a, _, _)| a),
            Err(PasskeyError::UnsupportedAlgorithm),
            "RS256 is not a key HomeVault can verify"
        );
    }

    #[test]
    fn test_cose_key_refuses_the_wrong_curve() {
        // kty EC2, alg ES256, crv P-384 (2).
        let mut key = vec![0xa3, 0x01, 0x02, 0x03, 0x26, 0x20, 0x02];
        key.extend_from_slice(&[0x21, 0x58, 0x30]);
        key.extend_from_slice(&[0x11; 48]);
        key.extend_from_slice(&[0x22, 0x58, 0x30]);
        key.extend_from_slice(&[0x22; 48]);
        assert!(matches!(
            parse_cose_key(&key),
            Err(PasskeyError::MalformedKey(_))
        ));
    }

    #[test]
    fn test_cose_key_refuses_a_short_p256_coordinate() {
        let mut key = vec![0xa5, 0x01, 0x02, 0x03, 0x26, 0x20, 0x01];
        key.extend_from_slice(&[0x21, 0x58, 0x10]);
        key.extend_from_slice(&[0x11; 16]);
        key.extend_from_slice(&[0x22, 0x58, 0x20]);
        key.extend_from_slice(&[0x22; 32]);
        assert!(matches!(
            parse_cose_key(&key),
            Err(PasskeyError::MalformedKey(_))
        ));
    }

    #[test]
    fn test_cose_key_refuses_a_trailing_byte_after_the_map() {
        let mut key = vec![0xa5, 0x01, 0x02, 0x03, 0x26, 0x20, 0x01];
        key.extend_from_slice(&[0x21, 0x58, 0x20]);
        key.extend_from_slice(&[0x11; 32]);
        key.extend_from_slice(&[0x22, 0x58, 0x20]);
        key.extend_from_slice(&[0x22; 32]);
        // The attested-credential length check is what must reject it: the
        // map is followed by a byte that is not part of any field.
        let mut data = Vec::new();
        data.extend_from_slice(&rp_id_hash());
        data.push(FLAG_UP | FLAG_AT);
        data.extend_from_slice(&0u32.to_be_bytes());
        data.extend_from_slice(&[0u8; 16]);
        data.extend_from_slice(&1u16.to_be_bytes());
        data.push(b'c');
        data.extend_from_slice(&key);
        data.push(0xff);
        assert!(parse_authenticator_data(&data).is_err());
    }

    // -----------------------------------------------------------------
    // Attestation object and registration
    // -----------------------------------------------------------------

    /// CBOR: `{fmt:"none", attStmt:{}, authData:<bytes>}`.
    fn attestation_object(auth_data: &[u8]) -> Vec<u8> {
        let mut out = vec![0xa3];
        out.extend_from_slice(&[0x63]);
        out.extend_from_slice(b"fmt");
        out.extend_from_slice(&[0x64]);
        out.extend_from_slice(b"none");
        out.extend_from_slice(&[0x67]);
        out.extend_from_slice(b"attStmt");
        out.push(0xa0); // empty map
        out.extend_from_slice(&[0x68]);
        out.extend_from_slice(b"authData");
        // bytes(authData.len())
        let len = auth_data.len();
        if len < 24 {
            out.push(0x40 | len as u8);
        } else if len <= 0xff {
            out.extend_from_slice(&[0x58, len as u8]);
        } else {
            out.extend_from_slice(&[0x59, (len >> 8) as u8, (len & 0xff) as u8]);
        }
        out.extend_from_slice(auth_data);
        out
    }

    #[test]
    fn test_parse_attestation_object_reads_none_attestation() {
        let auth = auth_data_with_credential(FLAG_UP | FLAG_UV, 0);
        let parsed = parse_attestation_object(&attestation_object(&auth)).expect("parse");
        assert_eq!(parsed.fmt, "none");
        assert_eq!(parsed.auth_data, auth);
    }

    #[test]
    fn test_parse_attestation_object_refuses_a_missing_auth_data() {
        // `{fmt:"none", attStmt:{}}`
        let mut out = vec![0xa2, 0x63];
        out.extend_from_slice(b"fmt");
        out.extend_from_slice(&[0x64]);
        out.extend_from_slice(b"none");
        out.extend_from_slice(&[0x67]);
        out.extend_from_slice(b"attStmt");
        out.push(0xa0);
        assert!(matches!(
            parse_attestation_object(&out),
            Err(PasskeyError::Malformed(_))
        ));
    }

    #[test]
    fn test_verify_registration_accepts_a_well_formed_response() {
        let challenge = new_challenge();
        let client = client_data("webauthn.create", &challenge, "https://vault.home");
        let attestation = attestation_object(&auth_data_with_credential(FLAG_UP | FLAG_UV, 0));
        let registered =
            verify_registration(&client, &attestation, &challenge, &rp(), true).expect("register");
        assert_eq!(registered.credential_id, b"credential-1");
        assert_eq!(registered.algorithm, KeyAlgorithm::Es256);
        assert_eq!(registered.fmt, "none");
        assert!(registered.user_verified);
    }

    #[test]
    fn test_verify_registration_refuses_a_foreign_rp_id() {
        let challenge = new_challenge();
        let client = client_data("webauthn.create", &challenge, "https://vault.home");
        // Authenticator data bound to another relying party.
        let mut auth = auth_data_with_credential(FLAG_UP, 0);
        auth[..32].copy_from_slice(&[0xAAu8; 32]);
        let attestation = attestation_object(&auth);
        assert_eq!(
            verify_registration(&client, &attestation, &challenge, &rp(), false),
            Err(PasskeyError::RpIdMismatch)
        );
    }

    #[test]
    fn test_verify_registration_refuses_a_missing_user_presence() {
        let challenge = new_challenge();
        let client = client_data("webauthn.create", &challenge, "https://vault.home");
        let attestation = attestation_object(&auth_data_with_credential(0, 0));
        assert_eq!(
            verify_registration(&client, &attestation, &challenge, &rp(), false),
            Err(PasskeyError::UserNotPresent)
        );
    }

    #[test]
    fn test_verify_registration_refuses_a_missing_user_verification_when_required() {
        let challenge = new_challenge();
        let client = client_data("webauthn.create", &challenge, "https://vault.home");
        let attestation = attestation_object(&auth_data_with_credential(FLAG_UP, 0));
        assert_eq!(
            verify_registration(&client, &attestation, &challenge, &rp(), true),
            Err(PasskeyError::UserNotVerified)
        );
        // …and the same response is accepted when UV is not required.
        assert!(verify_registration(&client, &attestation, &challenge, &rp(), false).is_ok());
    }

    #[test]
    fn test_verify_registration_refuses_a_replayed_challenge() {
        let used = new_challenge();
        let current = new_challenge();
        let client = client_data("webauthn.create", &used, "https://vault.home");
        let attestation = attestation_object(&auth_data_with_credential(FLAG_UP, 0));
        assert_eq!(
            verify_registration(&client, &attestation, &current, &rp(), false),
            Err(PasskeyError::ChallengeMismatch)
        );
    }

    // -----------------------------------------------------------------
    // Assertion signatures (real crypto: ring verifies the real key)
    // -----------------------------------------------------------------

    /// A real P-256 key pair, its COSE/WebAuthn public key and a signer.
    fn es256_keypair() -> (Vec<u8>, ring::signature::EcdsaKeyPair) {
        use ring::signature::{EcdsaKeyPair, KeyPair};
        let rng = ring::rand::SystemRandom::new();
        let pkcs8 =
            EcdsaKeyPair::generate_pkcs8(&ring::signature::ECDSA_P256_SHA256_ASN1_SIGNING, &rng)
                .expect("generate");
        let pair = EcdsaKeyPair::from_pkcs8(
            &ring::signature::ECDSA_P256_SHA256_ASN1_SIGNING,
            pkcs8.as_ref(),
            &rng,
        )
        .expect("load");
        // Uncompressed point: 0x04 || X || Y.
        let public = pair.public_key().as_ref().to_vec();
        (public, pair)
    }

    fn assertion_signed_bytes(authenticator_data: &[u8], client_data_json: &[u8]) -> Vec<u8> {
        let mut signed = authenticator_data.to_vec();
        let mut hasher = Sha256::new();
        hasher.update(client_data_json);
        signed.extend_from_slice(&hasher.finalize());
        signed
    }

    fn plain_auth_data(flags: u8, sign_count: u32) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&rp_id_hash());
        out.push(flags);
        out.extend_from_slice(&sign_count.to_be_bytes());
        out
    }

    #[test]
    fn test_verify_assertion_accepts_a_real_signature() {
        let (public, pair) = es256_keypair();
        let challenge = new_challenge();
        let client = client_data("webauthn.get", &challenge, "https://vault.home");
        let auth = plain_auth_data(FLAG_UP | FLAG_UV, 3);
        let signed = assertion_signed_bytes(&auth, &client);
        let rng = ring::rand::SystemRandom::new();
        let signature = pair.sign(&rng, &signed).expect("sign").as_ref().to_vec();

        let verified = verify_assertion(
            &client,
            &auth,
            &signature,
            &challenge,
            &rp(),
            KeyAlgorithm::Es256,
            &public,
            1,
            true,
        )
        .expect("verify");
        assert_eq!(verified.sign_count, 3);
        assert!(verified.user_verified);
    }

    #[test]
    fn test_verify_assertion_refuses_tampered_signed_bytes() {
        let (public, pair) = es256_keypair();
        let challenge = new_challenge();
        let client = client_data("webauthn.get", &challenge, "https://vault.home");
        let auth = plain_auth_data(FLAG_UP, 3);
        let signed = assertion_signed_bytes(&auth, &client);
        let rng = ring::rand::SystemRandom::new();
        let signature = pair.sign(&rng, &signed).expect("sign").as_ref().to_vec();

        // A different sign counter in the authenticator data invalidates it.
        let tampered_auth = plain_auth_data(FLAG_UP, 4);
        assert_eq!(
            verify_assertion(
                &client,
                &tampered_auth,
                &signature,
                &challenge,
                &rp(),
                KeyAlgorithm::Es256,
                &public,
                0,
                false
            ),
            Err(PasskeyError::BadSignature)
        );
    }

    #[test]
    fn test_verify_assertion_refuses_a_signature_from_another_key() {
        let (public, _pair) = es256_keypair();
        let (_other_public, other_pair) = es256_keypair();
        let challenge = new_challenge();
        let client = client_data("webauthn.get", &challenge, "https://vault.home");
        let auth = plain_auth_data(FLAG_UP, 1);
        let signed = assertion_signed_bytes(&auth, &client);
        let rng = ring::rand::SystemRandom::new();
        let signature = other_pair
            .sign(&rng, &signed)
            .expect("sign")
            .as_ref()
            .to_vec();

        assert_eq!(
            verify_assertion(
                &client,
                &auth,
                &signature,
                &challenge,
                &rp(),
                KeyAlgorithm::Es256,
                &public,
                0,
                false
            ),
            Err(PasskeyError::BadSignature)
        );
    }

    #[test]
    fn test_verify_assertion_refuses_a_counter_regression() {
        let (public, pair) = es256_keypair();
        let challenge = new_challenge();
        let client = client_data("webauthn.get", &challenge, "https://vault.home");
        let auth = plain_auth_data(FLAG_UP, 5);
        let signed = assertion_signed_bytes(&auth, &client);
        let rng = ring::rand::SystemRandom::new();
        let signature = pair.sign(&rng, &signed).expect("sign").as_ref().to_vec();

        // The last stored counter is 9 — a genuine authenticator never goes
        // backwards, so a copy is reporting.
        assert_eq!(
            verify_assertion(
                &client,
                &auth,
                &signature,
                &challenge,
                &rp(),
                KeyAlgorithm::Es256,
                &public,
                9,
                false
            ),
            Err(PasskeyError::CounterRegression)
        );
    }

    #[test]
    fn test_verify_assertion_exempts_an_authenticator_without_a_counter() {
        let (public, pair) = es256_keypair();
        let challenge = new_challenge();
        let client = client_data("webauthn.get", &challenge, "https://vault.home");
        // A counter of 0 means "this authenticator does not implement one".
        let auth = plain_auth_data(FLAG_UP, 0);
        let signed = assertion_signed_bytes(&auth, &client);
        let rng = ring::rand::SystemRandom::new();
        let signature = pair.sign(&rng, &signed).expect("sign").as_ref().to_vec();

        assert!(verify_assertion(
            &client,
            &auth,
            &signature,
            &challenge,
            &rp(),
            KeyAlgorithm::Es256,
            &public,
            0,
            false
        )
        .is_ok());
    }

    #[test]
    fn test_verify_assertion_refuses_a_foreign_rp_id_and_missing_up() {
        let (public, pair) = es256_keypair();
        let challenge = new_challenge();
        let client = client_data("webauthn.get", &challenge, "https://vault.home");
        let rng = ring::rand::SystemRandom::new();

        let mut foreign = plain_auth_data(FLAG_UP, 1);
        foreign[..32].copy_from_slice(&[0x11u8; 32]);
        let signature = pair
            .sign(&rng, &assertion_signed_bytes(&foreign, &client))
            .expect("sign")
            .as_ref()
            .to_vec();
        assert_eq!(
            verify_assertion(
                &client,
                &foreign,
                &signature,
                &challenge,
                &rp(),
                KeyAlgorithm::Es256,
                &public,
                0,
                false
            ),
            Err(PasskeyError::RpIdMismatch)
        );

        let no_up = plain_auth_data(0, 1);
        let signature = pair
            .sign(&rng, &assertion_signed_bytes(&no_up, &client))
            .expect("sign")
            .as_ref()
            .to_vec();
        assert_eq!(
            verify_assertion(
                &client,
                &no_up,
                &signature,
                &challenge,
                &rp(),
                KeyAlgorithm::Es256,
                &public,
                0,
                false
            ),
            Err(PasskeyError::UserNotPresent)
        );
    }

    // -----------------------------------------------------------------
    // Challenges and encodings
    // -----------------------------------------------------------------

    #[test]
    fn test_new_challenge_is_32_random_bytes_of_base64url() {
        let a = new_challenge();
        let b = new_challenge();
        assert_ne!(a, b, "two challenges must never collide");
        assert_eq!(decode_b64url(&a).expect("decode").len(), CHALLENGE_LEN);
        assert!(!a.contains('='), "base64url without padding");
        assert!(!a.contains('+') && !a.contains('/'), "url-safe alphabet");
    }

    #[test]
    fn test_b64url_round_trip() {
        let bytes = vec![0u8, 1, 2, 250, 255];
        let encoded = encode_b64url(&bytes);
        assert_eq!(decode_b64url(&encoded).expect("decode"), bytes);
    }

    #[test]
    fn test_decode_b64url_refuses_garbage() {
        assert!(matches!(
            decode_b64url("not base64!!"),
            Err(PasskeyError::Malformed(_))
        ));
    }

    // -----------------------------------------------------------------
    // Refusals carry a sentence and a status
    // -----------------------------------------------------------------

    #[test]
    fn test_every_refusal_says_what_it_means() {
        for error in [
            PasskeyError::Malformed("x"),
            PasskeyError::WrongType,
            PasskeyError::ChallengeMismatch,
            PasskeyError::OriginMismatch,
            PasskeyError::RpIdMismatch,
            PasskeyError::UserNotPresent,
            PasskeyError::UserNotVerified,
            PasskeyError::AttestedCredentialMissing,
            PasskeyError::UnsupportedAlgorithm,
            PasskeyError::MalformedKey("x"),
            PasskeyError::BadSignature,
            PasskeyError::CredentialMismatch,
            PasskeyError::CounterRegression,
            PasskeyError::MalformedExtensions,
        ] {
            assert!(!error.message().trim().is_empty(), "{error:?}");
        }
        // A bad signature is an auth failure, not a malformed request.
        assert_eq!(
            PasskeyError::BadSignature.status(),
            axum::http::StatusCode::UNAUTHORIZED
        );
    }
}

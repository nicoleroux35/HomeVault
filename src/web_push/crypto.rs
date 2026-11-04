/* =============================================================================
 * File          : crypto.rs
 * Author        : Nicolas Le Roux
 * Created       : 06/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   The two cryptographic halves of Web Push, implemented on `ring` (already
 *   a direct dependency): RFC 8291 message encryption (aes128gcm) and RFC 8292
 *   VAPID (ES256 JWT). No new crate, no home-grown curve or block cipher —
 *   every primitive is ring's.
 *
 *   The key schedule and the record format are pinned by the RFC 8291
 *   Appendix A vector, so a change that breaks interoperability fails the
 *   tests with the expected bytes in hand.
 *
 * Dependencies:
 *   - ring (>= 0.17), base64 (>= 0.23), serde_json
 *
 * Usage:
 *   `crate::web_push` for subscriptions/keys; tests here hold the RFC vector.
 * ============================================================================= */

//! RFC 8291 encryption and RFC 8292 VAPID on top of `ring`.

use base64::engine::general_purpose::{STANDARD, URL_SAFE, URL_SAFE_NO_PAD};
use base64::Engine as _;
use ring::aead;
use ring::agreement::{self, UnparsedPublicKey};
use ring::hkdf;
use ring::rand::{SecureRandom, SystemRandom};
use ring::signature::{EcdsaKeyPair, KeyPair};
use ring::{digest, signature};

/// An uncompressed P-256 point: `0x04 || X || Y`.
pub const PUBLIC_KEY_LEN: usize = 65;
/// The browser's authentication secret.
pub const AUTH_SECRET_LEN: usize = 16;
/// aes128gcm salt.
pub const SALT_LEN: usize = 16;
/// GCM authentication tag.
pub const TAG_LEN: usize = 16;
/// The single-record size in the content-coding header.
pub const RECORD_SIZE: u32 = 4096;

/// Largest plaintext that still fits one record: `rs` must be greater than
/// plaintext + padding delimiter + tag.
pub const MAX_PLAINTEXT: usize = RECORD_SIZE as usize - 1 - TAG_LEN - 1;

/// VAPID signing and verification share this algorithm, so the test that
/// verifies a minted token uses exactly what a push service uses.
pub const JWT_ALG: &str = "ES256";

/// HMAC output length used by `hkdf::KeyType`.
struct OutputLen(usize);

impl hkdf::KeyType for OutputLen {
    fn len(&self) -> usize {
        self.0
    }
}

/// Base64url without padding, the form browsers use for subscription keys.
pub fn b64url_encode(bytes: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

/// Decode a browser-supplied base64url value, tolerating optional padding.
pub fn b64url_decode(raw: &str) -> Result<Vec<u8>, String> {
    let raw = raw.trim();
    URL_SAFE_NO_PAD
        .decode(raw)
        .or_else(|_| URL_SAFE.decode(raw))
        .map_err(|_| "The value is not valid base64url.".to_string())
}

/// Generate a VAPID key pair. Returns the PKCS#8 bytes to seal at rest and the
/// base64url public key exposed to browsers.
pub fn generate_vapid_pkcs8() -> Result<(Vec<u8>, String), String> {
    let rng = SystemRandom::new();
    let pkcs8 = EcdsaKeyPair::generate_pkcs8(&signature::ECDSA_P256_SHA256_FIXED_SIGNING, &rng)
        .map_err(|e| format!("The VAPID key could not be generated: {e}"))?;
    let key_pair = EcdsaKeyPair::from_pkcs8(
        &signature::ECDSA_P256_SHA256_FIXED_SIGNING,
        pkcs8.as_ref(),
        &rng,
    )
    .map_err(|e| format!("The generated VAPID key was refused: {e}"))?;
    Ok((
        pkcs8.as_ref().to_vec(),
        b64url_encode(key_pair.public_key().as_ref()),
    ))
}

/// Rebuild the signing key from the sealed PKCS#8 bytes.
pub fn vapid_key_pair(pkcs8: &[u8]) -> Result<EcdsaKeyPair, String> {
    EcdsaKeyPair::from_pkcs8(
        &signature::ECDSA_P256_SHA256_FIXED_SIGNING,
        pkcs8,
        &SystemRandom::new(),
    )
    .map_err(|e| format!("The stored VAPID key could not be loaded: {e}"))
}

/// The VAPID public key in the form `PushManager.subscribe` expects.
pub fn vapid_public_key(key_pair: &EcdsaKeyPair) -> String {
    b64url_encode(key_pair.public_key().as_ref())
}

/// Derive the `aud` claim: the origin of the push resource.
pub fn endpoint_audience(endpoint: &str) -> Result<String, String> {
    let url = url::Url::parse(endpoint)
        .map_err(|_| "The push endpoint is not a valid URL.".to_string())?;
    let host = url
        .host_str()
        .ok_or_else(|| "The push endpoint has no host.".to_string())?;
    let port = url.port().unwrap_or(443);
    Ok(if port == 443 {
        format!("https://{host}")
    } else {
        format!("https://{host}:{port}")
    })
}

/// Mint a VAPID JWT: serialise the header/claims, sign with ES256 (ring's
/// fixed r||s form, which is what JWT needs) and base64url the signature.
pub fn vapid_token(
    key_pair: &EcdsaKeyPair,
    audience: &str,
    subject: &str,
    expires_at: u64,
) -> Result<String, String> {
    let header = serde_json::json!({"typ": "JWT", "alg": JWT_ALG});
    let claims = serde_json::json!({
        "aud": audience,
        "exp": expires_at,
        "sub": subject,
    });
    let header = b64url_encode(header.to_string().as_bytes());
    let claims = b64url_encode(claims.to_string().as_bytes());
    let signing_input = format!("{header}.{claims}");
    let signature = key_pair
        .sign(&SystemRandom::new(), signing_input.as_bytes())
        .map_err(|e| format!("The VAPID token could not be signed: {e}"))?;
    Ok(format!(
        "{signing_input}.{}",
        b64url_encode(signature.as_ref())
    ))
}

/// HKDF-Extract + HKDF-Expand, the one shape every derivation here uses.
fn hkdf_expand(salt: &[u8], ikm: &[u8], info: &[u8], len: usize) -> Result<Vec<u8>, String> {
    let prk = hkdf::Salt::new(hkdf::HKDF_SHA256, salt).extract(ikm);
    let info_slice = [info];
    let okm = prk
        .expand(&info_slice, OutputLen(len))
        .map_err(|_| "The push key schedule could not be expanded.".to_string())?;
    let mut out = vec![0u8; len];
    okm.fill(&mut out)
        .map_err(|_| "The push key schedule produced no bytes.".to_string())?;
    Ok(out)
}

/// The RFC 8291 key schedule: `auth_secret` + ECDH secret + both public keys
/// + salt → (content encryption key, nonce).
pub fn derive_cek_nonce(
    ecdh_secret: &[u8],
    auth_secret: &[u8],
    ua_public: &[u8],
    as_public: &[u8],
    salt: &[u8],
) -> Result<([u8; 16], [u8; 12]), String> {
    if auth_secret.len() != AUTH_SECRET_LEN {
        return Err("The authentication secret must be 16 bytes.".to_string());
    }
    if ua_public.len() != PUBLIC_KEY_LEN || as_public.len() != PUBLIC_KEY_LEN {
        return Err("A P-256 public key must be 65 bytes.".to_string());
    }
    if salt.len() != SALT_LEN {
        return Err("The push salt must be 16 bytes.".to_string());
    }

    let mut key_info = Vec::with_capacity(14 + 1 + 2 * PUBLIC_KEY_LEN);
    key_info.extend_from_slice(b"WebPush: info");
    key_info.push(0);
    key_info.extend_from_slice(ua_public);
    key_info.extend_from_slice(as_public);
    let ikm = hkdf_expand(auth_secret, ecdh_secret, &key_info, 32)?;

    let cek = hkdf_expand(salt, &ikm, b"Content-Encoding: aes128gcm\x00", 16)?;
    let nonce = hkdf_expand(salt, &ikm, b"Content-Encoding: nonce\x00", 12)?;

    let mut cek_out = [0u8; 16];
    cek_out.copy_from_slice(&cek);
    let mut nonce_out = [0u8; 12];
    nonce_out.copy_from_slice(&nonce);
    Ok((cek_out, nonce_out))
}

/// The aes128gcm header: salt(16) || rs(4, BE) || idlen(1) || keyid.
pub fn record_header(salt: &[u8], record_size: u32, as_public: &[u8]) -> Result<Vec<u8>, String> {
    if salt.len() != SALT_LEN {
        return Err("The push salt must be 16 bytes.".to_string());
    }
    if as_public.len() != PUBLIC_KEY_LEN {
        return Err("A P-256 public key must be 65 bytes.".to_string());
    }
    let mut header = Vec::with_capacity(SALT_LEN + 4 + 1 + PUBLIC_KEY_LEN);
    header.extend_from_slice(salt);
    header.extend_from_slice(&record_size.to_be_bytes());
    header.push(PUBLIC_KEY_LEN as u8);
    header.extend_from_slice(as_public);
    Ok(header)
}

/// Encrypt one record: append the `0x02` padding delimiter, then AES-128-GCM.
pub fn encrypt_record(cek: &[u8], nonce: &[u8], plaintext: &[u8]) -> Result<Vec<u8>, String> {
    if cek.len() != 16 || nonce.len() != 12 {
        return Err("The push record key or nonce has the wrong length.".to_string());
    }
    let mut record = Vec::with_capacity(plaintext.len() + 1 + TAG_LEN);
    record.extend_from_slice(plaintext);
    record.push(0x02);
    let nonce_bytes: [u8; 12] = nonce
        .try_into()
        .map_err(|_| "The push nonce is not 12 bytes.".to_string())?;
    let key = aead::LessSafeKey::new(
        aead::UnboundKey::new(&aead::AES_128_GCM, cek)
            .map_err(|_| "The push content key was refused.".to_string())?,
    );
    key.seal_in_place_append_tag(
        aead::Nonce::assume_unique_for_key(nonce_bytes),
        aead::Aad::empty(),
        &mut record,
    )
    .map_err(|_| "The push record could not be encrypted.".to_string())?;
    Ok(record)
}

/// Encrypt a push message for one subscription. Returns the complete request
/// body (86-byte header + ciphertext).
pub fn encrypt_push(
    ua_public: &[u8],
    auth_secret: &[u8],
    plaintext: &[u8],
) -> Result<Vec<u8>, String> {
    if ua_public.len() != PUBLIC_KEY_LEN {
        return Err("The subscription public key must be 65 bytes.".to_string());
    }
    if auth_secret.len() != AUTH_SECRET_LEN {
        return Err("The authentication secret must be 16 bytes.".to_string());
    }
    if plaintext.len() > MAX_PLAINTEXT {
        return Err(format!(
            "A push payload is limited to {MAX_PLAINTEXT} bytes."
        ));
    }

    let rng = SystemRandom::new();
    let ephemeral = agreement::EphemeralPrivateKey::generate(&agreement::ECDH_P256, &rng)
        .map_err(|e| format!("The push key agreement could not start: {e}"))?;
    let as_public = ephemeral
        .compute_public_key()
        .map_err(|e| format!("The push public key could not be computed: {e}"))?;
    let as_public = as_public.as_ref().to_vec();

    let peer = UnparsedPublicKey::new(&agreement::ECDH_P256, ua_public);
    let ecdh_secret = agreement::agree_ephemeral(ephemeral, &peer, |material| material.to_vec())
        .map_err(|_| "The subscription public key was refused.".to_string())?;

    let mut salt = [0u8; SALT_LEN];
    rng.fill(&mut salt)
        .map_err(|_| "The push salt could not be generated.".to_string())?;

    let (cek, nonce) = derive_cek_nonce(&ecdh_secret, auth_secret, ua_public, &as_public, &salt)?;
    let ciphertext = encrypt_record(&cek, &nonce, plaintext)?;

    let mut body = record_header(&salt, RECORD_SIZE, &as_public)?;
    body.extend_from_slice(&ciphertext);
    Ok(body)
}

/// `SHA-256` helper used to keep the JWT signing input explicit in tests.
#[allow(dead_code)]
pub fn sha256(bytes: &[u8]) -> Vec<u8> {
    digest::digest(&digest::SHA256, bytes).as_ref().to_vec()
}

/// Standard base64, for the sealed PKCS#8 representation at rest.
pub fn standard_encode(bytes: &[u8]) -> String {
    STANDARD.encode(bytes)
}

/// Decode the sealed PKCS#8 representation.
pub fn standard_decode(raw: &str) -> Result<Vec<u8>, String> {
    STANDARD
        .decode(raw.trim())
        .map_err(|_| "The stored VAPID key is not valid base64.".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ring::signature::UnparsedPublicKey;

    const PLAINTEXT_B64: &str = "V2hlbiBJIGdyb3cgdXAsIEkgd2FudCB0byBiZSBhIHdhdGVybWVsb24";
    const UA_PUBLIC_B64: &str =
        "BCVxsr7N_eNgVRqvHtD0zTZsEc6-VV-JvLexhqUzORcxaOzi6-AYWXvTBHm4bjyPjs7Vd8pZGH6SRpkNtoIAiw4";
    const AS_PUBLIC_B64: &str =
        "BP4z9KsN6nGRTbVYI_c7VJSPQTBtkgcy27mlmlMoZIIgDll6e3vCYLocInmYWAmS6TlzAC8wEqKK6PBru3jl7A8";
    const SALT_B64: &str = "DGv6ra1nlYgDCS1FRnbzlw";
    const AUTH_B64: &str = "BTBZMqHH6r4Tts7J_aSIgg";
    const ECDH_SECRET_B64: &str = "kyrL1jIIOHEzg3sM2ZWRHDRB62YACZhhSlknJ672kSs";
    const CEK_B64: &str = "oIhVW04MRdy2XN9CiKLxTg";
    const NONCE_B64: &str = "4h_95klXJ5E_qnoN";
    const CIPHERTEXT_B64: &str =
        "8pfeW0KbunFT06SuDKoJH9Ql87S1QUrdirN6GcG7sFz1y1sqLgVi1VhjVkHsUoEsbI_0LpXMuGvnzQ";
    const BODY_B64: &str = "DGv6ra1nlYgDCS1FRnbzlwAAEABBBP4z9KsN6nGRTbVYI_c7VJSPQTBtkgcy27mlmlMoZIIgDll6e3vCYLocInmYWAmS6TlzAC8wEqKK6PBru3jl7A_yl95bQpu6cVPTpK4Mqgkf1CXztLVBSt2Ks3oZwbuwXPXLWyouBWLVWGNWQexSgSxsj_Qulcy4a-fN";

    fn b64(raw: &str) -> Vec<u8> {
        b64url_decode(raw).unwrap()
    }

    #[test]
    fn test_rfc8291_key_schedule_matches_the_vector() {
        let (cek, nonce) = derive_cek_nonce(
            &b64(ECDH_SECRET_B64),
            &b64(AUTH_B64),
            &b64(UA_PUBLIC_B64),
            &b64(AS_PUBLIC_B64),
            &b64(SALT_B64),
        )
        .unwrap();
        assert_eq!(cek.to_vec(), b64(CEK_B64));
        assert_eq!(nonce.to_vec(), b64(NONCE_B64));
    }

    #[test]
    fn test_rfc8291_record_matches_the_vector() {
        let ciphertext =
            encrypt_record(&b64(CEK_B64), &b64(NONCE_B64), &b64(PLAINTEXT_B64)).unwrap();
        assert_eq!(ciphertext, b64(CIPHERTEXT_B64));
    }

    #[test]
    fn test_rfc8291_header_and_body_match_the_vector() {
        let header = record_header(&b64(SALT_B64), RECORD_SIZE, &b64(AS_PUBLIC_B64)).unwrap();
        assert_eq!(header.len(), 86);
        let ciphertext =
            encrypt_record(&b64(CEK_B64), &b64(NONCE_B64), &b64(PLAINTEXT_B64)).unwrap();
        let mut body = header;
        body.extend_from_slice(&ciphertext);
        assert_eq!(body, b64(BODY_B64));
    }

    #[test]
    fn test_encrypt_push_emits_a_well_formed_single_record() {
        let body = encrypt_push(&b64(UA_PUBLIC_B64), &b64(AUTH_B64), b"hello").unwrap();
        // 86-byte header + 5 plaintext + 1 delimiter + 16 tag.
        assert_eq!(body.len(), 86 + 5 + 1 + 16);
        let salt = &body[..16];
        assert_eq!(&body[16..20], &RECORD_SIZE.to_be_bytes());
        assert_eq!(body[20], PUBLIC_KEY_LEN as u8);
        assert_eq!(body[21], 0x04);
        assert_ne!(salt, &[0u8; 16]);
    }

    #[test]
    fn test_encrypt_push_refuses_bad_key_lengths() {
        assert!(encrypt_push(&[0x04; 64], &[0u8; 16], b"x").is_err());
        assert!(encrypt_push(&[0x04; 65], &[0u8; 15], b"x").is_err());
    }

    #[test]
    fn test_payload_over_the_record_limit_is_refused() {
        assert!(encrypt_push(
            &b64(UA_PUBLIC_B64),
            &b64(AUTH_B64),
            &vec![b'x'; MAX_PLAINTEXT + 1]
        )
        .is_err());
    }

    #[test]
    fn test_vapid_token_verifies_and_carries_the_expected_claims() {
        let (pkcs8, public) = generate_vapid_pkcs8().unwrap();
        assert_eq!(public.len(), 87); // 65 bytes, base64url, no padding
        let key_pair = vapid_key_pair(&pkcs8).unwrap();
        assert_eq!(vapid_public_key(&key_pair), public);

        let token = vapid_token(
            &key_pair,
            "https://push.example",
            "mailto:a@b.c",
            1_700_000_000,
        )
        .unwrap();
        let parts: Vec<&str> = token.split('.').collect();
        assert_eq!(parts.len(), 3);
        let header: serde_json::Value =
            serde_json::from_slice(&b64url_decode(parts[0]).unwrap()).unwrap();
        let claims: serde_json::Value =
            serde_json::from_slice(&b64url_decode(parts[1]).unwrap()).unwrap();
        assert_eq!(header["alg"], "ES256");
        assert_eq!(claims["aud"], "https://push.example");
        assert_eq!(claims["sub"], "mailto:a@b.c");
        assert_eq!(claims["exp"], 1_700_000_000u64);

        let public_key = UnparsedPublicKey::new(
            &signature::ECDSA_P256_SHA256_FIXED,
            b64url_decode(&public).unwrap(),
        );
        let signing_input = format!("{}.{}", parts[0], parts[1]);
        let sig = b64url_decode(parts[2]).unwrap();
        assert_eq!(sig.len(), 64);
        public_key
            .verify(signing_input.as_bytes(), &sig)
            .expect("the push service must accept this signature");
    }

    #[test]
    fn test_endpoint_audience_is_the_https_origin() {
        assert_eq!(
            endpoint_audience("https://fcm.googleapis.com/wp/abc?x=1").unwrap(),
            "https://fcm.googleapis.com"
        );
        assert_eq!(
            endpoint_audience("https://push.example:8443/x").unwrap(),
            "https://push.example:8443"
        );
    }

    #[test]
    fn test_base64url_helpers() {
        assert_eq!(b64url_encode(&[0xff, 0x00]), "_wA");
        assert_eq!(b64url_decode("_wA").unwrap(), vec![0xff, 0x00]);
        assert!(b64url_decode("not*base64").is_err());
    }

    #[test]
    fn test_standard_base64_round_trip_for_the_sealed_key() {
        let raw = vec![1u8, 2, 3, 250, 251];
        assert_eq!(standard_decode(&standard_encode(&raw)).unwrap(), raw);
    }
}

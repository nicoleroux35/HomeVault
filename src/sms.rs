/* =============================================================================
 * File          : sms.rs
 * Author        : Nicolas Le Roux
 * Created       : 02/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   SMS alert channel (phase B3): a provider-agnostic HTTP sink beside email
 *   and the webhook. No vendor SDK and no vendor-specific API shape — the
 *   admin configures a URL and a body template, and HomeVault posts to it.
 *
 *   Everything that decides *what is sent* is pure and unit-tested: the
 *   recipient number (E.164, with a country code the admin sets), the message
 *   text (SMS is 160 GSM-7 characters per segment, and a long alert is
 *   truncated with an explicit marker rather than silently cut), the body
 *   template substitution, and the redaction of the stored credentials.
 *
 * Dependencies:
 *   - serde (>= 1.0), url (>= 2.0), crate::storage::secret_box
 *
 * Usage:
 *   handlers::settings persists the configuration; notification_engine
 *   dispatches through `send`. See docs/guides/sms.md.
 * =============================================================================
 */

//! SMS alerts through a provider-agnostic HTTP sink (see docs/guides/sms.md).
//!
//! The point of "provider-agnostic" is that HomeVault never embeds a vendor's
//! API: Twilio, OVH, Vonage, a self-hosted SMS gateway and a home-made relay
//! all accept a POST with the recipient and the text in it somewhere. The admin
//! writes the body template with `%to%` and `%text%` placeholders, so no vendor
//! SDK is added and no vendor can break the appliance by changing its API.

use serde::{Deserialize, Serialize};

/// Scope under which the SMS bearer token is sealed.
pub const SMS_TOKEN_SCOPE: &str = "sms-token";

/// One SMS segment in the GSM-7 alphabet. Beyond this a message is billed (and
/// delivered) as several segments, and most providers silently drop the tail
/// of a very long one — so the text is truncated here instead.
pub const MAX_SMS_CHARS: usize = 160;

/// Marker appended to a truncated body. Deliberately short: it eats into the
/// 160-character budget, and an ellipsis alone would leave the reader unsure
/// whether the text ended or was cut.
pub const TRUNCATION_MARKER: &str = " …";

/// Why an SMS configuration or message was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SmsError {
    /// The recipient does not look like an international phone number.
    InvalidRecipient(String),
    /// No URL configured while the channel is enabled.
    MissingEndpoint,
    /// The template does not carry the placeholders a send needs.
    MissingPlaceholder(&'static str),
    /// The configuration is enabled but nothing would be delivered.
    EnabledWithoutRecipient,
}

impl SmsError {
    /// The sentence an admin sees. It names the field and the rule.
    pub fn message(&self) -> String {
        match self {
            Self::InvalidRecipient(value) => format!(
                "'{}' is not a phone number in international form (expected a leading '+' and 8–15 digits, e.g. +33612345678)",
                value
            ),
            Self::MissingEndpoint => {
                "An SMS endpoint URL is required when SMS alerts are enabled".to_string()
            }
            Self::MissingPlaceholder(which) => format!(
                "The SMS body must contain the {} placeholder",
                which
            ),
            Self::EnabledWithoutRecipient => {
                "A recipient phone number is required when SMS alerts are enabled".to_string()
            }
        }
    }
}

/// The persisted SMS configuration.
///
/// `token_sealed` is the bearer credential, sealed with [`SMS_TOKEN_SCOPE`] and
/// **never** returned by the API — the read path answers [`redacted`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SmsSettings {
    pub enabled: bool,
    /// Full URL the appliance posts to.
    pub endpoint: String,
    /// Body template with `%to%` and `%text%` placeholders.
    pub body_template: String,
    /// Default recipient, in E.164 form (`+33612345678`).
    pub recipient: String,
    /// Optional extra header name/value pairs (`Authorization` is built from
    /// `token_sealed` instead, so it cannot be smuggled here twice).
    #[serde(default)]
    pub extra_headers: Vec<(String, String)>,
    /// The bearer token, sealed. Empty means "no Authorization header".
    #[serde(default)]
    pub token_sealed: String,
}

impl Default for SmsSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            endpoint: String::new(),
            // A neutral default that satisfies the placeholder rule, so an
            // admin who only fills in the URL and the number gets a working
            // channel instead of a validation wall.
            body_template: r#"{"to":"%to%","message":"%text%"}"#.to_string(),
            recipient: String::new(),
            extra_headers: Vec::new(),
            token_sealed: String::new(),
        }
    }
}

/// The projection the API returns: the token is replaced by whether one is set.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SmsSettingsView {
    pub enabled: bool,
    pub endpoint: String,
    pub body_template: String,
    pub recipient: String,
    pub extra_headers: Vec<(String, String)>,
    /// `true` when a token is stored. The token itself is never returned.
    pub token_set: bool,
}

/// Strip the credential before the configuration leaves the process.
pub fn redacted(settings: &SmsSettings) -> SmsSettingsView {
    SmsSettingsView {
        enabled: settings.enabled,
        endpoint: settings.endpoint.clone(),
        body_template: settings.body_template.clone(),
        recipient: settings.recipient.clone(),
        extra_headers: settings.extra_headers.clone(),
        token_set: !settings.token_sealed.trim().is_empty(),
    }
}

/// Whether `value` is a phone number in international form.
///
/// Deliberately strict: a leading `+`, then 8–15 digits and nothing else. That
/// is E.164's own bound, and it refuses the shapes that look plausible but are
/// never deliverable — `0612345678` (no country code), `+33 6 12 34 56 78`
/// (separators a provider would have to guess at), `+33-6-12` (punctuation).
pub fn is_international_number(value: &str) -> bool {
    let value = value.trim();
    let Some(digits) = value.strip_prefix('+') else {
        return false;
    };
    if !(8..=15).contains(&digits.len()) {
        return false;
    }
    digits.bytes().all(|b| b.is_ascii_digit())
}

/// Validate a configuration, returning a redacted view on success.
///
/// A disabled configuration is never refused: an admin must be able to clear a
/// half-filled form without being told their empty numbers are invalid.
pub fn validate(settings: &SmsSettings) -> Result<SmsSettingsView, SmsError> {
    if !settings.enabled {
        return Ok(redacted(settings));
    }

    let recipient = settings.recipient.trim();
    if recipient.is_empty() {
        return Err(SmsError::EnabledWithoutRecipient);
    }
    if !is_international_number(recipient) {
        return Err(SmsError::InvalidRecipient(recipient.to_string()));
    }
    if settings.endpoint.trim().is_empty() {
        return Err(SmsError::MissingEndpoint);
    }
    if !settings.body_template.contains("%to%") {
        return Err(SmsError::MissingPlaceholder("%to%"));
    }
    if !settings.body_template.contains("%text%") {
        return Err(SmsError::MissingPlaceholder("%text%"));
    }

    Ok(redacted(settings))
}

/// Coerce an alert into one SMS segment.
///
/// The title goes first: an SMS is read on a lock screen, and "[critical]
/// Disk failure" is what tells the reader to open the appliance. Line breaks
/// and runs of whitespace collapse to single spaces (they waste the budget and
/// several gateways mangle them), and a message that does not fit is truncated
/// **with** [`TRUNCATION_MARKER`] so it is visibly cut.
pub fn compose_text(severity: &str, title: &str, message: &str) -> String {
    let flat = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ");
    let body = format!("[{}] {}: {}", severity, flat(title), flat(message));
    truncate(&body, MAX_SMS_CHARS)
}

/// Truncate to at most `limit` **characters** (not bytes), leaving room for the
/// marker, and never splitting a multi-byte character.
pub fn truncate(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_string();
    }
    let budget = limit.saturating_sub(TRUNCATION_MARKER.chars().count());
    let kept: String = text.chars().take(budget).collect();
    format!("{}{}", kept.trim_end(), TRUNCATION_MARKER)
}

/// Substitute the placeholders in the body template.
///
/// The values are inserted **verbatim**: this is a JSON or form body the admin
/// wrote, and re-escaping it would corrupt a template that already quotes its
/// values. The two inputs are the appliance's own number and text, both of
/// which it controls, so there is nothing untrusted to escape.
pub fn render_body(template: &str, to: &str, text: &str) -> String {
    template.replace("%to%", to).replace("%text%", text)
}

/// The headers a send carries: the admin's extra pairs, plus `Authorization`
/// from the sealed token when one is set.
pub fn headers(settings: &SmsSettings, token: Option<&str>) -> Vec<(String, String)> {
    let mut out = settings.extra_headers.clone();
    if let Some(token) = token {
        if !token.trim().is_empty() {
            out.push((
                "Authorization".to_string(),
                format!("Bearer {}", token.trim()),
            ));
        }
    }
    out
}

/// The exact POST a send performs, so it can be asserted without a network.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SmsRequest {
    pub endpoint: String,
    pub body: String,
    pub headers: Vec<(String, String)>,
}

/// Build the request for one alert, or refuse before anything is sent.
pub fn build_request(
    settings: &SmsSettings,
    token: Option<&str>,
    severity: &str,
    title: &str,
    message: &str,
) -> Result<SmsRequest, SmsError> {
    if settings.endpoint.trim().is_empty() {
        return Err(SmsError::MissingEndpoint);
    }
    let recipient = settings.recipient.trim();
    if !is_international_number(recipient) {
        return Err(SmsError::InvalidRecipient(recipient.to_string()));
    }
    let text = compose_text(severity, title, message);
    Ok(SmsRequest {
        endpoint: settings.endpoint.trim().to_string(),
        body: render_body(&settings.body_template, recipient, &text),
        headers: headers(settings, token),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn enabled() -> SmsSettings {
        SmsSettings {
            enabled: true,
            endpoint: "https://sms.example.test/send".to_string(),
            body_template: r#"{"to":"%to%","text":"%text%"}"#.to_string(),
            recipient: "+33612345678".to_string(),
            extra_headers: vec![("X-From".to_string(), "HomeVault".to_string())],
            token_sealed: String::new(),
        }
    }

    // -- recipient validation ------------------------------------------------

    #[test]
    fn international_numbers_are_accepted() {
        for good in ["+33612345678", "+14155552671", "+8612345678901"] {
            assert!(is_international_number(good), "{good}");
        }
    }

    #[test]
    fn plausible_but_undeliverable_numbers_are_refused() {
        for bad in [
            "0612345678",        // no country code
            "+33 6 12 34 56 78", // separators a gateway would have to guess
            "+33-6-12-34-56-78", // punctuation
            "+3361234",          // too short (7 digits)
            "+1234567890123456", // too long (16 digits)
            "++33612345678",     // two plus signs
            "+33abc45678",       // letters
            "",                  // empty
        ] {
            assert!(!is_international_number(bad), "{bad} must be refused");
        }
    }

    #[test]
    fn the_refusal_names_the_expected_shape() {
        let err = SmsError::InvalidRecipient("0612345678".to_string()).message();
        assert!(err.contains("0612345678"));
        assert!(err.contains("+33612345678"));
    }

    // -- validation ----------------------------------------------------------

    #[test]
    fn a_disabled_configuration_is_never_refused() {
        // An admin clearing a half-filled form must not be told their empty
        // number is invalid.
        let mut settings = enabled();
        settings.enabled = false;
        settings.recipient = String::new();
        settings.endpoint = String::new();
        assert!(validate(&settings).is_ok());
    }

    #[test]
    fn enabling_without_a_recipient_is_refused() {
        let mut settings = enabled();
        settings.recipient = String::new();
        assert_eq!(validate(&settings), Err(SmsError::EnabledWithoutRecipient));
    }

    #[test]
    fn enabling_without_an_endpoint_is_refused() {
        let mut settings = enabled();
        settings.endpoint = "   ".to_string();
        assert_eq!(validate(&settings), Err(SmsError::MissingEndpoint));
    }

    #[test]
    fn a_template_without_the_placeholders_is_refused() {
        let mut settings = enabled();
        settings.body_template = r#"{"text":"%text%"}"#.to_string();
        assert_eq!(
            validate(&settings),
            Err(SmsError::MissingPlaceholder("%to%"))
        );

        settings.body_template = r#"{"to":"%to%"}"#.to_string();
        assert_eq!(
            validate(&settings),
            Err(SmsError::MissingPlaceholder("%text%"))
        );
    }

    // -- secrets -------------------------------------------------------------

    #[test]
    fn the_token_is_never_returned_only_its_presence() {
        let mut settings = enabled();
        settings.token_sealed = "sealed-blob".to_string();
        let view = redacted(&settings);
        assert!(view.token_set);
        // The view has no field that could carry the secret at all.
        let json = serde_json::to_string(&view).unwrap();
        assert!(!json.contains("sealed-blob"), "{json}");
    }

    #[test]
    fn the_authorization_header_is_built_from_the_token_only() {
        let settings = enabled();
        let with = headers(&settings, Some("abc123"));
        assert!(with.contains(&("Authorization".to_string(), "Bearer abc123".to_string())));
        // The admin's own headers are preserved alongside.
        assert!(with.contains(&("X-From".to_string(), "HomeVault".to_string())));

        let without = headers(&settings, None);
        assert!(!without.iter().any(|(k, _)| k == "Authorization"));
    }

    #[test]
    fn a_blank_token_produces_no_header() {
        let settings = enabled();
        let out = headers(&settings, Some("   "));
        assert!(!out.iter().any(|(k, _)| k == "Authorization"));
    }

    // -- message shaping -----------------------------------------------------

    #[test]
    fn the_text_leads_with_severity_and_title() {
        let text = compose_text("critical", "Disk failure", "md0 is degraded");
        assert!(text.starts_with("[critical] Disk failure:"), "{text}");
        assert!(text.contains("md0 is degraded"));
    }

    #[test]
    fn whitespace_collapses_because_it_wastes_the_budget() {
        let text = compose_text("warning", "Backup\n  failed", "job 12\n\n  did not finish");
        assert!(!text.contains('\n'), "{text}");
        assert!(!text.contains("  "), "{text}");
    }

    #[test]
    fn a_short_message_is_not_truncated() {
        let text = compose_text("info", "Ok", "done");
        assert!(!text.contains(TRUNCATION_MARKER), "{text}");
    }

    #[test]
    fn a_long_message_is_truncated_with_a_visible_marker() {
        let long = "x".repeat(500);
        let text = compose_text("error", "Big", &long);
        assert!(
            text.chars().count() <= MAX_SMS_CHARS,
            "{}",
            text.chars().count()
        );
        assert!(text.ends_with(TRUNCATION_MARKER.trim_end()), "{text}");
    }

    #[test]
    fn truncation_never_splits_a_multibyte_character() {
        // 200 accented characters: a byte-index cut would panic or emit a
        // replacement character.
        let text = "é".repeat(200);
        let cut = truncate(&text, 50);
        assert!(cut.chars().count() <= 50);
        assert!(!cut.contains('\u{FFFD}'), "{cut}");
    }

    #[test]
    fn compose_text_uses_characters_not_bytes() {
        let accented = "é".repeat(200);
        let text = compose_text("warning", "Accents", &accented);
        assert!(text.chars().count() <= MAX_SMS_CHARS);
    }

    // -- request building ----------------------------------------------------

    #[test]
    fn the_body_substitutes_both_placeholders() {
        let body = render_body(r#"{"to":"%to%","message":"%text%"}"#, "+33612345678", "hi");
        assert_eq!(body, r#"{"to":"+33612345678","message":"hi"}"#);
    }

    #[test]
    fn build_request_produces_the_exact_post() {
        let request = build_request(&enabled(), None, "critical", "Disk", "md0 degraded")
            .expect("a valid configuration must build");
        assert_eq!(request.endpoint, "https://sms.example.test/send");
        assert!(request.body.contains("+33612345678"));
        assert!(request.body.contains("[critical] Disk: md0 degraded"));
        assert!(request
            .headers
            .contains(&("X-From".to_string(), "HomeVault".to_string())));
    }

    #[test]
    fn build_request_refuses_before_any_network_call() {
        let mut settings = enabled();
        settings.recipient = "0612345678".to_string();
        assert_eq!(
            build_request(&settings, None, "error", "x", "y"),
            Err(SmsError::InvalidRecipient("0612345678".to_string()))
        );

        let mut settings = enabled();
        settings.endpoint = String::new();
        assert_eq!(
            build_request(&settings, None, "error", "x", "y"),
            Err(SmsError::MissingEndpoint)
        );
    }
}

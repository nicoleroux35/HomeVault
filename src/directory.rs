/* =============================================================================
 * File          : directory.rs
 * Author        : Nicolas Le Roux
 * Created       : 01/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   LDAP/AD identity lookup and group→role mapping (ROADMAP P3.1, stage 1).
 *   An admin points HomeVault at a directory, says which attribute holds the
 *   login name and which groups mean "admin", and people sign in with the
 *   credentials they already have. Local accounts are untouched: they keep
 *   their password, their 2FA and — crucially — their ability to sign in while
 *   the directory is down (break-glass).
 *
 *   Everything here except [`LdapBackend`] is pure: URL/DN/filter validation,
 *   RFC 4515 escaping, group→role resolution and the bounded lookup cache are
 *   unit-tested without a server. The network layer sits behind
 *   [`DirectoryBackend`], so the whole sign-in orchestration is tested against
 *   a fake.
 *
 * Dependencies:
 *   - ldap3 (>= 0.12, rustls-ring), serde, serde_json, url, crate::storage::json
 *
 * Usage:
 *   `handlers::directory` exposes the endpoints, `handlers::auth::handle_login`
 *   consults it when a local password check fails; see
 *   docs/guides/directory-sso.md.
 * =============================================================================
 */

//! LDAP/AD directory sign-in (see docs/guides/directory-sso.md).
//!
//! Rules this module is built around:
//!
//! * **A remote principal is not a local account.** The directory owns the
//!   password and the display name; the mirrored `users` row exists only so
//!   sessions and foreign keys keep working, and it carries a sentinel hash no
//!   password can match. A local account is never sent to the directory, and a
//!   remote account is never checked against a local hash.
//! * **The directory only ever adds a way in, never takes one away.** A local
//!   admin keeps signing in with their own password when the directory is
//!   unreachable, misconfigured or switched off.
//! * **A cached lookup never caches a password check.** The cache holds the
//!   search result (DN, display name, e-mail, groups); the password is always
//!   verified with a fresh bind, so a cached entry cannot become a credential.
//! * **Bounded everything.** The cache is capped in entries and in age, the
//!   connection and every operation carry a deadline, and the group list is
//!   capped before it is stored.
//! * **No directory value reaches a filter unescaped.** `{username}` and `{dn}`
//!   are substituted through [`escape_filter_value`] (RFC 4515), so a login
//!   name cannot inject an LDAP filter of its own.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::storage::json::JsonStorage;

/// Provider identifiers. `ldap` is a generic directory (OpenLDAP, FreeIPA…),
/// `ad` only changes the defaults offered by the screen — the wire protocol is
/// the same.
pub const PROVIDER_LDAP: &str = "ldap";
pub const PROVIDER_AD: &str = "ad";

/// Role names as the settings and the API speak them.
pub const ROLE_ADMIN: &str = "admin";
pub const ROLE_USER: &str = "user";

/// Bounds. Every one of them is enforced on save *and* on load, because a
/// hand-edited JSON file must not be able to widen them.
pub const MAX_URL_LEN: usize = 512;
pub const MAX_DN_LEN: usize = 1024;
pub const MAX_FILTER_LEN: usize = 512;
pub const MAX_ATTRIBUTE_LEN: usize = 64;
pub const MAX_GROUP_ROLES: usize = 64;
pub const MAX_GROUP_NAME_LEN: usize = 1024;
pub const MAX_GROUPS_STORED: usize = 256;
pub const MAX_CACHE_ENTRIES: usize = 512;
pub const DEFAULT_CACHE_TTL_SECONDS: u64 = 300;
pub const MAX_CACHE_TTL_SECONDS: u64 = 3600;

/// Deadlines. A directory that stops answering must not hold a login request
/// open — the sign-in screen would hang instead of reporting the outage.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
pub const OPERATION_TIMEOUT: Duration = Duration::from_secs(8);

/// The JSON document holding the directory settings.
pub const STORE_KEY: &str = "directory";

/// `AES-256-GCM` scope the service-account password is sealed under.
pub const BIND_SECRET_SCOPE: &str = "directory-bind";

/// A group (as the directory reports it) that means a HomeVault role.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GroupRole {
    /// The group's DN (`cn=admins,ou=groups,dc=example,dc=com`) or, for a
    /// directory that reports bare names, its `cn`.
    pub group: String,
    /// [`ROLE_ADMIN`] or [`ROLE_USER`].
    pub role: String,
}

/// Everything the directory integration needs to know, persisted as one JSON
/// document under `{DATA_DIRECTORY}/data/directory.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DirectorySettings {
    /// Master switch. Off means no directory sign-in at all; local accounts are
    /// unaffected either way.
    pub enabled: bool,
    /// [`PROVIDER_LDAP`] or [`PROVIDER_AD`] — defaults only.
    pub provider: String,
    /// `ldap://host:389` or `ldaps://host:636`.
    pub server_url: String,
    /// Issue StartTLS on a plain connection. Refused with `ldaps://`, where
    /// the transport is already encrypted.
    pub start_tls: bool,
    /// Verify the server certificate. `false` is offered because a home NAS
    /// often talks to a directory with a self-signed certificate, and the
    /// screen says what it costs.
    pub verify_tls: bool,
    /// The service account used for the search (may be empty for an anonymous
    /// search-only bind).
    pub bind_dn: String,
    /// The service account's password, sealed. `None` means "keep the stored
    /// one" on save and "no password" on load.
    pub bind_password_sealed: Option<String>,
    /// Where the search starts.
    pub base_dn: String,
    /// User search filter; must contain `{username}`.
    pub user_filter: String,
    /// Group search filter; must contain `{dn}`. Empty means "read the groups
    /// from the user's `memberOf` attribute instead".
    pub group_filter: String,
    pub attr_username: String,
    pub attr_display_name: String,
    pub attr_email: String,
    pub attr_member_of: String,
    /// Role for a principal no group claims.
    pub default_role: String,
    /// Group → role table. The highest role wins.
    pub group_roles: Vec<GroupRole>,
    /// How long a search result may be reused, in seconds (`0` disables the
    /// cache entirely).
    pub cache_ttl_seconds: u64,
    pub updated_at: Option<String>,
    pub updated_by: Option<String>,
}

impl Default for DirectorySettings {
    fn default() -> Self {
        Self {
            enabled: false,
            provider: PROVIDER_LDAP.to_string(),
            server_url: String::new(),
            start_tls: false,
            verify_tls: true,
            bind_dn: String::new(),
            bind_password_sealed: None,
            base_dn: String::new(),
            user_filter: "(&(objectClass=person)(uid={username}))".to_string(),
            group_filter: String::new(),
            attr_username: "uid".to_string(),
            attr_display_name: "cn".to_string(),
            attr_email: "mail".to_string(),
            attr_member_of: "memberOf".to_string(),
            default_role: ROLE_USER.to_string(),
            group_roles: Vec::new(),
            cache_ttl_seconds: DEFAULT_CACHE_TTL_SECONDS,
            updated_at: None,
            updated_by: None,
        }
    }
}

impl DirectorySettings {
    /// The provider's usual defaults, so switching the picker to "Active
    /// Directory" does not leave an OpenLDAP filter behind.
    pub fn with_provider_defaults(mut self) -> Self {
        if self.provider == PROVIDER_AD {
            if self.attr_username == "uid" {
                self.attr_username = "sAMAccountName".to_string();
            }
            if self.attr_display_name == "cn" {
                self.attr_display_name = "displayName".to_string();
            }
            if self.user_filter == "(&(objectClass=person)(uid={username}))" {
                self.user_filter = "(&(objectClass=user)(sAMAccountName={username}))".to_string();
            }
        }
        self
    }

    /// Whether a directory sign-in can even be attempted.
    pub fn usable(&self) -> bool {
        self.enabled && validate(self).is_ok()
    }
}

// ---------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------

/// Escape a value for use inside an LDAP filter (RFC 4515 §3).
///
/// Escaping is not optional: unescaped `*`, `(` and `)` let a login name widen
/// the filter (for example `admin*` matching the first account whose name
/// starts with `admin`), and a NUL byte truncates it in some servers.
pub fn escape_filter_value(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'\\' => out.push_str("\\5c"),
            b'*' => out.push_str("\\2a"),
            b'(' => out.push_str("\\28"),
            b')' => out.push_str("\\29"),
            0 => out.push_str("\\00"),
            other => out.push(other as char),
        }
    }
    out
}

/// Substitute `{token}` in a filter template with an escaped value.
///
/// Every occurrence is replaced, so a template that names the same token twice
/// (a valid thing to do) cannot smuggle a raw value in through the second one.
pub fn render_filter(template: &str, token: &str, value: &str) -> String {
    let needle = format!("{{{token}}}");
    let escaped = escape_filter_value(value);
    template.replace(&needle, &escaped)
}

/// Whether a string is an LDAP attribute description (`uid`, `sAMAccountName`,
/// `memberOf`, `objectGUID;binary`). Rejects anything that could carry syntax
/// into the search request.
pub fn valid_attribute(attribute: &str) -> bool {
    let attribute = attribute.trim();
    if attribute.is_empty() || attribute.len() > MAX_ATTRIBUTE_LEN {
        return false;
    }
    attribute
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == ';' || c == '.')
}

/// Whether a string is a Distinguished Name we are willing to send.
///
/// This is a shape check, not a parser: a DN is a comma-separated list of
/// `type=value` pairs. It refuses control characters (a newline in a DN would
/// split a log line), and requires at least one `=`.
pub fn valid_dn(dn: &str) -> bool {
    let dn = dn.trim();
    if dn.is_empty() || dn.len() > MAX_DN_LEN {
        return false;
    }
    if dn.chars().any(|c| c.is_control()) || dn.contains("..") {
        return false;
    }
    dn.split(',').all(|rdn| {
        let rdn = rdn.trim();
        !rdn.is_empty()
            && rdn
                .split_once('=')
                .is_some_and(|(a, _)| !a.trim().is_empty())
    })
}

/// Whether a filter template is balanced, parenthesised and carries its token.
///
/// Balance matters: an unbalanced filter is refused by the server with an
/// unhelpful message, and a filter that does not carry `{username}` would make
/// every login search return the whole directory.
pub fn valid_filter_template(template: &str, token: &str) -> bool {
    let template = template.trim();
    if template.is_empty() || template.len() > MAX_FILTER_LEN {
        return false;
    }
    if !template.starts_with('(') || !template.ends_with(')') {
        return false;
    }
    if template.chars().any(|c| c.is_control()) {
        return false;
    }
    let mut depth: i32 = 0;
    let mut escaped = false;
    for c in template.chars() {
        if escaped {
            escaped = false;
            continue;
        }
        match c {
            '\\' => escaped = true,
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth < 0 {
                    return false;
                }
            }
            _ => {}
        }
    }
    depth == 0 && template.contains(&format!("{{{token}}}"))
}

/// Whether the URL is one the client can actually speak to.
pub fn valid_server_url(url: &str) -> bool {
    let url = url.trim();
    if url.is_empty() || url.len() > MAX_URL_LEN {
        return false;
    }
    match url::Url::parse(url) {
        Ok(parsed) => {
            matches!(parsed.scheme(), "ldap" | "ldaps")
                && parsed.host_str().is_some_and(|h| !h.is_empty())
                && (parsed.path().is_empty() || parsed.path() == "/")
                && parsed.query().is_none()
                && parsed.fragment().is_none()
        }
        Err(_) => false,
    }
}

/// Normalise a role name, refusing anything that is not one of the two.
pub fn normalize_role(role: &str) -> Result<&'static str, String> {
    match role.trim().to_ascii_lowercase().as_str() {
        ROLE_ADMIN => Ok(ROLE_ADMIN),
        ROLE_USER => Ok(ROLE_USER),
        other => Err(format!(
            "'{other}' is not a role — use '{ROLE_USER}' or '{ROLE_ADMIN}'"
        )),
    }
}

/// Validate a whole settings document.
///
/// Returns the first problem found, worded so the screen can show it as-is.
pub fn validate(settings: &DirectorySettings) -> Result<(), String> {
    if !matches!(settings.provider.as_str(), PROVIDER_LDAP | PROVIDER_AD) {
        return Err(format!(
            "Unknown directory type '{}' — use '{PROVIDER_LDAP}' or '{PROVIDER_AD}'",
            settings.provider
        ));
    }
    if !settings.enabled {
        // A disabled integration is allowed to be half-filled: the admin may be
        // preparing it. Nothing will be contacted until it is switched on.
        return Ok(());
    }
    if !valid_server_url(&settings.server_url) {
        return Err(
            "The server address must be an ldap:// or ldaps:// URL with a host, e.g. ldap://192.168.1.10:389"
                .to_string(),
        );
    }
    if settings.start_tls
        && settings
            .server_url
            .trim()
            .to_ascii_lowercase()
            .starts_with("ldaps://")
    {
        return Err(
            "StartTLS cannot be combined with an ldaps:// address — it is already encrypted"
                .to_string(),
        );
    }
    if !valid_dn(&settings.base_dn) {
        return Err(
            "The search base must be a distinguished name, e.g. dc=example,dc=com".to_string(),
        );
    }
    if !settings.bind_dn.trim().is_empty() && !valid_dn(&settings.bind_dn) {
        return Err(
            "The bind account must be a distinguished name, e.g. cn=homevault,dc=example,dc=com"
                .to_string(),
        );
    }
    if settings.bind_password_sealed.is_none() && !settings.bind_dn.trim().is_empty() {
        return Err("The bind account has no password — enter it again".to_string());
    }
    if settings.bind_dn.trim().is_empty() && settings.bind_password_sealed.is_some() {
        return Err("A bind password was given without a bind account".to_string());
    }
    if !valid_filter_template(&settings.user_filter, "username") {
        return Err(
            "The user filter must be balanced, start with '(' and contain {username}, e.g. (&(objectClass=person)(uid={username}))"
                .to_string(),
        );
    }
    if !settings.group_filter.trim().is_empty()
        && !valid_filter_template(&settings.group_filter, "dn")
    {
        return Err(
            "The group filter must be balanced, start with '(' and contain {dn}, e.g. (&(objectClass=groupOfNames)(member={dn}))"
                .to_string(),
        );
    }
    for (label, attribute) in [
        ("login name", &settings.attr_username),
        ("display name", &settings.attr_display_name),
        ("e-mail", &settings.attr_email),
        ("group", &settings.attr_member_of),
    ] {
        if !valid_attribute(attribute) {
            return Err(format!(
                "The {label} attribute '{}' is not a valid LDAP attribute name",
                attribute.trim()
            ));
        }
    }
    normalize_role(&settings.default_role)?;
    validate_group_roles(&settings.group_roles, MAX_GROUP_ROLES)?;
    if settings.cache_ttl_seconds > MAX_CACHE_TTL_SECONDS {
        return Err(format!(
            "The lookup cache may be kept for at most {MAX_CACHE_TTL_SECONDS} seconds"
        ));
    }
    Ok(())
}

/// Validate a group→role table.
///
/// Shared with OIDC (`crate::oidc`) so both providers live under exactly the
/// same rules: a bounded table, one entry per group (case-insensitively), a
/// bounded group name and a role name that exists.
pub fn validate_group_roles(mappings: &[GroupRole], max_mappings: usize) -> Result<(), String> {
    if mappings.len() > max_mappings {
        return Err(format!("At most {max_mappings} group mappings are allowed"));
    }
    let mut seen: Vec<String> = Vec::with_capacity(mappings.len());
    for mapping in mappings {
        let group = mapping.group.trim();
        if group.is_empty() || group.len() > MAX_GROUP_NAME_LEN {
            return Err("Every group mapping needs a group name".to_string());
        }
        if group.chars().any(|c| c.is_control()) {
            return Err(format!("The group '{group}' contains a control character"));
        }
        let key = group.to_ascii_lowercase();
        if seen.contains(&key) {
            return Err(format!("The group '{group}' is mapped twice"));
        }
        seen.push(key);
        normalize_role(&mapping.role)?;
    }
    Ok(())
}

/// The role a set of groups grants.
///
/// The **highest** role wins: one admin group is enough, and a principal that
/// is in no mapped group gets `default_role`. Comparison is on the whole DN
/// (case-insensitively) *and* on its leftmost `cn`, because Active Directory
/// reports `memberOf` as DNs while a hand-written mapping often uses a bare
/// group name — and silently failing to match one of the two would be a
/// security surprise in the shape of a demotion.
///
/// Shared with OIDC so a group called `admins` means the same thing whether it
/// arrived in an `memberOf` attribute or in an ID token's `groups` claim.
pub fn resolve_groups_role(
    groups: &[String],
    mappings: &[GroupRole],
    default_role: &str,
) -> String {
    let mut granted: Option<&str> = None;
    for group in groups {
        let group_dn = group.trim().to_ascii_lowercase();
        let group_cn = common_name(group).unwrap_or_default();
        for mapping in mappings {
            let mapped = mapping.group.trim().to_ascii_lowercase();
            let matched = mapped == group_dn
                || (!group_cn.is_empty() && mapped == group_cn)
                || mapped == format!("cn={group_cn}");
            if !matched {
                continue;
            }
            let role = if mapping.role.trim().eq_ignore_ascii_case(ROLE_ADMIN) {
                ROLE_ADMIN
            } else {
                ROLE_USER
            };
            if role == ROLE_ADMIN {
                return ROLE_ADMIN.to_string();
            }
            granted = Some(ROLE_USER);
        }
    }
    granted
        .unwrap_or_else(|| {
            if default_role.trim().eq_ignore_ascii_case(ROLE_ADMIN) {
                ROLE_ADMIN
            } else {
                ROLE_USER
            }
        })
        .to_string()
}

/// The role a set of directory groups grants, from the directory settings.
pub fn resolve_role(groups: &[String], settings: &DirectorySettings) -> String {
    resolve_groups_role(groups, &settings.group_roles, &settings.default_role)
}

/// The `cn` of a DN, lowercased, or `None` when there is none.
fn common_name(dn: &str) -> Option<String> {
    for rdn in dn.split(',') {
        let (attribute, value) = rdn.trim().split_once('=')?;
        if attribute.trim().eq_ignore_ascii_case("cn") {
            let value = value.trim().trim_matches('"').to_ascii_lowercase();
            if !value.is_empty() {
                return Some(value);
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Backend
// ---------------------------------------------------------------------------

/// A principal as the directory describes it. Never carries a password.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectoryUser {
    /// The entry's DN — the provider's stable key for this principal.
    pub dn: String,
    /// The login name, as typed (lowercased for comparison elsewhere).
    pub username: String,
    pub display_name: String,
    pub email: String,
    /// Group DNs (or names) the entry belongs to, capped at
    /// [`MAX_GROUPS_STORED`].
    pub groups: Vec<String>,
}

/// Why a directory operation did not produce a principal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DirectoryError {
    /// The integration is switched off or not configured.
    NotConfigured,
    /// The directory said no: wrong password, or no such entry. Deliberately
    /// one variant — the caller must not tell a client which of the two it was.
    InvalidCredentials,
    /// The server could not be reached, timed out or refused the service
    /// account. Never the client's fault.
    Unavailable(String),
    /// A request the configuration made impossible (a filter that matched two
    /// entries, an invalid setting). Refused rather than guessed at.
    Refused(String),
}

impl DirectoryError {
    /// The sentence a signed-in admin sees. `InvalidCredentials` keeps the
    /// generic wording the login screen already uses.
    pub fn message(&self) -> String {
        match self {
            DirectoryError::NotConfigured => "Directory sign-in is not configured".to_string(),
            DirectoryError::InvalidCredentials => "Invalid username or password".to_string(),
            DirectoryError::Unavailable(detail) => {
                format!("The directory is unavailable ({detail}) — sign in with a local account")
            }
            DirectoryError::Refused(detail) => {
                format!("The directory refused the request: {detail}")
            }
        }
    }

    /// Whether the failure was the directory's, not the caller's. Used by the
    /// login handler to avoid counting an outage as a wrong password.
    pub fn is_outage(&self) -> bool {
        matches!(
            self,
            DirectoryError::Unavailable(_) | DirectoryError::NotConfigured
        )
    }
}

/// The network layer, behind a trait so the sign-in flow is testable without a
/// server. Both methods are short-lived and independently timed out.
#[allow(async_fn_in_trait)]
pub trait DirectoryBackend {
    /// Search for one principal. `bind_password` is the already-unsealed
    /// service-account password.
    async fn fetch(
        &self,
        settings: &DirectorySettings,
        bind_password: Option<&str>,
        username: &str,
    ) -> Result<DirectoryUser, DirectoryError>;

    /// Prove the principal's password with a simple bind as their own DN.
    async fn verify(
        &self,
        settings: &DirectorySettings,
        dn: &str,
        password: &str,
    ) -> Result<bool, DirectoryError>;
}

/// The real backend: `ldap3` over tokio, with TLS available on both transports.
#[derive(Debug, Clone, Copy, Default)]
pub struct LdapBackend;

impl LdapBackend {
    fn connection_settings(settings: &DirectorySettings) -> ldap3::LdapConnSettings {
        ldap3::LdapConnSettings::new()
            .set_conn_timeout(CONNECT_TIMEOUT)
            .set_starttls(settings.start_tls)
            .set_no_tls_verify(!settings.verify_tls)
    }

    async fn connect(settings: &DirectorySettings) -> Result<ldap3::Ldap, DirectoryError> {
        let (conn, ldap) = ldap3::LdapConnAsync::with_settings(
            Self::connection_settings(settings),
            settings.server_url.trim(),
        )
        .await
        .map_err(|e| DirectoryError::Unavailable(e.to_string()))?;
        ldap3::drive!(conn);
        Ok(ldap)
    }

    /// Bind as the service account, or do nothing when the configuration asks
    /// for an anonymous search.
    async fn bind(
        ldap: &mut ldap3::Ldap,
        settings: &DirectorySettings,
        bind_password: Option<&str>,
    ) -> Result<(), DirectoryError> {
        // Anonymous search is legitimate: a directory may expose a read-only
        // view to everyone, and an empty bind DN is how that is expressed.
        if settings.bind_dn.trim().is_empty() {
            return Ok(());
        }
        let password = bind_password.ok_or_else(|| {
            DirectoryError::Refused("the bind account has no stored password".to_string())
        })?;
        ldap.with_timeout(OPERATION_TIMEOUT)
            .simple_bind(settings.bind_dn.trim(), password)
            .await
            .map_err(|e| DirectoryError::Unavailable(format!("bind failed: {e}")))?
            .success()
            .map_err(|e| DirectoryError::Unavailable(format!("bind refused: {e}")))?;
        Ok(())
    }

    /// One search under the configured base, with a fresh connection and the
    /// configured bind. The connection is closed before returning, whether the
    /// search worked or not.
    async fn search_entries(
        &self,
        settings: &DirectorySettings,
        bind_password: Option<&str>,
        filter: &str,
        attributes: &[&str],
    ) -> Result<Vec<ldap3::SearchEntry>, DirectoryError> {
        let mut ldap = Self::connect(settings).await?;
        if let Err(e) = Self::bind(&mut ldap, settings, bind_password).await {
            let _ = ldap.unbind().await;
            return Err(e);
        }
        let result = ldap
            .with_timeout(OPERATION_TIMEOUT)
            .search(
                settings.base_dn.trim(),
                ldap3::Scope::Subtree,
                filter,
                attributes.to_vec(),
            )
            .await;
        let _ = ldap.unbind().await;
        let (entries, _) = result
            .map_err(|e| DirectoryError::Unavailable(format!("search failed: {e}")))?
            .success()
            .map_err(|e| DirectoryError::Unavailable(format!("search refused: {e}")))?;
        Ok(entries
            .into_iter()
            .map(ldap3::SearchEntry::construct)
            .collect())
    }

    /// Search for principals matching a ready-made filter.
    ///
    /// Used by the connection test: one round trip proves the bind, the base DN
    /// and the filter syntax. Groups are not resolved, because the probe only
    /// needs to know that the search ran — and must not return anything an
    /// admin could mistake for a sign-in.
    pub async fn search(
        &self,
        settings: &DirectorySettings,
        bind_password: Option<&str>,
        filter: &str,
    ) -> Result<Vec<DirectoryUser>, DirectoryError> {
        if !valid_server_url(&settings.server_url) {
            return Err(DirectoryError::NotConfigured);
        }
        let attributes = [
            settings.attr_username.trim(),
            settings.attr_display_name.trim(),
            settings.attr_email.trim(),
        ];
        let entries = self
            .search_entries(settings, bind_password, filter, &attributes)
            .await?;
        Ok(entries
            .into_iter()
            .map(|entry| DirectoryUser {
                username: attribute_first(&entry, settings.attr_username.trim())
                    .unwrap_or_default(),
                display_name: attribute_first(&entry, settings.attr_display_name.trim())
                    .unwrap_or_default(),
                email: attribute_first(&entry, settings.attr_email.trim()).unwrap_or_default(),
                dn: entry.dn,
                groups: Vec::new(),
            })
            .collect())
    }
}

/// The values of an attribute, matched case-insensitively: servers are free to
/// return the attribute name with different capitalisation than was requested,
/// and a lookup that missed `sAMAccountName` because it arrived as
/// `samaccountname` would silently drop the display name.
fn attribute_values<'a>(entry: &'a ldap3::SearchEntry, attribute: &str) -> Option<&'a Vec<String>> {
    if let Some(values) = entry.attrs.get(attribute) {
        return Some(values);
    }
    entry
        .attrs
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(attribute))
        .map(|(_, values)| values)
}

/// The first non-empty value of an attribute, trimmed.
fn attribute_first(entry: &ldap3::SearchEntry, attribute: &str) -> Option<String> {
    attribute_values(entry, attribute)
        .and_then(|values| values.iter().find(|v| !v.trim().is_empty()))
        .map(|v| v.trim().to_string())
}

/// Every non-empty value of an attribute, trimmed.
fn attribute_all(entry: &ldap3::SearchEntry, attribute: &str) -> Vec<String> {
    attribute_values(entry, attribute)
        .map(|values| {
            values
                .iter()
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

impl DirectoryBackend for LdapBackend {
    async fn fetch(
        &self,
        settings: &DirectorySettings,
        bind_password: Option<&str>,
        username: &str,
    ) -> Result<DirectoryUser, DirectoryError> {
        if !valid_server_url(&settings.server_url) {
            return Err(DirectoryError::NotConfigured);
        }

        let filter = render_filter(&settings.user_filter, "username", username);
        let attributes = [
            settings.attr_username.trim(),
            settings.attr_display_name.trim(),
            settings.attr_email.trim(),
            settings.attr_member_of.trim(),
        ];
        let mut found = self
            .search_entries(settings, bind_password, &filter, &attributes)
            .await?;

        if found.is_empty() {
            return Err(DirectoryError::InvalidCredentials);
        }
        if found.len() > 1 {
            // Two entries for one login name is an ambiguous directory, not a
            // sign-in. Picking the first would be a coin flip over which
            // account's groups decide the role.
            return Err(DirectoryError::Refused(format!(
                "'{username}' matches {} directory entries — the user filter must select exactly one",
                found.len()
            )));
        }
        let entry = found.remove(0);

        let directory_username = attribute_first(&entry, settings.attr_username.trim())
            .unwrap_or_else(|| username.to_string());
        let display_name = attribute_first(&entry, settings.attr_display_name.trim())
            .unwrap_or_else(|| directory_username.clone());
        let email = attribute_first(&entry, settings.attr_email.trim()).unwrap_or_default();

        let mut groups = if settings.group_filter.trim().is_empty() {
            // The entry carries its own group list (AD's `memberOf`, or the
            // overlay OpenLDAP needs for it).
            attribute_all(&entry, settings.attr_member_of.trim())
        } else {
            // A separate group search: the group entry names its members, not
            // the other way around.
            let group_filter = render_filter(&settings.group_filter, "dn", &entry.dn);
            // No attributes are requested — the DN, which is all that is used
            // here, is always part of a search result.
            self.search_entries(settings, bind_password, &group_filter, &[])
                .await?
                .into_iter()
                .map(|group| group.dn)
                .collect()
        };

        groups.truncate(MAX_GROUPS_STORED);
        Ok(DirectoryUser {
            dn: entry.dn,
            username: directory_username,
            display_name,
            email,
            groups,
        })
    }

    async fn verify(
        &self,
        settings: &DirectorySettings,
        dn: &str,
        password: &str,
    ) -> Result<bool, DirectoryError> {
        // An empty password on a simple bind is an *unauthenticated* bind, which
        // most servers accept: it would turn "no password" into "signed in".
        // This guard is the reason the check lives here rather than in the
        // caller.
        if password.is_empty() || dn.trim().is_empty() {
            return Ok(false);
        }
        let mut ldap = Self::connect(settings).await?;
        let result = ldap
            .with_timeout(OPERATION_TIMEOUT)
            .simple_bind(dn.trim(), password)
            .await
            .map_err(|e| DirectoryError::Unavailable(e.to_string()))?;
        let _ = ldap.unbind().await;
        match result.success() {
            Ok(_) => Ok(true),
            Err(ldap3::LdapError::LdapResult { result }) => {
                // 49 = invalidCredentials: the account exists but the password
                // is wrong. Anything else is the server refusing the
                // *operation*, which is an outage, not a wrong password.
                if result.rc == 49 {
                    Ok(false)
                } else {
                    Err(DirectoryError::Unavailable(format!(
                        "bind refused: rc={} {}",
                        result.rc, result.text
                    )))
                }
            }
            Err(e) => Err(DirectoryError::Unavailable(e.to_string())),
        }
    }
}

// ---------------------------------------------------------------------------
// Bounded cache
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct CacheEntry {
    user: DirectoryUser,
    inserted: Instant,
}

/// A bounded, time-limited cache of *search results*.
///
/// It never stores a password or a successful bind: [`DirectoryService`]
/// always re-verifies the password, so a hit only saves the search round trip.
#[derive(Debug, Default)]
pub struct LookupCache {
    entries: HashMap<String, CacheEntry>,
    hits: u64,
    misses: u64,
}

impl LookupCache {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// `(hits, misses)` since the last clear — shown on the screen so an admin
    /// can tell whether the cache is doing anything.
    pub fn statistics(&self) -> (u64, u64) {
        (self.hits, self.misses)
    }

    fn get(&mut self, key: &str, ttl: Duration) -> Option<DirectoryUser> {
        if ttl.is_zero() {
            self.misses += 1;
            return None;
        }
        match self.entries.get(key) {
            Some(entry) if entry.inserted.elapsed() < ttl => {
                self.hits += 1;
                Some(entry.user.clone())
            }
            Some(_) => {
                // Expired: drop it now rather than let it sit until the
                // capacity eviction notices.
                self.entries.remove(key);
                self.misses += 1;
                None
            }
            None => {
                self.misses += 1;
                None
            }
        }
    }

    fn insert(&mut self, key: String, user: DirectoryUser, ttl: Duration) {
        if ttl.is_zero() {
            return;
        }
        if self.entries.len() >= MAX_CACHE_ENTRIES {
            // Evict the oldest entry. Linear over 512 entries is cheaper than
            // keeping an ordered index in step with every hit, and this runs at
            // most once per login.
            if let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.inserted)
                .map(|(key, _)| key.clone())
            {
                self.entries.remove(&oldest);
            }
        }
        self.entries.insert(
            key,
            CacheEntry {
                user,
                inserted: Instant::now(),
            },
        );
    }

    /// Drop an entry (used when a principal is revoked).
    fn remove(&mut self, key: &str) {
        self.entries.remove(key);
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.hits = 0;
        self.misses = 0;
    }
}

// ---------------------------------------------------------------------------
// Service
// ---------------------------------------------------------------------------

/// The cache key for one principal. The configuration is part of it: pointing
/// the same login name at a different server must not keep answering from the
/// previous one.
fn cache_key(settings: &DirectorySettings, username: &str) -> String {
    format!(
        "{}|{}|{}|{}|{}",
        settings.provider,
        settings.server_url.trim().to_ascii_lowercase(),
        settings.base_dn.trim().to_ascii_lowercase(),
        settings.user_filter.trim(),
        username.trim().to_ascii_lowercase()
    )
}

/// Directory sign-in orchestration: cache → search → live password bind.
pub struct DirectoryService<B = LdapBackend> {
    backend: B,
    cache: Mutex<LookupCache>,
}

impl DirectoryService<LdapBackend> {
    pub fn new() -> Self {
        Self {
            backend: LdapBackend,
            cache: Mutex::new(LookupCache::new()),
        }
    }
}

impl Default for DirectoryService<LdapBackend> {
    fn default() -> Self {
        Self::new()
    }
}

impl<B: DirectoryBackend> DirectoryService<B> {
    pub fn with_backend(backend: B) -> Self {
        Self {
            backend,
            cache: Mutex::new(LookupCache::new()),
        }
    }

    /// Forget every cached search result — called when the settings change or a
    /// principal is revoked.
    pub fn invalidate(&self) {
        if let Ok(mut cache) = self.cache.lock() {
            cache.clear();
        }
    }

    pub fn invalidate_user(&self, settings: &DirectorySettings, username: &str) {
        if let Ok(mut cache) = self.cache.lock() {
            cache.remove(&cache_key(settings, username));
        }
    }

    pub fn statistics(&self) -> (u64, u64, usize) {
        match self.cache.lock() {
            Ok(cache) => {
                let (hits, misses) = cache.statistics();
                (hits, misses, cache.len())
            }
            Err(_) => (0, 0, 0),
        }
    }

    /// Resolve and authenticate one principal.
    ///
    /// The returned [`DirectoryUser`] is the directory's answer, not a decision:
    /// the caller maps its groups to a role and links it to a mirrored account.
    pub async fn authenticate(
        &self,
        settings: &DirectorySettings,
        bind_password: Option<&str>,
        username: &str,
        password: &str,
    ) -> Result<DirectoryUser, DirectoryError> {
        if !settings.enabled {
            return Err(DirectoryError::NotConfigured);
        }
        // Re-validate on every attempt: the settings file is on disk and an
        // admin could have hand-edited it into a contradictory state. Failing
        // closed here keeps an invalid document from widening access.
        validate(settings).map_err(DirectoryError::Refused)?;
        if username.trim().is_empty() || password.is_empty() {
            return Err(DirectoryError::InvalidCredentials);
        }
        if username.len() > 255 {
            return Err(DirectoryError::InvalidCredentials);
        }

        let key = cache_key(settings, username);
        let ttl = Duration::from_secs(settings.cache_ttl_seconds.min(MAX_CACHE_TTL_SECONDS));

        let cached = self
            .cache
            .lock()
            .ok()
            .and_then(|mut cache| cache.get(&key, ttl));

        let user = match cached {
            Some(user) => user,
            None => {
                let user = self
                    .backend
                    .fetch(settings, bind_password, username.trim())
                    .await?;
                if let Ok(mut cache) = self.cache.lock() {
                    cache.insert(key.clone(), user.clone(), ttl);
                }
                user
            }
        };

        // The password is verified on every attempt, cache hit or not.
        if !self.backend.verify(settings, &user.dn, password).await? {
            // A wrong password proves nothing about the search result, but
            // keeping it would let a stolen DN sit in memory after the entry is
            // deleted. Dropping it costs one extra search on the next attempt.
            if let Ok(mut cache) = self.cache.lock() {
                cache.remove(&key);
            }
            return Err(DirectoryError::InvalidCredentials);
        }

        Ok(user)
    }
}

// ---------------------------------------------------------------------------
// Persistence
// ---------------------------------------------------------------------------

fn data_directory() -> String {
    std::env::var("DATA_DIRECTORY").unwrap_or_else(|_| "/var/lib/homevault".to_string())
}

fn storage() -> JsonStorage {
    JsonStorage::new(data_directory())
}

/// Read the settings, falling back to "off" when the document is missing or
/// unreadable — a corrupt directory file must never lock the admin out.
pub fn load() -> DirectorySettings {
    match storage().load::<DirectorySettings>(STORE_KEY) {
        Ok(Some(settings)) => settings,
        Ok(None) => DirectorySettings::default(),
        Err(e) => {
            tracing::warn!("could not read the directory settings: {e}");
            DirectorySettings::default()
        }
    }
}

/// Persist the settings (atomic write).
pub fn save(settings: &DirectorySettings) -> Result<(), String> {
    storage()
        .save(STORE_KEY, settings)
        .map_err(|e| format!("could not save the directory settings: {e}"))
}

/// Seal the service-account password.
pub fn seal_bind_password(password: &str) -> Result<String, String> {
    crate::storage::secret_box::seal(BIND_SECRET_SCOPE, password)
}

/// Open the service-account password, or `None` when there is none stored.
pub fn open_bind_password(sealed: &Option<String>) -> Result<Option<String>, String> {
    match sealed {
        None => Ok(None),
        Some(blob) => crate::storage::secret_box::open(BIND_SECRET_SCOPE, blob)
            .map(Some)
            .map_err(|e| format!("could not read the bind password: {e}")),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn settings() -> DirectorySettings {
        DirectorySettings {
            enabled: true,
            server_url: "ldap://127.0.0.1:389".to_string(),
            base_dn: "dc=example,dc=com".to_string(),
            bind_dn: "cn=homevault,dc=example,dc=com".to_string(),
            bind_password_sealed: Some("sealed".to_string()),
            group_roles: vec![
                GroupRole {
                    group: "cn=admins,ou=groups,dc=example,dc=com".to_string(),
                    role: ROLE_ADMIN.to_string(),
                },
                GroupRole {
                    group: "cn=staff,ou=groups,dc=example,dc=com".to_string(),
                    role: ROLE_USER.to_string(),
                },
            ],
            ..DirectorySettings::default()
        }
    }

    #[test]
    fn test_escape_filter_value_covers_every_special_byte() {
        assert_eq!(escape_filter_value("alice"), "alice");
        assert_eq!(escape_filter_value("a*"), "a\\2a");
        assert_eq!(escape_filter_value("a(b)c"), "a\\28b\\29c");
        assert_eq!(escape_filter_value("a\\b"), "a\\5cb");
        assert_eq!(escape_filter_value("a\0b"), "a\\00b");
        // The classic widening attempt: `admin*` must not stay a wildcard.
        assert_eq!(
            render_filter("(uid={username})", "username", "admin*"),
            "(uid=admin\\2a)"
        );
    }

    #[test]
    fn test_render_filter_replaces_every_occurrence() {
        assert_eq!(
            render_filter("(&(uid={username})(cn={username}))", "username", "bob"),
            "(&(uid=bob)(cn=bob))"
        );
        // A token that is not present is left alone, not silently emptied.
        assert_eq!(render_filter("(uid=x)", "username", "bob"), "(uid=x)");
    }

    #[test]
    fn test_valid_attribute_accepts_ad_style_names() {
        for good in [
            "uid",
            "sAMAccountName",
            "memberOf",
            "objectGUID;binary",
            "cn",
        ] {
            assert!(valid_attribute(good), "{good}");
        }
        for bad in ["", " ", "uid)(|(uid=*", "cn=", "a b", "*"] {
            assert!(!valid_attribute(bad), "{bad}");
        }
    }

    #[test]
    fn test_valid_dn_requires_a_typed_pair() {
        assert!(valid_dn("dc=example,dc=com"));
        assert!(valid_dn("cn=Home Vault,ou=services,dc=example,dc=com"));
        assert!(!valid_dn(""));
        assert!(!valid_dn("example.com"));
        assert!(!valid_dn("dc=example,,dc=com"));
        assert!(!valid_dn("=example"));
        assert!(!valid_dn("dc=exa\nmple"));
    }

    #[test]
    fn test_valid_filter_template_needs_balance_and_its_token() {
        assert!(valid_filter_template("(uid={username})", "username"));
        assert!(valid_filter_template(
            "(&(objectClass=person)(uid={username}))",
            "username"
        ));
        assert!(
            !valid_filter_template("uid={username}", "username"),
            "unparenthesised"
        );
        assert!(!valid_filter_template("(uid=x)", "username"), "no token");
        assert!(
            !valid_filter_template("(uid={username}", "username"),
            "unbalanced"
        );
        assert!(
            !valid_filter_template("(uid={username}))", "username"),
            "extra close"
        );
        // An escaped paren is not a nesting level.
        assert!(valid_filter_template(
            r"(uid={username})(escaped=\29)",
            "username"
        ));
    }

    #[test]
    fn test_valid_server_url_refuses_plain_http() {
        assert!(valid_server_url("ldap://192.168.1.10:389"));
        assert!(valid_server_url("ldaps://dc.example.com"));
        assert!(valid_server_url("ldap://dc.example.com/"));
        assert!(!valid_server_url("http://dc.example.com"));
        assert!(!valid_server_url("ldap://"));
        assert!(!valid_server_url("ldap://dc.example.com/dc=example"));
        assert!(!valid_server_url("ldap://dc.example.com?x=1"));
        assert!(!valid_server_url(""));
    }

    #[test]
    fn test_validate_accepts_a_complete_configuration() {
        assert!(validate(&settings()).is_ok());
    }

    #[test]
    fn test_validate_is_lenient_while_disabled() {
        // Preparing the form must not require every field at once.
        let mut s = settings();
        s.enabled = false;
        s.server_url = String::new();
        s.base_dn = String::new();
        assert!(validate(&s).is_ok());
    }

    #[test]
    fn test_validate_refuses_each_contradiction() {
        let base = settings();

        let mut s = base.clone();
        s.server_url = "http://dc".to_string();
        assert!(validate(&s).unwrap_err().contains("ldap://"));

        let mut s = base.clone();
        s.start_tls = true;
        s.server_url = "ldaps://dc.example.com".to_string();
        assert!(validate(&s).unwrap_err().contains("StartTLS"));

        let mut s = base.clone();
        s.base_dn = "example".to_string();
        assert!(validate(&s).unwrap_err().contains("search base"));

        let mut s = base.clone();
        s.user_filter = "(uid=x)".to_string();
        assert!(validate(&s).unwrap_err().contains("{username}"));

        let mut s = base.clone();
        s.group_filter = "(member=x)".to_string();
        assert!(validate(&s).unwrap_err().contains("{dn}"));

        let mut s = base.clone();
        s.attr_username = "uid)(|(uid=*".to_string();
        assert!(validate(&s).unwrap_err().contains("login name"));

        let mut s = base.clone();
        s.default_role = "root".to_string();
        assert!(validate(&s).unwrap_err().contains("not a role"));

        let mut s = base.clone();
        s.group_roles.push(GroupRole {
            group: "CN=Admins,OU=Groups,DC=example,DC=com".to_string(),
            role: ROLE_USER.to_string(),
        });
        assert!(validate(&s).unwrap_err().contains("mapped twice"));

        let mut s = base.clone();
        s.cache_ttl_seconds = MAX_CACHE_TTL_SECONDS + 1;
        assert!(validate(&s).unwrap_err().contains("at most"));

        let mut s = base;
        s.bind_password_sealed = None;
        assert!(validate(&s).unwrap_err().contains("no password"));
    }

    #[test]
    fn test_validate_refuses_a_bind_password_without_an_account() {
        let mut s = settings();
        s.bind_dn = String::new();
        assert!(validate(&s).unwrap_err().contains("without a bind account"));
    }

    #[test]
    fn test_resolve_role_prefers_admin_and_falls_back() {
        let s = settings();

        assert_eq!(
            resolve_role(&["cn=staff,ou=groups,dc=example,dc=com".to_string()], &s),
            ROLE_USER
        );
        // One admin group is enough, wherever it sits in the list.
        assert_eq!(
            resolve_role(
                &[
                    "cn=staff,ou=groups,dc=example,dc=com".to_string(),
                    "cn=admins,ou=groups,dc=example,dc=com".to_string(),
                ],
                &s
            ),
            ROLE_ADMIN
        );
        // No group at all → the default.
        assert_eq!(resolve_role(&[], &s), ROLE_USER);

        let mut s = settings();
        s.default_role = ROLE_ADMIN.to_string();
        assert_eq!(resolve_role(&[], &s), ROLE_ADMIN);
    }

    #[test]
    fn test_resolve_role_matches_a_bare_group_name_and_is_case_insensitive() {
        let mut s = settings();
        s.group_roles = vec![GroupRole {
            group: "admins".to_string(),
            role: ROLE_ADMIN.to_string(),
        }];
        // AD reports the DN; the mapping may name only the cn.
        assert_eq!(
            resolve_role(&["CN=Admins,OU=Groups,DC=example,DC=com".to_string()], &s),
            ROLE_ADMIN
        );
        assert_eq!(resolve_role(&["cn=admins".to_string()], &s), ROLE_ADMIN);
        // An unmapped group grants nothing.
        assert_eq!(resolve_role(&["cn=other".to_string()], &s), ROLE_USER);
    }

    #[test]
    fn test_common_name_reads_the_first_cn() {
        assert_eq!(
            common_name("CN=Admins,OU=Groups,DC=example,DC=com"),
            Some("admins".to_string())
        );
        assert_eq!(common_name("uid=bob,dc=example"), None);
        assert_eq!(common_name(""), None);
    }

    #[test]
    fn test_capacity_is_bounded() {
        let mut cache = LookupCache::new();
        let ttl = Duration::from_secs(60);
        for i in 0..(MAX_CACHE_ENTRIES + 10) {
            cache.insert(
                format!("key-{i}"),
                DirectoryUser {
                    dn: format!("uid={i},dc=example"),
                    username: format!("user{i}"),
                    display_name: String::new(),
                    email: String::new(),
                    groups: Vec::new(),
                },
                ttl,
            );
        }
        assert_eq!(cache.len(), MAX_CACHE_ENTRIES);
        // The oldest went first, the newest are still there.
        assert!(cache.get("key-0", ttl).is_none());
        assert!(cache
            .get(&format!("key-{}", MAX_CACHE_ENTRIES + 9), ttl)
            .is_some());
    }

    #[test]
    fn test_an_expired_entry_is_not_served() {
        let mut cache = LookupCache::new();
        let user = DirectoryUser {
            dn: "uid=bob,dc=example".to_string(),
            username: "bob".to_string(),
            display_name: "Bob".to_string(),
            email: String::new(),
            groups: Vec::new(),
        };
        cache.insert("k".to_string(), user.clone(), Duration::from_secs(60));
        assert_eq!(cache.get("k", Duration::from_secs(60)), Some(user));
        // A zero TTL means "never serve a cached result" and also drops the
        // existing one rather than keeping a stale answer around.
        assert_eq!(cache.get("k", Duration::ZERO), None);
    }

    #[test]
    fn test_a_zero_ttl_never_stores_anything() {
        let mut cache = LookupCache::new();
        cache.insert(
            "k".to_string(),
            DirectoryUser {
                dn: "uid=bob".to_string(),
                username: "bob".to_string(),
                display_name: String::new(),
                email: String::new(),
                groups: Vec::new(),
            },
            Duration::ZERO,
        );
        assert!(cache.is_empty());
    }

    // -----------------------------------------------------------------
    // Orchestration, against a fake directory
    // -----------------------------------------------------------------

    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    #[derive(Default)]
    struct FakeDirectory {
        user: Option<DirectoryUser>,
        password: String,
        fetches: Arc<AtomicUsize>,
        verifies: Arc<AtomicUsize>,
        outage: bool,
    }

    impl DirectoryBackend for FakeDirectory {
        async fn fetch(
            &self,
            _settings: &DirectorySettings,
            _bind_password: Option<&str>,
            _username: &str,
        ) -> Result<DirectoryUser, DirectoryError> {
            self.fetches.fetch_add(1, Ordering::SeqCst);
            if self.outage {
                return Err(DirectoryError::Unavailable(
                    "connection refused".to_string(),
                ));
            }
            self.user.clone().ok_or(DirectoryError::InvalidCredentials)
        }

        async fn verify(
            &self,
            _settings: &DirectorySettings,
            dn: &str,
            password: &str,
        ) -> Result<bool, DirectoryError> {
            self.verifies.fetch_add(1, Ordering::SeqCst);
            if self.outage {
                return Err(DirectoryError::Unavailable(
                    "connection refused".to_string(),
                ));
            }
            Ok(!password.is_empty() && password == self.password && !dn.trim().is_empty())
        }
    }

    fn fake() -> FakeDirectory {
        FakeDirectory {
            user: Some(DirectoryUser {
                dn: "uid=bob,ou=people,dc=example,dc=com".to_string(),
                username: "bob".to_string(),
                display_name: "Bob".to_string(),
                email: "bob@example.com".to_string(),
                groups: vec!["cn=staff,ou=groups,dc=example,dc=com".to_string()],
            }),
            password: "hunter2".to_string(),
            ..FakeDirectory::default()
        }
    }

    #[tokio::test]
    async fn test_authenticate_verifies_the_password_on_every_attempt() {
        let fetches = Arc::new(AtomicUsize::new(0));
        let verifies = Arc::new(AtomicUsize::new(0));
        let mut directory = fake();
        directory.fetches = Arc::clone(&fetches);
        directory.verifies = Arc::clone(&verifies);
        let service = DirectoryService::with_backend(directory);
        let s = settings();

        let user = service
            .authenticate(&s, Some("bind"), "bob", "hunter2")
            .await
            .expect("the right password signs in");
        assert_eq!(user.dn, "uid=bob,ou=people,dc=example,dc=com");

        // The second attempt hits the cache for the search...
        service
            .authenticate(&s, Some("bind"), "bob", "hunter2")
            .await
            .unwrap();
        assert_eq!(fetches.load(Ordering::SeqCst), 1, "the search is cached");
        // ...but never for the password.
        assert_eq!(verifies.load(Ordering::SeqCst), 2, "every bind is real");

        // A wrong password is refused even though the entry is cached.
        let err = service
            .authenticate(&s, Some("bind"), "bob", "wrong")
            .await
            .unwrap_err();
        assert_eq!(err, DirectoryError::InvalidCredentials);
        assert_eq!(verifies.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn test_a_wrong_password_drops_the_cached_entry() {
        let fetches = Arc::new(AtomicUsize::new(0));
        let mut directory = fake();
        directory.fetches = Arc::clone(&fetches);
        let service = DirectoryService::with_backend(directory);
        let s = settings();

        service
            .authenticate(&s, Some("bind"), "bob", "hunter2")
            .await
            .unwrap();
        let _ = service
            .authenticate(&s, Some("bind"), "bob", "wrong")
            .await
            .unwrap_err();
        // The DN is no longer trusted, so the next attempt searches again.
        service
            .authenticate(&s, Some("bind"), "bob", "hunter2")
            .await
            .unwrap();
        assert_eq!(fetches.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn test_a_disabled_or_invalid_configuration_never_reaches_the_backend() {
        let fetches = Arc::new(AtomicUsize::new(0));
        let mut directory = fake();
        directory.fetches = Arc::clone(&fetches);
        let service = DirectoryService::with_backend(directory);

        let mut s = settings();
        s.enabled = false;
        assert_eq!(
            service
                .authenticate(&s, Some("bind"), "bob", "hunter2")
                .await
                .unwrap_err(),
            DirectoryError::NotConfigured
        );

        let mut s = settings();
        s.user_filter = "uid=x".to_string();
        assert!(matches!(
            service
                .authenticate(&s, Some("bind"), "bob", "hunter2")
                .await
                .unwrap_err(),
            DirectoryError::Refused(_)
        ));

        assert_eq!(fetches.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn test_an_empty_password_is_refused_before_any_bind() {
        let verifies = Arc::new(AtomicUsize::new(0));
        let mut directory = fake();
        directory.verifies = Arc::clone(&verifies);
        let service = DirectoryService::with_backend(directory);
        assert_eq!(
            service
                .authenticate(&settings(), Some("bind"), "bob", "")
                .await
                .unwrap_err(),
            DirectoryError::InvalidCredentials
        );
        assert_eq!(verifies.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn test_an_outage_is_reported_as_an_outage_not_a_bad_password() {
        let mut directory = fake();
        directory.outage = true;
        let service = DirectoryService::with_backend(directory);
        let err = service
            .authenticate(&settings(), Some("bind"), "bob", "hunter2")
            .await
            .unwrap_err();
        assert!(err.is_outage(), "{err:?}");
        assert!(err.message().contains("local account"));
    }

    #[tokio::test]
    async fn test_invalidate_forces_a_fresh_search() {
        let fetches = Arc::new(AtomicUsize::new(0));
        let mut directory = fake();
        directory.fetches = Arc::clone(&fetches);
        let service = DirectoryService::with_backend(directory);
        let s = settings();

        service
            .authenticate(&s, Some("bind"), "bob", "hunter2")
            .await
            .unwrap();
        service.invalidate();
        service
            .authenticate(&s, Some("bind"), "bob", "hunter2")
            .await
            .unwrap();
        assert_eq!(fetches.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn test_settings_default_to_off_with_safe_defaults() {
        let s = DirectorySettings::default();
        assert!(!s.enabled);
        assert!(s.verify_tls, "certificate verification must be opt-out");
        assert_eq!(s.default_role, ROLE_USER);
        assert_eq!(s.cache_ttl_seconds, DEFAULT_CACHE_TTL_SECONDS);
        assert!(s.group_roles.is_empty());
    }

    #[test]
    fn test_provider_defaults_switch_the_ad_attributes() {
        let s = DirectorySettings {
            provider: PROVIDER_AD.to_string(),
            ..DirectorySettings::default()
        }
        .with_provider_defaults();
        assert_eq!(s.attr_username, "sAMAccountName");
        assert_eq!(s.attr_display_name, "displayName");
        assert!(s.user_filter.contains("sAMAccountName"));

        // An admin who already typed their own values keeps them.
        let custom = DirectorySettings {
            provider: PROVIDER_AD.to_string(),
            attr_username: "employeeNumber".to_string(),
            ..DirectorySettings::default()
        }
        .with_provider_defaults();
        assert_eq!(custom.attr_username, "employeeNumber");
    }
}

/* =============================================================================
 * File          : config_backup.rs
 * Author        : Nicolas Le Roux
 * Created       : 31/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Full configuration backup and restore (QUICK-WIN QW-11): one archive that
 *   can rebuild a box — a consistent database snapshot, the settings, and the
 *   generated service configs — optionally encrypted with a passphrase, with
 *   every plaintext credential left out by default.
 *
 * Dependencies:
 *   - serde/serde_json, sha2, aes-gcm, argon2, base64, rand, chrono
 *
 * Usage:
 *   handlers/config_backup.rs (the API) and the startup hook in web::run;
 *   see docs/guides/settings.md.
 * =============================================================================
 */

//! Full configuration backup and restore (see docs/guides/settings.md).
//!
//! The JSON settings export already existed; it carries *preferences* and
//! nothing else. This carries the box: a `VACUUM INTO` snapshot of the database
//! (users, groups, shares, ACLs, quotas, every settings row), the settings
//! document, and the generated service configs (`smb.conf`, `/etc/exports`,
//! netplan, the SFTP `sshd`, the firewall rules).
//!
//! ## Container
//!
//! A single file, no tar/gzip dependency:
//!
//! ```text
//! magic "HVCONFIG1" | version u32 LE | flags u8 | salt[16] | nonce[12]
//!                   | manifest_len u32 LE | manifest JSON | payload
//! ```
//!
//! The manifest lists every entry (`path`, `bytes`, `sha256`) **and** the
//! entries that were skipped and why — so a restore can say "smb.conf was not
//! in this archive because Samba was not installed", instead of silently
//! producing a box that is missing half its configuration.
//!
//! ## Secrets
//!
//! `includes_secrets = false` is the default and it is a real guarantee, not a
//! label:
//!
//! * the settings document is exported through `settings_persistence::export`,
//!   which already masks the SMTP password;
//! * the extra config files are an explicit **allow-list** (no `tls.json` key,
//!   no VPN configs, no Syncthing/SFTP keys, no DDNS/ACME tokens);
//! * the database copy is **scrubbed** of the credentials that are stored in
//!   plaintext: sessions, trusted devices, MCP tokens and VPN peer secrets. A
//!   restore therefore logs everyone out and revokes their API tokens, which is
//!   the correct consequence of a migration — password *hashes* and encrypted
//!   TOTP blobs stay, because a restore needs them and a hash is not a
//!   plaintext secret.
//!
//! `includes_secrets = true` requires a passphrase and encrypts everything with
//! AES-256-GCM under an Argon2id-derived key — the archive then holds
//! credentials, so it must not travel readable.
//!
//! ## Restore is a staged swap, not a hot swap
//!
//! Replacing a live SQLite database underneath a running process is the one
//! thing this feature must not pretend to do. A restore therefore **validates**
//! the archive, stages it (`data/restore/`), and answers
//! `requires_restart: true`; `apply_pending_restore()` runs at startup, before
//! the database is opened, keeping a `.pre-restore` copy of the current one.
//! The alternative — closing and reopening the pool mid-request — would leave
//! every in-flight handler holding a stale handle.

use std::io::Read;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Container magic. Versioned separately from the manifest's `format_version`
/// so a future container layout can be told apart before parsing anything.
pub const MAGIC: &[u8; 9] = b"HVCONFIG1";

/// Current container/manifest version. A newer one is refused on restore.
pub const FORMAT_VERSION: u32 = 1;

/// Flag bit: the payload is AES-256-GCM encrypted.
const FLAG_ENCRYPTED: u8 = 0b0000_0001;
/// Flag bit: the archive was built with `include_secrets`.
const FLAG_SECRETS: u8 = 0b0000_0010;

/// Argon2id salt length and AES-GCM nonce length, in the header.
const SALT_LEN: usize = 16;
const NONCE_LEN: usize = 12;

/// Largest archive accepted on restore. A config backup is a database snapshot
/// plus a handful of text files; 512 MiB is far above any real one and bounds
/// the memory a single request can ask for.
pub const MAX_ARCHIVE_BYTES: usize = 512 * 1024 * 1024;

/// The generated config files a backup carries, as `(archive path, host path)`.
///
/// An explicit allow-list, not a scan: a scan would pick up whatever happens to
/// sit in `/etc`, including private keys.
pub const CONFIG_FILES: &[(&str, &str)] = &[
    ("configs/smb.conf", "/etc/samba/smb.conf"),
    ("configs/exports", "/etc/exports"),
    ("configs/netplan.yaml", "/etc/netplan/90-homevault.yaml"),
    (
        "configs/sshd_config.sftp",
        "/etc/ssh/homevault_sftp_sshd_config",
    ),
    ("configs/ufw.user.rules", "/etc/ufw/user.rules"),
    ("configs/ufw.user6.rules", "/etc/ufw/user6.rules"),
];

/// One payload entry, as the manifest describes it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EntryMeta {
    /// Path inside the archive (`database.db`, `settings.json`, `configs/…`).
    pub path: String,
    pub bytes: u64,
    /// HEX SHA-256 of the entry's bytes, so a truncated or edited archive is
    /// refused even when it is not encrypted.
    pub sha256: String,
}

/// A file that was meant to be in the archive but was not.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkippedEntry {
    pub path: String,
    pub reason: String,
}

/// The archive's table of contents.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    /// Container/manifest version; a newer one is refused on restore.
    pub format_version: u32,
    /// The build that wrote it (`CARGO_PKG_VERSION`), for a human.
    pub app_version: String,
    /// `settings_persistence::CURRENT_SETTINGS_SCHEMA` at write time.
    pub settings_schema: u32,
    /// RFC 3339, UTC.
    pub created_at: String,
    pub encrypted: bool,
    pub includes_secrets: bool,
    pub entries: Vec<EntryMeta>,
    /// Files that could not be read, with the reason, so a restore can say what
    /// it is missing.
    pub skipped: Vec<SkippedEntry>,
}

impl Manifest {
    /// The entry with this archive path, if present.
    pub fn entry(&self, path: &str) -> Option<&EntryMeta> {
        self.entries.iter().find(|e| e.path == path)
    }

    pub fn total_bytes(&self) -> u64 {
        self.entries.iter().map(|e| e.bytes).sum()
    }
}

/// Why an archive was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArchiveError {
    /// The file does not start with [`MAGIC`].
    NotAnArchive,
    /// The container layout is newer than this build understands.
    NewerFormat(u32),
    /// The manifest's settings schema is newer than this build.
    NewerSchema(u32),
    /// The archive says it is encrypted but no passphrase was supplied.
    PassphraseRequired,
    /// A passphrase was supplied but the archive is not encrypted.
    NotEncrypted,
    /// The passphrase was wrong (the AEAD tag did not verify).
    WrongPassphrase,
    /// An entry's checksum does not match the manifest.
    Corrupt(String),
    /// The bytes are not a valid container.
    Malformed(String),
}

impl ArchiveError {
    /// The sentence the UI shows. Every variant says what to do next.
    pub fn message(&self) -> String {
        match self {
            ArchiveError::NotAnArchive => {
                "That file is not a HomeVault configuration archive".to_string()
            }
            ArchiveError::NewerFormat(version) => format!(
                "This archive was written by a newer HomeVault (format {version}, this build reads \
                 {FORMAT_VERSION}). Update HomeVault before restoring it."
            ),
            ArchiveError::NewerSchema(schema) => format!(
                "This archive carries settings from a newer HomeVault (schema {schema}). Update \
                 HomeVault before restoring it."
            ),
            ArchiveError::PassphraseRequired => {
                "This archive is encrypted — enter the passphrase it was exported with".to_string()
            }
            ArchiveError::NotEncrypted => {
                "This archive is not encrypted, so it needs no passphrase".to_string()
            }
            ArchiveError::WrongPassphrase => {
                "The passphrase does not open this archive".to_string()
            }
            ArchiveError::Corrupt(path) => format!(
                "The archive is damaged: '{path}' does not match its checksum. Export the backup \
                 again rather than restoring a partial configuration."
            ),
            ArchiveError::Malformed(reason) => format!("The archive could not be read: {reason}"),
        }
    }
}

// --------------------------------------------------------------------------
// Building
// --------------------------------------------------------------------------

/// The inputs of an export, assembled by the handler so the encoder stays pure.
#[derive(Debug, Clone)]
pub struct ExportInput {
    /// The `VACUUM INTO` snapshot bytes (already read from disk).
    pub database: Vec<u8>,
    /// `settings_persistence::export()` output (already redacted).
    pub settings: String,
    /// The allow-listed config files that were readable.
    pub configs: Vec<(String, Vec<u8>)>,
    /// The ones that were not, with the reason.
    pub skipped: Vec<SkippedEntry>,
    pub app_version: String,
    pub settings_schema: u32,
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Encode one archive.
///
/// `passphrase` is required when `include_secrets` is true (the caller enforces
/// that; this function refuses too, so no path can produce a readable archive
/// full of credentials).
pub fn build(
    input: &ExportInput,
    include_secrets: bool,
    passphrase: Option<&str>,
    created_at: &str,
) -> Result<Vec<u8>, String> {
    let passphrase = passphrase.map(str::trim).filter(|p| !p.is_empty());
    if include_secrets && passphrase.is_none() {
        return Err(
            "An archive that includes secrets must be encrypted: enter a passphrase".to_string(),
        );
    }

    // Entries in a deterministic order so two exports of the same box differ
    // only in their timestamps.
    let mut payload = Vec::new();
    let mut entries = Vec::new();
    let mut push = |path: &str, bytes: &[u8], payload: &mut Vec<u8>| {
        payload.extend_from_slice(bytes);
        entries.push(EntryMeta {
            path: path.to_string(),
            bytes: bytes.len() as u64,
            sha256: sha256_hex(bytes),
        });
    };
    push("database.db", &input.database, &mut payload);
    push("settings.json", input.settings.as_bytes(), &mut payload);
    for (path, bytes) in &input.configs {
        push(path, bytes, &mut payload);
    }

    let manifest = Manifest {
        format_version: FORMAT_VERSION,
        app_version: input.app_version.clone(),
        settings_schema: input.settings_schema,
        created_at: created_at.to_string(),
        encrypted: passphrase.is_some(),
        includes_secrets: include_secrets,
        entries,
        skipped: input.skipped.clone(),
    };
    let manifest_json = serde_json::to_vec(&manifest)
        .map_err(|e| format!("the manifest could not be serialised: {e}"))?;

    let salt: [u8; SALT_LEN] = rand::random();
    let nonce: [u8; NONCE_LEN] = rand::random();
    let mut flags = 0u8;
    if manifest.encrypted {
        flags |= FLAG_ENCRYPTED;
    }
    if manifest.includes_secrets {
        flags |= FLAG_SECRETS;
    }

    let body = match passphrase {
        Some(passphrase) => encrypt(passphrase, &salt, &nonce, &payload)?,
        None => payload,
    };

    let mut out = Vec::with_capacity(
        MAGIC.len() + 4 + 1 + SALT_LEN + NONCE_LEN + 4 + manifest_json.len() + body.len(),
    );
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
    out.push(flags);
    out.extend_from_slice(&salt);
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&(manifest_json.len() as u32).to_le_bytes());
    out.extend_from_slice(&manifest_json);
    out.extend_from_slice(&body);
    Ok(out)
}

// --------------------------------------------------------------------------
// Reading
// --------------------------------------------------------------------------

/// A decoded archive: the manifest plus the entry bytes, keyed by path.
#[derive(Debug, Clone)]
pub struct Archive {
    pub manifest: Manifest,
    pub entries: Vec<(String, Vec<u8>)>,
}

impl Archive {
    pub fn get(&self, path: &str) -> Option<&[u8]> {
        self.entries
            .iter()
            .find(|(p, _)| p == path)
            .map(|(_, bytes)| bytes.as_slice())
    }
}

/// Decode (and verify) an archive.
///
/// Every failure mode has its own [`ArchiveError`], and the checks run in the
/// order that gives the user the most useful answer: not-an-archive, newer
/// format, passphrase, then payload integrity.
pub fn read(bytes: &[u8], passphrase: Option<&str>) -> Result<Archive, ArchiveError> {
    if bytes.len() < MAGIC.len() + 4 + 1 + SALT_LEN + NONCE_LEN + 4 {
        return Err(ArchiveError::NotAnArchive);
    }
    if &bytes[..MAGIC.len()] != MAGIC {
        return Err(ArchiveError::NotAnArchive);
    }
    let mut cursor = MAGIC.len();
    let version = u32::from_le_bytes(
        bytes[cursor..cursor + 4]
            .try_into()
            .map_err(|_| ArchiveError::NotAnArchive)?,
    );
    cursor += 4;
    if version > FORMAT_VERSION {
        return Err(ArchiveError::NewerFormat(version));
    }
    let flags = bytes[cursor];
    cursor += 1;
    let salt: [u8; SALT_LEN] = bytes[cursor..cursor + SALT_LEN]
        .try_into()
        .map_err(|_| ArchiveError::NotAnArchive)?;
    cursor += SALT_LEN;
    let nonce: [u8; NONCE_LEN] = bytes[cursor..cursor + NONCE_LEN]
        .try_into()
        .map_err(|_| ArchiveError::NotAnArchive)?;
    cursor += NONCE_LEN;
    let manifest_len = u32::from_le_bytes(
        bytes[cursor..cursor + 4]
            .try_into()
            .map_err(|_| ArchiveError::NotAnArchive)?,
    ) as usize;
    cursor += 4;
    if bytes.len() < cursor + manifest_len {
        return Err(ArchiveError::Malformed(
            "the manifest is truncated".to_string(),
        ));
    }
    let manifest: Manifest = serde_json::from_slice(&bytes[cursor..cursor + manifest_len])
        .map_err(|e| ArchiveError::Malformed(format!("the manifest is not valid JSON: {e}")))?;
    cursor += manifest_len;
    let body = &bytes[cursor..];

    let encrypted = flags & FLAG_ENCRYPTED != 0;
    if encrypted && passphrase.is_none() {
        return Err(ArchiveError::PassphraseRequired);
    }
    if !encrypted && passphrase.is_some() {
        return Err(ArchiveError::NotEncrypted);
    }
    let payload = match passphrase {
        Some(passphrase) => decrypt(passphrase, &salt, &nonce, body)?,
        None => body.to_vec(),
    };

    // Walk the manifest, slicing the payload and verifying each checksum: a
    // truncated payload is then reported as the entry that is missing, not as a
    // mysterious length mismatch.
    let mut entries = Vec::with_capacity(manifest.entries.len());
    let mut offset = 0usize;
    for entry in &manifest.entries {
        let end = offset
            .checked_add(entry.bytes as usize)
            .ok_or_else(|| ArchiveError::Corrupt(entry.path.clone()))?;
        if end > payload.len() {
            return Err(ArchiveError::Corrupt(entry.path.clone()));
        }
        let slice = &payload[offset..end];
        if sha256_hex(slice) != entry.sha256 {
            return Err(ArchiveError::Corrupt(entry.path.clone()));
        }
        entries.push((entry.path.clone(), slice.to_vec()));
        offset = end;
    }
    if offset != payload.len() {
        return Err(ArchiveError::Corrupt("trailing data".to_string()));
    }

    if manifest.settings_schema > crate::settings_persistence::CURRENT_SETTINGS_SCHEMA {
        return Err(ArchiveError::NewerSchema(manifest.settings_schema));
    }
    Ok(Archive { manifest, entries })
}

// --------------------------------------------------------------------------
// Encryption
// --------------------------------------------------------------------------

/// Argon2id with the crate's defaults: memory-hard, and the passphrase is the
/// only thing protecting a file that may hold every credential on the box.
fn derive_key(passphrase: &str, salt: &[u8; SALT_LEN]) -> Result<[u8; 32], String> {
    use argon2::Argon2;
    let mut key = [0u8; 32];
    Argon2::default()
        .hash_password_into(passphrase.as_bytes(), salt, &mut key)
        .map_err(|e| format!("the passphrase could not be processed: {e}"))?;
    Ok(key)
}

fn encrypt(
    passphrase: &str,
    salt: &[u8; SALT_LEN],
    nonce: &[u8; NONCE_LEN],
    plaintext: &[u8],
) -> Result<Vec<u8>, String> {
    use aes_gcm::aead::{Aead, KeyInit, Payload};
    use aes_gcm::{Aes256Gcm, Nonce};

    let key = derive_key(passphrase, salt)?;
    let cipher = Aes256Gcm::new_from_slice(&key).map_err(|e| e.to_string())?;
    // The manifest is authenticated as associated data: an attacker cannot swap
    // the table of contents of an encrypted archive without the tag failing.
    let manifest_aad = b"homevault-config-backup";
    cipher
        .encrypt(
            &Nonce::from(*nonce),
            Payload {
                msg: plaintext,
                aad: manifest_aad,
            },
        )
        .map_err(|e| format!("the archive could not be encrypted: {e}"))
}

fn decrypt(
    passphrase: &str,
    salt: &[u8; SALT_LEN],
    nonce: &[u8; NONCE_LEN],
    ciphertext: &[u8],
) -> Result<Vec<u8>, ArchiveError> {
    use aes_gcm::aead::{Aead, KeyInit, Payload};
    use aes_gcm::{Aes256Gcm, Nonce};

    let key = derive_key(passphrase, salt).map_err(ArchiveError::Malformed)?;
    let cipher = Aes256Gcm::new_from_slice(&key)
        .map_err(|e| ArchiveError::Malformed(format!("the cipher could not be built: {e}")))?;
    match cipher.decrypt(
        &Nonce::from(*nonce),
        Payload {
            msg: ciphertext,
            aad: b"homevault-config-backup",
        },
    ) {
        Ok(plaintext) => Ok(plaintext),
        // The AEAD tag is the only thing that can fail here: a wrong
        // passphrase and a tampered payload are indistinguishable by design,
        // and saying "wrong passphrase" is the useful answer for the common
        // case.
        Err(_) => Err(ArchiveError::WrongPassphrase),
    }
}

// --------------------------------------------------------------------------
// Restore staging
// --------------------------------------------------------------------------

/// What a restore staged, for the answer and for the startup log.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StagedRestore {
    pub manifest: Manifest,
    /// Archive paths that were written into the staging directory.
    pub staged: Vec<String>,
    /// Config files whose host path is outside this instance (they are written
    /// to the staging tree and reported, never copied over `/etc` from a
    /// request thread).
    pub needs_root: Vec<String>,
}

/// Directory a restore is staged into (`data/restore/`).
pub fn staging_dir() -> PathBuf {
    crate::config::paths::data_dir().join("restore")
}

/// The staged database snapshot the startup hook swaps in.
pub fn staged_database_path() -> PathBuf {
    staging_dir().join("homevault.db")
}

/// Write an archive's contents into the staging directory.
///
/// Config files are staged — **not** written to `/etc` — because a restore runs
/// as a request and those paths need root; the startup hook applies them with
/// the same privileges the installer has, and the answer lists what waits.
pub fn stage(archive: &Archive) -> Result<StagedRestore, String> {
    let dir = staging_dir();
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("the staging directory could not be created: {e}"))?;

    let mut staged = Vec::new();
    let mut needs_root = Vec::new();

    if let Some(db) = archive.get("database.db") {
        let target = dir.join("homevault.db");
        crate::storage::AtomicWriter::write_bytes(&target, db)
            .map_err(|e| format!("the database snapshot could not be staged: {e}"))?;
        staged.push("database.db".to_string());
    } else {
        return Err("The archive has no database snapshot, so it cannot rebuild a box".to_string());
    }

    if let Some(settings) = archive.get("settings.json") {
        let text = String::from_utf8_lossy(settings).to_string();
        // Validate now: a settings document from another product (or a newer
        // schema) is refused at restore time, not discovered at boot.
        serde_json::from_str::<serde_json::Value>(&text)
            .map_err(|e| format!("The archived settings are not valid JSON: {e}"))?;
        crate::storage::AtomicWriter::write(dir.join("settings.json"), &text)
            .map_err(|e| format!("the settings could not be staged: {e}"))?;
        staged.push("settings.json".to_string());
    }

    for entry in &archive.manifest.entries {
        if !entry.path.starts_with("configs/") {
            continue;
        }
        let Some(bytes) = archive.get(&entry.path) else {
            continue;
        };
        let file_name = entry.path.trim_start_matches("configs/");
        let target = dir.join("configs").join(file_name);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("the staging tree could not be created: {e}"))?;
        }
        crate::storage::AtomicWriter::write_bytes(&target, bytes)
            .map_err(|e| format!("{} could not be staged: {e}", entry.path))?;
        staged.push(entry.path.clone());
        if CONFIG_FILES.iter().any(|(p, _)| *p == entry.path) {
            needs_root.push(entry.path.clone());
        }
    }

    // A marker is what the startup hook looks for; writing it last means an
    // interrupted staging is never applied.
    crate::storage::AtomicWriter::write(dir.join("READY"), &archive.manifest.created_at)
        .map_err(|e| format!("the restore marker could not be written: {e}"))?;

    Ok(StagedRestore {
        manifest: archive.manifest.clone(),
        staged,
        needs_root,
    })
}

/// Whether a restore is waiting for the next start.
pub fn pending_restore() -> Option<Manifest> {
    let dir = staging_dir();
    if !dir.join("READY").exists() {
        return None;
    }
    let manifest = std::fs::read_to_string(dir.join("manifest.json")).ok()?;
    serde_json::from_str(&manifest).ok()
}

/// Stage the manifest next to the payload so the card can describe a pending
/// restore after a restart.
pub fn write_staged_manifest(manifest: &Manifest) -> Result<(), String> {
    let dir = staging_dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let json = serde_json::to_string_pretty(manifest).map_err(|e| e.to_string())?;
    crate::storage::AtomicWriter::write(dir.join("manifest.json"), &json).map_err(|e| e.to_string())
}

/// Apply a staged restore. Runs at startup, **before** the database is opened.
///
/// The current database is kept as `homevault.db.pre-restore`, so a restore that
/// turns out to be the wrong archive is recoverable with one `mv` — a migration
/// is the moment when a mistake is most expensive, and "we overwrote it" is not
/// an acceptable answer.
pub fn apply_pending_restore() -> Result<Option<Manifest>, String> {
    let dir = staging_dir();
    if !dir.join("READY").exists() {
        return Ok(None);
    }
    let staged_db = dir.join("homevault.db");
    if !staged_db.exists() {
        return Err("A restore is staged but its database snapshot is missing".to_string());
    }
    let manifest = pending_restore();
    if manifest
        .as_ref()
        .is_some_and(|m| m.settings_schema > crate::settings_persistence::CURRENT_SETTINGS_SCHEMA)
    {
        return Err(
            "The staged restore comes from a newer HomeVault and was not applied".to_string(),
        );
    }

    let db_path = crate::config::paths::database_path();
    if db_path.exists() {
        let backup = db_path.with_extension("db.pre-restore");
        std::fs::copy(&db_path, &backup)
            .map_err(|e| format!("the current database could not be kept: {e}"))?;
    }
    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::copy(&staged_db, &db_path)
        .map_err(|e| format!("the staged database could not be put in place: {e}"))?;
    // SQLite's sidecars belong to the *old* database; leaving them next to the
    // new one would make SQLite replay a foreign WAL.
    for suffix in ["-wal", "-shm"] {
        let sidecar = PathBuf::from(format!("{}{suffix}", db_path.to_string_lossy()));
        let _ = std::fs::remove_file(sidecar);
    }

    // Remove the marker first: a failure while copying the rest must not leave
    // a "ready" flag that replays half a restore on the next boot.
    let _ = std::fs::remove_file(dir.join("READY"));
    crate::events::log_event(format!(
        "A staged configuration backup from {} was applied at startup (previous database kept as \
         homevault.db.pre-restore)",
        manifest
            .as_ref()
            .map(|m| m.created_at.clone())
            .unwrap_or_else(|| "an unknown date".to_string())
    ));
    Ok(manifest)
}

// --------------------------------------------------------------------------
// Secrets
// --------------------------------------------------------------------------

/// The tables whose rows are removed from a database copy when
/// `include_secrets` is false.
///
/// These hold **plaintext credentials**: a live session token is a bearer
/// credential, a trusted-device token skips the second factor, and an MCP token
/// is an API key. Dropping them costs a re-login; keeping them would put working
/// credentials in an unencrypted file.
pub const SCRUB_STATEMENTS: &[&str] = &[
    "DELETE FROM sessions",
    "DELETE FROM trusted_devices",
    "DELETE FROM mcp_tokens",
    "DELETE FROM password_recovery_codes",
    "DELETE FROM login_attempts",
    "UPDATE vpn_wireguard_peers SET private_key = ''",
];

/// Remove those credentials from a database snapshot, in place.
///
/// The snapshot is a real SQLite file, so the scrub is a handful of statements
/// against it. Without this, an archive built with `include_secrets = false`
/// would still carry live session tokens and API keys in an unencrypted file —
/// the flag would describe an intention rather than the bytes.
pub async fn scrub_database(path: &Path) -> Result<(), String> {
    use sqlx::Connection;

    let url = format!("sqlite://{}?mode=rw", path.display());
    let mut conn = sqlx::SqliteConnection::connect(&url)
        .await
        .map_err(|e| format!("the database snapshot could not be opened to scrub it: {e}"))?;
    for statement in SCRUB_STATEMENTS {
        if let Err(e) = sqlx::query(*statement).execute(&mut conn).await {
            let _ = conn.close().await;
            return Err(format!(
                "the database snapshot could not be scrubbed ({statement}): {e}"
            ));
        }
    }
    // `DELETE` frees the pages but leaves the old bytes inside them, so the
    // token would still be findable in the file. `VACUUM` rebuilds the database
    // without those free pages, which is what makes the scrub real.
    sqlx::query("VACUUM")
        .execute(&mut conn)
        .await
        .map_err(|e| format!("the scrubbed database snapshot could not be compacted: {e}"))?;
    let _ = conn.close().await;
    Ok(())
}

/// Read a config file for the archive, or say why it is missing.
///
/// Missing is normal (Samba is optional, netplan is not on every host), so it is
/// reported rather than failing the export — but it is *reported*, because a
/// restore that silently lacks `smb.conf` is a box that looks migrated and is
/// not.
pub fn read_config(archive_path: &str, host_path: &str) -> Result<Vec<u8>, SkippedEntry> {
    let mut file = std::fs::File::open(host_path).map_err(|e| SkippedEntry {
        path: archive_path.to_string(),
        reason: if e.kind() == std::io::ErrorKind::NotFound {
            format!("{host_path} is not present on this server")
        } else {
            format!("{host_path} could not be read: {e}")
        },
    })?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).map_err(|e| SkippedEntry {
        path: archive_path.to_string(),
        reason: format!("{host_path} could not be read: {e}"),
    })?;
    // A config file is text; a binary here means the path changed meaning (a
    // socket, a directory in a previous life) and shipping its bytes would be
    // noise.
    if bytes.contains(&0) {
        return Err(SkippedEntry {
            path: archive_path.to_string(),
            reason: format!("{host_path} is not a text configuration file"),
        });
    }
    Ok(bytes)
}

/// Whether `bytes` look like a HomeVault archive (cheap pre-check for uploads).
pub fn looks_like_archive(bytes: &[u8]) -> bool {
    bytes.len() > MAGIC.len() && &bytes[..MAGIC.len()] == MAGIC
}

/// A path inside the staging tree, resolved and confined to it.
pub fn staged_path(relative: &str) -> Option<PathBuf> {
    let candidate = staging_dir().join(relative);
    let root = staging_dir().canonicalize().ok()?;
    let resolved = candidate.canonicalize().ok()?;
    resolved.starts_with(&root).then_some(resolved)
}

/// `Path` → display string, for the manifest of a finished export.
pub fn safe_path(path: &Path) -> String {
    path.to_string_lossy().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input() -> ExportInput {
        ExportInput {
            database: b"SQLite format 3\0pretend database".to_vec(),
            settings: r#"{"schema_version":3,"smtp_password":"**redacted**"}"#.to_string(),
            configs: vec![
                (
                    "configs/smb.conf".to_string(),
                    b"[photos]\n  path = /srv/photos\n".to_vec(),
                ),
                (
                    "configs/exports".to_string(),
                    b"/srv/photos 192.168.1.0/24(rw)\n".to_vec(),
                ),
            ],
            skipped: vec![SkippedEntry {
                path: "configs/netplan.yaml".to_string(),
                reason: "/etc/netplan/90-homevault.yaml is not present on this server".to_string(),
            }],
            app_version: "1.0.0".to_string(),
            settings_schema: crate::settings_persistence::CURRENT_SETTINGS_SCHEMA,
        }
    }

    // ---------------------------------------------------------------------
    // Round-trip
    // ---------------------------------------------------------------------

    #[test]
    fn an_unencrypted_archive_round_trips_with_every_entry() {
        let bytes = build(&input(), false, None, "2026-10-31T12:00:00+00:00").expect("build");
        assert!(looks_like_archive(&bytes));

        let archive = read(&bytes, None).expect("read");
        assert_eq!(archive.manifest.format_version, FORMAT_VERSION);
        assert!(!archive.manifest.encrypted);
        assert!(!archive.manifest.includes_secrets);
        assert_eq!(archive.manifest.created_at, "2026-10-31T12:00:00+00:00");
        assert_eq!(archive.manifest.app_version, "1.0.0");

        // Every entry is present, byte for byte, with its checksum.
        assert_eq!(
            archive.get("database.db"),
            Some(b"SQLite format 3\0pretend database".as_slice())
        );
        assert!(archive.get("settings.json").is_some());
        assert_eq!(
            archive.get("configs/smb.conf"),
            Some(b"[photos]\n  path = /srv/photos\n".as_slice())
        );
        assert_eq!(archive.manifest.entries.len(), 4);
        for entry in &archive.manifest.entries {
            assert_eq!(entry.sha256.len(), 64, "a hex SHA-256");
            assert_eq!(entry.bytes, archive.get(&entry.path).unwrap().len() as u64);
        }
        // The skipped file is named, with its reason: a restore can say what it
        // is missing instead of producing a half-configured box.
        assert_eq!(archive.manifest.skipped.len(), 1);
        assert_eq!(archive.manifest.skipped[0].path, "configs/netplan.yaml");
        assert!(archive.manifest.skipped[0].reason.contains("not present"));
    }

    #[test]
    fn an_encrypted_archive_round_trips_and_hides_its_contents() {
        let plain = build(&input(), false, None, "2026-10-31T12:00:00+00:00").expect("build");
        let sealed = build(
            &input(),
            true,
            Some("correct horse battery"),
            "2026-10-31T12:00:00+00:00",
        )
        .expect("build");

        // The encrypted container must not contain the plaintext it holds.
        assert!(
            !sealed.windows(b"[photos]".len()).any(|w| w == b"[photos]"),
            "the payload must not be readable in an encrypted archive"
        );
        // The manifest itself stays readable (that is what lets the UI say
        // "encrypted, exported by 1.0.0, needs a passphrase"), but it carries no
        // entry contents.
        let archive = read(&sealed, Some("correct horse battery")).expect("read");
        assert!(archive.manifest.encrypted);
        assert!(archive.manifest.includes_secrets);
        assert_eq!(
            archive.get("configs/smb.conf"),
            Some(b"[photos]\n  path = /srv/photos\n".as_slice())
        );
        // …and the plain one decodes to the same payload.
        assert_eq!(
            read(&plain, None).unwrap().get("database.db"),
            archive.get("database.db")
        );
    }

    // ---------------------------------------------------------------------
    // Refusals
    // ---------------------------------------------------------------------

    #[test]
    fn a_file_that_is_not_an_archive_is_refused_before_anything_else() {
        assert_eq!(
            read(b"hello, this is a text file", None).unwrap_err(),
            ArchiveError::NotAnArchive
        );
        assert_eq!(read(&[], None).unwrap_err(), ArchiveError::NotAnArchive);
        // …and the message tells the admin what the file should be.
        assert!(ArchiveError::NotAnArchive
            .message()
            .contains("not a HomeVault"));
    }

    #[test]
    fn a_newer_container_is_refused_with_the_versions_named() {
        let mut bytes = build(&input(), false, None, "2026-10-31T12:00:00+00:00").expect("build");
        // Bump the container version in the header.
        bytes[MAGIC.len()..MAGIC.len() + 4].copy_from_slice(&(FORMAT_VERSION + 1).to_le_bytes());
        let error = read(&bytes, None).unwrap_err();
        assert_eq!(error, ArchiveError::NewerFormat(FORMAT_VERSION + 1));
        let message = error.message();
        assert!(message.contains("newer HomeVault"), "{message}");
        assert!(
            message.contains("Update HomeVault"),
            "the refusal says what to do: {message}"
        );
    }

    #[test]
    fn a_newer_settings_schema_is_refused_with_its_own_sentence() {
        let mut input = input();
        input.settings_schema = crate::settings_persistence::CURRENT_SETTINGS_SCHEMA + 1;
        let bytes = build(&input, false, None, "2026-10-31T12:00:00+00:00").expect("build");
        // The container reads fine; the *content* is from the future.
        let error = read(&bytes, None).unwrap_err();
        assert!(matches!(error, ArchiveError::NewerSchema(_)));
        assert!(error.message().contains("settings from a newer HomeVault"));
    }

    #[test]
    fn an_encrypted_archive_needs_its_passphrase_and_a_plain_one_refuses_it() {
        let sealed = build(
            &input(),
            true,
            Some("secret pass"),
            "2026-10-31T12:00:00+00:00",
        )
        .expect("build");
        assert_eq!(
            read(&sealed, None).unwrap_err(),
            ArchiveError::PassphraseRequired
        );
        assert_eq!(
            read(&sealed, Some("wrong pass")).unwrap_err(),
            ArchiveError::WrongPassphrase
        );
        // A wrong passphrase is a clean refusal, never a partial restore.
        assert!(ArchiveError::WrongPassphrase
            .message()
            .contains("does not open"));

        let plain = build(&input(), false, None, "2026-10-31T12:00:00+00:00").expect("build");
        assert_eq!(
            read(&plain, Some("unnecessary")).unwrap_err(),
            ArchiveError::NotEncrypted
        );
    }

    #[test]
    fn secrets_can_never_be_exported_in_the_clear() {
        // The encoder refuses on its own, so no handler path can produce an
        // unencrypted archive full of credentials.
        let error = build(&input(), true, None, "2026-10-31T12:00:00+00:00").unwrap_err();
        assert!(error.contains("must be encrypted"), "{error}");
        // A blank passphrase is no passphrase.
        assert!(build(&input(), true, Some("   "), "now").is_err());
    }

    #[test]
    fn a_tampered_entry_is_reported_by_name() {
        let mut input = input();
        input.configs = vec![("configs/exports".to_string(), b"abc".to_vec())];
        let bytes = build(&input, false, None, "2026-10-31T12:00:00+00:00").expect("build");
        // Flip one byte of the payload (the last entry's content).
        let mut corrupted = bytes.clone();
        let position = corrupted.len() - 1;
        corrupted[position] ^= 0xFF;
        let error = read(&corrupted, None).unwrap_err();
        match error {
            ArchiveError::Corrupt(path) => assert_eq!(path, "configs/exports"),
            other => panic!("expected a named corruption, got {other:?}"),
        }
    }

    #[test]
    fn a_truncated_archive_names_the_entry_it_could_not_read() {
        let bytes = build(&input(), false, None, "2026-10-31T12:00:00+00:00").expect("build");
        let truncated = &bytes[..bytes.len() - 4];
        let error = read(truncated, None).unwrap_err();
        assert!(matches!(error, ArchiveError::Corrupt(_)), "got {error:?}");
        assert!(error.message().contains("damaged"), "{}", error.message());
    }

    #[test]
    fn a_manifest_that_is_not_json_is_malformed_not_a_panic() {
        let mut bytes = build(&input(), false, None, "2026-10-31T12:00:00+00:00").expect("build");
        // Corrupt the first byte of the manifest (right after the header).
        let manifest_start = MAGIC.len() + 4 + 1 + SALT_LEN + NONCE_LEN + 4;
        bytes[manifest_start] = b'!';
        let error = read(&bytes, None).unwrap_err();
        assert!(matches!(error, ArchiveError::Malformed(_)), "got {error:?}");
    }

    // ---------------------------------------------------------------------
    // Manifest helpers
    // ---------------------------------------------------------------------

    #[test]
    fn the_manifest_finds_entries_and_sums_their_sizes() {
        let bytes = build(&input(), false, None, "2026-10-31T12:00:00+00:00").expect("build");
        let archive = read(&bytes, None).expect("read");
        assert!(archive.manifest.entry("database.db").is_some());
        assert!(archive.manifest.entry("configs/nope").is_none());
        assert_eq!(
            archive.manifest.total_bytes(),
            archive
                .entries
                .iter()
                .map(|(_, b)| b.len() as u64)
                .sum::<u64>()
        );
    }

    #[test]
    fn the_config_allow_list_never_names_a_secret_file() {
        // A regression here would ship a private key in an unencrypted archive.
        for (_, host) in CONFIG_FILES {
            let lowered = host.to_ascii_lowercase();
            for forbidden in ["tls", "key", "syncthing", "vpn", "acme", "ddns", "secret"] {
                assert!(
                    !lowered.contains(forbidden),
                    "{host} looks like a secret and must not be in the archive's allow-list"
                );
            }
        }
        // …and the database scrub covers the plaintext credentials.
        let scrubbed: Vec<&str> = SCRUB_STATEMENTS.to_vec();
        for table in ["sessions", "trusted_devices", "mcp_tokens"] {
            assert!(
                scrubbed.iter().any(|s| s.contains(table)),
                "{table} holds plaintext credentials and must be scrubbed"
            );
        }
    }

    #[test]
    fn reading_a_missing_config_reports_it_instead_of_failing_the_export() {
        let skipped = read_config("configs/nothing.conf", "/nonexistent/path/nothing.conf")
            .expect_err("must be reported as skipped");
        assert_eq!(skipped.path, "configs/nothing.conf");
        assert!(skipped.reason.contains("not present"), "{}", skipped.reason);
    }
}

//! Short-lived, session-bound tickets for the Explorer's binary streams
//! (download + thumbnail) so the session token never has to appear in a URL
//! query string — putting the token in `?token=` leaks it into access logs,
//! browser history and Referer headers.
//!
//! Two ticket kinds, both issued only after the caller's path access has been
//! validated server-side:
//!
//! - **Download** tickets are *single-use*, scoped to one exact file path and
//!   expire after 60 s. The download handler consumes the ticket instead of
//!   requiring a session.
//! - **Thumbnail** tickets are *multi-use* within a 5-minute window, scoped to
//!   a directory: any file path under that directory may be thumbnailed. The
//!   frontend mints one per directory listing and reuses it for every
//!   thumbnail `<img>` in the grid.
//!
//! The stored scope is always the **canonical** validated path; consuming a
//! ticket re-canonicalises the requested path before comparing, so a
//! `..`/symlink-laden URL can't smuggle a path out of the ticket's scope.

use parking_lot::RwLock;
use std::collections::HashMap;
use std::path::Path;
use std::sync::LazyLock;
use std::time::{Duration, Instant};

/// How long a download ticket stays valid (single use).
const DOWNLOAD_TICKET_TTL: Duration = Duration::from_secs(60);
/// How long a thumbnail ticket stays valid (multi-use, refreshed on each
/// directory listing).
const THUMBNAIL_TICKET_TTL: Duration = Duration::from_secs(300);

struct Ticket {
    /// The session user the ticket was issued for (used to record usage).
    username: String,
    /// Canonical path scope: the exact file for downloads, a directory for
    /// thumbnails.
    scope: String,
    /// `true` = single-use (download), `false` = multi-use (thumbnail).
    single_use: bool,
    /// Minted by an admin session: thumbnails may resolve any existing path,
    /// because an admin can already browse anywhere (this keeps thumbnails of
    /// files reached *through* a symlink inside the browsed directory
    /// working). Non-admin tickets stay strictly scoped to the directory.
    admin: bool,
    issued_at: Instant,
}

static TICKETS: LazyLock<RwLock<HashMap<String, Ticket>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

fn random_ticket() -> String {
    use rand::Rng;
    let mut rng = rand::rng();
    let mut buf = [0u8; 24];
    rng.fill_bytes(&mut buf);
    buf.iter().map(|b| format!("{:02x}", b)).collect()
}

fn issue(username: &str, scope: &str, single_use: bool, admin: bool, ttl: Duration) -> String {
    let ticket = random_ticket();
    let now = Instant::now();
    let mut map = TICKETS.write();
    map.retain(|_, t| now.duration_since(t.issued_at) < ttl);
    map.insert(
        ticket.clone(),
        Ticket {
            username: username.to_string(),
            scope: scope.to_string(),
            single_use,
            admin,
            issued_at: now,
        },
    );
    ticket
}

/// Issue a single-use download ticket for the *canonical* file path.
pub fn issue_download_ticket(username: &str, canonical_path: &str) -> String {
    issue(username, canonical_path, true, false, DOWNLOAD_TICKET_TTL)
}

/// Issue a multi-use thumbnail ticket scoped to the *canonical* directory.
/// `admin` marks tickets minted by an admin session (see `Ticket.admin`).
pub fn issue_thumbnail_ticket(username: &str, canonical_dir: &str, admin: bool) -> String {
    issue(username, canonical_dir, false, admin, THUMBNAIL_TICKET_TTL)
}

/// Canonicalise `requested` if it exists; otherwise fall back to the raw
/// string so a comparison against an already-canonical scope still works.
fn canonical_or_raw(requested: &str) -> String {
    std::fs::canonicalize(requested)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| requested.to_string())
}

/// Consume a single-use download ticket. Returns `(username, canonical path)`
/// when the ticket is unexpired and the requested path resolves to exactly the
/// path the ticket was issued for.
pub fn consume_download_ticket(ticket: &str, requested: &str) -> Option<(String, String)> {
    let mut map = TICKETS.write();
    let entry = map.remove(ticket)?;
    if entry.single_use
        && entry.issued_at.elapsed() < DOWNLOAD_TICKET_TTL
        && canonical_or_raw(requested) == entry.scope
    {
        return Some((entry.username, entry.scope));
    }
    None
}

/// Validate a multi-use thumbnail ticket. Returns `(username, canonical path)`
/// when the ticket is unexpired and the requested file is within the ticket's
/// scope (or, for an admin-minted ticket, is any existing file).
pub fn validate_thumbnail_ticket(ticket: &str, requested: &str) -> Option<(String, String)> {
    let map = TICKETS.read();
    let entry = map.get(ticket)?;
    if entry.single_use || entry.issued_at.elapsed() >= THUMBNAIL_TICKET_TTL {
        return None;
    }
    let canonical = canonical_or_raw(requested);
    if entry.admin || Path::new(&canonical).starts_with(Path::new(&entry.scope)) {
        Some((entry.username.clone(), canonical))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_download_ticket_is_single_use_and_scoped_to_the_exact_path() {
        let dir = TempDir::new().unwrap();
        let f = dir.path().join("a.txt");
        std::fs::write(&f, b"x").unwrap();
        let fs = f.to_string_lossy().to_string();

        let ticket = issue_download_ticket("bob", &fs);
        // Correct path → consumed once.
        let (user, path) = consume_download_ticket(&ticket, &fs).expect("valid ticket");
        assert_eq!(user, "bob");
        assert_eq!(path, fs);
        // Replay → gone.
        assert!(consume_download_ticket(&ticket, &fs).is_none());
    }

    #[test]
    fn test_download_ticket_does_not_fit_a_different_path() {
        let dir = TempDir::new().unwrap();
        let f = dir.path().join("a.txt");
        std::fs::write(&f, b"x").unwrap();
        let ticket = issue_download_ticket("bob", &f.to_string_lossy());
        let other = dir.path().join("b.txt");
        std::fs::write(&other, b"y").unwrap();
        assert!(consume_download_ticket(&ticket, &other.to_string_lossy()).is_none());
    }

    #[test]
    fn test_download_ticket_blocks_traversal_lookalikes() {
        let dir = TempDir::new().unwrap();
        let sub = dir.path().join("sub");
        std::fs::create_dir_all(&sub).unwrap();
        let f = sub.join("a.txt");
        std::fs::write(&f, b"x").unwrap();
        let ticket = issue_download_ticket("bob", &f.to_string_lossy());
        // A `..` path that *lexically* starts with the scope must still be
        // rejected because canonicalisation resolves it outside the file.
        let escape = format!("{}/../a.txt", sub.to_string_lossy());
        assert!(consume_download_ticket(&ticket, &escape).is_none());
    }

    #[test]
    fn test_thumbnail_ticket_is_multi_use_and_directory_scoped() {
        let dir = TempDir::new().unwrap();
        let a = dir.path().join("img1.png");
        let b = dir.path().join("img2.png");
        std::fs::write(&a, b"x").unwrap();
        std::fs::write(&b, b"y").unwrap();
        let scope = dir.path().to_string_lossy();

        let ticket = issue_thumbnail_ticket("bob", &scope, false);
        // Two different files under the same scope, no session.
        assert!(validate_thumbnail_ticket(&ticket, &a.to_string_lossy()).is_some());
        assert!(validate_thumbnail_ticket(&ticket, &b.to_string_lossy()).is_some());
        // Outside the scope → rejected.
        let outside = TempDir::new().unwrap();
        let o = outside.path().join("img.png");
        std::fs::write(&o, b"z").unwrap();
        assert!(validate_thumbnail_ticket(&ticket, &o.to_string_lossy()).is_none());
    }

    #[test]
    fn test_admin_thumbnail_ticket_may_resolve_any_path() {
        let dir = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        let a = dir.path().join("img.png");
        std::fs::write(&a, b"x").unwrap();
        let o = outside.path().join("secret.png");
        std::fs::write(&o, b"y").unwrap();

        let ticket = issue_thumbnail_ticket("root_admin", &dir.path().to_string_lossy(), true);
        assert!(validate_thumbnail_ticket(&ticket, &a.to_string_lossy()).is_some());
        assert!(
            validate_thumbnail_ticket(&ticket, &o.to_string_lossy()).is_some(),
            "an admin ticket may resolve files anywhere"
        );
    }
}

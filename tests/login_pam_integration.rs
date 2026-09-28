//! Opt-in, host-mutating integration tests for the parts of the auth stack
//! that can only be verified against a real Linux system:
//!
//!   * PAM login for a real (throwaway) Linux account — the fallback path in
//!     `AuthService::authenticate` for users that are not in the HomeVault DB.
//!   * Privileged-group → admin promotion in `AuthService::create_session`.
//!   * `SetupService::configure_sudoers` producing a `visudo`-valid file.
//!
//! Every test here is `#[ignore]` — they need root, `useradd`/`userdel`/
//! `chpasswd`, and the `PAM` Python module, none of which the normal
//! `cargo test` environment has. Run them explicitly on a disposable machine
//! or CI container:
//!
//!   sudo -E cargo test --test login_pam_integration -- --ignored --nocapture
//!
//! The sudoers test additionally requires `HOMEVAULT_DESTRUCTIVE_ROOT_TESTS=1`
//! because it rewrites `/etc/sudoers.d/homevault` (it backs up and restores any
//! pre-existing file, but you still don't want that on a real box).
//!
//! ---------------------------------------------------------------------------
//! These tests found the "log in with your host Linux account" path broken in
//! THREE independent ways (found + fixed 2026-09-01, all three verified):
//!
//!  1. `authenticate_with_pam` ran `ok = auth.authenticate();
//!     sys.exit(0 if ok else 1)`. PyPAM returns None on success (raises on
//!     failure), so `ok` was always falsy → every host login rejected.
//!     FIX (`src/auth/service.rs`): treat "no exception raised" as success.
//!
//!  2. `create_session` for a host account failed with
//!     `FOREIGN KEY constraint failed` — `sessions.username` references
//!     `users.username` and a PAM user has no `users` row.
//!     FIX (`src/auth/service.rs`): `ensure_pam_user_row` upserts a minimal
//!     `users` row (sentinel hash, `role_id` synced to host group membership)
//!     on every PAM login, before `create_session` runs.
//!
//!  3. The systemd unit runs the service as the unprivileged `homevault` user;
//!     `pam_unix` -> `unix_chkpwd` then refuses to verify *another* user's
//!     password unless the caller can read /etc/shadow.
//!     FIX (`installer/install.sh`): add `homevault` to the `shadow` group
//!     (verified: an unprivileged account in `shadow` authenticates via PyPAM).
//! ---------------------------------------------------------------------------

use std::process::Command;

use homevault::auth::AuthService;
use homevault::storage::Database;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn is_root() -> bool {
    Command::new("id")
        .arg("-u")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim() == "0")
        .unwrap_or(false)
}

fn python_pam_available() -> bool {
    Command::new("python3")
        .args(["-c", "import PAM"])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Create a throwaway shell-less local account with a known password.
/// Returns `false` (test should bail) if the environment can't do it.
fn create_linux_user(name: &str, password: &str) -> bool {
    let _ = Command::new("userdel").args(["-r", name]).status(); // clean slate

    let added = Command::new("useradd")
        .args(["-m", "-s", "/usr/sbin/nologin", name])
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !added {
        return false;
    }

    let mut child = match Command::new("chpasswd")
        .stdin(std::process::Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(_) => return false,
    };
    {
        use std::io::Write;
        // `take()` so the pipe is closed (EOF) when this block ends —
        // otherwise `chpasswd` blocks on stdin and `wait()` deadlocks.
        let mut stdin = child.stdin.take().unwrap();
        let _ = writeln!(stdin, "{}:{}", name, password);
    }
    child.wait().map(|s| s.success()).unwrap_or(false)
}

fn delete_linux_user(name: &str) {
    let _ = Command::new("userdel").args(["-r", name]).status();
}

async fn in_memory_auth() -> AuthService {
    let db = Database::new("sqlite::memory:").await.expect("db");
    db.run_migrations().await.expect("migrations");
    AuthService::new(db)
}

// ---------------------------------------------------------------------------
// PAM login
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "needs root + PAM + useradd"]
async fn pam_login_succeeds_for_a_real_linux_account() {
    if !is_root() || !python_pam_available() {
        eprintln!("skipping: not root or PAM module missing");
        return;
    }
    let user = "hv_pam_ok";
    let pass = "Hv-Pam-Pass-123";
    if !create_linux_user(user, pass) {
        eprintln!("skipping: could not create the test account");
        return;
    }

    let auth = in_memory_auth().await;

    // Correct password authenticates via the PAM fallback (user is not in the
    // DB). Regression guard for fix #1 (the PyPAM exit-code inversion).
    let ok = auth.authenticate(user, pass, "127.0.0.1", None).await;
    assert!(matches!(ok, Ok(true)), "PAM auth should succeed, got {ok:?}");

    // Wrong password fails.
    let bad = auth.authenticate(user, "totally-wrong", "127.0.0.1", None).await;
    assert!(bad.is_err(), "wrong password must fail, got {bad:?}");

    delete_linux_user(user);
}

#[tokio::test]
#[ignore = "needs root + PAM + useradd"]
async fn pam_user_in_sudo_group_is_promoted_to_admin() {
    if !is_root() || !python_pam_available() {
        eprintln!("skipping: not root or PAM module missing");
        return;
    }
    let user = "hv_pam_admin";
    let pass = "Hv-Pam-Pass-123";
    if !create_linux_user(user, pass) {
        eprintln!("skipping: could not create the test account");
        return;
    }
    // Put the account in a privileged group (`create_session` checks
    // sudo/admin/wheel/homevault_admin).
    let grp = if Command::new("getent").args(["group", "sudo"]).status().map(|s| s.success()).unwrap_or(false) {
        "sudo"
    } else {
        "wheel"
    };
    let _ = Command::new("usermod").args(["-aG", grp, user]).status();

    let auth = in_memory_auth().await;
    // `authenticate` upserts the mirrored `users` row (fix #2) with role_id
    // derived from host group membership.
    assert!(matches!(auth.authenticate(user, pass, "127.0.0.1", None).await, Ok(true)));

    let session = auth
        .create_session(user.to_string(), "127.0.0.1".to_string(), false)
        .await
        .expect("create_session should persist a session for a host account");
    assert!(
        session.is_admin,
        "a host account in `{grp}` should get an admin session"
    );

    // A second login still works (sentinel-hash fall-through to PAM).
    assert!(matches!(auth.authenticate(user, pass, "127.0.0.1", None).await, Ok(true)));

    delete_linux_user(user);
}

// ---------------------------------------------------------------------------
// sudoers
// ---------------------------------------------------------------------------

#[test]
#[ignore = "rewrites /etc/sudoers.d/homevault — needs root + HOMEVAULT_DESTRUCTIVE_ROOT_TESTS=1"]
fn configure_sudoers_produces_a_visudo_valid_file() {
    if !is_root() || std::env::var("HOMEVAULT_DESTRUCTIVE_ROOT_TESTS").as_deref() != Ok("1") {
        eprintln!("skipping: not root, or HOMEVAULT_DESTRUCTIVE_ROOT_TESTS != 1");
        return;
    }

    let path = std::path::Path::new("/etc/sudoers.d/homevault");
    let backup = std::fs::read(path).ok();

    let result = homevault::setup::SetupService::configure_sudoers();
    assert!(result.is_ok(), "configure_sudoers failed: {result:?}");
    assert!(path.exists(), "sudoers drop-in was not written");

    let check = Command::new("visudo")
        .args(["-cf", "/etc/sudoers.d/homevault"])
        .status()
        .expect("run visudo");
    assert!(check.success(), "generated sudoers file is not valid");

    // Restore whatever was there before (or remove our file).
    match backup {
        Some(bytes) => std::fs::write(path, bytes).unwrap(),
        None => {
            let _ = std::fs::remove_file(path);
        }
    }
}

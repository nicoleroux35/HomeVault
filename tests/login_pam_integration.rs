/* =============================================================================
 * File          : login_pam_integration.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 10/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Host (PAM) account login tests: mirrored users rows, admin-group derivation and lockout interplay.
 *
 * Dependencies:
 *   - none (test crate: axum router + tower oneshot / Playwright)
 *
 * Usage:
 *   Rust: cargo test  |  E2E: npx playwright test
 * =============================================================================
 */

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
    assert!(
        matches!(ok, Ok(true)),
        "PAM auth should succeed, got {ok:?}"
    );

    // Wrong password fails.
    let bad = auth
        .authenticate(user, "totally-wrong", "127.0.0.1", None)
        .await;
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
    let grp = if Command::new("getent")
        .args(["group", "sudo"])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
    {
        "sudo"
    } else {
        "wheel"
    };
    let _ = Command::new("usermod").args(["-aG", grp, user]).status();

    let auth = in_memory_auth().await;
    // `authenticate` upserts the mirrored `users` row (fix #2) with role_id
    // derived from host group membership.
    assert!(matches!(
        auth.authenticate(user, pass, "127.0.0.1", None).await,
        Ok(true)
    ));

    let session = auth
        .create_session(user.to_string(), "127.0.0.1".to_string(), false)
        .await
        .expect("create_session should persist a session for a host account");
    assert!(
        session.is_admin,
        "a host account in `{grp}` should get an admin session"
    );

    // A second login still works (sentinel-hash fall-through to PAM).
    assert!(matches!(
        auth.authenticate(user, pass, "127.0.0.1", None).await,
        Ok(true)
    ));

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

// ---------------------------------------------------------------------------
// Host account lifecycle (2026-10-21)
//
// `HostAccountService` is what makes a HomeVault role change / password change
// / lock / delete actually reach the Linux account. None of it can run in the
// normal test suite (every command needs root), so the real effects are pinned
// here, on a disposable machine:
//
//   sudo -E cargo test --test login_pam_integration -- --ignored --nocapture
// ---------------------------------------------------------------------------

use homevault::host_accounts::{HostAccountService, HOST_ADMIN_GROUPS};

/// The account's supplementary groups, as `id -nG` reports them.
fn host_groups(name: &str) -> Vec<String> {
    Command::new("id")
        .args(["-nG", name])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .split_whitespace()
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// `passwd -S` status letter: `L` locked, `P` usable password.
fn password_status(name: &str) -> String {
    Command::new("passwd")
        .args(["-S", name])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .split_whitespace()
                .nth(1)
                .unwrap_or("")
                .to_string()
        })
        .unwrap_or_default()
}

#[tokio::test]
#[ignore = "needs root + useradd/usermod/gpasswd"]
async fn grant_admin_provisions_the_account_and_every_admin_group() {
    if !is_root() {
        eprintln!("skipping: not root");
        return;
    }
    // The reserved group is created at startup by `ensure_admin_group`; make
    // sure it is there for this standalone test.
    let _ = Command::new("groupadd").arg("homevault_admin").status();

    let user = "hv_acct_grant";
    let _ = Command::new("userdel").args(["-r", user]).status();

    HostAccountService::grant_admin(user)
        .await
        .expect("grant_admin should provision the account");

    assert!(
        Command::new("id")
            .arg(user)
            .output()
            .unwrap()
            .status
            .success(),
        "the Linux account must exist"
    );
    let groups = host_groups(user);
    assert!(
        groups.iter().any(|g| g == "homevault_admin"),
        "expected homevault_admin, got {groups:?}"
    );
    for group in HOST_ADMIN_GROUPS {
        let exists = Command::new("getent")
            .args(["group", group])
            .output()
            .unwrap()
            .status
            .success();
        if exists {
            assert!(
                groups.iter().any(|g| g == group),
                "expected membership in '{}', got {groups:?}",
                group
            );
        }
    }

    delete_linux_user(user);
}

#[tokio::test]
#[ignore = "needs root + useradd/usermod/gpasswd"]
async fn revoke_admin_clears_every_admin_group() {
    if !is_root() {
        eprintln!("skipping: not root");
        return;
    }
    let _ = Command::new("groupadd").arg("homevault_admin").status();
    let user = "hv_acct_revoke";
    if !create_linux_user(user, "Hv-Pam-Pass-123") {
        eprintln!("skipping: could not create the test account");
        return;
    }
    for group in HOST_ADMIN_GROUPS {
        let _ = Command::new("usermod").args(["-aG", group, user]).status();
    }

    HostAccountService::revoke_admin(user)
        .await
        .expect("revoke_admin");

    let groups = host_groups(user);
    for group in HOST_ADMIN_GROUPS {
        assert!(
            !groups.iter().any(|g| g == group),
            "'{}' must be revoked, still in {groups:?}",
            group
        );
    }

    delete_linux_user(user);
}

#[tokio::test]
#[ignore = "needs root + PAM + useradd/chpasswd"]
async fn set_password_rotates_the_host_credential() {
    if !is_root() || !python_pam_available() {
        eprintln!("skipping: not root or PAM module missing");
        return;
    }
    let user = "hv_acct_passwd";
    let old_pass = "Hv-Pam-Pass-123";
    let new_pass = "Hv-Pam-Pass-456";
    if !create_linux_user(user, old_pass) {
        eprintln!("skipping: could not create the test account");
        return;
    }

    HostAccountService::set_password(user, new_pass)
        .await
        .expect("set_password");

    let auth = in_memory_auth().await;
    assert!(
        matches!(
            auth.authenticate(user, new_pass, "127.0.0.1", None).await,
            Ok(true)
        ),
        "the new host password must work"
    );
    assert!(
        auth.authenticate(user, old_pass, "127.0.0.1", None)
            .await
            .is_err(),
        "the old host password must stop working"
    );

    delete_linux_user(user);
}

#[tokio::test]
#[ignore = "needs root + useradd/usermod"]
async fn set_locked_locks_and_unlocks_the_host_credential() {
    if !is_root() {
        eprintln!("skipping: not root");
        return;
    }
    let user = "hv_acct_lock";
    if !create_linux_user(user, "Hv-Pam-Pass-123") {
        eprintln!("skipping: could not create the test account");
        return;
    }
    assert_eq!(password_status(user), "P", "precondition: usable password");

    HostAccountService::set_locked(user, true)
        .await
        .expect("lock");
    assert_eq!(
        password_status(user),
        "L",
        "the host password must be locked"
    );

    HostAccountService::set_locked(user, false)
        .await
        .expect("unlock");
    assert_eq!(
        password_status(user),
        "P",
        "the host password must be usable again"
    );

    delete_linux_user(user);
}

#[tokio::test]
#[ignore = "needs root + useradd/userdel"]
async fn delete_account_removes_the_linux_account_but_keeps_its_home() {
    if !is_root() {
        eprintln!("skipping: not root");
        return;
    }
    let user = "hv_acct_delete";
    if !create_linux_user(user, "Hv-Pam-Pass-123") {
        eprintln!("skipping: could not create the test account");
        return;
    }
    let home = std::path::Path::new("/home").join(user);
    assert!(home.exists(), "precondition: useradd -m created a home");

    HostAccountService::delete_account(user)
        .await
        .expect("delete_account");

    assert!(
        !Command::new("id")
            .arg(user)
            .output()
            .unwrap()
            .status
            .success(),
        "the Linux account must be gone"
    );
    // Home removal is the caller's explicit `delete_home_folder` choice, so
    // the account deletion itself must leave the files alone.
    assert!(home.exists(), "the home directory must be kept");

    let _ = Command::new("rm")
        .args(["-rf", home.to_str().unwrap()])
        .status();
}

#[tokio::test]
#[ignore = "needs root + useradd"]
async fn delete_account_refuses_a_system_account() {
    if !is_root() {
        eprintln!("skipping: not root");
        return;
    }
    // `nobody` exists everywhere and must never be deletable from the app.
    let err = HostAccountService::delete_account("nobody")
        .await
        .expect_err("deleting a system account must be refused");
    assert!(matches!(
        err,
        homevault::host_accounts::HostAccountError::SystemAccount(_)
    ));
}

/* =============================================================================
 * File          : snapshot_quiescence.rs
 * Author        : Nicolas Le Roux
 * Created       : 05/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Application-consistency policy for snapshots (tasklist P2-13). A snapshot
 *   is crash-consistent unless a working pre/post hook (PostgreSQL,
 *   SQLite, Incus, fs-freeze) was actually executed. The rule is encoded here
 *   so a missing hook can never be advertised as "application-consistent".
 *
 * Dependencies:
 *   - none
 *
 * Usage:
 *   See docs/guides/snapshots.md.
 * =============================================================================
 */

//! Crash- vs application-consistent snapshot policy.

use std::time::Duration;

use tokio::process::Command;

use crate::utils::process::run_timed_async_result;

/// A hook may block the snapshot for at most a minute; a hung database must
/// not hold the scheduler forever.
pub const HOOK_TIMEOUT: Duration = Duration::from_secs(60);

/// What a snapshot actually promises.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Consistency {
    /// The filesystem was snapped live; open applications may need recovery.
    CrashConsistent,
    /// Every configured hook ran successfully; the app was quiesced.
    ApplicationConsistent,
}

impl Consistency {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::CrashConsistent => "crash-consistent",
            Self::ApplicationConsistent => "application-consistent",
        }
    }
}

/// One application hook: an argv run before the snapshot and one after.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuiesceHook {
    pub name: String,
    pub pre: Vec<String>,
    pub post: Vec<String>,
}

/// The decision taken before a snapshot is created.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuiescePlan {
    pub consistency: Consistency,
    /// Hooks that will run, in order.
    pub hooks: Vec<QuiesceHook>,
    /// Why the snapshot is only crash-consistent, when it is.
    pub reason: Option<String>,
}

/// Build the plan. `requested` is the admin's choice; `hooks` is the list
/// whose executables are present. A hook with an empty `pre` is never run.
pub fn plan(requested: bool, available_hooks: Vec<QuiesceHook>, missing: &[String]) -> QuiescePlan {
    if !requested {
        return QuiescePlan {
            consistency: Consistency::CrashConsistent,
            hooks: Vec::new(),
            reason: Some("application consistency was not requested".to_string()),
        };
    }
    let hooks: Vec<QuiesceHook> = available_hooks
        .into_iter()
        .filter(|h| !h.pre.is_empty())
        .collect();
    if hooks.is_empty() {
        return QuiescePlan {
            consistency: Consistency::CrashConsistent,
            hooks,
            reason: Some(format!(
                "no application hook is configured (missing: {})",
                if missing.is_empty() {
                    "none declared".to_string()
                } else {
                    missing.join(", ")
                }
            )),
        };
    }
    QuiescePlan {
        consistency: Consistency::ApplicationConsistent,
        hooks,
        reason: None,
    }
}

/// If a hook fails, the snapshot must not be described as application-
/// consistent: it falls back, with the failure as the reason.
pub fn after_hook_failure(plan: &QuiescePlan, error: &str) -> QuiescePlan {
    QuiescePlan {
        consistency: Consistency::CrashConsistent,
        hooks: plan.hooks.clone(),
        reason: Some(format!("a quiesce hook failed: {error}")),
    }
}

/// Validate one configured hook. Commands are argv vectors, never a shell;
/// the executable must be absolute so a changed `PATH` cannot redirect it.
pub fn validate_hook(name: &str, pre: &[String], post: &[String]) -> Result<(), String> {
    let name = name.trim();
    if name.is_empty() || name.len() > 64 || name.chars().any(char::is_control) {
        return Err("the hook name must be 1-64 characters without control characters".to_string());
    }
    if pre.is_empty() {
        return Err("a pre-snapshot command is required".to_string());
    }
    validate_argv(pre, "pre")?;
    if !post.is_empty() {
        validate_argv(post, "post")?;
    }
    Ok(())
}

fn validate_argv(argv: &[String], which: &str) -> Result<(), String> {
    if argv.len() > 32 {
        return Err(format!("the {which} command has more than 32 arguments"));
    }
    if argv.iter().any(|arg| arg.len() > 256 || arg.contains('\0')) {
        return Err(format!(
            "each {which} argument must be at most 256 characters and contain no NUL"
        ));
    }
    if !argv[0].starts_with('/') {
        return Err(format!("the {which} executable must be an absolute path"));
    }
    Ok(())
}

/// Run the pre hooks in order. Returns the hooks that were attempted (so the
/// post pass can undo them) and the first error, if any. A failure downgrades
/// the snapshot to crash-consistent but does not abort it.
pub async fn run_pre_hooks(hooks: &[QuiesceHook]) -> (Vec<QuiesceHook>, Option<String>) {
    let mut attempted = Vec::new();
    for hook in hooks {
        attempted.push(hook.clone());
        if let Err(e) = run_argv(&hook.pre).await {
            return (attempted, Some(format!("{}: {e}", hook.name)));
        }
    }
    (attempted, None)
}

/// Run the post hooks in reverse order. Every failure is returned; the
/// snapshot has already been taken, so nothing is rolled back here.
pub async fn run_post_hooks(hooks: &[QuiesceHook]) -> Vec<String> {
    let mut errors = Vec::new();
    for hook in hooks.iter().rev() {
        if hook.post.is_empty() {
            continue;
        }
        if let Err(e) = run_argv(&hook.post).await {
            errors.push(format!("{}: {e}", hook.name));
        }
    }
    errors
}

async fn run_argv(argv: &[String]) -> Result<(), String> {
    let mut cmd = Command::new(&argv[0]);
    cmd.args(&argv[1..]);
    let output = run_timed_async_result(&mut cmd, HOOK_TIMEOUT)
        .await
        .map_err(|e| format!("could not run {}: {e}", argv[0]))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "{} exited with {}: {}",
            argv[0],
            output.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hook(name: &str) -> QuiesceHook {
        QuiesceHook {
            name: name.to_string(),
            pre: vec!["true".to_string()],
            post: vec!["true".to_string()],
        }
    }

    #[test]
    fn the_default_is_crash_consistent() {
        let decision = plan(false, vec![hook("postgres")], &[]);
        assert_eq!(decision.consistency, Consistency::CrashConsistent);
        assert!(decision.hooks.is_empty());
        assert!(decision.reason.is_some());
    }

    #[test]
    fn a_requested_hook_makes_it_application_consistent() {
        let decision = plan(true, vec![hook("postgres")], &[]);
        assert_eq!(decision.consistency, Consistency::ApplicationConsistent);
        assert_eq!(decision.hooks.len(), 1);
        assert!(decision.reason.is_none());
    }

    #[test]
    fn a_missing_or_empty_hook_never_claims_application_consistency() {
        let decision = plan(true, Vec::new(), &["postgres".to_string()]);
        assert_eq!(decision.consistency, Consistency::CrashConsistent);
        assert!(decision.reason.unwrap().contains("postgres"));

        let empty = plan(
            true,
            vec![QuiesceHook {
                name: "noop".to_string(),
                pre: Vec::new(),
                post: Vec::new(),
            }],
            &[],
        );
        assert_eq!(empty.consistency, Consistency::CrashConsistent);
    }

    #[test]
    fn a_failed_hook_downgrades_the_promise() {
        let decision = plan(true, vec![hook("postgres")], &[]);
        let downgraded = after_hook_failure(&decision, "pg_start_backup failed");
        assert_eq!(downgraded.consistency, Consistency::CrashConsistent);
        assert!(downgraded.reason.unwrap().contains("pg_start_backup"));
    }

    #[test]
    fn hooks_are_argv_and_absolute() {
        let pre = vec!["/usr/bin/true".to_string()];
        let post = vec!["/usr/bin/true".to_string(), "--undo".to_string()];
        assert!(validate_hook("postgres", &pre, &post).is_ok());
        assert!(validate_hook("", &pre, &post).is_err());
        assert!(validate_hook("x", &[], &post).is_err());
        assert!(validate_hook("x", &["true".to_string()], &[]).is_err());
        assert!(
            validate_hook("x", &["/bin/true".to_string()], &["/bin/false".to_string()]).is_ok()
        );
        let too_many = vec!["/bin/true".to_string(); 33];
        assert!(validate_hook("x", &too_many, &[]).is_err());
    }

    #[test]
    fn wire_names_are_stable() {
        assert_eq!(Consistency::CrashConsistent.as_str(), "crash-consistent");
        assert_eq!(
            Consistency::ApplicationConsistent.as_str(),
            "application-consistent"
        );
    }
}

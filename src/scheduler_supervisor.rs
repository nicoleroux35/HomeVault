/* =============================================================================
 * File          : scheduler_supervisor.rs
 * Author        : Nicolas Le Roux
 * Created       : 09/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Supervises the long-running background loops (FIAB-02).
 *
 *   `tokio::spawn` detaches a task: if the future panics or returns, the
 *   scheduler is simply gone while the process keeps serving HTTP. The
 *   systemd watchdog only sees a *global* freeze; it cannot notice one dead
 *   loop. `spawn` wraps a loop factory, records the outcome, waits with an
 *   exponential backoff and starts it again. The registry is exposed by
 *   `GET /api/health/schedulers` (admin) and shown in the Health screen.
 *
 *   One-shot boot tasks (static-route re-apply, firewall rules, recovery)
 *   deliberately stay on `tokio::spawn`: re-running them after a "return"
 *   would repeat a side effect rather than restore a service.
 *
 * Dependencies:
 *   - tokio, chrono, serde
 *
 * Usage:
 *   `scheduler_supervisor::spawn("restic", || restic_scheduler::run_loop(...));`
 * =============================================================================
 */

use std::collections::BTreeMap;
use std::future::Future;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use serde::Serialize;

/// First delay before restarting a loop that stopped.
pub const MIN_BACKOFF: Duration = Duration::from_secs(5);
/// Longest delay: a genuinely broken loop must not fill the journal.
pub const MAX_BACKOFF: Duration = Duration::from_secs(300);
/// A loop that ran at least this long is considered healthy; the next stop
/// starts from `MIN_BACKOFF` instead of the last backoff.
pub const STABLE_AFTER: Duration = Duration::from_secs(60);

/// What the Health screen and the API report for one loop.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct SchedulerState {
    pub name: String,
    /// True while the loop future is executing.
    pub running: bool,
    /// Number of times the supervisor had to start it again.
    pub restarts: u64,
    pub last_started: Option<String>,
    /// Why it stopped: `returned`, `panicked` or `cancelled`.
    pub last_error: Option<String>,
}

impl SchedulerState {
    fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            running: false,
            restarts: 0,
            last_started: None,
            last_error: None,
        }
    }
}

fn registry() -> &'static Mutex<BTreeMap<&'static str, SchedulerState>> {
    static REGISTRY: OnceLock<Mutex<BTreeMap<&'static str, SchedulerState>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn with_state<T>(name: &'static str, update: impl FnOnce(&mut SchedulerState) -> T) -> T {
    let mut registry = registry()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let state = registry
        .entry(name)
        .or_insert_with(|| SchedulerState::new(name));
    update(state)
}

/// The next delay after a stop. A loop that stayed up for `ran` starts over
/// at the minimum; an immediate return doubles the delay, capped.
pub fn next_backoff(current: Duration, ran: Duration) -> Duration {
    if ran >= STABLE_AFTER {
        MIN_BACKOFF
    } else {
        (current * 2).min(MAX_BACKOFF)
    }
}

/// Snapshot for the API/UI. Sorted by name so the screen does not jump.
pub fn snapshot() -> Vec<SchedulerState> {
    registry()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .values()
        .cloned()
        .collect()
}

fn mark_started(name: &'static str) {
    with_state(name, |state| {
        state.running = true;
        state.last_started = Some(chrono::Utc::now().to_rfc3339());
    });
}

fn mark_stopped(name: &'static str, reason: &str, restart: bool) {
    with_state(name, |state| {
        state.running = false;
        state.last_error = Some(reason.to_string());
        if restart {
            state.restarts = state.restarts.saturating_add(1);
        }
    });
}

/// Starts a supervised loop. The factory is called again after every stop, so
/// it must create a fresh future each time (clone the state inside it).
///
/// The returned handle is the *supervisor*, not the loop: aborting it stops
/// the restarts, and the loop itself is cancelled with the runtime.
pub fn spawn<F, Fut>(name: &'static str, factory: F) -> tokio::task::JoinHandle<()>
where
    F: Fn() -> Fut + Send + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    spawn_with(name, factory, MIN_BACKOFF, MAX_BACKOFF)
}

fn spawn_with<F, Fut>(
    name: &'static str,
    factory: F,
    min_backoff: Duration,
    max_backoff: Duration,
) -> tokio::task::JoinHandle<()>
where
    F: Fn() -> Fut + Send + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    // Register the loop before the task runs, so a Health read immediately
    // after boot lists it even before its first tick.
    with_state(name, |_| {});
    tokio::spawn(async move {
        let mut backoff = min_backoff;
        loop {
            mark_started(name);
            let started = std::time::Instant::now();
            // `tokio::spawn` is what turns a panic into a `JoinError`; without
            // it a panicking loop would take the supervisor down with it.
            let outcome = tokio::spawn(factory()).await;
            let ran = started.elapsed();
            let reason = match outcome {
                Ok(()) => "returned unexpectedly",
                Err(e) if e.is_panic() => "panicked",
                Err(_) => "cancelled",
            };
            mark_stopped(name, reason, true);
            tracing::error!(
                scheduler = name,
                reason,
                ran_secs = ran.as_secs(),
                backoff_secs = backoff.as_secs(),
                "Background loop stopped; restarting under supervisor"
            );
            tokio::time::sleep(backoff).await;
            backoff = next_backoff(backoff, ran).min(max_backoff);
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_doubles_then_caps_and_resets_after_a_stable_run() {
        assert_eq!(
            next_backoff(MIN_BACKOFF, Duration::ZERO),
            Duration::from_secs(10)
        );
        assert_eq!(
            next_backoff(Duration::from_secs(200), Duration::ZERO),
            MAX_BACKOFF
        );
        assert_eq!(next_backoff(MAX_BACKOFF, Duration::ZERO), MAX_BACKOFF);
        assert_eq!(
            next_backoff(Duration::from_secs(80), STABLE_AFTER),
            MIN_BACKOFF
        );
    }

    #[tokio::test]
    async fn a_returning_loop_is_recorded_and_restarted() {
        let name: &'static str = "test-returning-loop";
        let supervisor = spawn_with(
            name,
            || async {},
            Duration::from_millis(1),
            Duration::from_millis(2),
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
        let state = snapshot()
            .into_iter()
            .find(|state| state.name == name)
            .expect("registered");
        assert!(state.restarts >= 1, "{state:?}");
        assert!(!state.running, "{state:?}");
        assert_eq!(state.last_error.as_deref(), Some("returned unexpectedly"));
        supervisor.abort();
    }

    #[tokio::test]
    async fn a_panicking_loop_is_recorded_and_restarted() {
        let name: &'static str = "test-panicking-loop";
        let supervisor = spawn_with(
            name,
            || async { panic!("boom") },
            Duration::from_millis(1),
            Duration::from_millis(2),
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
        let state = snapshot()
            .into_iter()
            .find(|state| state.name == name)
            .expect("registered");
        assert!(state.restarts >= 1, "{state:?}");
        assert_eq!(state.last_error.as_deref(), Some("panicked"));
        supervisor.abort();
    }
}

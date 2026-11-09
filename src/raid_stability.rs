/* =============================================================================
 * File          : raid_stability.rs
 * Author        : Nicolas Le Roux
 * Created       : 05/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   RAID reliability helpers (tasklist P1-21): resync speed profiles, parsing
 *   `mdadm --monitor` events, spare selection and a poll loop that turns an
 *   array losing redundancy into a HomeVault notification.
 *
 * Dependencies:
 *   - tokio, serde, crate::disk::DiskService
 *
 * Usage:
 *   Spawned once at startup in src/web/mod.rs; see docs/guides/storage.md.
 * =============================================================================
 */

//! RAID resync speed, event parsing and degradation notifications.

use std::path::PathBuf;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::handlers::AppState;
use crate::storage::atomic::AtomicWriter;

const STARTUP_DELAY: std::time::Duration = std::time::Duration::from_secs(2 * 60);
const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(5 * 60);

/// A resync-speed profile exposed to the admin. `Low` is the safe default: a
/// rebuild must not starve the shares it is protecting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpeedProfile {
    Low,
    Balanced,
    Fast,
}

impl SpeedProfile {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "low" => Some(Self::Low),
            "balanced" | "medium" => Some(Self::Balanced),
            "fast" => Some(Self::Fast),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Balanced => "balanced",
            Self::Fast => "fast",
        }
    }

    /// `(speed_limit_min, speed_limit_max)` in kB/s. `min` is what mdadm uses
    /// while a rebuild competes with normal I/O; `max` caps the burst.
    pub fn limits(self) -> (u32, u32) {
        match self {
            Self::Low => (1_000, 25_000),
            Self::Balanced => (5_000, 100_000),
            Self::Fast => (25_000, 500_000),
        }
    }
}

/// The sysfs files a profile writes, in order.
pub fn speed_files(profile: SpeedProfile) -> Vec<(&'static str, String)> {
    let (min, max) = profile.limits();
    vec![
        ("/proc/sys/dev/raid/speed_limit_min", min.to_string()),
        ("/proc/sys/dev/raid/speed_limit_max", max.to_string()),
    ]
}

/// Path of the persisted resync-speed profile.
fn speed_path() -> PathBuf {
    crate::config::paths::data_dir()
        .join("state")
        .join("raid-speed.json")
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct SpeedState {
    profile: String,
}

/// The profile last saved by the admin. `Low` is the safe default: a rebuild
/// must not starve the shares it protects.
pub fn load_speed_profile() -> SpeedProfile {
    std::fs::read_to_string(speed_path())
        .ok()
        .and_then(|raw| serde_json::from_str::<SpeedState>(&raw).ok())
        .and_then(|state| SpeedProfile::parse(&state.profile))
        .unwrap_or(SpeedProfile::Low)
}

/// Persists the choice even when the host has no md array yet: the profile is
/// applied at the next boot or as soon as an array is created.
pub fn save_speed_profile(profile: SpeedProfile) -> Result<(), String> {
    if let Some(parent) = speed_path().parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("could not create {}: {e}", parent.display()))?;
    }
    let raw = serde_json::to_string(&SpeedState {
        profile: profile.as_str().to_string(),
    })
    .map_err(|e| format!("could not encode the RAID speed profile: {e}"))?;
    AtomicWriter::write(&speed_path(), &raw)
        .map_err(|e| format!("could not save the RAID speed profile: {e}"))
}

/// Applies the saved profile; called at startup and after an array is created
/// or rebuilt. A host without `/proc/sys/dev/raid` returns the kernel error,
/// which the caller reports instead of pretending the setting is active.
pub async fn apply_saved_speed_profile() -> Result<SpeedProfile, String> {
    let profile = load_speed_profile();
    apply_speed_profile(profile).await?;
    Ok(profile)
}

/// Write both limits through `sudo tee`. `mdadm` has no API for these; they
/// are plain sysctls and a write failure is returned, never ignored.
pub async fn apply_speed_profile(profile: SpeedProfile) -> Result<(), String> {
    for (path, value) in speed_files(profile) {
        let mut cmd = tokio::process::Command::new("sudo");
        cmd.args(["tee", path]);
        let output = crate::utils::process::run_timed_async_with_stdin(
            &mut cmd,
            std::time::Duration::from_secs(10),
            format!("{value}\n").as_bytes(),
        )
        .await
        .map_err(|e| format!("could not write {path}: {e}"))?;
        if !output.status.success() {
            return Err(format!(
                "could not write {path}: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
    }
    Ok(())
}

/// One event `mdadm --monitor --program` would emit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RaidEvent {
    Fail,
    DegradedArray,
    SpareActive,
    RebuildStarted,
    RebuildFinished,
    Other,
}

/// Parse one `mdadm --monitor` line (`--program` receives
/// `md/array <event> <device>`). The event is the second field.
pub fn parse_monitor_event(line: &str) -> Option<RaidEvent> {
    let event = line.split_whitespace().nth(1)?;
    Some(match event {
        "Fail" | "FailSpare" | "TestMessage" => RaidEvent::Fail,
        "DegradedArray" => RaidEvent::DegradedArray,
        "SpareActive" => RaidEvent::SpareActive,
        "RebuildStarted" => RaidEvent::RebuildStarted,
        "RebuildFinished" => RaidEvent::RebuildFinished,
        _ => RaidEvent::Other,
    })
}

/// Whether an event means the array lost redundancy *now* and deserves a
/// warning. RebuildStarted is informational; SpareActive is good news.
pub fn event_severity(event: RaidEvent) -> Option<&'static str> {
    match event {
        RaidEvent::Fail | RaidEvent::DegradedArray => Some("critical"),
        RaidEvent::SpareActive | RaidEvent::RebuildStarted => Some("info"),
        RaidEvent::RebuildFinished | RaidEvent::Other => None,
    }
}

/// Pick a whole free disk to add as a spare. `excluded` holds the member and
/// existing spare devices; a candidate must not be in it.
pub fn pick_spare<'a>(candidates: &'a [String], excluded: &[String]) -> Option<&'a String> {
    candidates
        .iter()
        .find(|candidate| !excluded.iter().any(|e| e == *candidate))
}

fn state_path() -> PathBuf {
    crate::config::paths::data_dir()
        .join("state")
        .join("raid-states.json")
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct ArrayState {
    name: String,
    state: String,
}

/// Compare the previous poll with the current one and return the arrays that
/// just stopped being clean. Pure so the notification policy is testable.
pub fn degraded_transitions(
    previous: &[ArrayState],
    current: &[ArrayState],
) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for now in current {
        let was_clean = previous
            .iter()
            .find(|p| p.name == now.name)
            .map(|p| is_clean(&p.state))
            .unwrap_or(true);
        if was_clean && !is_clean(&now.state) {
            out.push((now.name.clone(), now.state.clone()));
        }
    }
    out
}

fn is_clean(state: &str) -> bool {
    let s = state.to_ascii_lowercase();
    !s.contains("degrad") && !s.contains("fail") && !s.contains("inactive")
}

/// A sentence to append to a degradation notification: either a free disk
/// that could become the spare, or an explicit "none available". The actual
/// `mdadm --add` stays a human action (it changes the machine's inventory).
pub fn spare_suggestion(candidates: &[String], members: &[String]) -> String {
    match pick_spare(candidates, members) {
        Some(device) => {
            format!("Candidate spare: {device}. Add it from Storage after checking it is empty.")
        }
        None => {
            "No free disk is available as a spare; attach one before another failure.".to_string()
        }
    }
}

/// One poll: read the arrays, diff against the saved snapshot, notify on a
/// transition to degraded, then persist the new snapshot.
pub async fn poll_once(state: &Arc<AppState>) -> Result<Vec<String>, String> {
    let arrays = crate::disk::DiskService::list_raid_arrays()
        .await
        .map_err(|e| format!("could not list RAID arrays: {e}"))?;
    let current: Vec<ArrayState> = arrays
        .iter()
        .map(|a| ArrayState {
            name: a.name.clone(),
            state: a.state.clone(),
        })
        .collect();
    let previous: Vec<ArrayState> = std::fs::read_to_string(state_path())
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default();

    let degraded = degraded_transitions(&previous, &current);
    let candidates: Vec<String> = crate::disk::DiskService::get_available_devices()
        .await
        .unwrap_or_default()
        .into_iter()
        .filter(|disk| !disk.in_use)
        .filter(|disk| disk.smart_status.as_deref() != Some("Failed"))
        .map(|disk| {
            if disk.name.starts_with("/dev/") {
                disk.name
            } else {
                format!("/dev/{}", disk.name)
            }
        })
        .collect();
    for (name, status) in &degraded {
        let members = arrays
            .iter()
            .find(|array| &array.name == name)
            .map(|array| array.devices.clone())
            .unwrap_or_default();
        let message = format!(
            "RAID array '{name}' is no longer redundant: {status}. Check the failed member. {}",
            spare_suggestion(&candidates, &members)
        );
        crate::notification_engine::notify(
            state,
            "health.raid_arrays",
            "RAID array degraded",
            &message,
            "critical",
            "storage",
            "raid_stability",
        )
        .await;
        crate::events::log_event_warn(&message);
    }

    if let Some(parent) = state_path().parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(raw) = serde_json::to_string(&current) {
        let _ = AtomicWriter::write(&state_path(), &raw);
    }
    Ok(degraded.into_iter().map(|(name, _)| name).collect())
}

pub async fn run_monitor_loop(state: Arc<AppState>) {
    tokio::time::sleep(STARTUP_DELAY).await;
    // FIAB-10: the sysctls are volatile kernel state. Re-apply the saved
    // profile once the md module is loaded; a host without RAID logs at debug
    // and the monitor keeps running.
    match apply_saved_speed_profile().await {
        Ok(profile) => tracing::info!("RAID resync speed profile applied: {}", profile.as_str()),
        Err(e) => tracing::debug!("RAID resync speed not applied: {e}"),
    }
    loop {
        if let Err(e) = poll_once(&state).await {
            tracing::warn!("RAID monitor: {e}");
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn speed_profiles_are_a_closed_set_with_ordered_limits() {
        assert_eq!(SpeedProfile::parse("LOW"), Some(SpeedProfile::Low));
        assert_eq!(SpeedProfile::parse("medium"), Some(SpeedProfile::Balanced));
        assert_eq!(SpeedProfile::parse("fast"), Some(SpeedProfile::Fast));
        assert_eq!(SpeedProfile::parse("turbo"), None);
        for profile in [
            SpeedProfile::Low,
            SpeedProfile::Balanced,
            SpeedProfile::Fast,
        ] {
            let (min, max) = profile.limits();
            assert!(min > 0 && min < max);
            let files = speed_files(profile);
            assert_eq!(files.len(), 2);
            assert_eq!(files[0].0, "/proc/sys/dev/raid/speed_limit_min");
            assert_eq!(files[0].1, min.to_string());
        }
    }

    #[test]
    fn speed_profile_state_round_trips_and_unknown_falls_back() {
        let raw = serde_json::to_string(&SpeedState {
            profile: "balanced".to_string(),
        })
        .unwrap();
        let parsed: SpeedState = serde_json::from_str(&raw).unwrap();
        assert_eq!(
            SpeedProfile::parse(&parsed.profile),
            Some(SpeedProfile::Balanced)
        );
        assert!(SpeedProfile::parse("turbo").is_none());
        let unknown: SpeedState = serde_json::from_str(r#"{"profile":"turbo"}"#).unwrap();
        assert_eq!(SpeedProfile::parse(&unknown.profile), None);
    }

    #[test]
    fn monitor_events_are_parsed_from_the_second_field() {
        assert_eq!(
            parse_monitor_event("md/raid1 Fail /dev/sdb1"),
            Some(RaidEvent::Fail)
        );
        assert_eq!(
            parse_monitor_event("md/raid5 DegradedArray /dev/md0"),
            Some(RaidEvent::DegradedArray)
        );
        assert_eq!(
            parse_monitor_event("md/raid1 SpareActive /dev/sdc1"),
            Some(RaidEvent::SpareActive)
        );
        assert_eq!(parse_monitor_event("garbage"), None);
        assert_eq!(event_severity(RaidEvent::DegradedArray), Some("critical"));
        assert_eq!(event_severity(RaidEvent::RebuildFinished), None);
    }

    #[test]
    fn a_spare_must_not_be_a_member_or_an_existing_spare() {
        let candidates = vec![
            "/dev/sda".to_string(),
            "/dev/sdb".to_string(),
            "/dev/sdc".to_string(),
        ];
        let excluded = vec!["/dev/sda".to_string(), "/dev/sdb".to_string()];
        assert_eq!(
            pick_spare(&candidates, &excluded),
            Some(&"/dev/sdc".to_string())
        );
        let suggestion = spare_suggestion(&candidates, &excluded);
        assert!(suggestion.contains("/dev/sdc"));
        let none = spare_suggestion(&candidates, &candidates);
        assert!(none.contains("No free disk"));
        assert!(pick_spare(&candidates[..2], &excluded).is_none());
    }

    #[test]
    fn only_a_clean_to_degraded_transition_notifies() {
        let state = |name: &str, state: &str| ArrayState {
            name: name.to_string(),
            state: state.to_string(),
        };
        let previous = vec![state("md0", "clean"), state("md1", "degraded")];
        let current = vec![
            state("md0", "degraded"),
            state("md1", "degraded"),
            state("md2", "clean"),
        ];
        let transitions = degraded_transitions(&previous, &current);
        assert_eq!(
            transitions,
            vec![("md0".to_string(), "degraded".to_string())]
        );
    }
}

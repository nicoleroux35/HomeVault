/* =============================================================================
 * File          : notification_events.rs
 * Author        : Nicolas Le Roux
 * Created       : 13/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Per-event notification control: the catalog of notification event ids
 *   the engine can raise (health checks, backup jobs, app updates), the
 *   opt-out lookup against NotificationSettings.event_toggles, and the
 *   sink payload builders (generic JSON webhook, Slack, Discord) used by
 *   the dispatch path.
 *
 * Dependencies:
 *   - serde (>= 1.0)
 *
 * Usage:
 *   notification_engine.rs (before raising/sending), handlers/notifications.rs
 *   (GET/PUT /notifications/events expose the catalog + state).
 * =============================================================================
 */

//! Notification event catalog + per-event toggles + sink payload formats.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// One entry of the event catalog: the stable id stored in
/// `event_toggles`, the group it renders under in Settings, and the i18n
/// key the frontend labels it with.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NotificationEvent {
    pub id: &'static str,
    pub group: &'static str,
    pub label_key: &'static str,
}

/// The events the notification engine can raise today. Order = Settings
/// display order. Ids are stable API: stored per admin in
/// `event_toggles`, so renaming one silently resets a user's choice —
/// don't rename.
pub const EVENT_CATALOG: &[NotificationEvent] = &[
    NotificationEvent {
        id: "backup.jobs",
        group: "backup",
        label_key: "notifications.events.backup_jobs",
    },
    NotificationEvent {
        id: "app.updates",
        group: "system",
        label_key: "notifications.events.app_updates",
    },
    NotificationEvent {
        id: "health.authentication",
        group: "health",
        label_key: "notifications.events.health_authentication",
    },
    NotificationEvent {
        id: "health.storage",
        group: "health",
        label_key: "notifications.events.health_storage",
    },
    NotificationEvent {
        id: "health.memory",
        group: "health",
        label_key: "notifications.events.health_memory",
    },
    NotificationEvent {
        id: "health.disk_space",
        group: "health",
        label_key: "notifications.events.health_disk_space",
    },
    NotificationEvent {
        id: "health.disk_smart",
        group: "health",
        label_key: "notifications.events.health_disk_smart",
    },
    NotificationEvent {
        id: "health.raid_arrays",
        group: "health",
        label_key: "notifications.events.health_raid_arrays",
    },
    NotificationEvent {
        id: "health.network_interfaces",
        group: "health",
        label_key: "notifications.events.health_network_interfaces",
    },
    NotificationEvent {
        id: "health.systemd_services",
        group: "health",
        label_key: "notifications.events.health_systemd_services",
    },
    NotificationEvent {
        id: "health.temperatures",
        group: "health",
        label_key: "notifications.events.health_temperatures",
    },
];

/// Webhook payload shape. `generic` is the historical HomeVault JSON;
/// `slack`/`discord` match what those services expect in an incoming
/// webhook so admins can paste their channel URL without a relay.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum WebhookFormat {
    #[default]
    Generic,
    Slack,
    Discord,
}

impl WebhookFormat {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "generic" => Some(WebhookFormat::Generic),
            "slack" => Some(WebhookFormat::Slack),
            "discord" => Some(WebhookFormat::Discord),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            WebhookFormat::Generic => "generic",
            WebhookFormat::Slack => "slack",
            WebhookFormat::Discord => "discord",
        }
    }

    /// Builds the POST body for this sink format.
    pub fn payload(
        &self,
        title: &str,
        message: &str,
        severity: &str,
        category: &str,
    ) -> serde_json::Value {
        match self {
            WebhookFormat::Generic => serde_json::json!({
                "title": title,
                "message": message,
                "severity": severity,
                "category": category,
            }),
            WebhookFormat::Slack => serde_json::json!({
                "text": format!("[{severity}] *{title}*\n{message}"),
            }),
            WebhookFormat::Discord => serde_json::json!({
                "content": format!("**[{severity}] {title}**\n{message}"),
            }),
        }
    }
}

/// Whether an event should notify, given the admin's toggle map. Absent id
/// (or absent map) = enabled: the catalog can grow without flipping every
/// existing install to silent.
pub fn is_event_enabled(event_id: &str, toggles: Option<&HashMap<String, bool>>) -> bool {
    match toggles {
        None => true,
        Some(map) => map.get(event_id).copied().unwrap_or(true),
    }
}

/// Builds the event id for a health-check notification from the check's
/// name (the engine raises one event per check).
pub fn health_event_id(check_name: &str) -> String {
    format!("health.{}", check_name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_ids_are_unique_and_well_formed() {
        let mut seen = std::collections::HashSet::new();
        for event in EVENT_CATALOG {
            assert!(seen.insert(event.id), "duplicate event id {}", event.id);
            assert!(event.id.starts_with("health.") || event.id.contains('.'));
            assert!(event.label_key.starts_with("notifications.events."));
        }
        // The nine health checks of HealthService::run_all_checks must all
        // have a catalog entry (check names: authentication, storage, memory,
        // disk_space, disk_smart, raid_arrays, network_interfaces,
        // systemd_services, temperatures — see src/health.rs).
        for check in [
            "authentication",
            "storage",
            "memory",
            "disk_space",
            "disk_smart",
            "raid_arrays",
            "network_interfaces",
            "systemd_services",
            "temperatures",
        ] {
            let id = health_event_id(check);
            assert!(
                EVENT_CATALOG.iter().any(|e| e.id == id),
                "health check '{check}' missing from the catalog"
            );
        }
    }

    #[test]
    fn toggles_default_to_enabled() {
        assert!(is_event_enabled("anything", None));
        assert!(is_event_enabled("health.cpu", Some(&HashMap::new())));
        let mut map = HashMap::new();
        map.insert("backup.jobs".to_string(), false);
        assert!(!is_event_enabled("backup.jobs", Some(&map)));
        assert!(
            is_event_enabled("app.updates", Some(&map)),
            "absent id stays enabled"
        );
    }

    #[test]
    fn sink_payloads_match_each_service_shape() {
        let generic =
            WebhookFormat::Generic.payload("Backup failed", "repo-x failed", "error", "backup");
        assert_eq!(generic["title"], "Backup failed");
        assert_eq!(generic["severity"], "error");
        assert!(generic.get("text").is_none());

        let slack =
            WebhookFormat::Slack.payload("Backup failed", "repo-x failed", "error", "backup");
        assert_eq!(slack["text"], "[error] *Backup failed*\nrepo-x failed");
        assert!(slack.get("title").is_none(), "slack has no title field");

        let discord =
            WebhookFormat::Discord.payload("Backup failed", "repo-x failed", "error", "backup");
        assert_eq!(
            discord["content"],
            "**[error] Backup failed**\nrepo-x failed"
        );
        assert!(discord.get("title").is_none());
    }

    #[test]
    fn webhook_format_parses_strictly() {
        assert_eq!(
            WebhookFormat::parse("generic"),
            Some(WebhookFormat::Generic)
        );
        assert_eq!(WebhookFormat::parse("slack"), Some(WebhookFormat::Slack));
        assert_eq!(
            WebhookFormat::parse("discord"),
            Some(WebhookFormat::Discord)
        );
        assert_eq!(WebhookFormat::parse("teams"), None);
        assert_eq!(WebhookFormat::parse(""), None);
        // Default is generic — the historical behaviour for existing installs.
        assert_eq!(WebhookFormat::default(), WebhookFormat::Generic);
    }
}

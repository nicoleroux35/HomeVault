/* =============================================================================
 * File          : fail2ban.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 28/09/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   In-memory brute-force protection: counts failed attempts per client IP
 *   over a rolling 15-minute window and temporarily bans IPs that exceed
 *   the allowed number of attempts. Everything lives in memory only, so a
 *   restart clears every ban and failure count.
 *
 * Dependencies:
 *   - parking_lot (>= 0.12)
 *
 * Usage:
 *   Intended to guard authentication endpoints: call `record_failure` when
 *   an attempt fails and `is_blocked` before accepting a new one. Note:
 *   no route or handler currently references `Fail2BanState` — it is
 *   defined and re-exported but not wired anywhere yet.
 * =============================================================================
 */

//! In-memory IP ban list for brute-force protection.

use parking_lot::RwLock as ParkingRwLock;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// In-memory fail2ban: tracks failed attempts and temporarily bans IPs.
#[derive(Clone)]
pub struct Fail2BanState {
    offenders: Arc<ParkingRwLock<HashMap<String, OffenderRecord>>>,
    max_attempts: u32,
    ban_duration: Duration,
}

/// Tracks failure count and ban state for a single IP.
#[derive(Debug)]
struct OffenderRecord {
    attempts: u32,
    first_failure: Instant,
    blocked_until: Option<Instant>,
}

impl Fail2BanState {
    /// Create a new fail2ban state.
    pub fn new(max_attempts: u32, ban_duration: Duration) -> Self {
        Self {
            offenders: Arc::new(ParkingRwLock::new(HashMap::new())),
            max_attempts,
            ban_duration,
        }
    }

    /// Record a failed attempt. Returns `true` if the IP should now be banned.
    pub fn record_failure(&self, ip: &str) -> bool {
        let mut offenders = self.offenders.write();
        let now = Instant::now();

        let entry = offenders.entry(ip.to_string()).or_insert(OffenderRecord {
            attempts: 0,
            first_failure: now,
            blocked_until: None,
        });
        entry.attempts += 1;

        if now.duration_since(entry.first_failure) > Duration::from_secs(900) {
            entry.attempts = 1;
            entry.first_failure = now;
        }

        if entry.attempts >= self.max_attempts {
            entry.blocked_until = Some(now + self.ban_duration);
            true
        } else {
            false
        }
    }

    /// Check whether an IP is currently banned.
    pub fn is_blocked(&self, ip: &str) -> bool {
        let offenders = self.offenders.read();
        let now = Instant::now();

        if let Some(entry) = offenders.get(ip) {
            if let Some(until) = entry.blocked_until {
                return now < until;
            }
        }
        false
    }

    /// Clear failure history after a successful login.
    pub fn record_success(&self, ip: &str) {
        let mut offenders = self.offenders.write();
        offenders.remove(ip);
    }

    /// Remove expired entries (call periodically).
    pub fn cleanup_expired(&self) {
        let mut offenders = self.offenders.write();
        let now = Instant::now();

        offenders.retain(|_, entry| entry.blocked_until.is_some_and(|until| now < until));
    }
}

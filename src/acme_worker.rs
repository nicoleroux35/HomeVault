/* =============================================================================
 * File          : acme_worker.rs
 * Author        : Nicolas Le Roux
 * Created       : 29/10/2026
 * Last modified : 29/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Daily certificate-automation worker: refreshes the status from disk,
 *   renews inside the renewal window, honours the retry backoff, and raises a
 *   notification when a renewal fails. One run at a time, process-wide.
 *
 * Dependencies:
 *   - tokio (>= 1, timers), crate::acme, crate::notification_engine
 *
 * Usage:
 *   Spawned once from web::run when automation is on; the Settings card's
 *   "Renew now" button reuses the same path through `spawn_issuance`.
 * =============================================================================
 */

//! The renewal worker (see docs/guides/acme.md).

use std::sync::Arc;

use crate::acme::{self, IssueOutcome, Issuer, ReqwestTransport};
use crate::handlers::AppState;

/// Give the server a moment to finish starting before talking to a CA.
pub const FIRST_TICK_DELAY: std::time::Duration = std::time::Duration::from_secs(30);

/// Why an issuance did not happen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IssueTrigger {
    /// The daily worker noticed the certificate is inside its renewal window.
    Scheduled,
    /// An admin pressed "Issue now".
    Manual,
    /// An admin confirmed they created the manual DNS record.
    ConfirmDns,
}

/// The daily loop. Never exits: a failure is logged, recorded in the settings
/// and retried once the backoff allows, and the loop itself must not die — a
/// transient CA outage would otherwise disable renewal until the next restart.
pub async fn run(state: Arc<AppState>) {
    tokio::time::sleep(FIRST_TICK_DELAY).await;
    loop {
        if let Err(e) = tick(&state).await {
            tracing::warn!("Certificate automation: {e}");
        }
        tokio::time::sleep(acme::WORKER_INTERVAL).await;
    }
}

/// One pass: refresh the status, then renew if the settings say it is due.
pub async fn tick(state: &Arc<AppState>) -> Result<(), String> {
    let mut settings = acme::load_settings();
    if !settings.enabled {
        return Ok(());
    }
    if acme::refresh_status(&mut settings) {
        let _ = acme::save_settings(&settings);
    }
    if !acme::should_renew(&settings) {
        return Ok(());
    }
    run_issuance(state, IssueTrigger::Scheduled).await
}

/// Start an issuance in the background and return `false` when one is already
/// running. The handler answers immediately: a DNS-01 order can wait minutes
/// for propagation, which is far past any sensible request deadline.
pub fn spawn_issuance(state: Arc<AppState>, trigger: IssueTrigger) -> bool {
    if !acme::try_begin_run() {
        return false;
    }
    tokio::spawn(async move {
        if let Err(e) = run_issuance(&state, trigger).await {
            tracing::warn!("Certificate automation: {e}");
        }
    });
    true
}

/// The issuance itself. Assumes the run lock is held (or is the worker).
pub async fn run_issuance(state: &Arc<AppState>, trigger: IssueTrigger) -> Result<(), String> {
    let mut settings = acme::load_settings();
    let confirm = trigger == IssueTrigger::ConfirmDns;
    let result = if confirm {
        match acme::load_pending_run() {
            Some(run) => issuer().finish_manual(&settings, &run).await,
            None => Err(acme::AcmeError::final_error(
                "there is no DNS record waiting for confirmation",
            )),
        }
    } else {
        issuer().issue(&settings).await
    };
    acme::end_run();

    match result {
        Ok(IssueOutcome::Installed { not_after }) => {
            acme::record_success(&mut settings);
            // Push the newly written pair to the running HTTPS listener so a
            // renewal takes effect without a restart. Best-effort: a failure is
            // logged and never undoes the success already recorded on disk.
            match crate::tls::reload_acme_certificate().await {
                Ok(true) => tracing::info!("ACME: the live HTTPS listener now serves the renewed certificate"),
                Ok(false) => {}
                Err(e) => tracing::warn!(
                    "ACME: certificate renewed on disk but the live TLS listener did not reload: {e}"
                ),
            }
            if let Some(expiry) = not_after {
                settings.last_success = Some(expiry.clone());
                settings.last_error = None;
                notify(
                    state,
                    "certificate.issued",
                    "A new certificate is installed",
                    &format!(
                        "The certificate for {} was issued and is valid until {}.",
                        settings.domain, expiry
                    ),
                    "info",
                )
                .await;
            }
            acme::refresh_status(&mut settings);
        }
        Ok(IssueOutcome::AwaitingDns { record }) => {
            settings.status = acme::AcmeStatus::Pending;
            settings.pending_dns_record = Some(record);
            settings.last_error = None;
            settings.last_attempt = Some(acme::now_rfc3339());
        }
        Err(e) => {
            acme::record_failure(&mut settings, &e.message, e.retryable);
            settings.pending_dns_record = None;
            notify(
                state,
                "certificate.renewal_failed",
                "Certificate renewal failed",
                &e.message,
                "error",
            )
            .await;
        }
    }
    acme::save_settings(&settings)?;
    Ok(())
}

fn issuer() -> Issuer {
    match ReqwestTransport::new() {
        Ok(transport) => Issuer::new(Arc::new(transport)),
        // Without an HTTP client there is no way to reach a CA: surface it as
        // an issuance failure rather than panicking in the background.
        Err(_) => Issuer::new(Arc::new(DeadTransport)),
    }
}

/// Stands in when the HTTP client cannot be built; every request fails with a
/// clear message instead of the process panicking.
struct DeadTransport;

impl acme::Transport for DeadTransport {
    fn send<'a>(
        &'a self,
        _request: crate::outbound::HttpRequest,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = Result<crate::outbound::HttpResponse, acme::AcmeError>>
                + Send
                + 'a,
        >,
    > {
        Box::pin(async {
            Err(acme::AcmeError::new(
                "the HTTP client for certificate automation could not be created",
                true,
            ))
        })
    }
}

async fn notify(state: &Arc<AppState>, event: &str, title: &str, message: &str, severity: &str) {
    crate::notification_engine::notify(
        state,
        event,
        title,
        message,
        severity,
        "system",
        "certificate",
    )
    .await;
}

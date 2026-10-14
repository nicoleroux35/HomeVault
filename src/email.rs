/* =============================================================================
 * File          : email.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 10/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Outgoing email for notifications: sends through whichever local mail
 *   transfer agent binary is available (mutt, then sendmail, then msmtp),
 *   with a deadline-aware child-process wait so a hung MTA cannot stall a
 *   notification. SMTP credentials are read by the MTA from its own
 *   configuration, never stored by HomeVault beyond the settings form.
 *
 * Dependencies:
 *   - serde (>= 1.0), thiserror (>= 2), tracing (>= 0.1)
 *
 * Usage:
 *   notification_engine.rs (alerts) and the email test endpoint.
 * =============================================================================
 */

//! Outgoing email via the host's available mail transfer agent binaries.

use serde::{Deserialize, Serialize};
use std::process::{Command, ExitStatus};
use std::time::Duration;
use thiserror::Error;
use tracing::warn;

/// Maximum time (seconds) to wait for an external MTA process to complete.
const EMAIL_TIMEOUT_SECS: u64 = 30;

/// Errors that can occur when sending email.
#[derive(Error, Debug)]
pub enum EmailError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Configuration error: {0}")]
    Config(String),
    #[error("Send error: {0}")]
    Send(String),
    #[error("Timeout error")]
    Timeout,
}

/// Extension trait that adds a deadline-aware `wait` to `std::process::Child`.
trait WaitWithTimeout {
    fn wait_with_timeout(self) -> std::io::Result<ExitStatus>;
}

impl WaitWithTimeout for std::process::Child {
    /// Wait for the child process to exit, killing it if `EMAIL_TIMEOUT_SECS` elapses.
    fn wait_with_timeout(mut self) -> std::io::Result<ExitStatus> {
        use std::thread;
        use std::time::Instant;

        let timeout = Duration::from_secs(EMAIL_TIMEOUT_SECS);
        let start = Instant::now();

        loop {
            match self.try_wait()? {
                Some(status) => return Ok(status),
                None => {
                    if start.elapsed() >= timeout {
                        let _ = self.kill();
                        let _ = self.wait();
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::TimedOut,
                            "process timed out",
                        ));
                    }
                    thread::sleep(Duration::from_millis(10));
                }
            }
        }
    }
}

/// SMTP connection parameters.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmailConfig {
    pub smtp_host: String,
    pub smtp_port: u16,
    pub smtp_username: String,
    pub smtp_password: String,
    pub from_address: String,
    pub from_name: String,
    pub use_tls: bool,
}

impl Default for EmailConfig {
    fn default() -> Self {
        Self {
            smtp_host: "localhost".to_string(),
            smtp_port: 587,
            smtp_username: String::new(),
            smtp_password: String::new(),
            from_address: "homevault@localhost".to_string(),
            from_name: "HomeVault".to_string(),
            use_tls: true,
        }
    }
}

/// A single email recipient, optionally identified by a display name.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmailRecipient {
    pub email: String,
    pub name: Option<String>,
}

/// A prepared email message that can be sent via any available MTA.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmailMessage {
    pub to: Vec<EmailRecipient>,
    pub subject: String,
    pub body: String,
    /// When `true`, the body is rendered as HTML; otherwise as plain text.
    pub is_html: bool,
}

/// A pre-formatted system alert email with severity-aware HTML styling.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemAlertEmail {
    pub severity: String,
    pub title: String,
    pub message: String,
    pub details: Option<String>,
}

/// Service for sending email via available MTA binaries (mutt, sendmail, msmtp).
pub struct EmailService;

impl EmailService {
    /// Send an email. Tries `mutt → sendmail → msmtp` in order, returning the first success.
    pub fn send_email(message: &EmailMessage, config: &EmailConfig) -> Result<(), EmailError> {
        let to_addrs: String = message
            .to
            .iter()
            .map(|r| {
                if let Some(name) = &r.name {
                    format!("\"{}\" <{}>", name, r.email)
                } else {
                    r.email.clone()
                }
            })
            .collect::<Vec<_>>()
            .join(", ");

        let content_type = if message.is_html {
            "text/html"
        } else {
            "text/plain"
        };

        let email_content = format!(
            "From: \"{}\" <{}>\n\
             To: {}\n\
             Subject: {}\n\
             Content-Type: {}; charset=UTF-8\n\
             \n\
             {}",
            config.from_name,
            config.from_address,
            to_addrs,
            message.subject,
            content_type,
            message.body
        );

        Self::send_via_mutt(&to_addrs, &message.subject, &message.body, message.is_html)
            .or_else(|_| Self::send_via_sendmail(&to_addrs, &email_content))
            .or_else(|_| Self::send_via_msmtp(&to_addrs, &message.body, config))
    }

    /// Send via `mutt`, piping the body on stdin. Supports HTML content via `content_type` flag.
    fn send_via_mutt(to: &str, subject: &str, body: &str, is_html: bool) -> Result<(), EmailError> {
        let content_type = if is_html { "text/html" } else { "text/plain" };

        let mut child = Command::new("mutt")
            .args(["-e", &format!("set content_type={}", content_type)])
            .args(["-s", subject])
            .arg("--")
            .arg(to)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|e| EmailError::Send(format!("mutt not found: {}", e)))?;

        if let Some(mut stdin) = child.stdin.take() {
            use std::io::Write;
            if stdin.write_all(body.as_bytes()).is_err() {
                warn!("Failed to write email body to mutt stdin");
            }
        }

        let status = child.wait_with_timeout().map_err(|_| EmailError::Timeout)?;

        if status.success() {
            Ok(())
        } else {
            Err(EmailError::Send("mutt failed".to_string()))
        }
    }

    /// Send via `sendmail`, piping the fully-formed raw email on stdin.
    fn send_via_sendmail(to: &str, content: &str) -> Result<(), EmailError> {
        let mut child = Command::new("sendmail")
            .args(["-f", "homevault@localhost"])
            .arg(to)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|e| EmailError::Send(format!("sendmail not found: {}", e)))?;

        if let Some(mut stdin) = child.stdin.take() {
            use std::io::Write;
            if stdin.write_all(content.as_bytes()).is_err() {
                warn!("Failed to write email content to sendmail stdin");
            }
        }

        let status = child.wait_with_timeout().map_err(|_| EmailError::Timeout)?;

        if status.success() {
            Ok(())
        } else {
            Err(EmailError::Send("sendmail failed".to_string()))
        }
    }

    /// Send via `msmtp`, which reads SMTP credentials from its own config file.
    fn send_via_msmtp(to: &str, body: &str, config: &EmailConfig) -> Result<(), EmailError> {
        let mut child = Command::new("msmtp")
            .args(["--from", &config.from_address])
            .arg(to)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|e| EmailError::Send(format!("msmtp not found: {}", e)))?;

        if let Some(mut stdin) = child.stdin.take() {
            use std::io::Write;
            if stdin.write_all(body.as_bytes()).is_err() {
                warn!("Failed to write email body to msmtp stdin");
            }
        }

        let status = child.wait_with_timeout().map_err(|_| EmailError::Timeout)?;

        if status.success() {
            Ok(())
        } else {
            Err(EmailError::Send("msmtp failed".to_string()))
        }
    }

    /// Send a severity-colored HTML alert email to the given recipients,
    /// using the caller-supplied SMTP configuration.
    pub fn send_system_alert(
        alert: &SystemAlertEmail,
        recipients: &[EmailRecipient],
        config: &EmailConfig,
    ) -> Result<(), EmailError> {
        let severity_color = match alert.severity.to_lowercase().as_str() {
            "critical" => "#dc3545",
            "error" => "#fd7e14",
            "warning" => "#ffc107",
            "info" => "#17a2b8",
            _ => "#6c757d",
        };

        let html_body = format!(
            r#"<!DOCTYPE html>
<html>
<head>
    <meta charset="UTF-8">
    <style>
        body {{ font-family: Arial, sans-serif; margin: 0; padding: 20px; background-color: #f5f5f5; }}
        .container {{ max-width: 600px; margin: 0 auto; background: white; border-radius: 8px; box-shadow: 0 2px 4px rgba(0,0,0,0.1); }}
        .header {{ background-color: {}; color: white; padding: 20px; border-radius: 8px 8px 0 0; }}
        .header h1 {{ margin: 0; font-size: 24px; }}
        .content {{ padding: 20px; }}
        .alert-box {{ background: #f8f9fa; border-left: 4px solid {}; padding: 15px; margin: 15px 0; }}
        .label {{ font-weight: bold; color: #666; font-size: 12px; text-transform: uppercase; }}
        .value {{ font-size: 16px; margin-bottom: 15px; }}
        .details {{ background: #f8f9fa; padding: 10px; font-family: monospace; font-size: 12px; white-space: pre-wrap; }}
        .footer {{ padding: 15px 20px; background: #f8f9fa; border-top: 1px solid #dee2e6; font-size: 12px; color: #666; border-radius: 0 0 8px 8px; }}
    </style>
</head>
<body>
    <div class="container">
        <div class="header">
            <h1>HomeVault Alert: {}</h1>
        </div>
        <div class="content">
            <div class="alert-box">
                <div class="label">Severity</div>
                <div class="value">{}</div>
                <div class="label">Title</div>
                <div class="value">{}</div>
                <div class="label">Message</div>
                <div class="value">{}</div>
            </div>
            {}
        </div>
        <div class="footer">
            <p>This is an automated alert from HomeVault. Do not reply to this email.</p>
        </div>
    </div>
</body>
</html>"#,
            severity_color,
            severity_color,
            alert.severity,
            alert.severity,
            alert.title,
            alert.message,
            if let Some(details) = &alert.details {
                format!(
                    "<div class=\"label\">Details</div><div class=\"details\">{}</div>",
                    details
                )
            } else {
                String::new()
            }
        );

        let message = EmailMessage {
            to: recipients.to_vec(),
            subject: format!(
                "[{}] HomeVault: {}",
                alert.severity.to_uppercase(),
                alert.title
            ),
            body: html_body,
            is_html: true,
        };

        Self::send_email(&message, config)
    }

    /// Send a test email to verify the system is configured correctly,
    /// using the caller-supplied SMTP configuration.
    pub fn send_test_email(
        recipient: &EmailRecipient,
        config: &EmailConfig,
    ) -> Result<(), EmailError> {
        let html_body = r#"
<!DOCTYPE html>
<html>
<head>
    <meta charset="UTF-8">
    <style>
        body { font-family: Arial, sans-serif; margin: 0; padding: 20px; background-color: #f5f5f5; }
        .container { max-width: 600px; margin: 0 auto; background: white; border-radius: 8px; box-shadow: 0 2px 4px rgba(0,0,0,0.1); padding: 20px; }
        .success { color: #28a745; font-size: 48px; text-align: center; }
        .message { text-align: center; color: #666; }
    </style>
</head>
<body>
    <div class="container">
        <div class="success">&#10004;</div>
        <h2 class="message">HomeVault Email Configuration Test</h2>
        <p class="message">This is a test email from HomeVault. Your email configuration is working correctly.</p>
    </div>
</body>
</html>"#.to_string();

        let message = EmailMessage {
            to: vec![recipient.clone()],
            subject: "HomeVault Email Configuration Test".to_string(),
            body: html_body,
            is_html: true,
        };

        Self::send_email(&message, config)
    }

    /// Return `true` if at least one MTA binary (sendmail, msmtp, or mutt) is installed.
    pub fn is_email_configured() -> bool {
        Command::new("which")
            .arg("sendmail")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
            || Command::new("which")
                .arg("msmtp")
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false)
            || Command::new("which")
                .arg("mutt")
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false)
    }
}

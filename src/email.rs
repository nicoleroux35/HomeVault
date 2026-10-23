/* =============================================================================
 * File          : email.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 23/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Outgoing email for notifications: sends through whichever local mail
 *   transfer agent binary is available (mutt, then sendmail, then msmtp),
 *   with a deadline-aware child-process wait so a hung MTA cannot stall a
 *   notification. When the admin configured an SMTP relay in Settings → Email,
 *   a private msmtp configuration is generated from those values so the
 *   configured server/port/credentials/TLS are what is actually used.
 *
 * Dependencies:
 *   - serde (>= 1.0), thiserror (>= 2), tracing (>= 0.1)
 *
 * Usage:
 *   notification_engine.rs (alerts) and the email test endpoint.
 * =============================================================================
 */

//! Outgoing email via the host's available mail transfer agent binaries.
//!
//! Two modes, chosen by `EmailConfig::is_explicit()`:
//!
//! * **Relay configured** (a real `smtp_server`) — HomeVault writes a private
//!   `msmtp.conf` (mode `0600`) from the saved Settings → Email values and
//!   sends through `msmtp --file <path>`. This is what makes those fields
//!   mean something: before ROADMAP 13.3 they were persisted and ignored, and
//!   only `--from` reached msmtp, so the credentials had to be pre-configured
//!   in msmtp's own file by hand.
//! * **No relay** (blank, or `localhost`) — the historical behaviour: try the
//!   local `mutt` → `sendmail` → `msmtp` in order, exactly as a local MTA
//!   setup expects.

use serde::{Deserialize, Serialize};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};
use std::sync::OnceLock;
use std::time::{Duration, Instant};
use thiserror::Error;
use tracing::warn;

/// Maximum time (seconds) to wait for an external MTA process to complete.
const EMAIL_TIMEOUT_SECS: u64 = 30;

/// Mode of the generated msmtp configuration.
///
/// msmtp refuses a configuration file that is readable by anybody else (it
/// holds the SMTP password), so this is not cosmetic: `0644` would make every
/// send fail. Kept public so a test can pin it.
pub const MSMTP_CONFIG_MODE: u32 = 0o600;

/// How much of msmtp's stderr is quoted in a failure. Enough for
/// "authentication failed" / "connection refused", short enough not to paste a
/// wall of text into a notification row.
const MAX_MTA_STDERR: usize = 512;

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

impl EmailConfig {
    /// `true` when the admin named a real relay. A blank host — or the
    /// historical `localhost` default — means "use whatever local MTA the host
    /// has", which is the pre-13.3 behaviour and must keep working.
    pub fn is_explicit(&self) -> bool {
        let host = self.smtp_host.trim();
        !host.is_empty() && !host.eq_ignore_ascii_case("localhost")
    }

    /// Validate every field that is about to become a line of the generated
    /// msmtp configuration. A newline in any of them would inject a directive
    /// (`password`/`host`/…), so a control character is refused outright
    /// rather than escaped.
    pub fn validate(&self) -> Result<(), EmailError> {
        let host = self.smtp_host.trim();
        if host.is_empty() {
            return Err(EmailError::Config("SMTP server must not be empty".into()));
        }
        if host.len() > 255 {
            return Err(EmailError::Config("SMTP server is too long".into()));
        }
        if !host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | ':' | '[' | ']'))
        {
            return Err(EmailError::Config(
                "SMTP server must be a host name or an IP address".into(),
            ));
        }
        if self.smtp_port == 0 {
            return Err(EmailError::Config("SMTP port must be 1-65535".into()));
        }
        for (field, value) in [
            ("SMTP username", &self.smtp_username),
            ("SMTP password", &self.smtp_password),
            ("From address", &self.from_address),
        ] {
            if value.chars().any(|c| c.is_control()) {
                return Err(EmailError::Config(format!(
                    "{field} must not contain control characters"
                )));
            }
            if value.len() > 512 {
                return Err(EmailError::Config(format!("{field} is too long")));
            }
        }
        let from = self.from_address.trim();
        if from.is_empty() {
            return Err(EmailError::Config("From address must not be empty".into()));
        }
        Ok(())
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

/// Where the generated msmtp configuration lives.
///
/// Deliberately inside HomeVault's own data directory (`DATA_DIRECTORY` aware)
/// rather than `/etc/msmtprc`: the service account can write it without another
/// grant, the file never collides with an operator's own msmtp setup, and it is
/// produced from the saved settings on every send — so editing Settings → Email
/// takes effect immediately instead of "when someone updates /etc/msmtprc".
pub fn msmtp_config_path() -> PathBuf {
    crate::config::paths::data_dir().join("msmtp.conf")
}

/// Quote one value for msmtp's INI-ish parser.
///
/// Control characters are rejected by `EmailConfig::validate` before this is
/// reached; the escaping here only has to keep a `"`/`\` inside the value from
/// ending the string early.
fn quoted(value: &str) -> String {
    let escaped = value.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{escaped}\"")
}

/// Render the msmtp configuration for `config`.
///
/// Pure and unit-tested: this is where the saved settings actually become the
/// SMTP parameters, so the exact shape matters. `tls_starttls` is off for the
/// implicit-TLS port (465) and on otherwise, matching how the two are deployed
/// in practice.
pub fn msmtp_config_text(config: &EmailConfig) -> Result<String, EmailError> {
    config.validate()?;
    let mut text = String::from(
        "# Generated by HomeVault from Settings → Email. Do not edit this file:\n\
         # it is rewritten on every notification and load.\n\
         \n\
         account homevault\n",
    );
    text.push_str(&format!("host {}\n", config.smtp_host.trim()));
    text.push_str(&format!("port {}\n", config.smtp_port));
    let auth = !config.smtp_username.trim().is_empty();
    text.push_str(&format!("auth {}\n", if auth { "on" } else { "off" }));
    if auth {
        text.push_str(&format!("user {}\n", quoted(config.smtp_username.trim())));
        text.push_str(&format!("password {}\n", quoted(&config.smtp_password)));
    }
    if config.use_tls {
        text.push_str("tls on\n");
        text.push_str(&format!(
            "tls_starttls {}\n",
            if config.smtp_port == 465 { "off" } else { "on" }
        ));
        text.push_str("tls_trust_file system\n");
    } else {
        text.push_str("tls off\n");
    }
    text.push_str(&format!("from {}\n", quoted(config.from_address.trim())));
    text.push_str("\naccount default : homevault\n");
    Ok(text)
}

/// Write the generated configuration with mode `0600` (repairing a stale mode
/// left by an older build, exactly like the rclone credential file).
pub fn write_msmtp_config(config: &EmailConfig) -> Result<PathBuf, EmailError> {
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

    let text = msmtp_config_text(config)?;
    let path = msmtp_config_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(MSMTP_CONFIG_MODE)
        .open(&path)?;
    file.write_all(text.as_bytes())?;
    file.flush()?;
    // `mode()` only applies at creation, so re-apply it every time: a file
    // created 0644 by an earlier build must be repaired, not merely reused.
    let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(MSMTP_CONFIG_MODE));
    Ok(path)
}

/// The exact argv of one `msmtp` invocation. Pure so the security-relevant
/// shape (`--file` before the recipient, no shell) is unit-testable.
pub fn msmtp_argv<'a>(config_file: Option<&'a Path>, from: &'a str, to: &'a str) -> Vec<String> {
    let mut args = Vec::new();
    if let Some(path) = config_file {
        args.push("--file".to_string());
        args.push(path.to_string_lossy().to_string());
    }
    args.push("--from".to_string());
    args.push(from.to_string());
    args.push(to.to_string());
    args
}

/// Service for sending email via available MTA binaries (mutt, sendmail, msmtp).
pub struct EmailService;

impl EmailService {
    /// Send an email. With a configured relay, `msmtp` + the generated config
    /// comes first; otherwise `mutt → sendmail → msmtp` in order. The first
    /// success wins; the last failure is what the caller sees.
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

        if config.is_explicit() {
            // ROADMAP 13.3: the admin's relay is the configured channel, so it
            // is not silently skipped in favour of a local `mutt` whose own
            // configuration they never saw.
            return Self::send_via_msmtp(&to_addrs, &message.body, config);
        }

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
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| EmailError::Send(format!("mutt not found: {}", e)))?;

        if let Some(mut stdin) = child.stdin.take() {
            if stdin.write_all(body.as_bytes()).is_err() {
                warn!("Failed to write email body to mutt stdin");
            }
        }

        let output = child.wait_with_captured_stderr()?;

        if output.status.success() {
            Ok(())
        } else {
            Err(EmailError::Send(format!(
                "mutt failed: {}",
                tail(&output.stderr)
            )))
        }
    }

    /// Send via `sendmail`, piping the fully-formed raw email on stdin.
    fn send_via_sendmail(to: &str, content: &str) -> Result<(), EmailError> {
        let mut child = Command::new("sendmail")
            .args(["-f", "homevault@localhost"])
            .arg(to)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| EmailError::Send(format!("sendmail not found: {}", e)))?;

        if let Some(mut stdin) = child.stdin.take() {
            if stdin.write_all(content.as_bytes()).is_err() {
                warn!("Failed to write email content to sendmail stdin");
            }
        }

        let output = child.wait_with_captured_stderr()?;

        if output.status.success() {
            Ok(())
        } else {
            Err(EmailError::Send(format!(
                "sendmail failed: {}",
                tail(&output.stderr)
            )))
        }
    }

    /// Send via `msmtp`.
    ///
    /// With a configured relay the SMTP parameters come from a configuration
    /// generated from the saved settings (`--file`); without one msmtp reads its
    /// own default configuration, exactly as before. stderr is captured and
    /// quoted: "msmtp failed" alone is what made a wrong password
    /// indistinguishable from a missing binary.
    fn send_via_msmtp(to: &str, body: &str, config: &EmailConfig) -> Result<(), EmailError> {
        let config_file = if config.is_explicit() {
            Some(write_msmtp_config(config)?)
        } else {
            None
        };
        let from = config.from_address.trim();

        let mut child = Command::new("msmtp")
            .args(msmtp_argv(config_file.as_deref(), from, to))
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| EmailError::Send(format!("msmtp not found: {}", e)))?;

        if let Some(mut stdin) = child.stdin.take() {
            if stdin.write_all(body.as_bytes()).is_err() {
                warn!("Failed to write email body to msmtp stdin");
            }
        }

        let output = child.wait_with_captured_stderr()?;

        if output.status.success() {
            Ok(())
        } else {
            Err(EmailError::Send(format!(
                "msmtp failed: {}",
                tail(&output.stderr)
            )))
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

    /// Return `true` if at least one MTA binary (sendmail, msmtp, or mutt) is
    /// installed.
    ///
    /// ROADMAP 13.3: this used to fork `which` three times on every call. It is
    /// a file-existence question, answered by looking at the standard
    /// locations (`/usr/sbin` included — `sendmail` lives there and is not on
    /// the service account's `PATH`), and the answer is cached because a
    /// package install mid-process is not a case this status line has to
    /// handle.
    pub fn is_email_configured() -> bool {
        static CACHED: OnceLock<bool> = OnceLock::new();
        *CACHED.get_or_init(|| {
            const CANDIDATES: [&str; 8] = [
                "/usr/sbin/sendmail",
                "/usr/bin/sendmail",
                "/sbin/sendmail",
                "/usr/bin/msmtp",
                "/usr/sbin/msmtp",
                "/usr/local/bin/msmtp",
                "/usr/bin/mutt",
                "/usr/bin/neomutt",
            ];
            CANDIDATES.iter().any(|p| Path::new(p).exists())
        })
    }
}

/// Last `MAX_MTA_STDERR` bytes of an MTA's stderr, trimmed and single-lined.
fn tail(stderr: &[u8]) -> String {
    let text = String::from_utf8_lossy(stderr);
    let text = text.trim();
    if text.is_empty() {
        return "no diagnostic output".to_string();
    }
    let text = text.lines().last().unwrap_or(text).trim();
    if text.chars().count() <= MAX_MTA_STDERR {
        text.to_string()
    } else {
        text.chars().take(MAX_MTA_STDERR).collect::<String>() + "…"
    }
}

/// Extension trait: wait for a child with stderr captured, so a failure can be
/// explained instead of guessed.
trait WaitWithCapturedStderr {
    fn wait_with_captured_stderr(self) -> std::io::Result<std::process::Output>;
}

impl WaitWithCapturedStderr for std::process::Child {
    fn wait_with_captured_stderr(mut self) -> std::io::Result<std::process::Output> {
        // stderr is drained on its own thread: reading it inline would block
        // until the child exits and defeat the deadline below (a hung MTA is
        // the exact case this has to survive). On a timeout the child is
        // killed, which closes the pipe and lets the reader finish.
        use std::io::Read as _;
        let mut pipe = self.stderr.take();
        let reader = std::thread::spawn(move || {
            let mut buf = Vec::new();
            if let Some(pipe) = pipe.as_mut() {
                let _ = pipe.read_to_end(&mut buf);
            }
            buf
        });

        let status = self.wait_with_timeout();
        let stderr = reader.join().unwrap_or_default();
        Ok(std::process::Output {
            status: status?,
            stdout: Vec::new(),
            stderr,
        })
    }
}

#[cfg(test)]
mod tests {
    //! Pure tests for the relay path (ROADMAP 13.3): what the generated msmtp
    //! configuration contains, that the mode is `0600`, that injection through a
    //! field is refused, and that the legacy local-MTA mode is still selected
    //! when no relay is configured.
    //!
    //! Run: `cargo test email::tests`

    use super::*;

    fn relay() -> EmailConfig {
        EmailConfig {
            smtp_host: "smtp.example.com".to_string(),
            smtp_port: 587,
            smtp_username: "homevault@example.com".to_string(),
            smtp_password: "s3cr\"et\\pw".to_string(),
            from_address: "homevault@example.com".to_string(),
            from_name: "HomeVault".to_string(),
            use_tls: true,
        }
    }

    #[test]
    fn test_a_blank_or_localhost_host_keeps_the_local_mta_mode() {
        assert!(!EmailConfig::default().is_explicit());
        assert!(!EmailConfig {
            smtp_host: "  ".into(),
            ..relay()
        }
        .is_explicit());
        assert!(!EmailConfig {
            smtp_host: "LOCALHOST".into(),
            ..relay()
        }
        .is_explicit());
        assert!(relay().is_explicit());
    }

    #[test]
    fn test_msmtp_config_carries_the_configured_relay() {
        let text = msmtp_config_text(&relay()).expect("valid config");
        assert!(text.contains("host smtp.example.com"));
        assert!(text.contains("port 587"));
        assert!(text.contains("auth on"));
        assert!(text.contains("user \"homevault@example.com\""));
        assert!(text.contains("tls on"));
        assert!(text.contains("tls_starttls on"));
        assert!(text.contains("tls_trust_file system"));
        assert!(text.contains("account default : homevault"));
        // The password is written, quoted and escaped — it is the only place it
        // can reach msmtp.
        assert!(text.contains("password \"s3cr\\\"et\\\\pw\""));
        // No credentials leaks into the header comment.
        assert!(!text.starts_with("s3cr"));
    }

    #[test]
    fn test_msmtp_config_uses_implicit_tls_on_465() {
        let text = msmtp_config_text(&EmailConfig {
            smtp_port: 465,
            ..relay()
        })
        .expect("valid config");
        assert!(text.contains("tls_starttls off"));
    }

    #[test]
    fn test_msmtp_config_without_a_username_turns_auth_off() {
        let text = msmtp_config_text(&EmailConfig {
            smtp_username: String::new(),
            smtp_password: String::new(),
            ..relay()
        })
        .expect("valid config");
        assert!(text.contains("auth off"));
        assert!(!text.contains("password"));
    }

    #[test]
    fn test_msmtp_config_without_tls_is_plain() {
        let text = msmtp_config_text(&EmailConfig {
            use_tls: false,
            ..relay()
        })
        .expect("valid config");
        assert!(text.contains("tls off"));
        assert!(!text.contains("tls_starttls"));
        assert!(!text.contains("tls_trust_file"));
    }

    #[test]
    fn test_a_newline_in_any_field_is_refused_instead_of_injected() {
        for bad in [
            EmailConfig {
                smtp_host: "smtp.example.com\npassword evil".into(),
                ..relay()
            },
            EmailConfig {
                smtp_username: "me\nhost evil".into(),
                ..relay()
            },
            EmailConfig {
                from_address: "a@b\nfrom evil".into(),
                ..relay()
            },
            EmailConfig {
                smtp_host: "smtp example.com".into(),
                ..relay()
            },
            EmailConfig {
                smtp_port: 0,
                ..relay()
            },
        ] {
            assert!(
                msmtp_config_text(&bad).is_err(),
                "an injected/invalid field must be refused: {bad:?}"
            );
        }
    }

    #[test]
    fn test_msmtp_argv_puts_the_generated_file_first() {
        let path = Path::new("/tmp/msmtp.conf");
        assert_eq!(
            msmtp_argv(Some(path), "from@example.com", "to@example.com"),
            vec![
                "--file".to_string(),
                "/tmp/msmtp.conf".to_string(),
                "--from".to_string(),
                "from@example.com".to_string(),
                "to@example.com".to_string(),
            ]
        );
        // The legacy path keeps msmtp's own configuration.
        assert_eq!(
            msmtp_argv(None, "from@example.com", "to@example.com"),
            vec![
                "--from".to_string(),
                "from@example.com".to_string(),
                "to@example.com".to_string(),
            ]
        );
    }

    #[test]
    fn test_the_generated_config_is_private() {
        // 0600: msmtp refuses a group/other-readable file (it holds the
        // password), so this constant is load-bearing.
        assert_eq!(MSMTP_CONFIG_MODE, 0o600);
        let path = msmtp_config_path();
        assert!(path.ends_with("msmtp.conf"));
        assert!(path.parent().is_some(), "the path must be absolute");
    }

    #[test]
    fn test_the_mta_diagnostic_is_trimmed_and_bounded() {
        assert_eq!(tail(b""), "no diagnostic output");
        assert_eq!(
            tail(b"msmtp: authentication failed\n"),
            "msmtp: authentication failed"
        );
        let long = "x".repeat(MAX_MTA_STDERR + 10);
        let out = tail(long.as_bytes());
        assert!(out.ends_with('…'));
        assert_eq!(out.chars().count(), MAX_MTA_STDERR + 1);
    }
}

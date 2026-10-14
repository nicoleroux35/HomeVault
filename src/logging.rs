/* =============================================================================
 * File          : logging.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 28/09/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Global logger singleton implementing the log facade: writes application
 *   log lines to HomeVault's own rotating log files (the System Journal's
 *   application source) alongside the main tracing setup.
 *
 * Dependencies:
 *   - log (facade)
 *
 * Usage:
 *   Initialized once at startup; see docs/guides/journal.md.
 * =============================================================================
 */

use log::{Level, Log, Metadata, Record};
use std::sync::{LazyLock, RwLock};

/// Global singleton logger instance, lazily initialized.
static LOGGER: LazyLock<Logger> = LazyLock::new(Logger::new);

/// Return a reference to the global logger.
#[allow(dead_code)]
pub fn get_logger() -> &'static Logger {
    &LOGGER
}

/// Severity levels for the HomeVault logging system.
///
/// Ordered from least to most severe; only messages at or above the
/// configured minimum level are emitted.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum LogLevel {
    Trace = 0,
    Debug = 1,
    Info = 2,
    Warning = 3,
    Error = 4,
    Fatal = 5,
}

impl From<LogLevel> for Level {
    fn from(level: LogLevel) -> Self {
        match level {
            LogLevel::Trace => Level::Trace,
            LogLevel::Debug => Level::Debug,
            LogLevel::Info => Level::Info,
            LogLevel::Warning => Level::Warn,
            LogLevel::Error => Level::Error,
            LogLevel::Fatal => Level::Error,
        }
    }
}

impl From<Level> for LogLevel {
    fn from(level: Level) -> Self {
        match level {
            Level::Trace => LogLevel::Trace,
            Level::Debug => LogLevel::Debug,
            Level::Info => LogLevel::Info,
            Level::Warn => LogLevel::Warning,
            Level::Error => LogLevel::Error,
        }
    }
}

/// Thread-safe logger with configurable severity threshold and optional file output.
pub struct Logger {
    min_level: RwLock<LogLevel>,
    file_logging_enabled: RwLock<bool>,
    file_path: RwLock<String>,
}

impl Logger {
    /// Create a logger with `Info` as the default threshold and no file output.
    pub fn new() -> Self {
        Self {
            min_level: RwLock::new(LogLevel::Info),
            file_logging_enabled: RwLock::new(false),
            file_path: RwLock::new(String::new()),
        }
    }

    /// Set the minimum log level; messages below this level are silently dropped.
    pub fn set_log_level(&self, level: LogLevel) {
        *self.min_level.write().unwrap() = level;
    }

    /// Return the current minimum log level.
    pub fn get_log_level(&self) -> LogLevel {
        *self.min_level.read().unwrap()
    }

    /// Enable or disable logging to a file at `file_path`.
    pub fn enable_file_logging(&self, enabled: bool, file_path: &str) {
        *self.file_logging_enabled.write().unwrap() = enabled;
        *self.file_path.write().unwrap() = file_path.to_string();
    }

    /// Returns `true` if logging to a file is currently enabled.
    pub fn is_file_logging_enabled(&self) -> bool {
        *self.file_logging_enabled.read().unwrap()
    }

    /// Return the path of the log file, if one is configured.
    pub fn get_file_path(&self) -> String {
        self.file_path.read().unwrap().clone()
    }

    /// Log a message at `Debug` level.
    pub fn debug(&self, message: &str) {
        self.log(LogLevel::Debug, message);
    }

    /// Log a message at `Info` level.
    pub fn info(&self, message: &str) {
        self.log(LogLevel::Info, message);
    }

    /// Log a message at `Warning` level.
    pub fn warning(&self, message: &str) {
        self.log(LogLevel::Warning, message);
    }

    /// Log a message at `Error` level.
    pub fn error(&self, message: &str) {
        self.log(LogLevel::Error, message);
    }

    /// Log a message at `Fatal` level.
    pub fn fatal(&self, message: &str) {
        self.log(LogLevel::Fatal, message);
    }

    /// Core logging implementation: formats, prints to stderr/stdout, and optionally appends to file.
    fn log(&self, level: LogLevel, message: &str) {
        let min_level = *self.min_level.read().unwrap();
        if level < min_level {
            return;
        }

        let timestamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f");
        let level_str = match level {
            LogLevel::Trace => "TRACE",
            LogLevel::Debug => "DEBUG",
            LogLevel::Info => "INFO",
            LogLevel::Warning => "WARN",
            LogLevel::Error => "ERROR",
            LogLevel::Fatal => "FATAL",
        };

        let formatted = format!("[{}] [{}] {}", timestamp, level_str, message);

        match level {
            LogLevel::Fatal | LogLevel::Error => eprintln!("{}", formatted),
            _ => println!("{}", formatted),
        }

        if self.is_file_logging_enabled() {
            if let Ok(path) = self.file_path.read() {
                if !path.is_empty() {
                    let _ = std::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(&*path)
                        .and_then(|mut file| {
                            use std::io::Write;
                            writeln!(file, "{}", formatted)
                        });
                }
            }
        }
    }
}

impl Default for Logger {
    fn default() -> Self {
        Self::new()
    }
}

impl Log for Logger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        let level = LogLevel::from(metadata.level());
        level >= self.get_log_level()
    }

    fn log(&self, record: &Record) {
        if self.enabled(record.metadata()) {
            let level = LogLevel::from(record.level());
            self.log(level, &record.args().to_string());
        }
    }

    fn flush(&self) {}
}

/// Initialize the global logger with the given level, and optionally enable file output.
pub fn init_logging(level: LogLevel, file_logging: bool, file_path: &str) {
    LOGGER.set_log_level(level);
    if file_logging {
        LOGGER.enable_file_logging(true, file_path);
    }
}

/// Set the minimum log level on the global logger.
pub fn set_log_level(level: LogLevel) {
    LOGGER.set_log_level(level);
}

/// Return the current minimum log level of the global logger.
pub fn get_log_level() -> LogLevel {
    LOGGER.get_log_level()
}

/// Enable or disable file logging on the global logger.
pub fn enable_file_logging(enabled: bool, file_path: &str) {
    LOGGER.enable_file_logging(enabled, file_path);
}

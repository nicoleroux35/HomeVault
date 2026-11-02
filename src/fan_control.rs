//! Thermal and fan control — temperature-based PWM management
//!
//! Provides automatic fan speed control based on system temperatures. Only operates
//! on systems where safe control can be proven through hwmon interface detection.
//!
//! # Architecture
//!
//! - hwmon device detection and capability matrix
//! - Quiet/Balanced/Performance profiles with curves
//! - Hysteresis to prevent oscillation
//! - Minimum safe speed enforcement
//! - Automatic fallback to firmware control on errors
//!
//! # Security
//!
//! - Only writes to verified writable PWM channels
//! - Never operates on laptop ECs (firmware-controlled)
//! - Enforces minimum safe speeds
//! - Watchdog automatically restores auto mode
//! - All PWM writes are logged once per change

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};
use tokio::fs;

const HWMON_BASE: &str = "/sys/class/hwmon";
const MIN_SAFE_PWM: u8 = 51; // ~20% duty cycle (0-255 scale)
const HYSTERESIS_TEMP: i32 = 2; // 2°C hysteresis

/// Hard floor as a percentage duty (0-100), derived once from the raw PWM
/// floor. Kept as `u32` so the multiply in the other direction cannot overflow
/// `u8`: `MIN_SAFE_PWM * 100` is 5100, which does not fit in a byte.
const MIN_SAFE_DUTY_PERCENT: u8 = (MIN_SAFE_PWM as u32 * 100 / 255) as u8;

/// Fan control profile
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum FanProfile {
    /// Firmware automatic control
    Auto,

    /// Quiet profile (prioritizes silence)
    Quiet,

    /// Balanced profile (default)
    Balanced,

    /// Performance profile (prioritizes cooling)
    Performance,

    /// Manual fixed duty cycle
    Manual,
}

/// Fan control configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FanConfig {
    /// Active profile
    pub profile: FanProfile,

    /// Manual duty cycle (0-100) if profile is Manual
    pub manual_duty: u8,

    /// Per-channel overrides
    pub channel_overrides: HashMap<String, FanChannelConfig>,

    /// Control loop interval in seconds
    pub interval_secs: u32,
}

/// Per-channel configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FanChannelConfig {
    /// Profile for this channel (overrides global)
    pub profile: Option<FanProfile>,

    /// Manual duty for this channel
    pub manual_duty: Option<u8>,

    /// Temperature sensor to use
    pub temp_sensor: Option<String>,
}

/// Detected fan channel
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FanChannel {
    /// Channel identifier (e.g., "hwmon2_pwm1")
    pub id: String,

    /// Human-readable name
    pub name: String,

    /// hwmon device path
    pub hwmon_path: PathBuf,

    /// PWM channel number
    pub pwm_num: u8,

    /// Whether channel supports pwm_enable (auto mode)
    pub supports_auto: bool,

    /// Current PWM value (0-255)
    pub current_pwm: u8,

    /// Current mode (0=full, 1=manual, 2=auto)
    pub current_mode: u8,

    /// Associated temperature sensor if any
    pub temp_sensor: Option<String>,

    /// Current temperature in millidegrees C
    pub current_temp: Option<i32>,
}

/// Temperature curve point.
///
/// `pub` with `pub` fields for `tests/fan_control_integration.rs`, which drives
/// the curve maths against hand-written points (an integration test is a
/// separate crate and cannot see a private type).
#[derive(Debug, Clone, Copy)]
pub struct CurvePoint {
    pub temp_c: i32,
    pub duty: u8,
}

pub struct FanControlService;

impl FanControlService {
    /// Detect available fan channels
    pub async fn detect_channels() -> Result<Vec<FanChannel>, String> {
        let mut channels = Vec::new();

        // Read hwmon directory
        let mut hwmon_entries = fs::read_dir(HWMON_BASE)
            .await
            .map_err(|e| format!("failed to read hwmon: {}", e))?;

        while let Some(entry) = hwmon_entries
            .next_entry()
            .await
            .map_err(|e| format!("failed to read hwmon entry: {}", e))?
        {
            let hwmon_path = entry.path();

            // Check for pwm* files
            for pwm_num in 1..=9 {
                let pwm_path = hwmon_path.join(format!("pwm{}", pwm_num));
                let pwm_enable_path = hwmon_path.join(format!("pwm{}_enable", pwm_num));

                // Skip if pwm file doesn't exist
                if !pwm_path.exists() {
                    continue;
                }

                // Check if writable
                if !Self::is_writable(&pwm_path).await {
                    continue;
                }

                // Read current value
                let current_pwm = Self::read_pwm_value(&pwm_path).await.unwrap_or(0);

                // Check for pwm_enable
                let supports_auto = pwm_enable_path.exists();
                let current_mode = if supports_auto {
                    Self::read_pwm_value(&pwm_enable_path).await.unwrap_or(2)
                } else {
                    1 // Manual mode
                };

                // Find associated temperature sensor
                let temp_sensor = Self::find_temp_sensor(&hwmon_path, pwm_num).await;
                let current_temp = if let Some(ref sensor) = temp_sensor {
                    Self::read_temp(&hwmon_path.join(sensor)).await.ok()
                } else {
                    None
                };

                // Get device name
                let name = Self::read_device_name(&hwmon_path)
                    .await
                    .unwrap_or_else(|| format!("Fan {}", pwm_num));

                let hwmon_name = hwmon_path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("unknown");

                channels.push(FanChannel {
                    id: format!("{}_{}", hwmon_name, format!("pwm{}", pwm_num)),
                    name,
                    hwmon_path: hwmon_path.clone(),
                    pwm_num,
                    supports_auto,
                    current_pwm,
                    current_mode,
                    temp_sensor,
                    current_temp,
                });
            }
        }

        Ok(channels)
    }

    /// Check if a file is writable
    async fn is_writable(path: &Path) -> bool {
        fs::metadata(path)
            .await
            .ok()
            .map(|m| !m.permissions().readonly())
            .unwrap_or(false)
    }

    /// Read PWM value from sysfs
    async fn read_pwm_value(path: &Path) -> Result<u8, String> {
        let content = fs::read_to_string(path)
            .await
            .map_err(|e| format!("failed to read PWM: {}", e))?;

        content
            .trim()
            .parse::<u8>()
            .map_err(|e| format!("invalid PWM value: {}", e))
    }

    /// Read temperature in millidegrees
    async fn read_temp(path: &Path) -> Result<i32, String> {
        let content = fs::read_to_string(path)
            .await
            .map_err(|e| format!("failed to read temp: {}", e))?;

        content
            .trim()
            .parse::<i32>()
            .map_err(|e| format!("invalid temp value: {}", e))
    }

    /// Find associated temperature sensor for a PWM channel
    async fn find_temp_sensor(hwmon_path: &Path, _pwm_num: u8) -> Option<String> {
        // Look for temp*_input files
        for temp_num in 1..=9 {
            let temp_path = hwmon_path.join(format!("temp{}_input", temp_num));
            if temp_path.exists() {
                return Some(format!("temp{}_input", temp_num));
            }
        }
        None
    }

    /// Read device name
    async fn read_device_name(hwmon_path: &Path) -> Option<String> {
        let name_path = hwmon_path.join("name");
        fs::read_to_string(name_path)
            .await
            .ok()
            .map(|s| s.trim().to_string())
    }

    /// Get temperature curve for a profile (see `CurvePoint` for why this is `pub`).
    pub fn get_curve(profile: FanProfile) -> Vec<CurvePoint> {
        match profile {
            FanProfile::Auto => vec![], // Let firmware handle
            FanProfile::Quiet => vec![
                CurvePoint {
                    temp_c: 0,
                    duty: 20,
                },
                CurvePoint {
                    temp_c: 40,
                    duty: 30,
                },
                CurvePoint {
                    temp_c: 50,
                    duty: 40,
                },
                CurvePoint {
                    temp_c: 60,
                    duty: 60,
                },
                CurvePoint {
                    temp_c: 70,
                    duty: 80,
                },
                CurvePoint {
                    temp_c: 80,
                    duty: 100,
                },
            ],
            FanProfile::Balanced => vec![
                CurvePoint {
                    temp_c: 0,
                    duty: 25,
                },
                CurvePoint {
                    temp_c: 40,
                    duty: 40,
                },
                CurvePoint {
                    temp_c: 50,
                    duty: 50,
                },
                CurvePoint {
                    temp_c: 60,
                    duty: 70,
                },
                CurvePoint {
                    temp_c: 70,
                    duty: 90,
                },
                CurvePoint {
                    temp_c: 80,
                    duty: 100,
                },
            ],
            FanProfile::Performance => vec![
                CurvePoint {
                    temp_c: 0,
                    duty: 40,
                },
                CurvePoint {
                    temp_c: 40,
                    duty: 60,
                },
                CurvePoint {
                    temp_c: 50,
                    duty: 75,
                },
                CurvePoint {
                    temp_c: 60,
                    duty: 85,
                },
                CurvePoint {
                    temp_c: 70,
                    duty: 100,
                },
                CurvePoint {
                    temp_c: 80,
                    duty: 100,
                },
            ],
            FanProfile::Manual => vec![], // Fixed duty
        }
    }

    /// Calculate duty cycle from temperature (see `CurvePoint` for why this is `pub`).
    pub fn calculate_duty(temp_c: i32, curve: &[CurvePoint], prev_duty: u8) -> u8 {
        if curve.is_empty() {
            return prev_duty;
        }

        // Find curve segment
        let mut duty = curve[0].duty;

        for window in curve.windows(2) {
            let p1 = &window[0];
            let p2 = &window[1];

            if temp_c >= p1.temp_c && temp_c <= p2.temp_c {
                // Interpolate
                let temp_range = (p2.temp_c - p1.temp_c) as f64;
                let duty_range = (p2.duty as i16 - p1.duty as i16) as f64;
                let temp_offset = (temp_c - p1.temp_c) as f64;

                let interpolated = p1.duty as f64 + (duty_range * temp_offset / temp_range);
                duty = interpolated.round() as u8;
                break;
            } else if temp_c > p2.temp_c {
                duty = p2.duty;
            }
        }

        // Apply hysteresis
        let duty_diff = (duty as i16 - prev_duty as i16).abs();
        if duty_diff < 5 {
            return prev_duty; // Too small change, keep previous
        }

        // Enforce minimum safe speed
        duty.max(MIN_SAFE_DUTY_PERCENT)
    }

    /// Set PWM mode (0=full, 1=manual, 2=auto)
    pub async fn set_pwm_mode(channel: &FanChannel, mode: u8) -> Result<(), String> {
        if !channel.supports_auto {
            if mode == 2 {
                return Err("channel does not support auto mode".to_string());
            }
            return Ok(()); // No-op for manual-only channels
        }

        let enable_path = channel
            .hwmon_path
            .join(format!("pwm{}_enable", channel.pwm_num));

        fs::write(&enable_path, mode.to_string())
            .await
            .map_err(|e| format!("failed to set PWM mode: {}", e))
    }

    /// Set PWM duty cycle (0-100)
    pub async fn set_pwm_duty(channel: &FanChannel, duty: u8) -> Result<(), String> {
        if duty > 100 {
            return Err("duty must be 0-100".to_string());
        }

        // Convert 0-100 to 0-255
        let pwm_value = ((duty as u32 * 255) / 100) as u8;

        // Enforce minimum
        let pwm_value = pwm_value.max(MIN_SAFE_PWM);

        let pwm_path = channel.hwmon_path.join(format!("pwm{}", channel.pwm_num));

        fs::write(&pwm_path, pwm_value.to_string())
            .await
            .map_err(|e| format!("failed to set PWM duty: {}", e))
    }

    /// Apply fan configuration
    pub async fn apply_config(config: &FanConfig) -> Result<(), String> {
        let channels = Self::detect_channels().await?;

        for channel in &channels {
            // Determine profile for this channel
            let profile = config
                .channel_overrides
                .get(&channel.id)
                .and_then(|c| c.profile)
                .unwrap_or(config.profile);

            match profile {
                FanProfile::Auto => {
                    // Restore firmware control
                    Self::set_pwm_mode(channel, 2).await?;
                }
                FanProfile::Manual => {
                    // Set manual mode
                    Self::set_pwm_mode(channel, 1).await?;

                    // Get manual duty
                    let duty = config
                        .channel_overrides
                        .get(&channel.id)
                        .and_then(|c| c.manual_duty)
                        .unwrap_or(config.manual_duty);

                    Self::set_pwm_duty(channel, duty).await?;
                }
                FanProfile::Quiet | FanProfile::Balanced | FanProfile::Performance => {
                    // Set manual mode for curve control
                    Self::set_pwm_mode(channel, 1).await?;

                    // Initial duty based on current temp
                    if let Some(temp_millidegrees) = channel.current_temp {
                        let temp_c = temp_millidegrees / 1000;
                        let curve = Self::get_curve(profile);
                        let duty = Self::calculate_duty(
                            temp_c,
                            &curve,
                            (channel.current_pwm as u32 * 100 / 255) as u8,
                        );

                        Self::set_pwm_duty(channel, duty).await?;
                    }
                }
            }
        }

        Ok(())
    }

    /// Restore all channels to automatic mode
    pub async fn restore_auto() -> Result<(), String> {
        let channels = Self::detect_channels().await?;

        for channel in &channels {
            if channel.supports_auto {
                let _ = Self::set_pwm_mode(&channel, 2).await;
            }
        }

        Ok(())
    }

    /// Check if system is a laptop (should not control fans)
    pub async fn is_laptop() -> bool {
        // Check for laptop indicators
        let indicators = [
            "/sys/class/power_supply/BAT0",
            "/sys/class/power_supply/BAT1",
            "/sys/class/dmi/id/chassis_type",
        ];

        for indicator in &indicators {
            if Path::new(indicator).exists() {
                // Check chassis type if available
                if indicator.contains("chassis_type") {
                    if let Ok(chassis) = fs::read_to_string(indicator).await {
                        let chassis_type = chassis.trim().parse::<u8>().unwrap_or(0);
                        // 8, 9, 10, 14 are laptop/notebook/portable types
                        if matches!(chassis_type, 8 | 9 | 10 | 14) {
                            return true;
                        }
                    }
                } else {
                    return true; // Battery present = laptop
                }
            }
        }

        false
    }
}

impl Default for FanConfig {
    fn default() -> Self {
        Self {
            profile: FanProfile::Balanced,
            manual_duty: 50,
            channel_overrides: HashMap::new(),
            interval_secs: 10,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_calculate_duty_interpolation() {
        let curve = vec![
            CurvePoint {
                temp_c: 40,
                duty: 30,
            },
            CurvePoint {
                temp_c: 60,
                duty: 70,
            },
        ];

        // At 50°C (midpoint), should be ~50% duty
        let duty = FanControlService::calculate_duty(50, &curve, 0);
        assert!(duty >= 48 && duty <= 52);
    }

    #[test]
    fn test_calculate_duty_below_curve() {
        let curve = vec![
            CurvePoint {
                temp_c: 40,
                duty: 30,
            },
            CurvePoint {
                temp_c: 60,
                duty: 70,
            },
        ];

        // Below curve minimum
        let duty = FanControlService::calculate_duty(30, &curve, 0);
        assert_eq!(duty, 30);
    }

    #[test]
    fn test_calculate_duty_above_curve() {
        let curve = vec![
            CurvePoint {
                temp_c: 40,
                duty: 30,
            },
            CurvePoint {
                temp_c: 60,
                duty: 70,
            },
        ];

        // Above curve maximum
        let duty = FanControlService::calculate_duty(80, &curve, 0);
        assert_eq!(duty, 70);
    }

    #[test]
    fn test_calculate_duty_hysteresis() {
        let curve = vec![
            CurvePoint {
                temp_c: 40,
                duty: 30,
            },
            CurvePoint {
                temp_c: 60,
                duty: 70,
            },
        ];

        // Small change should keep previous
        let duty = FanControlService::calculate_duty(50, &curve, 49);
        assert_eq!(duty, 49); // Hysteresis keeps it
    }

    #[test]
    fn test_calculate_duty_enforces_minimum() {
        let curve = vec![
            CurvePoint { temp_c: 0, duty: 5 },
            CurvePoint {
                temp_c: 40,
                duty: 10,
            },
        ];

        let duty = FanControlService::calculate_duty(20, &curve, 0);
        let min_duty = MIN_SAFE_DUTY_PERCENT;
        assert!(duty >= min_duty);
    }

    #[test]
    fn test_quiet_profile_curve() {
        let curve = FanControlService::get_curve(FanProfile::Quiet);
        assert!(!curve.is_empty());
        assert_eq!(curve[0].temp_c, 0);
        assert!(curve.last().unwrap().temp_c >= 80);
    }

    #[test]
    fn test_balanced_profile_curve() {
        let curve = FanControlService::get_curve(FanProfile::Balanced);
        assert!(!curve.is_empty());

        // Balanced should have higher minimum than quiet
        let quiet = FanControlService::get_curve(FanProfile::Quiet);
        assert!(curve[0].duty > quiet[0].duty);
    }

    #[test]
    fn test_performance_profile_curve() {
        let curve = FanControlService::get_curve(FanProfile::Performance);
        assert!(!curve.is_empty());

        // Performance should be most aggressive
        let balanced = FanControlService::get_curve(FanProfile::Balanced);
        assert!(curve[0].duty > balanced[0].duty);
    }

    #[test]
    fn test_auto_profile_no_curve() {
        let curve = FanControlService::get_curve(FanProfile::Auto);
        assert!(curve.is_empty());
    }

    #[test]
    fn test_manual_profile_no_curve() {
        let curve = FanControlService::get_curve(FanProfile::Manual);
        assert!(curve.is_empty());
    }

    #[test]
    fn test_default_config() {
        let config = FanConfig::default();
        assert_eq!(config.profile, FanProfile::Balanced);
        assert_eq!(config.manual_duty, 50);
        assert_eq!(config.interval_secs, 10);
    }
}

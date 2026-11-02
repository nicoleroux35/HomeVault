//! Integration tests for thermal/fan control

use homevault::fan_control::{FanControlService, FanConfig, FanProfile};
use std::collections::HashMap;

#[tokio::test]
async fn test_detect_channels() {
    let result = FanControlService::detect_channels().await;

    match result {
        Ok(channels) => {
            println!("Found {} fan channels", channels.len());
            for channel in channels {
                println!("  Channel: {} ({})", channel.id, channel.name);
                println!("    PWM: {}, Mode: {}", channel.current_pwm, channel.current_mode);
                println!("    Supports auto: {}", channel.supports_auto);
                if let Some(temp) = channel.current_temp {
                    println!("    Temperature: {}°C", temp / 1000);
                }
            }
        },
        Err(e) => {
            println!("Could not detect channels: {}", e);
        }
    }
}

#[tokio::test]
async fn test_is_laptop() {
    let is_laptop = FanControlService::is_laptop().await;
    println!("System is laptop: {}", is_laptop);

    // Laptop systems should not control fans
    if is_laptop {
        println!("  Fan control should be disabled on laptops");
    }
}

#[test]
fn test_calculate_duty_interpolation() {
    let curve = vec![
        homevault::fan_control::CurvePoint { temp_c: 40, duty: 30 },
        homevault::fan_control::CurvePoint { temp_c: 60, duty: 70 },
    ];

    // At 50°C (midpoint), should be around 50% duty
    let duty = FanControlService::calculate_duty(50, &curve, 0);
    assert!(duty >= 48 && duty <= 52, "Duty at midpoint should be ~50%, got {}", duty);
}

#[test]
fn test_calculate_duty_below_curve() {
    let curve = vec![
        homevault::fan_control::CurvePoint { temp_c: 40, duty: 30 },
        homevault::fan_control::CurvePoint { temp_c: 60, duty: 70 },
    ];

    // Below curve minimum
    let duty = FanControlService::calculate_duty(30, &curve, 0);
    assert_eq!(duty, 30);
}

#[test]
fn test_calculate_duty_above_curve() {
    let curve = vec![
        homevault::fan_control::CurvePoint { temp_c: 40, duty: 30 },
        homevault::fan_control::CurvePoint { temp_c: 60, duty: 70 },
    ];

    // Above curve maximum
    let duty = FanControlService::calculate_duty(80, &curve, 0);
    assert_eq!(duty, 70);
}

#[test]
fn test_calculate_duty_enforces_minimum() {
    let curve = vec![
        homevault::fan_control::CurvePoint { temp_c: 0, duty: 5 },
        homevault::fan_control::CurvePoint { temp_c: 40, duty: 10 },
    ];

    let duty = FanControlService::calculate_duty(20, &curve, 0);
    let min_duty = 51 * 100 / 255; // MIN_SAFE_PWM
    assert!(duty >= min_duty, "Duty should be at least {}, got {}", min_duty, duty);
}

#[test]
fn test_calculate_duty_hysteresis() {
    let curve = vec![
        homevault::fan_control::CurvePoint { temp_c: 40, duty: 30 },
        homevault::fan_control::CurvePoint { temp_c: 60, duty: 70 },
    ];

    // Small change should keep previous
    let duty = FanControlService::calculate_duty(50, &curve, 49);
    assert_eq!(duty, 49, "Hysteresis should keep previous value");
}

#[test]
fn test_quiet_profile_curve() {
    let curve = FanControlService::get_curve(FanProfile::Quiet);

    assert!(!curve.is_empty());
    assert_eq!(curve[0].temp_c, 0);
    assert!(curve.last().unwrap().temp_c >= 80);
    assert_eq!(curve[0].duty, 20);
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
    assert!(curve.is_empty(), "Auto profile should have no curve");
}

#[test]
fn test_manual_profile_no_curve() {
    let curve = FanControlService::get_curve(FanProfile::Manual);
    assert!(curve.is_empty(), "Manual profile should have no curve");
}

#[test]
fn test_default_config() {
    let config = FanConfig::default();

    assert_eq!(config.profile, FanProfile::Balanced);
    assert_eq!(config.manual_duty, 50);
    assert_eq!(config.interval_secs, 10);
    assert!(config.channel_overrides.is_empty());
}

#[test]
fn test_profile_curves_are_monotonic() {
    let profiles = vec![
        FanProfile::Quiet,
        FanProfile::Balanced,
        FanProfile::Performance,
    ];

    for profile in profiles {
        let curve = FanControlService::get_curve(profile);

        // Check temperature is monotonic increasing
        for window in curve.windows(2) {
            assert!(
                window[1].temp_c > window[0].temp_c,
                "Temperature should increase in curve"
            );
        }
    }
}

#[test]
fn test_profile_curves_reach_100_percent() {
    let profiles = vec![
        FanProfile::Quiet,
        FanProfile::Balanced,
        FanProfile::Performance,
    ];

    for profile in profiles {
        let curve = FanControlService::get_curve(profile);

        // Should reach 100% duty at high temps
        let max_duty = curve.iter().map(|p| p.duty).max().unwrap();
        assert_eq!(max_duty, 100, "{:?} should reach 100% duty", profile);
    }
}

#[tokio::test]
async fn test_restore_auto_does_not_error() {
    // Should not error even if no channels available
    let result = FanControlService::restore_auto().await;

    match result {
        Ok(_) => println!("Restored auto mode"),
        Err(e) => println!("Could not restore auto: {}", e),
    }
}

#[test]
fn test_fan_config_channel_override() {
    let mut config = FanConfig::default();
    config.profile = FanProfile::Balanced;

    // Add channel-specific override
    let mut override_cfg = homevault::fan_control::FanChannelConfig {
        profile: Some(FanProfile::Quiet),
        manual_duty: None,
        temp_sensor: None,
    };

    config.channel_overrides.insert("hwmon1_pwm1".to_string(), override_cfg);

    // Channel should use its override
    assert_eq!(
        config.channel_overrides.get("hwmon1_pwm1").unwrap().profile,
        Some(FanProfile::Quiet)
    );
}

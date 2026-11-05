/* =============================================================================
 * File          : disk_prediction.rs
 * Author        : Nicolas Le Roux
 * Created       : 05/11/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Failure-trend estimate from the SMART history collected by P0-8
 *   (tasklist P2-10). A regression over 5/187/188/197/198 and temperature
 *   yields a trend and an estimated number of days until a configured
 *   threshold. A single sample — or a stable attribute — never produces a
 *   warning.
 *
 * Dependencies:
 *   - chrono, crate::storage::disk_health_repo
 *
 * Usage:
 *   GET /api/health/disk-health/prediction?serial=…; see docs/guides/health.md.
 * =============================================================================
 */

//! SMART regression-based failure prediction.

use chrono::{DateTime, Utc};
use serde::Serialize;

use crate::storage::disk_health_repo::{DiskHealthRepository, DiskHealthSample};

/// Minimum samples before a slope is trusted. Five daily points is already a
/// week of history; anything less is noise.
pub const MIN_POINTS: usize = 5;

/// Attributes that indicate wear when they grow. `None` threshold means the
/// value is informational (temperature uses its own policy).
pub fn threshold_for(attr_id: i64) -> Option<f64> {
    match attr_id {
        5 => Some(10.0),   // Reallocated_Sector_Ct
        187 => Some(1.0),  // Reported_Uncorrect
        188 => Some(1.0),  // Command_Timeout
        197 => Some(1.0),  // Current_Pending_Sector
        198 => Some(1.0),  // Offline_Uncorrectable
        194 => Some(60.0), // Temperature_Celsius
        _ => None,
    }
}

/// One `(days since first sample, raw value)` point.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Point {
    pub days: f64,
    pub value: f64,
}

/// The trend of one attribute.
#[derive(Debug, Clone, Copy, Serialize, PartialEq)]
pub struct Trend {
    pub attr_id: i64,
    pub slope_per_day: f64,
    pub r_squared: f64,
    /// Days until the threshold, when the slope is positive and the fit is
    /// not flat.
    pub days_remaining: Option<f64>,
    /// 95 %-ish interval around `days_remaining` from the residual error.
    pub days_low: Option<f64>,
    pub days_high: Option<f64>,
}

/// Ordinary least squares. Returns `(slope, intercept, r_squared, std_error)`.
pub fn linear_fit(points: &[Point]) -> Option<(f64, f64, f64, f64)> {
    if points.len() < 2 {
        return None;
    }
    let n = points.len() as f64;
    let mean_x = points.iter().map(|p| p.days).sum::<f64>() / n;
    let mean_y = points.iter().map(|p| p.value).sum::<f64>() / n;
    let sxx: f64 = points.iter().map(|p| (p.days - mean_x).powi(2)).sum();
    if sxx <= f64::EPSILON {
        return None;
    }
    let sxy: f64 = points
        .iter()
        .map(|p| (p.days - mean_x) * (p.value - mean_y))
        .sum();
    let slope = sxy / sxx;
    let intercept = mean_y - slope * mean_x;
    let ss_tot: f64 = points.iter().map(|p| (p.value - mean_y).powi(2)).sum();
    let ss_res: f64 = points
        .iter()
        .map(|p| (p.value - (intercept + slope * p.days)).powi(2))
        .sum();
    let r_squared = if ss_tot <= f64::EPSILON {
        1.0
    } else {
        (1.0 - ss_res / ss_tot).clamp(0.0, 1.0)
    };
    let std_error = if points.len() > 2 {
        (ss_res / (n - 2.0)).sqrt()
    } else {
        f64::INFINITY
    };
    Some((slope, intercept, r_squared, std_error))
}

/// Estimate the days until `threshold`. Requires [`MIN_POINTS`], a positive
/// slope and a fit that explains at least a quarter of the variance.
pub fn predict(points: &[Point], attr_id: i64, threshold: f64) -> Option<Trend> {
    if points.len() < MIN_POINTS {
        return None;
    }
    let (slope, intercept, r_squared, std_error) = linear_fit(points)?;
    if slope <= 0.0 || r_squared < 0.25 {
        return None;
    }
    let current = intercept + slope * points.last()?.days;
    if current >= threshold {
        // Already at the threshold: nothing to estimate, the finding itself
        // is the alert.
        return None;
    }
    let days = (threshold - current) / slope;
    // Propagate the residual error as a rough interval: a noisy series gets a
    // wide one instead of a falsely precise number.
    let span = if std_error.is_finite() {
        (1.96 * std_error / slope).min(days.max(1.0))
    } else {
        days
    };
    Some(Trend {
        attr_id,
        slope_per_day: slope,
        r_squared,
        days_remaining: Some(days.max(0.0)),
        days_low: Some((days - span).max(0.0)),
        days_high: Some(days + span),
    })
}

/// Build the points of one attribute from ascending samples.
pub fn points_from_samples(samples: &[DiskHealthSample]) -> Vec<Point> {
    let mut dated: Vec<(DateTime<Utc>, f64)> = samples
        .iter()
        .filter_map(|s| {
            let at = DateTime::parse_from_rfc3339(&s.sampled_at)
                .ok()?
                .with_timezone(&Utc);
            Some((at, s.raw as f64))
        })
        .collect();
    dated.sort_by_key(|(at, _)| *at);
    let Some((first, _)) = dated.first() else {
        return Vec::new();
    };
    dated
        .iter()
        .map(|(at, value)| Point {
            days: at.signed_duration_since(*first).num_seconds() as f64 / 86_400.0,
            value: *value,
        })
        .collect()
}

/// Every trusted trend for one disk.
pub async fn predictions_for(
    repo: &DiskHealthRepository,
    serial: &str,
) -> Result<Vec<Trend>, String> {
    let samples = repo
        .list_samples(serial, 5_000)
        .await
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for attr_id in [5i64, 187, 188, 194, 197, 198] {
        let Some(threshold) = threshold_for(attr_id) else {
            continue;
        };
        let subset: Vec<DiskHealthSample> = samples
            .iter()
            .filter(|s| s.attr_id == attr_id)
            .cloned()
            .collect();
        let points = points_from_samples(&subset);
        if let Some(trend) = predict(&points, attr_id, threshold) {
            out.push(trend);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(attr: i64, raw: i64, day: u32) -> DiskHealthSample {
        DiskHealthSample {
            id: day as i64,
            serial: "S1".to_string(),
            model: "X".to_string(),
            attr_id: attr,
            raw,
            normalized: Some(100),
            sampled_at: format!("2026-10-{:02}T00:00:00Z", day + 1),
        }
    }

    fn points(values: &[f64]) -> Vec<Point> {
        values
            .iter()
            .enumerate()
            .map(|(i, value)| Point {
                days: i as f64,
                value: *value,
            })
            .collect()
    }

    #[test]
    fn a_single_point_never_produces_a_trend() {
        assert!(predict(&points(&[4.0]), 197, 1.0).is_none());
        assert!(predict(&points(&[4.0, 5.0, 6.0, 7.0]), 197, 1.0).is_none());
    }

    #[test]
    fn a_stable_attribute_does_not_warn() {
        let stable = points(&[3.0; 10]);
        assert!(predict(&stable, 5, 10.0).is_none());
    }

    #[test]
    fn a_rising_attribute_estimates_days_until_the_threshold() {
        let rising = points(&[0.0, 0.2, 0.4, 0.6, 0.8, 1.0]);
        // Threshold 10 is 50 days away at 0.2/day.
        let trend = predict(&rising, 5, 10.0).expect("trend");
        assert!(trend.slope_per_day > 0.0);
        assert!(trend.r_squared > 0.9);
        let days = trend.days_remaining.unwrap();
        assert!((days - 50.0).abs() < 1.0, "{days}");
        assert!(trend.days_low.unwrap() <= days);
        assert!(trend.days_high.unwrap() >= days);
    }

    #[test]
    fn a_noisy_series_is_rejected_when_the_fit_is_poor() {
        let noisy = points(&[0.0, 8.0, 1.0, 9.0, 0.0, 10.0]);
        assert!(predict(&noisy, 5, 10.0).is_none());
    }

    #[test]
    fn samples_are_converted_to_days_since_the_first() {
        let samples = vec![
            sample(5, 1, 2),
            sample(5, 0, 0),
            sample(5, 2, 4),
            sample(5, 99, 4),
        ];
        let points = points_from_samples(&samples);
        assert_eq!(points.len(), 4);
        assert_eq!(points[0].days, 0.0);
        assert_eq!(points[1].days, 2.0);
        assert_eq!(points[2].value, 2.0);
    }
}

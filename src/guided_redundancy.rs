//! Guided redundancy profiles — SHR-like layout recommendations
//!
//! Provides intelligent RAID layout suggestions optimized for mixed disk sizes,
//! similar to Synology Hybrid RAID but using standard mdadm/ZFS/Btrfs.
//!
//! # Architecture
//!
//! - Capacity calculations for various RAID levels
//! - Layout recommendations based on disk inventory
//! - Risk assessment for each option
//! - No proprietary metadata - uses standard tools
//!
//! # Principles
//!
//! - Transparent: shows exact layout before creation
//! - Standard: uses mdadm/ZFS/Btrfs, readable elsewhere
//! - Conservative: prefers safety over maximum capacity

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Disk in the system
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiskInfo {
    /// Device path
    pub device: String,

    /// Size in bytes
    pub size_bytes: u64,

    /// Model name
    pub model: Option<String>,

    /// Whether disk is already in use
    pub in_use: bool,
}

/// Redundancy level
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, PartialOrd)]
#[serde(rename_all = "lowercase")]
pub enum RedundancyLevel {
    /// No redundancy (RAID 0, concatenation)
    None,

    /// Single disk failure tolerance
    Single,

    /// Double disk failure tolerance
    Double,
}

/// Storage technology
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum StorageTech {
    /// Linux mdadm
    Mdadm,

    /// ZFS pool
    Zfs,

    /// Btrfs filesystem
    Btrfs,
}

/// Layout recommendation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LayoutRecommendation {
    /// Recommendation ID
    pub id: String,

    /// Human-readable name
    pub name: String,

    /// Description of what this layout provides
    pub description: String,

    /// Storage technology
    pub technology: StorageTech,

    /// RAID level or equivalent
    pub level: String,

    /// Redundancy capability
    pub redundancy: RedundancyLevel,

    /// Usable capacity in bytes
    pub usable_bytes: u64,

    /// Raw capacity in bytes
    pub raw_bytes: u64,

    /// Efficiency percentage
    pub efficiency_pct: f64,

    /// Risk assessment
    pub risk: RiskAssessment,

    /// Exact disk layout
    pub layout: DiskLayout,

    /// Warnings
    pub warnings: Vec<String>,

    /// Recommendation score (higher is better)
    pub score: u32,
}

/// Risk assessment
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RiskAssessment {
    /// Data loss risk level
    pub data_loss_risk: RiskLevel,

    /// Rebuild time estimate
    pub rebuild_time: RebuildTime,

    /// Can tolerate N disk failures
    pub failure_tolerance: u8,

    /// Explanation
    pub explanation: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum RiskLevel {
    Low,
    Medium,
    High,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RebuildTime {
    Fast,     // < 2 hours
    Moderate, // 2-8 hours
    Slow,     // 8-24 hours
    VerySlow, // > 24 hours
}

/// Disk layout specification
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiskLayout {
    /// Disk groups (for RAID 10, etc.)
    pub groups: Vec<DiskGroup>,

    /// Metadata/parity allocation
    pub metadata_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiskGroup {
    /// Disks in this group
    pub disks: Vec<String>,

    /// Role (mirror, stripe, parity, etc.)
    pub role: String,
}

pub struct GuidedRedundancyService;

impl GuidedRedundancyService {
    /// Generate layout recommendations for given disks
    pub fn recommend_layouts(
        disks: &[DiskInfo],
        min_redundancy: RedundancyLevel,
    ) -> Vec<LayoutRecommendation> {
        let mut recommendations = Vec::new();

        // Filter available disks
        let available: Vec<&DiskInfo> = disks.iter().filter(|d| !d.in_use).collect();

        if available.is_empty() {
            return recommendations;
        }

        let disk_count = available.len();

        // Sort by size for layout calculations
        let mut sorted_disks = available.clone();
        sorted_disks.sort_by_key(|d| d.size_bytes);

        // Generate recommendations based on disk count
        match disk_count {
            1 => {
                // Single disk - no redundancy possible
                if min_redundancy == RedundancyLevel::None {
                    recommendations.push(Self::single_disk_layout(&sorted_disks));
                }
            }
            2 => {
                // RAID 1 (mirror)
                if min_redundancy <= RedundancyLevel::Single {
                    recommendations.push(Self::raid1_layout(&sorted_disks));
                }

                // RAID 0 if no redundancy required
                if min_redundancy == RedundancyLevel::None {
                    recommendations.push(Self::raid0_layout(&sorted_disks));
                }
            }
            3 => {
                // RAID 5 (recommended)
                if min_redundancy <= RedundancyLevel::Single {
                    recommendations.push(Self::raid5_layout(&sorted_disks));
                }

                // RAID 1 with spare
                if min_redundancy <= RedundancyLevel::Single {
                    recommendations.push(Self::raid1_with_spare_layout(&sorted_disks));
                }
            }
            _ => {
                // 4+ disks

                // RAID 10 if even number
                if disk_count % 2 == 0 && min_redundancy <= RedundancyLevel::Single {
                    recommendations.push(Self::raid10_layout(&sorted_disks));
                }

                // RAID 6 (double parity)
                if disk_count >= 4 && min_redundancy <= RedundancyLevel::Double {
                    recommendations.push(Self::raid6_layout(&sorted_disks));
                }

                // RAID 5
                if min_redundancy <= RedundancyLevel::Single {
                    recommendations.push(Self::raid5_layout(&sorted_disks));
                }
            }
        }

        // Score recommendations
        for rec in &mut recommendations {
            rec.score = Self::score_recommendation(rec, disk_count);
        }

        // Sort by score (highest first)
        recommendations.sort_by_key(|r| std::cmp::Reverse(r.score));

        recommendations
    }

    /// Single disk layout
    fn single_disk_layout(disks: &[&DiskInfo]) -> LayoutRecommendation {
        let disk = disks[0];
        let usable = disk.size_bytes;

        LayoutRecommendation {
            id: "single".to_string(),
            name: "Single Disk".to_string(),
            description: "No redundancy - all space available".to_string(),
            technology: StorageTech::Btrfs,
            level: "single".to_string(),
            redundancy: RedundancyLevel::None,
            usable_bytes: usable,
            raw_bytes: usable,
            efficiency_pct: 100.0,
            risk: RiskAssessment {
                data_loss_risk: RiskLevel::High,
                rebuild_time: RebuildTime::Fast,
                failure_tolerance: 0,
                explanation: "Any disk failure causes total data loss".to_string(),
            },
            layout: DiskLayout {
                groups: vec![DiskGroup {
                    disks: vec![disk.device.clone()],
                    role: "data".to_string(),
                }],
                metadata_bytes: 0,
            },
            warnings: vec!["No redundancy: disk failure = data loss".to_string()],
            score: 10,
        }
    }

    /// RAID 0 layout
    fn raid0_layout(disks: &[&DiskInfo]) -> LayoutRecommendation {
        let total: u64 = disks.iter().map(|d| d.size_bytes).sum();

        LayoutRecommendation {
            id: "raid0".to_string(),
            name: "Maximum Capacity (RAID 0)".to_string(),
            description: "Stripes data across all disks for maximum capacity and speed".to_string(),
            technology: StorageTech::Mdadm,
            level: "0".to_string(),
            redundancy: RedundancyLevel::None,
            usable_bytes: total,
            raw_bytes: total,
            efficiency_pct: 100.0,
            risk: RiskAssessment {
                data_loss_risk: RiskLevel::High,
                rebuild_time: RebuildTime::Fast,
                failure_tolerance: 0,
                explanation: "Any single disk failure destroys all data".to_string(),
            },
            layout: DiskLayout {
                groups: vec![DiskGroup {
                    disks: disks.iter().map(|d| d.device.clone()).collect(),
                    role: "stripe".to_string(),
                }],
                metadata_bytes: 0,
            },
            warnings: vec![
                "No redundancy: any disk failure = total data loss".to_string(),
                "Not recommended for important data".to_string(),
            ],
            score: 5,
        }
    }

    /// RAID 1 layout
    fn raid1_layout(disks: &[&DiskInfo]) -> LayoutRecommendation {
        let usable = disks.iter().map(|d| d.size_bytes).min().unwrap();
        let raw: u64 = disks.iter().map(|d| d.size_bytes).sum();

        LayoutRecommendation {
            id: "raid1".to_string(),
            name: "Mirror (RAID 1)".to_string(),
            description: "Data is mirrored across all disks for maximum safety".to_string(),
            technology: StorageTech::Mdadm,
            level: "1".to_string(),
            redundancy: RedundancyLevel::Single,
            usable_bytes: usable,
            raw_bytes: raw,
            efficiency_pct: (usable as f64 / raw as f64) * 100.0,
            risk: RiskAssessment {
                data_loss_risk: RiskLevel::Low,
                rebuild_time: RebuildTime::Moderate,
                failure_tolerance: (disks.len() - 1) as u8,
                explanation: format!("Can survive {} disk failure(s)", disks.len() - 1),
            },
            layout: DiskLayout {
                groups: vec![DiskGroup {
                    disks: disks.iter().map(|d| d.device.clone()).collect(),
                    role: "mirror".to_string(),
                }],
                metadata_bytes: 0,
            },
            warnings: if disks.len() > 2 {
                vec!["Using more than 2 disks in mirror is inefficient".to_string()]
            } else {
                vec![]
            },
            score: 80,
        }
    }

    /// RAID 5 layout
    fn raid5_layout(disks: &[&DiskInfo]) -> LayoutRecommendation {
        let smallest = disks.iter().map(|d| d.size_bytes).min().unwrap();
        let usable = smallest * (disks.len() as u64 - 1);
        let raw: u64 = disks.iter().map(|d| d.size_bytes).sum();

        LayoutRecommendation {
            id: "raid5".to_string(),
            name: "Efficient Redundancy (RAID 5)".to_string(),
            description: "Balanced capacity and safety with distributed parity".to_string(),
            technology: StorageTech::Mdadm,
            level: "5".to_string(),
            redundancy: RedundancyLevel::Single,
            usable_bytes: usable,
            raw_bytes: raw,
            efficiency_pct: (usable as f64 / raw as f64) * 100.0,
            risk: RiskAssessment {
                data_loss_risk: RiskLevel::Medium,
                rebuild_time: Self::estimate_rebuild_time(smallest),
                failure_tolerance: 1,
                explanation: "Can survive 1 disk failure. Vulnerable during rebuild.".to_string(),
            },
            layout: DiskLayout {
                groups: vec![DiskGroup {
                    disks: disks.iter().map(|d| d.device.clone()).collect(),
                    role: "raid5".to_string(),
                }],
                metadata_bytes: smallest,
            },
            warnings: Self::raid5_warnings(disks, smallest),
            score: 90,
        }
    }

    /// RAID 6 layout
    fn raid6_layout(disks: &[&DiskInfo]) -> LayoutRecommendation {
        let smallest = disks.iter().map(|d| d.size_bytes).min().unwrap();
        let usable = smallest * (disks.len() as u64 - 2);
        let raw: u64 = disks.iter().map(|d| d.size_bytes).sum();

        LayoutRecommendation {
            id: "raid6".to_string(),
            name: "Maximum Safety (RAID 6)".to_string(),
            description: "Double parity for maximum protection".to_string(),
            technology: StorageTech::Mdadm,
            level: "6".to_string(),
            redundancy: RedundancyLevel::Double,
            usable_bytes: usable,
            raw_bytes: raw,
            efficiency_pct: (usable as f64 / raw as f64) * 100.0,
            risk: RiskAssessment {
                data_loss_risk: RiskLevel::Low,
                rebuild_time: Self::estimate_rebuild_time(smallest),
                failure_tolerance: 2,
                explanation: "Can survive 2 simultaneous disk failures".to_string(),
            },
            layout: DiskLayout {
                groups: vec![DiskGroup {
                    disks: disks.iter().map(|d| d.device.clone()).collect(),
                    role: "raid6".to_string(),
                }],
                metadata_bytes: smallest * 2,
            },
            warnings: vec![],
            score: 95,
        }
    }

    /// RAID 10 layout
    fn raid10_layout(disks: &[&DiskInfo]) -> LayoutRecommendation {
        let smallest = disks.iter().map(|d| d.size_bytes).min().unwrap();
        let usable = smallest * (disks.len() as u64 / 2);
        let raw: u64 = disks.iter().map(|d| d.size_bytes).sum();

        // Group disks into pairs
        let mut groups = Vec::new();
        for pair in disks.chunks(2) {
            groups.push(DiskGroup {
                disks: pair.iter().map(|d| d.device.clone()).collect(),
                role: "mirror".to_string(),
            });
        }

        LayoutRecommendation {
            id: "raid10".to_string(),
            name: "Fast Redundancy (RAID 10)".to_string(),
            description: "Mirrored stripes for best performance with redundancy".to_string(),
            technology: StorageTech::Mdadm,
            level: "10".to_string(),
            redundancy: RedundancyLevel::Single,
            usable_bytes: usable,
            raw_bytes: raw,
            efficiency_pct: (usable as f64 / raw as f64) * 100.0,
            risk: RiskAssessment {
                data_loss_risk: RiskLevel::Low,
                rebuild_time: RebuildTime::Fast,
                failure_tolerance: 1,
                explanation: "Can survive 1 disk failure per mirror pair".to_string(),
            },
            layout: DiskLayout {
                groups,
                metadata_bytes: 0,
            },
            warnings: vec![],
            score: 85,
        }
    }

    /// RAID 1 with hot spare
    fn raid1_with_spare_layout(disks: &[&DiskInfo]) -> LayoutRecommendation {
        let usable = disks[0].size_bytes.min(disks[1].size_bytes);
        let raw: u64 = disks.iter().map(|d| d.size_bytes).sum();

        LayoutRecommendation {
            id: "raid1_spare".to_string(),
            name: "Mirror with Hot Spare".to_string(),
            description: "2-disk mirror with automatic spare activation".to_string(),
            technology: StorageTech::Mdadm,
            level: "1".to_string(),
            redundancy: RedundancyLevel::Single,
            usable_bytes: usable,
            raw_bytes: raw,
            efficiency_pct: (usable as f64 / raw as f64) * 100.0,
            risk: RiskAssessment {
                data_loss_risk: RiskLevel::Low,
                rebuild_time: RebuildTime::Moderate,
                failure_tolerance: 2,
                explanation: "Can survive 1 failure with automatic rebuild, then 1 more"
                    .to_string(),
            },
            layout: DiskLayout {
                groups: vec![
                    DiskGroup {
                        disks: vec![disks[0].device.clone(), disks[1].device.clone()],
                        role: "mirror".to_string(),
                    },
                    DiskGroup {
                        disks: vec![disks[2].device.clone()],
                        role: "spare".to_string(),
                    },
                ],
                metadata_bytes: 0,
            },
            warnings: vec![],
            score: 75,
        }
    }

    /// Estimate rebuild time category based on disk size
    fn estimate_rebuild_time(size_bytes: u64) -> RebuildTime {
        let size_tb = size_bytes as f64 / 1_000_000_000_000.0;

        if size_tb < 2.0 {
            RebuildTime::Fast
        } else if size_tb < 8.0 {
            RebuildTime::Moderate
        } else if size_tb < 16.0 {
            RebuildTime::Slow
        } else {
            RebuildTime::VerySlow
        }
    }

    /// Generate RAID 5 specific warnings
    fn raid5_warnings(disks: &[&DiskInfo], smallest_size: u64) -> Vec<String> {
        let mut warnings = Vec::new();

        // Warn about large disks
        let size_tb = smallest_size as f64 / 1_000_000_000_000.0;
        if size_tb > 4.0 {
            warnings.push(format!(
                "Large disks ({:.1} TB): rebuild can take 24+ hours, during which a second failure causes data loss",
                size_tb
            ));
        }

        // Warn about mixed sizes
        let largest = disks.iter().map(|d| d.size_bytes).max().unwrap();
        if largest > smallest_size * 11 / 10 {
            let wasted_tb =
                ((largest - smallest_size) * disks.len() as u64) as f64 / 1_000_000_000_000.0;
            warnings.push(format!(
                "Mixed disk sizes: {:.1} TB will be unused",
                wasted_tb
            ));
        }

        warnings
    }

    /// Score a recommendation (higher is better)
    fn score_recommendation(rec: &LayoutRecommendation, disk_count: usize) -> u32 {
        let mut score = rec.score;

        // Penalize low efficiency
        if rec.efficiency_pct < 50.0 {
            score = score.saturating_sub(20);
        }

        // Bonus for good redundancy
        match rec.redundancy {
            RedundancyLevel::Double => score += 10,
            RedundancyLevel::Single => score += 5,
            RedundancyLevel::None => score = score.saturating_sub(30),
        }

        // Bonus for appropriate disk count usage
        if disk_count >= 4 && rec.level == "10" {
            score += 5;
        }

        // Penalize warnings
        score = score.saturating_sub(rec.warnings.len() as u32 * 5);

        score
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_disk(device: &str, size_gb: u64) -> DiskInfo {
        DiskInfo {
            device: device.to_string(),
            size_bytes: size_gb * 1_000_000_000,
            model: None,
            in_use: false,
        }
    }

    #[test]
    fn test_single_disk_recommendation() {
        let disks = vec![make_disk("/dev/sda", 1000)];
        let recs = GuidedRedundancyService::recommend_layouts(&disks, RedundancyLevel::None);

        assert_eq!(recs.len(), 1);
        assert_eq!(recs[0].id, "single");
        assert_eq!(recs[0].redundancy, RedundancyLevel::None);
    }

    #[test]
    fn test_two_disk_raid1() {
        let disks = vec![make_disk("/dev/sda", 1000), make_disk("/dev/sdb", 1000)];
        let recs = GuidedRedundancyService::recommend_layouts(&disks, RedundancyLevel::Single);

        assert!(!recs.is_empty());
        let raid1 = recs.iter().find(|r| r.id == "raid1").unwrap();
        assert_eq!(raid1.redundancy, RedundancyLevel::Single);
        assert_eq!(raid1.usable_bytes, 1000 * 1_000_000_000);
    }

    #[test]
    fn test_three_disk_raid5() {
        let disks = vec![
            make_disk("/dev/sda", 1000),
            make_disk("/dev/sdb", 1000),
            make_disk("/dev/sdc", 1000),
        ];
        let recs = GuidedRedundancyService::recommend_layouts(&disks, RedundancyLevel::Single);

        let raid5 = recs.iter().find(|r| r.id == "raid5").unwrap();
        assert_eq!(raid5.redundancy, RedundancyLevel::Single);
        assert_eq!(raid5.usable_bytes, 2000 * 1_000_000_000);
        assert!(raid5.efficiency_pct > 60.0);
    }

    #[test]
    fn test_four_disk_raid10() {
        let disks = vec![
            make_disk("/dev/sda", 1000),
            make_disk("/dev/sdb", 1000),
            make_disk("/dev/sdc", 1000),
            make_disk("/dev/sdd", 1000),
        ];
        let recs = GuidedRedundancyService::recommend_layouts(&disks, RedundancyLevel::Single);

        let raid10 = recs.iter().find(|r| r.id == "raid10").unwrap();
        assert_eq!(raid10.level, "10");
        assert_eq!(raid10.layout.groups.len(), 2); // 2 mirror pairs
        assert_eq!(raid10.usable_bytes, 2000 * 1_000_000_000);
    }

    #[test]
    fn test_four_disk_raid6() {
        let disks = vec![
            make_disk("/dev/sda", 1000),
            make_disk("/dev/sdb", 1000),
            make_disk("/dev/sdc", 1000),
            make_disk("/dev/sdd", 1000),
        ];
        let recs = GuidedRedundancyService::recommend_layouts(&disks, RedundancyLevel::Double);

        let raid6 = recs.iter().find(|r| r.id == "raid6").unwrap();
        assert_eq!(raid6.redundancy, RedundancyLevel::Double);
        assert_eq!(raid6.risk.failure_tolerance, 2);
        assert_eq!(raid6.usable_bytes, 2000 * 1_000_000_000);
    }

    #[test]
    fn test_mixed_disk_sizes() {
        let disks = vec![
            make_disk("/dev/sda", 1000),
            make_disk("/dev/sdb", 2000),
            make_disk("/dev/sdc", 3000),
        ];
        let recs = GuidedRedundancyService::recommend_layouts(&disks, RedundancyLevel::Single);

        let raid5 = recs.iter().find(|r| r.id == "raid5").unwrap();
        // Uses smallest disk size
        assert_eq!(raid5.usable_bytes, 2000 * 1_000_000_000);
        assert!(!raid5.warnings.is_empty());
    }

    #[test]
    fn test_filters_in_use_disks() {
        let disks = vec![
            make_disk("/dev/sda", 1000),
            DiskInfo {
                device: "/dev/sdb".to_string(),
                size_bytes: 1000 * 1_000_000_000,
                model: None,
                in_use: true,
            },
        ];
        let recs = GuidedRedundancyService::recommend_layouts(&disks, RedundancyLevel::None);

        // Should only see single disk layout
        assert_eq!(recs.len(), 1);
        assert_eq!(recs[0].id, "single");
    }

    #[test]
    fn test_respects_min_redundancy() {
        let disks = vec![make_disk("/dev/sda", 1000), make_disk("/dev/sdb", 1000)];

        let recs = GuidedRedundancyService::recommend_layouts(&disks, RedundancyLevel::Single);

        // Should not include RAID 0
        assert!(recs.iter().all(|r| r.redundancy != RedundancyLevel::None));
    }

    #[test]
    fn test_recommendations_are_scored() {
        let disks = vec![
            make_disk("/dev/sda", 1000),
            make_disk("/dev/sdb", 1000),
            make_disk("/dev/sdc", 1000),
        ];
        let recs = GuidedRedundancyService::recommend_layouts(&disks, RedundancyLevel::None);

        // All should have scores
        assert!(recs.iter().all(|r| r.score > 0));

        // Should be sorted by score descending
        for window in recs.windows(2) {
            assert!(window[0].score >= window[1].score);
        }
    }
}

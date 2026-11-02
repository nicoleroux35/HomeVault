//! Integration tests for guided redundancy recommendations

use homevault::guided_redundancy::{
    DiskInfo, GuidedRedundancyService, RedundancyLevel, StorageTech,
};

fn make_disk(device: &str, size_gb: u64) -> DiskInfo {
    DiskInfo {
        device: device.to_string(),
        size_bytes: size_gb * 1_000_000_000,
        model: Some(format!("Test Disk {}", device)),
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
    assert_eq!(recs[0].usable_bytes, 1000 * 1_000_000_000);
    assert_eq!(recs[0].efficiency_pct, 100.0);
}

#[test]
fn test_single_disk_requires_no_redundancy() {
    let disks = vec![make_disk("/dev/sda", 1000)];

    // Should return nothing if redundancy required
    let recs = GuidedRedundancyService::recommend_layouts(&disks, RedundancyLevel::Single);

    assert_eq!(recs.len(), 0);
}

#[test]
fn test_two_disk_raid1() {
    let disks = vec![make_disk("/dev/sda", 1000), make_disk("/dev/sdb", 1000)];

    let recs = GuidedRedundancyService::recommend_layouts(&disks, RedundancyLevel::Single);

    assert!(!recs.is_empty());

    let raid1 = recs.iter().find(|r| r.id == "raid1").unwrap();
    assert_eq!(raid1.level, "1");
    assert_eq!(raid1.redundancy, RedundancyLevel::Single);
    assert_eq!(raid1.usable_bytes, 1000 * 1_000_000_000);
    assert_eq!(raid1.risk.failure_tolerance, 1);
}

#[test]
fn test_two_disk_raid0_when_no_redundancy() {
    let disks = vec![make_disk("/dev/sda", 1000), make_disk("/dev/sdb", 1000)];

    let recs = GuidedRedundancyService::recommend_layouts(&disks, RedundancyLevel::None);

    let raid0 = recs.iter().find(|r| r.id == "raid0");
    assert!(raid0.is_some());

    let raid0 = raid0.unwrap();
    assert_eq!(raid0.usable_bytes, 2000 * 1_000_000_000);
    assert_eq!(raid0.efficiency_pct, 100.0);
    assert!(!raid0.warnings.is_empty());
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
    assert_eq!(raid5.level, "5");
    assert_eq!(raid5.redundancy, RedundancyLevel::Single);
    assert_eq!(raid5.usable_bytes, 2000 * 1_000_000_000);
    assert!(raid5.efficiency_pct > 60.0);
    assert_eq!(raid5.risk.failure_tolerance, 1);
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
    assert_eq!(raid10.efficiency_pct, 50.0);
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
    assert_eq!(raid6.level, "6");
    assert_eq!(raid6.redundancy, RedundancyLevel::Double);
    assert_eq!(raid6.risk.failure_tolerance, 2);
    assert_eq!(raid6.usable_bytes, 2000 * 1_000_000_000);
}

#[test]
fn test_mixed_disk_sizes_uses_smallest() {
    let disks = vec![
        make_disk("/dev/sda", 1000),
        make_disk("/dev/sdb", 2000),
        make_disk("/dev/sdc", 3000),
    ];

    let recs = GuidedRedundancyService::recommend_layouts(&disks, RedundancyLevel::Single);

    let raid5 = recs.iter().find(|r| r.id == "raid5").unwrap();

    // Should use smallest disk (1000 GB)
    assert_eq!(raid5.usable_bytes, 2000 * 1_000_000_000);

    // Should warn about wasted space
    assert!(!raid5.warnings.is_empty());
    assert!(raid5.warnings.iter().any(|w| w.contains("Mixed disk sizes")));
}

#[test]
fn test_large_disks_get_rebuild_warning() {
    let disks = vec![
        make_disk("/dev/sda", 8000), // 8 TB
        make_disk("/dev/sdb", 8000),
        make_disk("/dev/sdc", 8000),
    ];

    let recs = GuidedRedundancyService::recommend_layouts(&disks, RedundancyLevel::Single);

    let raid5 = recs.iter().find(|r| r.id == "raid5").unwrap();

    // Should warn about long rebuild time
    assert!(raid5.warnings.iter().any(|w| w.contains("rebuild")));
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

    // Should only see single disk layout (sdb is in use)
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

    // Every layout is scored. `>= 0` rather than `> 0`: a no-redundancy layout
    // is deliberately penalised below zero-equivalent (`score 5 - 30`), so a
    // zero is a real answer, not a missing score.
    assert!(!recs.is_empty(), "three disks must produce recommendations");
    assert!(recs.iter().all(|r| r.score > 0 || r.redundancy == RedundancyLevel::None));

    // Should be sorted by score descending
    for window in recs.windows(2) {
        assert!(
            window[0].score >= window[1].score,
            "Recommendations should be sorted by score"
        );
    }
}

/// Parity is rewarded over mirroring on the same disk count.
///
/// This test used to compare RAID 5 against RAID 0 *on three disks*, which the
/// engine never produces: `raid0_layout` is only offered for 2 disks and
/// `raid5_layout` only from 3 up, so the two ids can never appear in the same
/// answer and the old `unwrap()` on `raid0` panicked.
#[test]
fn test_raid5_scores_higher_than_raid1_with_spare() {
    let disks = vec![
        make_disk("/dev/sda", 1000),
        make_disk("/dev/sdb", 1000),
        make_disk("/dev/sdc", 1000),
    ];

    let recs = GuidedRedundancyService::recommend_layouts(&disks, RedundancyLevel::None);

    let raid5 = recs.iter().find(|r| r.id == "raid5").unwrap();
    let raid1_spare = recs.iter().find(|r| r.id == "raid1_spare").unwrap();

    // RAID 5 keeps two thirds of the raw capacity with parity; a mirror plus a
    // hot spare keeps a third. Both are single-redundancy, so capacity wins.
    assert!(
        raid5.score > raid1_spare.score,
        "raid5 {} should outrank raid1_spare {}",
        raid5.score,
        raid1_spare.score
    );
}

#[test]
fn test_raid6_scores_higher_with_double_redundancy() {
    let disks = vec![
        make_disk("/dev/sda", 1000),
        make_disk("/dev/sdb", 1000),
        make_disk("/dev/sdc", 1000),
        make_disk("/dev/sdd", 1000),
    ];

    let recs = GuidedRedundancyService::recommend_layouts(&disks, RedundancyLevel::Double);

    let raid6 = recs.iter().find(|r| r.id == "raid6").unwrap();

    // RAID 6 should have high score for double redundancy
    assert!(raid6.score >= 90);
}

#[test]
fn test_three_disk_raid1_with_spare() {
    let disks = vec![
        make_disk("/dev/sda", 1000),
        make_disk("/dev/sdb", 1000),
        make_disk("/dev/sdc", 1000),
    ];

    let recs = GuidedRedundancyService::recommend_layouts(&disks, RedundancyLevel::Single);

    let raid1_spare = recs.iter().find(|r| r.id == "raid1_spare");
    assert!(raid1_spare.is_some());

    let raid1_spare = raid1_spare.unwrap();
    assert_eq!(raid1_spare.level, "1");
    assert_eq!(raid1_spare.layout.groups.len(), 2); // Mirror group + spare group
    assert_eq!(raid1_spare.risk.failure_tolerance, 2);
}

#[test]
fn test_empty_disk_list() {
    let disks = vec![];

    let recs = GuidedRedundancyService::recommend_layouts(&disks, RedundancyLevel::None);

    assert_eq!(recs.len(), 0);
}

#[test]
fn test_all_disks_in_use() {
    let disks = vec![
        DiskInfo {
            device: "/dev/sda".to_string(),
            size_bytes: 1000 * 1_000_000_000,
            model: None,
            in_use: true,
        },
        DiskInfo {
            device: "/dev/sdb".to_string(),
            size_bytes: 1000 * 1_000_000_000,
            model: None,
            in_use: true,
        },
    ];

    let recs = GuidedRedundancyService::recommend_layouts(&disks, RedundancyLevel::None);

    assert_eq!(recs.len(), 0);
}

#[test]
fn test_efficiency_calculation() {
    let disks = vec![
        make_disk("/dev/sda", 1000),
        make_disk("/dev/sdb", 1000),
    ];

    let recs = GuidedRedundancyService::recommend_layouts(&disks, RedundancyLevel::Single);

    let raid1 = recs.iter().find(|r| r.id == "raid1").unwrap();

    // RAID 1 efficiency should be 50%
    assert_eq!(raid1.efficiency_pct, 50.0);
    assert_eq!(raid1.usable_bytes, 1000 * 1_000_000_000);
    assert_eq!(raid1.raw_bytes, 2000 * 1_000_000_000);
}

#[test]
fn test_layout_includes_disk_list() {
    let disks = vec![
        make_disk("/dev/sda", 1000),
        make_disk("/dev/sdb", 1000),
        make_disk("/dev/sdc", 1000),
    ];

    let recs = GuidedRedundancyService::recommend_layouts(&disks, RedundancyLevel::Single);

    let raid5 = recs.iter().find(|r| r.id == "raid5").unwrap();

    // Should list all disks
    assert_eq!(raid5.layout.groups.len(), 1);
    assert_eq!(raid5.layout.groups[0].disks.len(), 3);
    assert!(raid5.layout.groups[0].disks.contains(&"/dev/sda".to_string()));
    assert!(raid5.layout.groups[0].disks.contains(&"/dev/sdb".to_string()));
    assert!(raid5.layout.groups[0].disks.contains(&"/dev/sdc".to_string()));
}

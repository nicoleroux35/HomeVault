//! Integration tests for USB one-touch copy

use homevault::usb_copy::{
    ConflictPolicy, CopyStrategy, UsbCopyProfile, UsbCopyService, DEFAULT_VERSIONS_TO_KEEP,
};

#[tokio::test]
async fn test_list_devices() {
    // Test device listing (may be empty)
    let result = UsbCopyService::list_devices().await;

    // Should not error even with no devices
    match result {
        Ok(devices) => {
            println!("Found {} removable devices", devices.len());
            for dev in devices {
                println!("  Device: {} (UUID: {:?})", dev.device, dev.uuid);
            }
        }
        Err(e) => {
            // lsblk not available or permission issue
            println!("Could not list devices: {}", e);
        }
    }
}

#[test]
fn test_validate_profile_valid() {
    let profile = UsbCopyProfile {
        id: "usb_photos".to_string(),
        name: "Camera Photos".to_string(),
        device_uuid: "1234-5678".to_string(),
        device_label: Some("CAMERA".to_string()),
        source_path: "/DCIM".to_string(),
        destination_path: "/mnt/photos".to_string(),
        verify_checksums: true,
        strategy: CopyStrategy::Incremental,
        conflict: ConflictPolicy::Overwrite,
        versions_to_keep: DEFAULT_VERSIONS_TO_KEEP,
        auto_eject: true,
        enabled: true,
        last_run: None,
        last_status: None,
    };

    let result = UsbCopyService::validate_profile(&profile);
    assert!(result.is_ok());
}

#[test]
fn test_validate_profile_id_length() {
    let mut profile = make_test_profile();
    profile.id = "a".repeat(65);

    let result = UsbCopyService::validate_profile(&profile);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("1-64 characters"));
}

#[test]
fn test_validate_profile_id_characters() {
    let mut profile = make_test_profile();
    profile.id = "test@profile".to_string();

    let result = UsbCopyService::validate_profile(&profile);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("alphanumeric"));
}

#[test]
fn test_validate_profile_requires_uuid() {
    let mut profile = make_test_profile();
    profile.device_uuid = "".to_string();

    let result = UsbCopyService::validate_profile(&profile);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("UUID is required"));
}

#[test]
fn test_validate_profile_source_no_parent_dir() {
    let mut profile = make_test_profile();
    profile.source_path = "/../etc".to_string();

    let result = UsbCopyService::validate_profile(&profile);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("'..'"));
}

#[test]
fn test_validate_destination_must_be_absolute() {
    let mut profile = make_test_profile();
    profile.destination_path = "relative/path".to_string();

    let result = UsbCopyService::validate_profile(&profile);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("absolute"));
}

#[test]
fn test_validate_destination_forbidden_paths() {
    let forbidden = ["/", "/etc", "/bin", "/sys", "/proc"];

    for path in &forbidden {
        let mut profile = make_test_profile();
        profile.destination_path = path.to_string();

        let result = UsbCopyService::validate_profile(&profile);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("cannot be in"));
    }
}

#[test]
fn test_validate_destination_allows_safe_paths() {
    let safe_paths = [
        "/mnt/usb",
        "/srv/backup",
        "/home/user/photos",
        "/media/data",
    ];

    for path in &safe_paths {
        let mut profile = make_test_profile();
        profile.destination_path = path.to_string();

        let result = UsbCopyService::validate_profile(&profile);
        assert!(result.is_ok(), "Path {} should be allowed", path);
    }
}

#[test]
fn test_parse_lsblk_json_empty() {
    let json = r#"{"blockdevices": []}"#;
    let devices = UsbCopyService::parse_lsblk_json(json).unwrap();
    assert_eq!(devices.len(), 0);
}

#[test]
fn test_parse_lsblk_json_removable_device() {
    let json = r#"{
        "blockdevices": [
            {
                "name": "sdb1",
                "uuid": "1234-5678",
                "label": "USB_DRIVE",
                "size": "16000000000",
                "mountpoint": "/media/usb",
                "rm": "1",
                "type": "part"
            }
        ]
    }"#;

    let devices = UsbCopyService::parse_lsblk_json(json).unwrap();
    assert_eq!(devices.len(), 1);

    let dev = &devices[0];
    assert_eq!(dev.device, "/dev/sdb1");
    assert_eq!(dev.uuid, Some("1234-5678".to_string()));
    assert_eq!(dev.label, Some("USB_DRIVE".to_string()));
    assert!(dev.mounted);
    assert_eq!(dev.size, 16000000000);
}

#[test]
fn test_parse_lsblk_json_ignores_non_removable() {
    let json = r#"{
        "blockdevices": [
            {
                "name": "sda1",
                "uuid": "aaaa-bbbb",
                "size": "500000000000",
                "mountpoint": "/",
                "rm": "0",
                "type": "part"
            }
        ]
    }"#;

    let devices = UsbCopyService::parse_lsblk_json(json).unwrap();
    assert_eq!(devices.len(), 0);
}

#[test]
fn test_parse_lsblk_json_ignores_disk_type() {
    let json = r#"{
        "blockdevices": [
            {
                "name": "sdb",
                "size": "16000000000",
                "rm": "1",
                "type": "disk"
            }
        ]
    }"#;

    let devices = UsbCopyService::parse_lsblk_json(json).unwrap();
    assert_eq!(devices.len(), 0);
}

#[test]
fn test_parse_lsblk_json_unmounted_device() {
    let json = r#"{
        "blockdevices": [
            {
                "name": "sdb1",
                "uuid": "1234-5678",
                "label": "USB_DRIVE",
                "size": "16000000000",
                "mountpoint": null,
                "rm": "1",
                "type": "part"
            }
        ]
    }"#;

    let devices = UsbCopyService::parse_lsblk_json(json).unwrap();
    assert_eq!(devices.len(), 1);

    let dev = &devices[0];
    assert!(!dev.mounted);
    assert!(dev.mount_point.is_none());
}

#[tokio::test]
async fn test_is_pool_member_returns_false_for_nonexistent() {
    // Test with a device that doesn't exist
    let result = UsbCopyService::is_pool_member("/dev/nonexistent999").await;

    // Should return Ok(false) for non-pool devices
    if let Ok(is_member) = result {
        assert!(!is_member);
    }
}

#[tokio::test]
async fn test_count_files_empty_dir() {
    use tempfile::tempdir;

    let temp_dir = tempdir().unwrap();
    let count = UsbCopyService::count_files(temp_dir.path()).await.unwrap();

    assert_eq!(count, 0);
}

#[tokio::test]
async fn test_count_files_with_files() {
    use tempfile::tempdir;
    use tokio::fs;

    let temp_dir = tempdir().unwrap();

    // Create some test files
    fs::write(temp_dir.path().join("file1.txt"), "test")
        .await
        .unwrap();
    fs::write(temp_dir.path().join("file2.txt"), "test")
        .await
        .unwrap();

    let count = UsbCopyService::count_files(temp_dir.path()).await.unwrap();

    assert_eq!(count, 2);
}

#[tokio::test]
async fn test_count_files_with_subdirectories() {
    use tempfile::tempdir;
    use tokio::fs;

    let temp_dir = tempdir().unwrap();

    // Create files in subdirectory
    let sub_dir = temp_dir.path().join("subdir");
    fs::create_dir(&sub_dir).await.unwrap();
    fs::write(sub_dir.join("file1.txt"), "test").await.unwrap();
    fs::write(temp_dir.path().join("file2.txt"), "test")
        .await
        .unwrap();

    let count = UsbCopyService::count_files(temp_dir.path()).await.unwrap();

    assert_eq!(count, 2);
}

#[tokio::test]
async fn test_verify_copy_rejects_a_corrupted_destination() {
    use std::fs;
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src.bin");
    let dst = dir.path().join("dst.bin");
    fs::write(&src, b"the-original-bytes").unwrap();
    fs::write(&dst, b"the-corruptd-bytes").unwrap();

    let error = UsbCopyService::verify_copy(&src, &dst)
        .await
        .expect_err("same length is not the same content");
    assert!(error.contains("checksum"), "{error}");
}

fn make_test_profile() -> UsbCopyProfile {
    UsbCopyProfile {
        id: "test_profile".to_string(),
        name: "Test Profile".to_string(),
        device_uuid: "1234-5678".to_string(),
        device_label: Some("TEST".to_string()),
        source_path: "/".to_string(),
        destination_path: "/mnt/test".to_string(),
        verify_checksums: false,
        strategy: CopyStrategy::Incremental,
        conflict: ConflictPolicy::Overwrite,
        versions_to_keep: DEFAULT_VERSIONS_TO_KEEP,
        auto_eject: false,
        enabled: true,
        last_run: None,
        last_status: None,
    }
}

#[test]
fn test_profile_defaults_keep_older_json_loadable() {
    // A profile written before FIAB-13 has no `strategy` / `conflict`; it must
    // still load, with the historical behaviour (incremental + overwrite).
    let json = r#"{
        "id": "legacy",
        "name": "Legacy profile",
        "device_uuid": "1234-5678",
        "device_label": null,
        "source_path": "/DCIM",
        "destination_path": "/mnt/photos",
        "verify_checksums": true,
        "auto_eject": false,
        "enabled": true,
        "last_run": null,
        "last_status": null
    }"#;
    let profile: UsbCopyProfile = serde_json::from_str(json).unwrap();
    assert_eq!(profile.strategy, CopyStrategy::Incremental);
    assert_eq!(profile.conflict, ConflictPolicy::Overwrite);
    assert_eq!(profile.versions_to_keep, DEFAULT_VERSIONS_TO_KEEP);
}

#[test]
fn test_version_retention_is_bounded_by_validation() {
    let mut profile = make_test_profile();
    profile.strategy = CopyStrategy::Versioned;
    profile.versions_to_keep = 5;
    assert!(UsbCopyService::validate_profile(&profile).is_ok());

    profile.versions_to_keep = 0;
    assert!(UsbCopyService::validate_profile(&profile).is_ok());

    profile.versions_to_keep = 1_001;
    let error = UsbCopyService::validate_profile(&profile).expect_err("over the ceiling");
    assert!(error.contains("versions to keep"), "{error}");
}

#[test]
fn test_copy_strategy_serializes_as_the_api_literal() {
    for (strategy, literal) in [
        (CopyStrategy::Incremental, "incremental"),
        (CopyStrategy::Mirror, "mirror"),
        (CopyStrategy::Versioned, "versioned"),
        (CopyStrategy::PhotoImport, "photo_import"),
    ] {
        assert_eq!(
            serde_json::to_string(&strategy).unwrap(),
            format!("\"{literal}\"")
        );
        assert_eq!(strategy.as_str(), literal);
    }

    for (conflict, literal) in [
        (ConflictPolicy::Overwrite, "overwrite"),
        (ConflictPolicy::Skip, "skip"),
        (ConflictPolicy::Rename, "rename"),
    ] {
        assert_eq!(
            serde_json::to_string(&conflict).unwrap(),
            format!("\"{literal}\"")
        );
        assert_eq!(conflict.as_str(), literal);
    }
}

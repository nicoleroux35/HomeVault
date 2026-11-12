#![no_main]

//! SMB/NFS share configuration: protocol parsing, name/path validation and
//! the JSON shapes an admin (or an older UI) can post.

use libfuzzer_sys::fuzz_target;
use homevault::shares::{
    validate_nfs_config, validate_nfs_options, validate_nfs_rules, validate_share_comment,
    validate_share_name, validate_share_path, NfsClientRule, NfsConfig, ShareType,
};

fuzz_target!(|data: &[u8]| {
    let text = String::from_utf8_lossy(data);

    let _ = ShareType::parse(&text);
    let _ = validate_share_name(&text);
    let _ = validate_share_comment(&text);
    let _ = validate_share_path(&text);

    let options: Vec<String> = text.split('\n').map(str::to_string).collect();
    let _ = validate_nfs_options(&options);

    // Arbitrary JSON into the public config types, then the real validator.
    if let Ok(config) = serde_json::from_str::<NfsConfig>(&text) {
        let _ = validate_nfs_config(&config);
    }
    if let Ok(rules) = serde_json::from_str::<Vec<NfsClientRule>>(&text) {
        let _ = validate_nfs_rules(&rules);
    }
});

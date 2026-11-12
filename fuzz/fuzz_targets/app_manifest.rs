#![no_main]

//! Community app manifests (TOML) from the catalog and from third-party
//! stores. Parsing must be total: a malformed manifest is a refused install,
//! never a panic in the catalog endpoint.

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let text = String::from_utf8_lossy(data);
    let _ = homevault::app_catalog::manifest::Manifest::parse(&text);
    let _ = homevault::app_catalog::manifest::validate_share_mount(&text);
    let _ = homevault::app_catalog::manifest::is_valid_env_name(&text);
});

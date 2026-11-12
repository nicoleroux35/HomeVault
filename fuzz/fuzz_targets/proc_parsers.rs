#![no_main]

//! Parsers for `/proc`, SMART and mdadm output.
//!
//! These read text produced by the kernel and by vendor tools; a truncated
//! line, a locale change or a NUL byte must never panic the agent (a panic in
//! a background scheduler loop takes the whole status card down).

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let text = String::from_utf8_lossy(data);
    let _ = homevault::disk::parse_mdadm_detail(&text);
    let _ = homevault::disk_health::parse_smart_attributes(&text);
    let _ = homevault::disk_health::parse_health(&text);
    let _ = homevault::disk_health::parse_io_errors(&text);
    let _ = homevault::disk_health::parse_md_mismatch(&text);
    let _ = homevault::system_stability::parse_write_cache(&text);
});

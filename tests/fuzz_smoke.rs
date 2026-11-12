/* =============================================================================
 * File          : fuzz_smoke.rs
 * Author        : Nicolas Le Roux
 * Created       : 11/11/2026
 * Version       : 1.0.0
 *
 * Description:
 *   Deterministic, stable-toolchain counterpart of the cargo-fuzz targets.
 *   cargo-fuzz needs nightly + the sanitizer runtime, so it cannot run in the
 *   default CI matrix; this test feeds the same public parsers thousands of
 *   adversarial inputs (control characters, path separators, NULs, invalid
 *   UTF-8) and asserts the invariants the fuzzers check. A real fuzz campaign
 *   remains the deeper check (see fuzz/README.md).
 *
 * Dependencies:
 *   - none (test crate)
 *
 * Usage:
 *   cargo test --test fuzz_smoke
 * =============================================================================
 */

//! Fuzz smoke suite — runs on stable, every `cargo test`.
//!
//! The generators are an xorshift PRNG plus a hostile alphabet, so the run is
//! deterministic and reproducible: a failure points at an exact iteration.

use homevault::app_catalog::manifest::{is_valid_env_name, validate_share_mount, Manifest};
use homevault::disk::parse_mdadm_detail;
use homevault::disk_health::{
    parse_health, parse_io_errors, parse_md_mismatch, parse_smart_attributes,
};
use homevault::docker::{validate_digest, validate_image_reference, DockerService};
use homevault::handlers::explorer::{sanitize_download_filename, sanitize_upload_filename};
use homevault::remote_mounts::{
    validate_account, validate_name, validate_options, validate_server, validate_source, RemoteKind,
};
use homevault::shares::{
    validate_nfs_config, validate_nfs_options, validate_nfs_rules, validate_share_comment,
    validate_share_name, validate_share_path, NfsClientRule, NfsConfig, ShareType,
};
use homevault::system_stability::parse_write_cache;

/// xorshift64* — small, deterministic, good enough for byte soup.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, bound: usize) -> usize {
        (self.next() % bound as u64) as usize
    }
}

/// An alphabet deliberately full of the bytes that break parsers.
const HOSTILE: &[u8] = b"abcXYZ019 -_./\\:@,;[]()#'\"\n\r\t\0{}%$&|<>*?=+\x7f\x80\xff";

fn hostile_string(rng: &mut Rng, max_len: usize) -> String {
    let len = rng.below(max_len + 1);
    let mut bytes = Vec::with_capacity(len);
    for _ in 0..len {
        let byte = if rng.below(10) == 0 {
            (rng.next() % 256) as u8
        } else {
            HOSTILE[rng.below(HOSTILE.len())]
        };
        bytes.push(byte);
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

const ITERATIONS: usize = 4_000;

#[test]
fn parsers_never_panic_on_hostile_text() {
    let mut rng = Rng(0x1234_5678_9ABC_DEF0);
    for i in 0..ITERATIONS {
        let text = hostile_string(&mut rng, 512);
        let lines: Vec<String> = text.split('\n').map(str::to_string).collect();

        // /proc, SMART and mdadm.
        parse_mdadm_detail(&text);
        let _ = parse_smart_attributes(&text);
        let _ = parse_health(&text);
        let _ = parse_io_errors(&text);
        let _ = parse_md_mismatch(&text);
        let _ = parse_write_cache(&text);

        // Share configuration.
        let _ = ShareType::parse(&text);
        let _ = validate_share_name(&text);
        let _ = validate_share_comment(&text);
        let _ = validate_share_path(&text);
        let _ = validate_nfs_options(&lines);
        if let Ok(config) = serde_json::from_str::<NfsConfig>(&text) {
            let _ = validate_nfs_config(&config);
        }
        if let Ok(rules) = serde_json::from_str::<Vec<NfsClientRule>>(&text) {
            let _ = validate_nfs_rules(&rules);
        }

        // Remote mounts.
        let _ = validate_name(&text);
        let _ = validate_server(&text);
        for kind in [RemoteKind::Cifs, RemoteKind::Nfs] {
            let _ = validate_source(kind, &text);
            let _ = validate_options(kind, &lines);
        }
        let _ = validate_account("username", Some(&text));

        // Catalog, Docker, filenames, WS-Discovery.
        let _ = Manifest::parse(&text);
        let _ = validate_share_mount(&text);
        let _ = is_valid_env_name(&text);
        let _ = validate_image_reference(&text);
        let _ = validate_digest(&text);
        let _ = DockerService::validate_daemon_config(&text);
        let unit = homevault::ws_discovery::render_unit(&text, &text, &text);
        assert!(
            !unit.contains('\0'),
            "iteration {i}: NUL survived into the WS-Discovery unit"
        );
        assert_eq!(
            unit.lines()
                .filter(|line| line.starts_with("ExecStart="))
                .count(),
            1,
            "iteration {i}: a hostile input added an ExecStart line"
        );

        if let Some(name) = sanitize_upload_filename(&text) {
            assert!(!name.is_empty(), "iteration {i}: empty name");
            assert!(
                !name.chars().any(char::is_control),
                "iteration {i}: control char survived: {name:?}"
            );
            assert_eq!(
                std::path::Path::new(&name)
                    .file_name()
                    .and_then(|n| n.to_str()),
                Some(name.as_str()),
                "iteration {i}: not one path component: {name:?}"
            );
        }
        let download = sanitize_download_filename(&text);
        assert!(
            !download
                .chars()
                .any(|c| c.is_control() || c == '"' || c == '\\'),
            "iteration {i}: header-breaking filename: {download:?}"
        );
    }
}

#[test]
fn validated_smb_nfs_values_never_carry_a_delimiter_escape() {
    let mut rng = Rng(0xDEAD_BEEF_CAFE_F00D);
    for i in 0..ITERATIONS {
        let text = hostile_string(&mut rng, 256);

        if let Ok(name) = validate_name(&text) {
            assert!(
                name.chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'),
                "iteration {i}: accepted name {name:?}"
            );
            assert!(!name.starts_with('-') && !name.ends_with('-'));
        }
        if let Ok(server) = validate_server(&text) {
            assert!(
                !server.contains(['/', '\\', ':', ' ']) && !server.chars().any(char::is_control),
                "iteration {i}: accepted server {server:?}"
            );
        }
        for kind in [RemoteKind::Cifs, RemoteKind::Nfs] {
            if let Ok(source) = validate_source(kind, &text) {
                assert!(
                    !source.chars().any(char::is_control),
                    "iteration {i}: accepted source {source:?}"
                );
            }
        }
    }
}

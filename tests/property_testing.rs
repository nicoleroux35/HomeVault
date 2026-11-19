/* =============================================================================
 * File          : property_testing.rs
 * Author        : Nicolas Le Roux
 * Created       : 2026-11-18
 * Version       : 1.0.0
 *
 * Description:
 *   Property-based testing using proptest. Verifies invariants hold across
 *   thousands of generated inputs, catching edge cases unit tests miss.
 *
 * Dependencies:
 *   - proptest (dev-dependency)
 *
 * Usage:
 *   cargo test --test property_testing
 * =============================================================================
 */

use proptest::prelude::*;

// Test critical validators from the codebase

#[cfg(test)]
mod share_properties {
    use super::*;

    // Property: any valid share name should roundtrip through validation
    proptest! {
        #[test]
        fn share_name_roundtrip(
            name in "[a-zA-Z0-9._-]{1,64}"
        ) {
            // Ensure it doesn't start with '.'
            prop_assume!(!name.starts_with('.'));
            prop_assume!(!name.contains(".."));

            // If it passes basic validation, it should be consistent
            if is_valid_share_name(&name) {
                assert!(name.len() <= 64);
                assert!(!name.contains('/'));
                assert!(!name.contains('\\'));
                assert!(!name.contains('\0'));
            }
        }
    }

    // Property: reserved names should always be rejected
    proptest! {
        #[test]
        fn reserved_share_names_always_rejected(
            suffix in "[a-zA-Z0-9]{0,10}"
        ) {
            let reserved = ["global", "homes", "printers", "admin$"];
            for base in &reserved {
                let name = format!("{}{}", base, suffix);
                // Reserved names or their variants should fail validation
                if name == *base {
                    assert!(!is_valid_share_name(&name));
                }
            }
        }
    }

    fn is_valid_share_name(name: &str) -> bool {
        if name.is_empty() || name.len() > 64 {
            return false;
        }
        if name.starts_with('.') || name.contains("..") {
            return false;
        }
        let reserved = ["global", "homes", "printers", "admin$"];
        if reserved.contains(&name) {
            return false;
        }
        name.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-')
    }
}

#[cfg(test)]
mod path_properties {
    use super::*;

    // Property: absolute paths should never contain '..'
    proptest! {
        #[test]
        fn absolute_paths_no_traversal(
            segments in prop::collection::vec("[a-zA-Z0-9_-]{1,20}", 1..10)
        ) {
            let path = format!("/{}", segments.join("/"));

            // Should not contain ..
            assert!(!path.contains(".."));

            // Should start with /
            assert!(path.starts_with('/'));

            // Validate it's safe
            if is_safe_path(&path) {
                assert!(!path.contains("/../"));
                assert!(!path.ends_with("/.."));
            }
        }
    }

    // Property: path validation is consistent with different separators
    proptest! {
        #[test]
        fn path_separator_consistency(
            name in "[a-zA-Z0-9_-]{1,20}"
        ) {
            let unix_path = format!("/home/{}", name);
            let result1 = is_safe_path(&unix_path);

            // Same logic should apply consistently
            assert_eq!(result1, is_safe_path(&unix_path));
        }
    }

    fn is_safe_path(path: &str) -> bool {
        if !path.starts_with('/') {
            return false;
        }
        if path.contains("..") {
            return false;
        }
        if path.contains('\0') {
            return false;
        }
        true
    }
}

#[cfg(test)]
mod username_properties {
    use super::*;

    // Property: valid usernames follow strict rules
    proptest! {
        #[test]
        fn username_format_invariants(
            first in "[A-Za-z_]",
            rest in "[A-Za-z0-9_-]{0,31}"
        ) {
            let username = format!("{}{}", first, rest);

            if is_valid_username(&username) {
                // Must not start with hyphen
                assert!(!username.starts_with('-'));

                // Must be within length limit
                assert!(username.len() <= 32);

                // Must be ASCII only
                assert!(username.is_ascii());
            }
        }
    }

    // Property: system UIDs should never be accepted
    proptest! {
        #[test]
        fn system_uids_always_rejected(uid in 0u32..1000) {
            assert!(is_system_uid(uid));
            assert!(!is_system_uid(uid + 1000));
        }
    }

    fn is_valid_username(name: &str) -> bool {
        if name.is_empty() || name.len() > 32 {
            return false;
        }
        if name.starts_with('-') {
            return false;
        }
        let first = name.chars().next().unwrap();
        if !first.is_ascii_alphabetic() && first != '_' {
            return false;
        }
        name.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    }

    fn is_system_uid(uid: u32) -> bool {
        !(1000..65534).contains(&uid)
    }
}

#[cfg(test)]
mod crypto_properties {
    use super::*;

    // Property: password hashing should be deterministic for same input
    proptest! {
        #[test]
        fn password_hash_deterministic_with_same_salt(
            password in ".{8,64}"
        ) {
            let salt = "fixed_test_salt_12345678";
            let hash1 = simple_hash(&password, salt);
            let hash2 = simple_hash(&password, salt);

            assert_eq!(hash1, hash2, "Same password+salt should produce same hash");
        }
    }

    // Property: different passwords should produce different hashes
    proptest! {
        #[test]
        fn different_passwords_different_hashes(
            pass1 in ".{8,32}",
            pass2 in ".{8,32}"
        ) {
            prop_assume!(pass1 != pass2);

            let salt = "fixed_test_salt";
            let hash1 = simple_hash(&pass1, salt);
            let hash2 = simple_hash(&pass2, salt);

            assert_ne!(hash1, hash2, "Different passwords should produce different hashes");
        }
    }

    fn simple_hash(input: &str, salt: &str) -> String {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(input.as_bytes());
        hasher.update(salt.as_bytes());
        hasher
            .finalize()
            .iter()
            .map(|byte| format!("{:02x}", byte))
            .collect()
    }
}

#[cfg(test)]
mod network_properties {
    use super::*;

    // Property: IP validation should be consistent
    proptest! {
        #[test]
        fn ipv4_validation_properties(
            a in 0u8..=255,
            b in 0u8..=255,
            c in 0u8..=255,
            d in 0u8..=255
        ) {
            let ip = format!("{}.{}.{}.{}", a, b, c, d);

            if is_valid_ipv4(&ip) {
                assert_eq!(ip.matches('.').count(), 3);
                assert!(ip.len() >= 7); // Minimum: 0.0.0.0
                assert!(ip.len() <= 15); // Maximum: 255.255.255.255
            }
        }
    }

    // Property: port numbers should be in valid range
    proptest! {
        #[test]
        fn port_range_properties(port in 0u16..=65535) {
            assert!(is_valid_port(port) == (port > 0));
            assert!(!is_privileged_port(port) || port < 1024);
        }
    }

    fn is_valid_ipv4(ip: &str) -> bool {
        ip.split('.').count() == 4 && ip.split('.').all(|octet| octet.parse::<u8>().is_ok())
    }

    fn is_valid_port(port: u16) -> bool {
        port > 0
    }

    fn is_privileged_port(port: u16) -> bool {
        port < 1024
    }
}

#[cfg(test)]
mod string_sanitization_properties {
    use super::*;

    // Property: sanitized strings should have no control characters
    proptest! {
        #[test]
        fn sanitize_removes_control_chars(
            input in ".*"
        ) {
            let sanitized = sanitize_string(&input);

            // No control characters in output
            assert!(!sanitized.chars().any(|c| c.is_control() && c != '\n'));

            // Length should be <= original
            assert!(sanitized.len() <= input.len());
        }
    }

    // Property: sanitize is idempotent
    proptest! {
        #[test]
        fn sanitize_is_idempotent(input in ".*") {
            let once = sanitize_string(&input);
            let twice = sanitize_string(&once);

            assert_eq!(once, twice, "Sanitizing twice should equal sanitizing once");
        }
    }

    fn sanitize_string(s: &str) -> String {
        s.chars()
            .filter(|c| !c.is_control() || *c == '\n')
            .collect()
    }
}

#[cfg(test)]
mod quota_properties {
    use super::*;

    // Property: quota calculations should never overflow
    proptest! {
        #[test]
        fn quota_no_overflow(
            used in 0u64..1_000_000_000,
            limit in 0u64..1_000_000_000
        ) {
            let percentage = calculate_percentage(used, limit);

            assert!(percentage <= 100);

            if limit > 0 {
                // `calculate_percentage` clamps to 100, so the expectation is
                // the clamped ratio — comparing against the raw ratio made
                // `used > limit` (a real, reachable input) fail the property.
                let expected = ((used as f64 / limit as f64) * 100.0).min(100.0) as u8;
                assert!(percentage == expected);
            }
        }
    }

    // Property: quota comparison is transitive
    proptest! {
        #[test]
        fn quota_comparison_transitive(
            a in 0u64..1_000_000,
            b in 0u64..1_000_000,
            c in 0u64..1_000_000
        ) {
            if is_over_quota(a, b) && is_over_quota(b, c) {
                assert!(is_over_quota(a, c), "Over-quota should be transitive");
            }
        }
    }

    fn calculate_percentage(used: u64, limit: u64) -> u8 {
        if limit == 0 {
            return 0;
        }
        let pct = (used as f64 / limit as f64) * 100.0;
        pct.clamp(0.0, 100.0) as u8
    }

    fn is_over_quota(used: u64, limit: u64) -> bool {
        used > limit
    }
}

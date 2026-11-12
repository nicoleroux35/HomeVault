#![no_main]

//! Client-provided upload / download filenames.
//!
//! The sanitizers are the gate between a multipart field and a filesystem
//! write. The invariant asserted here is the one the unit tests state:
//! a returned upload name is a single path component with no separators and
//! no control characters.

use libfuzzer_sys::fuzz_target;
use homevault::handlers::explorer::{
    sanitize_download_filename, sanitize_upload_filename,
};

fuzz_target!(|data: &[u8]| {
    let text = String::from_utf8_lossy(data);

    if let Some(name) = sanitize_upload_filename(&text) {
        assert!(!name.is_empty(), "empty sanitized name");
        assert_eq!(
            std::path::Path::new(&name).file_name().and_then(|n| n.to_str()),
            Some(name.as_str()),
            "sanitized name is not one path component: {name:?}"
        );
        assert!(
            !name.chars().any(char::is_control),
            "control character survived: {name:?}"
        );
    }

    let download = sanitize_download_filename(&text);
    assert!(
        !download
            .chars()
            .any(|c| c.is_control() || c == '"' || c == '\\'),
        "download filename still breaks Content-Disposition: {download:?}"
    );
});

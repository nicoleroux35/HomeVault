#![no_main]

//! Docker image references, digests and `daemon.json`.
//!
//! `validate_image_reference` is the gate in front of `docker pull`; a panic
//! or an accepted control character would be a command-injection surface.

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let text = String::from_utf8_lossy(data);
    let _ = homevault::docker::validate_image_reference(&text);
    let _ = homevault::docker::validate_digest(&text);
    let _ = homevault::docker::DockerService::validate_daemon_config(&text);
});

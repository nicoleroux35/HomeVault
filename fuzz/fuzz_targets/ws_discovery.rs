#![no_main]

//! WS-Discovery unit renderer: host name and workgroup are spliced into an
//! XML/unit body, so a newline or an XML metacharacter must not add a
//! directive.

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let text = String::from_utf8_lossy(data);
    let rendered = homevault::ws_discovery::render_unit(&text, &text, &text);
    // The renderer must always produce a non-empty unit body and must never
    // leave a bare NUL (exec would reject it, but a panic-free render is the
    // contract).
    assert!(!rendered.is_empty());
    assert!(!rendered.contains('\0'));
});

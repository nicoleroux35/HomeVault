#![no_main]

//! Remote-mount (CIFS/NFS) validators.
//!
//! Every value here is spliced into either `//server/share`, `server:/export`
//! or a credentials file, so the invariant the smoke test also asserts is:
//! a value that validates must never contain a delimiter that could start a
//! second option, directive or header line.

use libfuzzer_sys::fuzz_target;
use homevault::remote_mounts::{
    validate_account, validate_name, validate_options, validate_server, validate_source,
    RemoteKind,
};

fuzz_target!(|data: &[u8]| {
    let text = String::from_utf8_lossy(data);
    let items: Vec<String> = text.split('\n').map(str::to_string).collect();

    let _ = validate_name(&text);
    let _ = validate_server(&text);
    for kind in [RemoteKind::Cifs, RemoteKind::Nfs] {
        let _ = validate_source(kind, &text);
        let _ = validate_options(kind, &items);
    }
    let _ = validate_account("username", Some(&text));
    let _ = validate_account("domain", Some(&text));
});

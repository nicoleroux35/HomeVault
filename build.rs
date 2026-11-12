//! Rebuild the binary whenever an embedded frontend asset changes.
//!
//! `src/handlers/spa.rs` embeds the whole `static/` tree with `include_dir!`.
//! Cargo cannot see through that macro, so without this script a `cargo build`
//! after `npm run build` can keep serving the previous `index-*.js` / CSS
//! chunks (the hashed filenames change but the old bytes stay embedded). The
//! script lists every path so an edited, added or removed asset invalidates
//! the crate exactly like a source change.

use std::path::Path;

fn walk(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        println!("cargo:rerun-if-changed={}", path.display());
        if path.is_dir() {
            walk(&path);
        }
    }
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    walk(Path::new("static"));
}

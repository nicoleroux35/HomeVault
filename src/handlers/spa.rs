use axum::{
    extract::Path,
    http::{header, HeaderValue, StatusCode},
    response::{Html, IntoResponse, Response},
};
use include_dir::{include_dir, Dir};

/// The whole `static/` tree (built Svelte frontend under `dist/`, plus
/// notification sounds under `sounds/`), embedded into the binary at
/// compile time. Previously served straight off disk via
/// `tower_http::services::ServeDir`/`std::fs::read_to_string`, which meant a
/// bare copy of the binary alone could never serve the UI — the caller also
/// had to ship `static/` alongside it, in the exact working directory the
/// process was launched from. Embedding removes that dependency entirely:
/// the binary is now the single artifact needed to run HomeVault.
static STATIC_DIR: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/static");

/// Look up `path` (relative to the embedded `static/` root, no leading
/// slash) and serve it with a content type guessed from its extension.
fn embedded_file_response(path: &str) -> Response {
    match STATIC_DIR.get_file(path) {
        Some(file) => {
            let mime = mime_guess::from_path(path).first_or_octet_stream();
            let content_type = HeaderValue::from_str(mime.as_ref())
                .unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream"));
            ([(header::CONTENT_TYPE, content_type)], file.contents()).into_response()
        }
        None => (StatusCode::NOT_FOUND, "Not found").into_response(),
    }
}

/// Serve the sprite.svg for Lucide icons
pub async fn serve_sprite() -> Response {
    embedded_file_response("dist/sprite.svg")
}

/// Serve the unified sprite with all icon libraries (Lucide + Phosphor + Tabler)
pub async fn serve_sprite_unified() -> Response {
    embedded_file_response("dist/sprite-unified.svg")
}

/// Serve a single named file from the embedded `dist/` root (favicon,
/// manifest, service worker, etc.)
pub async fn serve_dist_file(path: &str, _content_type: &str) -> Response {
    embedded_file_response(&format!("dist/{}", path))
}

/// Serve any file under `dist/assets/` (Vite's content-hashed JS/CSS output).
pub async fn serve_assets_file(Path(path): Path<String>) -> Response {
    embedded_file_response(&format!("dist/assets/{}", path))
}

/// Serve any file under `dist/fonts/`.
pub async fn serve_fonts_file(Path(path): Path<String>) -> Response {
    embedded_file_response(&format!("dist/fonts/{}", path))
}

/// Serve any file under `dist/wallpapers/` — the bundled desktop wallpapers
/// (`frontend/public/wallpapers/`, copied verbatim by `vite build`). An
/// explicit route is required: without it `/wallpapers/hv-01.webp` would
/// fall through to the SPA fallback and return `index.html`.
pub async fn serve_wallpapers_file(Path(path): Path<String>) -> Response {
    embedded_file_response(&format!("dist/wallpapers/{}", path))
}

/// Serve any file under the embedded `static/` root directly by its path
/// (replaces the old `ServeDir::new("static")` catch-all).
pub async fn serve_static_file(Path(path): Path<String>) -> Response {
    embedded_file_response(&path)
}

/// Serve the SPA index.html for all non-API routes
pub async fn serve_spa() -> Response {
    match STATIC_DIR.get_file("dist/index.html").and_then(|f| f.contents_utf8()) {
        Some(content) => Html(content).into_response(),
        None => (
            StatusCode::INTERNAL_SERVER_ERROR,
            "Failed to load application. Please rebuild the frontend."
        )
            .into_response(),
    }
}

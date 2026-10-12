//! Static-asset and SPA serving.
//!
//! Everything under `static/` is embedded into the binary at compile time, so
//! the binary is the single artifact needed to run HomeVault. On top of that,
//! this module gives every embedded file a validator (`ETag`) and a freshness
//! lifetime (`Cache-Control`), and — when the build produced pre-compressed
//! siblings (see `frontend/scripts/precompress-dist.mjs`) — serves the `br` /
//! `gzip` bytes directly instead of re-compressing on every request.

use axum::{
    extract::Path,
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{Html, IntoResponse, Response},
};
use include_dir::{include_dir, Dir};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::LazyLock;

/// The whole `static/` tree (built Svelte frontend under `dist/`, plus
/// notification sounds under `sounds/`), embedded into the binary at
/// compile time. Previously served straight off disk via
/// `tower_http::services::ServeDir`/`std::fs::read_to_string`, which meant a
/// bare copy of the binary alone could never serve the UI — the caller also
/// had to ship `static/` alongside it, in the exact working directory the
/// process was launched from. Embedding removes that dependency entirely:
/// the binary is now the single artifact needed to run HomeVault.
static STATIC_DIR: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/static");

/// Cache policy for files whose name *is* their identity: Vite's content-hashed
/// bundles, the bundled wallpapers, and the hand-authored files under
/// `static/`. None of them can change without a new release, and the hashed
/// ones change name when they change at all, so a year of `immutable` can
/// never pin a stale body.
const CACHE_IMMUTABLE: &str = "public, max-age=31536000, immutable";

/// Cache policy for the generated icon sprites.
///
/// Their URL is fixed (`/sprite-unified.svg`) but their content is regenerated
/// from the frontend sources on every build, so a release *can* add or drop
/// symbols under the same path. A year of `immutable` would then pin the old
/// sprite in every browser — the new icons would silently stay blank. One day
/// of freshness plus the `ETag` below keeps the steady-state cost at a `304`
/// while still letting an upgrade land.
const CACHE_REVALIDATE: &str = "public, max-age=86400";

/// One embedded file, plus the pre-compressed variants the build emitted next
/// to it (see `frontend/scripts/precompress-dist.mjs`).
struct Asset {
    /// Bytes exactly as committed to the repository.
    identity: &'static [u8],
    /// `<file>.br`, when the build produced one.
    brotli: Option<&'static [u8]>,
    /// `<file>.gz`, when the build produced one.
    gzip: Option<&'static [u8]>,
    content_type: HeaderValue,
    /// One validator per representation. HTTP requires the `ETag` to identify
    /// the *encoded* form: a cache that stored the identity body must not
    /// answer a later `Accept-Encoding: br` request with it, nor the reverse.
    etag_identity: HeaderValue,
    etag_brotli: Option<HeaderValue>,
    etag_gzip: Option<HeaderValue>,
}

/// Every embedded file, keyed by its path relative to `static/`
/// (`dist/index.html`, `sounds/…`). Built once, lazily, so the per-request
/// path does nothing but a hash lookup.
static ASSETS: LazyLock<HashMap<String, Asset>> = LazyLock::new(build_assets);

/// Recursively collect every file below `dir`.
///
/// `include_dir` only exposes one directory level at a time (`files()` /
/// `dirs()`), and there is no built-in recursive walk.
fn collect_files(dir: &'static Dir<'static>, out: &mut Vec<&'static include_dir::File<'static>>) {
    out.extend(dir.files());
    for sub in dir.dirs() {
        collect_files(sub, out);
    }
}

/// A strong cache validator for `bytes`, with an optional content-coding
/// suffix so each representation gets its own.
///
/// A truncated digest: 128 bits is far past collision risk for a cache
/// validator, and it keeps the header short. Derived from the *identity* bytes
/// even for the encoded variants — the encoding is a pure function of them, so
/// rehashing the compressed copy would buy nothing.
fn make_etag(bytes: &[u8], suffix: &str) -> HeaderValue {
    let digest = Sha256::digest(bytes);
    let mut quoted = String::with_capacity(34 + suffix.len());
    quoted.push('"');
    for byte in digest.iter().take(16) {
        quoted.push_str(&format!("{:02x}", byte));
    }
    quoted.push_str(suffix);
    quoted.push('"');
    HeaderValue::from_str(&quoted).unwrap_or_else(|_| HeaderValue::from_static("\"invalid\""))
}

/// Build the path → [`Asset`] table from the embedded tree.
fn build_assets() -> HashMap<String, Asset> {
    let mut files = Vec::new();
    collect_files(&STATIC_DIR, &mut files);

    // Index the raw bytes by path first: a `.br` / `.gz` sibling is attached to
    // its identity file afterwards, whatever order the walk happened to yield.
    let mut raw: HashMap<String, &'static [u8]> = HashMap::with_capacity(files.len());
    for file in files {
        raw.insert(file.path().to_string_lossy().into_owned(), file.contents());
    }

    let mut assets = HashMap::with_capacity(raw.len());
    for (path, identity) in &raw {
        // A compressed sibling is never addressable on its own — it is only
        // ever reachable through its identity path, after content negotiation.
        if path.ends_with(".br") || path.ends_with(".gz") {
            continue;
        }
        // Empty files have no useful representation to validate.
        if identity.is_empty() {
            continue;
        }

        let mime = mime_guess::from_path(path).first_or_octet_stream();
        let content_type = HeaderValue::from_str(mime.as_ref())
            .unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream"));

        let brotli = raw.get(&format!("{}.br", path)).copied();
        let gzip = raw.get(&format!("{}.gz", path)).copied();

        assets.insert(
            path.clone(),
            Asset {
                identity,
                brotli,
                gzip,
                content_type,
                etag_identity: make_etag(identity, ""),
                etag_brotli: brotli.map(|_| make_etag(identity, "-br")),
                etag_gzip: gzip.map(|_| make_etag(identity, "-gz")),
            },
        );
    }

    assets
}

/// Does the client's `Accept-Encoding` allow `coding`?
///
/// A missing header means identity only (RFC 9110 §12.5.3), so the answer is
/// `false`; an explicit `q=0` is a refusal; `*` covers anything not named, and
/// a `q=0` of its own can still veto it.
fn client_accepts(headers: &HeaderMap, coding: &str) -> bool {
    let Some(raw) = headers
        .get(header::ACCEPT_ENCODING)
        .and_then(|v| v.to_str().ok())
    else {
        return false;
    };

    let mut wildcard: Option<bool> = None;
    for entry in raw.split(',') {
        let mut params = entry.split(';');
        let name = params.next().unwrap_or("").trim();
        let refused = params.any(|p| {
            let p = p.trim();
            matches!(
                p.split_once('='),
                Some((key, value))
                    if key.trim().eq_ignore_ascii_case("q")
                        && matches!(value.trim().parse::<f32>(), Ok(q) if q == 0.0)
            )
        });

        if name.eq_ignore_ascii_case(coding) {
            return !refused;
        }
        if name == "*" {
            wildcard = Some(!refused);
        }
    }

    wildcard.unwrap_or(false)
}

/// `If-None-Match` evaluation for a `GET` (RFC 9110 §13.1.2): `*` matches any
/// existing representation, and the `W/` weakness prefix is ignored because
/// the comparison is weak here.
fn if_none_match_matches(headers: &HeaderMap, etag: &HeaderValue) -> bool {
    let Some(raw) = headers
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
    else {
        return false;
    };
    if raw.trim() == "*" {
        return true;
    }

    let ours = etag.to_str().unwrap_or("");
    raw.split(',').any(|candidate| {
        let candidate = candidate.trim();
        let candidate = candidate.strip_prefix("W/").unwrap_or(candidate);
        candidate == ours
    })
}

/// Serve `path` (relative to the embedded `static/` root) or `404`.
fn serve(path: &str, headers: &HeaderMap, cache_control: Option<&'static str>) -> Response {
    match ASSETS.get(path) {
        Some(asset) => asset_response(asset, headers, cache_control),
        None => (StatusCode::NOT_FOUND, "Not found").into_response(),
    }
}

/// Build the response for an already-resolved [`Asset`], with content
/// negotiation, a validator and — when `cache_control` is set — a freshness
/// lifetime.
fn asset_response(
    asset: &Asset,
    headers: &HeaderMap,
    cache_control: Option<&'static str>,
) -> Response {
    // `br` first (best ratio), then `gzip`, then the identity bytes that are
    // always embedded as the fallback.
    let mut chosen: (&'static [u8], Option<&'static str>, &HeaderValue) =
        (asset.identity, None, &asset.etag_identity);
    if let (Some(body), Some(etag)) = (asset.brotli, asset.etag_brotli.as_ref()) {
        if client_accepts(headers, "br") {
            chosen = (body, Some("br"), etag);
        }
    }
    if chosen.1.is_none() {
        if let (Some(body), Some(etag)) = (asset.gzip, asset.etag_gzip.as_ref()) {
            if client_accepts(headers, "gzip") {
                chosen = (body, Some("gzip"), etag);
            }
        }
    }
    let (body, encoding, etag) = chosen;

    let has_variants = asset.brotli.is_some() || asset.gzip.is_some();

    // Conditional GET: the client already has this exact byte sequence, so the
    // only thing to send back is the confirmation.
    if if_none_match_matches(headers, etag) {
        let mut res = StatusCode::NOT_MODIFIED.into_response();
        let out = res.headers_mut();
        out.insert(header::ETAG, etag.clone());
        if has_variants {
            out.insert(header::VARY, HeaderValue::from_static("accept-encoding"));
        }
        if let Some(cc) = cache_control {
            out.insert(header::CACHE_CONTROL, HeaderValue::from_static(cc));
        }
        return res;
    }

    let mut res = (StatusCode::OK, body).into_response();
    let out = res.headers_mut();
    out.insert(header::CONTENT_TYPE, asset.content_type.clone());
    out.insert(header::ETAG, etag.clone());
    if let Some(encoding) = encoding {
        // `tower_http::compression::CompressionLayer` never re-compresses a
        // response that already carries a `Content-Encoding`, so this both
        // marks the body and opts it out of the on-the-fly compressor.
        out.insert(header::CONTENT_ENCODING, HeaderValue::from_static(encoding));
    }
    if has_variants {
        out.insert(header::VARY, HeaderValue::from_static("accept-encoding"));
    }
    if let Some(cc) = cache_control {
        out.insert(header::CACHE_CONTROL, HeaderValue::from_static(cc));
    }
    res
}

/// Serve the sprite.svg for Lucide icons
pub async fn serve_sprite(headers: HeaderMap) -> Response {
    serve("dist/sprite.svg", &headers, Some(CACHE_REVALIDATE))
}

/// Serve the unified sprite with all icon libraries (Lucide + Phosphor + Tabler)
pub async fn serve_sprite_unified(headers: HeaderMap) -> Response {
    serve("dist/sprite-unified.svg", &headers, Some(CACHE_REVALIDATE))
}

/// Serve a single named file from the embedded `dist/` root (favicon,
/// manifest, service worker, etc.).
///
/// No `Cache-Control` is set on purpose: these names are stable but their
/// content is not content-hashed (`sw.js` in particular *must* be allowed to
/// update), so the browser keeps them with its own heuristics and revalidates
/// through the `ETag`.
pub async fn serve_dist_file(path: &'static str, headers: HeaderMap) -> Response {
    serve(&format!("dist/{}", path), &headers, None)
}

/// Serve any file under `dist/assets/` (Vite's content-hashed JS/CSS output).
pub async fn serve_assets_file(Path(path): Path<String>, headers: HeaderMap) -> Response {
    serve(
        &format!("dist/assets/{}", path),
        &headers,
        Some(CACHE_IMMUTABLE),
    )
}

/// Serve any file under `dist/fonts/`.
pub async fn serve_fonts_file(Path(path): Path<String>, headers: HeaderMap) -> Response {
    serve(
        &format!("dist/fonts/{}", path),
        &headers,
        Some(CACHE_IMMUTABLE),
    )
}

/// Serve any file under `dist/wallpapers/` — the bundled desktop wallpapers
/// (`frontend/public/wallpapers/`, copied verbatim by `vite build`). An
/// explicit route is required: without it `/wallpapers/hv-01.webp` would
/// fall through to the SPA fallback and return `index.html`.
pub async fn serve_wallpapers_file(Path(path): Path<String>, headers: HeaderMap) -> Response {
    serve(
        &format!("dist/wallpapers/{}", path),
        &headers,
        Some(CACHE_IMMUTABLE),
    )
}

/// Serve any file under the embedded `static/` root directly by its path
/// (replaces the old `ServeDir::new("static")` catch-all).
pub async fn serve_static_file(Path(path): Path<String>, headers: HeaderMap) -> Response {
    serve(&path, &headers, Some(CACHE_IMMUTABLE))
}

/// Serve the SPA index.html for all non-API routes.
///
/// Deliberately not cached and not validated: `index.html` is what points the
/// browser at the currently-hashed bundles, so a stale copy would keep an
/// upgraded client on a previous release forever.
pub async fn serve_spa() -> Response {
    match STATIC_DIR
        .get_file("dist/index.html")
        .and_then(|f| f.contents_utf8())
    {
        Some(content) => Html(content).into_response(),
        None => (
            StatusCode::INTERNAL_SERVER_ERROR,
            "Failed to load application. Please rebuild the frontend.",
        )
            .into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const IDENTITY: &[u8] = b"body{color:red} /* a body long enough to compress */";
    const BROTLI: &[u8] = b"\x0b\x02\x80body";
    const GZIP: &[u8] = b"\x1f\x8b\x08\x00gz";

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (name, value) in pairs {
            map.insert(
                header::HeaderName::from_bytes(name.as_bytes()).unwrap(),
                HeaderValue::from_str(value).unwrap(),
            );
        }
        map
    }

    /// An `Asset` with no content-coding variants.
    fn plain() -> Asset {
        Asset {
            identity: IDENTITY,
            brotli: None,
            gzip: None,
            content_type: HeaderValue::from_static("text/css"),
            etag_identity: make_etag(IDENTITY, ""),
            etag_brotli: None,
            etag_gzip: None,
        }
    }

    /// An `Asset` with both pre-compressed variants.
    fn compressed() -> Asset {
        Asset {
            brotli: Some(BROTLI),
            gzip: Some(GZIP),
            etag_brotli: Some(make_etag(IDENTITY, "-br")),
            etag_gzip: Some(make_etag(IDENTITY, "-gz")),
            ..plain()
        }
    }

    fn header(res: &Response, name: header::HeaderName) -> Option<&str> {
        res.headers().get(name).and_then(|v| v.to_str().ok())
    }

    // =====================================================================
    // make_etag
    // =====================================================================

    #[test]
    fn etag_is_quoted_and_coding_specific() {
        assert_eq!(make_etag(IDENTITY, "").to_str().unwrap().len(), 34);
        assert!(make_etag(IDENTITY, "").to_str().unwrap().starts_with('"'));
        assert!(make_etag(IDENTITY, "").to_str().unwrap().ends_with('"'));
        assert_ne!(make_etag(IDENTITY, ""), make_etag(IDENTITY, "-br"));
        assert_ne!(make_etag(IDENTITY, ""), make_etag(b"other", ""));
    }

    #[test]
    fn etag_is_stable_across_calls() {
        assert_eq!(make_etag(IDENTITY, ""), make_etag(IDENTITY, ""));
    }

    // =====================================================================
    // client_accepts
    // =====================================================================

    #[test]
    fn accepts_brotli_when_listed() {
        let h = headers(&[("accept-encoding", "gzip, deflate, br, zstd")]);
        assert!(client_accepts(&h, "br"));
        assert!(client_accepts(&h, "gzip"));
    }

    #[test]
    fn missing_header_means_identity_only() {
        // RFC 9110 §12.5.3 — no `Accept-Encoding` is not "accept everything".
        let h = headers(&[]);
        assert!(!client_accepts(&h, "br"));
        assert!(!client_accepts(&h, "gzip"));
    }

    #[test]
    fn explicit_q_zero_is_a_refusal() {
        let h = headers(&[("accept-encoding", "gzip, deflate, identity")]);
        assert!(!client_accepts(&h, "br"));
    }

    #[test]
    fn q_zero_on_the_named_coding_overrides_its_listing() {
        let h = headers(&[("accept-encoding", "br;q=0, gzip")]);
        assert!(!client_accepts(&h, "br"));
        assert!(client_accepts(&h, "gzip"));
    }

    #[test]
    fn wildcard_covers_unlisted_codings() {
        let h = headers(&[("accept-encoding", "*")]);
        assert!(client_accepts(&h, "br"));
        assert!(client_accepts(&h, "gzip"));

        let h = headers(&[("accept-encoding", "*;q=0")]);
        assert!(!client_accepts(&h, "br"));
    }

    #[test]
    fn matching_is_case_insensitive_and_ignores_other_parameters() {
        let h = headers(&[("accept-encoding", "GZIP;q=0.8, BR;q=1.0")]);
        assert!(client_accepts(&h, "br"));
        assert!(client_accepts(&h, "gzip"));
    }

    // =====================================================================
    // if_none_match_matches
    // =====================================================================

    #[test]
    fn no_if_none_match_never_matches() {
        let h = headers(&[]);
        assert!(!if_none_match_matches(&h, &make_etag(IDENTITY, "")));
    }

    #[test]
    fn exact_etag_matches() {
        let etag = make_etag(IDENTITY, "");
        let h = headers(&[("if-none-match", etag.to_str().unwrap())]);
        assert!(if_none_match_matches(&h, &etag));
        assert!(!if_none_match_matches(&h, &make_etag(b"other", "")));
    }

    #[test]
    fn weak_prefix_is_ignored() {
        let etag = make_etag(IDENTITY, "");
        let h = headers(&[("if-none-match", &format!("W/{}", etag.to_str().unwrap()))]);
        assert!(if_none_match_matches(&h, &etag));
    }

    #[test]
    fn star_matches_any_representation() {
        let h = headers(&[("if-none-match", "*")]);
        assert!(if_none_match_matches(&h, &make_etag(IDENTITY, "")));
    }

    #[test]
    fn one_of_a_list_matches() {
        let etag = make_etag(IDENTITY, "");
        let h = headers(&[(
            "if-none-match",
            &format!("\"aaa\", {}, \"ccc\"", etag.to_str().unwrap()),
        )]);
        assert!(if_none_match_matches(&h, &etag));
        assert!(!if_none_match_matches(&h, &make_etag(b"other", "")));
    }

    #[test]
    fn each_content_coding_gets_its_own_validator() {
        // Guards the reason `Asset` stores three ETags: a cache holding the
        // identity body must not validate against the `br` request.
        let asset = compressed();
        let h = headers(&[("if-none-match", asset.etag_identity.to_str().unwrap())]);

        let identity_req = asset_response(&asset, &h, None);
        assert_eq!(identity_req.status(), StatusCode::NOT_MODIFIED);

        let h = headers(&[(
            "if-none-match",
            asset.etag_brotli.as_ref().unwrap().to_str().unwrap(),
        )]);
        // The br validator is not the identity one, so a plain request must
        // still get a full body.
        let plain_req = asset_response(&asset, &h, None);
        assert_eq!(plain_req.status(), StatusCode::OK);
    }

    // =====================================================================
    // asset_response — content negotiation + headers
    // =====================================================================

    #[test]
    fn identity_is_served_when_nothing_is_accepted() {
        let res = asset_response(&plain(), &headers(&[]), Some(CACHE_IMMUTABLE));
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(header(&res, header::CONTENT_ENCODING), None);
        assert_eq!(header(&res, header::CACHE_CONTROL), Some(CACHE_IMMUTABLE));
        assert!(header(&res, header::ETAG).is_some());
        // No variants embedded -> nothing to vary on.
        assert_eq!(header(&res, header::VARY), None);
    }

    #[test]
    fn brotli_wins_over_gzip_when_both_are_offered() {
        let h = headers(&[("accept-encoding", "gzip, br")]);
        let res = asset_response(&compressed(), &h, None);
        assert_eq!(header(&res, header::CONTENT_ENCODING), Some("br"));
        // The variant header is load-bearing: without it a shared cache could
        // hand the brotli body to a client that never asked for it.
        assert_eq!(header(&res, header::VARY), Some("accept-encoding"));
        // And the validator must be the one for the *encoded* body.
        assert_eq!(
            header(&res, header::ETAG),
            Some(make_etag(IDENTITY, "-br").to_str().unwrap())
        );
    }

    #[test]
    fn gzip_is_used_when_brotli_is_not_offered() {
        let h = headers(&[("accept-encoding", "gzip")]);
        let res = asset_response(&compressed(), &h, None);
        assert_eq!(header(&res, header::CONTENT_ENCODING), Some("gzip"));
        assert_eq!(
            header(&res, header::ETAG),
            Some(make_etag(IDENTITY, "-gz").to_str().unwrap())
        );
    }

    #[test]
    fn identity_falls_back_when_only_variants_exist_but_none_is_accepted() {
        let res = asset_response(&compressed(), &headers(&[]), None);
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(header(&res, header::CONTENT_ENCODING), None);
        assert_eq!(
            header(&res, header::ETAG),
            Some(make_etag(IDENTITY, "").to_str().unwrap())
        );
        // `Vary` is still required: the response *would* differ.
        assert_eq!(header(&res, header::VARY), Some("accept-encoding"));
    }

    #[test]
    fn conditional_get_returns_304_without_a_body() {
        let asset = compressed();
        let etag = make_etag(IDENTITY, "-br").to_str().unwrap().to_string();
        let h = headers(&[("accept-encoding", "br"), ("if-none-match", etag.as_str())]);

        let res = asset_response(&asset, &h, Some(CACHE_IMMUTABLE));
        assert_eq!(res.status(), StatusCode::NOT_MODIFIED);
        assert_eq!(header(&res, header::CACHE_CONTROL), Some(CACHE_IMMUTABLE));
        assert_eq!(header(&res, header::VARY), Some("accept-encoding"));
        // A 304 must not re-send the encoding: it describes the stored body.
        assert_eq!(header(&res, header::CONTENT_ENCODING), None);
    }

    #[test]
    fn conditional_get_on_the_identity_validator_still_sends_the_body_when_br_is_picked() {
        // The client holds the identity representation but now accepts br, so
        // the validators differ and the full (encoded) body must come back.
        let asset = compressed();
        let identity_etag = asset.etag_identity.to_str().unwrap().to_string();
        let h = headers(&[
            ("accept-encoding", "br"),
            ("if-none-match", identity_etag.as_str()),
        ]);

        let res = asset_response(&asset, &h, None);
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(header(&res, header::CONTENT_ENCODING), Some("br"));
    }

    #[test]
    fn content_type_is_mime_guessed_from_the_extension() {
        assert_eq!(
            header(
                &asset_response(&plain(), &headers(&[]), None),
                header::CONTENT_TYPE
            ),
            Some("text/css")
        );
    }
}

/* =============================================================================
 * File          : global_search.rs
 * Author        : Nicolas Le Roux
 * Created       : 31/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   The cross-root file provider behind the desktop's global search (QW-12 /
 *   DSM's Universal Search): one query fanned out over every root the caller
 *   may read, merged, deduplicated, ranked and capped, so "find this file
 *   anywhere I can read" stops meaning "open the Explorer and guess which
 *   share it is in".
 *
 *   Two contracts the callers rely on:
 *   - **The ACL filter is the Explorer's, not a second one.** Roots come from
 *     `PathGuard` for a non-admin session and from the HomeVault share ACLs
 *     when the box has them; a file the Explorer would refuse to open can
 *     never appear here.
 *   - **Bounded, always.** Query length, root count, per-root time budget and
 *     total result count all have hard ceilings, so a wedged network mount can
 *     slow one answer but never stall the ⌘K overlay.
 *
 * Dependencies:
 *   - serde (>= 1.0), tokio (spawn_blocking + timeout), crate::browser
 *
 * Usage:
 *   handlers/search.rs (`GET /api/search/files`); the desktop search overlay.
 * =============================================================================
 */

//! Cross-root file search (QW-12), see docs/guides/explorer.md.
//!
//! Content indexing is explicitly out of scope: this is a **name** search, and
//! the module says so in its own type names so nobody mistakes it for one.

use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::browser::FileInfo;

/// Longest query the provider will run. A search box query longer than this
/// is a paste accident, not a search.
pub const MAX_QUERY_LEN: usize = 128;

/// Shortest query worth a filesystem walk: one character matches everything
/// and would turn ⌘K into a `find /` on every keystroke.
pub const MIN_QUERY_LEN: usize = 2;

/// Hard ceiling on the roots one query may fan out over. A share list with
/// hundreds of entries must not become hundreds of concurrent walks.
pub const MAX_ROOTS: usize = 16;

/// Hard ceiling on the merged answer, whatever the caller asked for.
pub const MAX_RESULTS: usize = 100;

/// Per-root wall-clock budget. A slow or hung mount degrades to "this root did
/// not answer" (`timed_out`), never to a stalled overlay.
pub const PER_ROOT_TIMEOUT: Duration = Duration::from_millis(1_500);

/// Overall budget for the whole fan-out, on top of the per-root one.
pub const TOTAL_TIMEOUT: Duration = Duration::from_millis(4_000);

/// One root as the provider sees it: the path to walk plus a label for the UI
/// (the share name, the user's home, the pool name).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchRoot {
    pub path: String,
    /// Stable machine key (`home`, `users`, `shares`, `storage`…), mapped to a
    /// translated label by the frontend.
    pub kind: String,
    /// Human fallback when there is no i18n key for `kind`.
    pub name: String,
}

/// One hit, with the root that produced it so the UI can say where the file
/// lives rather than only which folder.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SearchHit {
    #[serde(flatten)]
    pub info: FileInfo,
    /// Path of the root this hit was found under.
    pub root: String,
    pub root_kind: String,
}

/// The whole answer, including why it may be incomplete. A search that hides
/// its own truncation is worse than no search.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SearchReport {
    pub query: String,
    pub hits: Vec<SearchHit>,
    /// Roots that were walked.
    pub roots: Vec<SearchRoot>,
    /// Roots whose walk exceeded its budget (their hits may be missing).
    pub timed_out: Vec<String>,
    /// Roots that could not be resolved or read at all.
    pub unreadable: Vec<String>,
    /// `true` when the answer was cut at the cap, so the UI can say "refine".
    pub truncated: bool,
    /// How long the whole fan-out took, in milliseconds.
    pub elapsed_ms: u64,
    /// The tag the answer was filtered by, when the caller asked for one. Echoed
    /// so a late answer cannot be mistaken for the unfiltered list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,
}

/// Why a query was refused, or `Ok` with the normalised query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueryError {
    /// Empty or whitespace-only.
    Empty,
    /// Shorter than [`MIN_QUERY_LEN`] once trimmed.
    TooShort,
    /// Longer than [`MAX_QUERY_LEN`].
    TooLong,
    /// Contains NUL or path-separator characters — a query names a file, it
    /// is never a path and never a traversal.
    InvalidChar,
}

impl QueryError {
    pub fn message(&self) -> String {
        match self {
            QueryError::Empty => "Enter a file name to search for".to_string(),
            QueryError::TooShort => {
                format!("Type at least {MIN_QUERY_LEN} characters to search for a file")
            }
            QueryError::TooLong => {
                format!("That search is too long: keep it under {MAX_QUERY_LEN} characters")
            }
            QueryError::InvalidChar => {
                "A file name search cannot contain '/' or a NUL byte".to_string()
            }
        }
    }
}

/// Validate and normalise a search query. Pure, so the whole matrix is
/// unit-tested without touching a filesystem.
pub fn validate_query(raw: &str) -> Result<String, QueryError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(QueryError::Empty);
    }
    if trimmed.contains('\0') || trimmed.contains('/') || trimmed.contains('\\') {
        return Err(QueryError::InvalidChar);
    }
    if trimmed.chars().count() > MAX_QUERY_LEN {
        return Err(QueryError::TooLong);
    }
    if trimmed.chars().count() < MIN_QUERY_LEN {
        return Err(QueryError::TooShort);
    }
    Ok(trimmed.to_lowercase())
}

/// Clamp a caller-supplied `limit` into `1..=MAX_RESULTS`.
pub fn clamp_limit(requested: Option<u32>) -> usize {
    match requested {
        Some(n) => (n as usize).clamp(1, MAX_RESULTS),
        None => 30,
    }
}

/// Deduplicate roots by resolved path (two share entries can point at the same
/// folder), keeping the first — which is the more specific label in every
/// caller — and refusing to fan out over more than [`MAX_ROOTS`].
pub fn dedupe_roots(roots: Vec<SearchRoot>) -> Vec<SearchRoot> {
    let mut seen: HashSet<PathBuf> = HashSet::new();
    let mut out = Vec::with_capacity(roots.len().min(MAX_ROOTS));
    for root in roots {
        if root.path.trim().is_empty() {
            continue;
        }
        // Resolve symlinks so `/mnt/data` and `/mnt/data-link` are one root;
        // an unresolvable path is kept as-is (the walk will report it).
        let key = std::fs::canonicalize(&root.path).unwrap_or_else(|_| PathBuf::from(&root.path));
        if !seen.insert(key) {
            continue;
        }
        out.push(root);
        if out.len() >= MAX_ROOTS {
            break;
        }
    }
    out
}

/// Merge hits from every root: deduplicate by path (a symlinked duplicate in
/// two roots must appear once), rank exact name matches first then shorter
/// names (a query matches "Taxes.pdf" before "2024-taxes-archive-final.pdf"),
/// directories before files at equal rank, and stop at `limit`.
///
/// Pure — no filesystem access — so the ranking and dedupe rules are
/// unit-tested directly.
pub fn merge_hits(mut hits: Vec<SearchHit>, query: &str, limit: usize) -> (Vec<SearchHit>, bool) {
    let needle = query.to_lowercase();
    let mut seen: HashSet<String> = HashSet::new();
    hits.retain(|hit| seen.insert(hit.info.path.clone()));

    hits.sort_by(|a, b| {
        let rank = |hit: &SearchHit| -> (u8, usize, u8, String) {
            let name = hit.info.name.to_lowercase();
            let name_rank = if name == needle {
                0
            } else if name.starts_with(&needle) {
                1
            } else {
                2
            };
            let dir_rank = if hit.info.is_directory { 0 } else { 1 };
            (
                name_rank,
                name.chars().count(),
                dir_rank,
                hit.info.path.clone(),
            )
        };
        rank(a).cmp(&rank(b))
    });

    let truncated = hits.len() > limit;
    hits.truncate(limit);
    (hits, truncated)
}

/// Pure post-processing for one root's raw results: tag each hit with the root
/// that produced it. Extracted so the tagging is testable without a walk.
pub fn tag_hits(root: &SearchRoot, infos: Vec<FileInfo>) -> Vec<SearchHit> {
    infos
        .into_iter()
        .map(|info| SearchHit {
            info,
            root: root.path.clone(),
            root_kind: root.kind.clone(),
        })
        .collect()
}

/// Path-confinement check for a hit: it must live under the root that produced
/// it. The walk cannot produce anything else today, but the answer is
/// serialized straight to a client that opens it, so the rule is asserted
/// rather than assumed.
pub fn hit_is_confined(hit: &SearchHit) -> bool {
    let root = Path::new(&hit.root);
    let path = Path::new(&hit.info.path);
    path == root || path.starts_with(root)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file_info(name: &str, path: &str, is_directory: bool) -> FileInfo {
        FileInfo {
            name: name.to_string(),
            path: path.to_string(),
            is_directory,
            size_bytes: 0,
            modified: String::new(),
            permissions: String::new(),
            owner: String::new(),
            filesystem: None,
        }
    }

    fn hit(root: &str, name: &str, path: &str, is_directory: bool) -> SearchHit {
        SearchHit {
            info: file_info(name, path, is_directory),
            root: root.to_string(),
            root_kind: "storage".to_string(),
        }
    }

    // -----------------------------------------------------------------
    // Query validation
    // -----------------------------------------------------------------

    #[test]
    fn test_validate_query_rejects_empty_and_whitespace() {
        assert_eq!(validate_query(""), Err(QueryError::Empty));
        assert_eq!(validate_query("   "), Err(QueryError::Empty));
        assert_eq!(validate_query("\t\n"), Err(QueryError::Empty));
    }

    #[test]
    fn test_validate_query_needs_two_characters() {
        assert_eq!(validate_query("a"), Err(QueryError::TooShort));
        assert_eq!(validate_query(" a "), Err(QueryError::TooShort));
        assert_eq!(validate_query("ab").as_deref(), Ok("ab"));
    }

    #[test]
    fn test_validate_query_lowercases_and_trims() {
        assert_eq!(
            validate_query("  Report.PDF  ").as_deref(),
            Ok("report.pdf")
        );
        assert_eq!(validate_query("ÉTÉ").as_deref(), Ok("été"));
    }

    #[test]
    fn test_validate_query_refuses_a_path_shaped_query() {
        // A query names a file; it is never a path and never a traversal.
        assert_eq!(validate_query("a/b"), Err(QueryError::InvalidChar));
        assert_eq!(validate_query("../../etc"), Err(QueryError::InvalidChar));
        assert_eq!(validate_query("a\\b"), Err(QueryError::InvalidChar));
        assert_eq!(validate_query("ab\0cd"), Err(QueryError::InvalidChar));
    }

    #[test]
    fn test_validate_query_bounds_the_length() {
        let at_limit = "x".repeat(MAX_QUERY_LEN);
        assert!(validate_query(&at_limit).is_ok());
        let over = "x".repeat(MAX_QUERY_LEN + 1);
        assert_eq!(validate_query(&over), Err(QueryError::TooLong));
    }

    #[test]
    fn test_validate_query_counts_characters_not_bytes() {
        // 128 multi-byte characters are 128 characters, not 384 bytes: a
        // byte-based cap would refuse a legitimate accented file name.
        let accented = "é".repeat(MAX_QUERY_LEN);
        assert!(validate_query(&accented).is_ok());
    }

    #[test]
    fn test_query_error_messages_name_the_rule() {
        assert!(QueryError::TooShort.message().contains('2'));
        assert!(QueryError::TooLong
            .message()
            .contains(&MAX_QUERY_LEN.to_string()));
        assert!(QueryError::InvalidChar.message().contains('/'));
    }

    // -----------------------------------------------------------------
    // Limit clamping
    // -----------------------------------------------------------------

    #[test]
    fn test_clamp_limit_defaults_and_bounds() {
        assert_eq!(clamp_limit(None), 30);
        assert_eq!(clamp_limit(Some(0)), 1);
        assert_eq!(clamp_limit(Some(5)), 5);
        assert_eq!(clamp_limit(Some(MAX_RESULTS as u32)), MAX_RESULTS);
        assert_eq!(clamp_limit(Some(10_000)), MAX_RESULTS);
    }

    // -----------------------------------------------------------------
    // Root fan-out
    // -----------------------------------------------------------------

    #[test]
    fn test_dedupe_roots_collapses_the_same_folder() {
        let roots = dedupe_roots(vec![
            SearchRoot {
                path: "/tmp".to_string(),
                kind: "storage".to_string(),
                name: "tmp".to_string(),
            },
            // The same folder reached by a trailing slash and by `..`.
            SearchRoot {
                path: "/tmp/../tmp".to_string(),
                kind: "shares".to_string(),
                name: "tmp again".to_string(),
            },
        ]);
        assert_eq!(roots.len(), 1, "one folder, one walk: {roots:?}");
        assert_eq!(roots[0].kind, "storage", "the first label wins");
    }

    #[test]
    fn test_dedupe_roots_drops_blank_paths_and_caps_the_fan_out() {
        let mut roots = vec![SearchRoot {
            path: "   ".to_string(),
            kind: "storage".to_string(),
            name: "blank".to_string(),
        }];
        for i in 0..(MAX_ROOTS + 5) {
            roots.push(SearchRoot {
                path: format!("/tmp/hv-root-{i}"),
                kind: "storage".to_string(),
                name: format!("root {i}"),
            });
        }
        let deduped = dedupe_roots(roots);
        assert_eq!(deduped.len(), MAX_ROOTS, "the fan-out is capped");
        assert!(deduped.iter().all(|r| !r.path.trim().is_empty()));
    }

    // -----------------------------------------------------------------
    // Merge, dedupe and ranking
    // -----------------------------------------------------------------

    #[test]
    fn test_merge_hits_deduplicates_by_path() {
        let hits = vec![
            hit("/srv", "notes.txt", "/srv/notes.txt", false),
            hit("/mnt", "notes.txt", "/srv/notes.txt", false),
        ];
        let (merged, truncated) = merge_hits(hits, "notes.txt", 10);
        assert_eq!(merged.len(), 1);
        assert!(!truncated);
    }

    #[test]
    fn test_merge_hits_ranks_exact_then_prefix_then_substring() {
        let hits = vec![
            hit(
                "/srv",
                "2024-taxes-archive-final.pdf",
                "/srv/2024-taxes-archive-final.pdf",
                false,
            ),
            hit("/srv", "taxes-draft.pdf", "/srv/taxes-draft.pdf", false),
            hit("/srv", "taxes.pdf", "/srv/taxes.pdf", false),
        ];
        let (merged, _) = merge_hits(hits, "taxes", 10);
        let names: Vec<&str> = merged.iter().map(|h| h.info.name.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "taxes.pdf",
                "taxes-draft.pdf",
                "2024-taxes-archive-final.pdf"
            ],
            "exact name first, then prefix, then a name that merely contains the query"
        );
    }

    #[test]
    fn test_merge_hits_prefers_a_directory_over_a_file_at_equal_rank() {
        let hits = vec![
            hit("/srv", "photos", "/srv/photos-2019", false),
            hit("/srv", "photos", "/srv/photos", true),
        ];
        let (merged, _) = merge_hits(hits, "photos", 10);
        assert!(merged[0].info.is_directory);
        assert_eq!(merged[0].info.path, "/srv/photos");
    }

    #[test]
    fn test_merge_hits_is_deterministic_for_identical_rank() {
        // Same name, same length, same kind: the path breaks the tie so two
        // identical runs cannot reorder the list under the user.
        let hits = vec![
            hit("/srv", "report.pdf", "/srv/b/report.pdf", false),
            hit("/srv", "report.pdf", "/srv/a/report.pdf", false),
        ];
        let (first, _) = merge_hits(hits.clone(), "report.pdf", 10);
        let (second, _) = merge_hits(hits, "report.pdf", 10);
        assert_eq!(first, second);
        assert_eq!(first[0].info.path, "/srv/a/report.pdf");
    }

    #[test]
    fn test_merge_hits_reports_truncation() {
        let hits: Vec<SearchHit> = (0..5)
            .map(|i| {
                hit(
                    "/srv",
                    &format!("f{i}.txt"),
                    &format!("/srv/f{i}.txt"),
                    false,
                )
            })
            .collect();
        let (merged, truncated) = merge_hits(hits.clone(), "f", 2);
        assert_eq!(merged.len(), 2);
        assert!(truncated, "the caller must be told the answer was cut");

        let (all, truncated) = merge_hits(hits, "f", 10);
        assert_eq!(all.len(), 5);
        assert!(!truncated, "an answer under the cap is not truncated");
    }

    // -----------------------------------------------------------------
    // Tagging and confinement
    // -----------------------------------------------------------------

    #[test]
    fn test_tag_hits_carries_the_root_of_each_hit() {
        let root = SearchRoot {
            path: "/srv".to_string(),
            kind: "shares".to_string(),
            name: "srv".to_string(),
        };
        let tagged = tag_hits(&root, vec![file_info("a.txt", "/srv/a.txt", false)]);
        assert_eq!(tagged.len(), 1);
        assert_eq!(tagged[0].root, "/srv");
        assert_eq!(tagged[0].root_kind, "shares");
    }

    #[test]
    fn test_hit_is_confined_to_its_root() {
        assert!(hit_is_confined(&hit("/srv", "a", "/srv/a", false)));
        assert!(hit_is_confined(&hit("/srv", "srv", "/srv", true)));
        // A sibling with a shared prefix is not inside the root.
        assert!(!hit_is_confined(&hit("/srv", "x", "/srv-other/x", false)));
        assert!(!hit_is_confined(&hit("/srv", "x", "/etc/passwd", false)));
    }
}

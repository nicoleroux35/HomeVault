# File tags

**Purpose:** attach private labels to files and folders so they can be found again without exposing paths to other accounts.

## Overview

Explorer tags are private to the account that created them. A tag is attached to a canonical path, is limited to 32 visible characters, and each path can have at most 20 tags. Tags are stored in SQLite for as long as the owner account exists; deleting that account purges its tags. Tags do not grant access: reads and writes validate the target against the same Explorer roots as browsing, and ⌘K tag filtering first searches the caller's currently accessible roots and only then filters those results. A stale tag can therefore never disclose an inaccessible path.

This feature stores account-linked organizational metadata locally; it does not send tags or paths to a third party. The controller is the appliance operator, who should remove stale account data through normal account deletion.

## API

All endpoints require a valid session. Paths are canonicalized and checked against the caller's Explorer roots.

| Method | Path | Request | Response |
|---|---|---|---|
| GET | `/explorer/tags` | optional `?path=` | `{ success, data: [{ path, tag, created_at }] }`, own tags only |
| POST | `/explorer/tags` | `{ path, tag }` | created/existing tag row; duplicate creation is idempotent |
| DELETE | `/explorer/tags` | `{ path, tag }` | `{ removed: true }`; unknown tag is an error. Works on a path that no longer exists — a label a deleted file left behind is exactly what the sidebar × clears. Only the caller's own rows are reachable, so a foreign tag is `NotFound` |
| GET | `/search/files` | `?q=&tag=` | name-search report filtered to the caller's own tag |

The table is `explorer_file_tags` (migration 075). `FileTagRepository` owns storage and per-path bounds; the Explorer handlers validate access before calling it.

## Frontend

The Explorer sidebar has a **File tags** section: a small form that tags the current folder (refused at `/`), the current folder's tags, and a "Show tags from all folders" switch that lists the account's whole tag set. Clicking a tag row opens the folder it labels; the × beside it removes that tag. Adding or removing a tag reports through the app toast, never a modal, so several tags can be added in a row.

The desktop ⌘K overlay offers the caller's tags in a filter next to the query as soon as the account has any. While a tag is selected, each file row carries a tag action that applies that same tag to the hit — tagging from scratch stays in the Explorer sidebar, so the overlay never has to invent a label. The regular file-name query rules and navigation-result priority remain unchanged.

## Invariants

- A tag is metadata, never an authorization grant.
- Every endpoint uses the session username as owner; owner is never accepted from the request.
- Adding requires an existing, accessible path; removing requires only ownership, so a label on a deleted file stays clearable.
- Tag filtering is applied only to results of the ACL-confined global search.
- An invalid or unknown path is rejected without distinguishing whether it exists outside the caller's roots.
- Account deletion removes tags; duplicate tags are case-insensitive and idempotent.
- No filesystem walk is initiated by tag listing or deletion.

## Related documentation

- [File Explorer](./explorer.md)
- [Global search](./global-search.md)
- [API conventions](../API.md)

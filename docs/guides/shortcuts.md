# Web shortcuts

**Purpose:** Pin web pages as icons on the desktop. Each shortcut stores the
page's title, description, favicon and origin, fetched once when it is added.
The metadata fetch is server-side and SSRF-guarded. Shortcuts are **added from
the launcher** (its last tile) and only *shown* on the desktop.

## Backend

Handler in `src/handlers/shortcuts.rs`.

| Endpoint | Auth | Notes |
|----------|------|-------|
| `POST /shortcuts/fetch-metadata` | auth | `{ url }` → `{ url, title, description, icon_url, site_name }` |

The endpoint makes the server perform an outbound HTTP request to a
user-supplied URL, so the SSRF guards are the boundary, not a nicety:

1. **Scheme allow-list** — `http` / `https` only. A bare host is upgraded to
   `https://`.
2. **Resolution check** — the host is resolved and **every** address must be
   public (`is_public_ip`). Loopback, private, link-local, unique-local,
   multicast, broadcast, documentation and unspecified addresses are refused.
   IPv4-mapped IPv6 (`::ffff:127.0.0.1`) is unwrapped first, so it cannot slip
   through. This blocks `http://169.254.169.254/` (cloud metadata) and
   `http://127.0.0.1:8080/` (the admin UI).
3. **No automatic redirects — every hop is re-validated** — reqwest's redirect
   policy is `none`. Up to `MAX_REDIRECTS` (5) hops are resolved manually: the
   `Location` is joined against the previous *absolute* URL, the scheme is
   checked against rule 1, the host is resolved and checked against rule 2, and
   only then is the hop dialled. Only `301/302/303/307/308` are followed —
   `300`, `304`, `305` and `306` end the chain (they used to be read as a
   redirect and answered `Redirect without a Location header`). A longer chain
   answers `400` with `Too many redirects (at most 5 are followed)`; a hop
   pointing at `file:` (or any non-http scheme) is refused exactly like a
   top-level one. *This used to follow exactly one hop, so a second redirect's
   empty body was parsed as "the page" and the metadata silently came back
   empty on every short-link / CDN chain.*
4. **Bounded work** — one 10 s budget for the **whole chain** (every `send` and
   the body read share the same deadline; it is not a per-hop timeout) and a
   512 KB cap enforced **while the body streams in**: `read_capped_stream`
   stops pulling as soon as the cap is reached and drops the rest of the
   response. The old code awaited `Response::bytes()`, which materialised the
   entire body before truncating it, so the cap bounded the parser and not the
   server's memory.
5. **Connect-time address pinning** — the shared client resolves through
   `PublicOnlyResolver` (`reqwest::dns::Resolve`), which refuses any answer
   containing a non-public address. reqwest re-resolves when it dials, so the
   pre-check alone left a window in which a hostile DNS answer could change
   between validation and connection (DNS rebinding); with the resolver, the
   address that is dialled is the one that was just validated, in the same
   resolution. The pre-check in `resolve_public` stays: it produces the precise
   error message and covers literal IPs (hyper dials those without asking a
   resolver at all).

One `reqwest::Client` is built per process (`LazyLock<Result<Client, String>>`
shared by every request), so reqwest's connection pool, TLS session cache and
resolver stay warm; the module used to build a client per request.

Parsing is hand-rolled (title, meta description / `og:description`, favicon
`<link rel=…icon>`), because `regex` is only a dev-dependency and an HTML
parser would be a poor trade for three fields. `parse_html` is pure and
unit-tested.

## Frontend

- `frontend/src/lib/stores/shortcuts.ts` — the per-account store
  (`hv-shortcuts:<username>` in localStorage, namespaced like `monitorStore`).
  `MAX_SHORTCUTS` caps the set at 12; `add()` deduplicates by URL.
- `frontend/src/lib/components/desktop/DesktopShortcuts.svelte` — the desktop
  column of tiles. Each tile opens the URL in a new tab
  (`noopener,noreferrer`); the remove button is visible on hover **and**
  `focus-within`, with an `aria-label`, so removal is not hover-only. With no
  shortcut pinned the column is not rendered at all.
- `frontend/src/lib/components/desktop/HvLaunchpad.svelte` — the **add**
  control. Adding a shortcut is a launcher action, not a desktop one: the add
  tile is the launcher grid's **last cell**, after every app tile, and it
  opens `ShortcutAddDialog.svelte`. It is withheld while a search query or a
  category chip narrows the grid (a filtered grid is an answer to "where is
  X?", not a menu) and once `MAX_SHORTCUTS` is reached.
- `frontend/src/lib/components/desktop/ShortcutAddDialog.svelte` — the URL
  dialog, mounted by `Desktop.svelte` (`theme/Modal`, so focus trap + focus
  restore come from the design system).

## Invariants

- The metadata fetch must never bypass `is_public_ip` — a shortcut is a
  convenience, an SSRF is a compromise. Adding a redirect-following mode
  without re-validating each hop reopens the hole; so does removing
  `PublicOnlyResolver` from the client (the pre-check alone is the rebinding
  window).
- The 512 KB cap is a property of the **reader** (`read_capped_stream`), not of
  a slice taken after the body landed. Reverting to `Response::bytes()` makes
  "512 KB" a documentation lie again.
- The 10 s deadline covers the chain, not each hop: five hops with a per-hop
  budget is a one-minute request.
- The add control belongs to the launcher, not to the desktop: the desktop
  renders the pinned set and nothing else (no empty column, no "add" tile).
- The store is per-account: two accounts on one browser never share shortcuts.
- A fetched title is rendered as text, never as markup (E2E 3.2 asserts a
  `<img onerror>` title stays literal text).

## Tests

19 unit (SSRF address matrix incl. IPv4-mapped, `parse_url` normalisation, the
followed-status set, relative / protocol-relative / cross-host hops, a
three-hop chain, per-hop scheme refusal, the cap message, `hop_destination`,
the parser, and the four streaming-reader cases — **including "the second
chunk is never polled once the cap is reached"**), 4 integration (auth, scheme,
private/loopback literals, and every private literal form a hop could target),
10 E2E `test(` (the two new ones mock the endpoint: a refused redirect chain
and a timeout each keep the dialog open with the server's sentence and the
typed URL, and the submit button comes back enabled). No test performs a real
outbound request — a loopback test server would be refused by the guard under
test, so the chain and the stream are driven as pure functions instead.

## Related

- [Desktop](./desktop.md), [Widgets](./widgets.md), [API Contract](../API.md)

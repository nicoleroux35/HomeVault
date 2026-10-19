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
3. **No automatic redirects** — `reqwest`'s redirect policy is `none`. A
   single redirect is followed manually and the target re-validated against
   rule 2, so a public URL cannot 302 the server into a private one.
4. **Bounded work** — 10 s timeout, 512 KB response cap.

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
  without re-validating each hop reopens the hole.
- The add control belongs to the launcher, not to the desktop: the desktop
  renders the pinned set and nothing else (no empty column, no "add" tile).
- The store is per-account: two accounts on one browser never share shortcuts.
- A fetched title is rendered as text, never as markup (E2E 3.2 asserts a
  `<img onerror>` title stays literal text).

## Related

- [Desktop](./desktop.md), [Widgets](./widgets.md), [API Contract](../API.md)

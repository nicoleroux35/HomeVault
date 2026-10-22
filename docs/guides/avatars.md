# Avatars

**Purpose:** Per-user avatar images generated offline (DiceBear-style crates),
deterministic from a seed. No external network calls.

## Backend

- `GET /avatars/preview` (`handle_avatar_preview`) — **public by design**: it
  runs before any session exists (the setup wizard's live preview), renders an
  SVG from a client-supplied seed and touches no user data. It is the only
  unauthenticated avatar route; do not widen that to the per-user routes.
- `POST /auth/me/avatar` (`handle_update_my_avatar`) — auth; persists a new seed
  for the current user and returns the regenerated SVG.
- `GET /users/{username}/avatar.svg` (`handle_user_avatar`) — auth; serves a
  user's persistent avatar SVG from `users.avatar_seed`.

## Frontend

- `frontend/src/lib/services/api.ts`: `userAvatarUrl(username, size)` (builds the
  `/users/{username}/avatar.svg?size=` URL), `previewAvatarSvg(seed)` (fetches a
  `data:` URI), `updateMyAvatar(seed)`.
- Used in `Users.svelte`, `Account.svelte`, topbar user menu, group member lists.

## Invariants

- Avatars are generated server-side from the stored seed. A persisted seed is
  served with `Cache-Control: public, max-age=300, immutable`; `?fresh=true`
  (and a user with **no** stored seed, which renders a random avatar) answers
  `no-store`. The account page regenerates the seed through
  `POST /auth/me/avatar`, so the SVG changes only when the stored seed changes.
- The endpoint is offline (bundled crates), so no outbound requests.
- The old `render_user_avatar` / `missing_avatar_response` helpers had no callers
  and were removed (2026-10-22) — the seed-storage + `GET /users/{username}/
  avatar.svg` path is the whole feature; there is no separate upload path.

## Related

- [Users](./users.md), [My Account](./account.md), [Authentication](./auth.md)

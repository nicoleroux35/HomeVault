# Avatars

**Purpose:** Per-user avatar images generated offline (DiceBear-style crates),
deterministic from a seed. No external network calls.

## Backend

- `GET /avatars/preview` (`handle_avatar_preview`) — public; returns raw SVG for a
  given `seed` (used for live preview before saving).
- `POST /auth/me/avatar` (`handle_update_my_avatar`) — auth; persists a new seed
  for the current user and regenerates the cached avatar.
- `GET /users/{username}/avatar.svg` (`handle_user_avatar`) — auth; serves a
  user's persistent avatar SVG.

## Frontend

- `frontend/src/lib/services/api.ts`: `userAvatarUrl(username, size)` (builds the
  `/users/{username}/avatar.svg?size=` URL), `previewAvatarSvg(seed)` (fetches a
  `data:` URI), `updateMyAvatar(seed)`.
- Used in `Users.svelte`, `Account.svelte`, topbar user menu, group member lists.

## Invariants

- Avatars are generated server-side and cached; each seed change replaces the
  cached SVG. The endpoint is offline (bundled crates), so no outbound requests.

## Related

- [Users](./users.md), [My Account](./account.md), [Authentication](./auth.md)

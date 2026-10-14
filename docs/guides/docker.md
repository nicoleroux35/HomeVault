# Docker

**Purpose:** Manage containers, images, networks, and Docker Compose projects via
the local Docker daemon. Most mutations require admin. **Editable since
2026-09-12** — `CLAUDE.md`'s frozen-area registry is gone, so `Docker.svelte`
(and `src/docker.rs`, `src/compose.rs`, `src/app_catalog/*`, `src/app_proxy/*`,
`catalog/`) is open to edits; the invariants below still bind. `Docker.svelte`
is a two-pane split workspace since 2026-09-03.

## Backend

Handlers in `src/handlers/docker.rs` and `src/handlers/compose.rs`. Docker
availability checked by `handle_docker_available`.

Almost the entire surface is `admin`-gated, including plain reads — Docker
exposes real host details (images, container config, logs), so it isn't
merely session-gated the way most other "read" endpoints in this app are.
There are exactly two exceptions: `handle_docker_available` (`GET
/docker/available`, gates whether the app is even shown to a logged-in user)
and `handle_catalog_list` (`GET /docker/catalog`) use `require_auth`;
everything else below is `admin`.

| Group | Endpoints | Auth |
|-------|-----------|------|
| Info/availability | `GET /docker/info` | admin |
| | `GET /docker/available` | **auth** |
| Containers | `GET /docker/containers`, `/containers/running`; `PUT /containers/{name}/start\|stop\|restart`; `GET /containers/{name}/logs`; `POST /docker/containers`; `DELETE /docker/containers/{name}` | admin |
| Images | `GET /docker/images`; `POST /docker/images/pull`; `DELETE /docker/images/{image_id}` | admin |
| Hub | `GET /docker/hub/search` | admin |
| Networks | `GET /docker/networks`; `POST/DELETE /docker/networks`; `GET/DELETE /docker/networks/{name}`; `POST /docker/networks/prune\|connect\|disconnect` | admin |
| Compose | `GET/POST /docker/compose/projects`; `GET/PUT/DELETE /docker/compose/projects/{name}`; `GET /{name}/config`; `POST /{name}/start\|stop\|restart`; `GET /{name}/services`; `GET /docker/compose/available` | admin |
| App catalog | `GET /docker/catalog`; `GET /docker/catalog/{id}/icon\|gallery/{n}` | **auth** |
| | `POST /docker/catalog/{id}/install\|update\|proxy`; `DELETE /docker/catalog/{id}`; `GET /docker/catalog/updates` | admin |
| Community stores | `GET/POST/DELETE /docker/stores`; `POST /docker/stores/{id}/sync` | admin |
| Port check | `GET /docker/port-check` | admin |

`GET /docker/compose/projects/{name}/config` returns the raw
`docker-compose.yml` text as `{ "config": "<yaml>" }`. It backs the
containers tab's **Edit** button: a container that carries a
`com.docker.compose.project` label gets an Edit action that fetches this,
shows the file in a modal editor, and writes it back with the existing
`PUT /docker/compose/projects/{name}` — optionally followed by
`POST /{name}/restart` ("Save & redeploy"). If the label points at a project
HomeVault does not manage (dir absent under `compose_dir()`), the read fails
cleanly and the modal explains it can't be edited there.

Every Compose handler that takes a project `name` path segment (`get`,
`update`, `delete`, `start`, `stop`, `restart`, `services`, `config`, plus
`create`) runs it through `ComposeManager::validate_name` (alphanumeric + `_-.`,
must start alphanumeric) before joining it onto a real filesystem path — this
closes a real path-traversal bug (a `..%2F..%2Fetc`-style name could escape
`get_compose_dir()`, and `delete_project`'s `remove_dir_all` made this a
traversal-reachable recursive delete) found in a 2026-07-16 audit. The
one-click catalog handlers were never vulnerable to this — they only ever
pass the matched, catalog-defined `app.id`, never the raw request `id`.

### App catalog (manifest-driven — 2026-09-03 overhaul)

`src/docker_catalog.rs::CATALOG` was **deleted**. Apps are now *packages*: a
`homevault-app.toml` manifest + a sibling `docker-compose.yml` template
(`{{DATA_DIR}}`/`{{PORT}}`/`{{APP_ID}}`), from the embedded `catalog/` dir
(15 built-in packages) **plus community stores** — git repos an admin adds by
URL (`GET/POST/DELETE /api/docker/stores`, cloned into `apps_dir()/stores/<id>/`).
Logic lives in `src/app_catalog/{mod,manifest,store,updates}.rs`.

- **Namespaced ids on the wire**: `<store>:<app>` (`encodeURIComponent` in URLs).
  The built-in store id is `homevault` and its apps keep **bare** install keys
  (compose project + data dir); other stores prefix `<store>-<app>`. Never use
  the namespaced id as the install key.
- Manifests carry `description` + `description_fr/_de/_es` (replaces the old
  `catalog_desc_*` i18n keys).
- Install writes a `.install.toml` marker (store id, app id, package version,
  host port, `proxy_port`, `proxy_auth_required`) next to the app data; uninstall
  keeps `apps_dir()` data so a reinstall doesn't lose it.

### App catalog phase B — dependencies, settings, compatibility (2026-10-13)

Catalog phase-B additions on the same package shape (no new manifest file):

- **Dependencies with alternatives**: `dependencies = ["postgres"]` is
  satisfied by an installed app whose bare id matches **or** whose
  `implements = ["postgres"]` declares it. `providers_of()` lists every
  provider (canonical first); `resolve_install_plan()` maps each dependency
  to its installed satisfiers and its selected provider (explicit
  `alternatives[dep]`, else same-store-first). `GET
  /docker/catalog/{id}/install-review` (session) returns the plan plus the
  declared environment and the compatibility verdict, so the install dialog
  offers choices *before* the pull; `POST
  /docker/catalog/{id}/install` accepts `{port?, alternatives?, environment?}`
  and hard-gates missing providers (400 naming what to install first —
  dependency installs stay explicit, never implicit pulls).
- **Advanced environment settings**: `[[environment]]` declarations (`name`
  in `UPPER_SNAKE_CASE`, `services`, `default`, `options`, `required`,
  `note`) validated at parse (bad names/duplicates/default-outside-options
  rejected) and at install (`validate_env_values`); values render into the
  compose template as `{{NAME}}` placeholders (`render_with_env`).
- **Compatibility**: `arch` (existing) plus `min_homevault_version`
  (`major.minor.patch`, checked against `CARGO_PKG_VERSION`) surface as
  `incompatibility_reason` in the list view and are enforced by install.
- **Per-app logs**: `GET /docker/catalog/{id}/logs?tail=N` (session, 1–2000,
  default 200) tails `docker compose logs` for the install key.
- **Update queue**: `POST /docker/catalog/update-all` (admin) runs the same
  `apply_update` path sequentially over every pending update and reports
  `{updated[], failed[]}` — one failure never aborts the queue. Surfaced as
  a `docker_update_all` progress job.
- Install/uninstall share their core with the MCP `apps.install/uninstall`
  tools (`install_catalog_app` / `uninstall_catalog_app`, actor-labelled
  event log) — fix one path and both move.

### Per-app reverse proxy + app-scoped auth (`src/app_proxy/*`)

Each installed app gets its own hyper HTTP/1 listener on a dedicated port
(47000–47999, `AppProxyManager` / `APP_PROXY` global) forwarding to the
container's published port on `127.0.0.1`, WebSocket `Upgrade` passed through.

**Hardened 2026-10-17 — the proxy no longer trusts the API session.**
Previously the gate read the full API session cookie (`hv_session`) directly,
which meant a proxied app could be handed a credential valid against the whole
dashboard. Now:

- The gate accepts only an **app-proxy-scoped** session (`hv_app_session`,
  `src/app_proxy/app_auth.rs`), minted by `GET /app-auth?app=<ns_id>` on the
  **main origin** — the server-chosen, restricted entry. `/app-auth` validates
  the real `hv_session` (browser navigation), confirms the app has a running
  proxy, sets `hv_app_session` and redirects to the app origin. The token is
  meaningless against the API and is revoked when the proxy stops or the user
  logs out.
- A request with no valid app session gets a 302 to `/app-auth?app=<ns_id>`,
  never a bare `/login`.
- **Both** HomeVault cookies (`hv_session`, `hv_app_session`) are stripped from
  the request before it is forwarded upstream, so an installed app never sees
  either credential while its own cookies survive
  (`cookie::strip_homevault_cookies`).
- `POST /docker/catalog/{id}/proxy` toggles the gate (on by default).

**Deliberate:** the app's own published port stays bound to `0.0.0.0` — the
proxy is an extra consistent entry point, not a lockdown; do not "harden" by
binding containers to `127.0.0.1` without an explicit request.

### Update detection (`src/app_catalog/updates.rs`)

Compares each installed app's running image digest (`docker inspect`
`RepoDigests`) to the digest the registry serves for that tag (anonymous v2
bearer flow — Docker Hub + GHCR + generic; **read-only, no pull**), plus a
package-version check. `GET /docker/catalog/updates` + `POST
/docker/catalog/{id}/update` (`compose pull` + re-render + `up -d`). The
notification engine runs `check_app_updates` every 6h. "Unknown" digest ⇒ "no
update", never an error.

### Other Docker fixes

- `DockerService::parse_ports` (`src/docker.rs`): Docker's `{{.Ports}}` is
  `IP:HOSTPORT->CONTAINERPORT/proto` — **the colon is LEFT of `->`**.
- **Host-port conflict check**: `GET /api/docker/port-check` (admin) +
  re-checked server-side in `handle_docker_container_create` and
  `handle_catalog_install` — must stay server-side, not UI-only.
- Per-container I/O: `get_container_stats` also parses `{{.NetIO}}`/`{{.BlockIO}}`
  (`net_rx_bytes`/`net_tx_bytes`/`blk_read_bytes`/`blk_write_bytes`).

## Frontend

- `frontend/src/routes/Docker.svelte` — has its own local `authedFetch` helper
  (reads `session-token`/`csrf-token` from `localStorage`) rather than going
  through `frontend/src/lib/services/api.ts` for Docker/Compose/catalog calls;
  the new "App Catalog" tab (`tab === 'catalog'`) follows the same pattern.
  **Verified on 2026-10-12: the two paths are behaviourally identical** —
  `authedFetch` sends the same `X-Session-Token` + `X-CSRF-Token` pair as
  `api.ts` (which is what `src/middleware/csrf.rs` requires on every mutating
  method), so this is a maintenance duplication, not a live divergence bug.
  **Consolidating `Docker.svelte` onto `api.ts` is deliberately not done:**
  `api.ts` carries only 6 Docker methods (`getDockerContainers`,
  `getDockerCatalog`, `isDockerAvailable`, `startContainer`,
  `stopContainer`, `restartContainer` — all consumed by the desktop Docker
  monitor), while `Docker.svelte` drives containers, images, networks, volumes,
  Compose files, the catalog and third-party stores. Routing it through
  `api.ts` means adding roughly 50 wrapper methods and rewriting ~100 call
  sites, with the 51-test `docker.spec.ts` as the only net — a large,
  regression-prone change for no user-visible gain while both paths work. If it
  is ever done, do it one tab at a time with the E2E suite green in between,
  and keep the `authedFetch` error shape (`{success:false,error}`) until the
  last call site is migrated.
- `frontend/src/lib/services/api.ts` has a small, separate set of
  Docker methods (`getDockerContainers`, `startContainer`,
  `isDockerAvailable`, …) — these are not wired into
  `Docker.svelte` itself (which uses its own `authedFetch`), but
  `isDockerAvailable`/`getDockerContainers`/`startContainer`/`stopContainer`/
  `restartContainer` **are** actively used by the desktop's Docker monitor
  widget (`Desktop.svelte`, see [Widgets](./widgets.md)).
- The hand-rolled modal components
  (`frontend/src/lib/components/docker/{ImagePull,ContainerCreate,Logs,DeleteConfirm,Compose}Modal.svelte`)
  were deleted on 2026-09-23 (UI audit C2): they were orphaned — `Docker.svelte`
  renders every modal on the shared `theme/Modal` (bits-ui Dialog: focus trap,
  Escape, focus restore), never `role="document"`. The `api.ts` methods only
  they called (`getContainerLogs`, `createContainer`, `pullImage`,
  `removeContainer`, `removeImage`, `createComposeProject`) went with them.
  Don't reintroduce a per-app modal skin.

## Invariants

- `GET`/info endpoints are `auth`; create/start/stop/remove/pull/prune/connect/
  catalog-install/catalog-uninstall are `admin`.
- `handle_catalog_install` and `handle_catalog_update` register an
  **indeterminate** `PROGRESS_MANAGER` job (`docker_install` / `docker_update`)
  around the `docker compose` work so the desktop activity taskbar shows the
  operation; the job is completed/failed on every existing return branch and
  the success/error response bodies are unchanged. See
  [Activity taskbar](./activity.md).
- If Docker is not installed, `handle_docker_available` returns
  `{ available: false }` and the UI shows "Docker is not available". If the
  `docker compose` plugin specifically isn't installed,
  `handle_compose_available`/catalog install return `{ available: false }` /
  a clear error rather than hanging or crashing.
- Verified live via `curl` with a real session: `GET /docker/catalog` returns
  all 15 apps with correct image/port/`installed_port` data, and the
  auth split is enforced. A live install (`docker compose up`) was **not**
  exercised in this pass — the verification environment has no outbound
  network access to pull images — so treat the compose templates themselves
  (image names, required env vars, volume paths) as reviewed-but-not-executed
  until the first real install on a networked host.

## Related

- [Cloud Sync](./cloud-sync.md), [API Contract](../API.md)

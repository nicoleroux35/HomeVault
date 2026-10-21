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
| Containers | `GET /docker/containers`, `/containers/running`; `PUT /containers/{name}/start\|stop\|restart\|pause\|unpause\|rename`; `POST /containers/{name}/kill\|update\|recreate\|exec`; `GET /containers/{name}/inspect`; `GET /containers/{name}/logs`; `POST /containers/{name}/logs/ticket`; `GET /containers/{name}/logs/stream`; `POST /docker/containers`; `DELETE /docker/containers/{name}` | admin |
| Images | `GET /docker/images`; `POST /docker/images/pull`; `POST /docker/images/build`; `DELETE /docker/images/{image_id}` | admin |
| Hub | `GET /docker/hub/search` | admin |
| Volumes | `GET/POST /docker/volumes`; `DELETE /docker/volumes/{name}` | admin |
| Disk usage | `GET /docker/system/df`; `POST /docker/prune` | admin |
| Registries | `GET /docker/registries`; `POST /docker/registries/login\|logout` | admin |
| Daemon | `GET/PUT /docker/daemon/config`; `POST /docker/daemon/start`; `GET /docker/daemon/logs` | admin |
| Networks | `GET /docker/networks`; `POST/DELETE /docker/networks`; `GET/DELETE /docker/networks/{name}`; `POST /docker/networks/prune\|connect\|disconnect` | admin |
| Compose | `GET/POST /docker/compose/projects`; `GET/PUT/DELETE /docker/compose/projects/{name}`; `GET /{name}/config`; `POST /{name}/start\|stop\|restart\|update`; `GET /{name}/services`; `GET /docker/compose/available` | admin |
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
  never a bare `/login`. **TLS-aware (2026-10-21):** under an active HTTPS
  listener the redirect targets `https://{host}:{https_port}` — `hv_session`
  is `Secure` there and would never be re-sent over plain HTTP, which would
  loop the round-trip forever. The `hv_app_session` cookie itself stays
  *without* `Secure` on purpose: the proxy listener is plain HTTP even under
  an HTTPS dashboard, and a `Secure` cookie would never travel to an
  `http://` origin. Both redirects mint auth cookies, so they carry
  `Cache-Control: no-store`.
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

### Production hardening (2026-10-20)

The module went from "manage containers" to a complete engine console. The
rules below are invariants, not implementation notes.

**Every CLI call is time-bounded.** No `Command::output()` on the bare
`docker` binary anywhere in `src/docker.rs` / `src/compose.rs`. Everything
goes through `run_docker(args, timeout)`, `run_command(cmd, timeout)` or
`run_command_with_stdin(cmd, payload, timeout)`, all of which funnel into
`wait_with_timeout` and kill the child on expiry, returning
`DockerError::Timeout(secs)`. `SHORT_TIMEOUT` = 30 s, `LONG_TIMEOUT` = 900 s
(builds, pulls, recreates). A wedged daemon now produces an error instead of
parking a Tokio worker forever.

**Every mutation runs on the blocking pool.** `src/handlers/docker.rs` has a
local `blocking()` helper wrapping `tokio::task::spawn_blocking`; every
handler that shells out uses it. A synchronous `docker` call on the async
runtime stalls *all* concurrent requests, not just its own.

**Live logs use a single-use ticket.** `EventSource` cannot send the
`X-Session-Token` header, so `POST /containers/{name}/logs/ticket` mints a
30 s, single-use ticket (`issue_log_ticket` / `consume_log_ticket`,
`LOG_TICKETS` static) that `GET /containers/{name}/logs/stream?ticket=`
redeems. The ticket stores container + tail + follow + timestamps, so the
query string carries no authority, and a ticket minted for `alpha` is
rejected on `beta`'s stream (401). The stream merges **stdout and stderr**
(previously stderr was dropped), honours `?tail=` (clamped 1–5000) and
`?timestamps=`, and tears the reader threads down through a `KillOnDrop`
guard when the client disconnects.

**Destructive actions validate before acting.** `PruneTarget::parse` accepts
only `images|volumes|containers|networks|builders|system`; an unknown target
is rejected *before* the daemon is contacted — never defaulted to a
destructive action. `ALLOWED_SIGNALS` whitelists the kill endpoint
(SIGKILL, SIGTERM, SIGINT, SIGHUP, SIGQUIT, SIGUSR1, SIGUSR2, SIGSTOP);
anything else falls back to SIGKILL rather than being passed through.

**Container updates are reversible.** `recreate_container` pulls first, then
renames the live container to `<name>-hv-old-<epoch>`, runs the rebuilt
`docker run` argument list, and **rolls the rename back** if the new
container fails to start. The parked container is removed only on success, so
a bad image never leaves the user with nothing. `run_args_from_inspect`
reconstructs the arguments from `docker inspect`: it drops
`com.docker.compose.*` labels (the recreated container is no longer compose's
— otherwise the Compose tab would offer to edit a project that no longer owns
it), skips the default `bridge` network, and replays `--entrypoint` only when
it is a single argument (Docker's flag takes one string).

**Build context is path-guarded.** `POST /docker/images/build` validates the
context with `state.path_guard.validate()` — the same guard as the file
explorer — so the builder is not a way to read the whole host.

**Secrets never hit a command line or a log.** Registry login uses
`--password-stdin` (the password is never visible in `/proc/<pid>/cmdline`),
and `GET /docker/registries` reads only the `auths` **keys** of
`~/.docker/config.json` — never a credential. `daemon.json` is validated as a
JSON object *before* it is written, then written via `sudo tee` with the
payload on stdin (no `echo` of the config into a shell), then
`sudo systemctl restart docker`.

**Compose "update" re-pulls.** `ComposeManager::pull_project` runs
`compose pull` then `up -d`; `up -d` alone never re-pulls, so the button
would have been a no-op.

### Other Docker fixes

- `DockerService::parse_ports` (`src/docker.rs`): Docker's `{{.Ports}}` is
  `IP:HOSTPORT->CONTAINERPORT/proto` — **the colon is LEFT of `->`**.
- **Host-port conflict check**: `GET /api/docker/port-check` (admin) +
  re-checked server-side in `handle_docker_container_create` and
  `handle_catalog_install` — must stay server-side, not UI-only.
- Per-container I/O: `get_container_stats` also parses `{{.NetIO}}`/`{{.BlockIO}}`
  (`net_rx_bytes`/`net_tx_bytes`/`blk_read_bytes`/`blk_write_bytes`).

### Prerequisites — Docker engine and Compose (installer, 2026-10-19)

`installer/install.sh` provisions the engine and the Compose v2 plugin, and
Step 4 verifies all of it:

| Piece | Package | Why it matters |
|---|---|---|
| Engine | `docker.io` (Debian/Ubuntu) | `dockerd` + CLI; the unit is `enable --now`'d, and Step 4 checks `docker info` — a CLI with no daemon would look installed and fail every page |
| Compose v2 | `docker-compose-v2` (Ubuntu 24.04+/Debian 13+), else `docker-compose-plugin` (Docker's repo) | `docker compose up -d` backs the Compose tab **and every catalog app** |
| Buildx | `docker-buildx-plugin` / `docker-buildx` | `docker compose build` delegates to it; the bundled catalog pulls prebuilt images, a community store may not |

The Compose plugin installs `/usr/libexec/docker/cli-plugins/docker-compose`,
which the `docker.io` CLI (>= 20.10) finds on its own — Docker's own engine
packages are **not** required, and `docker-compose-plugin` has no `Depends:`
that would displace `docker.io`.

Docker's apt repository is only added when no distribution package provides
the plugin, and the whole step is best-effort: a failed key fetch or
`apt-get update` warns, removes the list file again (a broken `docker.list`
would break every later `apt-get update` on the host) and lets the install
continue — Docker itself keeps working.

## Frontend

Seven tabs since 2026-10-20: **Containers, Images, Volumes, Networks,
Discover, App Catalog, Compose**. The Volumes tab also carries the
`docker system df` table and the four prune buttons, because the number and
the fix belong in the same place. Container secondary actions (details,
pause/resume, kill, rename, limits, exec, update image) live behind one
"more actions" menu so the card keeps start/stop/restart/logs as its visible
path. The logs modal is a live SSE tail with tail-size, timestamps and follow
controls, a status pill (live / connecting / ended / disconnected), download
and restart-stream. Everything still renders on `theme/Modal` (bits-ui
Dialog: focus trap, Escape, focus restore).

Two popover menus live in the container card ("Open app" and "More
actions"). They are `position: fixed` with a viewport-anchored `top`/`left`
computed from the trigger's `getBoundingClientRect()` — **not** `absolute`.
The card is `overflow-hidden` (it rounds its own corners), so an absolutely
positioned menu is clipped away and becomes invisible and unclickable as soon
as the menu is taller than the remaining card height. Escape closes them
through a `window` keydown listener, because focus is still on the trigger
button when the menu opens.

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
  `api.ts` means adding roughly 60 wrapper methods and rewriting ~120 call
  sites, with the 56-test `docker.spec.ts` as the only net — a large,
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

- Reads are `admin` too (see above), with exactly two `auth` exceptions:
  `GET /docker/available` and `GET /docker/catalog`. Every mutation —
  create/start/stop/restart/pause/kill/rename/exec/recreate/update,
  remove/pull/build/prune, volume and network changes, registry login,
  `daemon.json`, catalog install/uninstall — is `admin`.
- Every Docker CLI call is time-bounded and every mutation runs on the
  blocking pool (see "Production hardening"): never reintroduce a bare
  `Command::output()` on `docker`, and never call a `DockerService` method
  directly from an `async fn` handler.
- Live logs go through the ticket, never a session token in a URL.
- Destructive actions validate their input **before** touching the daemon
  (`PruneTarget`, signals, names, build context, `daemon.json`).
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

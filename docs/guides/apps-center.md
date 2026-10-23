# Apps Center (Launcher)

**Purpose:** Browse and launch installed applications from a searchable grid
opened from the desktop topbar.

## Backend

The launcher itself has no dedicated backend: it reads the same app registry
the desktop uses and opens native apps as desktop windows. Installed Docker
catalog apps come from `GET /api/docker/catalog` (which also reports each
app's per-app reverse-proxy port — see
[Docker's app catalog](./docker.md#app-catalog-manifest-driven--2026-09-03-overhaul)
and the proxy section below).

## Frontend

- Real implementation: `frontend/src/lib/components/desktop/HvLaunchpad.svelte`,
  opened from `Desktop.svelte` (`showAppsCenter`) and rendered with
  `apps={allApps}` + `onAppClick={(appId) => openApp(allApps.find(a => a.id === appId)!)}`.
- Registry source of truth: the static `apps` array in
  `frontend/src/routes/Desktop.svelte` (id, name, icon, category, component,
  desc), merged with the dynamic `installedDockerApps` list (below) into
  `allApps`. `HvLaunchpad` has **no hardcoded app list of its own** — it
  derives its search/category grouping directly from whatever array it's
  given, so it cannot show an app that doesn't really exist.
- Opening a native app routes through `Desktop.svelte::openApp`, which calls
  `windowManager.openWindow(app.id, app.name, app.icon, app.component)`.

### Dynamic Docker apps

Installed Docker catalog apps (see
[Docker's app catalog](./docker.md#app-catalog-manifest-driven--2026-09-03-overhaul))
show up as real Launchpad tiles too, not just inside the Docker app's own tab:

- `Desktop.svelte` fetches `GET /api/docker/catalog` on mount
  (`loadDockerCatalog`) into `dockerApps`, and a derived `installedDockerApps`
  maps every `installed` entry to an app-shaped object
  (`id: 'docker-<catalogId>'`, `isDockerApp: true`). The launch port prefers
  the per-app reverse proxy (`proxy_port`) and falls back to the container's
  own published port; Home Assistant runs with `network_mode: host` and has no
  compose-published port, so its UI is always on the fixed `8123`.
- **Opening behaviour**: Docker apps never open as an in-app window or
  iframe. Their tile routes the click to `openExternalUrl` — the app's own
  web UI at `http://<hostname>:<port>` opens in a **new browser tab** (or a
  popup window when the user picks it), same as the Docker page's container
  "Open" menu. This is a deliberate design (2026-10-18): a top-level
  navigation from an HTTPS dashboard to a plain-HTTP app origin is *not*
  mixed content — browsers block only embedded subresources (iframes,
  `fetch`) — so no per-app TLS is needed for apps to be reachable under an
  HTTPS admin entry. The scheme stays `http://` because the per-app proxy
  listener is plain HTTP even when the dashboard runs HTTPS.
- The catalog is refetched on a 30-second interval, when the Launchpad opens,
  when the Docker app's own window closes, and on desktop switches, so
  installing/uninstalling an app updates the Launchpad without a dedicated
  polling endpoint. A stopped app renders its tile **greyed out** (not
  clickable); an installed app that publishes no port renders disabled with a
  "no web UI" label.

### The per-app reverse proxy and the auth round-trip

Each installed (and running) catalog app gets its own reverse-proxy listener
on a dedicated port (47000–47999), forwarding to the container's published
port, with an optional auth gate (on by default, toggleable per app via
`POST /api/docker/catalog/{id}/proxy`). The gate's auth model
(`src/app_proxy/`, contract of 2026-10-17):

1. A request without a valid `hv_app_session` is issued a `302` to the
   **server-chosen** entry on the main origin,
   `GET /app-auth?app=<store>:<app>`. The app proxy **never** accepts
   `hv_session`; both HomeVault cookies are stripped from the request before
   it is forwarded upstream, so the container never sees either credential.
2. `/app-auth` validates the real API session cookie (`hv_session`), mints an
   app-proxy-scoped session (`hv_app_session`) and redirects back to the
   app's proxy origin. That cookie is *deliberately never `Secure`*: the
   proxy listener is plain HTTP even under an HTTPS dashboard, and a `Secure`
   cookie would never be sent to an `http://` origin, breaking every gated
   app. The token is proxy-scoped only — it is meaningless against the API —
   **and bound to the app it was minted for**: it is refused on any other
   app's proxy, revoked on logout, and revoked when **that** app's proxy
   stops (every other app keeps its sessions — stopping app A no longer signs
   the household out of apps B..Z).
3. The login screen honours the `next` parameter of the redirect
   (`/login?next=/app-auth?app=…`): after sign-in the browser continues the
   round-trip as a full navigation instead of dead-dropping on the desktop.
   `next` is restricted to same-origin paths — an absolute or scheme-relative
   value would be an open redirect.

**Redirect targets come from server state, not from `Host` (fixed
2026-10-23).** Both redirects in the round-trip are absolute URLs back to the
appliance, so their host is resolved by `src/app_proxy/origin.rs`:
the request's `Host` header is normalised (host name, IPv4 or bracketed IPv6 —
never a URL, userinfo, path or list) and honoured **only** when it names this
appliance — `HOMEVAULT_HOST` (defaults to the OS host name), `<host>.local`,
`localhost`, loopback, the primary LAN address, or an entry listed in
`HOMEVAULT_TRUSTED_HOSTS`. Anything else falls back to the canonical host, so
a forged header (or a reverse proxy in front that rewrites it) can no longer
turn an auth redirect that carries a freshly minted cookie into an open
redirect. A custom DNS name or a second NIC is declared with
`HOMEVAULT_TRUSTED_HOSTS=nas.example.org,192.168.1.11` (comma-separated).

**HTTPS deployments (fixed 2026-10-21).** The gate's redirect target is now
TLS-aware. Under TLS the `hv_session` cookie is `Secure`, so pointing an
unauthenticated browser at the plain-HTTP main port would never receive the
cookie back and the round-trip would loop forever: the redirect now targets
`https://{host}:{https_port}/app-auth` whenever the HTTPS listener is up
(otherwise the plain-HTTP main port, as before). Additionally:

- `/app-auth` responses and the gate challenge carry `Cache-Control: no-store`
  (they mint auth cookies; no intermediary may cache them).
- The app-session store (`APP_AUTH`) is capped at 512 live sessions — the
  soonest-expiring entry is evicted at the cap — so repeated `/app-auth` hits
  cannot grow the in-memory store without limit between the 12-hour expiries.

### Install and uninstall are honest and off the runtime (fixed 2026-10-23)

- `docker compose up -d` / `stop` / the dependency gate's `get_project` calls
  run on the **blocking pool**. A first install pulls images for minutes; on a
  runtime worker it pinned that worker and concurrent installs starved.
- The `.install.toml` marker is written **before** anything runs and its write
  failure aborts the install. It used to be discarded, so a read-only or full
  apps directory produced an install that looked successful, vanished from the
  list after a restart and lost its reverse proxy (nothing to restore from).
  The same rule applies to app updates and to
  `POST /api/docker/catalog/{id}/proxy`, whose response now carries
  `applied: false` when the app's proxy is not running (the value is saved and
  takes effect when it next starts) instead of a bare success.

### Known tradeoffs

- The app's own published port stays open on the LAN; the proxy is an
  additional consistent entry (single host name, uniform auth), not a
  firewall. Documented by design — do not silently change this.
- Proxy listeners are plain HTTP even under an HTTPS dashboard (per-app TLS
  termination is not implemented; the top-level navigation design above keeps
  apps reachable either way).

> **History:** `frontend/src/routes/AppsCenter.svelte`,
> `frontend/src/routes/Apps.svelte`, and
> `frontend/src/lib/components/theme/AppsCenter.svelte` were three separate,
> **dead** launcher implementations — none was imported by anything (`grep`
> confirmed zero references), and each hardcoded its own stale app list
> (several entries pointing at apps that were never implemented: Mail Server,
> Notes, Calendar, Contacts, Surveillance, Video/Photo/Audio Station; plus a
> `window.location.href = /${id}` navigation that the SPA router doesn't
> support and so never worked). They were removed rather than fixed, since
> `HvLaunchpad` was already the correct, currently-used implementation.
> Removing them also surfaced that `VPN.svelte` — a fully-built page calling
> real, already-routed backend endpoints — had never been added to the `apps`
> array and so was unreachable from any UI; it's now a normal entry like every
> other app (see [VPN](./vpn.md)). The earlier design that rendered proxied
> apps inside a sandboxed `windowManager` iframe (with a mixed-content toast
> pointing at the direct URL) was never shipped on this path: Docker apps
> open in a new tab instead.

## Invariants

- Adding an app to the desktop grid **and** the launcher is a single change:
  add one entry to the `apps` array in `Desktop.svelte`. There is no second
  place to keep in sync.
- Do not reintroduce a component-local hardcoded app list for the launcher —
  that's exactly the bug class the dead files above had.
- Do not add a `hv_session`-based shortcut to the proxy gate (see
  `CLAUDE.md` → "Client/server contract & app-proxy auth"): the gate accepts
  only `hv_app_session`, and both HomeVault cookies are stripped upstream.
- `hv_app_session` must stay without the `Secure` flag, and the proxy
  listeners must stay plain HTTP: make them consistent together or the gated
  apps become unreachable.
- Never build a redirect (or any absolute URL back to the appliance) from the
  request's `Host` header — resolve it through `app_proxy::origin`
  (`resolve_redirect_host` / `authority`), which only accepts the appliance's
  own names and otherwise falls back to the configured canonical host.
- App-session revocation is scoped: `revoke_app(ns_id)` on a proxy stop,
  `revoke_user` on logout. A `revoke_all`-style sweep belongs nowhere.

## Related

- [Desktop](./desktop.md), [Docker](./docker.md), [VPN](./vpn.md),
  [TLS](./tls.md)

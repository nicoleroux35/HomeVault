# Apps Center (Launcher)

**Purpose:** Browse and launch installed applications from a searchable grid
opened from the desktop topbar.

## Backend

No dedicated backend; the launcher reads the same app registry the desktop
uses and opens apps as desktop windows.

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

- `Desktop.svelte` fetches `GET /docker/catalog` on mount (`loadDockerCatalog`)
  into `dockerApps`, and a derived `installedDockerApps` maps every
  `installed` entry to an app-shaped object (`id: 'docker-<catalogId>'`,
  `isDockerApp: true`, `dockerPort: installed_port`).
- `openApp()` branches on `isDockerApp`: instead of opening a Svelte
  component, it calls the new `windowManager.openWindowUrl(id, name, icon,
  url)` with `url = http://<hostname>:<port>` (fixed `8123` for Home
  Assistant, which has no `installed_port`). The window renders an `<iframe
  sandbox="allow-scripts allow-forms allow-same-origin allow-popups
  allow-modals">` instead of a component — a genuinely new window-content
  type (`Window.contentType === 'url'`), added without touching any existing
  `component`/`componentLoader` window.
- **Mixed content**: if HomeVault itself is served over HTTPS and the catalog
  app is plain HTTP (every template here is, by default), the browser may
  block the iframe outright. `openApp()` shows a one-time toast pointing at
  the direct `http://host:port` URL in that case — documented as a v1
  limitation, not solved with per-app TLS termination.
- The catalog is refetched when the Docker app's own window closes (an
  event-driven hook on the existing close handler, not a new polling
  interval), so installing/uninstalling an app updates the Launchpad the next
  time it's opened.

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
> other app (see [VPN](./vpn.md)).

## Invariants

- Adding an app to the desktop grid **and** the launcher is a single change:
  add one entry to the `apps` array in `Desktop.svelte`. There is no second
  place to keep in sync.
- Do not reintroduce a component-local hardcoded app list for the launcher —
  that's exactly the bug class the dead files above had.

## Related

- [Desktop](./desktop.md), [VPN](./vpn.md)

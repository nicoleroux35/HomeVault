# HTTPS / TLS (built-in listener)

**Purpose:** let HomeVault terminate HTTPS itself, so a home admin gets an
encrypted web UI without standing up a reverse proxy. Opt-in and **additive** —
the plain-HTTP listener on `PORT` is unchanged. Modelled on the standard appliance pattern:
a self-signed certificate HomeVault generates and renews, or an admin-supplied
PEM pair, plus an HTTP→HTTPS redirect toggle. No ACME/Let's Encrypt yet (a
possible later phase). Lives in **Settings > Security > HTTPS/TLS**.

## Overview

When enabled, `web::run()` binds a second listener via `axum_server` on
`HTTPS_PORT` (default 8443) serving the *same* Axum router as the HTTP one. The
listener binds once at startup; changing settings reports `requires_restart:
true` and the card offers a restart button (mirrors OMV restarting nginx).

A missing or broken certificate is logged and the instance stays **HTTP-only** —
TLS trouble never stops the server.

## Configuration

Resolved by `src/tls.rs::load_settings()`: the persisted file
`{DATA_DIRECTORY}/data/tls.json` if the admin has saved one, otherwise
environment defaults.

| Field | Env | Default | Notes |
|---|---|---|---|
| `enabled` | `TLS_ENABLED` | `false` | `1`/`true`/`yes`/`on` |
| `https_port` | `HTTPS_PORT` | `8443` | must differ from `PORT` |
| `mode` | `TLS_MODE` | `self-signed` | `self-signed` \| `manual` |
| `cert_path` | `TLS_CERT_PATH` | — | manual mode; absolute, traversal-free, readable PEM |
| `key_path` | `TLS_KEY_PATH` | — | manual mode; absolute, traversal-free, readable PEM key |
| `redirect` | `TLS_REDIRECT` | `false` | 301 non-loopback HTTP → HTTPS |

`tls.json` stores **paths only** — never key material — so it needs no export
redaction. `settings_persistence::reset()` deletes it alongside `settings.json`.

## Self-signed certificate lifecycle

`src/tls.rs`:

- Files: `{DATA_DIRECTORY}/tls/self-signed.crt` (`0644`) +
  `self-signed.key` (`0600`), dir `0700`.
- Generated with `rcgen` (ECDSA P-256), validity ~397 days.
- SAN list (`self_signed_sans()`): `localhost`, `127.0.0.1`, `::1`, the OS
  hostname, `<hostname>.local`, `homevault.local`
  (`crate::mdns::MDNS_HOSTNAME`), and the LAN IPv4 from
  `crate::network::get_connection_summary()`.
- `ensure_self_signed()` regenerates when the cert is missing or has
  **< 30 days** left, so a running instance never serves an expired cert.
- `POST /api/settings/tls/regenerate` forces a fresh one immediately.

Browsers show a trust warning for the self-signed cert — expected, as with every
self-signed appliance. For a trusted cert, use **manual** mode with a cert from
your own CA, `certbot`, `mkcert`, or an existing reverse proxy.

## HTTP → HTTPS redirect (on by default, 2026-10-17)

`middleware::https_redirect` is layered onto the **HTTP** router only (never the
HTTPS one) when `redirect` is on. Loopback clients (`127.0.0.1`, `::1`) are
exempt so the installer health check (`curl http://127.0.0.1:PORT/...`) and the
internal app-proxy keep working over plain HTTP. Requests with no usable `Host`
header are served normally rather than redirected to a broken URL.

**`redirect` now defaults to `true`.** It used to default to `false`, which
meant enabling HTTPS produced a server that still served the login form — and
the admin password — in clear on `PORT`, with nothing telling the operator.
`TlsSettings::default()` sets it, and `#[serde(default = "redirect_default")]`
makes a `data/tls.json` written before the field existed deserialize to `true`
rather than silently to `false`. Opt out with `TLS_REDIRECT=0` (or the toggle in
Settings) when both listeners must stay usable.

## The session cookie follows the connection

`app_proxy::cookie::attach_session(resp, token, remember_me, over_tls)` adds
`Secure` **only when the request arrived over TLS**. It cannot be
unconditional: HomeVault is commonly served over plain HTTP on a LAN, and a
`Secure` cookie is dropped outright by the browser there — the app proxy would
break with no visible cause. `over_tls` comes from the `ArrivedOverTls`
extension the HTTPS listener inserts.

## Plain-HTTP warning in the UI (2026-10-17)

Because plain HTTP remains a supported deployment, the wizard and the login
screen warn when they are served over `http://` on a **non-loopback** host
(`data-testid="setup-insecure-banner"` / `login-insecure-banner`,
`role="status"`): the password is about to cross the network unencrypted, and
the browser gives no other hint. `localhost` / `127.0.0.1` / `::1` are exempt —
nothing leaves the machine. The copy points at Settings → Security, where the
TLS card turns HTTPS on in one toggle plus a restart.

## HSTS

`middleware/security_headers.rs` sends `Strict-Transport-Security:
max-age=31536000; includeSubDomains` **only** when `crate::tls::tls_active()` is
true (the HTTPS listener came up). `preload` is deliberately omitted — submitting
a domain to the browser preload list is the operator's decision. A browser
ignores HSTS received over plain HTTP anyway.

## Endpoints

All admin-only (`require_admin`). Paths shown without the `/api` prefix.

| Method | Path | Body / response |
|---|---|---|
| GET | `/settings/tls` | → `{ enabled, https_port, effective_https_port, http_port, mode, cert_path, key_path, redirect, active, cert_info }` |
| PUT | `/settings/tls` | `{ enabled, https_port?, mode, cert_path?, key_path?, redirect }` → `{ ok, requires_restart: true, settings }` — 400 on validation failure with a specific message |
| POST | `/settings/tls/regenerate` | → `{ ok, requires_restart, cert_info }` |

`cert_info` (`src/tls.rs::CertInfo`, via `x509-parser`): `subject`, `issuer`,
`sans[]`, `not_before`, `not_after` (RFC 3339), `days_remaining`, `self_signed`,
`error`. Never errors — problems land in `error`.

Handlers: `src/handlers/tls.rs`. Routes: `src/web/mod.rs`. Frontend:
`frontend/src/lib/components/settings/TlsCard.svelte` (in the `security`
category of `Settings.svelte`), `api.ts` `getTlsSettings` / `setTlsSettings` /
`regenerateTlsCert`. i18n: `src/i18n/*.toml` `[settings]` `tls_*`.

## Firewall

The installer's opt-in HTTPS prompt runs `ufw allow 8443/tcp`. Enabling HTTPS
later from the Settings card does **not** touch the firewall — add an allow rule for `HTTPS_PORT` from the Firewall card if `ufw`
is active.

## Invariants

- The HTTP listener on `PORT` is never removed or replaced — only optionally
  wrapped with the redirect middleware.
- TLS failure is logged, never fatal.
- `resolved_cert_key_paths()` for manual mode rejects relative paths, `.`/`..`
  components, control chars, and non-readable / non-file targets.
- `validate_settings()` rejects `https_port == 0` and `https_port == PORT`.
- `tls.json` never contains key material.

## Not covered / later

- ACME / Let's Encrypt (public trusted certs). DNS-01 avoids the open-port
  requirement; HTTP-01 / TLS-ALPN-01 need an open port 80/443, which most
  home NATs lack.
- Hot rebind without a restart.
- HTTP/2 is left in the ALPN set (`axum-server` default) — WebSockets
  (Terminal, app-proxy) still negotiate HTTP/1.1; verified to work.

## Related docs

- [Architecture](../ARCHITECTURE.md), [API Contract](../API.md),
  [Network](./network.md), [Server control](./server-control.md),
  [Settings](./settings.md).

# Certificate automation (ACME)

**Purpose:** get a certificate a browser accepts without the admin doing
anything by hand every three months. HomeVault asks a certificate authority for a
certificate for one name, installs the pair where the HTTPS listener reads it,
and renews it a month before it expires. Lives in **Settings > Security >
HTTPS/TLS**, below the listener settings.

## Overview

The feature is a straight RFC 8555 client, written in-house:

- `src/acme.rs` — settings, validation, status derivation, the retry policy, the
  challenge registry, and the base64url / SHA-256 helpers.
- `src/acme/client.rs` — the protocol: account key, JWS, orders,
  authorizations, finalization, installation, and the DNS publishers.
- `src/outbound.rs` — the HTTP seam both this feature and dynamic DNS use.
- `src/acme_worker.rs` — the daily pass and the background issuance.
- `src/handlers/acme.rs` — the endpoints, including the public challenge route.

Two choices drive the design:

1. **The network is a trait.** `Transport` is the only way `client.rs` talks to
   anything. Tests drive the whole flow against a scripted authority, so the
   suite asserts every request in order without ever reaching the internet.
2. **The certificate key is not the account key.** A certificate renewal never
   touches the registration, and a leaked certificate key does not hand over the
   account.

## Files on disk

| Path | Content | Mode |
|---|---|---|
| `data/acme.json` | settings | `0600` |
| `data/acme/account.key` | ACME account key (PEM) | `0600` |
| `data/acme/run.json` | a manual DNS-01 run waiting for the admin | `0600` |
| `data/acme/tls/cert.pem` | the issued certificate chain | `0644` |
| `data/acme/tls/key.pem` | the issued private key | `0600` |

`settings_persistence::reset()` deletes all of it, and clears the in-memory
challenge registry.

## Settings

| Field | Default | Notes |
|---|---|---|
| `enabled` | `false` | the worker is inert while off; enabling needs no restart |
| `domain` | — | one bare DNS name (no scheme, port, path, wildcard, trailing dot) |
| `contact_email` | — | sent to the authority as `mailto:` |
| `directory_url` | the production directory | must be `https://` |
| `tos_agreed` | `false` | required before an order |
| `challenge` | `http-01` | `http-01` or `dns-01` |
| `provider` | `manual` | `manual` or `cloudflare` |
| `dns_api_token` | — | write-only from the API's point of view |
| `dns_propagation_seconds` | `60` | bounded (30 s – 1 h) |

Status is never set optimistically. `refresh_status()` derives it:

| Status | Meaning |
|---|---|
| `disabled` | the feature is off |
| `idle` | on, but no certificate and no attempt yet |
| `pending` | a manual DNS record is waiting for the admin |
| `valid` | an issued certificate with more than 30 days left |
| `expiring` | issued, but 30 days or fewer remain (or it has expired) |
| `error` | the last attempt failed; `last_error` says why |

## The two challenges

**HTTP-01** — the authority fetches
`http://<domain>/.well-known/acme-challenge/<token>`. The answer is published in
a process-wide registry and served by
`GET /.well-known/acme-challenge/{token}`, deliberately **outside** `/api` so no
session, CSRF or contract layer applies. A miss is a bodyless `404`; an answer
carries `Cache-Control: no-store`. This needs port 80 to reach HomeVault.

**DNS-01** — the answer goes into a `_acme-challenge.<domain>` TXT record. With
`provider = manual` the flow stops, persists `run.json`, and shows the record in
the card; the admin creates it and clicks **Confirm**. With
`provider = cloudflare` the record is published, the propagation wait runs, and
the record is removed afterwards — the run finishes on its own.

## Cloudflare zone lookup

A challenge record can live in a delegated subdomain, so the client asks the API
about every candidate zone, longest first (`nas.example.com`, then
`example.com`), and uses the first one the token can see. No public-suffix list
is involved: guessing one is wrong for every private suffix and would add a
dependency to get it. The API call is the authority on which zone exists.

## Retry policy

`should_renew()` is the whole policy:

- off → never;
- a pending manual run → never (the admin still has to act);
- a certificate with more than 30 days left → never;
- otherwise, a backoff that grows with `consecutive_failures`.

A **final** error — the authority refused the domain — sets
`consecutive_failures = u32::MAX`, so the worker stops retrying until the admin
changes the domain, the challenge, the provider or the directory URL. The save
handler resets the counter when any of those changes: the admin just acted on the
error, and a backoff must not keep the worker parked afterwards.

## Endpoints

| Method | Path | Guard |
|---|---|---|
| `GET` | `/api/settings/acme` | admin |
| `PUT` | `/api/settings/acme` | admin |
| `POST` | `/api/settings/acme/issue` | admin |
| `POST` | `/api/settings/acme/confirm` | admin |
| `POST` | `/api/settings/acme/test-challenge` | admin |
| `GET` | `/.well-known/acme-challenge/{token}` | public |

Issuance runs in the background — a DNS-01 order waits minutes for propagation,
which no request deadline allows — and answers `{ "started": true }`. A second
attempt while one is running is refused rather than queued.

**`POST /test-challenge`** publishes a throwaway token and fetches it over plain
HTTP at the public name. It is the one check that tells an admin whether port 80
really reaches HomeVault *before* an authority fails the order for them.

## Token handling

`dns_api_token` is write-only:

- `GET` answers `"set"` when a token is stored, `""` otherwise;
- `PUT` with `"set"` keeps the stored value, `""` clears it, anything else
  replaces it;
- the value never appears in a log line, an event, or an error message.

## Using the certificate

`TlsMode::Acme` makes the listener read `data/acme/tls/{cert,key}.pem`. Saving
that mode is refused until a certificate has actually been issued. If the pair
disappears later, the listener falls back to the self-signed pair with a warning
rather than refusing to serve HTTPS.

## Notifications

Two events, both under the `system` group:

- `certificate.issued` — a certificate was installed (with its expiry);
- `certificate.renewal_failed` — an attempt failed, with the authority's reason.

## Tests

- Unit (`src/acme.rs`): domain / contact / directory / propagation validation,
  status derivation, `should_renew` across every status, backoff growth,
  redaction, the run lock, and SHA-256 known-answer vectors.
- Unit (`src/acme/client.rs`): a scripted authority drives HTTP-01, manual
  DNS-01, automated DNS-01, an invalid authorization, and the Cloudflare
  publisher. Known-answer tests pin the JWK coordinates, the RFC 7638 thumbprint,
  the 64-byte raw ECDSA signature, and the DER-padding rule.
- Integration (`tests/acme_integration.rs`): 401/403 on every endpoint, the
  validation that happens before any authority is contacted, the round-trip of a
  token that is never returned, the pending-token rule of the challenge route, and
  the rejection of a traversal-style token.
- E2E (`tests/e2e/settings/acme.spec.ts`): the card's controls, the status badge
  being one of the documented values, a copied URL refused without storing
  anything, and the DNS fields appearing only for `dns-01`. The suite never starts
  an issuance and never changes the saved settings.

## Related

- [HTTPS / TLS](./tls.md), [Dynamic DNS](./ddns.md),
  [Reachability](./reachability.md), [Notifications](./notifications.md)

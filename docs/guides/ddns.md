# Dynamic DNS

**Purpose:** keep the name you use to reach HomeVault pointing at the address
your ISP handed out this month. Without it a certificate goes stale, a bookmark
breaks, and the reachability advisor cannot tell you why. Lives in **Settings >
Security > Dynamic DNS**.

## Overview

`src/ddns.rs` holds the logic and `src/handlers/ddns.rs` the endpoints. The
worker reads the address this box would use to reach the internet and calls the
provider **only when the address changed** — a home box must not hammer a free
provider, and a provider that rate-limits drops updates silently.

## Configuration

Persisted in `data/ddns.json`.

| Field | Default | Notes |
|---|---|---|
| `enabled` | `false` | enforced while off? no — a disabled entry may be incomplete |
| `provider` | `duckdns` | `duckdns` \| `dynv6` \| `cloudflare` \| `custom` |
| `hostname` | — | bare host name, two labels minimum |
| `token` | — | required by every provider but `custom`; write-only in the API |
| `custom_update_url` | — | `custom` only; must be HTTPS and contain `{ip}` |
| `status` | `disabled` | `disabled` \| `unknown` \| `up-to-date` \| `error` |
| `last_ip` | — | the address the provider is believed to hold |
| `last_update` | — | RFC 3339, UTC |
| `last_error` | — | why the last call failed |

`status` is normalized on load and save: a disabled entry always reads
`disabled`, and an enabled one that has never run reads `unknown` rather than
claiming to be disabled.

## The four providers

| Provider | Request |
|---|---|
| `duckdns` | `GET https://www.duckdns.org/update?domains=<sub>&token=<token>&ip=<ip>`, success is the body `OK` |
| `dynv6` | `GET https://dynv6.com/api/update?hostname=<host>&token=<token>&ipv4=<ip>` |
| `cloudflare` | the A record is found and patched through the API; the token travels as a bearer header, never in a URL |
| `custom` | `GET <custom_update_url>` with `{ip}` (and optionally `{host}`) substituted |

Request building is pure (`build_update`), so the exact URL, method and body are
asserted byte for byte in tests — that is where a token ends up in a URL, and it
is the one place worth pinning.

A plain-text provider that answers `200` with a refusal in the body is treated as
a failure: the status alone is not enough. For DuckDNS, `200 OK` is checked
against the literal marker.

## Address detection

`detect_public_ip()` opens a UDP socket, `connect`s it to `1.1.1.1:53`, and reads
the local address the kernel picked. **A UDP `connect` sends nothing** — it only
asks which source address would be used. No external service is contacted, so
detection works offline and leaks nothing.

Only IPv4 is published: a home connection with no IPv6 lease would otherwise send
an address nobody can reach.

## The worker

`ddns::run()` starts once from `web::run()` and loops every five minutes:

1. load the settings, stop if disabled;
2. detect the current address;
3. compare with `last_ip` — no change, no call;
4. otherwise update, then record the address, the timestamp and the status.

A failure is recorded and retried on the next pass. The loop never exits: a
transient provider outage must not disable dynamic DNS until a restart.

## Endpoints

| Method | Path | Guard |
|---|---|---|
| `GET` | `/api/settings/ddns` | admin |
| `PUT` | `/api/settings/ddns` | admin |
| `POST` | `/api/settings/ddns/update` | admin |

`GET` also reports `current_ip`, so the card can show what the host would publish
before anything is sent.

`PUT` keeps the token write-only (`"set"` / `""` / new value), refuses provider
state as client input, and resets the recorded result when the name or the
provider changes.

`POST /update` sends an update right now regardless of whether the worker thinks
one is due — the "did I get the token right?" button — and reports the provider's
own reason on failure.

## Security

- The token is never returned by `GET`, never logged, and never in a URL for a
  provider that offers a header (Cloudflare).
- `custom` must be HTTPS: plain HTTP would publish the address to anyone on the
  path.
- `hostname` is validated as a bare DNS name — no scheme, port, wildcard,
  whitespace or trailing dot — so a copy-pasted URL is refused instead of sent.

## Tests

- Unit (`src/ddns.rs`): host-name validation across every bad shape, per-provider
  token requirements, the HTTPS / `{ip}` rules of a custom URL, the exact
  DuckDNS / dynv6 / Cloudflare / custom request, plain-text refusal handling,
  redaction, `needs_update`, the settings round-trip and legacy defaults, and
  both Cloudflare write paths (patch an existing record, create a missing one).
- Integration (`tests/ddns_integration.rs`): 401 and 403 on every endpoint,
  defaults with the token redacted, validation before any provider is contacted,
  the write-only round-trip, the provider change that clears `last_ip`, and the
  refusal to update while disabled.
- E2E (`tests/e2e/settings/ddns.spec.ts`): the four providers, the status badge
  being a documented value, a provider token never rendered in clear, "Update now"
  disabled while dynamic DNS is off, a bare URL refused without storing anything,
  and the reachability report's ordered checks and counts.

## Related

- [Certificate automation](./acme.md), [Reachability](./reachability.md),
  [HTTPS / TLS](./tls.md), [Network](./network.md)

# Email OAuth2 (Gmail / Microsoft 365) — P1-16

**Purpose:** send HomeVault's notifications through Gmail or Microsoft 365
without storing the account password and without an app password that the
provider is retiring. HomeVault keeps a **refresh token**, sealed on disk, and
exchanges it for a short-lived access token at send time.

The SMTP relay itself is still configured in **Settings → Notifications**
(host, port, sender). OAuth2 only replaces the *authentication* half.

## Pieces

| Piece | Where |
|---|---|
| Configuration | `data/email_oauth.json` — client id (public), client secret and refresh token **sealed** (`hv1:`), callback URL, provider, last error |
| Access-token cache | process memory only, refreshed 60 s before expiry |
| Authorization endpoints | `/api/settings/email/oauth` (GET/PUT/DELETE), `/start` (POST), `/callback` (GET, public) |
| msmtp configuration | `data/msmtp.conf` (0600), rewritten on every send; its `password` field carries the short-lived access token |
| Settings card | **Notifications → OAuth2 (Gmail / Microsoft 365)** |

## Trust chain

1. The admin creates an OAuth client in the provider console and registers the
   **exact** callback URL, e.g.
   `https://nas.example.com/api/settings/email/oauth/callback`.
2. The card saves the client id, the client secret (sealed with the appliance
   key) and the callback URL. The secret is **never returned** by any endpoint;
   leaving the field blank keeps the stored value.
3. **Connect** calls `POST /start`, which builds the provider URL and sets a
   one-time `hv_email_oauth_state` cookie (`HttpOnly`, `SameSite=Lax`, `Secure`
   when the request arrived over TLS, 10-minute lifetime).
4. The provider redirects the browser to `GET /callback?code=…&state=…`. The
   callback is public by design, but it refuses a request whose `state` does
   not match the cookie or was already used: a prepared link cannot connect an
   account the admin did not start.
5. The token exchange runs on a blocking worker (`reqwest::blocking`, 20 s
   deadline, redirects disabled). The refresh token is sealed and stored; the
   access token is only cached in memory.
6. The next send refreshes the token if needed, writes `msmtp.conf` with
   `auth oauthbearer` (Google) or `auth xoauth2` (Microsoft), and lets msmtp do
   the SMTP conversation.

## Providers

| Provider | Authorization | Token | Scope | msmtp `auth` | Relay |
|---|---|---|---|---|---|
| Google | `accounts.google.com/o/oauth2/v2/auth` (+ `access_type=offline`, `prompt=consent`) | `oauth2.googleapis.com/token` | `https://mail.google.com/` | `oauthbearer` | `smtp.gmail.com:587` |
| Microsoft 365 | `login.microsoftonline.com/common/oauth2/v2.0/authorize` | `login.microsoftonline.com/common/oauth2/v2.0/token` | `offline_access https://outlook.office.com/SMTP.Send` | `xoauth2` | `smtp.office365.com:587` |

Gmail has no send-only scope; the mail scope is all-or-nothing. Microsoft
offers `SMTP.Send`, which is send-only. The card states which relay to use and
links to the provider page again if a pop-up was blocked.

## Failure behaviour

- **Not connected** → the password path is used exactly as before. The feature
  is opt-in.
- **Connected but the refresh fails** (`invalid_grant`, revoked consent,
  network) → the send **fails with the provider's reason**, and the card shows
  it. It never silently falls back to the stored password: that would hide an
  expired consent behind "authentication failed".
- A rotated refresh token is stored before the send continues; losing it would
  force a manual reconnection.
- The last error is bounded (300 characters), stripped of control characters,
  and never contains a token or a secret.
- **Disconnect** deletes `data/email_oauth.json` and clears the memory cache.
  Mail through the relay then uses the password until OAuth2 is connected
  again, or stops if no password is configured.

## Security rules

- The client secret and the refresh token are sealed with the appliance key
  (`crate::storage::secret_box`) before they touch disk; the API returns
  neither, in any endpoint or error message.
- The callback URL is **stored**, never derived from a `Host` header: a forged
  header must not choose where a code is delivered.
- The authorization state is single-use, expires in 10 minutes and is bound to
  the browser by an HttpOnly cookie (the OIDC sign-in model).
- The state, code and tokens are never logged. The audit trail records
  "OAuth2 connected", "disconnected" and "registration saved", nothing else.
- The generated `msmtp.conf` is mode `0600` and is rewritten on every send; the
  access token it holds expires within the hour.

## Tests

- **Unit** (`src/email_oauth.rs`): provider parsing and defaults, the Google
  `offline`/consent parameters, the Microsoft endpoint, percent-encoded
  callback, input validation, token-response parsing (rotation, missing
  `expires_in`, bounded errors), the public status contains no secret, and the
  state is single-use.
- **Unit** (`src/email.rs`): the msmtp config uses `oauthbearer`/`xoauth2`,
  quotes the access token, refuses an unknown mechanism, control characters
  and a cleartext (no-TLS) relay.
- **Unit** (`src/handlers/email_oauth.rs`): HTML escaping of the callback echo,
  the result page's content type and `no-store`, and the state lifetime.
- **Integration** (`tests/email_oauth_integration.rs`): admin gate, validation,
  sealed file, authorization URL + cookie, callback refusal without a cookie /
  with an unknown state / on replay, disconnect. **No test reaches a provider**
  — every case stops before the token endpoint.
- **E2E** (`tests/e2e/settings/settings.spec.ts` §31): labelled card, save
  without echoing the secret, invalid redirect message, Start URL and state,
  admin-only status, disconnect confirmation.

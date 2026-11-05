# Firewall (ufw management)

**Purpose:** Manage the host firewall via `ufw` (Debian/Ubuntu only,
matching `installer/install.sh`'s existing OS check) — view status and the
default policy, add/remove rules, change the default policy, enable/disable
and read the change log — with a hard, server-enforced guard against
locking yourself out of a remote NAS. Lives in the **Settings** page as
the Firewall card.

## Backend

Migration `sqlx/migrations/020_firewall.sql` adds two tables:

- `firewall_settings` — singleton row (`id = 1`): `enabled` (a
  best-effort cache; live `ufw status` is always the source of truth for
  reads), `ssh_port` (default 22), `app_port` (default 8080),
  `confirmed_at`.
- `firewall_change_log` — genuine audit trail (not a rules mirror) of
  changes made through HomeVault: `id`, `action`
  (`enabled`/`disabled`/`rule_added`/`rule_deleted`/
  `default_policy_changed`), `rule_description`, `performed_by`,
  `created_at`. Read back through `GET /firewall/history`.

There is **no** `firewall_rules` table — rules are never mirrored in the
database. `ufw show added`/`ufw status verbose` on the live system is always
the source of truth, mirroring how `src/vpn.rs` reads `wg show` live rather
than caching WireGuard state. This avoids drift between what HomeVault
displays and what the kernel is actually enforcing.

Storage layer `src/storage/firewall_repo.rs` (`FirewallRepository`).

### `src/firewall.rs`

**Availability**

- `ufw_available()` — checked via `sudo ufw --version`, **not** a bare
  `Command::new("ufw")`. `ufw` lives in `/usr/sbin`, commonly absent from
  a non-root service account's `PATH` even though `sudo`'s own
  `secure_path` (used by every real call below) always includes it — a
  bare-PATH check was confirmed live to report "not installed" on a
  correctly-provisioned real server. Deliberately **synchronous**: it is a
  `fn() -> bool` probe for `src/dependencies.rs`, and `--version` returns in
  milliseconds (with `stdin` closed, `sudo` fails immediately instead of
  waiting for a password).

**Reading live state**

- `get_status()` → `FirewallStatus { enabled, default_policy, rules }`.
  `enabled` and `default_policy` come from `sudo ufw status verbose`; the
  rule list comes from `sudo ufw show added`, **not** `ufw status numbered`
  — confirmed live that `ufw status` only lists rules while ufw is *active*,
  so a rule added before ever enabling the firewall (the natural first-time
  flow) would otherwise appear not to exist.
- `parse_default_policy` — parses `Default: deny (incoming), allow
  (outgoing), disabled (routed)`. Without it the page could not answer the
  one question that makes a rule list meaningful (what happens to traffic no
  rule matches), and a firewall left on `allow (incoming)` — a no-op —
  looked identical to a locked-down one.
- `parse_added_rule_line` — handles the simple form (`ufw allow 22/tcp`),
  the source-restricted form `add_rule` produces, interface-scoped rules
  added from a shell (`ufw allow in on eth0 to any port 80 proto tcp`),
  rules with no port at all (`ufw allow from 1.2.3.4`), `limit`, and
  application profiles (`ufw allow OpenSSH`). The `comment '…'` clause is
  split off **before** tokenising, so a comment containing a keyword
  (`to any port 22 proto tcp`) can never be read as part of the rule.
  `is_mandatory` is derived purely by matching the rule's own **comment
  string** against `MANDATORY_SSH_COMMENT`/`MANDATORY_APP_COMMENT` — never
  from client input.

**Argument validation** (every one of these was interpolated straight into
`sudo ufw …` before 2026-10-21)

| Helper | Accepts | Why |
|--------|---------|-----|
| `validate_action` | `allow` `deny` `reject` `limit` | `action` is the **verb** of `sudo ufw <action> …`: `{"action":"delete"}` ran `sudo ufw delete …` from the "add rule" endpoint |
| `validate_protocol` | `tcp` `udp` `any` (empty → `any`) | anything else is passed to ufw as a protocol it will reject with an opaque error |
| `validate_port` | `1`–`65535` or an ascending `a:b` range | `22/tcp` used to be accepted and then concatenated again (`22/tcp/tcp`); the protocol belongs in its own field |
| `validate_source` | IPv4/IPv6 address or CIDR | `from any` would silently turn a "restricted" rule into a global one |
| `validate_comment` | ≤128 chars, no `'`, no control chars | a quote breaks `ufw show added`'s single-quoted round-trip, which `is_mandatory` depends on |
| `validate_default_policy` / `validate_default_direction` | `allow`/`deny`/`reject`, `incoming`/`outgoing`/`routed` | the default policy is a mutation like any other |

**Mutations**

- `add_rule(port, protocol, action, source, comment)` → `AddRuleOutcome`:
  `Added` or `AlreadyPresent`. ufw prints "Skipping adding existing rule"
  and exits 0 for a duplicate, which used to be reported (and audited) as a
  fresh change.
- `delete_rule(spec)` — `spec` is [`FirewallRule::spec`], the exact
  argument string `ufw show added` printed minus its comment clause, so
  `ufw delete <spec>` removes precisely the rule the admin is looking at.
  The previous `delete <action> <port>/<proto>` form silently dropped the
  `from <source>` clause: a source-restricted rule could not be deleted at
  all, and when a *global* rule with the same port existed that global rule
  was deleted instead.
- `ensure_mandatory_rules(&[u16], u16)` — idempotently asserts the rules
  that must always exist. Called at server startup (when `ufw_available()`)
  and as the first step of enable, both with server-detected ports
  (`detect_ssh_ports()` / `detect_app_port()`), never a client value.
- `detect_ssh_ports()` — reads `/etc/ssh/sshd_config` **and**
  `/etc/ssh/sshd_config.d/*.conf`. Debian/Ubuntu ship drop-ins that
  `Include` from the top of the main file, so a `Port 2222` set there is the
  port SSH actually uses while the main file still says 22 (or says
  nothing): protecting only port 22 locked the admin out on the first
  `ufw enable`. Every `Port` value is kept (sshd listens on all of them) and
  the union errs towards an extra open port, never towards a closed SSH
  port. `detect_app_port()` reuses `crate::config::Settings::load().port`,
  the exact port the server itself binds to.
- `enable_verified(&[u16], u16)` — re-asserts the mandatory rules, runs
  `ufw --force enable`, then **re-reads the live state**. `ufw --force
  enable` exits 0 even when iptables refused to load ("ERROR: problem
  running iptables"), so the exit code alone is not evidence. If the
  firewall came up without a mandatory allow rule, it is turned back off and
  the call fails with an explanation instead of leaving the appliance
  unreachable.
- `disable()` — verifies the firewall really is inactive afterwards.
- `set_default_policy(direction, policy)` — `ufw default <policy>
  <direction>`.

**Process plumbing** — every call runs through `run_ufw`, a
`tokio::process::Command` with `kill_on_drop`, `stdin(null)` and a 30 s
`UFW_TIMEOUT`. `ufw` shells out to `iptables`, which takes the xtables lock:
a blocking `Command::output()` in an async handler could park a Tokio worker
for the life of the process. Locale is pinned (`LC_ALL=C`, `LANGUAGE=""`)
so the parsed status strings are stable on a non-English host.

### Endpoints

| Endpoint | Auth | Notes |
|----------|------|-------|
| `GET /firewall/status` | auth | `available`, `enabled`, `default_policy`, live rule list |
| `GET /firewall/enable/preview` | auth | `ssh_port`/`ssh_ports`, `app_port`, `default_policy`, existing rules, `warnings` |
| `GET /firewall/history` | auth | `?limit=` (clamped 1..200, default 50) change log |
| `POST /firewall/enable` | admin | **only** with `{"confirmed": true}`; re-asserts mandatory rules first, then verifies |
| `POST /firewall/disable` | admin | **only** with `{"confirmed": true}`; verifies the result |
| `POST /firewall/default-policy` | admin | `{"direction":"incoming","policy":"deny"}` |
| `POST /firewall/rules` | admin | add a rule; `already_exists: true` when ufw skipped a duplicate |
| `DELETE /firewall/rules` | admin | select a live rule by `spec` (or its identity fields); 404 if absent, 403 if mandatory |

All handlers in `src/handlers/firewall.rs`. Read endpoints use
`require_auth`; mutations use `require_admin`. Argument validation runs
**before** the `ufw_available()` probe, so a malformed request is answered
with the field that is wrong rather than "ufw is not installed" — and is
refused without any process being spawned.

### Self-lockout safety (enforced server-side)

- No generic "set enabled=true" endpoint exists — enabling only happens
  through `handle_firewall_enable`, which requires an explicit
  `{"confirmed": true}` body, always re-asserts the mandatory rules first
  and then verifies the live state (rolling back if they are missing).
- Disabling — which opens every port the firewall was blocking — requires
  the same explicit confirmation. It used to take none, which made "turn the
  security feature off" the only one-click destructive action on the page.
- `handle_firewall_rule_delete` **selects** a rule from the live rule set
  and passes that rule's own `spec` to `ufw delete`; client input never
  becomes an argv entry. A request that matches no live rule is a 404 —
  the handler used to proceed and delete anyway — and a rule whose live
  comment marks it mandatory is a 403, checked against live system state, so
  a raw `curl DELETE` is rejected exactly like a UI click, both while ufw is
  active and inactive.
- The enable preview warns when the default incoming policy is `allow`:
  enabling then filters nothing, and the admin would read "active" while
  still fully exposed.

## Frontend

- `frontend/src/lib/components/settings/FirewallCard.svelte` — status
  badge, default policy row with a "Change" dialog, rule list (mandatory
  rows render a "required" badge and no remove button at all, not merely a
  disabled one), labelled add-rule form, a collapsible "Recent changes"
  list, and one focus-trapped confirmation dialog shared by enable / disable
  / remove / default policy. Every action reports its outcome in a
  `role="status"` line; nothing fails silently any more. The GeoIP block
  adds the country-code/allow-deny form, the managed-country rows and an
  Apply button; a missing database shows the exact path, and the remove
  action uses the same confirmation dialog.
- `frontend/src/lib/services/api.ts`: `getFirewallStatus`,
  `getFirewallEnablePreview`, `enableFirewall`, `disableFirewall`,
  `addFirewallRule`, `deleteFirewallRule`, `setFirewallDefaultPolicy`,
  `getFirewallHistory`, `getGeoIpStatus`, `setGeoIpRule`,
  `removeGeoIpRule`, `applyGeoIpRules`. Types `FirewallRule` (with `spec`),
  `FirewallDefaultPolicy`, `FirewallHistoryEntry`, `GeoIpStatus`,
  `GeoIpCountryRule`.
- Wired into `frontend/src/routes/Settings.svelte` as `<FirewallCard />`,
  after the "Privacy & Data" card.

### Accessibility

- The dialog is `role="dialog"` + `aria-modal="true"` +
  `aria-labelledby`, with `use:focusTrap` (focus moves in, Tab cycles,
  Escape closes, focus returns to the trigger). It used to be a
  `role="presentation"` div inside a `role="button"` overlay with no focus
  handling at all.
- Every input and select has a label (`sr-only` where the row must stay
  compact); the per-row remove button carries an `aria-label` naming the
  rule.
- Confirmation dialogs name the consequence ("every port … will be open
  again", "the rule for 5900/tcp stops applying immediately") instead of
  asking "are you sure".

## i18n

All strings under `[firewall]` in `src/i18n/{en,fr,de,es}.toml`, including
the confirm dialogs, `confirm_ssh_rule`/`confirm_app_rule` (`%port%`),
`default_policy_summary` (`%incoming%`/`%outgoing%`),
`confirm_delete_warning` (`%rule%`), the `history_action_*` labels and the
`geoip_*` strings (`%path%`, `%managed%`, `%max%`, `%count%`, `%country%`).

## Sudoers & installer

`ufw` is invoked as `sudo ufw …` via the `ufw_command()` helper (which also
forces `LC_ALL=C` + `LANGUAGE=""` so status parsing is locale-independent).
Since 2026-09-04 the process runs as **root** and no sudoers file is generated
(`configure_sudoers()` is dead code — see
[Architecture § Deployment model](../ARCHITECTURE.md#deployment-model-runs-as-root));
the `sudo` wrappers stay for a possible future non-root mode. That future
policy is scoped per subcommand (ROADMAP 7.4): `ufw --version` (the
installed-check), `status *`, `show *`, `allow|deny|limit|reject *`,
`delete *` (the tail is the live rule spec `ufw show added` printed),
`--force enable`, `disable` and `default * *` — never `reset` or `reload`.
`installer/install.sh` installs `ufw` via the
existing `install_optional` helper — non-fatal if declined;
`ufw_available()` degrades the UI to "not installed" rather than
erroring.

## GeoIP country policy (P1-14, 2026-11-05)

`src/geoip.rs` turns a manually maintained country database into **managed
ufw rules**. It deliberately does not add a custom iptables chain: ufw's own
`/etc/ufw/user.rules` is the persistence layer, so a rule installed here
survives a reboot exactly like a hand-written one.

- **Database** (`{DATA_DIRECTORY}/data/geoip/countries.json`, overridable with
  `HOMEVAULT_GEOIP_DB`) maps ISO codes to CIDRs:
  `{"FR": ["192.0.2.0/24"], "US": ["198.51.100.0/24"]}`. It is updated
  manually; every network is parsed with `ipnetwork`, duplicate networks are
  removed, and a country may hold at most 512 CIDRs. The global applied-rule
  ceiling is **2048**.
- **Policy** (`{DATA_DIRECTORY}/data/geoip/policy.json`) maps a country to
  `allow` or `deny`. It is written atomically (temp file + rename) only after
  the expanded plan passes the cap check, so a policy that cannot be applied
  never replaces the one in force.
- **Apply** reads `ufw show added`, deletes only rules whose comment starts
  `hv-geoip:` (project-owned), then adds one source-only rule per CIDR:
  `ufw allow|deny from <network> comment 'hv-geoip:<action>:<CC>'`. A failure
  mid-apply removes the rules that run added and reports the error instead of
  leaving a half-applied country.
- **Capabilities:** no `ufw` → **`503`**; missing database → **`503`**
  naming the path; unknown country or bad action/network → **`400`**. Every
  privileged call is argv-only under `UFW_TIMEOUT` (30 s).
- **Endpoints:** `GET /api/firewall/geoip` (auth) reports `available`,
  `ufw_available`, `database_present`, `database_path`, `database_error`,
  the selected countries and the managed-rule count. `POST` sets one
  country and reconciles; `DELETE /api/firewall/geoip/{country}` removes it;
  `POST /api/firewall/geoip/apply` reconciles after a manual database
  refresh. Mutations are admin-only and event-logged.
- **UI:** the Firewall card gains a GeoIP block — country code, allow/deny,
  add, managed countries with their CIDR count/removal, and an Apply button.
  A missing database is shown with its exact path rather than a generic
  failure.

## The login-ban rule (2026-10-31, QW-05)

The single place the firewall is driven by something other than an explicit
admin action: Settings → Security → *Failed sign-in blocking* can install
`ufw deny from <ip>` when an address trips the auto-block threshold.

- `src/firewall.rs::ban_source(ip, comment)` — validates the address with
  `validate_source` (an IPv4/IPv6 address or network, never a host name or a
  shell fragment), validates the comment, then runs exactly
  `sudo ufw deny from <ip> comment homevault-autoblock-<ip>`. Idempotent: ufw
  answers "Skipping adding existing rule" and exits 0.
- `src/firewall.rs::unban_source(ip)` — rebuilds the live spec
  (`delete deny from <ip>`) from the **validated address**, so it can never
  delete a different rule; "rule does not exist" is treated as success, because
  the desired state already holds.
- The rules are tagged with the `homevault-autoblock-` comment prefix, so an
  admin (and the code) can tell them apart from a hand-written rule.
- `ufw` is only touched while both the auto-block **and** its *firewall* switch
  are on; the default is off, because cutting an address off every service is a
  bigger hammer than refusing a sign-in.
- Every invocation goes through `run_ufw` (30 s `UFW_TIMEOUT`) — a wedged `ufw`
  cannot hold the login path.
- **No new sudoers grant**: the ban uses `deny *` and `delete *`, both already in
  the per-subcommand policy above, so enabling the feature cannot widen the
  process's privileges.
- A ban whose rule could not be installed still blocks the web sign-in and
  records `firewall_rule = 0`; the card shows "firewall rule installed" only
  when it really was, and a pardon removes only what that flag claims.

**Not covered by automated tests**: the live effect (a rule appearing in
`ufw status`). It needs root and would modify the host; the argv shape and the
address validation are unit-tested, and the decision is integration-tested with
the firewall switched on (asserting the recorded flag follows
`ufw_available()`).

## Invariants

- Covers `ufw` on Debian/Ubuntu only, matching the installer's existing
  OS check. `ufw_available()` false on any other distro/OS just shows
  "not installed", no crash.
- `ufw_available()` must keep checking via `sudo`, and `get_status()`
  must keep reading `ufw show added` for the rule list — see the Backend
  section above for the two live-confirmed bugs this prevents from
  regressing.
- Nothing unvalidated may reach the `sudo ufw` argv. A delete request
  selects a live rule; it never supplies the command.
- Never weaken the mandatory-rule/confirm-required enforcement described
  above; it's the specific mitigation for a real host-firewall self-
  lockout risk on a remotely-administered NAS. In particular
  `detect_ssh_ports()` must keep reading the `sshd_config.d` drop-ins.
- `dependencies.rs` models `ufw` as **presence-only** (`check_running: None`,
  2026-09-15): its "active" flag is the firewall state the admin controls from
  this app, not a daemon that must be up. Giving it a `running` check made
  every startup warn "installed but its service isn't running" on the intended
  default (firewall off). The regression is pinned by
  `dependencies.rs::tests::test_ufw_is_presence_only_not_a_running_daemon`.

## Verified

Audited live end-to-end on 2026-07-16, including — with the user's
explicit sign-off, since it's a live host-firewall change — a real `ufw`
enable and disable cycle (`sudo ufw --force enable` / `disable`) with the
sandbox remaining reachable throughout. See the "Security suite" section
in `CLAUDE.md` for the full bug list and verification narrative: real
rule add/delete, mandatory-rule deletion rejected via raw `curl` both
while active and inactive, enable-without-confirmation rejected, and the
mandatory SSH/app-port rules confirmed present via `sudo ufw show
added`/`status verbose` both before and after a real enable.

**Production-ready pass, 2026-10-21** (no live `ufw` mutation: `sudo ufw`
asks for a password on this host, so `ufw_available()` is `false`):

- `cargo test --lib firewall` → **31/31** (rule-line parsing for every form
  `ufw show added` emits, the comment/keyword trap, `is_mandatory` from the
  comment only, default-policy parsing, all six validators including
  `action: "delete"` / `port: "22/tcp"` / `source: "any"`, `sshd` port
  detection incl. drop-ins and the never-empty fallback).
- `cargo test --test security_integration` → **25/25**; the firewall tests
  assert the auth guards, the confirmation gates for enable **and** disable,
  the pre-ufw input rejections (action/protocol/port/source), the 404 on a
  rule that is not live, the default-policy validation, and the history
  envelope.
- `tests/e2e/security/security.spec.ts` → **35 passed / 8 skipped**
  (Firefox, scratch server `:8102`): 16 firewall tests, of which §5b's four
  run against `page.route` mocks so the card's default policy, focus-trapped
  remove dialog, `spec`-based delete payload, disable confirmation and error
  feedback are exercised on a host without `ufw`.

## Related

- [Settings](./settings.md), [Antivirus](./antivirus.md),
  [Anti-Ransomware](./ransomware.md), [VPN](./vpn.md), [Network](./network.md)
- Backend: `src/firewall.rs`, `src/storage/firewall_repo.rs`,
  `src/handlers/firewall.rs`

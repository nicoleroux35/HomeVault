# SNMP agent

**Purpose:** let a monitoring tool on the LAN read this appliance's interfaces
and disk usage over SNMP — the appliance is the *agent* here, not the client
(that is [`settings.md`](./settings.md#snmp-roadmap-135)'s half: querying other
devices).

## Overview

SNMP is **off by default**. Nothing answers an SNMP request until an admin
turns the agent on in Settings → Monitoring → **SNMP agent (this appliance)**.
That card writes one file HomeVault owns
(`/etc/snmp/snmpd.conf.d/99-homevault-agent.conf`) and drives the `snmpd`
service; the distro's own `/etc/snmp/snmpd.conf` is never edited.

Two access methods are offered:

- **SNMPv2c** (default) — one generated read-only community.
- **SNMPv3** — a dedicated user, **SHA** authentication and **AES** privacy
  (`authPriv`). The passphrases are generated on enable unless the admin types
  them, and are sealed with the appliance key before they reach
  `snmp-agent.json`.

Three rules are enforced in code, not just documented:

1. **Never `public`.** The community is generated (16 bytes of `/dev/urandom`,
   rendered as 32 hex characters) and `public`/`private` are refused by name —
   they are the two strings every scanner tries first. A community must be 8–32
   characters, with no whitespace, quote, `#` or control character (any of
   those would silently change the meaning of the `snmpd.conf` line). The v3
   user name is limited to letters, digits, `.`, `-` and `_`; each v3
   passphrase is 8–64 printable characters, without whitespace, quotes,
   backslashes or control characters (they sit between double quotes).
2. **Never internet-facing.** The bind scope is either **loopback**
   (`agentaddress 127.0.0.1,[::1]` — only processes on the appliance itself) or
   **the local network** (`agentaddress udp:<address>:161` on *one* of the
   appliance's own private addresses). `0.0.0.0`, a public address and
   `udp:*` are all refused, so there is no configuration that publishes the
   agent.
3. **Read-only and minimal.** v2c renders `rocommunity … -V systemonly`; v3
   renders `createUser … SHA … AES …` plus `rouser … authpriv systemonly`.
   There is no write community and no `rwuser`. The view is limited to
   `system`, `interfaces` and `storage` — the routing table is not reachable,
   and the MAC-address column (`ifPhysAddress`) is excluded.

Disabling removes the file and stops **and** disables the unit, so a reboot
cannot bring a stopped agent back.

## API

Admin only; `snmpd` absent answers **`503`** (capability), never a `400` and
never an optimistic `200` — the screen then says which package to install
instead of showing a dead form.

| Method | Path | Answers |
|---|---|---|
| GET | `/api/snmp/agent` | `{ enabled, scope, lan_address, version, v3_username, location, contact, has_community, has_v3_auth_secret, has_v3_priv_secret, installed, serving, lan_candidates[] }` — **never a community or a passphrase** |
| PUT | `/api/snmp/agent` | `{ enabled?, scope?, lan_address?, version?, community?, v3_username?, v3_auth_secret?, v3_priv_secret?, location?, contact? }` → the same payload plus `message`. A field left out keeps its stored value; an empty `community`/v3 passphrase keeps the current one (or generates one when none is set). `version` is `v2c` (default) or `v3` |

The settings live in `snmp-agent.json` under the data directory, a **separate**
key from `snmp.json` (the client's): enabling the appliance's own agent and
pointing the client at a remote device are two unrelated features that happen
to share the word "SNMP".

A v3 passphrase is persisted as an `hv1:` AES-256-GCM blob, scoped separately
for authentication and privacy (swapping the two fails authentication rather
than handing back the wrong secret). It is never returned by the API and never
written in plaintext outside the mode-600 drop-in the daemon reads. On a data
directory restored on another machine the blobs cannot be opened: the API says
so and asks for the passphrases to be re-entered instead of silently rendering
a half-configured user.

`PUT` validates before it writes anything, then applies to the host, and only
persists once the host accepted the change. When the daemon step fails the
previous configuration is re-applied, so "saved" and "running" can never
disagree silently, and the answer names which half failed.

## Frontend

`frontend/src/lib/components/settings/SnmpAgentCard.svelte`, in the Monitoring
group next to the SNMP *client* card. The switch comes first ("serve SNMP
requests"); the scope, version, credential, location and contact fields only
appear once it is on — there is nothing to configure for a feature that answers
nobody. The version selector swaps the credential block: v2c shows the
community, v3 shows the dedicated user and the two passphrase fields. The LAN
address is a **select of the appliance's real addresses**, never a free text
field. Saving reports through the toast, and a refusal shows the server's own
sentence verbatim.

## Privileges

`src/setup.rs` grants exactly what the module runs, scoped to our own file and
to the `snmpd` unit:

```
tee   /etc/snmp/snmpd.conf.d/99-homevault-agent.conf
chmod 600 /etc/snmp/snmpd.conf.d/99-homevault-agent.conf
rm -f /etc/snmp/snmpd.conf.d/99-homevault-agent.conf
systemctl start|stop|restart|enable|disable snmpd
```

The file carries the community, hence the `600` mode.
`test_snmp_agent_grants_are_scoped_to_its_own_file_and_unit` fails if the grant
widens (a bare `tee` could overwrite any root file; a bare `systemctl` could
control `sshd`).

## Invariants

- Off by default; enabling is an explicit admin action.
- `public`/`private` are never accepted, and a generated community is never
  `public`.
- The agent binds to loopback or to one private address — never a wildcard,
  never a public address.
- Read-only only: no `rwcommunity`, no `rwuser`, no write view, no routing
  table.
- v3 is fixed to SHA authentication + AES privacy with a dedicated user; a
  half-configured user is never rendered (the file says it answers nobody).
- Neither the community nor a v3 passphrase is returned by the API
  (`has_community` / `has_v3_auth_secret` / `has_v3_priv_secret` instead); v3
  passphrases are sealed at rest and the rendered file is mode 600.
- `snmpd` missing is `503`, with the package named.
- Every host call is bounded by a deadline (`utils::process::run_timed*`), so a
  wedged `systemctl` cannot pin a blocking thread.

## Related documentation

- [Settings — SNMP client](./settings.md#snmp-roadmap-135) — querying *other* devices
- [API reference](../API.md)
- [Design system](./design-system.md)

# Real-hardware testing

The default suites run against an in-process router or a scratch server, so
they deliberately never touch real disks, systemd, networking or a second
machine. This guide covers the paths that only a **provisioned host** can
prove, how to run them, and the proof to keep.

Nothing here runs in the default CI matrix. A normal `cargo test` /
`npx playwright test` skips these cases with a named reason.

## What is covered where

| Path | Harness | Gate |
|---|---|---|
| Rescue clone / cancel / resume | `tests/vm/hardware.vm.test.ts` | `HV_VM_RESCUE_*` |
| SMB/NFS service config | `tests/vm/hardware.vm.test.ts` | VM credentials |
| SMB/NFS real write | `scripts/hardware/smb-nfs-acl.sh` (client host) | manual |
| WireGuard peer provision + config | `tests/vm/hardware.vm.test.ts` | VM credentials |
| WireGuard real handshake + traffic | `scripts/hardware/vpn-tunnel.sh` (client host) | manual |
| Netplan rollback | `tests/vm/hardware.vm.test.ts` | `HV_VM_NETPLAN_IFACE` |
| ACL revocation enforcement | `tests/vm/hardware.vm.test.ts` | `HV_VM_ACL_FOLDER` + member creds |
| Per-app proxy listener | `tests/vm/hardware.vm.test.ts` | an installed, running app |
| App-proxy / dashboard HTTPS | `tests/vm/hardware.vm.test.ts` | TLS enabled on the host |

The opt-in smoke suite (`tests/vm/smoke.vm.test.ts`) still covers reachability,
login, the health/storage APIs and mDNS. The hardware suite is the deeper one.

## Run the VM suite

```bash
HV_VM_BASE_URL=http://hv-test.local:8080 \
HV_VM_ADMIN_USER=admin HV_VM_ADMIN_PASS='…' \
npx playwright test --config=playwright.vm.config.ts
```

Every test skips (rather than fails) when its gate is missing, so the same
command is safe on a host that only has some of the capabilities.

### Gates

| Variable | Test | Warning |
|---|---|---|
| `HV_VM_RESCUE_SOURCE` | rescue clone | a dedicated **loop device**, never a live disk |
| `HV_VM_RESCUE_DEST` | rescue clone | a dedicated **loop device**, contents are overwritten |
| `HV_VM_RESCUE_LOGFILE` | rescue clone | a path the rescue service may write (its data dir) |
| `HV_VM_NETPLAN_IFACE` | netplan rollback | a **non-management** interface |
| `HV_VM_ACL_FOLDER` | ACL revocation | a test share the member may reach |
| `HV_VM_MEMBER_USER` / `HV_VM_MEMBER_PASS` | ACL revocation | a non-admin account |
| `HV_VM_HOSTNAME` | mDNS (smoke) | the `.local` host name |

Destructive gates (`HV_VM_RESCUE_*`, `HV_VM_NETPLAN_IFACE`) must point at
disposable devices/interfaces. The suite never guesses one.

## SMB / NFS real write

Run on a client machine that can mount the share:

```bash
sudo SMB_USER=alice SMB_PASS='…' \
  scripts/hardware/smb-nfs-acl.sh smb //hv-test.local/family /mnt/hv-proof

sudo scripts/hardware/smb-nfs-acl.sh nfs hv-test.local:/volume1/family /mnt/hv-proof
```

It mounts, writes, reads back, renames, deletes, verifies the share is clean
and unmounts — even on failure. A read-only export or a stale credential fails
the run. Pass `HV_EXPECT_READONLY=1` to assert the opposite: the account must
not be able to write at all.

## WireGuard tunnel

Run on a client with `wireguard-tools`:

```bash
sudo scripts/hardware/vpn-tunnel.sh hv-peer.conf http://10.8.0.1:8080/api/meta
```

The URL must only be reachable through the tunnel. The script waits for a
non-zero `wg show latest-handshakes` before requesting it, so a route leak
cannot produce a false pass.

## Proof to collect

For a release or a recovery drill, keep in `TESTING.md`:

1. host identity (VM image / model, kernel, HomeVault version);
2. the exact command and its output for each path;
3. `journalctl -u homevault` excerpt for the rescue / netplan / VPN changes;
4. `wg show` and `mount | grep` output for the client-side proofs;
5. for the netplan path, the before/after `ip addr` of the tested interface.

Do not paste secrets (VPN private keys, SMB passwords, session tokens) into
`TESTING.md` or a ticket.

## Known gaps

- A real multi-hour soak with a saturated SMB client and a memory-leak
  verdict over days needs a lab host; the load harness is in
  `scripts/load/README.md`.
- A real reboot after a netplan apply is still a manual step (the API only
  proves the immediate rollback).
- App-proxy HTTPS is asserted when the host already has TLS enabled; enabling
  TLS is itself an admin action this suite does not perform.
- Browser compatibility runs real WebKit and real mobile viewports, but on a
  desktop host; a pass on physical phones/tablets remains a manual lab step.

#!/bin/bash
# ---------------------------------------------------------------------------
# Systemd / AppArmor hardening tests (P2-7).
#
# Like the update-engine suite, this sources `installer/install.sh` with
# `HV_LIB_ONLY=1` and never runs an install step. Run by hand:
#
#   bash installer/tests/unit-hardening.test.sh
# ---------------------------------------------------------------------------
set -uo pipefail

HERE=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
ROOT=$(cd -- "$HERE/../.." && pwd)

HV_LIB_ONLY=1
# shellcheck disable=SC1090,SC1091
source "$ROOT/installer/install.sh"
set +e

PASS=0
FAIL=0
ok()  { PASS=$((PASS + 1)); printf 'ok   %s\n' "$1"; }
bad() { FAIL=$((FAIL + 1)); printf 'FAIL %s\n' "$1"; }

has() {
    local desc="$1" haystack="$2" needle="$3"
    if [[ "$haystack" == *"$needle"* ]]; then ok "$desc"; else bad "$desc (missing: $needle)"; fi
}
lacks() {
    local desc="$1" haystack="$2" needle="$3"
    if [[ "$haystack" != *"$needle"* ]]; then ok "$desc"; else bad "$desc (unexpected: $needle)"; fi
}

# Values `render_service_unit` reads from the caller's environment.
INSTALL_DIR=/opt/homevault
PORT=8080
DATA_DIR=/var/lib/homevault
ENV_FILE=/etc/homevault.env
HTTPS_PORT=8443

ROOT_UNIT=$(render_service_unit root root "" /root)
USER_UNIT=$(render_service_unit homevault homevault "SupplementaryGroups=docker" /var/lib/homevault)

# --- unit: identity and base settings -------------------------------------
has "unit: root user" "$ROOT_UNIT" "User=root"
has "unit: root group" "$ROOT_UNIT" "Group=root"
has "unit: unprivileged user" "$USER_UNIT" "User=homevault"
has "unit: unprivileged group" "$USER_UNIT" "Group=homevault"
has "unit: docker supplementary group" "$USER_UNIT" "SupplementaryGroups=docker"
has "unit: home is explicit" "$ROOT_UNIT" 'Environment="HOME=/root"'
has "unit: data directory" "$ROOT_UNIT" 'Environment="DATA_DIRECTORY=/var/lib/homevault"'
has "unit: exec start" "$ROOT_UNIT" "ExecStart=/opt/homevault/homevault"
has "unit: bind privileged ports" "$ROOT_UNIT" "AmbientCapabilities=CAP_NET_BIND_SERVICE"
has "unit: restart policy" "$ROOT_UNIT" "Restart=on-failure"
has "unit: memory ceiling" "$ROOT_UNIT" "MemoryMax=2G"
has "unit: file descriptor limit" "$ROOT_UNIT" "LimitNOFILE=65536"
has "unit: crash-loop guard" "$ROOT_UNIT" "StartLimitBurst=5"

# --- unit: hardening present ----------------------------------------------
has "hardening: ProtectSystem" "$ROOT_UNIT" "ProtectSystem=true"
has "hardening: binary read-only" "$ROOT_UNIT" "ReadOnlyPaths=-/opt/homevault"
has "hardening: private tmp" "$ROOT_UNIT" "PrivateTmp=true"
has "hardening: no setuid creation" "$ROOT_UNIT" "RestrictSUIDSGID=true"
has "hardening: personality locked" "$ROOT_UNIT" "LockPersonality=true"
has "hardening: no realtime" "$ROOT_UNIT" "RestrictRealtime=true"
has "hardening: kernel logs closed" "$ROOT_UNIT" "ProtectKernelLogs=true"
has "hardening: root gets NNP" "$ROOT_UNIT" "NoNewPrivileges=true"
has "watchdog: systemd restarts a wedged process" "$ROOT_UNIT" "WatchdogSec=30"
has "watchdog: accepts the main PID heartbeat" "$ROOT_UNIT" "NotifyAccess=main"

# --- unit: directives that would break shipped features stay absent -------
lacks "not set: ProtectKernelTunables (VPN sysctl)" "$ROOT_UNIT" $'\nProtectKernelTunables='
lacks "not set: ProtectKernelModules (iSCSI modprobe)" "$ROOT_UNIT" $'\nProtectKernelModules='
lacks "not set: ProtectHostname (server rename)" "$ROOT_UNIT" $'\nProtectHostname='
lacks "not set: ProtectClock (timezone)" "$ROOT_UNIT" $'\nProtectClock='
lacks "not set: PrivateDevices (disks)" "$ROOT_UNIT" $'\nPrivateDevices='
lacks "not set: ProtectHome (user homes)" "$ROOT_UNIT" $'\nProtectHome='
lacks "not set: CapabilityBoundingSet (sudo)" "$ROOT_UNIT" $'\nCapabilityBoundingSet='

# The unprivileged deployment must not set NNP: sudo is its privilege path.
lacks "unprivileged: no NNP" "$USER_UNIT" $'\nNoNewPrivileges='
has "unprivileged: still hardened" "$USER_UNIT" "ProtectSystem=true"

# Neither render may leave an unexpanded command substitution behind.
lacks "unit: helper expanded" "$ROOT_UNIT" '$(service_hardening_block'

# --- AppArmor profile ------------------------------------------------------
PROFILE=$(render_apparmor_profile)
has "apparmor: attaches to the binary" "$PROFILE" "profile homevault /opt/homevault/homevault"
has "apparmor: complain by default" "$PROFILE" "flags=(complain)"
has "apparmor: data directory writable" "$PROFILE" "/var/lib/homevault/** rwkl"
has "apparmor: user homes writable" "$PROFILE" "/home/homevault/** rwkl"
has "apparmor: includes base abstraction" "$PROFILE" "#include <abstractions/base>"
lacks "apparmor: no unexpanded substitution" "$PROFILE" '$('

# --- Update engine unit (P2-8, UI half) -----------------------------------
UPDATE_UNIT=$(render_update_unit /opt/homevault)
has "update unit: fixed ExecStart" "$UPDATE_UNIT" "ExecStart=/opt/homevault/homevault --update-engine"
has "update unit: oneshot" "$UPDATE_UNIT" "Type=oneshot"
has "update unit: needs the installer script" "$UPDATE_UNIT" "ConditionPathExists=/opt/homevault/installer/install.sh"
has "update unit: survives the restart it causes" "$UPDATE_UNIT" "KillMode=process"
has "update unit: bounded start" "$UPDATE_UNIT" "TimeoutStartSec=3600"
# The argv is fixed: no parameter, no shell, no wildcard.
lacks "update unit: no shell" "$UPDATE_UNIT" "/bin/sh"
lacks "update unit: no unexpanded substitution" "$UPDATE_UNIT" '$('

ENFORCE_PROFILE=$(HV_APPARMOR_ENFORCE=1 render_apparmor_profile)
has "apparmor: enforce switch" "$ENFORCE_PROFILE" "flags=(enforce)"
lacks "apparmor: enforce is not complain" "$ENFORCE_PROFILE" "flags=(complain)"

# --- Trust anchors (FIAB-12) ----------------------------------------------
ANCHOR_DIR=$(mktemp -d)
printf 'release-pem-bytes\n' > "$ANCHOR_DIR/release.pem"
if install_trust_anchor "$ANCHOR_DIR/release.pem" "$ANCHOR_DIR/out/release-key.pem"; then
    ok "trust anchor: local file installed"
else
    bad "trust anchor: local file installed"
fi
if [[ -f "$ANCHOR_DIR/out/release-key.pem" ]]; then
    ok "trust anchor: destination exists"
else
    bad "trust anchor: destination exists"
fi
has "trust anchor: bytes copied" "$(cat "$ANCHOR_DIR/out/release-key.pem" 2>/dev/null)" "release-pem-bytes"
if install_trust_anchor "a1b2c3" "$ANCHOR_DIR/out/catalog-key.ed25519"; then
    ok "trust anchor: inline material written"
else
    bad "trust anchor: inline material written"
fi
has "trust anchor: inline value" "$(cat "$ANCHOR_DIR/out/catalog-key.ed25519" 2>/dev/null)" "a1b2c3"
if install_trust_anchor "" "$ANCHOR_DIR/out/never"; then
    bad "trust anchor: empty source refused"
else
    ok "trust anchor: empty source refused"
fi
[[ ! -e "$ANCHOR_DIR/out/never" ]] \
    && ok "trust anchor: refused source writes nothing" \
    || bad "trust anchor: refused source writes nothing"
rm -rf "$ANCHOR_DIR"

# --- Kernel stability + monitoring choice (P1-28) -------------------------
SYSCTL=$(render_sysctl)
has "sysctl: panic reboots" "$SYSCTL" "kernel.panic = 10"
has "sysctl: oops reboots" "$SYSCTL" "kernel.panic_on_oops = 1"
has "sysctl: bounded background writeback" "$SYSCTL" "vm.dirty_background_ratio = 5"
has "sysctl: bounded writeback" "$SYSCTL" "vm.dirty_ratio = 10"
lacks "sysctl: no unexpanded substitution" "$SYSCTL" '$('

# The host daemons are opt-in, and an already-enabled one is never doubled.
has "monitor: missing package" "$(monitor_decision n n 0)" "unavailable"
has "monitor: already enabled wins over the request" "$(monitor_decision y y 1)" "already"
has "monitor: explicit enable" "$(monitor_decision y n 1)" "enable"
has "monitor: default is leave" "$(monitor_decision y n 0)" "leave"

printf '\n%d passed, %d failed\n' "$PASS" "$FAIL"
[[ "$FAIL" -eq 0 ]]

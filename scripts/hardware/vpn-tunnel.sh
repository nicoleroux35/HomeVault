#!/usr/bin/env bash
# ---------------------------------------------------------------------------
# Real WireGuard tunnel proof — run on a CLIENT machine with wireguard-tools.
#
# The VM suite proves the server provisions a peer and returns a valid config;
# this script proves the tunnel actually carries traffic. It brings the
# config up, checks the handshake, fetches a URL through it, then tears it
# down. The URL must only be reachable through the tunnel (e.g. the NAS's
# private address), otherwise a successful curl proves nothing.
#
# Usage:
#   sudo scripts/hardware/vpn-tunnel.sh peer.conf http://10.8.0.1:8080/api/meta
#
# Requirements: root, wireguard-tools, curl.
# ---------------------------------------------------------------------------
set -euo pipefail

CONFIG=${1:-}
URL=${2:-}
IFACE=${HV_WG_IFACE:-$(basename "$CONFIG" .conf)}

if [[ -z "$CONFIG" || -z "$URL" ]]; then
    echo "usage: $0 <peer.conf> <url-only-reachable-through-the-tunnel>" >&2
    exit 2
fi
if [[ $EUID -ne 0 ]]; then
    echo "must run as root (wg-quick)" >&2
    exit 1
fi
command -v wg-quick >/dev/null || { echo "wg-quick not found" >&2; exit 1; }
command -v curl >/dev/null || { echo "curl not found" >&2; exit 1; }
test -f "$CONFIG" || { echo "config not found: $CONFIG" >&2; exit 1; }

cleanup() { wg-quick down "$CONFIG" 2>/dev/null || true; }
trap cleanup EXIT

wg-quick up "$CONFIG"

# A handshake proves the peer is authenticated, not just that the interface is
# up. Retry briefly: the first packet triggers the handshake.
HANDSHAKE=""
for _ in $(seq 1 15); do
    HANDSHAKE=$(wg show "$IFACE" latest-handshakes 2>/dev/null | awk '{print $2; exit}')
    [[ -n "$HANDSHAKE" && "$HANDSHAKE" != "0" ]] && break
    sleep 1
done
if [[ -z "$HANDSHAKE" || "$HANDSHAKE" == "0" ]]; then
    echo "no WireGuard handshake after 15s — check keys, endpoint and firewall" >&2
    exit 1
fi

CODE=$(curl -fsS -o /dev/null -w '%{http_code}' --max-time 10 "$URL")
test "$CODE" = "200"
echo "ok: tunnel handshake + HTTP 200 through $URL"

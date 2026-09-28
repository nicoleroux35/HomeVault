#!/bin/bash

# HomeVault Release Startup Script
# Usage: ./start HomeVault release.sh

set -e

echo "========================================"
echo "   Starting HomeVault (Release Mode)"
echo "========================================"
echo ""

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
MAGENTA='\033[0;35m'
CYAN='\033[0;36m'
BOLD='\033[1m'
DIM='\033[2m'
NC='\033[0m'

# ---------------------------------------------------------------------------
# HomeVault always runs as root.
#
# Every privileged path — `ensure_dirs` below and the `sudo` wraps inside the
# binary (network.rs, disk.rs, pools.rs, encryption.rs, vpn.rs, setup.rs) —
# assumes root: `sudo` is a transparent no-op once the process already is
# root (see `ip_sudo()` in network.rs), so the share services (SFTP/FTP/SMB/
# NFS), `groupadd` and the mDNS publish stop failing with "A terminal is
# required to authenticate" when no TTY is attached.
#
# We re-exec the *script*, not just the final `exec "$BINARY"`: the artifact
# checks, directory creation and the SQLite probe then run with exactly the
# privileges the server runs with, so ownership and the WAL check describe
# the real runtime state instead of a non-root view of it.
# ---------------------------------------------------------------------------
if [ "$(id -u)" -ne 0 ]; then
    # `sudo` resets HOME to /root and strips DATA_DIRECTORY (env_reset), which
    # would silently move CONFIG_DIR under /root and drop a data-dir/port
    # override. Forward the settings the binary actually reads (config/settings.rs
    # + the rclone HOME fallback) through `env`, whose arguments sudo cannot
    # filter. PATH is deliberately NOT forwarded: sudo's secure_path is the
    # safer default for a root process.
    PRESERVE=()
    for _var in HOME DATA_DIRECTORY DATA_DIR PORT LOG_LEVEL MAX_CONNECTIONS SESSION_LIFETIME; do
        _val="${!_var}"
        if [ -n "$_val" ]; then
            PRESERVE+=("$_var=$_val")
        fi
    done
    unset _var _val

    echo -e "${YELLOW}Re-launching with sudo — HomeVault always runs as root.${NC}"
    # Absolute path + explicit bash: $0 is relative to the caller's cwd and
    # this file is not executable, so `sudo env <path>` alone would not run.
    SCRIPT_PATH="$(readlink -f "${BASH_SOURCE[0]}")"
    exec sudo -- env "${PRESERVE[@]}" /bin/bash "$SCRIPT_PATH" "$@"
fi

BINARY="./target/release/homevault"
FRONTEND_DIR="./static/dist"
CONFIG_DIR="${HOME}/.config/homevault"
# The binary reads DATA_DIRECTORY (not DATA_DIR); keep both aligned so an
# override actually reaches the server.
DATA_DIR="${DATA_DIRECTORY:-${DATA_DIR:-/var/lib/homevault}}"
DB_FILE="${DATA_DIR}/homevault.db"
PORT="${PORT:-8080}"

check_files() {
    echo -e "${YELLOW}Verifying release artifacts...${NC}"
    echo ""

    # Proof that the guard above held: uid 0 means every `sudo` the binary
    # shells out to resolves without a password and without a TTY.
    if [ "$(id -u)" -eq 0 ]; then
        echo -e "  ${GREEN}✓${NC} Running as:${NC} ${BOLD}root${NC} ${DIM}(uid 0 — internal sudo calls are no-ops)${NC}"
    else
        echo -e "  ${RED}✗${NC} Running as:${NC} ${RED}$(id -un) (uid $(id -u)) — privileged calls will fail${NC}"
    fi

    local missing=0

    if [ ! -f "$BINARY" ]; then
        echo -e "${RED}  ✗ Backend binary not found:${NC} ${DIM}$BINARY${NC}"
        echo -e "${YELLOW}    Run ./compile backend release.sh first${NC}"
        missing=1
    else
        echo -e "${GREEN}  ✓ Backend binary found:${NC} ${DIM}$BINARY${NC}"
        echo -e "     ${DIM}Size: $(du -h "$BINARY" | cut -f1)${NC}"
    fi

    if [ ! -d "$FRONTEND_DIR" ]; then
        echo -e "${RED}  ✗ Frontend build not found:${NC} ${DIM}$FRONTEND_DIR${NC}"
        echo -e "${YELLOW}    Run ./compile frontend release.sh first${NC}"
        missing=1
    else
        echo -e "${GREEN}  ✓ Frontend build found:${NC} ${DIM}$FRONTEND_DIR${NC}"
        echo -e "     ${DIM}Size: $(du -sh "$FRONTEND_DIR" | cut -f1)${NC}"
    fi

    echo ""

    if [ "$missing" -eq 1 ]; then
        echo -e "${RED}========================================"
        echo "   Missing artifacts!"
        echo "========================================${NC}"
        echo ""
        echo -e "Please compile the missing components:"
        echo -e "  ${YELLOW}./compile frontend release.sh${NC}   # Build Svelte frontend"
        echo -e "  ${YELLOW}./compile backend release.sh${NC}    # Build Rust backend"
        echo ""
        exit 1
    fi
}

ensure_dirs() {
    # Root by this point (see the guard above), so no nested `sudo` is needed;
    # SUDO_USER is who invoked us and keeps the ownership meaningful now that
    # USER/LOGNAME are reset to root by sudo.
    local owner="${SUDO_USER:-$USER}"
    if [ ! -d "$DATA_DIR" ]; then
        echo -e "${YELLOW}Creating data directory: $DATA_DIR${NC}"
        mkdir -p "$DATA_DIR"
        chown "$owner:$owner" "$DATA_DIR"
    fi
    mkdir -p "$CONFIG_DIR"
    chown "$owner:$owner" "$CONFIG_DIR" 2>/dev/null || true
}

check_db() {
    if [ -f "$DB_FILE" ]; then
        local wal_check=$(sqlite3 "$DB_FILE" "PRAGMA journal_mode;" 2>/dev/null || echo "")
        if [ "$wal_check" = "wal" ]; then
            echo -e "   ${GREEN}✓${NC} ${DIM}Database optimized (WAL mode active)${NC}"
        else
            echo -e "   ${YELLOW}⚠${NC} ${DIM}Database not optimized (WAL will be enabled on startup)${NC}"
        fi
        local db_size=$(du -h "$DB_FILE" 2>/dev/null | cut -f1)
        if [ -n "$db_size" ]; then
            echo -e "   ${BLUE}ℹ${NC} ${DIM}Database size: ${db_size}${NC}"
        fi
    else
        echo -e "   ${CYAN}⟲${NC} ${DIM}New database will be created with optimizations${NC}"
    fi
}

# The backend embeds the built frontend into the binary at compile time.
# If the binary is older than the current frontend build, the UI would serve
# stale/missing assets (HTTP 500 "Failed to load application"), so warn early.
check_stale_binary() {
    if [ -f "$BINARY" ] && [ -f "$FRONTEND_DIR/index.html" ] && [ "$FRONTEND_DIR/index.html" -nt "$BINARY" ]; then
        echo -e "${YELLOW}⚠ The backend binary is OLDER than the current frontend build.${NC}"
        echo -e "   The binary embeds the frontend at compile time, so the UI would be stale."
        echo -e "   Recompile the backend: ${YELLOW}./compile backend release.sh${NC}"
        echo ""
    fi
}

get_local_ip() {
    local_ip=""
    if command -v ip &> /dev/null; then
        local_ip=$(ip route get 1.1.1.1 2>/dev/null | grep -oP 'src \K\S+' | head -1)
    fi
    if [ -z "$local_ip" ] && command -v hostname &> /dev/null; then
        local_ip=$(hostname -I 2>/dev/null | awk '{print $1}')
    fi
    if [ -z "$local_ip" ] && command -v ifconfig &> /dev/null; then
        local_ip=$(ifconfig 2>/dev/null | grep -E "inet " | grep -v 127.0.0.1 | awk '{print $2}' | head -1)
    fi
    echo "$local_ip"
}

get_public_ip() {
    timeout 5 curl -s --max-time 3 https://api.ipify.org 2>/dev/null || \
    timeout 5 curl -s --max-time 3 https://icanhazip.com 2>/dev/null || \
    echo ""
}

display_welcome() {
    clear
    BANNER='
    ╔═══════════════════════════════════════════════════════════════════════╗
    ║                                                                       ║
    ║   ██╗    ██╗███████╗██╗      ██████╗ ██████╗ ███╗   ███╗███████╗      ║
    ║   ██║    ██║██╔════╝██║     ██╔════╝██╔═══██╗████╗ ████║██╔════╝      ║
    ║   ██║ █╗ ██║█████╗  ██║     ██║     ██║   ██║██╔████╔██║█████╗        ║
    ║   ██║███╗██║██╔══╝  ██║     ██║     ██║   ██║██║╚██╔╝██║██╔══╝    ║
    ║   ╚███╔███╔╝███████╗███████╗╚██████╗╚██████╔╝██║ ╚═╝ ██║███████╗  ║
    ║    ╚══╝╚══╝ ╚══════╝╚══════╝ ╚═════╝ ╚═════╝ ╚═╝     ╚═╝╚══════╝  ║
    ║                                                                       ║
    ║                    Your NAS, but make it cute.                        ║
    ║                                                                       ║
    ╚═══════════════════════════════════════════════════════════════════════╝
'
    echo -e "${CYAN}$BANNER${NC}"
    echo -e "   ${MAGENTA}🚀 ${BOLD}Release Mode${NC}${MAGENTA} — Production Ready${NC}"
    echo ""
    echo -e "   ${DIM}─────────────────────────────────────────────────────────────────${NC}"
    echo ""
}

display_connection_info() {
    local local_ip=$(get_local_ip)
    local public_ip=$(get_public_ip)

    echo -e "   ${BOLD}${GREEN}Your HomeVault is ready!${NC}"
    echo ""
    echo -e "   ${DIM}Access your NAS from:${NC}"
    echo ""

    if [ -n "$local_ip" ]; then
        echo -e "   ${YELLOW}┌─ LOCAL (LAN) ────────────────────────────────────────┐${NC}"
        echo -e "   ${YELLOW}│${NC}                                                          ${YELLOW}│${NC}"
        echo -e "   ${YELLOW}│${NC}   🌐  ${BOLD}http://${local_ip}:${PORT}${NC}                              ${YELLOW}│${NC}"
        echo -e "   ${YELLOW}│${NC}   💻  http://127.0.0.1:${PORT}                             ${YELLOW}│${NC}"
        echo -e "   ${YELLOW}│${NC}                                                          ${YELLOW}│${NC}"
        echo -e "   ${YELLOW}└──────────────────────────────────────────────────────────┘${NC}"
        echo ""
    else
        echo -e "   ${YELLOW}┌─ LOCAL ────────────────────────────────────────────┐${NC}"
        echo -e "   ${YELLOW}│${NC}                                                          ${YELLOW}│${NC}"
        echo -e "   ${YELLOW}│${NC}   💻  ${BOLD}http://127.0.0.1:${PORT}${NC}                              ${YELLOW}│${NC}"
        echo -e "   ${YELLOW}│${NC}   (Local IP could not be detected)                 ${YELLOW}│${NC}"
        echo -e "   ${YELLOW}│${NC}                                                          ${YELLOW}│${NC}"
        echo -e "   ${YELLOW}└──────────────────────────────────────────────────────────┘${NC}"
        echo ""
    fi

    if [ -n "$public_ip" ]; then
        echo -e "   ${CYAN}┌─ REMOTE (Internet) ─────────────────────────────────┐${NC}"
        echo -e "   ${CYAN}│${NC}                                                          ${CYAN}│${NC}"
        echo -e "   ${CYAN}│${NC}   🌎  ${BOLD}http://${public_ip}:${PORT}${NC}                           ${CYAN}│${NC}"
        echo -e "   ${CYAN}│${NC}   ⚠️   (Requires port forwarding on your router!)  ${CYAN}│${NC}"
        echo -e "   ${CYAN}│${NC}                                                          ${CYAN}│${NC}"
        echo -e "   ${CYAN}└──────────────────────────────────────────────────────────┘${NC}"
        echo ""
    else
        echo -e "   ${DIM}┌─ REMOTE ────────────────────────────────────────────┐${NC}"
        echo -e "   ${DIM}│${NC}                                                          ${DIM}│${NC}"
        echo -e "   ${DIM}│${NC}   (Public IP not available - check internet connection)  ${DIM}│${NC}"
        echo -e "   ${DIM}│${NC}                                                          ${DIM}│${NC}"
        echo -e "   ${DIM}└──────────────────────────────────────────────────────────┘${NC}"
        echo ""
    fi

    if [ -f "$DB_FILE" ]; then
        echo -e "   ${MAGENTA}┌─ ACCOUNT ────────────────────────────────────────────┐${NC}"
        echo -e "   ${MAGENTA}│${NC}                                                          ${MAGENTA}│${NC}"
        echo -e "   ${MAGENTA}│${NC}   👤  Log in with the admin account you already created ${MAGENTA}│${NC}"
        echo -e "   ${MAGENTA}│${NC}                                                          ${MAGENTA}│${NC}"
        echo -e "   ${MAGENTA}└──────────────────────────────────────────────────────────┘${NC}"
        echo ""
    else
        echo -e "   ${MAGENTA}┌─ FIRST-TIME SETUP ──────────────────────────────────┐${NC}"
        echo -e "   ${MAGENTA}│${NC}                                                          ${MAGENTA}│${NC}"
        echo -e "   ${MAGENTA}│${NC}   👤  No admin account exists yet.                       ${MAGENTA}│${NC}"
        echo -e "   ${MAGENTA}│${NC}   ✨  Open the URL above - the Setup Wizard will guide   ${MAGENTA}│${NC}"
        echo -e "   ${MAGENTA}│${NC}       you through creating your own admin account.      ${MAGENTA}│${NC}"
        echo -e "   ${MAGENTA}│${NC}                                                          ${MAGENTA}│${NC}"
        echo -e "   ${MAGENTA}└──────────────────────────────────────────────────────────┘${NC}"
        echo ""
    fi

    echo -e "   ${DIM}─────────────────────────────────────────────────────────────────${NC}"
    echo ""
    check_db
    echo ""
    echo -e "   ${DIM}─────────────────────────────────────────────────────────────────${NC}"
    echo -e "   ${DIM}Tips:${NC}"
    echo -e "   ${DIM}• Use HTTPS in production (reverse proxy recommended)${NC}"
    echo -e "   ${DIM}• Stop server: Ctrl+C${NC}"
    echo ""
    echo -e "${BOLD}   Starting server with optimized SQLite backend...${NC}"
    echo -e "${DIM}   (WAL mode, 32MB cache, mmap enabled)${NC}"
    echo ""
}

# Main execution
check_files
ensure_dirs
check_stale_binary
display_welcome
display_connection_info

trap 'echo ""; echo -e "${YELLOW}Shutting down HomeVault...${NC}"; exit 0' INT TERM

export DATA_DIRECTORY="${DATA_DIRECTORY:-$DATA_DIR}"
# The guard at the top guarantees uid 0 by the time we get here, so the
# binary — and every `sudo` it shells out to for shares, disks and VPN —
# runs as root: no password, no TTY, no "A terminal is required" surprise.
exec "$BINARY" "$@"

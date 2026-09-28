#!/bin/bash

# HomeVault Frontend Release Compiler
# Usage: ./compile frontend release.sh   (or sudo ./compile frontend release.sh)
#
# The frontend build never needs root: node/npm only write static/dist and
# frontend/node_modules, both owned by the developer. sudo resets PATH to the
# system Node (often v18), which Vite 8 / rolldown / Tailwind v4 reject, so
# this script re-resolves the invoking user's real toolchain (nvm ≥ 20) and,
# when launched via sudo, hands the build back to that user.

set -e

# ---------------------------------------------------------------------------
# Toolchain resolution — find a Node >= 20 and return its bin directory.
# Order: explicit $NODE_BIN > nvm installs of the given home > node on PATH.
# ---------------------------------------------------------------------------
pick_node_bin() {
    local home="$1"

    # 1) explicit override
    if [ -n "$NODE_BIN" ] && [ -x "$NODE_BIN" ]; then
        dirname "$NODE_BIN"
        return 0
    fi

    local best="" best_major=0 cand major
    # 2) nvm installs, highest version first
    for cand in "$home"/.nvm/versions/node/*/bin/node; do
        [ -x "$cand" ] || continue
        major="$("$cand" --version 2>/dev/null | sed -E 's/^v?([0-9]+).*/\1/')"
        if [ "${major:-0}" -ge 20 ] && [ "${major:-0}" -gt "$best_major" ]; then
            best="$cand"
            best_major="$major"
        fi
    done
    if [ -n "$best" ]; then
        dirname "$best"
        return 0
    fi

    # 3) a Node >= 20 already on PATH
    if command -v node >/dev/null 2>&1; then
        major="$(node --version 2>/dev/null | sed -E 's/^v?([0-9]+).*/\1/')"
        if [ "${major:-0}" -ge 20 ]; then
            dirname "$(command -v node)"
            return 0
        fi
    fi

    echo ""
}

# If launched via sudo, re-run as the invoking user with THEIR nvm toolchain on
# PATH (sudo's default PATH only exposes the system Node, which is too old).
if [ -n "$SUDO_USER" ] && [ "$(id -u)" -eq 0 ]; then
    INVOKER_HOME="/home/$SUDO_USER"
    TOOL_BIN="$(pick_node_bin "$INVOKER_HOME")"
    export HOME="$INVOKER_HOME"
    export PATH="${TOOL_BIN:+$TOOL_BIN:}$PATH"
    exec sudo -u "$SUDO_USER" env HOME="$HOME" PATH="$PATH" "$0" "$@"
fi

# Normal run (direct or after the sudo re-exec): resolve against $HOME.
TOOL_BIN="$(pick_node_bin "${HOME:-$PWD}")"

if [ -z "$TOOL_BIN" ]; then
    echo -e "\033[0;31mError: no Node.js >= 20 found.\033[0m"
    echo -e "Install one, e.g. via nvm (https://github.com/nvm-sh/nvm), then retry."
    echo -e "You can point at a specific binary with: NODE_BIN=/path/to/node ./compile frontend release.sh"
    exit 1
fi

export PATH="$TOOL_BIN:$PATH"
export NODE_VERSION="$(node --version 2>/dev/null || echo unknown)"

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

check_deps() {
    echo -e "${YELLOW}Checking dependencies...${NC}"

    if ! command -v node &> /dev/null; then
        echo -e "${RED}Error: Node.js is not installed!${NC}"
        exit 1
    fi

    if ! command -v npm &> /dev/null; then
        echo -e "${RED}Error: npm is not installed!${NC}"
        exit 1
    fi

    echo -e "Using Node ${NODE_VERSION} at ${YELLOW}$(command -v node)${NC}"
    echo -e "${GREEN}All dependencies OK!${NC}"
}

clean_build() {
    if [ -d "static/dist" ]; then
        echo -e "${YELLOW}Cleaning previous frontend build...${NC}"
        rm -rf static/dist
    fi
    if [ -d "frontend/.svelte-kit" ]; then
        rm -rf frontend/.svelte-kit
    fi
}

install_deps() {
    echo ""
    echo -e "${YELLOW}Installing frontend dependencies...${NC}"
    cd frontend
    npm install
    cd ..
}

build_frontend() {
    echo ""
    echo -e "${YELLOW}Compiling frontend (Svelte + Vite)...${NC}"
    echo ""

    cd frontend
    if npm run build 2>&1; then
        echo ""
        echo -e "${GREEN}========================================"
        echo "   Frontend Build Successful!"
        echo "========================================${NC}"
        echo ""
        echo -e "Output directory: ${YELLOW}static/dist/${NC}"
        if [ -d "static/dist" ]; then
            echo -e "Total size: ${YELLOW}$(du -sh static/dist | cut -f1)${NC}"
        fi
    else
        echo ""
        echo -e "${RED}========================================"
        echo "   Frontend Build Failed!"
        echo "========================================${NC}"
        exit 1
    fi
    cd ..
}

main() {
    check_deps
    clean_build
    install_deps
    build_frontend

    echo ""
    echo -e "Frontend ready for production at: ${YELLOW}static/dist/${NC}"
    echo ""
}

main
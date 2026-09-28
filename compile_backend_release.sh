#!/bin/bash

# HomeVault Backend Release Compiler
# Usage: ./compile backend release.sh

set -e

echo "========================================"
echo " Compiling HomeVault Backend (Release)"
echo "========================================"
echo ""

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

check_deps() {
    echo -e "${YELLOW}Checking dependencies...${NC}"

    if ! command -v cargo &> /dev/null; then
        echo -e "${RED}Error: Rust/Cargo is not installed!${NC}"
        echo "Install Rust: curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh"
        exit 1
    fi

    if [[ "$OSTYPE" == "linux-gnu"* ]]; then
        echo -e "${YELLOW}Checking Linux dependencies...${NC}"

        if ! dpkg -s pkg-config &> /dev/null; then
            echo -e "${YELLOW}Installing pkg-config...${NC}"
            sudo apt-get update && sudo apt-get install -y pkg-config
        fi

        if ! dpkg -s libsqlite3-dev &> /dev/null; then
            echo -e "${YELLOW}Installing libsqlite3-dev...${NC}"
            sudo apt-get install -y libsqlite3-dev
        fi

        if ! dpkg -s build-essential &> /dev/null; then
            echo -e "${YELLOW}Installing build-essential...${NC}"
            sudo apt-get install -y build-essential
        fi
    fi

    echo -e "${GREEN}All dependencies OK!${NC}"
}

check_frontend() {
    # The backend embeds the whole `static/` tree (built Svelte UI under
    # `dist/`) into the binary at compile time. Compiling without a frontend
    # build produces a binary whose UI 500s with "Failed to load application",
    # so refuse to build rather than silently shipping a broken binary.
    echo -e "${YELLOW}Checking frontend build...${NC}"
    if [ ! -f "static/dist/index.html" ]; then
        echo -e "${RED}Error: frontend build not found (static/dist/index.html).${NC}"
        echo -e "The backend embeds the frontend at compile time — build it first:"
        echo -e "  ${YELLOW}./compile frontend release.sh${NC}"
        exit 1
    fi
    # Warn if the frontend sources are newer than the build that will be
    # embedded, so a stale UI doesn't sneak into the release binary.
    if [ -n "$(find frontend/src -newer static/dist/index.html -print -quit 2>/dev/null)" ]; then
        echo -e "${YELLOW}⚠ Frontend sources are newer than the current build — the embedded UI will be stale.${NC}"
        echo -e "   Rebuild it first: ${YELLOW}./compile frontend release.sh${NC}"
    fi
    echo -e "${GREEN}Frontend build OK${NC}"
}

clean_build() {
    if [ -d "target/release" ]; then
        echo -e "${YELLOW}Cleaning previous backend build...${NC}"
        cargo clean --release
    fi
}

build_backend() {
    echo ""
    echo -e "${YELLOW}Compiling backend (Rust)...${NC}"
    echo ""

    if cargo build --release 2>&1; then
        echo ""
        echo -e "${GREEN}========================================"
        echo "   Backend Build Successful!"
        echo "========================================${NC}"
        echo ""
        echo -e "Binary location: ${YELLOW}target/release/homevault${NC}"
        if [ -f "target/release/homevault" ]; then
            echo -e "Size: ${YELLOW}$(du -h target/release/homevault | cut -f1)${NC}"
        fi
    else
        echo ""
        echo -e "${RED}========================================"
        echo "   Backend Build Failed!"
        echo "========================================${NC}"
        exit 1
    fi
}

main() {
    check_deps
    check_frontend
    clean_build
    build_backend

    echo ""
    echo -e "Backend ready for production at: ${YELLOW}target/release/homevault${NC}"
    echo ""
}

main

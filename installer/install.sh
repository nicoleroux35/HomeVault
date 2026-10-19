#!/bin/bash
set -euo pipefail

# HomeVault Installation Script — the only installer file: packages, the
# systemd unit, the udiskie automount config, the download and the final
# verification all live here (nothing is read from a sibling file).
# Supported OS: Debian 10+, Ubuntu 20.04+
# Architecture: x86_64, aarch64, armv7l
#
# Usage (recommended — downloads + installs everything in one command):
#   curl -fsSL https://raw.githubusercontent.com/nicoleroux35/HomeVault/main/installer/install.sh | sudo bash
#   wget -qO- https://raw.githubusercontent.com/nicoleroux35/HomeVault/main/installer/install.sh | sudo bash
#
# Usage with an already-downloaded archive (skips the download step):
#   sudo ./install.sh /path/to/homevault.tar.gz
#
# The pre-built binary is downloaded from the project's GitHub Releases by
# default, as a per-architecture asset (`homevault-<arch>.tar.gz`). Override
# the source without editing the script:
#   sudo HV_RELEASE_URL=https://mirror/homevault-x86_64.tar.gz bash -c "$(curl -fsSL <install-url>)"
# Optionally pin the download's integrity:
#   sudo HV_RELEASE_SHA256=<hex> bash -c "$(curl -fsSL <install-url>)"
#
# Maintainers publish those assets with this same file (needs Rust + Node ≥ 20,
# run from a checkout):
#   ./installer/install.sh --package
#
# ---------------------------------------------------------------------------
# Maintainer mode — `install.sh --package`
#
# Builds the per-architecture release archive this installer downloads:
#   installer/release/homevault-<arch>.tar.gz  (+ .sha256)
# It never touches the running system and never needs root.
#
# PORTABILITY WARNING — the binary is linked against the glibc of the build
# machine. A binary built on Ubuntu 24.04 (glibc 2.39) will not run on the
# supported matrix below (Debian 12 = 2.36, Ubuntu 22.04 = 2.35): it crashes
# at startup with `version GLIBC_2.3X not found`. Before publishing a
# release, build inside the OLDEST supported base:
#   docker run --rm -v "$PWD":/repo -w /repo rust:1-bookworm \
#       cargo build --release --bin homevault
# (then re-run `--package` so the tarball picks it up — sqlx bundles SQLite,
# so the glibc is the only portability axis).
# ---------------------------------------------------------------------------
if [[ "${1:-}" == "--package" || "${1:-}" == "-p" ]]; then
    case "$0" in
        */*) PKG_ROOT=$(cd -- "$(dirname -- "$0")/.." && pwd) ;;
        *)   PKG_ROOT=$(pwd) ;;
    esac
    # `bash install.sh` from inside installer/ resolves to installer/ — walk
    # up to the checkout root that actually holds Cargo.toml.
    if [[ ! -f "$PKG_ROOT/Cargo.toml" && -f "$PKG_ROOT/../Cargo.toml" ]]; then
        PKG_ROOT=$(cd -- "$PKG_ROOT/.." && pwd)
    fi
    cd "$PKG_ROOT"
    PKG_ARCH=$(uname -m)
    for tool in cargo npm; do
        if ! command -v "$tool" &> /dev/null; then
            echo "Error: $tool is required to build a release (cargo: https://rustup.rs, npm: https://nodejs.org)." >&2
            exit 1
        fi
    done
    echo "▶ Building the frontend…"
    ( cd frontend && npm ci && npm run build )
    # The backend embeds `static/` at compile time — refuse to package a
    # binary whose UI is missing rather than shipping a broken release.
    if [[ ! -f static/dist/index.html ]]; then
        echo "Error: static/dist/index.html is missing after the frontend build." >&2
        exit 1
    fi
    echo "▶ Building the backend (release)…"
    cargo build --release --bin homevault
    mkdir -p installer/release
    ASSET="installer/release/homevault-${PKG_ARCH}.tar.gz"
    tar -czf "$ASSET" -C target/release homevault
    sha256sum "$ASSET" | awk '{print $1}' > "${ASSET}.sha256"
    echo "✓ $ASSET"
    echo "  sha256: $(cat "${ASSET}.sha256")"
    echo "  Publish it as a GitHub Release asset so the default install URL resolves."
    exit 0
fi

# ---------------------------------------------------------------------------
# Uninstall / rollback / dry-run
#
# `--uninstall` removes the application but KEEPS the data: an appliance that
# silently deleted accounts, settings and backups on an uninstall would be a
# trap. `--purge` (only valid with `--uninstall`) also removes the data root and
# the environment file, behind a typed confirmation.
#
# `--rollback` swaps back to `homevault.previous`, which every upgrade keeps.
# It does NOT restore the database: migrations are append-only, so rolling the
# binary back over a newer schema is refused at startup (see
# `DbError::SchemaTooNew`). The message says so and points at
# `data/backups/…pre-migration.db`.
#
# `--dry-run` (any mode) prints what would happen and changes nothing.
# ---------------------------------------------------------------------------

INSTALL_DIR_DEFAULT="/opt/homevault"
DATA_DIR_DEFAULT="/var/lib/homevault"
SERVICE_NAME_DEFAULT="homevault"
SERVICE_FILE_DEFAULT="/etc/systemd/system/homevault.service"
ENV_FILE_DEFAULT="/etc/homevault.env"

MODE="install"
PURGE=0
DRY_RUN=0
for arg in "$@"; do
    case "$arg" in
        --uninstall) MODE="uninstall" ;;
        --rollback)  MODE="rollback" ;;
        --purge)     PURGE=1 ;;
        --dry-run)   DRY_RUN=1 ;;
    esac
done

# Print-or-run: every destructive action goes through this, so --dry-run is a
# faithful preview rather than an approximation.
run() {
    if [[ "$DRY_RUN" -eq 1 ]]; then
        echo "  [dry-run] $*"
    else
        "$@"
    fi
}

if [[ "$MODE" == "uninstall" ]]; then
    # A dry run changes nothing, so it does not need root — that is what makes
    # it safe to preview before deciding.
    if [[ $EUID -ne 0 && "$DRY_RUN" -eq 0 ]]; then
        echo "This operation must be run as root:  sudo $0 --uninstall [--purge] [--dry-run]"
        exit 1
    fi
    if [[ "$PURGE" -eq 1 && "$DRY_RUN" -eq 0 ]]; then
        echo "This will permanently DELETE all HomeVault data:"
        echo "  $DATA_DIR_DEFAULT  (database, snapshots, app data, VMs)"
        echo "  $ENV_FILE_DEFAULT"
        read -r -p "Type PURGE to confirm: " CONFIRM || CONFIRM=""
        if [[ "$CONFIRM" != "PURGE" ]]; then
            echo "Aborted — nothing was deleted."
            exit 1
        fi
    fi

    echo "▶ Uninstalling HomeVault"
    run systemctl stop "$SERVICE_NAME_DEFAULT" 2>/dev/null || true
    run systemctl disable "$SERVICE_NAME_DEFAULT" 2>/dev/null || true
    run rm -f "$SERVICE_FILE_DEFAULT"
    run rm -f "${SERVICE_FILE_DEFAULT}.bak"
    run systemctl daemon-reload 2>/dev/null || true
    run rm -rf "$INSTALL_DIR_DEFAULT"

    if [[ "$PURGE" -eq 1 ]]; then
        run rm -rf "$DATA_DIR_DEFAULT"
        run rm -f "$ENV_FILE_DEFAULT"
        echo "  Data and environment file removed."
    else
        echo "  Data kept in $DATA_DIR_DEFAULT (use --purge to remove it too)."
    fi
    echo "✓ Done."
    exit 0
fi

if [[ "$MODE" == "rollback" ]]; then
    if [[ $EUID -ne 0 && "$DRY_RUN" -eq 0 ]]; then
        echo "This operation must be run as root:  sudo $0 --rollback [--dry-run]"
        exit 1
    fi
    PREV="$INSTALL_DIR_DEFAULT/homevault.previous"
    if [[ ! -x "$PREV" ]]; then
        echo "No previous version to roll back to ($PREV is missing)."
        echo "Every upgrade keeps one; a fresh install has none."
        exit 1
    fi

    echo "▶ Rolling back to the previous HomeVault build"
    run systemctl stop "$SERVICE_NAME_DEFAULT" 2>/dev/null || true
    run mv "$INSTALL_DIR_DEFAULT/homevault" "$INSTALL_DIR_DEFAULT/homevault.failed"
    run mv "$PREV" "$INSTALL_DIR_DEFAULT/homevault"
    run chmod +x "$INSTALL_DIR_DEFAULT/homevault"
    run systemctl start "$SERVICE_NAME_DEFAULT" 2>/dev/null || true

    cat <<'EOM'

  The binary is rolled back. The DATABASE is not.
  Migrations are append-only: if the newer build already migrated the database,
  this older binary will refuse to start ("database was written by a newer
  HomeVault"). In that case restore the snapshot taken before the upgrade:

    systemctl stop homevault
    cp /var/lib/homevault/data/backups/*pre-migration.db /var/lib/homevault/data/homevault.db
    rm -f /var/lib/homevault/data/homevault.db-wal /var/lib/homevault/data/homevault.db-shm
    systemctl start homevault
EOM
    echo "✓ Done."
    exit 0
fi

if [[ "$DRY_RUN" -eq 1 ]]; then
    # A dry run of the *install* path stops here: the installer is long and
    # touches apt, so a preview that then went ahead and did it would be worse
    # than no flag at all.
    echo "▶ Dry run — nothing will be changed."
    echo "  install dir : $INSTALL_DIR_DEFAULT"
    echo "  data dir    : $DATA_DIR_DEFAULT"
    echo "  service     : $SERVICE_FILE_DEFAULT"
    # `$1` may be a flag rather than an archive path — only show it when it is
    # an actual file the caller passed.
    ARCHIVE_ARG=""
    for a in "$@"; do
        case "$a" in --*) ;; *) [[ -f "$a" ]] && ARCHIVE_ARG="$a" ;; esac
    done
    echo "  archive     : ${ARCHIVE_ARG:-<downloaded from the GitHub release asset>}"
    echo ""
    echo "  Actions a real run would take: verify packages, download/extract the"
    echo "  binary to $INSTALL_DIR_DEFAULT, write $SERVICE_FILE_DEFAULT, enable and"
    echo "  start $SERVICE_NAME_DEFAULT."
    echo "  Use --uninstall [--purge] or --rollback to act on an existing install."
    exit 0
fi

# Root gate FIRST, before the log redirection further down: as a non-root
# user `/var/log` is not writable, so `tee -a` would die, and every later
# write would hit a broken pipe instead of printing this very message.
if [[ $EUID -ne 0 ]]; then
    echo "This script must be run with administrator (root) privileges."
    echo ""
    echo "Re-run it with:  curl -fsSL <install-url> | sudo bash"
    exit 1
fi

INSTALL_DIR="/opt/homevault"
DATA_DIR="/var/lib/homevault"
SERVICE_NAME="homevault"
SERVICE_FILE="/etc/systemd/system/homevault.service"
ENV_FILE="/etc/homevault.env"
PORT="8080"
HTTPS_PORT="8443"

# Pre-built application archive — one compressed `homevault` binary. Published
# per architecture on the project's GitHub Releases as `homevault-<arch>.tar.gz`
# (x86_64 / aarch64 / armv7l), exactly what `install.sh --package` produces.
# `HV_RELEASE_URL` overrides the whole URL (mirror, pinned version, …) and
# `HV_RELEASE_SHA256` optionally pins the archive's checksum.
RELEASE_BASE="${HV_RELEASE_BASE:-https://github.com/nicoleroux35/HomeVault/releases/latest/download}"
RELEASE_URL="${HV_RELEASE_URL:-}"
RELEASE_SHA256="${HV_RELEASE_SHA256:-}"

# Avoid apt/debconf ever popping an interactive prompt mid-install (a
# headless box or a `curl | sudo bash` pipe has no one to answer it, so a
# stray prompt would just hang forever instead of failing loudly).
export DEBIAN_FRONTEND=noninteractive
export NEEDRESTART_MODE=a

# Detect a real terminal *before* stdout gets tee'd to a log file below —
# once that pipe is in place `[ -t 1 ]` would always read false.
if [ -t 1 ]; then IS_TTY=1; else IS_TTY=0; fi

# Everything printed from here on (successes, warnings, raw apt/systemctl
# output alike) is also kept on disk — a failure a non-technical user can't
# make sense of on screen is still fully diagnosable by whoever they ask for
# help, without re-running the installer.
LOG_FILE="/var/log/homevault-install.log"
exec > >(tee -a "$LOG_FILE") 2>&1

# ---------------------------------------------------------------------------
# Small output helpers — a plain terminal (no color codes) falls back
# gracefully.
# ---------------------------------------------------------------------------
if [ "$IS_TTY" = "1" ]; then
    C_RESET='\033[0m'; C_BOLD='\033[1m'; C_DIM='\033[2m'
    C_GREEN='\033[32m'; C_YELLOW='\033[33m'; C_RED='\033[31m'; C_CYAN='\033[36m'
else
    C_RESET=''; C_BOLD=''; C_DIM=''; C_GREEN=''; C_YELLOW=''; C_RED=''; C_CYAN=''
fi

STEP_TOTAL=5
STEP_NUM=0
step() {
    STEP_NUM=$((STEP_NUM + 1))
    echo ""
    echo -e "${C_BOLD}${C_CYAN}▶ Step ${STEP_NUM}/${STEP_TOTAL} — $1${C_RESET}"
}
ok()   { echo -e "  ${C_GREEN}✓${C_RESET} $1"; }
warn() { echo -e "  ${C_YELLOW}⚠${C_RESET} $1"; }
err()  { echo -e "  ${C_RED}✗${C_RESET} $1"; }
info() { echo -e "  ${C_DIM}$1${C_RESET}"; }

# Catch-all for anything that fails in a way none of the checks below
# anticipated (network drop mid-`apt-get`, a corrupt package cache...) — with
# `set -e`, that would otherwise stop the script on a raw, unstyled shell
# error with no pointer to the log this non-technical audience wouldn't
# think to look for on their own.
on_unexpected_error() {
    local line="$1"
    echo ""
    err "Installation stopped unexpectedly (line ${line})."
    echo -e "  Full log: ${C_BOLD}${LOG_FILE}${C_RESET}"
}
trap 'on_unexpected_error $LINENO' ERR

echo -e "${C_BOLD}"
echo "╔══════════════════════════════════════════════╗"
echo "║            HomeVault Installation            ║"
echo "╚══════════════════════════════════════════════╝"
echo -e "${C_RESET}"

# ---------------------------------------------------------------------------
# Step 1 — pre-flight checks (fail fast, before touching anything or
# spending 5-10 minutes downloading packages: sudo, OS and above all the CPU
# architecture get checked up front).
# ---------------------------------------------------------------------------
step "Verifying architecture compatibility"

# Root was already gated before the log redirection above (a second check
# here would be unreachable); this only confirms it in the step listing.
ok "Running with administrator privileges (sudo)"

if grep -qE '(debian|ubuntu)' /etc/os-release 2>/dev/null; then
    OS_NAME=$(. /etc/os-release && echo "$PRETTY_NAME")
    ok "Compatible system: $OS_NAME"
else
    err "Unsupported system. HomeVault requires Debian 10+ or Ubuntu 20.04+."
    exit 1
fi

# The CPU architecture gate. Only these three targets ship a pre-built
# binary, so a mismatched board is caught here — before any package is
# downloaded — instead of ending in a confusing "cannot execute binary".
ARCH=$(uname -m)
if [[ "$ARCH" =~ ^(x86_64|aarch64|armv7l)$ ]]; then
    ok "Compatible architecture: $ARCH"
else
    err "Unsupported architecture: $ARCH (x86_64, aarch64 or armv7l required)"
    exit 1
fi

FREE_KB=$(df --output=avail / 2>/dev/null | tail -1 | tr -d '[:space:]')
if [[ -n "$FREE_KB" ]] && [[ "$FREE_KB" -lt 2097152 ]]; then
    warn "Less than 2 GB of free disk space on / — installing the packages and the application may fail."
else
    ok "Enough free disk space"
fi

# ---------------------------------------------------------------------------
# Step 2 — verify that everything HomeVault needs is installed, installing
# the missing parts. This covers both the on-disk folders and every system
# package the apps shell out to at runtime.
# ---------------------------------------------------------------------------
step "Verifying and installing system packages"

# Folders HomeVault needs on disk.
#
# No dedicated service account: HomeVault runs as root (see the systemd unit
# below — no User=/Group=). It manages the host itself (RAID, ZFS,
# Samba/NFS/vsftpd, Docker, disk encryption, real Linux user/group
# accounts...) deeply enough that a scoped unprivileged account plus a
# generated sudoers file was mostly re-granting root piecemeal anyway, and —
# critically — bootstrapping that sudoers file always needed root in the
# first place, which a freshly `useradd --system`'d account never had until
# *after* the Setup Wizard ran, so the very first "create the admin's real
# Linux account + homevault_admin group" attempt silently failed on every
# fresh install. Running as root sidesteps that chicken-and-egg problem
# entirely: every `sudo` call already in the codebase (RAID/mdadm, ZFS, VPN,
# Docker, journalctl, useradd/groupadd, hostnamectl, ...) succeeds against
# the *default* Debian/Ubuntu sudoers rule `root ALL=(ALL:ALL) ALL` — no
# per-install sudoers file to generate at all. The only account creation
# from now on happens inside the Setup Wizard itself, for the specific admin
# the user names there.
mkdir -p "$INSTALL_DIR" "$DATA_DIR" "/etc/homevault"
# Everything lives under /var/lib/homevault:
#   data/       SQLite DB + JSON settings
#   compose/    Docker Compose projects (one dir per app)
#   apps/       persistent data of installed Docker apps
#   rescue/     file-recovery working data
#   syncthing/  Syncthing configuration
#   logs/       HomeVault's own rotating application log
#   quarantine/ antivirus quarantine
#   vm/isos/    Virtual Machines install media
# (tls/ and trash/ are created on demand by the application.)
# Personal user home folders live under /home/homevault.
for sub in data compose apps rescue syncthing logs quarantine vm/isos; do
    mkdir -p "$DATA_DIR/$sub"
done
mkdir -p "/home/homevault"
# Mount point base directories so the first RAID/pool/encrypted/rclone item
# doesn't fail trying to create a subdirectory under root-owned /mnt/.
mkdir -p /mnt/raid /mnt/pools /mnt/encrypted /mnt/rclone
ok "Directories created under $INSTALL_DIR and $DATA_DIR"

# Dependency checker — is this Debian package actually installed and usable?
# Reads dpkg's status field instead of `dpkg -s`: a package left in the
# `config-files` state (removed, configuration kept) still answers positively
# to `dpkg -s`, but it is NOT installed — the installer would then skip the
# real install and ship the feature dead. Only `install ok installed` counts.
pkg_installed() {
    dpkg-query -W -f='${db:Status-Status}' "$1" 2>/dev/null | grep -qx installed
}

# Does the current apt configuration actually offer this package for
# install (as opposed to it just being present already)? Reads the
# *Candidate* line of `apt-cache policy`. `LC_ALL=C` forces the label to the
# canonical English spelling — without it, a localized system (French prints
# `Candidat :`, for example) would make every such probe fail and the
# installer would mis-detect availability on those machines.
pkg_available() {
    LC_ALL=C apt-cache policy "$1" 2>/dev/null | grep -q "Candidate: [^(]"
}

# Install the packages in "$@" but ONLY the missing ones — never ask apt to
# re-install something already present (idempotent re-runs and upgrades stay
# fast, quiet, and offline-safe for the already-satisfied part). `$1` is the
# human label, `$2` the severity:
#   required -> a failed apt-get aborts the whole install;
#   optional -> degrades to a warning, the feature just stays disabled.
# apt output is kept in the journal (not swallowed) so a failure stays
# diagnosable from $LOG_FILE.
#
# `apt-get install a b c` is all-or-nothing: if ANY requested package is
# missing from this release's repos (dropped or renamed — e.g. `p7zip-full`
# → `7zip`), apt rejects the *entire* batch and nothing installs. So a
# failed combined install retries each package on its own: for an optional
# group the still-available members land and only the genuinely unavailable
# ones are reported; for a required group the failure stays fatal.
install_packages() {
    local label="$1" mode="$2"
    shift 2
    local missing=()
    local already=()
    for pkg in "$@"; do
        if pkg_installed "$pkg"; then
            already+=("$pkg")
        else
            missing+=("$pkg")
        fi
    done
    if [[ ${#missing[@]} -eq 0 ]]; then
        ok "$label (already installed)"
        return 0
    fi
    if apt-get install -y --no-install-recommends -qq "${missing[@]}"; then
        ok "$label"
        return 0
    fi
    if [[ "$mode" == "required" ]]; then
        err "Failed to install $label: ${missing[*]}"
        return 1
    fi
    local failed=()
    local installed_any=0
    for pkg in "${missing[@]}"; do
        if apt-get install -y --no-install-recommends -qq "$pkg"; then
            installed_any=1
        else
            failed+=("$pkg")
        fi
    done
    if [[ "$installed_any" -eq 1 ]] && [[ ${#failed[@]} -eq 0 ]]; then
        ok "$label"
    else
        warn "$label — packages unavailable: ${failed[*]} (corresponding feature disabled)"
    fi
}

install_optional() {
    local pkg="$1"
    local name="$2"
    install_packages "$name" optional "$pkg"
}

info "Checking internet connectivity..."
if curl -fsS --max-time 5 http://deb.debian.org &> /dev/null \
    || curl -fsS --max-time 5 http://archive.ubuntu.com &> /dev/null; then
    ok "Internet connection available"
else
    err "No internet connection detected."
    echo -e "  Installing packages requires internet. Check the cable/Wi-Fi"
    echo -e "  of this machine, then re-run the installation."
    exit 1
fi

info "Updating the package list..."
apt-get update -qq

info "Base tools (build toolchain, databases)..."
# `git` backs the Docker App Catalog (community stores are cloned from git
# repos — src/app_catalog/store.rs) and `python3` the Linux-user/PAM login
# path (src/auth/service.rs). Both are Priority-standard and always present
# in the base repos, so they ride along the required group.
# `sudo` is the workhorse of host management (180+ call sites across the
# app). Even though HomeVault itself runs as root, the binary must exist —
# Ubuntu always ships it, but a minimal Debian (netinst without the standard
# taskset, containers, small VPS images) can genuinely lack it, and every
# feature that shells out would break at once.
install_packages "Base tools (build toolchain, databases)" required \
    build-essential pkg-config libsqlite3-dev sqlite3 wget curl ca-certificates git python3 sudo

info "Storage and monitoring..."
# `dmidecode` (Dashboard: motherboard/BIOS/RAM details), `ethtool` (Network:
# link-speed diagnostics) and `iputils-ping` (Network diagnostics) are small,
# always available in the base repos, and each backs a frozen page that
# degrades to "unknown" without them — installed unconditionally here rather
# than left to a runtime surprise.
install_packages "Storage and monitoring tools" required \
    acl quota smartmontools mdadm rsync gnupg iproute2 util-linux \
    lsb-release lm-sensors udisks2 udiskie dmidecode ethtool iputils-ping

info "Media tools..."
install_packages "Media player (FFmpeg)" required ffmpeg

info "File archives and hardware tools..."
# `unzip`/`7z` back the Explorer "extract" action and `zip` the Browser
# "compress to .zip"; `lspci` (pciutils) the Dashboard's GPU card; `hostname`
# this script's own `hostname -I` LAN-IP fallback. Each already degrades with
# a clear `which`-checked message or an empty card when absent (see
# explorer.rs / browser.rs / utils/proc.rs), so they live in one optional,
# non-fatal group.
# `7z` is a single binary with TWO package names depending on the release:
# `p7zip-full` (Debian ≤11 / Ubuntu ≤22.04) was dropped upstream over
# unfixed CVE-2016-{2334,9298} and replaced by `7zip` on Debian 12+ /
# Ubuntu 23.04+ — both still ship /usr/bin/7z (see src/dependencies.rs).
# Request the one this host's repos actually carry so the group never fails
# wholesale (apt rejects a batch containing an unavailable package).
install_packages "File archive and hardware tools (unzip/zip/7z, lspci, hostname)" optional \
    unzip zip pciutils hostname
if pkg_available 7zip; then
    install_optional 7zip "7-Zip (7z archives)"
else
    install_optional p7zip-full "7-Zip (7z archives)"
fi

install_optional aria2 "aria2 (multi-protocol downloader)"

# Security tools (antivirus scanning, firewall). Non-fatal like the rest of
# install_optional: HomeVault must still run fine if these are declined, and
# detects their absence at runtime to degrade the UI instead of crashing.
# Deliberately clamav + clamav-freshclam, NOT clamav-daemon: the real-time
# clamd scan daemon is out of scope (on-demand/scheduled scanning only, to
# fit the low-power hardware this targets).
install_optional clamav "ClamAV (antivirus)"
install_optional clamav-freshclam "ClamAV (virus definition updates)"
install_optional ufw "ufw (firewall)"

# Disk encryption (Storage app — LUKS2 volumes). Small, always in the base
# repos; the Storage app hides the "Encrypted volume" flow without it.
install_optional cryptsetup "cryptsetup (LUKS2 encrypted volumes)"

# Sharing services
install_optional samba "Samba (SMB/CIFS)"
install_optional nfs-kernel-server "NFS server"
install_optional vsftpd "FTP server (vsftpd)"
# WebDAV uses a built-in Python server — no external package needed.
# SFTP runs a *second*, dedicated sshd instance on its own port (see
# src/sftp.rs) — never the system's own sshd/SSH access, but it still needs
# the openssh-server package installed for the /usr/sbin/sshd binary and
# host keys under /etc/ssh.
install_optional openssh-server "OpenSSH Server (for SFTP)"

# Network discovery (mDNS / Zeroconf) — makes HomeVault reachable on
# `homevault.local` and shows its shares in the Finder / file managers.
# Strictly link-local traffic (224.0.0.251:5353), no outgoing calls; this is
# the default behaviour of every NAS (OpenMediaVault, TrueNAS, Synology...).
# `libnss-mdns` additionally lets this machine resolve the other `.local`
# names on the network.
install_optional avahi-daemon "Avahi (mDNS: homevault.local + share discovery)"
install_optional libnss-mdns "libnss-mdns (.local name resolution)"

# VPN app — WireGuard (`wg`/`wg-quick`) and OpenVPN client. Each connection
# type degrades independently at runtime (src/vpn.rs), so a host that only
# wants one of them still works.
install_optional wireguard-tools "WireGuard (VPN)"
install_optional openvpn "OpenVPN (VPN)"

# Rescue app — data-recovery tooling. `gddrescue` provides /usr/bin/ddrescue;
# `testdisk` provides both /usr/bin/testdisk and /usr/bin/photorec. Without
# them the Rescue app's clone / partition-repair / file-carving flows are
# unavailable (they still fail cleanly — see src/rescue/service.rs).
install_optional gddrescue "GNU ddrescue (Rescue: disk cloning)"
install_optional testdisk "TestDisk + PhotoRec (Rescue: repair/recovery)"

# Backup & sync — restic (Backup app) and syncthing (Syncthing app) are
# genuine system packages this app drives directly, not Docker catalog apps.
install_optional restic "restic (Backup)"
install_optional syncthing "Syncthing (P2P synchronization)"

# ZFS and Btrfs storage-pool support (Storage app). Installed outright as
# options — an earlier `command -v zpool`/`command -v btrfs` guard was a
# chicken-and-egg that skipped the package exactly when it was missing, so on
# a fresh host the Storage app's ZFS/Btrfs pools were never available.
# `install_optional` degrades to a warning if a package is unavailable (e.g.
# Debian without the contrib component for zfsutils-linux), leaving that pool
# type simply hidden from the UI as designed (see src/dependencies.rs).
install_optional zfsutils-linux "ZFS tools"
install_optional btrfs-progs "Btrfs tools"

# Docker
if ! command -v docker &> /dev/null; then
    install_optional docker.io "Docker"
else
    ok "Docker (already installed)"
fi

# Docker Compose plugin is required for the Compose tab (`docker compose
# ...`). Debian/Ubuntu default repos never carry `docker-compose-plugin`
# (only Docker's own apt repo does), so without this, installing it below
# would silently fail via install_optional's fallback and `docker compose`
# would never become available. Add Docker's official repo first, but only if
# the package isn't already resolvable from what's currently configured.
if command -v docker &> /dev/null; then
    if ! pkg_available docker-compose-plugin; then
        info "Adding the official Docker repository (docker-compose-plugin not in the current repositories)..."
        . /etc/os-release
        install -m 0755 -d /etc/apt/keyrings
        curl -fsSL "https://download.docker.com/linux/${ID}/gpg" | gpg --dearmor -o /etc/apt/keyrings/docker.gpg
        chmod a+r /etc/apt/keyrings/docker.gpg
        echo "deb [arch=$(dpkg --print-architecture) signed-by=/etc/apt/keyrings/docker.gpg] https://download.docker.com/linux/${ID} ${VERSION_CODENAME} stable" \
            > /etc/apt/sources.list.d/docker.list
        apt-get update -qq
    fi
    install_optional docker-compose-plugin "Docker Compose"
fi

# Incus — container + VM hypervisor. Deliberately best-effort like Docker/
# Ollama: the Virtual Machines app degrades cleanly without it.
# qemu-system-modules-spice is REQUIRED for any VM to boot on Debian/Ubuntu
# (Incus always passes -spice to qemu); ovmf provides the UEFI + Secure Boot
# firmware Windows needs; swtpm + swtpm-tools the software TPM 2.0 (Win11).
#
# Incus is NOT in every supported base repository, so where it is missing the
# same pattern as the Docker repository above applies (keyring + sources
# entry + update, best-effort):
#   Debian 13+, Ubuntu 24.04+ → in the base repos, nothing to add;
#   Debian 12                 → bookworm-backports only;
#   Ubuntu 20.04 / 22.04      → not packaged: official Zabbly repository.
ensure_incus_repo() {
    if pkg_available incus; then
        return 0
    fi
    . /etc/os-release
    info "Incus not in the current repositories — adding one (VM stack)..."
    if grep -qi '^ID=debian' /etc/os-release; then
        # Backports carry Incus on stable Debian releases.
        if ! grep -qs "${VERSION_CODENAME}-backports" /etc/apt/sources.list \
                /etc/apt/sources.list.d/* 2>/dev/null; then
            echo "deb http://deb.debian.org/debian ${VERSION_CODENAME}-backports main" \
                > /etc/apt/sources.list.d/incus-backports.list
        fi
    else
        install -m 0755 -d /etc/apt/keyrings
        if ! curl -fsSL https://pkgs.zabbly.com/key.asc -o /etc/apt/keyrings/zabbly.asc \
                || ! chmod a+r /etc/apt/keyrings/zabbly.asc; then
            warn "Could not fetch the Zabbly repository key — VMs will stay disabled"
            return 0
        fi
        echo "deb [signed-by=/etc/apt/keyrings/zabbly.asc] https://pkgs.zabbly.com/incus/stable ${VERSION_CODENAME} main" \
            > /etc/apt/sources.list.d/zabbly-incus-stable.list
    fi
    apt-get update -qq
}

# A freshly installed Incus daemon starts EMPTY: no storage pool and a
# `default` profile without a NIC — the very first vm/create would fail with
# "No storage pool found". The VM app inherits the default profile's NIC
# (src/incus.rs), so provisioning that baseline non-interactively here makes
# VMs work out of the box. Idempotent: an initialized daemon is detected via
# its storage pool list and left untouched; failures are non-fatal (the VM
# app then shows its "Incus unavailable" state until init is run).
init_incus_daemon() {
    systemctl is-active --quiet incus || return 0
    if incus storage list --format csv 2>/dev/null | grep -q .; then
        ok "Incus daemon already initialized"
        return 0
    fi
    local incus_ready=0
    for _ in $(seq 1 10); do
        if incus admin init --auto >/dev/null 2>&1; then
            incus_ready=1
            break
        fi
        sleep 1
    done
    if [[ "$incus_ready" -eq 1 ]]; then
        ok "Incus daemon initialized (default storage pool + incusbr0 bridge)"
    else
        warn "Incus daemon not initialized — run 'incus admin init --auto' before creating VMs"
    fi
}

install_incus() {
    if command -v incus &> /dev/null; then
        ok "Incus (containers + VMs) already installed"
        # Upgrade path: make sure the VM boot modules land on an existing
        # Incus host too. `qemu-system-modules-spice` is REQUIRED for any VM
        # to boot (Incus always passes -spice to qemu); swtpm-tools provides
        # `swtpm_setup`, which initialises the emulated TPM state a Windows 11
        # VM needs before it can start.
        install_packages "Incus: VM modules (QEMU SPICE, TPM)" optional qemu-system-modules-spice swtpm-tools
    else
        # First install, in dependency order: incus FIRST — it depends on
        # qemu-system-x86 (>= 1:8.0) and pulls a consistent QEMU from the
        # backports/Zabbly repo added above (bookworm stable only has 7.2,
        # which would conflict). swtpm/ovmf are version-independent.
        ensure_incus_repo
        install_packages "Incus (containers + VMs)" optional \
            incus swtpm swtpm-tools ovmf
        # Then the QEMU pieces that incus's dependencies may not cover:
        # qemu-system-modules-spice is REQUIRED for any VM to boot when QEMU
        # is >= 8.1 (Incus always passes -spice to qemu); virtiofsd does not
        # exist on Debian 12 and is only requested where the repos carry it
        # (same pattern as the 7zip rename above) — the app never requires it.
        install_packages "Incus: QEMU + SPICE VM modules" optional \
            qemu-system-x86 qemu-system-modules-spice
        if pkg_available virtiofsd; then
            install_optional virtiofsd "virtiofsd (VM shared folders)"
        fi
    fi

    if command -v incus &> /dev/null; then
        systemctl enable --now incus &> /dev/null || true
        init_incus_daemon
    else
        warn "Incus not installed — virtual machines will be disabled"
    fi
}
install_incus

# SNMP monitoring (Settings > Monitoring & Storage has a full card for it).
# `snmpd` is the local agent; `snmp` provides the client tools (snmpget /
# snmpwalk) that src/snmp.rs actually calls to read the OIDs.
install_optional snmpd "SNMP agent"
install_optional snmp "SNMP client tools (snmpget/snmpwalk)"

# The Debian/Ubuntu default `snmpd.conf` restricts `rocommunity public` to the
# `systemonly` view (`.1.3.6.1.2.1.1` + `.1.3.6.1.2.1.25.1`), which makes the
# HomeVault SNMP card's interface and storage sections empty (`snmpwalk
# 1.3.6.1.2.1.2.2.1` and `1.3.6.1.2.1.25.2.3.1` return nothing). Extend the
# view via a drop-in — the same file the installer drops here, see
# `src/snmp.rs::get_interface_stats`/`get_storage_stats`. Views are additive;
# `agentaddress 127.0.0.1,[::1]` keeps it localhost-only.
if command -v snmpd &> /dev/null; then
    mkdir -p /etc/snmp/snmpd.conf.d
    cat > /etc/snmp/snmpd.conf.d/homevault.conf <<'EOF'
view systemonly included .1.3.6.1.2.1.2
view systemonly included .1.3.6.1.2.1.25.2
EOF
    systemctl restart snmpd &> /dev/null || true
fi

# lm-sensors is already part of the required "Storage and monitoring" group
# above — the Dashboard reads `sensors` at runtime (see src/dependencies.rs),
# so it never needs a second, guard-based install here.

# UPS monitoring (Settings > Monitoring & Storage has a full card for it).
# `upsc` ships inside `nut-client`, which is a dependency of the `nut`
# metapackage — so the gate below MUST NOT test `command -v upsc` (that
# would only be true after NUT is already installed: chicken-and-egg, and
# the card would never be offered on a fresh system). `nut` is offered
# unconditionally like the SNMP packages above.
install_optional nut "NUT (UPS)"

# Configure NUT for automatic USB UPS detection:
#   * MODE=standalone so upsd (nut-server) runs at all — the packaged default
#     is MODE=none, which makes upsd refuse to start;
#   * one generic `usbhid-ups` section with `port = auto`, which scans the USB
#     buses and attaches to whichever supported UPS is present — no per-model
#     vendor/product id is ever hardcoded, and the driver's `Restart=always`
#     retry loop picks up a UPS plugged in later.
# Never clobber an existing NUT config: only the MODE line is adjusted, and the
# section is added only when ups.conf has no section yet.
if command -v upsd &> /dev/null; then
    if ! grep -qE '^[[:space:]]*MODE[[:space:]]*=[[:space:]]*(standalone|netserver)' /etc/nut/nut.conf 2>/dev/null; then
        if grep -qE '^[[:space:]]*MODE[[:space:]]*=' /etc/nut/nut.conf 2>/dev/null; then
            sed -i 's/^[[:space:]]*MODE[[:space:]]*=.*/MODE=standalone/' /etc/nut/nut.conf
        else
            echo 'MODE=standalone' >> /etc/nut/nut.conf
        fi
    fi
    if ! grep -qE '^[[:space:]]*\[' /etc/nut/ups.conf 2>/dev/null; then
        cat >> /etc/nut/ups.conf <<'EOF'

# HomeVault — auto-detect a USB (HID) UPS. `port = auto` attaches to the first
# supported UPS on the USB buses; the driver retries until one appears.
[ups]
    driver = usbhid-ups
    port = auto
    desc = "HomeVault auto-detected USB UPS"
EOF
    fi
    systemctl enable --now nut-driver.target nut-server &> /dev/null || true
fi

if ! command -v sendmail &> /dev/null && ! command -v msmtp &> /dev/null && ! command -v mutt &> /dev/null; then
    install_optional mutt "Mail client (notifications)"
fi

# rclone, FUSE, NTFS and exFAT — same chicken-and-egg as ZFS/Btrfs above: the
# old `command -v` guards only matched once the tool was already installed, so
# on a fresh host rclone mount (FUSE3) and NTFS/exFAT USB drives were never
# available. All are small and in the base repos; installed as options.
install_optional rclone "rclone (cloud synchronization)"
# FUSE3 only — and that is enough: the fuse3 package ships a
# `/bin/fusermount -> fusermount3` compatibility symlink (Replaces: fuse) that
# the app's unmount path calls by its FUSE2 name (src/handlers/rclone.rs),
# while the real fuse2 package `Breaks: fuse` unversioned against fuse3 and
# cannot coinstall with it on Debian 12. No second FUSE generation here.
install_optional fuse3 "FUSE3 (rclone mounting + fusermount)"
install_optional ntfs-3g "NTFS-3G"
install_optional exfat-fuse "exFAT (USB drives / SD cards)"

# ---------------------------------------------------------------------------
# AI Assistant — Ollama runs the local language model that powers the Chat
# app. It is NOT an apt package, so it can't go through install_optional:
# use its official installer, then enable the daemon. We deliberately do
# NOT download any model here — the model is the user's choice (size vs.
# quality depends on the host's RAM/GPU) and is picked from Settings > AI,
# which proposes five models ranked for this machine and pulls the selected
# one.
#
# Why Ollama and not llama.cpp / LocalAI / vLLM:
#   - one static Go binary per architecture, no Python runtime on the
#     appliance (LocalAI/vLLM pull a whole Python stack);
#   - a model registry: `pull` downloads the right quantisation by name, so
#     the UI can offer a list instead of asking the operator to fetch GGUF
#     files (llama.cpp server needs the file and the tokenizer by hand);
#   - streaming + an [OI]-compatible /v1 surface, which is what the SSE
#     endpoint in src/handlers/chat.rs consumes;
#   - automatic CPU/GPU split (CUDA, ROCm, Apple Metal) without flags.
# vLLM is GPU-only and needs far more than the 1 GB floor HomeVault targets.
#
# Ollama publishes amd64 and arm64 builds only — no 32-bit ARM. On armv7l
# the install is skipped on purpose: a half-working daemon would look like a
# bug in the app. The Chat then shows its "AI offline" state.
# Everything here is best-effort — HomeVault runs fine without Ollama.
# ---------------------------------------------------------------------------
install_ollama() {
    case "$(uname -m)" in
        armv7l|armv6l)
            warn "Ollama has no 32-bit ARM build — the AI assistant stays offline on $(uname -m)."
            info "Alternative: run an [OI]-compatible server (llama.cpp) elsewhere and point the app at it."
            return
            ;;
    esac

    if command -v ollama &> /dev/null; then
        ok "Ollama (AI assistant) already installed"
    elif curl -fsSL https://ollama.com/install.sh 2>/dev/null | sh &> /dev/null \
        && command -v ollama &> /dev/null; then
        ok "Ollama (AI assistant) installed"
    else
        warn "Ollama not installed — the AI assistant (Chat) will be disabled. To add it later: curl -fsSL https://ollama.com/install.sh | sh"
        return
    fi

    systemctl enable --now ollama &> /dev/null || true

    # Post-start check, per Ollama's own Linux docs
    # (docs.ollama.com/linux): `systemctl status ollama` proves the unit
    # started, but the Chat app talks to the HTTP API, so also poll that
    # port 11434 really answers. The daemon can take a few seconds to bind
    # it, hence the retry loop — only a persistent failure is reported, and
    # never fatally (best-effort like the rest of this function).
    local ollama_ready=0
    for _ in $(seq 1 10); do
        if curl -fsS --max-time 2 http://127.0.0.1:11434/ &> /dev/null; then
            ollama_ready=1
            break
        fi
        sleep 1
    done
    if [[ "$ollama_ready" -eq 1 ]]; then
        ok "Ollama daemon responding on port 11434"
    else
        warn "Ollama installed but port 11434 does not answer — check: systemctl status ollama"
    fi

    info "No AI model is downloaded automatically — pick one of the five models Settings > AI ranks for this machine."
}
install_ollama

# Configure automatic mounting of external drives (USB, SD cards) the same
# way a desktop does: udisks2 + udiskie, a systemd unit, and plugdev/storage
# group membership. Inlined so this single script needs no sibling file;
# udisks2 + udiskie are already in the required package group above, so on a
# normal run nothing is installed here. Best-effort like the rest: a failing
# daemon just skips the feature instead of aborting the install.
setup_automount() {
    if ! command -v udiskie &> /dev/null; then
        warn "Automount skipped — udiskie not installed"
        return 0
    fi
    mkdir -p /etc/homevault
    cat > /etc/homevault/udiskie.conf <<'EOF'
# udiskie automount configuration for HomeVault
# udiskie uses UDisks2 (not the obsolete UDisks v1 used by udisks-glue)

[General]
# Automatically mount newly inserted devices
automount = true

# Use a file browser to open the mounted directory
use_notify_desktop = false

# Do not mount internal devices (SATA/NVMe system disks)
ignore_internal = true

[Mount Options]
# Default mount options applied to all filesystems
default_options = nosuid,nodev,noexec,noatime

# Per-filesystem mount options (appended to defaults)
ntfs_options = rw,uid=1000,gid=1000
vfat_options = rw,uid=1000,gid=1000,utf8=1
exfat_options = rw,uid=1000,gid=1000
ext4_options = rw,noatime
xfs_options = rw,noatime
btrfs_options = rw,noatime
EOF
    cat > /etc/systemd/system/udiskie.service <<'EOF'
[Unit]
Description=udiskie automount daemon (UDisks2)
After=udisks2.service
Documentation=man:udiskie(1)

[Service]
Type=simple
ExecStart=/usr/bin/udiskie -c /etc/homevault/udiskie.conf -n
Restart=on-failure
RestartSec=5

# Environment: tell udiskie to mount under /media/<user>
Environment=UDISKS_USE_UDisks2=1
EOF
    systemctl daemon-reload
    systemctl enable udiskie.service &> /dev/null || true
    systemctl start udiskie.service &> /dev/null || true
    if id "homevault" &>/dev/null; then
        usermod -a -G plugdev,storage,cdrom "homevault" 2>/dev/null || true
    fi
    # Also grant the logged-in desktop user (first `who` session) device
    # access — the behaviour the old standalone automount helper had.
    local invoker=""
    invoker=$(who 2>/dev/null | head -n 1 | awk '{print $1}' || true)
    if [[ -n "$invoker" ]]; then
        usermod -a -G plugdev,storage,cdrom "$invoker" 2>/dev/null || true
    fi
    ok "Automatic external-drive mounting configured"
}

info "Cleaning the apt cache..."
apt-get clean
rm -rf /var/lib/apt/lists/*

# ---------------------------------------------------------------------------
# Step 3 — download the compressed HomeVault application, extract it to
# /opt/homevault and register it as a systemd service.
# ---------------------------------------------------------------------------
step "Downloading and installing the HomeVault application"

# Resolve the archive: use a local file if one was passed as `$1`, otherwise
# download the published release archive.
PACKAGE=""
if [[ $# -ge 1 ]] && [[ -f "$1" ]]; then
    PACKAGE="$1"
    info "Using the provided archive: $PACKAGE"
fi
if [[ -z "$PACKAGE" ]]; then
    # Default to the per-architecture release asset for this host.
    if [[ -z "$RELEASE_URL" ]]; then
        RELEASE_URL="$RELEASE_BASE/homevault-${ARCH}.tar.gz"
    fi
    TMP_ARCHIVE=$(mktemp /tmp/homevault.XXXXXX.tar.gz)
    PACKAGE="$TMP_ARCHIVE"
    info "Downloading HomeVault ($ARCH) from $RELEASE_URL ..."
    if ! curl -fL --retry 3 --retry-delay 2 -o "$PACKAGE" "$RELEASE_URL"; then
        err "Failed to download HomeVault from $RELEASE_URL"
        echo -e "  Check the URL, or pass a local archive: ${C_BOLD}sudo $0 /path/to/homevault.tar.gz${C_RESET}"
        echo -e "  Or point at another source: ${C_BOLD}HV_RELEASE_URL=<url>${C_RESET}"
        exit 1
    fi
    ok "Downloaded: $PACKAGE"
fi

if ! tar -tzf "$PACKAGE" &> /dev/null; then
    err "Archive $PACKAGE is corrupt or incomplete (interrupted download?)."
    echo -e "  Re-download it and re-run the installation."
    exit 1
fi
ok "Archive found and valid: $PACKAGE"

# Optional integrity check — the publisher can ship a `<asset>.sha256` sidecar
# (see `install.sh --package`) and the admin passes its digest here. Verified
# before anything is extracted into /opt/homevault.
if [[ -n "$RELEASE_SHA256" ]]; then
    ACTUAL_SHA=$(sha256sum "$PACKAGE" | awk '{print $1}')
    if [[ "$ACTUAL_SHA" != "$RELEASE_SHA256" ]]; then
        err "SHA-256 mismatch for the downloaded archive."
        echo -e "  expected: $RELEASE_SHA256"
        echo -e "  actual:   $ACTUAL_SHA"
        rm -f "$PACKAGE"
        exit 1
    fi
    ok "Archive checksum verified"
fi

UPGRADE=0
if [[ -f "$SERVICE_FILE" ]]; then
    UPGRADE=1
    info "Existing installation detected — updating the application (your data is kept)"
    # Keep the running binary so `--rollback` has something to go back to. Only
    # one previous build is retained: more would just accumulate disk usage
    # without a way to choose between them.
    if [[ -x "$INSTALL_DIR/homevault" ]]; then
        cp -f "$INSTALL_DIR/homevault" "$INSTALL_DIR/homevault.previous"
        ok "Previous build kept for rollback ($INSTALL_DIR/homevault.previous)"
    fi
fi

tar -xzf "$PACKAGE" -C "$INSTALL_DIR"
chmod +x "$INSTALL_DIR/homevault"
ok "Application extracted to $INSTALL_DIR"

# A downloaded archive was only needed at install time — free the disk space.
if [[ -n "${TMP_ARCHIVE:-}" ]] && [[ "$PACKAGE" == "$TMP_ARCHIVE" ]]; then
    rm -f "$PACKAGE"
fi

if [[ -f "$SERVICE_FILE" ]]; then
    cp "$SERVICE_FILE" "${SERVICE_FILE}.bak" 2>/dev/null || true
fi

# Systemd unit — written inline so this single script is self-contained (there
# is no sibling `homevault.service` to fall back to). No dedicated service
# account: HomeVault runs as root because it manages the host itself; see the
# note in Step 2 above.
cat > "$SERVICE_FILE" <<EOF
[Unit]
Description=HomeVault Server
After=network.target

[Service]
Type=simple
WorkingDirectory=$INSTALL_DIR
ExecStart=$INSTALL_DIR/homevault
Restart=on-failure
RestartSec=5
Environment="PORT=$PORT"
Environment="DATA_DIRECTORY=$DATA_DIR"
# Built-in HTTPS is opt-in (Settings > Security > HTTPS/TLS, or set
# TLS_ENABLED=true here / in /etc/homevault.env). When enabled, HomeVault
# adds a TLS listener on HTTPS_PORT (default $HTTPS_PORT) alongside HTTP.
# Plain HTTP is then redirected to it by default (TLS_REDIRECT=0 keeps both
# listeners usable), so the admin password is never served in clear.
#Environment="TLS_ENABLED=true"
#Environment="HTTPS_PORT=$HTTPS_PORT"
#Environment="TLS_REDIRECT=0"
EnvironmentFile=-$ENV_FILE

# Resource ceiling: HomeVault targets low-power boards and modest mini-PCs,
# often shared with other host services (Samba, Docker apps). These give the
# process room for legitimate heavy work (large Restic backups, Explorer
# archive operations) while still capping a runaway feature or memory leak
# instead of letting it starve the whole machine. MemoryHigh is a soft
# throttle (reclaim pressure, no kill); MemoryMax is the hard cap. Tune both
# against real \`systemd-cgtop\`/\`systemctl status\` measurements for your
# hardware before relying on them in production — these are conservative
# starting points, not measured limits.
MemoryHigh=1536M
MemoryMax=2G
TasksMax=1024
# CPU is intentionally left unrestricted by default (backup/compression
# throughput benefits from all available cores); uncomment and size to your
# hardware's core count if HomeVault needs to share CPU with other services:
#CPUQuota=300%

[Install]
WantedBy=multi-user.target
EOF
ok "System service unit written to $SERVICE_FILE"

# Optional: turn on built-in HTTPS with a self-signed certificate. Opt-in and
# only offered on a fresh install; can also be toggled later from
# Settings > Security > HTTPS/TLS. The plain-HTTP port keeps working.
if [[ "$UPGRADE" -eq 0 ]]; then
    REPLY="n"
    # Default YES: the wizard and the login form carry the administrator
    # password, and over plain HTTP it crosses the network in clear. The cost is
    # a one-time browser warning about the self-signed certificate.
    read -r -p "Enable HTTPS with a self-signed certificate? (browser warning, encrypts the LAN) [Y/n] " REPLY || REPLY="y"
    if [[ ! "$REPLY" =~ ^[Nn]$ ]]; then
        touch "$ENV_FILE"
        grep -q '^TLS_ENABLED=' "$ENV_FILE" || echo "TLS_ENABLED=true" >> "$ENV_FILE"
        grep -q '^HTTPS_PORT='  "$ENV_FILE" || echo "HTTPS_PORT=$HTTPS_PORT" >> "$ENV_FILE"
        chmod 600 "$ENV_FILE"
        ok "HTTPS enabled — reachable on https://<host>:$HTTPS_PORT after startup"
        command -v ufw >/dev/null 2>&1 && ufw allow "$HTTPS_PORT/tcp" >/dev/null 2>&1 || true
    fi
fi
systemctl daemon-reload
systemctl enable "$SERVICE_NAME" &> /dev/null

if ss -Hltn "( sport = :${PORT} )" 2>/dev/null | grep -q ":${PORT}"; then
    warn "Port $PORT appears to be in use by another program on this machine."
    echo -e "  HomeVault may not start until that port is freed."
fi

setup_automount

# `restart` (not `enable --now`) unconditionally: on a fresh install the unit
# isn't running yet, so this just starts it; on a re-run it guarantees the
# freshly-extracted binary is actually the one running — `enable --now`
# leaves an already-active old process untouched, silently "upgrading"
# nothing on disk while the service keeps serving the previous version.
systemctl restart "$SERVICE_NAME"
if [[ "$UPGRADE" -eq 1 ]]; then
    ok "System service updated and restarted"
else
    ok "System service installed and started"
fi

# ---------------------------------------------------------------------------
# Step 4 — global verification: re-check every package/binary, the systemd
# service and sharing daemons, where the application actually landed, and
# that the HTTP server really responds (not just "systemctl start didn't
# error" — Type=simple returns before the process is necessarily listening).
# ---------------------------------------------------------------------------
step "Verifying the installation"

VFAIL=0

check() {
    local label="$1" cmd="$2"
    if eval "$cmd" &> /dev/null; then
        ok "$label"
    else
        VFAIL=1
        warn "$label — missing (feature disabled in HomeVault)"
    fi
}

check "Database (sqlite3)"             "command -v sqlite3"
check "Storage (smartmontools)"        "command -v smartctl"
check "Software RAID (mdadm)"          "command -v mdadm"
check "Backups (rsync)"                "command -v rsync"
check "Network (iproute2)"             "command -v ip"
check "Media player (ffmpeg/ffprobe)"  "command -v ffmpeg && command -v ffprobe"
check "Downloader (aria2)"             "command -v aria2c"
check "Antivirus (ClamAV)"             "command -v clamscan"
check "Firewall (ufw)"                 "command -v ufw"
check "Samba share (SMB/CIFS)"         "command -v smbd"
check "NFS share"                      "command -v exportfs || dpkg -s nfs-kernel-server"
check "FTP share (vsftpd)"             "command -v vsftpd"
check "SFTP (OpenSSH)"                 "command -v sshd"
check "Docker containers"              "command -v docker"
if command -v docker &> /dev/null; then
    check "Docker Compose"             "docker compose version"
fi
check "Incus hypervisor"               "command -v incus"
check "VM virtualizer (QEMU)"          "command -v qemu-system-x86_64"
# Incus always passes -spice to qemu, so without the SPICE module NO VM can
# boot (containers are unaffected) — mirrors dependencies.rs::check_qemu_spice.
check "VM display (SPICE module)"      "ls /usr/lib/x86_64-linux-gnu/qemu /usr/lib/qemu /usr/lib64/qemu 2>/dev/null | grep -q spice"
check "SNMP monitoring"                "command -v snmpd"
check "UPS monitoring (NUT)"           "command -v upsc"
check "Hardware sensors (lm-sensors)"  "command -v sensors"
check "AI assistant (Ollama)"          "command -v ollama"
# Presence alone isn't enough for Ollama: the Chat app talks to the
# daemon's HTTP API, so verify the unit is active and :11434 answers —
# a binary with a dead service still shows "AI offline". Only meaningful
# when the binary exists at all (the warn above already covers its absence).
if command -v ollama &> /dev/null; then
    check "Ollama daemon + API"        "systemctl is-active --quiet ollama && curl -fsS --max-time 3 http://127.0.0.1:11434/"
fi
check "Docker app catalog (git)"       "command -v git"
check "File archives (unzip/zip/7z)"   "command -v unzip && command -v zip && command -v 7z"
check "Hardware info (lspci)"          "command -v lspci"
check "Host identity (hostname)"       "command -v hostname"
check "Linux-user login (python3)"     "command -v python3"
# Feature groups that arrived after the first verification list was written,
# aligned with src/dependencies.rs so the report matches what the app probes
# at runtime (each one degrades in the UI when absent).
check "ACLs (setfacl)"                 "command -v setfacl"
check "Disk quotas (repquota)"         "command -v repquota"
check "Disk encryption (LUKS2)"        "command -v cryptsetup"
check "ZFS storage pools"              "command -v zpool"
check "Btrfs storage pools"            "command -v btrfs"
check "Backups (restic)"               "command -v restic"
check "Syncthing"                      "command -v syncthing"
check "Cloud sync (rclone)"            "command -v rclone"
check "Cloud sync unmount (fusermount)" "command -v fusermount"
check "Rescue cloning (ddrescue)"      "command -v ddrescue"
check "Rescue repair (testdisk + photorec)" "command -v testdisk && command -v photorec"
check "mDNS discovery (avahi)"         "command -v avahi-daemon"
check "Hardware info (dmidecode)"      "command -v dmidecode"
check "Network diagnostics (ethtool)"  "command -v ethtool"
check "VM UEFI firmware (OVMF)"        "test -f /usr/share/OVMF/OVMF_CODE.fd || test -f /usr/share/OVMF/OVMF_CODE_4M.fd"
check "VM TPM (swtpm_setup)"           "command -v swtpm_setup"
check "Web terminal (native PTY)"      "true" # always available, built into the binary

# Where the application actually landed + is executable.
if [[ -x "$INSTALL_DIR/homevault" ]]; then
    ok "Application binary at $INSTALL_DIR/homevault (executable)"
else
    VFAIL=1
    err "Application binary missing or not executable at $INSTALL_DIR/homevault"
fi
if systemctl is-active --quiet "$SERVICE_NAME"; then
    ok "Service $SERVICE_NAME started"
else
    VFAIL=1
    err "The service $SERVICE_NAME did not start."
    echo -e "  See: ${C_BOLD}journalctl -u $SERVICE_NAME -e${C_RESET}"
fi

HV_READY=0
info "Waiting for the server to respond..."
for _ in $(seq 1 20); do
    if curl -fsS "http://127.0.0.1:${PORT}/api/auth/setup-status" > /dev/null 2>&1; then
        HV_READY=1
        break
    fi
    sleep 1
done
if [[ "$HV_READY" -eq 1 ]]; then
    ok "HomeVault is responding on port $PORT"
else
    VFAIL=1
    err "HomeVault is still not responding after 20 seconds."
    echo -e "  See: ${C_BOLD}journalctl -u $SERVICE_NAME -e${C_RESET}"
fi

for svc_pair in "smbd:Samba" "nfs-kernel-server:NFS" "vsftpd:FTP" "docker:Docker" "snmpd:SNMP"; do
    svc="${svc_pair%%:*}"; label="${svc_pair##*:}"
    if systemctl list-unit-files "${svc}.service" &> /dev/null; then
        if systemctl is-active --quiet "$svc" 2>/dev/null; then
            ok "$label active"
        else
            info "$label installed but inactive (normal while no share is configured)"
        fi
    fi
done

# ---------------------------------------------------------------------------
# Step 5 — done. A single success message with the two addresses every user
# needs: the LAN address (http://<local-ip>:port) and, when reachable, the
# public address (http://<public-ip>:port, only valid with port forwarding).
# ---------------------------------------------------------------------------
step "Done"

LAN_IP=$(ip route get 1.1.1.1 2>/dev/null | sed -n 's/.* src \([0-9.]*\).*/\1/p')
if [[ -z "$LAN_IP" ]]; then
    LAN_IP=$(hostname -I | awk '{print $1}')
fi
PUBLIC_IP=$(timeout 5 curl -fsSL --max-time 4 https://api.ipify.org 2>/dev/null || \
            timeout 5 curl -fsSL --max-time 4 https://ifconfig.me/ip 2>/dev/null || \
            timeout 5 curl -fsSL --max-time 4 https://icanhazip.com 2>/dev/null || \
            echo "")

echo ""
if [[ "$HV_READY" -eq 1 ]] && [[ "$VFAIL" -eq 0 ]]; then
    echo -e "${C_BOLD}${C_GREEN}HomeVault has been installed and started successfully.${C_RESET}"
elif [[ "$HV_READY" -eq 1 ]]; then
    echo -e "${C_BOLD}${C_YELLOW}HomeVault is installed and running, but some optional components are missing.${C_RESET}"
    echo -e "  Scroll up to see the warnings — the affected features are disabled, everything else works."
else
    echo -e "${C_BOLD}${C_YELLOW}HomeVault is installed, but the server is not responding yet.${C_RESET}"
    echo -e "  Check ${C_BOLD}journalctl -u $SERVICE_NAME -e${C_RESET} before opening the page below."
fi
echo ""
echo -e "${C_BOLD}Open HomeVault in your browser:${C_RESET}"
echo ""

if [[ -n "$LAN_IP" ]]; then
    echo -e "  ${C_CYAN}🌐 LOCAL    ${C_BOLD}http://${LAN_IP}:${PORT}${C_RESET}"
else
    echo -e "  ${C_CYAN}💻 LOCAL    ${C_BOLD}http://127.0.0.1:${PORT}${C_RESET}  (LAN IP not detected)"
fi
if [[ -n "$PUBLIC_IP" ]]; then
    echo -e "  ${C_CYAN}🌎 INTERNET ${C_BOLD}http://${PUBLIC_IP}:${PORT}${C_RESET}"
    echo -e "  ${C_DIM}   (only works if port $PORT is forwarded to this machine)${C_RESET}"
else
    echo -e "  ${C_DIM}🌎 INTERNET (public IP not detected — check the internet connection)${C_RESET}"
fi
echo ""
echo -e "   ${C_DIM}(from any device on the same network — computer,"
echo -e "    phone, tablet — no need to be on this machine)${C_RESET}"
echo ""

if [[ -t 0 ]] && [[ -n "${DISPLAY:-}${WAYLAND_DISPLAY:-}" ]]; then
    read -r -p "Open this page in the browser now? [Y/n] " REPLY || REPLY="n"
    case "$REPLY" in
        [nN]*) ;;
        *)
            OPENER=""
            for cand in xdg-open sensible-browser gio gnome-open kde-open; do
                if command -v "$cand" &> /dev/null; then OPENER="$cand"; break; fi
            done
            if [ -n "$OPENER" ]; then
                # Run as the user who invoked sudo, not root, so the browser
                # opens in their actual desktop session.
                sudo -u "${SUDO_USER:-root}" "$OPENER" "http://${LAN_IP:-127.0.0.1}:${PORT}" &> /dev/null &
                ok "Opening http://${LAN_IP:-127.0.0.1}:${PORT}..."
            else
                warn "No browser detected automatically — open the address manually."
            fi
            ;;
    esac
fi

echo ""
echo -e "${C_DIM}----------------------------------------${C_RESET}"
echo -e "${C_DIM}Technical info (optional):${C_RESET}"
echo -e "${C_DIM}📂 Installation      : $INSTALL_DIR${C_RESET}"
echo -e "${C_DIM}💾 Data              : $DATA_DIR${C_RESET}"
echo -e "${C_DIM}🔧 Service status    : systemctl status $SERVICE_NAME${C_RESET}"
echo -e "${C_DIM}📝 Service log       : journalctl -u $SERVICE_NAME -f${C_RESET}"
echo -e "${C_DIM}📋 Install log       : $LOG_FILE${C_RESET}"
echo ""
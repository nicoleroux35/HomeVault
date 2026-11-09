#!/bin/bash
set -euo pipefail

# ---------------------------------------------------------------------------
# English only — for the installer's own text *and* for everything it relays.
#
# The script's messages are written in English, but three things would still
# answer in the host's language without this: apt/dpkg warnings, systemctl
# errors, and the whiptail buttons (newt translates Yes/No/Ok/Cancel from the
# locale). `C` also makes the parsing below deterministic — `apt-cache policy`
# prints the canonical `Candidate:` label instead of a localized one (French
# `Candidat :` used to make every availability probe fail).
# ---------------------------------------------------------------------------
export LC_ALL=C
export LANG=C

# ---------------------------------------------------------------------------
# Entry bootstrap — one file, three ways in.
#
#   sudo ./install.sh                CLI, terminal, already root
#   curl … | sudo bash               piped: root, but stdin is the *script*
#   double-click in a file manager   no terminal at all, not root
#
# The last two are why this block exists and why it runs before everything
# else. A file manager executes the script with no controlling terminal and as
# the logged-in user, so `read`/whiptail would have nowhere to draw and `sudo`
# nowhere to ask for a password. Both are solved here, once:
#   1. no terminal -> re-exec inside a terminal emulator;
#   2. not root    -> re-exec through sudo, inside that terminal so the
#                     password prompt is actually visible.
# `HV_INSTALLER_STAGE` breaks the loop between the two re-execs.
#
# `--package` (maintainer build) and `--dry-run` (read-only preview) are
# deliberately excluded: neither needs root and neither may pop a window.
# ---------------------------------------------------------------------------
HV_SELF="$(readlink -f -- "${BASH_SOURCE[0]:-}" 2>/dev/null || true)"
[[ -n "$HV_SELF" && -f "$HV_SELF" ]] || HV_SELF=""

HV_BOOTSTRAP=1
# `HV_LIB_ONLY=1` turns this file into a function library for
# `installer/tests/update-engine.test.sh`: no elevation, no terminal relaunch,
# no filesystem change.
if [[ "${HV_LIB_ONLY:-0}" == "1" ]]; then HV_BOOTSTRAP=0; fi
for _hv_arg in "$@"; do
    case "$_hv_arg" in
        --package|-p|--dry-run|--verify-manifest|--help|-h) HV_BOOTSTRAP=0 ;;
        # Known flags: accepted here so an unknown one is rejected *before*
        # any elevation or terminal relaunch happens.
        --uninstall|--rollback|--purge|--cli|--tui|--yes|-y|--non-interactive) ;;
        -*)
            echo "Unknown option: $_hv_arg" >&2
            echo "Try: --help" >&2
            exit 1 ;;
        *) : ;;   # positional: the path to a pre-downloaded archive
    esac
done
unset _hv_arg

# 1. No terminal at all (double-clicked): reopen ourselves inside one. Only
#    attempted when a graphical session exists — on a headless box there is
#    nothing to open, and the plain-text path below still works.
if [[ "$HV_BOOTSTRAP" -eq 1 && "${HV_INSTALLER_STAGE:-}" != terminal ]] \
        && [[ ! -t 1 && ! -t 2 ]] && [[ -n "${DISPLAY:-}${WAYLAND_DISPLAY:-}" ]]; then
    HV_TERM=""
    for _hv_t in x-terminal-emulator gnome-terminal konsole xfce4-terminal \
                 mate-terminal lxterminal tilix kitty alacritty xterm; do
        if command -v "$_hv_t" >/dev/null 2>&1; then HV_TERM="$_hv_t"; break; fi
    done
    unset _hv_t
    if [[ -n "$HV_TERM" && -n "$HV_SELF" ]]; then
        export HV_INSTALLER_STAGE=terminal
        case "$HV_TERM" in
            gnome-terminal|tilix)
                "$HV_TERM" -- bash "$HV_SELF" "$@" >/dev/null 2>&1 && exit 0 ;;
            *)
                # `-e` is the classic form; the `--` fallback covers terminals
                # that dropped it (newer gnome-terminal). One of the two runs
                # in the background, so a successful launch returns 0 here.
                "$HV_TERM" -e bash "$HV_SELF" "$@" >/dev/null 2>&1 && exit 0
                "$HV_TERM" -- bash "$HV_SELF" "$@" >/dev/null 2>&1 && exit 0 ;;
        esac
    fi
fi

# 2. Not root: elevate. sudo resets the environment, so the documented HV_*
#    knobs are forwarded explicitly — `sudo -E` is rejected by the default
#    sudoers policy and is not used.
if [[ "$HV_BOOTSTRAP" -eq 1 && $EUID -ne 0 ]]; then
    if [[ -z "$HV_SELF" ]]; then
        # Piped without sudo: the script body lives only on stdin, so it
        # cannot re-exec itself. Name the exact command to run instead.
        echo "This installer needs administrator (root) privileges." >&2
        echo "" >&2
        echo "Re-run it with sudo:" >&2
        echo "  curl -fsSL <install-url> | sudo bash" >&2
        exit 1
    fi
    if ! command -v sudo >/dev/null 2>&1; then
        echo "This installer needs root, and 'sudo' is not installed." >&2
        echo "Log in as root and run:  bash $HV_SELF" >&2
        exit 1
    fi
    if [[ "${HV_INSTALLER_STAGE:-}" == reexec ]]; then
        # sudo ran us and we are still unprivileged: never loop.
        echo "sudo did not grant root privileges. Check the sudoers policy." >&2
        exit 1
    fi
    _hv_envs=(HV_INSTALLER_STAGE=reexec)
    for _hv_v in HV_RELEASE_BASE HV_RELEASE_URL HV_RELEASE_SHA256 HV_RELEASE_TOKEN \
                 HV_RELEASE_MANIFEST HV_RELEASE_SIGNATURE HV_RELEASE_PUBKEY \
                 HV_CATALOG_PUBKEY HV_SMARTD HV_MDMONITOR HV_SKIP_SYSCTL \
                 HV_APPARMOR HV_APPARMOR_ENFORCE \
                 HV_MODE HV_YES HV_SKIP_OPTIONAL HV_SERVICE_USER DATA_DIRECTORY PORT HTTPS_PORT; do
        if [[ -n "${!_hv_v:-}" ]]; then _hv_envs+=("$_hv_v=${!_hv_v}"); fi
    done
    unset _hv_v
    exec sudo "${_hv_envs[@]}" bash "$HV_SELF" "$@"
fi

# 3. A real TTY for prompts. Under `curl | sudo bash` stdin is the script
#    itself, so a bare `read` consumes the *next line of the script* instead of
#    the keyboard — that is exactly how the old HTTPS prompt aborted installs
#    with "syntax error near unexpected token 'fi'". Every prompt now reads
#    fd 3, bound to /dev/tty whenever one exists.
#    `: </dev/tty` is the probe, not `-r`: /dev/tty is world-readable but
#    opening it fails with ENXIO when the process has no controlling terminal
#    (cron, CI, a container, this script's own test harness).
exec 3<&0
if { : </dev/tty; } 2>/dev/null; then exec 3</dev/tty; fi

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
# Double-click: mark the file executable and open it from the file manager
# (Nemo/Thunar/Dolphin run it; GNOME's Nautilus needs the .desktop launcher
# below). It reopens itself in a terminal and elevates with sudo on its own —
# no command line needed.
#
# Interface selection (automatic by default):
#   whiptail TUI   when a terminal is available (Debian `important` priority,
#                  so it is present on every supported base) and stdin is
#                  interactive;
#   plain text     otherwise — headless, `--cli`, `--yes`, cron/CI.
#   --tui / --cli  force one of the two.
#   --yes, -y, --non-interactive
#                  never prompt: every question takes its documented default.
#   --help, -h     print this usage.
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

    # Signed release manifest (P2-8). The manifest is what the installer
    # verifies before it downloads anything; without `HV_SIGNING_KEY` it is
    # written unsigned and the installer will say so.
    HV_VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)
    if [[ -z "$HV_VERSION" ]]; then
        echo "Error: cannot read the package version from Cargo.toml." >&2
        exit 1
    fi
    MANIFEST="installer/release/homevault-${PKG_ARCH}.json"
    HV_ARCHIVE_SHA=$(cat "${ASSET}.sha256")
    HV_ARCHIVE_SIZE=$(stat -c %s "$ASSET")
    cat > "$MANIFEST" <<EOF
{
  "version": "$HV_VERSION",
  "arch": "$PKG_ARCH",
  "url": "${HV_RELEASE_BASE:-https://github.com/nicoleroux35/HomeVault/releases/latest/download}/homevault-${PKG_ARCH}.tar.gz",
  "sha256": "$HV_ARCHIVE_SHA",
  "size": $HV_ARCHIVE_SIZE,
  "min_version": "1.0.0",
  "released_at": "$(date -u +%Y-%m-%dT%H:%M:%SZ)"
}
EOF
    echo "✓ $MANIFEST"
    if [[ -n "${HV_SIGNING_KEY:-}" ]]; then
        if openssl pkeyutl -sign -inkey "$HV_SIGNING_KEY" -rawin \
                -in "$MANIFEST" -out "${MANIFEST}.sig" 2>/dev/null \
                || openssl dgst -sha256 -sign "$HV_SIGNING_KEY" \
                    -out "${MANIFEST}.sig" "$MANIFEST"; then
            echo "  signature: ${MANIFEST}.sig"
        else
            echo "Error: HV_SIGNING_KEY is set but signing failed." >&2
            exit 1
        fi
    else
        echo "  unsigned manifest — set HV_SIGNING_KEY to sign it before publishing"
    fi
    echo "  Publish the archive, its .sha256, the .json and the .json.sig as"
    echo "  GitHub Release assets so the default install URL resolves."
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

usage() {
    cat <<'EOM'
HomeVault installer — one file, for a terminal or a double-click.

  sudo ./install.sh [archive]        install, or upgrade in place (data kept)
  curl -fsSL <url> | sudo bash       same, from a single command
  double-click the file              reopens itself in a terminal, elevates
                                     with sudo, then uses the whiptail UI

Interface (chosen automatically, overridable):
  --tui                 force the whiptail text interface
  --cli                 force plain text output
  --yes, -y, --non-interactive
                        never prompt; every question takes its default

Lifecycle:
  --uninstall           remove the app, KEEP the data
  --uninstall --purge   also delete /var/lib/homevault and /etc/homevault.env
  --rollback            swap back to /opt/homevault/homevault.previous
  --dry-run             print what would happen, change nothing (no root)
  --verify-manifest     verify HV_RELEASE_MANIFEST's signature and exit (no root)
  --package, -p         maintainer: build installer/release/homevault-<arch>.tar.gz
  --help, -h            this text

Environment:
  HV_RELEASE_URL        override the download URL (mirror, pinned version)
  HV_RELEASE_SHA256     pin the archive checksum
  HV_RELEASE_TOKEN      token for a private-repository release
  HV_RELEASE_MANIFEST   signed release manifest (path or URL); pins version,
                        arch, sha256 and size. Refused if the signature is bad.
  HV_RELEASE_SIGNATURE  detached signature (default: <manifest>.sig)
  HV_RELEASE_PUBKEY     release trust anchor, PEM (default:
                        /usr/local/lib/homevault/release-key.pem)
  HV_SIGNING_KEY        maintainer: private key used by --package to sign the
                        manifest it writes (never commit this file)
  HV_CATALOG_PUBKEY     Ed25519 public key of community app stores (inline
                        hex/base64 or a path); installed for P1-27. Without
                        it, stores are listed as unsigned.
  HV_SMARTD=1           enable smartd too (off by default: HomeVault samples
                        SMART itself, running both duplicates every alert)
  HV_MDMONITOR=1        enable mdmonitor too (same duplication rule as smartd)
  HV_SKIP_SYSCTL=1      write but do not apply the kernel stability settings
  HV_YES=1              same as --yes
  HV_MODE=cli|tui       same as --cli / --tui
  HV_SERVICE_USER       root (default) or homevault
                        homevault runs the unit as a dedicated system account
                        with a scoped /etc/sudoers.d/homevault; root keeps the
                        current all-privileged deployment
  HV_APPARMOR=1         install and load the optional AppArmor profile
                        (complain mode by default)
  HV_APPARMOR_ENFORCE=1 with HV_APPARMOR=1, load it in enforce mode after the
                        profile has been validated on this host

Log: /var/log/homevault-install.log
EOM
}

MODE="install"
PURGE=0
DRY_RUN=0
# Interface request. "auto" picks whiptail when a terminal exists (see the
# IS_TTY block below); `--yes` always wins and forces the silent text path.
UI_REQUEST="${HV_MODE:-auto}"
YES=0
[[ "${HV_YES:-0}" == "1" ]] && YES=1
for arg in "$@"; do
    case "$arg" in
        --uninstall) MODE="uninstall" ;;
        --rollback)  MODE="rollback" ;;
        --verify-manifest) MODE="verify-manifest" ;;
        --purge)     PURGE=1 ;;
        --dry-run)   DRY_RUN=1 ;;
        --cli)       UI_REQUEST="cli" ;;
        --tui)       UI_REQUEST="tui" ;;
        --yes|-y|--non-interactive) YES=1 ;;
        --help|-h)
            usage
            exit 0 ;;
    esac
done
case "$UI_REQUEST" in auto|cli|tui) ;; *)
    echo "Unknown HV_MODE '$UI_REQUEST' (auto, cli or tui)." >&2; exit 1 ;;
esac

# Print-or-run: every destructive action goes through this, so --dry-run is a
# faithful preview rather than an approximation.
run() {
    if [[ "$DRY_RUN" -eq 1 ]]; then
        echo "  [dry-run] $*"
    else
        "$@"
    fi
}

# ---------------------------------------------------------------------------
# Update engine (P2-8) — signed manifests, atomic swap, automatic rollback.
#
# Everything below is side-effect free until `hv_swap_binary` /
# `hv_rollback_binary` / `hv_write_result`. `installer/tests/update-engine.test.sh`
# sources this file with `HV_LIB_ONLY=1` and exercises these functions without
# touching the host. Keep them dependency-light: bash, coreutils, python3,
# openssl and curl are already required by the installer.
# ---------------------------------------------------------------------------

hv_die() {
    echo "update: $*" >&2
    return 1
}

# Returns 0 when dotted version $1 is >= $2. Missing/non-numeric parts count as
# 0, so "1.2" >= "1.2.0" and "1.2.3-beta" == "1.2.3".
hv_version_ge() {
    local a="$1" b="$2"
    local IFS='.'
    local -a av=() bv=()
    read -r -a av <<< "$a"
    read -r -a bv <<< "$b"
    local n=${#av[@]}
    if (( ${#bv[@]} > n )); then n=${#bv[@]}; fi
    local i x y
    for ((i = 0; i < n; i++)); do
        x="${av[i]:-0}"
        y="${bv[i]:-0}"
        x="${x%%[!0-9]*}"
        y="${y%%[!0-9]*}"
        x="${x:-0}"
        y="${y:-0}"
        if (( 10#$x > 10#$y )); then return 0; fi
        if (( 10#$x < 10#$y )); then return 1; fi
    done
    return 0
}

hv_sha256() {
    sha256sum -- "$1" | awk '{print $1}'
}

hv_size() {
    stat -c %s -- "$1"
}

# Prints one field from a JSON manifest. Exits non-zero when the field is
# absent, so callers can distinguish "0" from "not set".
hv_manifest_field() {
    local manifest="$1" field="$2"
    if ! command -v python3 >/dev/null 2>&1; then
        hv_die "python3 is required to read a signed manifest"
        return 1
    fi
    python3 - "$manifest" "$field" <<'PY'
import json
import sys

try:
    with open(sys.argv[1], encoding="utf-8") as fh:
        data = json.load(fh)
except Exception as exc:  # noqa: BLE001 - reported to the operator
    print("invalid manifest: %s" % exc, file=sys.stderr)
    sys.exit(1)

value = data.get(sys.argv[2])
if value is None:
    sys.exit(1)
if isinstance(value, bool):
    print("true" if value else "false")
else:
    print(value)
PY
}

# Verifies a detached signature over the exact manifest bytes. Tries Ed25519
# without a pre-hash first (OpenSSL 3), then RSA/ECDSA with SHA-256 (works on
# OpenSSL 1.1.1 too). Returns 0 only for a valid signature.
hv_verify_signature() {
    local manifest="$1" signature="$2" pubkey="$3"
    if [[ ! -s "$manifest" || ! -s "$signature" || ! -s "$pubkey" ]]; then
        return 1
    fi
    if openssl pkeyutl -verify -pubin -inkey "$pubkey" -rawin \
            -in "$manifest" -sigfile "$signature" >/dev/null 2>&1; then
        return 0
    fi
    if openssl dgst -sha256 -verify "$pubkey" \
            -signature "$signature" "$manifest" >/dev/null 2>&1; then
        return 0
    fi
    return 1
}

# Copies a local path or downloads an http(s) URL.
hv_fetch() {
    local src="$1" dst="$2"
    case "$src" in
        http://*|https://*)
            local -a auth=()
            if [[ -n "${RELEASE_TOKEN:-}" ]]; then
                auth=(-H "Authorization: Bearer $RELEASE_TOKEN")
            fi
            curl -fL --retry 3 --retry-delay 2 "${auth[@]}" -o "$dst" "$src" ;;
        *)
            cp -f -- "$src" "$dst" ;;
    esac
}

# Fetches and verifies a signed manifest, then exports HV_MANIFEST_* globals.
hv_load_manifest() {
    local manifest_src="$1" signature_src="$2" pubkey="$3" workdir="$4"
    local manifest="$workdir/manifest.json"
    local signature="$workdir/manifest.json.sig"

    if [[ -z "$manifest_src" ]]; then
        hv_die "no manifest supplied"
        return 1
    fi
    if [[ -z "$signature_src" ]]; then
        signature_src="${manifest_src}.sig"
    fi
    if [[ ! -f "$pubkey" ]]; then
        hv_die "release public key not found: $pubkey"
        return 1
    fi
    if ! hv_fetch "$manifest_src" "$manifest"; then
        hv_die "could not fetch the manifest"
        return 1
    fi
    if [[ "$signature_src" == http://* || "$signature_src" == https://* ]]; then
        if ! hv_fetch "$signature_src" "$signature"; then
            hv_die "could not fetch the signature"
            return 1
        fi
    elif [[ -f "$signature_src" ]]; then
        cp -f -- "$signature_src" "$signature"
    else
        hv_die "signature not found: $signature_src"
        return 1
    fi
    if ! hv_verify_signature "$manifest" "$signature" "$pubkey"; then
        hv_die "manifest signature is invalid — refusing the update"
        return 1
    fi

    HV_MANIFEST_FILE="$manifest"
    if ! HV_MANIFEST_VERSION=$(hv_manifest_field "$manifest" version); then
        hv_die "manifest has no version"
        return 1
    fi
    HV_MANIFEST_ARCH=$(hv_manifest_field "$manifest" arch 2>/dev/null || true)
    HV_MANIFEST_URL=$(hv_manifest_field "$manifest" url 2>/dev/null || true)
    HV_MANIFEST_SHA256=$(hv_manifest_field "$manifest" sha256 2>/dev/null || true)
    HV_MANIFEST_SIZE=$(hv_manifest_field "$manifest" size 2>/dev/null || true)
    return 0
}

# Checks the archive against the manifest's size and SHA-256 (either may be
# omitted; at least one should be present in a production manifest).
hv_verify_archive() {
    local archive="$1" expected_sha="$2" expected_size="$3"
    if [[ ! -f "$archive" ]]; then
        hv_die "archive not found: $archive"
        return 1
    fi
    if [[ -n "$expected_size" ]]; then
        local actual_size
        actual_size=$(hv_size "$archive")
        if [[ "$actual_size" != "$expected_size" ]]; then
            hv_die "archive size mismatch (expected $expected_size, got $actual_size)"
            return 1
        fi
    fi
    if [[ -n "$expected_sha" ]]; then
        local actual_sha
        actual_sha=$(hv_sha256 "$archive")
        if [[ "${actual_sha,,}" != "${expected_sha,,}" ]]; then
            hv_die "archive SHA-256 mismatch"
            return 1
        fi
    fi
    return 0
}

# Runs the staged binary before it can replace anything. Sets
# HV_STAGED_VERSION from `homevault X.Y.Z`.
hv_smoke_binary() {
    local binary="$1" want_version="$2" out=""
    if [[ ! -x "$binary" ]]; then
        hv_die "staged binary is not executable"
        return 1
    fi
    if ! out=$(timeout 10 "$binary" --version 2>/dev/null); then
        hv_die "staged binary failed its --version smoke test"
        return 1
    fi
    if [[ -n "$want_version" && "$out" != *"$want_version"* ]]; then
        hv_die "staged binary reports '$out', manifest says '$want_version'"
        return 1
    fi
    HV_STAGED_VERSION="${out#* }"
    return 0
}

# Replaces $install_dir/homevault atomically, keeping exactly one previous
# build. It does NOT touch the service: the caller restarts it, health-checks,
# and calls hv_rollback_binary on failure.
hv_swap_binary() {
    local staged="$1" install_dir="$2"
    local current="$install_dir/homevault"
    local previous="$install_dir/homevault.previous"
    local incoming="$install_dir/.homevault.new"

    if [[ ! -x "$staged" ]]; then
        hv_die "staged binary is missing or not executable"
        return 1
    fi
    if ! install -m 0755 -- "$staged" "$incoming"; then
        hv_die "cannot stage the new binary"
        return 1
    fi
    sync
    if [[ -x "$current" ]]; then
        if ! cp -f -- "$current" "$previous"; then
            hv_die "cannot keep the previous build"
            rm -f -- "$incoming"
            return 1
        fi
    fi
    if ! mv -f -- "$incoming" "$current"; then
        hv_die "cannot install the new binary"
        rm -f -- "$incoming"
        return 1
    fi
    sync
    return 0
}

# Restores homevault.previous over homevault. The failed build is kept as
# homevault.failed for inspection.
hv_rollback_binary() {
    local install_dir="$1"
    local current="$install_dir/homevault"
    local previous="$install_dir/homevault.previous"
    local failed="$install_dir/homevault.failed"

    if [[ ! -x "$previous" ]]; then
        hv_die "no previous build to roll back to"
        return 1
    fi
    if [[ -e "$current" ]]; then
        mv -f -- "$current" "$failed" 2>/dev/null || true
    fi
    if ! mv -f -- "$previous" "$current"; then
        hv_die "cannot restore the previous build"
        return 1
    fi
    chmod 0755 -- "$current" 2>/dev/null || true
    sync
    return 0
}

# Polls a readiness URL until it answers 2xx or the deadline passes.
hv_wait_healthy() {
    local url="$1" timeout_s="${2:-20}"
    local deadline=$(( $(date +%s) + timeout_s ))
    while (( $(date +%s) <= deadline )); do
        if curl -fsS --max-time 2 -- "$url" >/dev/null 2>&1; then
            return 0
        fi
        sleep 1
    done
    return 1
}

# Writes the last update result and appends it to a history log. Best-effort:
# a failed write must never turn a successful update into a failure.
hv_write_result() {
    local data_dir="$1" status="$2" version="$3" previous="$4" message="$5"
    local dir="$data_dir/updates"
    mkdir -p "$dir" 2>/dev/null || return 0
    command -v python3 >/dev/null 2>&1 || return 0
    local ts
    ts=$(date -u +%Y-%m-%dT%H:%M:%SZ)
    python3 - "$dir/last-result.json" "$dir/history.jsonl" \
        "$status" "$version" "$previous" "$message" "$ts" <<'PY'
import json
import os
import sys

result_path, history_path, status, version, previous, message, ts = sys.argv[1:8]
doc = {
    "status": status,
    "version": version,
    "previous_version": previous,
    "message": message,
    "finished_at": ts,
}
tmp = result_path + ".tmp"
with open(tmp, "w", encoding="utf-8") as fh:
    json.dump(doc, fh, ensure_ascii=False, indent=2)
    fh.write("\n")
os.replace(tmp, result_path)
# `history.jsonl` is one *compact* JSON object per line, so a consumer can
# read it with `readline()`/`jq -c`. `last-result.json` above stays pretty for
# humans; copying it verbatim here wrote a 7-line record and broke JSONL.
try:
    with open(history_path, "a", encoding="utf-8") as fh:
        fh.write(json.dumps(doc, ensure_ascii=False) + "\n")
except OSError:
    pass
PY
    return 0
}

# ---------------------------------------------------------------------------
# Process isolation (P2-7).
#
# The hardening block is deliberately conservative: HomeVault shells out to
# privileged host tools through `sudo` (Samba, nftables/ufw, mdadm/zpool,
# mount, modprobe, hostnamectl, timedatectl, wg-quick…). A directive that
# blocks one of those turns a shipped feature into a silent 503, so the block
# below only contains restrictions that cannot break them.
#
# Deliberately NOT set, and why:
#   ProtectKernelTunables — VPN PostUp runs `sysctl -w net.ipv4.ip_forward=1`
#   ProtectKernelModules  — iSCSI runs `modprobe target_core_mod` (via sudo)
#   ProtectHostname       — Settings > General renames the server
#   ProtectClock          — Settings > General changes the timezone
#   PrivateDevices        — Storage/RAID/SMART need the real /dev nodes
#   ProtectHome           — user home folders live under /home/homevault
#   CapabilityBoundingSet — sudo is setuid; a bounding set would mask it
#   NoNewPrivileges       — set only in root mode; in the unprivileged mode
#                           `sudo` is the whole privilege path and NNP would
#                           make every privileged feature fail
# ---------------------------------------------------------------------------
service_hardening_block() {
    local run_user="$1"
    cat <<'EOF'
# --- Hardening (P2-7) -------------------------------------------------------
# Read-only system binaries: the service cannot rewrite /usr or /boot, which
# also protects the installed binary from its own process.
ProtectSystem=true
# The binary and its assets are read-only to the service. An update is applied
# by the installer (or a privileged transient unit), never by this process.
ReadOnlyPaths=-/opt/homevault
# Each unit gets its own /tmp and /var/tmp; no cross-service temp-file reads.
PrivateTmp=true
# No new setuid/setgid files can be created by the service or its children.
RestrictSUIDSGID=true
# No execution-domain switching (the backend has no JIT).
LockPersonality=true
# No realtime scheduling from the service.
RestrictRealtime=true
# /dev/kmsg and syslog(2) are closed; journal files stay readable.
ProtectKernelLogs=true
# Optional, only after measuring on the target host:
#RestrictAddressFamilies=AF_UNIX AF_INET AF_INET6 AF_NETLINK
EOF
    if [[ "$run_user" == "root" ]]; then
        cat <<'EOF'
# Root deployment only: no child may *gain* privileges (setuid binaries,
# file capabilities). Every privileged tool is already running as root, so
# sudo, mount and ping keep working; the unprivileged deployment below must
# leave this off because sudo is its privilege path.
NoNewPrivileges=true
EOF
    fi
}

render_service_unit() {
    local run_user="$1" run_group="$2" service_supp="$3" run_home="$4"
    cat <<EOF
[Unit]
Description=HomeVault Server
After=network.target
# P1-28: a crash loop must not spin forever.
StartLimitIntervalSec=60
StartLimitBurst=5

[Service]
Type=simple
# FIAB-02: systemd restarts a process that stops answering. The heartbeat is
# sent by src/systemd_notify.rs every 10 s; three missed ticks (30 s) mean a
# blocked runtime, not a slow backup.
WatchdogSec=30
NotifyAccess=main
User=$run_user
Group=$run_group
$service_supp
Environment="HOME=$run_home"
WorkingDirectory=$INSTALL_DIR
ExecStart=$INSTALL_DIR/homevault
Restart=on-failure
RestartSec=5
# P1-28: the appliance serves many concurrent shares/backups; the default
# 1024-descriptor limit is too low. OOMPolicy=stop makes an OOM kill visible
# instead of leaving a half-alive unit.
LimitNOFILE=65536
TimeoutStopSec=30
OOMPolicy=stop
# A custom privileged PORT (e.g. 80) must still bind when the unit is not
# root; harmless for the root deployment.
AmbientCapabilities=CAP_NET_BIND_SERVICE
Environment="PORT=$PORT"
Environment="DATA_DIRECTORY=$DATA_DIR"
# Provision the Linux/Samba identity of a new HomeVault account (useradd,
# chpasswd, smbpasswd). Without this the web login works but SMB/FTP do not
# have a credential. Tests and a bare \`cargo run\` leave it unset on purpose.
Environment="HV_HOST_SYNC=1"
# Built-in HTTPS is opt-in (Settings > Security > HTTPS/TLS, or set
# TLS_ENABLED=true here / in /etc/homevault.env). When enabled, HomeVault
# adds a TLS listener on HTTPS_PORT (default $HTTPS_PORT) alongside HTTP.
# Plain HTTP is then redirected to it by default (TLS_REDIRECT=0 keeps both
# listeners usable), so the admin password is never served in clear.
#Environment="TLS_ENABLED=true"
#Environment="HTTPS_PORT=$HTTPS_PORT"
#Environment="TLS_REDIRECT=0"
EnvironmentFile=-$ENV_FILE

$(service_hardening_block "$run_user")

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
}

# Optional AppArmor attachment. Complain mode by default: the service runs a
# lot of host tools through sudo, so enforcement is a field-validated decision,
# not an installer default. `HV_APPARMOR_ENFORCE=1` flips the flag.
render_apparmor_profile() {
    local flags="complain"
    if [[ "${HV_APPARMOR_ENFORCE:-0}" == "1" ]]; then
        flags="enforce"
    fi
    cat <<EOF
# HomeVault — generated by installer/install.sh (P2-7).
# Default is complain mode until this profile has been validated on the host
# (\`dmesg | grep -i apparmor\`, \`aa-logprof\`, then HV_APPARMOR_ENFORCE=1).
#include <tunables/global>

profile homevault $INSTALL_DIR/homevault flags=($flags) {
  #include <abstractions/base>
  #include <abstractions/nameservice>

  capability,
  network,
  mount,
  umount,
  pivot_root,
  signal,
  unix,

  $INSTALL_DIR/homevault mr,
  $INSTALL_DIR/** r,
  /var/lib/homevault/** rwkl,
  /etc/homevault/** rw,
  /etc/homevault.env r,
  /etc/homevault-rsync/** rw,
  /etc/homevault-backup/** rw,
  /etc/samba/** rw,
  /etc/exports rw,
  /etc/vsftpd.conf rw,
  /etc/vsftpd.user_list rw,
  /etc/netplan/** rw,
  /etc/sudoers.d/homevault rw,
  /etc/nut/** rw,
  /etc/systemd/system/** rw,
  /etc/apparmor.d/homevault r,
  /home/homevault/** rwkl,
  /mnt/** rwkl,
  /media/** rwkl,
  /srv/** rwkl,
  /run/** rw,
  /tmp/** rw,
  /var/tmp/** rw,
  /proc/** r,
  /sys/** rw,
  /dev/** rw,
  /usr/** r,
  /bin/** r,
  /sbin/** r,
  /lib/** r,
  /lib64/** r,
  /boot/** r,
  owner /root/** rw,
}
EOF
}

# Signed update engine unit (P2-8, UI half).
#
# Source unique, testable (`installer/tests/unit-hardening.test.sh` asserts the
# fixed ExecStart and the restart-survival settings). The web handler can only
# `systemctl start` this unit; the argv is fixed here and the engine still
# refuses an unsigned manifest before touching anything.
render_update_unit() {
    local install_dir="${1:-$INSTALL_DIR}"
    cat <<EOF
[Unit]
Description=HomeVault signed update engine (atomic swap with rollback)
Documentation=file://$install_dir/installer/install.sh
After=network-online.target
Wants=network-online.target
# No installer script means no engine: refuse rather than fail halfway.
ConditionPathExists=$install_dir/installer/install.sh

[Service]
Type=oneshot
# Fixed argv, root-owned, and the only thing the web handler may start.
ExecStart=$install_dir/homevault --update-engine
# A full download + swap takes minutes; this is the ceiling, not the target.
TimeoutStartSec=3600
# The engine restarts homevault.service in the middle of the swap; it must
# outlive the process it replaces.
KillMode=process
Nice=10
EOF
}

# Kernel stability knobs (P1-28). A NAS that hits a kernel oops should reboot
# rather than hang with its shares half-mounted, and keeping dirty writeback
# bounded protects a slow USB/rotational disk from a multi-minute flush stall.
# The file is always written so the values are explicit in `sysctl --system`;
# an operator overrides them in a later-sorting file.
render_sysctl() {
    cat <<'EOF'
# Generated by HomeVault (installer). Override in /etc/sysctl.d/99-*.conf
# (a file sorting after this one) if the host needs different values.
# Reboot on a kernel panic/oops instead of hanging with half-mounted shares.
kernel.panic = 10
kernel.panic_on_oops = 1
# Keep dirty writeback bounded: a slow disk must not accumulate minutes of
# unflushed data when a backup or a large copy fills the page cache.
vm.dirty_background_ratio = 5
vm.dirty_ratio = 10
EOF
}

# Should the installer enable a host monitoring daemon?
#
# Usage: monitor_decision <available:y|n> <enabled:y|n> <requested:0|1>
# Prints exactly one word:
#   unavailable - the package is not installed
#   already     - the operator already enabled it; never double silently
#   enable      - requested and currently disabled
#   leave       - not requested; HomeVault's own monitoring is the only one
#
# The rule from the backlog: run the host daemons or HomeVault's jobs, never
# both without saying so. Enabling one is therefore an explicit opt-in.
monitor_decision() {
    local available="$1" enabled="$2" requested="$3"
    if [[ "$available" != "y" ]]; then
        echo "unavailable"
    elif [[ "$enabled" == "y" ]]; then
        echo "already"
    elif [[ "$requested" == "1" ]]; then
        echo "enable"
    else
        echo "leave"
    fi
}

# Sourced by `installer/tests/*.test.sh`: stop here.
if [[ "${HV_LIB_ONLY:-0}" == "1" ]]; then
    return 0 2>/dev/null || exit 0
fi


if [[ "$MODE" == "verify-manifest" ]]; then
    MANIFEST_SRC="${HV_RELEASE_MANIFEST:-}"
    MANIFEST_PUBKEY="${HV_RELEASE_PUBKEY:-/usr/local/lib/homevault/release-key.pem}"
    if [[ -z "$MANIFEST_SRC" ]]; then
        echo "Usage: HV_RELEASE_MANIFEST=<path|url> [HV_RELEASE_PUBKEY=<pem>] $0 --verify-manifest" >&2
        exit 2
    fi
    VERIFY_WORK=$(mktemp -d /tmp/homevault-verify.XXXXXX)
    trap 'rm -rf "$VERIFY_WORK"' EXIT
    if ! hv_load_manifest "$MANIFEST_SRC" "${HV_RELEASE_SIGNATURE:-}" \
            "$MANIFEST_PUBKEY" "$VERIFY_WORK"; then
        echo "✗ Manifest verification failed — refuse to trust this release." >&2
        exit 1
    fi
    echo "✓ Signed manifest verified"
    echo "  version: $HV_MANIFEST_VERSION"
    echo "  arch   : ${HV_MANIFEST_ARCH:-<unspecified>}"
    if [[ -n "${HV_MANIFEST_SHA256:-}" ]]; then
        echo "  sha256 : $HV_MANIFEST_SHA256"
    fi
    if [[ -n "${HV_MANIFEST_SIZE:-}" ]]; then
        echo "  size   : $HV_MANIFEST_SIZE bytes"
    fi
    exit 0
fi

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
        read -r -p "Type PURGE to confirm: " -u 3 CONFIRM || CONFIRM=""
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
    # The scoped sudoers file belongs to this install; a leftover would keep
    # granting a future `homevault` account root commands.
    run rm -f /etc/sudoers.d/homevault
    run systemctl daemon-reload 2>/dev/null || true
    run rm -rf "$INSTALL_DIR_DEFAULT"

    if [[ "$PURGE" -eq 1 ]]; then
        run rm -rf "$DATA_DIR_DEFAULT"
        run rm -f "$ENV_FILE_DEFAULT"
        run userdel homevault 2>/dev/null || true
        run groupdel homevault 2>/dev/null || true
        echo "  Data, environment file and service account removed."
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
    echo "  runs as     : ${HV_SERVICE_USER:-root} (HV_SERVICE_USER=homevault for the unprivileged mode)"
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

# Root gate — normally already satisfied by the bootstrap above (which
# re-execs through sudo, or explains why it cannot). Kept as the last line of
# defence so no code below can ever run unprivileged.
if [[ $EUID -ne 0 ]]; then
    echo "This script must be run with administrator (root) privileges." >&2
    echo "" >&2
    echo "Re-run it with:  sudo bash $0" >&2
    echo "or:              curl -fsSL <install-url> | sudo bash" >&2
    exit 1
fi

INSTALL_DIR="/opt/homevault"
DATA_DIR="/var/lib/homevault"
SERVICE_NAME="homevault"
SERVICE_FILE="/etc/systemd/system/homevault.service"
ENV_FILE="/etc/homevault.env"
PORT="8080"
HTTPS_PORT="8443"

# Deployment model. `root` is the default, proven on real appliances: the
# service manages RAID/ZFS/Samba/Docker and reads every user's files, so an
# unprivileged process needs either a very broad sudoers file or loses file
# access. `homevault` is the optional split deployment: a dedicated system
# account plus a scoped /etc/sudoers.d/homevault (rendered by the binary
# itself), so the long-running process never holds uid 0. Both modes run the
# same code; every host mutation already goes through `sudo`.
RUN_USER="${HV_SERVICE_USER:-root}"
case "$RUN_USER" in
    root)
        RUN_GROUP="root"
        RUN_HOME="/root"
        ;;
    homevault)
        RUN_GROUP="homevault"
        RUN_HOME="$DATA_DIR"
        ;;
    *)
        echo "HV_SERVICE_USER must be 'root' or 'homevault' (got '$RUN_USER')." >&2
        exit 1
        ;;
esac

# Pre-built application archive — one compressed `homevault` binary. Published
# per architecture on the project's GitHub Releases as `homevault-<arch>.tar.gz`
# (x86_64 / aarch64 / armv7l), exactly what `install.sh --package` produces.
# `HV_RELEASE_URL` overrides the whole URL (mirror, pinned version, …),
# `HV_RELEASE_SHA256` optionally pins the archive's checksum, and
# `HV_RELEASE_TOKEN` authenticates the download when the release lives in a
# PRIVATE repository — GitHub answers 404 (not 401) to an anonymous request
# there, so without a token the default URL is indistinguishable from a typo.
RELEASE_BASE="${HV_RELEASE_BASE:-https://github.com/nicoleroux35/HomeVault/releases/latest/download}"
RELEASE_URL="${HV_RELEASE_URL:-}"
RELEASE_SHA256="${HV_RELEASE_SHA256:-}"
RELEASE_TOKEN="${HV_RELEASE_TOKEN:-}"
# Signed release channel (P2-8). When HV_RELEASE_MANIFEST is set, its signature
# is verified before anything is downloaded, and its version/arch/sha256/size
# are enforced. Without it, the installer keeps the pre-existing behaviour
# (optional pinned checksum) and says so instead of implying a signature.
RELEASE_MANIFEST="${HV_RELEASE_MANIFEST:-}"
RELEASE_SIGNATURE="${HV_RELEASE_SIGNATURE:-}"
RELEASE_PUBKEY="${HV_RELEASE_PUBKEY:-/usr/local/lib/homevault/release-key.pem}"
# Readiness endpoint used by the automatic rollback. `/api/health/ready` is
# public and proves the database is reachable, not just that the process is up.
HEALTH_URL_DEFAULT="http://127.0.0.1:${PORT}/api/health/ready"

# Avoid apt/debconf ever popping an interactive prompt mid-install (a
# headless box or a `curl | sudo bash` pipe has no one to answer it, so a
# stray prompt would just hang forever instead of failing loudly).
export DEBIAN_FRONTEND=noninteractive
export NEEDRESTART_MODE=a

# Detect a real terminal *before* stdout gets tee'd to a log file below —
# once that pipe is in place `[ -t 1 ]` would always read false.
if [ -t 1 ]; then IS_TTY=1; else IS_TTY=0; fi

# ---------------------------------------------------------------------------
# Interface selection — whiptail TUI or plain text.
#
# whiptail ships at Debian/Ubuntu priority `important`, i.e. present on every
# supported base, so no extra package is needed. It is still optional: an
# image that trimmed it (or a container) falls back to the text UI, and
# `--yes` always does, because there is nobody to answer.
#
# In TUI mode stdout/stderr go to the log file only: whiptail owns the
# terminal (drawn on fd 4) and engine chatter would otherwise paint over its
# screens. The log stays the detailed record, the TUI the summary.
# ---------------------------------------------------------------------------
TUI=0
if [[ "$YES" -eq 0 && "$DRY_RUN" -eq 0 && "$UI_REQUEST" != "cli" ]]; then
    if command -v whiptail >/dev/null 2>&1; then
        if [[ "$UI_REQUEST" == "tui" ]] || [[ "$IS_TTY" -eq 1 ]]; then
            # Probe by *opening* /dev/tty: `[[ -w ]]` succeeds on a process
            # with no controlling terminal, the open then fails with ENXIO.
            if { : >/dev/tty; } 2>/dev/null; then
                TUI=1
            elif [[ "$UI_REQUEST" == "tui" ]]; then
                echo "--tui needs a terminal (/dev/tty is not available)." >&2
                exit 1
            fi
        fi
    elif [[ "$UI_REQUEST" == "tui" ]]; then
        echo "--tui requires whiptail:  apt-get install -y whiptail" >&2
        exit 1
    fi
fi

LOG_FILE="/var/log/homevault-install.log"
if [[ "$TUI" -eq 1 ]]; then
    exec 4>/dev/tty      # whiptail's private channel to the terminal
    exec >>"$LOG_FILE" 2>&1
else
    exec > >(tee -a "$LOG_FILE") 2>&1
fi

# Everything printed from here on (successes, warnings, raw apt/systemctl
# output alike) is also kept on disk — a failure a non-technical user can't
# make sense of on screen is still fully diagnosable by whoever they ask for
# help, without re-running the installer. The redirection itself was chosen
# above, together with the interface mode.

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

# A 40-column progress bar, drawn as text so it works in whiptail's --infobox
# and in a plain terminal alike.
bar() {
    local pct="$1" width=40 filled i out=""
    filled=$(( pct * width / 100 ))
    for (( i = 0; i < width; i++ )); do
        if [ "$i" -lt "$filled" ]; then out="$out#"; else out="$out-"; fi
    done
    printf '[%s] %3d%%' "$out" "$pct"
}

# ---------------------------------------------------------------------------
# Interface layer — whiptail TUI with a plain-text fallback.
#
# `--output-fd 1` puts whiptail's *answer* on stdout (captured by `$(…)`),
# while `2>&4` sends its drawing straight to the terminal, so the answer is
# never mixed with escape sequences and the log stays readable. Input comes
# from /dev/tty too: under `curl | sudo bash` stdin is the script itself.
# ---------------------------------------------------------------------------
ui_msg()   { whiptail --backtitle "HomeVault installer" --title "$1" \
                --scrolltext --msgbox "$2" 20 78 >&4 2>&4 </dev/tty; }
ui_yesno() { # $1 title, $2 text, $3 default (y|n)
    if [[ "${3:-y}" == "n" ]]; then
        whiptail --backtitle "HomeVault installer" --title "$1" --defaultno \
            --yesno "$2" 14 78 >&4 2>&4 </dev/tty
    else
        whiptail --backtitle "HomeVault installer" --title "$1" \
            --yesno "$2" 14 78 >&4 2>&4 </dev/tty
    fi
}
ui_input() { whiptail --backtitle "HomeVault installer" --title "$1" \
                --output-fd 1 --inputbox "$2" 12 78 "${3:-}" 2>&4 </dev/tty; }
ui_info()  { whiptail --backtitle "HomeVault installer" --title "$1" \
                --infobox "$2" 10 72 >&4 2>&4 </dev/tty; }

# One prompt, three backends. `--yes` and a missing terminal both take the
# documented default instead of blocking an unattended run.
ask() {
    local question="$1" default="${2:-y}"
    if [[ "$YES" -eq 1 ]]; then REPLY="$default"; return 0; fi
    if [[ "$TUI" -eq 1 ]]; then
        if ui_yesno "Confirmation" "$question" "$default"; then REPLY="y"; else REPLY="n"; fi
        return 0
    fi
    if [[ ! -t 3 ]]; then REPLY="$default"; return 0; fi
    read -r -p "$question " -u 3 REPLY || REPLY="$default"
}

step() {
    STEP_NUM=$((STEP_NUM + 1))
    local pct=$(( STEP_NUM * 100 / STEP_TOTAL ))
    if [[ "$TUI" -eq 1 ]]; then
        ui_info "Installing HomeVault — step ${STEP_NUM}/${STEP_TOTAL}" \
            "$(bar "$pct")

$1"
        echo "▶ Step ${STEP_NUM}/${STEP_TOTAL} — $1"
        return 0
    fi
    echo ""
    echo -e "${C_BOLD}${C_CYAN}▶ Step ${STEP_NUM}/${STEP_TOTAL} — $1${C_RESET}"
}

# In TUI mode these lines are the *log* (stdout is already redirected to
# $LOG_FILE) — never the screen, where whiptail owns the terminal.
if [[ "$TUI" -eq 1 ]]; then
    ok()   { echo "  ✓ $1"; }
    warn() { echo "  ⚠ $1"; }
    err()  { echo "  ✗ $1"; }
    info() { echo "    $1"; }
else
    ok()   { echo -e "  ${C_GREEN}✓${C_RESET} $1"; }
    warn() { echo -e "  ${C_YELLOW}⚠${C_RESET} $1"; }
    err()  { echo -e "  ${C_RED}✗${C_RESET} $1"; }
    info() { echo -e "  ${C_DIM}$1${C_RESET}"; }
fi

# Catch-all for anything that fails in a way none of the checks below
# anticipated (network drop mid-`apt-get`, a corrupt package cache...) — with
# `set -e`, that would otherwise stop the script on a raw, unstyled shell
# error with no pointer to the log this non-technical audience wouldn't
# think to look for on their own.
on_unexpected_error() {
    local line="$1"
    if [[ "$TUI" -eq 1 ]]; then
        ui_msg "Installation stopped" "Installation stopped unexpectedly (line ${line}).

The affected feature may be incomplete. Re-run the installer once the cause
is fixed, or read the full log:

  ${LOG_FILE}" || true
        return 0
    fi
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
# Generated service state owned by the service account (create mode 0700 so a
# half-written module list is never world-readable):
#   /etc/homevault-rsync       Rsync server modules + per-user secrets
#   /etc/homevault-backup      backup-destination tenants + rest-server auth
mkdir -p /etc/homevault-rsync /etc/homevault-backup
chmod 700 /etc/homevault-rsync /etc/homevault-backup
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
    if [[ "$mode" == "optional" ]] && [[ "${HV_SKIP_OPTIONAL:-0}" == "1" ]]; then
        info "Skipping optional package group: $label (HV_SKIP_OPTIONAL=1)"
        return 0
    fi
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
# `procps` (`ps`/`pgrep`/`kill`), `debianutils` (`which`, used by the runtime
# dependency probe) and `e2fsprogs` (`mkfs.ext4`, the Explorer's and the RAID
# app's default filesystem) are all Priority: required, so they are present on
# any supported base — requested explicitly so a trimmed image fails here,
# loudly, instead of at the first page that shells out.
install_packages "Base tools (build toolchain, databases)" required \
    build-essential pkg-config libsqlite3-dev sqlite3 wget curl ca-certificates git python3 sudo \
    procps debianutils e2fsprogs

info "Storage and monitoring..."
# `dmidecode` (Dashboard: motherboard/BIOS/RAM details), `ethtool` (Network:
# link-speed diagnostics) and `iputils-ping` (Network diagnostics) are small,
# always available in the base repos, and each backs a frozen page that
# degrades to "unknown" without them — installed unconditionally here rather
# than left to a runtime surprise.
install_packages "Storage and monitoring tools" required \
    acl quota smartmontools mdadm rsync gnupg iproute2 util-linux \
    lsb-release lm-sensors udisks2 udiskie dmidecode ethtool iputils-ping hdparm

# iSCSI exports (P3.3) are driven through `targetcli` (python3-rtslib). The
# app hides its editors and names the missing piece when the tool is absent,
# so this stays optional instead of failing the whole install.
install_optional targetcli-fb "iSCSI targetcli (disk exports)"

# `hdparm` above backs the Power app's per-disk spindown (APM level + idle
# timer, re-applied at boot by `src/power.rs`). Priority: standard and tiny,
# but NOT part of a minimal install — without it that page reports
# "hdparm not available" and the setting never applies.

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
# WS-Discovery makes the appliance appear in Windows Explorer without typing a
# path. Optional: when the package is unavailable the Shares card says so and
# SMB itself keeps working.
install_optional wsdd2 "wsdd2 (Windows network discovery)"
# `pv` is the bandwidth limiter of the snapshot-replication pipeline
# (Backup → Replication: `zfs send | pv -L <rate> | …`). Without it a job
# still replicates, it just runs unthrottled and the UI says so.
install_optional pv "pv (snapshot replication bandwidth limit)"

# Network persistence — netplan materializes the Network screen's persisted
# interface config (`/etc/netplan/99-homevault-<if>.yaml` → `netplan apply`,
# applied and verified in the same session, not just at the next reboot;
# `src/network.rs`). Ubuntu always ships it; Debian hosts without it keep the
# ifupdown backend or answer "(temporary — persistence unavailable)". Non-fatal
# like the rest of install_optional — the absence is probed at runtime.
install_optional netplan.io "Netplan (persist interface configuration)"

# `dhclient` renews/releases the DHCP lease from the Setup wizard and the
# Network screen (`src/setup.rs`). Debian netinst and Raspberry Pi OS (dhcpcd)
# do not ship it, so the action used to fail with nothing to install.
install_optional isc-dhcp-client "DHCP client (dhclient)"

# `upnpc` lists and edits the router's UPnP-IGD port mappings (Settings ->
# Security -> Router port forwarding, `src/upnp.rs`). Optional like the rest:
# the card names the package and shows no dead form when it is absent.
install_optional miniupnpc "UPnP port mapping (upnpc)"

# `dnsmasq` backs the DHCP/TFTP/PXE server (`src/dhcp_server.rs`). Serving
# addresses on a home LAN is a choice, not a baseline: `install_optional`
# keeps a declined answer non-fatal, and the DHCP card hides its editors and
# names the missing package when it is absent.
install_optional dnsmasq "DHCP/TFTP/PXE server (dnsmasq)"

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
# `smbtree` (Explorer → network discovery, SMB half) lives in samba-common-bin;
# `samba` recommends it, but a host with recommends disabled would miss it.
install_optional samba-common-bin "Samba client tools (smbtree: SMB discovery)"
install_optional nfs-kernel-server "NFS server"
# Network locations (Explorer → mount a share from *another* server): the
# server side above is not enough — `mount -t cifs` delegates to `mount.cifs`
# (cifs-utils) and `mount -t nfs` to `mount.nfs` (nfs-common). The panel names
# whichever is missing rather than offering a button that cannot work.
install_optional cifs-utils "SMB client (mount a CIFS share: network locations)"
install_optional nfs-common "NFS client (mount an NFS export: network locations)"
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
# `avahi-utils` provides `avahi-browse`, the mDNS half of the Explorer's
# network-discovery list — the daemon package does not ship it.
install_optional avahi-utils "Avahi client tools (avahi-browse: mDNS discovery)"
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

# XFS filesystem. The RAID app formats an array as ext4 **or xfs** and the
# Storage/encryption paths accept xfs too (`src/disk.rs::format_device` maps
# `xfs` → `mkfs.xfs`), but xfsprogs is Priority: optional and absent from a
# minimal base — without it, creating an XFS array fails at the mkfs step.
install_optional xfsprogs "XFS filesystem (mkfs.xfs)"

# Wi-Fi management (Network app: scan, join, status) drives `wpa_cli`, which
# ships in wpasupplicant (`src/network.rs`, also granted in the sudoers rules).
# Ethernet-only hosts are unaffected; Wi-Fi pages degrade without it.
install_optional wpasupplicant "Wi-Fi management (wpa_cli)"

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

# The daemon is what every Docker feature talks to. `docker.io` enables and
# starts it on install, but a host where it was masked or disabled would leave
# a working CLI with no engine behind it.
if command -v docker &> /dev/null; then
    systemctl enable --now docker &> /dev/null || true
fi

# Adds Docker's official apt repository. Returns non-zero (and removes the
# list file again) when the key or the index cannot be fetched: leaving a
# broken `docker.list` behind would make every later `apt-get update` fail,
# including the ones run by the operator.
add_docker_repo() {
    . /etc/os-release
    local list=/etc/apt/sources.list.d/docker.list
    install -m 0755 -d /etc/apt/keyrings
    if ! curl -fsSL "https://download.docker.com/linux/${ID}/gpg" \
            | gpg --dearmor -o /etc/apt/keyrings/docker.gpg 2>/dev/null; then
        rm -f /etc/apt/keyrings/docker.gpg
        warn "Could not fetch the Docker repository key — Compose stays unavailable"
        return 1
    fi
    chmod a+r /etc/apt/keyrings/docker.gpg
    echo "deb [arch=$(dpkg --print-architecture) signed-by=/etc/apt/keyrings/docker.gpg] https://download.docker.com/linux/${ID} ${VERSION_CODENAME} stable" \
        > "$list"
    if ! apt-get update -qq; then
        rm -f "$list"
        warn "The Docker repository is unreachable — Compose stays unavailable (Docker itself works)"
        return 1
    fi
    return 0
}

# Docker Compose (v2) is required for the Compose tab and every catalog app
# (`docker compose up -d`). Preference order, most local first:
#   1. whatever already answers `docker compose version` (host installed);
#   2. `docker-compose-v2` — Ubuntu 24.04+ / Debian 13+ ship it in the base
#      repositories;
#   3. `docker-compose-plugin` — Docker's official repo. Debian/Ubuntu base
#      repositories never carry it, hence the repository added below.
# The plugin installs `/usr/libexec/docker/cli-plugins/docker-compose`, which
# the `docker.io` CLI (>= 20.10) does find — no need for Docker's own engine.
if command -v docker &> /dev/null && ! docker compose version &> /dev/null; then
    if pkg_available docker-compose-v2; then
        install_optional docker-compose-v2 "Docker Compose"
    elif pkg_available docker-compose-plugin; then
        install_optional docker-compose-plugin "Docker Compose"
    elif add_docker_repo; then
        install_optional docker-compose-plugin "Docker Compose"
    fi
    if ! docker compose version &> /dev/null; then
        warn "Docker Compose is not available — the Compose tab and the app catalog stay disabled"
    fi
fi

# Buildx: `docker compose build` and `docker build` delegate to it. The bundled
# catalog only pulls prebuilt images, but a community catalog store may ship a
# compose file with a `build:` section.
if command -v docker &> /dev/null; then
    for buildx_pkg in docker-buildx-plugin docker-buildx; do
        if pkg_available "$buildx_pkg"; then
            install_optional "$buildx_pkg" "Docker Buildx (compose build)"
            break
        fi
    done
    unset buildx_pkg
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
    # Same rule as the Docker repository: a repository that cannot be reached
    # must not abort the install, and must not be left behind — a broken list
    # file breaks every later `apt-get update` on the host.
    if ! apt-get update -qq; then
        rm -f /etc/apt/sources.list.d/incus-backports.list \
              /etc/apt/sources.list.d/zabbly-incus-stable.list
        warn "The Incus repository is unreachable — VMs will stay disabled"
    fi
}

# A freshly installed Incus daemon starts EMPTY: no storage pool and a
# `default` profile without a NIC — the very first vm/create would fail with
# "No storage pool found". The VM app inherits the default profile's NIC
# (src/incus.rs), so provisioning that baseline non-interactively here makes
# VMs work out of the box. Idempotent: an initialized daemon is detected via
# its storage pool list and left untouched; failures are non-fatal (the VM
# app then shows its "Incus unavailable" state until init is run).
init_incus_daemon() {
    # `enable --now` returns once the unit is started, but a slow host can
    # still be a moment away from answering — a false "not active" here would
    # skip the init and leave the VM app on "No storage pool found".
    local incus_active=0
    for _ in $(seq 1 10); do
        if systemctl is-active --quiet incus; then incus_active=1; break; fi
        sleep 1
    done
    [[ "$incus_active" -eq 1 ]] || return 0
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
    if [[ "${HV_SKIP_OPTIONAL:-0}" == "1" ]]; then
        info "Skipping Incus (HV_SKIP_OPTIONAL=1)"
        return 0
    fi
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
#
# Turning the appliance's *own* agent on (Settings → Monitoring → SNMP agent,
# `src/snmp_agent.rs`) writes a second, separate drop-in —
# `99-homevault-agent.conf` — which `snmpd` reads after this one and which
# carries the agent's own `agentaddress`, community and view. Both files can
# coexist: this one widens the view, that one decides who may ask. Nothing here
# needs to change when the agent is enabled.
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
# exFAT comes in two halves and the package split differs by release:
#   * mounting — `exfat-fuse` (FUSE helper, still useful on kernels older
#     than 5.7);
#   * formatting — `mkfs.exfat`, which the Explorer's "format as exfat" runs.
#     Debian 12+ / Ubuntu 23.04+ ship it in `exfatprogs`; older releases in
#     `exfat-utils`. Installing only `exfat-fuse` (what this script used to
#     do) left the format action dead on current releases with no diagnostic.
install_optional exfat-fuse "exFAT mount helper (USB drives / SD cards)"
if pkg_available exfatprogs; then
    install_optional exfatprogs "exFAT formatting (mkfs.exfat)"
elif pkg_available exfat-utils; then
    install_optional exfat-utils "exFAT formatting (mkfs.exfat)"
else
    warn "No exFAT formatting tool available (exfatprogs / exfat-utils) — Explorer cannot format exFAT drives"
fi

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
# Ollama's official installer downloads `ollama-linux-<arch>.tar.zst` and
# aborts with "This version requires zstd for extraction" when `zstd` is not
# installed. `zstd` is Priority: optional, so a fresh Debian/Ubuntu does not
# have it — without this line the AI assistant silently stayed offline on
# every new host. Installed as an option because Ollama itself is optional.
install_optional zstd "zstd (required by the Ollama installer)"

# rest-server — the append-only restic backend HomeVault exposes as a backup
# destination (Backup → Destinations, docs/guides/backup-target.md). It is not
# an apt package: fetch the upstream release for this architecture, best-effort
# like Ollama. Without it the destination role simply reports
# "rest-server is not installed" and stays off; every other backup path keeps
# working.
install_rest_server() {
    if [[ "${HV_SKIP_OPTIONAL:-0}" == "1" ]]; then
        info "Skipping rest-server (HV_SKIP_OPTIONAL=1)"
        return 0
    fi
    if command -v rest-server &> /dev/null; then
        ok "rest-server (backup destination) already installed"
        return 0
    fi
    local arch
    case "$(uname -m)" in
        x86_64)  arch="linux_amd64" ;;
        aarch64) arch="linux_arm64" ;;
        armv7l)  arch="linux_armv7" ;;
        *)       warn "rest-server has no build for $(uname -m) — the backup destination role stays off."
                 return 0 ;;
    esac
    # Pinned release: a moving "latest" asset name cannot be fetched by URL.
    local version="0.14.0"
    local url="https://github.com/restic/rest-server/releases/download/v${version}/rest-server_${version}_${arch}.tar.gz"
    local tmp
    tmp="$(mktemp -d)"
    if curl -fsSL "$url" -o "$tmp/rest-server.tar.gz" 2>/dev/null \
        && tar -xzf "$tmp/rest-server.tar.gz" -C "$tmp" 2>/dev/null \
        && install -m 0755 "$(find "$tmp" -type f -name rest-server | head -n 1)" /usr/local/bin/rest-server 2>/dev/null; then
        ok "rest-server installed (backup destination)"
    else
        warn "rest-server not installed — HomeVault cannot receive restic backups. To add it later: download https://github.com/restic/rest-server/releases into /usr/local/bin."
    fi
    rm -rf "$tmp"
}
install_rest_server

install_ollama() {
    if [[ "${HV_SKIP_OPTIONAL:-0}" == "1" ]]; then
        info "Skipping Ollama (HV_SKIP_OPTIONAL=1)"
        return 0
    fi
    case "$(uname -m)" in
        armv7l|armv6l)
            warn "Ollama has no 32-bit ARM build — the AI assistant stays offline on $(uname -m)."
            info "Alternative: run an [OI]-compatible server (llama.cpp) elsewhere and point the app at it."
            return
            ;;
    esac

    if ! command -v zstd &> /dev/null; then
        warn "zstd is missing — Ollama's installer cannot unpack its release (apt-get install zstd). AI assistant disabled."
        return
    fi

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

# Enable per-user disk quotas on the filesystem hosting personal home folders
# (`/home/homevault`), so the Users page's "home folder size" slider actually
# limits anything (ROADMAP 3.1).
#
# Quota enforcement is per filesystem, and it needs three things that cannot be
# faked from the app later:
#   1. the mount carries `usrquota,grpquota` — a limit set on a mount without
#      it is stored by the kernel but never enforced;
#   2. the fstab entry carries the same options, or the next reboot silently
#      drops accounting;
#   3. the quota files exist and accounting is on (`quotacheck` + `quotaon`) —
#      `quotaon` state is volatile, so `src/quota.rs::activate_all` re-arms it
#      at every start.
# Runs on `/` on a stock install (no separate /home), which is the common case;
# a separate /home array is recognised by `findmnt --target`. Filesystems that
# cannot do per-user quotas (btrfs, exfat, overlay…) are reported and skipped:
# the UI then shows the limit as "not enforced" with the reason, never as
# applied. Best-effort like every other helper here — a host without quota
# tooling still boots and runs.
setup_home_quota() {
    local home_root="/home/homevault"
    local fstab="/etc/fstab"

    if ! command -v quotaon &> /dev/null || ! command -v quotacheck &> /dev/null; then
        warn "Disk quotas skipped — quota-tools (quotaon/quotacheck) not installed"
        return 0
    fi

    # One field per `findmnt` call: a mount point or source containing a space
    # would otherwise be split by `read`.
    local target fstype
    target=$(findmnt -n -o TARGET --target "$home_root" 2>/dev/null | head -n1)
    fstype=$(findmnt -n -o FSTYPE --target "$home_root" 2>/dev/null | head -n1)
    if [[ -z "$target" || -z "$fstype" ]]; then
        warn "Disk quotas skipped — could not locate the filesystem hosting $home_root"
        return 0
    fi

    local want_opts
    case "$fstype" in
        ext2|ext3|ext4)
            want_opts="usrquota,grpquota"
            ;;
        xfs)
            # XFS has no quota files: the mount option alone turns accounting
            # on, so `quotacheck` must not be run there.
            want_opts="usrquota"
            ;;
        zfs)
            info "Home folders live on ZFS ($target) — per-user limits use the native userquota property, nothing to enable here"
            return 0
            ;;
        *)
            info "Home folders live on '$fstype' ($target) — per-user disk quotas are not supported there; the Users page reports any limit as not enforced"
            return 0
            ;;
    esac

    if findmnt -n -o OPTIONS --target "$home_root" 2>/dev/null | grep -q 'usrquota'; then
        ok "Disk quota accounting already enabled on $target ($fstype)"
    else
        info "Enabling disk quota accounting on $target ($fstype)"
        # Add the options to the fstab line that mounts this filesystem, if it
        # does not carry them yet. Written through `cat` (not `mv`) so the
        # file keeps its inode, owner and mode — a replaced /etc/fstab with
        # the wrong permissions is a boot hazard.
        cp -f "$fstab" "$fstab.homevault.bak" 2>/dev/null || true
        awk -v mp="$target" -v want="$want_opts" '
            /^[[:space:]]*#/ { print; next }
            NF >= 4 && $2 == mp {
                u = 0; g = 0; add = ""
                n = split($4, a, ",")
                for (i = 1; i <= n; i++) { if (a[i] == "usrquota" || a[i] == "quota") u = 1; if (a[i] == "grpquota") g = 1 }
                if (!u) add = add ",usrquota"
                if (want ~ /grpquota/ && !g) add = add ",grpquota"
                $4 = $4 add
            }
            { print }
        ' "$fstab" > "$fstab.homevault.new" 2>/dev/null
        if [[ -s "$fstab.homevault.new" ]]; then
            cat "$fstab.homevault.new" > "$fstab"
        fi
        rm -f "$fstab.homevault.new"

        # A malformed fstab breaks the next boot: verify exactly like mount
        # would, and roll back instead of keeping a broken entry.
        if ! findmnt --verify --fstab &> /dev/null; then
            if [[ -f "$fstab.homevault.bak" ]]; then
                cat "$fstab.homevault.bak" > "$fstab"
            fi
            warn "Disk quotas skipped — the edited /etc/fstab failed validation and was rolled back"
            return 0
        fi
        rm -f "$fstab.homevault.bak"

        # Apply on the live mount so quotas work before the next reboot.
        # Remounting keeps the existing flags (rw, errors=remount-ro…).
        if ! mount -o "remount,$want_opts" "$target" 2>/dev/null; then
            warn "Disk quotas skipped — the live remount of $target with $want_opts failed (the fstab entry is ready for the next boot)"
            return 0
        fi
        ok "Quota mount options applied to $target"
        # No fstab line for this mount (a manually mounted volume, a systemd
        # .mount unit, a bind…): accounting works now but not after a reboot.
        if ! findmnt --fstab -n -o OPTIONS --target "$target" 2>/dev/null | grep -q 'usrquota'; then
            warn "No /etc/fstab entry carries usrquota for $target — quota accounting will be lost on reboot"
        fi
    fi

    if [[ "$fstype" != "xfs" ]]; then
        # Build the quota files once; `-m` avoids the historical read-only
        # remount and `-f` is required on a mounted filesystem.
        if [[ ! -f "$target/aquota.user" ]]; then
            info "Building quota files on $target (one-off scan of existing usage)..."
            if ! quotacheck -cugm -f "$target" &> /dev/null; then
                warn "Disk quotas skipped — quotacheck failed on $target"
                return 0
            fi
        fi
        if ! quotaon -ug "$target" &> /dev/null; then
            warn "Disk quotas skipped — quotaon failed on $target"
            return 0
        fi
    fi
    ok "Per-user disk quotas active on $target ($fstype)"
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

# --- Signed manifest (P2-8) -------------------------------------------------
# Verified *before* the download so a tampered manifest cannot point us at an
# attacker-controlled archive, and so the manifest's URL/sha256/size become the
# only accepted values.
MANIFEST_WORK=""
MANIFEST_VERSION=""
MANIFEST_EXPECTED_SIZE=""
if [[ -n "$RELEASE_MANIFEST" ]]; then
    MANIFEST_WORK=$(mktemp -d /tmp/homevault-manifest.XXXXXX)
    if ! hv_load_manifest "$RELEASE_MANIFEST" "$RELEASE_SIGNATURE" \
            "$RELEASE_PUBKEY" "$MANIFEST_WORK"; then
        rm -rf "$MANIFEST_WORK"
        err "Update refused: the signed manifest is invalid."
        echo -e "  A manifest must be signed by the release key, and the key must be trusted."
        exit 1
    fi
    if [[ -n "$HV_MANIFEST_ARCH" && "$HV_MANIFEST_ARCH" != "$ARCH" ]]; then
        rm -rf "$MANIFEST_WORK"
        err "Update refused: the manifest is for '$HV_MANIFEST_ARCH', this host is '$ARCH'."
        exit 1
    fi
    # A signed manifest that pins no digest only authenticates its own fields,
    # not the bytes at the URL — refuse it.
    if [[ -z "$HV_MANIFEST_SHA256" ]]; then
        rm -rf "$MANIFEST_WORK"
        err "Update refused: the signed manifest does not pin a SHA-256."
        exit 1
    fi
    if [[ -z "$RELEASE_URL" && -n "$HV_MANIFEST_URL" ]]; then
        RELEASE_URL="$HV_MANIFEST_URL"
    fi
    if [[ -z "$RELEASE_SHA256" && -n "$HV_MANIFEST_SHA256" ]]; then
        RELEASE_SHA256="$HV_MANIFEST_SHA256"
    fi
    MANIFEST_VERSION="$HV_MANIFEST_VERSION"
    MANIFEST_EXPECTED_SIZE="$HV_MANIFEST_SIZE"
    ok "Signed manifest verified (version $MANIFEST_VERSION)"
elif [[ -n "$RELEASE_SHA256" ]]; then
    info "No signed manifest supplied — relying on the pinned SHA-256."
else
    warn "No signed manifest and no pinned checksum: this download is not authenticated."
    echo -e "  For a verified release, pass ${C_BOLD}HV_RELEASE_MANIFEST=<url>${C_RESET}."
fi

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
    CURL_AUTH=()
    if [[ -n "$RELEASE_TOKEN" ]]; then
        CURL_AUTH=(-H "Authorization: Bearer $RELEASE_TOKEN")
    fi
    if ! curl -fL --retry 3 --retry-delay 2 "${CURL_AUTH[@]}" -o "$PACKAGE" "$RELEASE_URL"; then
        err "Failed to download HomeVault from $RELEASE_URL"
        # A private GitHub repository answers 404 to an anonymous request, so
        # "not found" here usually means "not authorised", not "wrong URL".
        if [[ "$RELEASE_URL" == *github.com* ]]; then
            echo -e "  If the project is private, the release needs a token:"
            echo -e "    ${C_BOLD}sudo HV_RELEASE_TOKEN=<github-token> bash -c \"\$(curl -fsSL <install-url>)\"${C_RESET}"
            echo -e "  The token only needs read access to the repository's releases."
        fi
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

# Integrity check. A signed manifest pins both the size and the SHA-256; the
# legacy path only pins the SHA-256 when the admin supplied one.
if [[ -n "$MANIFEST_VERSION" ]]; then
    if ! hv_verify_archive "$PACKAGE" "$RELEASE_SHA256" "$MANIFEST_EXPECTED_SIZE"; then
        rm -f "$PACKAGE"
        [[ -n "$MANIFEST_WORK" ]] && rm -rf "$MANIFEST_WORK"
        err "Update refused: the archive does not match its signed manifest."
        exit 1
    fi
    ok "Archive matches the signed manifest (SHA-256 + size)"
elif [[ -n "$RELEASE_SHA256" ]]; then
    ACTUAL_SHA=$(sha256sum "$PACKAGE" | awk '{print $1}')
    if [[ "${ACTUAL_SHA,,}" != "${RELEASE_SHA256,,}" ]]; then
        err "SHA-256 mismatch for the downloaded archive."
        echo -e "  expected: $RELEASE_SHA256"
        echo -e "  actual:   $ACTUAL_SHA"
        rm -f "$PACKAGE"
        exit 1
    fi
    ok "Archive checksum verified"
fi

UPGRADE=0
CURRENT_VERSION=""
if [[ -f "$SERVICE_FILE" ]]; then
    UPGRADE=1
    info "Existing installation detected — updating the application (your data is kept)"
    if [[ -x "$INSTALL_DIR/homevault" ]]; then
        # `timeout` matters when the *installed* build predates the
        # `--version` flag (added 2026-11-08): an unknown flag used to start
        # the server, which would hang the installer here.
        CURRENT_VERSION=$(timeout 5 "$INSTALL_DIR/homevault" --version 2>/dev/null \
            | awk '{print $NF}' || true)
    fi
fi

# Extract to a staging directory, never over the live binary. The old
# `tar -xzf … -C $INSTALL_DIR` truncated the running executable in place.
STAGE_DIR=$(mktemp -d /tmp/homevault-stage.XXXXXX)
if ! tar -xzf "$PACKAGE" -C "$STAGE_DIR"; then
    rm -rf "$STAGE_DIR"
    [[ -n "$MANIFEST_WORK" ]] && rm -rf "$MANIFEST_WORK"
    err "Could not extract the update archive."
    exit 1
fi
if [[ ! -x "$STAGE_DIR/homevault" ]]; then
    rm -rf "$STAGE_DIR"
    [[ -n "$MANIFEST_WORK" ]] && rm -rf "$MANIFEST_WORK"
    err "The archive does not contain an executable 'homevault'."
    exit 1
fi
if ! hv_smoke_binary "$STAGE_DIR/homevault" "$MANIFEST_VERSION"; then
    rm -rf "$STAGE_DIR"
    [[ -n "$MANIFEST_WORK" ]] && rm -rf "$MANIFEST_WORK"
    err "The staged binary failed its smoke test — nothing was changed."
    exit 1
fi
ok "Staged build $HV_STAGED_VERSION passed its smoke test"

# Idempotence: a signed manifest older than or equal to the running build is a
# no-op, not a downgrade.
if [[ "$UPGRADE" -eq 1 && -n "$MANIFEST_VERSION" && -n "$CURRENT_VERSION" ]]; then
    if hv_version_ge "$CURRENT_VERSION" "$MANIFEST_VERSION"; then
        ok "Already at $CURRENT_VERSION; the signed manifest is not newer. Nothing to do."
        rm -rf "$STAGE_DIR"
        [[ -n "$MANIFEST_WORK" ]] && rm -rf "$MANIFEST_WORK"
        [[ -n "${TMP_ARCHIVE:-}" && "$PACKAGE" == "$TMP_ARCHIVE" ]] && rm -f "$PACKAGE"
        exit 0
    fi
fi

if [[ "$UPGRADE" -eq 1 ]]; then
    if ! hv_swap_binary "$STAGE_DIR/homevault" "$INSTALL_DIR"; then
        rm -rf "$STAGE_DIR"
        [[ -n "$MANIFEST_WORK" ]] && rm -rf "$MANIFEST_WORK"
        err "Atomic swap failed — the running build was left untouched."
        exit 1
    fi
    ok "New build swapped in atomically (previous build kept for rollback)"
else
    install -m 0755 "$STAGE_DIR/homevault" "$INSTALL_DIR/homevault"
    ok "Application installed to $INSTALL_DIR"
fi
rm -rf "$STAGE_DIR"
[[ -n "$MANIFEST_WORK" ]] && rm -rf "$MANIFEST_WORK"

# Optional non-root deployment: create the service account, hand it the state
# it writes directly, and let the binary render its own scoped sudoers file.
# The binary stays root-owned (the service must not be able to rewrite
# itself); it only needs read/execute.
if [[ "$RUN_USER" == "homevault" ]]; then
    if ! getent group homevault >/dev/null 2>&1; then
        groupadd --system homevault
    fi
    if ! id homevault >/dev/null 2>&1; then
        useradd --system --gid homevault --home-dir "$DATA_DIR" \
                --shell /usr/sbin/nologin homevault
    fi
    # Device access the appliance already relied on when it ran as root.
    for grp in docker plugdev storage cdrom nut incus incus-admin; do
        if getent group "$grp" >/dev/null 2>&1; then
            usermod -a -G "$grp" homevault
        fi
    done
    chmod -R a+rX "$INSTALL_DIR"
    chown -R "$RUN_USER:$RUN_GROUP" /etc/homevault /etc/homevault-rsync /etc/homevault-backup
    chown "$RUN_USER:$RUN_GROUP" "$DATA_DIR" "$DATA_DIR/apps"
    for sub in data compose rescue syncthing logs quarantine vm; do
        chown -R "$RUN_USER:$RUN_GROUP" "$DATA_DIR/$sub"
    done
    chown "$RUN_USER:$RUN_GROUP" /home/homevault
    # One source of truth for the grants: the same renderer the unit tests
    # assert on, executed here as root before the unprivileged unit starts.
    if ! DATA_DIRECTORY="$DATA_DIR" "$INSTALL_DIR/homevault" --install-sudoers; then
        err "Could not write /etc/sudoers.d/homevault — refusing to start an unprivileged unit without its grants."
        exit 1
    fi
    ok "Non-root service account ready (homevault + scoped sudoers)"
else
    # A previous non-root install upgraded with the default root mode must not
    # keep a stale sudoers file granting the old account.
    rm -f /etc/sudoers.d/homevault
fi

# A downloaded archive was only needed at install time — free the disk space.
if [[ -n "${TMP_ARCHIVE:-}" ]] && [[ "$PACKAGE" == "$TMP_ARCHIVE" ]]; then
    rm -f "$PACKAGE"
fi

if [[ -f "$SERVICE_FILE" ]]; then
    cp "$SERVICE_FILE" "${SERVICE_FILE}.bak" 2>/dev/null || true
fi

# Systemd unit — written inline so this single script is self-contained (there
# is no sibling `homevault.service` to fall back to). `User=`/`Group=` are
# always written (`root` by default, `homevault` with HV_SERVICE_USER=
# homevault) so the deployment model is explicit in `systemctl cat` instead of
# being implied by an absent directive.
SERVICE_SUPP=""
if getent group docker >/dev/null 2>&1; then
    SERVICE_SUPP="SupplementaryGroups=docker"
fi
render_service_unit "$RUN_USER" "$RUN_GROUP" "$SERVICE_SUPP" "$RUN_HOME" > "$SERVICE_FILE"
ok "System service unit written to $SERVICE_FILE"

# Signed update engine (P2-8, UI half). Two pieces, both root-owned:
#   * the installer itself is copied under INSTALL_DIR, because a
#     `curl | bash` install otherwise leaves no script on disk — and the
#     Settings button, the unit's ConditionPathExists and this copy must all
#     agree on one path;
#   * a oneshot unit whose ExecStart is the *fixed* argv `homevault
#     --update-engine`. The web handler can only start this unit (one sudoers
#     entry, no parameters); the engine reads data/updates/request.json and
#     runs this script, which still refuses an unsigned manifest before
#     touching anything.
mkdir -p "$INSTALL_DIR/installer"
if [[ -n "${HV_SELF:-}" && -f "${HV_SELF:-}" ]]; then
    install -m 0755 -o root -g root "$HV_SELF" "$INSTALL_DIR/installer/install.sh" 2>/dev/null \
        || cp -f "$HV_SELF" "$INSTALL_DIR/installer/install.sh" 2>/dev/null || true
    if [[ -f "$INSTALL_DIR/installer/install.sh" ]]; then
        ok "Update engine installed at $INSTALL_DIR/installer/install.sh"
    else
        warn "Could not copy the installer — the in-app update button stays disabled"
    fi
else
    warn "Installer path unknown — the in-app update button stays disabled"
fi

render_update_unit "$INSTALL_DIR" > /etc/systemd/system/homevault-update.service
chmod 0644 /etc/systemd/system/homevault-update.service
ok "Update engine unit written to /etc/systemd/system/homevault-update.service"

# Kernel stability settings (P1-28). Writing the file is always safe; applying
# it is not (a container or a locked-down kernel may refuse), so a failure is a
# warning, not an aborted install. HV_SKIP_SYSCTL=1 skips the whole block.
if [[ "${HV_SKIP_SYSCTL:-0}" != "1" ]]; then
    mkdir -p /etc/sysctl.d
    render_sysctl > /etc/sysctl.d/99-homevault.conf
    chmod 0644 /etc/sysctl.d/99-homevault.conf
    if command -v sysctl >/dev/null 2>&1; then
        if sysctl -q -p /etc/sysctl.d/99-homevault.conf >/dev/null 2>&1; then
            ok "Kernel stability settings applied from /etc/sysctl.d/99-homevault.conf"
        else
            warn "Kernel stability settings written but not applied (read-only /proc/sys?)"
        fi
    else
        warn "sysctl is not installed — kernel stability settings will apply at the next boot"
    fi
fi

# Host monitoring daemons (P1-28). HomeVault samples SMART and RAID itself, so
# smartd/mdmonitor are opt-in: running both would duplicate every alert. The
# decision is explicit and printed, never a silent enable or disable.
for _hv_mon in smartd mdmonitor; do
    case "$_hv_mon" in
        smartd)    _hv_req="${HV_SMARTD:-0}"; _hv_var="HV_SMARTD" ;;
        mdmonitor) _hv_req="${HV_MDMONITOR:-0}"; _hv_var="HV_MDMONITOR" ;;
    esac
    _hv_avail="n"
    if systemctl list-unit-files "${_hv_mon}.service" 2>/dev/null | grep -q "^${_hv_mon}.service"; then
        _hv_avail="y"
    fi
    _hv_enabled="n"
    if systemctl is-enabled --quiet "$_hv_mon" 2>/dev/null; then
        _hv_enabled="y"
    fi
    case "$(monitor_decision "$_hv_avail" "$_hv_enabled" "$_hv_req")" in
        enable)
            if systemctl enable --now "$_hv_mon" >/dev/null 2>&1; then
                ok "$_hv_mon enabled and started on request — HomeVault still samples in parallel"
            else
                warn "Could not enable $_hv_mon; HomeVault's own monitoring remains active"
            fi
            ;;
        already)
            warn "$_hv_mon is already enabled: HomeVault also samples. Disable one to avoid duplicate alerts."
            ;;
        unavailable)
            warn "$_hv_mon is not installed; HomeVault's own monitoring is the only one"
            ;;
        *)
            ok "$_hv_mon left disabled (HomeVault monitors); set $_hv_var=1 to use it instead"
            ;;
    esac
done
unset _hv_mon _hv_req _hv_var _hv_avail _hv_enabled

# Catalog trust anchor (P1-27). HV_CATALOG_PUBKEY may be the inline key
# (hex/base64) or a path to it; either way it is installed at the path the
# binary checks by default, so the service environment does not have to carry
# it. No key = the historical unsigned behaviour (stores are listed as
# "Unsigned"); HV_REQUIRE_SIGNED_STORES=1 makes the key mandatory at runtime.
if [[ -n "${HV_CATALOG_PUBKEY:-}" ]]; then
    mkdir -p /usr/local/lib/homevault
    if [[ -f "$HV_CATALOG_PUBKEY" ]]; then
        install -m 0644 -o root -g root "$HV_CATALOG_PUBKEY" \
            /usr/local/lib/homevault/catalog-key.ed25519 2>/dev/null \
            && ok "Catalog public key installed at /usr/local/lib/homevault/catalog-key.ed25519" \
            || warn "Could not install the catalog public key — community stores stay unsigned"
    else
        printf '%s\n' "$HV_CATALOG_PUBKEY" > /usr/local/lib/homevault/catalog-key.ed25519 \
            && chmod 0644 /usr/local/lib/homevault/catalog-key.ed25519 \
            && ok "Catalog public key written to /usr/local/lib/homevault/catalog-key.ed25519" \
            || warn "Could not write the catalog public key — community stores stay unsigned"
    fi
fi

# Optional AppArmor confinement (P2-7). Opt-in: a wrong profile can block a
# disk, network or Samba operation, so compliance is never claimed silently.
# When requested but impossible, warn and leave the unit exactly as it was.
if [[ "${HV_APPARMOR:-0}" == "1" ]]; then
    APPARMOR_ENABLED="N"
    if [[ -r /sys/module/apparmor/parameters/enabled ]]; then
        APPARMOR_ENABLED=$(cat /sys/module/apparmor/parameters/enabled 2>/dev/null || echo N)
    fi
    if [[ "$APPARMOR_ENABLED" == "Y" ]] && command -v apparmor_parser >/dev/null 2>&1; then
        mkdir -p /etc/apparmor.d
        render_apparmor_profile > /etc/apparmor.d/homevault
        if apparmor_parser -r /etc/apparmor.d/homevault 2>/dev/null; then
            APPARMOR_MODE="complain"
            if [[ "${HV_APPARMOR_ENFORCE:-0}" == "1" ]]; then APPARMOR_MODE="enforce"; fi
            ok "AppArmor profile loaded in $APPARMOR_MODE mode (/etc/apparmor.d/homevault)"
            echo -e "  Validate it with ${C_BOLD}dmesg | grep -i apparmor${C_RESET}, then re-run with "
            echo -e "  ${C_BOLD}HV_APPARMOR=1 HV_APPARMOR_ENFORCE=1${C_RESET} to enforce."
        else
            warn "AppArmor profile written but not loaded — check apparmor_parser output"
        fi
    else
        warn "HV_APPARMOR=1 ignored: AppArmor is not enabled on this kernel (or apparmor_parser is missing)"
    fi
fi

# Optional: turn on built-in HTTPS with a self-signed certificate. Opt-in and
# only offered on a fresh install; can also be toggled later from
# Settings > Security > HTTPS/TLS. The plain-HTTP port keeps working.
if [[ "$UPGRADE" -eq 0 ]]; then
    # Default YES: the wizard and the login form carry the administrator
    # password, and over plain HTTP it crosses the network in clear. The cost is
    # a one-time browser warning about the self-signed certificate.
    ask "Enable HTTPS with a self-signed certificate?\n\nThe browser will warn about the self-signed certificate once, but the\nadministrator password then crosses the LAN encrypted.\n\nEnable HTTPS? [Y/n]" y
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

# Arm per-user home-directory quotas before the service starts, so the first
# "home folder size" save is enforced instead of silently degrading.
setup_home_quota

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
# Bandwidth control needs `tc` from the same package; `ip` alone does not
# imply it on a minimal image.
check "Bandwidth control (tc)"         "command -v tc"
# iSCSI targets need targetcli (python3-rtslib) plus the LIO modules.
check "iSCSI targets (targetcli)"      "command -v targetcli"
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
    # A CLI without a reachable daemon looks installed but every Docker page
    # fails — `docker info` is the only check that proves the engine answers.
    check "Docker daemon"              "docker info >/dev/null 2>&1"
    check "Docker Compose"             "docker compose version"
    check "Docker Buildx"              "docker buildx version"
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
# The home-size slider needs all four tools, not just the reporter: a missing
# `setquota`/`quotaon` means limits are stored but never enforced.
check "Disk quotas (quota-tools)"      "command -v repquota && command -v setquota && command -v quotaon && command -v quotacheck"
check "Disk spindown (hdparm)"         "command -v hdparm"
check "XFS formatting (mkfs.xfs)"      "command -v mkfs.xfs"
check "exFAT formatting (mkfs.exfat)"  "command -v mkfs.exfat"
check "Wi-Fi management (wpa_cli)"     "command -v wpa_cli"
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
check "mDNS browsing (avahi-browse)"   "command -v avahi-browse"
check "SMB discovery (smbtree)"        "command -v smbtree"
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
if [[ "$RUN_USER" == "homevault" ]]; then
    if id homevault >/dev/null 2>&1; then
        ok "Service account homevault exists"
    else
        VFAIL=1
        err "Service account homevault is missing"
    fi
    if [[ -f /etc/sudoers.d/homevault ]] \
        && visudo -c -f /etc/sudoers.d/homevault >/dev/null 2>&1; then
        ok "Scoped sudoers file /etc/sudoers.d/homevault is valid"
    else
        VFAIL=1
        err "Scoped sudoers file is missing or invalid — privileged features will fail"
    fi
fi
if systemctl is-active --quiet "$SERVICE_NAME"; then
    ok "Service $SERVICE_NAME started"
else
    VFAIL=1
    err "The service $SERVICE_NAME did not start."
    echo -e "  See: ${C_BOLD}journalctl -u $SERVICE_NAME -e${C_RESET}"
fi

# P2-7: report the measured exposure instead of claiming a level. The number
# comes from systemd itself, so an operator can compare hosts and releases.
if command -v systemd-analyze >/dev/null 2>&1; then
    HV_EXPOSURE=$(systemd-analyze security --no-pager "$SERVICE_NAME" 2>/dev/null \
        | tail -n 1 | awk '{print $NF}')
    if [[ -n "$HV_EXPOSURE" ]]; then
        info "systemd hardening exposure for $SERVICE_NAME: $HV_EXPOSURE"
        echo -e "  ${C_DIM}Lower is better; review with: systemd-analyze security $SERVICE_NAME${C_RESET}"
    fi
fi

HV_READY=0
HV_ROLLED_BACK=0
info "Waiting for the server to respond..."
if hv_wait_healthy "http://127.0.0.1:${PORT}/api/auth/setup-status" 20; then
    HV_READY=1
    ok "HomeVault is responding on port $PORT"
elif [[ "$UPGRADE" -eq 1 && -x "$INSTALL_DIR/homevault.previous" ]]; then
    # Automatic rollback (P2-8): an update that does not become ready is not a
    # successful update. Restore the previous build and prove it starts again.
    warn "The new build did not answer within 20 s — rolling back automatically."
    systemctl stop "$SERVICE_NAME" >/dev/null 2>&1 || true
    if hv_rollback_binary "$INSTALL_DIR"; then
        systemctl start "$SERVICE_NAME" >/dev/null 2>&1 || true
        if hv_wait_healthy "http://127.0.0.1:${PORT}/api/auth/setup-status" 20; then
            HV_READY=1
            HV_ROLLED_BACK=1
        fi
    fi
    if [[ "$HV_ROLLED_BACK" -eq 1 ]]; then
        warn "Rolled back to ${CURRENT_VERSION:-the previous build}. The failed build is at $INSTALL_DIR/homevault.failed."
        hv_write_result "$DATA_DIR" "rolled_back" \
            "${MANIFEST_VERSION:-$HV_STAGED_VERSION}" "${CURRENT_VERSION:-unknown}" \
            "health check failed after update; previous build restored"
    else
        VFAIL=1
        err "The update failed and the automatic rollback could not bring the service back."
        echo -e "  See: ${C_BOLD}journalctl -u $SERVICE_NAME -e${C_RESET}"
        hv_write_result "$DATA_DIR" "failed" \
            "${MANIFEST_VERSION:-$HV_STAGED_VERSION}" "${CURRENT_VERSION:-unknown}" \
            "health check failed and rollback failed"
    fi
else
    VFAIL=1
    err "HomeVault is still not responding after 20 seconds."
    echo -e "  See: ${C_BOLD}journalctl -u $SERVICE_NAME -e${C_RESET}"
    hv_write_result "$DATA_DIR" "failed" \
        "${MANIFEST_VERSION:-$HV_STAGED_VERSION}" "${CURRENT_VERSION:-unknown}" \
        "service did not become ready"
fi

# A healthy upgrade is recorded so Settings can show the last update result.
if [[ "$UPGRADE" -eq 1 && "$HV_READY" -eq 1 && "$HV_ROLLED_BACK" -eq 0 ]]; then
    hv_write_result "$DATA_DIR" "succeeded" "$HV_STAGED_VERSION" \
        "${CURRENT_VERSION:-unknown}" "update applied and health check passed"
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
if [[ "${HV_ROLLED_BACK:-0}" -eq 1 ]]; then
    echo -e "${C_BOLD}${C_YELLOW}The update did not become ready and was rolled back automatically.${C_RESET}"
    echo -e "  HomeVault is running ${CURRENT_VERSION:-the previous build}. The failed build is"
    echo -e "  kept at ${C_BOLD}$INSTALL_DIR/homevault.failed${C_RESET} for inspection."
elif [[ "$HV_READY" -eq 1 ]] && [[ "$VFAIL" -eq 0 ]]; then
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

# The TUI shows the outcome in a dialog: in that mode the block above only
# wrote to $LOG_FILE, and the terminal belongs to whiptail.
if [[ "$TUI" -eq 1 ]]; then
    if [[ "$HV_READY" -eq 1 ]] && [[ "$VFAIL" -eq 0 ]]; then
        ui_msg "Installation complete" "HomeVault is installed and running.

Open it from any device on your network:

  http://${LAN_IP:-127.0.0.1}:${PORT}

Next: create the administrator account in the browser.

Full log: ${LOG_FILE}"
    elif [[ "$HV_READY" -eq 1 ]]; then
        ui_msg "Installed with warnings" "HomeVault is installed and running, but some optional components are
missing. The features that depend on them are disabled; everything else
works.

Open it from any device on your network:

  http://${LAN_IP:-127.0.0.1}:${PORT}

Details: ${LOG_FILE}"
    else
        ui_msg "Installed, not responding yet" "HomeVault is installed but the server is not answering yet.

Check the service before opening the page:

  journalctl -u ${SERVICE_NAME} -e

Full log: ${LOG_FILE}"
    fi
fi

if [[ -n "${DISPLAY:-}${WAYLAND_DISPLAY:-}" ]]; then
    ask "Open HomeVault in the browser now?\n\n  http://${LAN_IP:-127.0.0.1}:${PORT}" y
    case "$REPLY" in
        [nN]*) ;;
        *)
            OPENER=""
            for cand in xdg-open sensible-browser gio gnome-open kde-open; do
                if command -v "$cand" &> /dev/null; then OPENER="$cand"; break; fi
            done
            if [ -n "$OPENER" ]; then
                # Run as the user who invoked sudo, not root, so the browser
                # opens in their actual desktop session. The URL follows the
                # TLS choice made a few lines above.
                HV_SCHEME="http"
                if grep -qs '^TLS_ENABLED=true' "$ENV_FILE" 2>/dev/null; then
                    HV_SCHEME="https"
                    HV_URL="${HV_SCHEME}://${LAN_IP:-127.0.0.1}:${HTTPS_PORT}"
                else
                    HV_URL="${HV_SCHEME}://${LAN_IP:-127.0.0.1}:${PORT}"
                fi
                sudo -u "${SUDO_USER:-root}" "$OPENER" "$HV_URL" &> /dev/null &
                ok "Opening $HV_URL..."
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
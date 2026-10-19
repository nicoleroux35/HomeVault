# HomeVault

[![CI](https://github.com/nicoleroux35/HomeVault/actions/workflows/ci.yml/badge.svg)](https://github.com/nicoleroux35/HomeVault/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![ko-fi](https://ko-fi.com/img/githubbutton_sm.svg)](https://ko-fi.com/M1R82932SP)

**Turn any small computer into your own private cloud.**

HomeVault is a self-hosted NAS you run yourself: one binary, one browser tab,
no cloud account, no subscription and no vendor lock-in. Install it on a small
PC, an old laptop, a Raspberry Pi or a mini-PC, add a disk, and your data —
documents, photos, videos, backups — is available from any device with a
browser, at home or anywhere else.

## Why HomeVault

- **A NAS for everyone.** No RAID controller, no dedicated appliance, no
  proprietary hardware: the machine you already own is enough.
- **Built for small configurations.** It runs on **1 CPU core, 1 GB of RAM and
  an 8 GB system disk**, with a typical resident footprint of 40–120 MB at
  rest — on `x86_64`, `aarch64` (Raspberry Pi 4/5, ARM mini-PCs) and
  `armv7l`. Every optional feature (antivirus, ZFS, Docker, AI) is off until
  you turn it on, so the base install stays light.
- **Your data, everywhere.** Any browser, any device — desktop, laptop,
  tablet, phone, work or home — over the local network or the internet.
  Nothing to install on the client.
- **Simple, reliable, robust.** One self-contained installer, one binary, an
  embedded SQLite database, a guided first-run wizard, and automated tests at
  every layer (Rust unit + integration, Playwright end-to-end against a real
  server).

Under the hood: a Rust (Axum) backend with an embedded SQLite database and a
Svelte 5 frontend rendered as a desktop-like windowing UI.

## Features

- **Dashboard**: monitor your server's activity (CPU,
  memory, disks, network, sensors).
- **Users and groups**: manage accounts, their permissions and their
  home folders.
- **Storage**: manage your disks, partitions and storage pools.
- **RAID**: create and monitor RAID volumes (mdadm).
- **Shares**: expose your files over SMB, NFS, FTP and WebDAV, with
  fine-grained permission management.
- **File explorer**: browse, search, upload and organize
  your files from the browser, with a built-in trash.
- **Backups**: schedule and restore backups with restic.
- **Cloud sync**: connect remote storage via rclone.
- **Network**: configure your interfaces and Wi-Fi, and diagnose the
  connection.
- **Security**: antivirus (ClamAV), anti-ransomware protection and firewall
  (ufw).
- **Docker and app catalog**: containers, Compose stacks and a
  one-click app catalog.
- **Virtual machines**: Incus VMs and containers, cloud images, console.
- **VPN**: WireGuard and OpenVPN, Syncthing sync, aria2
  downloader.
- **AI assistant**: a local chat that runs on the appliance itself — no
  provider, no API key, nothing leaves the machine. It answers questions about
  HomeVault's own features from its knowledge base, and Settings proposes five
  models ranked for the host's RAM/GPU (from 1 GB to 12 GB).
- **MCP server**: expose HomeVault to an external agent (Claude, an IDE, a
  script) over the Model Context Protocol — apps, files, VMs and system
  actions, each behind a permission scope and a revocable token.
- **Health, journal and quotas**: system monitoring, event journal
  and transfer quotas.
- **Metrics and scheduled tasks**: historical performance charts (1 h /
  24 h / 7 d) and a generic cron engine (snapshots, SMART tests, power
  actions, commands).
- **Power management**: CPU governor, host reboot/shutdown/suspend and
  per-disk spindown.
- **Terminal**: access a secure terminal directly in the browser.
- **Settings**: customize the appearance, notifications, updates and
  more.

## Requirements

- **Debian 10+** or **Ubuntu 20.04+** (and derivatives such as
  Raspberry Pi OS 64-bit)
- **x86_64**, **aarch64** or **armv7l** architecture
- **1 CPU core**, **1 GB of RAM**, **8 GB of system disk** — plus the data
  disk(s) of your choice
- **root** (sudo) access for installation

Minimum and recommended hardware, the per-feature cost (ZFS, antivirus,
Docker, AI) and tested reference machines: see [`HARDWARE.md`](HARDWARE.md).

## Installation

Install HomeVault and all its dependencies with a single command.
Choose `curl` **or** `wget` according to your preference:

With `curl`:

```bash
curl -fsSL https://raw.githubusercontent.com/nicoleroux35/HomeVault/main/installer/install.sh | sudo bash
```

With `wget`:

```bash
wget -qO- https://raw.githubusercontent.com/nicoleroux35/HomeVault/main/installer/install.sh | sudo bash
```

> Adjust the branch (`main`) if needed. The script runs in **5 steps**:
>
> 1. **Step 1** — checks compatibility: `sudo`, distribution (Debian 10+
>    or Ubuntu 20.04+), architecture (`x86_64`, `aarch64`, `armv7l`) and disk
>    space.
> 2. **Step 2** — checks and then installs the system packages (essential
>    packages are blocking, optional packages degrade to a warning).
> 3. **Step 3** — downloads the pre-built binary (per-architecture asset
>    `homevault-<arch>.tar.gz` from the project's GitHub Releases), unpacks it
>    into `/opt/homevault`, writes the systemd service
>    (`/etc/systemd/system/homevault.service`) and configures the automount of
>    external disks.
> 4. **Step 4** — global verification: installed packages, services to
>    start, actual startup of HomeVault (HTTP response on the port) and
>    location of the application.
> 5. **Step 5** — end message **in English** confirming the installation and
>    startup, with the **local** address (`http://<local-ip>:8080`) and
>    the **internet** address (`http://<public-ip>:8080`, subject to port
>    forwarding to this machine).
>
> Everything is logged in `/var/log/homevault-install.log`.

`installer/install.sh` is the **only installer file** — packages, the systemd
unit, the automount config and the download all live in it. The application
archive comes from the `release/latest/download` assets on the project's GitHub
Releases (`homevault-x86_64.tar.gz` / `-aarch64` / `-armv7l`). Redirect it or
pin its checksum without editing the script:

```bash
sudo HV_RELEASE_URL=https://mirror/homevault.tar.gz bash -c "$(curl -fsSL https://raw.githubusercontent.com/nicoleroux35/HomeVault/main/installer/install.sh)"
sudo HV_RELEASE_SHA256=<hex> bash -c "$(curl -fsSL https://raw.githubusercontent.com/nicoleroux35/HomeVault/main/installer/install.sh)"
sudo HV_RELEASE_TOKEN=<github-token> bash -c "$(curl -fsSL https://raw.githubusercontent.com/nicoleroux35/HomeVault/main/installer/install.sh)"
```

Alternative method (from a local copy of the repository, with an archive
already downloaded):

```bash
sudo bash installer/install.sh /path/to/homevault.tar.gz
```

### Running it from a desktop (double-click)

The same file works without a command line: mark it executable, then open it
from the file manager (Nemo, Thunar and Dolphin run it directly; GNOME's
Nautilus opens scripts in a text editor, so use the terminal form there). The
installer **reopens itself in a terminal** and **elevates with `sudo`** on its
own — the password prompt is therefore visible, and nothing has to be typed
beforehand.

```bash
chmod +x install.sh      # once, before the first double-click
```

### Interface: `whiptail` or plain text

One file, three ways to drive it — picked automatically:

| Situation | Interface |
|---|---|
| Terminal (CLI, SSH, `curl \| sudo bash`, double-click) | **`whiptail` TUI** — dialog boxes, a progress bar per step |
| No terminal, no `whiptail`, or `--yes` | plain coloured text |

`whiptail` ships at Debian/Ubuntu priority `important`, so it is already on
every supported base; it is never installed by the script itself. Force either
mode, or make the whole run unattended:

```bash
sudo ./installer/install.sh --tui    # force the dialog UI
sudo ./installer/install.sh --cli    # force plain text
sudo ./installer/install.sh --yes    # never prompt; defaults are used
```

In TUI mode the detailed output (every package, every warning) goes to
`/var/log/homevault-install.log`; the dialogs show the progress and the
outcome. `--yes` also works for automation: it takes the documented default of
every question, HTTPS included.

> **Private project.** The repository is private, so the `curl`/`wget` commands
> in *Installation* only work from a machine that is authenticated against
> GitHub — an anonymous request gets a 404, not a 401. Either pass a token with
> read access to the releases:
>
> ```bash
> sudo HV_RELEASE_TOKEN=<github-token> bash -c "$(curl -fsSL https://raw.githubusercontent.com/nicoleroux35/HomeVault/main/installer/install.sh)"
> ```
>
> or copy the archive to the target machine and install from it:
>
> ```bash
> sudo bash installer/install.sh /path/to/homevault-x86_64.tar.gz
> ```

After installation, open your browser at the address displayed at the end
of execution. On first launch, a setup wizard will guide you through
creating the administrator account. Useful commands:
`systemctl status homevault` and `journalctl -u homevault -f`.

### Upgrading, rolling back, uninstalling

```bash
sudo ./installer/install.sh                 # upgrade (data kept)
sudo ./installer/install.sh --rollback      # back to the previous build
sudo ./installer/install.sh --uninstall     # remove the app, KEEP the data
sudo ./installer/install.sh --uninstall --purge   # ...and delete the data too
./installer/install.sh --dry-run            # preview, changes nothing
```

Every upgrade keeps the running binary as `/opt/homevault/homevault.previous`,
which is what `--rollback` swaps back in. It does **not** roll back the
database: migrations are append-only, so an older binary refuses to start on a
newer schema (it says so at startup). To go back all the way, restore the
snapshot the upgrade wrote first — `data/backups/homevault-*pre-migration.db`.
See [docs/guides/backup-restic.md](docs/guides/backup-restic.md).

`--uninstall` deliberately keeps `/var/lib/homevault`: accounts, settings,
snapshots and app data survive so a reinstall picks up where it left off.
`--purge` deletes them and asks you to type `PURGE` to confirm.

## Project structure

| Folder       | Contents                                             |
| ------------ | ---------------------------------------------------- |
| `src/`       | Rust backend code (Axum server, handlers, services)  |
| `frontend/`  | Svelte + Vite web interface                          |
| `sqlx/`      | SQLite database migrations                           |
| `installer/` | Self-contained installation script                   |
| `static/`    | Static assets served by the backend                  |
| `docs/`      | Developer reference (`INDEX.md` → architecture, API) |
| `tests/`     | Rust integration tests + Playwright E2E suites       |
| `catalog/`   | Docker application catalog                           |
| `scripts/`   | Developer helper scripts (isolated dev instances)    |

## Development

```bash
cargo test                         # backend: unit + integration
cd frontend && npm run check && npm run build   # frontend: types + build
npx playwright test tests/e2e/<screen>/         # E2E for one screen
```

Contributions: see [`CONTRIBUTING.md`](CONTRIBUTING.md). Security: see
[`SECURITY.md`](SECURITY.md). Changes: see [`CHANGELOG.md`](CHANGELOG.md).

## Roadmap

Progress status, known unfixed bugs and incomplete features are tracked in
[`ROADMAP.md`](ROADMAP.md).

## Languages

The interface is available in **French**, **English**, **German** and
**Spanish**. The language can be changed at any time in the settings.

## License

Distributed under the [MIT](LICENSE) license.
Copyright (c) 2026 Nicolas Le Roux.

Contributions are welcome — see [`CONTRIBUTING.md`](CONTRIBUTING.md).

## Support

If HomeVault is useful to you, you can support its development on Ko-fi:

[![ko-fi](https://ko-fi.com/img/githubbutton_sm.svg)](https://ko-fi.com/M1R82932SP)

# HomeVault

[![CI](https://github.com/nicoleroux35/HomeVault/actions/workflows/ci.yml/badge.svg)](https://github.com/nicoleroux35/HomeVault/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)

HomeVault is a personal server that lets you administer, store and
share your data from a simple and accessible web interface. It consists
of a backend written in Rust and a modern frontend in Svelte.

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
- **AI assistant**: local chat via Ollama, RAG knowledge base.
- **Health, journal and quotas**: system monitoring, event journal
  and transfer quotas.
- **Terminal**: access a secure terminal directly in the browser.
- **Settings**: customize the appearance, notifications, updates and
  more.

## Requirements

- **Debian 10+** or **Ubuntu 20.04+** system
- **x86_64**, **aarch64** or **armv7l** architecture
- **root** (sudo) access for installation

Detailed minimum and recommended hardware configuration (RAM, disk,
cost of the optional ZFS / antivirus / AI features): see
[`HARDWARE.md`](HARDWARE.md).

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
sudo HV_RELEASE_URL=https://mirror/homevault.tar.gz bash -c "$(curl -fsSL https://raw.githubusercontent.com/nicleroux35/HomeVault/main/installer/install.sh)"
sudo HV_RELEASE_SHA256=<hex> bash -c "$(curl -fsSL https://raw.githubusercontent.com/nicleroux35/HomeVault/main/installer/install.sh)"
```

Alternative method (from a local copy of the repository, with an archive
already downloaded):

```bash
sudo bash installer/install.sh /path/to/homevault.tar.gz
```

After installation, open your browser at the address displayed at the end
of execution. On first launch, a setup wizard will guide you through
creating the administrator account. Useful commands:
`systemctl status homevault` and `journalctl -u homevault -f`.

## Project structure

| Folder       | Contents                                             |
| ------------ | ---------------------------------------------------- |
| `src/`       | Rust backend code (Axum server, handlers, services)  |
| `frontend/`  | Svelte + Vite web interface                          |
| `sqlx/`      | SQLite database migrations                           |
| `installer/` | Installation script and systemd files                |
| `static/`    | Static assets served by the backend                  |
| `docs/`      | Additional documentation                             |
| `tests/`     | Rust integration tests + Playwright E2E suites       |
| `catalog/`   | Docker application catalog                           |
| `scripts/`   | Utility scripts                                      |

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

Distributed under the [MIT](LICENSE) license. Copyright (c) 2026 HomeVault Team.

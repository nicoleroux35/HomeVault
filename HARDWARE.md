# HomeVault — Supported hardware and systems

This document describes the **minimum configuration** and the **recommended
configuration** for running HomeVault, as well as the officially supported
operating systems and architectures.

HomeVault is a **single binary** (Rust / Axum / Tokio, embedded SQLite
database, web interface included). The server itself is lightweight; it is
the **optional features** (antivirus, ZFS, AI assistant, Docker) that
really determine the memory and disk requirements.

> Network requirements are limited to **one interface** (wired preferably).
> No permanent Internet access is required: update checks are set to
> *Manual* by default and all processing is local.

---

## 1. Supported operating systems

| | Minimum version | Recommended |
|---|---|---|
| **Debian** | 10 (Buster) | 12 (Bookworm) or newer |
| **Ubuntu Server** | 20.04 LTS | 24.04 LTS |

Other software prerequisites:

- **systemd** (HomeVault installs as a systemd service)
- **Root access** for installation and execution
  (the service runs as `root` — it manages RAID, ZFS, Samba/NFS, disk
  encryption, real Linux accounts, etc.)
- **APT**-based distribution (the installer uses `apt-get`)
- **64-bit installation strongly advised** (mandatory if using ZFS)

Derived distributions not tested but generally compatible if they are
based on Debian/Ubuntu **and** APT (Raspberry Pi OS 64-bit, Linux
Mint, Zorin OS…). Non-APT distributions (Fedora, openSUSE, Arch,
Alpine) **are not supported** by the installer.

### CPU architectures

| Architecture | Support | Note |
|---|---|---|
| `x86_64` (amd64) | ✅ Full | Main target |
| `aarch64` (ARM64) | ✅ Full | Raspberry Pi 4/5 64-bit, ARM mini-PCs |
| `armv7l` (32-bit ARM) | ⚠️ Functional | ZFS unavailable; more limited RAM |

---

## 2. Minimum configuration

**Target: server OS + HomeVault alone, without antivirus, without AI
assistant, without Docker containers.**

| Resource | Minimum |
|---|---|
| **CPU** | 1 x86-64 / ARM64 / ARMv7 core |
| **RAM** | **1 GB** (512 MB possible with some swap, without margin) |
| **System disk** | **8 GB** (OS + `/opt/homevault` + APT dependencies: `build-essential`, `ffmpeg`, `smartmontools`, `mdadm`…) |
| **Data disks** | as needed, added separately |
| **Network** | 1 Ethernet interface |

What this configuration covers: accounts and 2FA, recovery code,
SMB / NFS / SFTP shares, file explorer (copy, trash,
upload), transfer quotas, `ufw` firewall, SMART monitoring, `mdadm`
RAID, dashboard, system journal, network, `restic` backups,
`rclone` Cloud Sync, Syncthing, downloader.

Typical process memory footprint at rest: **40–120 MB resident**
(the systemd service is capped at `MemoryHigh=1536M` / `MemoryMax=2G`).

---

## 3. Recommended configuration

**Target: full NAS use — shares, RAID, encryption, antivirus,
notifications, a few applications.**

| Resource | Recommended |
|---|---|
| **CPU** | 2 x86-64 cores (4 cores if frequent compression/backups) |
| **RAM** | **4 GB** |
| **System disk** | **32 GB SSD** (excluding data disks) |
| **Data disks** | dedicated SSD/HDD; 2+ disks for RAID/redundancy |
| **Network** | Gigabit Ethernet |

Margin included for: `clamd` (ClamAV antivirus, ~1–1.5 GB resident for
the signature database — **the heaviest memory item**),
anti-ransomware, storage cache, explorer archive operations,
and several `restic` backups in parallel.

---

## 4. Requirements of the optional features

Each feature below is **disabled by default** or has no effect until it is
configured. Add its cost to the base configuration.

| Feature | Additional RAM | Disk | Notes |
|---|---|---|---|
| **Antivirus (ClamAV)** | **+1 to 1.5 GB** | +2 GB (signatures) | `clamd` keeps the whole database in memory |
| **ZFS** (virtual disks) | **+1 GB minimum**, ideally +1 GB per TB of pool | — | 64-bit mandatory; **8 GB of RAM recommended** for multi-TB pools. Btrfs and mdadm do not have this requirement |
| **LUKS2 encryption (cryptsetup)** | negligible | — | CPU cost on write/read |
| **AI assistant (Ollama)** | **+4 GB minimum** | +2 to 30 GB depending on the model | No model downloaded automatically. gemma3:1b ≈ 2 GB RAM; 7–8B model ≈ 8–16 GB RAM. GPU optional |
| **Docker** (applications) | depends on the containers | +1 GB (engine) + images | Do not install if unused — the app then simply stays inactive |
| **`restic` backups** | +100 to 500 MB during a job | separate repository | Memory peak proportional to volume |
| **Syncthing / rclone** | +100 to 300 MB per active service | — | Separate daemons |
| **Media transcoding (`ffmpeg`)** | temporary, CPU-intensive | — | Installed by default; used on demand |

---

## 5. Example platforms

| Platform | Suited to |
|---|---|
| Raspberry Pi 4 (4 GB) / Pi 5, USB-SATA SSD disk | Minimum to recommended configuration, **without** ZFS or AI |
| x86 mini-PC (N100/N150, 8–16 GB, NVMe) | Recommended configuration **with** ZFS, antivirus, a few Docker apps |
| Server / tower with ECC, 16–32 GB, several disks | Multi-TB ZFS, RAID, antivirus, local AI assistant |
| VM (Proxmox, KVM): 2 vCPU, 4 GB, 32 GB disk | Recommended configuration excluding AI |

---

## 6. Installation notes

- The installer (`installer/install.sh`) pulls a compilation toolchain
  (`build-essential`, `libsqlite3-dev`) and `ffmpeg` **without
  `--install-recommends`** — allow space on the system disk.
- No **swap** is created automatically: create some on
  configurations with 1 GB of RAM or less.
- The service sets **no** CPU quota by default (backup throughput benefits
  from all cores); `CPUQuota=` can be added
  in the systemd unit `installer/install.sh` writes if HomeVault shares the machine with
  other services.
- The service's memory limits (`MemoryHigh` / `MemoryMax`) are prudent
  starting points: adjust them with `systemd-cgtop` on your real
  hardware before relying on them in production.

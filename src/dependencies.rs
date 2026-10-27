/* =============================================================================
 * File          : dependencies.rs
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 12/10/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Centralized inventory of the external system packages/binaries
 *   HomeVault shells out to (smartctl, ufw, restic, zfs, ...), with a
 *   presence check per entry so the startup output shows an admin at a
 *   glance what is missing instead of a failure on the specific page.
 *
 * Dependencies:
 *   - internal catalog of expected binaries
 *
 * Usage:
 *   Startup banner in src/web/mod.rs; availability endpoints.
 * =============================================================================
 */

//! Centralized inventory of external system packages/binaries HomeVault
//! shells out to at runtime, and whether each is actually present on this
//! host — surfaced in the startup terminal so an admin can see at a glance
//! what's missing, instead of finding out only when a specific page's
//! feature silently fails. Distinct from `installer/install.sh` (which
//! installs all of these) and from each subsystem's own narrower
//! `is_available()` check (VPN/Docker/Compose/Restic/UPS/SNMP/Antivirus) —
//! this reuses those where one already exists rather than duplicating their
//! detection logic, and adds a direct check only for binaries with no
//! existing checker.
//!
//! Every `package` named below is asserted to appear in `installer/install.sh`
//! by the test at the bottom of this file, so the two never drift apart. The
//! same test module also pins the systemd unit and the udiskie automount
//! config `install.sh` writes inline: `installer/install.sh` is the *only*
//! installer file (a `curl | sudo bash` install has no checkout on disk), so
//! those heredocs are the single source of truth — the tests assert their
//! critical directives directly instead of comparing against a tracked file.
//!
//! For daemon-backed dependencies (docker, smbd, vsftpd, nfs-server,
//! snmpd, the dedicated homevault-sftp instance), being *installed* isn't
//! the whole picture — the package can be present but the service itself
//! stopped or disabled, which looks identical to "not installed" from the
//! affected page's point of view. Those entries carry an extra `running`
//! check on top of `installed`; CLI-only tools (mdadm, smartctl, restic,
//! rclone, etc.) have no persistent service to check, so `running` is
//! `None` for them.
//!
//! `ufw` is deliberately in the CLI-only group even though it has a systemd
//! unit: its "active" flag is whether the *firewall* is enabled, which is a
//! state the admin controls from the Firewall app (`POST /firewall/enable`,
//! guarded by a mandatory-rule check + explicit confirmation). A host with
//! ufw installed but the firewall off is the intended default, not a broken
//! service — so warning "installed but not running" at every startup was
//! both noisy and wrong. Only `installed` (the `ufw` binary) gates the app.

use crate::utils::process::run_timed;
use serde::Serialize;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

/// Deadline on the two probes of the dependency inventory (`which`, `systemctl
/// is-active`).
///
/// This runs at startup and in the dependency screen; an unbounded `which` on a
/// host whose `PATH` points at a dead automount, or `systemctl` on a bus that
/// stopped answering, used to block the caller with no ceiling (ROADMAP X.9).
const DEP_PROBE_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Serialize)]
pub struct DependencyStatus {
    pub name: &'static str,
    pub package: &'static str,
    pub category: &'static str,
    pub required: bool,
    pub used_by: &'static str,
    pub installed: bool,
    /// `None` when this dependency has no persistent service to check
    /// (a one-shot CLI tool) — `Some(bool)` for a daemon that's expected
    /// to stay running.
    pub running: Option<bool>,
}

struct DependencySpec {
    name: &'static str,
    package: &'static str,
    category: &'static str,
    required: bool,
    used_by: &'static str,
    check_installed: fn() -> bool,
    check_running: Option<fn() -> bool>,
}

/// True if `bin` resolves via `which`, or exists at `default_path`. `which`
/// alone can miss a binary under `/usr/sbin` when that directory isn't on
/// this process's own `PATH` — the same gap already fixed for
/// `firewall::ufw_available()` by checking via `sudo` instead. We don't
/// want to shell out through `sudo` just to ask "does this file exist", so
/// the well-known absolute install path is checked directly instead.
fn binary_present(bin: &str, default_path: &str) -> bool {
    let via_which = run_timed(Command::new("which").arg(bin), DEP_PROBE_TIMEOUT)
        .map(|o| o.status.success() && !o.stdout.is_empty())
        .unwrap_or(false);
    via_which || Path::new(default_path).exists()
}

/// True if the given systemd unit is currently active. Read-only status
/// query — unlike the start/stop/restart calls elsewhere in this codebase
/// (`src/shares.rs`, `src/setup.rs::configure_sudoers`), `systemctl
/// is-active` needs no root and no sudoers grant.
fn service_active(unit: &str) -> bool {
    run_timed(
        Command::new("systemctl").args(["is-active", "--quiet", unit]),
        DEP_PROBE_TIMEOUT,
    )
    .map(|o| o.status.success())
    .unwrap_or(false)
}

fn check_mdadm() -> bool {
    binary_present("mdadm", "/usr/sbin/mdadm")
}
fn check_smartctl() -> bool {
    binary_present("smartctl", "/usr/sbin/smartctl")
}
fn check_acl() -> bool {
    binary_present("setfacl", "/usr/bin/setfacl")
}
fn check_quota() -> bool {
    // All four ship in the same `quota` package and HomeVault needs each of
    // them: `setquota` writes the limit, `repquota` reads it back (no
    // "enforced" answer is ever produced without it), `quotacheck` builds the
    // accounting files and `quotaon` arms accounting after a reboot
    // (`src/quota.rs`, installer's `setup_home_quota`).
    binary_present("setquota", "/usr/sbin/setquota")
        && binary_present("repquota", "/usr/sbin/repquota")
        && binary_present("quotaon", "/usr/sbin/quotaon")
        && binary_present("quotacheck", "/usr/sbin/quotacheck")
}
fn check_cryptsetup() -> bool {
    binary_present("cryptsetup", "/usr/sbin/cryptsetup")
}
fn check_zfs() -> bool {
    // The Storage app drives both: `zpool` for pools, `zfs` for datasets.
    binary_present("zpool", "/usr/sbin/zpool") && binary_present("zfs", "/usr/sbin/zfs")
}
fn check_btrfs() -> bool {
    // `btrfs` for pools/snapshots, `mkfs.btrfs` to create one.
    binary_present("btrfs", "/usr/bin/btrfs")
        && binary_present("mkfs.btrfs", "/usr/sbin/mkfs.btrfs")
}
// Disk spindown (Power app): `src/power.rs` runs `sudo hdparm -B/-S` per disk.
// Priority: standard, but absent from a minimal install — the page reports
// "hdparm not available" and the setting never applies without it.
fn check_hdparm() -> bool {
    binary_present("hdparm", "/usr/sbin/hdparm")
}
// The RAID app formats an array as ext4 or xfs (`src/disk.rs::format_device`).
// xfsprogs is Priority: optional and not part of a minimal base.
fn check_mkfs_xfs() -> bool {
    binary_present("mkfs.xfs", "/usr/sbin/mkfs.xfs")
}
// Explorer "format as exfat". The package that ships `mkfs.exfat` changed
// name: `exfatprogs` on Debian 12+/Ubuntu 23.04+, `exfat-utils` before.
fn check_mkfs_exfat() -> bool {
    binary_present("mkfs.exfat", "/usr/sbin/mkfs.exfat")
}
// Wi-Fi management (`src/network.rs` drives `wpa_cli` through sudo).
fn check_wpa_cli() -> bool {
    binary_present("wpa_cli", "/usr/sbin/wpa_cli")
}
// Interface-config persistence (`src/network.rs`): `netplan apply` is what
// materializes the written `/etc/netplan/99-homevault-*.yaml` so the Network
// screen's persisted config is applied and verified in the same session, not
// just at the next reboot. Ubuntu ships netplan.io on every install; a Debian
// host without it falls through to the ifupdown backend or answers
// "(temporary — persistence unavailable)".
fn check_netplan() -> bool {
    binary_present("netplan", "/usr/sbin/netplan")
}
// The Setup wizard renews/releases the DHCP lease with `resolve_bin("dhclient")`
// (`src/setup.rs`), which ships in `isc-dhcp-client` — NOT in the base: Debian
// netinst and Raspberry Pi OS (dhcpcd) can boot without it, and the call then
// failed with no package to install.
fn check_dhclient() -> bool {
    binary_present("dhclient", "/usr/sbin/dhclient")
}
fn check_sensors() -> bool {
    binary_present("sensors", "/usr/bin/sensors")
}
fn check_rsync() -> bool {
    binary_present("rsync", "/usr/bin/rsync")
}
fn check_smbd() -> bool {
    binary_present("smbd", "/usr/sbin/smbd")
}
fn check_exportfs() -> bool {
    binary_present("exportfs", "/usr/sbin/exportfs")
}
fn check_vsftpd() -> bool {
    binary_present("vsftpd", "/usr/sbin/vsftpd")
}
fn check_sshd() -> bool {
    binary_present("sshd", "/usr/sbin/sshd")
}
fn check_syncthing() -> bool {
    binary_present("syncthing", "/usr/bin/syncthing")
}
fn check_rclone() -> bool {
    binary_present("rclone", "/usr/bin/rclone")
}
fn check_aria2c() -> bool {
    binary_present("aria2c", "/usr/bin/aria2c")
}
fn check_ollama() -> bool {
    binary_present("ollama", "/usr/local/bin/ollama")
}
fn check_ddrescue() -> bool {
    binary_present("ddrescue", "/usr/bin/ddrescue")
}
// The `testdisk` package ships both binaries; the Rescue app uses each.
fn check_testdisk_photorec() -> bool {
    binary_present("testdisk", "/usr/bin/testdisk")
        && binary_present("photorec", "/usr/bin/photorec")
}
fn check_dmidecode() -> bool {
    binary_present("dmidecode", "/usr/sbin/dmidecode")
}
fn check_ethtool() -> bool {
    binary_present("ethtool", "/usr/sbin/ethtool")
}
fn check_avahi() -> bool {
    binary_present("avahi-daemon", "/usr/sbin/avahi-daemon")
}
// Explorer's network-discovery modal browses mDNS with `avahi-browse`, which
// ships in `avahi-utils` — NOT in `avahi-daemon`. Installing only the daemon
// (what this script used to do) left that half of the discovery list empty.
fn check_avahi_browse() -> bool {
    binary_present("avahi-browse", "/usr/bin/avahi-browse")
}
// Explorer's SMB discovery parses `smbtree -N -b` (from samba-common-bin,
// pulled in by the `samba` package).
fn check_smbtree() -> bool {
    binary_present("smbtree", "/usr/bin/smbtree")
}
fn check_snmp_client() -> bool {
    binary_present("snmpget", "/usr/bin/snmpget")
}
fn check_git() -> bool {
    binary_present("git", "/usr/bin/git")
}
fn check_unzip() -> bool {
    binary_present("unzip", "/usr/bin/unzip")
}
fn check_zip() -> bool {
    binary_present("zip", "/usr/bin/zip")
}
fn check_7z() -> bool {
    binary_present("7z", "/usr/bin/7z")
}
fn check_lspci() -> bool {
    binary_present("lspci", "/usr/bin/lspci")
}
fn check_hostname() -> bool {
    binary_present("hostname", "/bin/hostname")
}
fn check_python3() -> bool {
    binary_present("python3", "/usr/bin/python3")
}
fn check_incus() -> bool {
    binary_present("incus", "/usr/bin/incus")
}
fn check_qemu() -> bool {
    binary_present("qemu-system-x86_64", "/usr/bin/qemu-system-x86_64")
}
/// `swtpm` alone is not enough — Incus needs `swtpm_setup` (in `swtpm-tools`)
/// to initialise the emulated TPM state, or a Windows 11 VM fails to start.
fn check_swtpm() -> bool {
    binary_present("swtpm", "/usr/bin/swtpm")
        && binary_present("swtpm_setup", "/usr/bin/swtpm_setup")
}
fn check_swtpm_setup() -> bool {
    binary_present("swtpm_setup", "/usr/bin/swtpm_setup")
}
fn check_ovmf() -> bool {
    Path::new("/usr/share/OVMF/OVMF_CODE.fd").exists()
        || Path::new("/usr/share/OVMF/OVMF_CODE_4M.fd").exists()
}
/// The qemu SPICE display module — Incus always passes `-spice` to qemu, so
/// without this package NO VM can boot (containers are unaffected). The module
/// ships `.so` files under the qemu lib dir; presence of any is enough.
fn check_qemu_spice() -> bool {
    [
        "/usr/lib/x86_64-linux-gnu/qemu",
        "/usr/lib64/qemu",
        "/usr/lib/qemu",
    ]
    .iter()
    .any(|dir| {
        std::fs::read_dir(dir)
            .map(|rd| {
                rd.flatten()
                    .any(|e| e.file_name().to_string_lossy().contains("spice"))
            })
            .unwrap_or(false)
    })
}

fn running_docker() -> bool {
    service_active("docker")
}
fn running_smbd() -> bool {
    service_active("smbd")
}
fn running_vsftpd() -> bool {
    service_active("vsftpd")
}
// Package is `nfs-kernel-server`, but the systemd unit it ships is
// `nfs-server` — same naming split already relied on by the sudoers grant
// in `src/setup.rs::configure_sudoers` (`systemctl start/stop/restart
// nfs-server`).
fn running_nfs() -> bool {
    service_active("nfs-server")
}
fn running_snmpd() -> bool {
    service_active("snmpd")
}
fn running_avahi() -> bool {
    service_active("avahi-daemon")
}
// SFTP shares are served by a second, dedicated `sshd` instance
// (`homevault-sftp.service`, see `src/sftp.rs`) — never the host's own
// regular `ssh`/`sshd` service, which this app never touches.
fn running_homevault_sftp() -> bool {
    service_active("homevault-sftp")
}

/// One entry per dependency, paired with its (possibly subprocess-spawning)
/// check functions — reuses each subsystem's own existing `is_available()`
/// where one already exists, rather than duplicating its detection logic.
fn specs() -> Vec<DependencySpec> {
    vec![
        // --- Storage & disk health (CLI tools, no persistent service) ---
        DependencySpec {
            name: "mdadm",
            package: "mdadm",
            category: "Storage",
            required: true,
            used_by: "RAID arrays (RAID app, Rescue)",
            check_installed: check_mdadm,
            check_running: None,
        },
        DependencySpec {
            name: "smartctl",
            package: "smartmontools",
            category: "Storage",
            required: true,
            used_by: "Disk S.M.A.R.T. health (Dashboard, Health, Storage)",
            check_installed: check_smartctl,
            check_running: None,
        },
        DependencySpec {
            name: "setfacl / getfacl",
            package: "acl",
            category: "Storage",
            required: true,
            used_by: "Share permissions (ACLs)",
            check_installed: check_acl,
            check_running: None,
        },
        DependencySpec {
            name: "quota tools (setquota / repquota / quotaon / quotacheck)",
            package: "quota",
            category: "Storage",
            required: true,
            used_by: "Per-user home-folder disk quotas (Users)",
            check_installed: check_quota,
            check_running: None,
        },
        DependencySpec {
            name: "cryptsetup",
            package: "cryptsetup",
            category: "Storage",
            required: false,
            used_by: "Disk encryption - LUKS2 (Storage app)",
            check_installed: check_cryptsetup,
            check_running: None,
        },
        DependencySpec {
            name: "zpool / zfs",
            package: "zfsutils-linux",
            category: "Storage",
            required: false,
            used_by: "ZFS storage pools (Storage app)",
            check_installed: check_zfs,
            check_running: None,
        },
        DependencySpec {
            name: "btrfs",
            package: "btrfs-progs",
            category: "Storage",
            required: false,
            used_by: "Btrfs storage pools + snapshots (Storage app)",
            check_installed: check_btrfs,
            check_running: None,
        },
        DependencySpec {
            name: "hdparm",
            package: "hdparm",
            category: "Storage",
            required: false,
            used_by: "Disk spindown - APM level + idle timer (Power app)",
            check_installed: check_hdparm,
            check_running: None,
        },
        DependencySpec {
            name: "mkfs.xfs",
            package: "xfsprogs",
            category: "Storage",
            required: false,
            used_by: "XFS arrays (RAID app) and XFS-formatting paths",
            check_installed: check_mkfs_xfs,
            check_running: None,
        },
        DependencySpec {
            name: "mkfs.exfat",
            package: "exfatprogs, exfat-utils",
            category: "Storage",
            required: false,
            used_by: "Format a disk as exFAT (Explorer)",
            check_installed: check_mkfs_exfat,
            check_running: None,
        },
        DependencySpec {
            name: "wpa_cli",
            package: "wpasupplicant",
            category: "Network",
            required: false,
            used_by: "Wi-Fi scan / join / status (Network app)",
            check_installed: check_wpa_cli,
            check_running: None,
        },
        DependencySpec {
            name: "netplan",
            package: "netplan.io",
            category: "Network",
            required: false,
            used_by: "Persisting interface configuration across reboots (Network app)",
            check_installed: check_netplan,
            check_running: None,
        },
        DependencySpec {
            name: "dhclient",
            package: "isc-dhcp-client",
            category: "Network",
            required: false,
            used_by: "DHCP lease renew/release (Setup wizard, Network app)",
            check_installed: check_dhclient,
            check_running: None,
        },
        DependencySpec {
            name: "sensors",
            package: "lm-sensors",
            category: "Storage",
            required: true,
            used_by: "Temperature/fan/voltage sensors (Dashboard)",
            check_installed: check_sensors,
            check_running: None,
        },
        DependencySpec {
            name: "dmidecode",
            package: "dmidecode",
            category: "Storage",
            required: false,
            used_by: "Motherboard / BIOS / RAM details (Dashboard)",
            check_installed: check_dmidecode,
            check_running: None,
        },
        DependencySpec {
            name: "rsync",
            package: "rsync",
            category: "Storage",
            required: true,
            used_by: "Cache-tier mover",
            check_installed: check_rsync,
            check_running: None,
        },
        // --- Data recovery (Rescue app, CLI tools) ---
        DependencySpec {
            name: "ddrescue",
            package: "gddrescue",
            category: "Rescue",
            required: false,
            used_by: "Disk cloning (Rescue app)",
            check_installed: check_ddrescue,
            check_running: None,
        },
        DependencySpec {
            name: "testdisk / photorec",
            package: "testdisk",
            category: "Rescue",
            required: false,
            used_by: "Partition repair + file carving (Rescue app)",
            check_installed: check_testdisk_photorec,
            check_running: None,
        },
        // --- Sharing (systemd services) ---
        DependencySpec {
            name: "smbd",
            package: "samba",
            category: "Shares",
            required: false,
            used_by: "SMB/CIFS shares",
            check_installed: check_smbd,
            check_running: Some(running_smbd),
        },
        DependencySpec {
            name: "exportfs",
            package: "nfs-kernel-server",
            category: "Shares",
            required: false,
            used_by: "NFS shares",
            check_installed: check_exportfs,
            check_running: Some(running_nfs),
        },
        DependencySpec {
            name: "vsftpd",
            package: "vsftpd",
            category: "Shares",
            required: false,
            used_by: "FTP shares",
            check_installed: check_vsftpd,
            check_running: Some(running_vsftpd),
        },
        DependencySpec {
            name: "sshd",
            package: "openssh-server",
            category: "Shares",
            required: false,
            used_by: "SFTP shares (dedicated homevault-sftp instance)",
            check_installed: check_sshd,
            check_running: Some(running_homevault_sftp),
        },
        // --- Security ---
        DependencySpec {
            name: "ufw",
            package: "ufw",
            category: "Security",
            required: false,
            used_by: "Firewall management",
            check_installed: crate::firewall::ufw_available,
            check_running: None,
        },
        DependencySpec {
            name: "clamscan",
            package: "clamav",
            category: "Security",
            required: false,
            used_by: "Antivirus scanning",
            check_installed: crate::antivirus::clamscan_available,
            check_running: None,
        },
        // --- VPN (per-connection, not a single system-wide state) ---
        DependencySpec {
            name: "wg",
            package: "wireguard-tools",
            category: "VPN",
            required: false,
            used_by: "WireGuard connections",
            check_installed: crate::vpn::wireguard_available,
            check_running: None,
        },
        DependencySpec {
            name: "openvpn",
            package: "openvpn",
            category: "VPN",
            required: false,
            used_by: "OpenVPN connections",
            check_installed: crate::vpn::openvpn_available,
            check_running: None,
        },
        // --- Backup & sync ---
        DependencySpec {
            name: "restic",
            package: "restic",
            category: "Backup",
            required: false,
            used_by: "Backup app",
            check_installed: crate::restic::ResticService::is_available,
            check_running: None,
        },
        DependencySpec {
            name: "syncthing",
            package: "syncthing",
            category: "Backup",
            required: false,
            used_by: "Syncthing app",
            check_installed: check_syncthing,
            check_running: None,
        },
        DependencySpec {
            name: "rclone",
            package: "rclone",
            category: "Backup",
            required: false,
            used_by: "Cloud Sync app",
            check_installed: check_rclone,
            check_running: None,
        },
        DependencySpec {
            name: "aria2c",
            package: "aria2",
            category: "Backup",
            required: false,
            used_by: "Downloader app",
            check_installed: check_aria2c,
            check_running: None,
        },
        // --- AI ---
        // Ollama is not an apt package (installed via its own script by
        // `install.sh`'s `install_ollama`), so `package` here is the name its
        // installer/binary uses, not an apt id — the install.sh cross-check
        // test exempts it and asserts `install_ollama` exists instead.
        DependencySpec {
            name: "ollama",
            package: "ollama",
            category: "AI",
            required: false,
            used_by: "AI Assistant (Chat app)",
            check_installed: check_ollama,
            check_running: None,
        },
        // --- Containers ---
        DependencySpec {
            name: "docker",
            package: "docker.io",
            category: "Containers",
            required: false,
            used_by: "Docker app",
            check_installed: crate::docker::DockerService::is_available,
            check_running: Some(running_docker),
        },
        DependencySpec {
            name: "docker compose",
            package: "docker-compose-plugin",
            category: "Containers",
            required: false,
            used_by: "Docker Compose projects",
            check_installed: crate::compose::ComposeManager::is_available,
            check_running: None,
        },
        // --- Virtualization ---
        DependencySpec {
            name: "Incus",
            package: "incus",
            category: "Virtualization",
            required: false,
            used_by: "Virtual Machines app",
            check_installed: check_incus,
            check_running: None,
        },
        DependencySpec {
            name: "QEMU",
            package: "qemu-system-x86",
            category: "Virtualization",
            required: false,
            used_by: "Virtual Machines app (VM hypervisor)",
            check_installed: check_qemu,
            check_running: None,
        },
        DependencySpec {
            name: "QEMU SPICE module",
            package: "qemu-system-modules-spice",
            category: "Virtualization",
            required: false,
            used_by: "Virtual Machines app (required for any VM to boot + the graphical screen)",
            check_installed: check_qemu_spice,
            check_running: None,
        },
        DependencySpec {
            name: "swtpm",
            package: "swtpm",
            category: "Virtualization",
            required: false,
            used_by: "Virtual Machines app (TPM 2.0 emulator for Windows 11)",
            check_installed: check_swtpm,
            check_running: None,
        },
        DependencySpec {
            name: "swtpm-tools",
            package: "swtpm-tools",
            category: "Virtualization",
            required: false,
            used_by: "Virtual Machines app (initialises the emulated TPM state)",
            check_installed: check_swtpm_setup,
            check_running: None,
        },
        DependencySpec {
            name: "OVMF",
            package: "ovmf",
            category: "Virtualization",
            required: false,
            used_by: "Virtual Machines app (UEFI firmware)",
            check_installed: check_ovmf,
            check_running: None,
        },
        // --- Email ---
        // No single package name covers this - EmailService::send tries
        // mutt -> sendmail -> msmtp in order and only needs one of them.
        DependencySpec {
            name: "mutt / sendmail / msmtp",
            package: "mutt, sendmail, or msmtp",
            category: "Email",
            required: false,
            used_by: "Settings > Email (test/notification emails)",
            check_installed: crate::email::EmailService::is_email_configured,
            check_running: None,
        },
        // --- Network ---
        DependencySpec {
            name: "ethtool",
            package: "ethtool",
            category: "Network",
            required: false,
            used_by: "Link-speed / driver diagnostics (Network app)",
            check_installed: check_ethtool,
            check_running: None,
        },
        DependencySpec {
            name: "avahi-daemon",
            package: "avahi-daemon",
            category: "Network",
            required: false,
            used_by: "mDNS — reachable as homevault.local + share discovery on the LAN",
            check_installed: check_avahi,
            check_running: Some(running_avahi),
        },
        DependencySpec {
            name: "avahi-browse",
            package: "avahi-utils",
            category: "Network",
            required: false,
            used_by: "Explorer — network discovery (mDNS service browsing)",
            check_installed: check_avahi_browse,
            check_running: None,
        },
        DependencySpec {
            name: "smbtree",
            package: "samba",
            category: "Network",
            required: false,
            used_by: "Explorer — network discovery (SMB workgroups/hosts/shares)",
            check_installed: check_smbtree,
            check_running: None,
        },
        // --- Monitoring ---
        DependencySpec {
            name: "snmpd",
            package: "snmpd",
            category: "Monitoring",
            required: false,
            used_by: "SNMP monitoring (local agent)",
            check_installed: crate::snmp::SnmpService::is_available,
            check_running: Some(running_snmpd),
        },
        DependencySpec {
            name: "snmpget / snmpwalk",
            package: "snmp",
            category: "Monitoring",
            required: false,
            used_by: "SNMP monitoring (client tools that read the OIDs)",
            check_installed: check_snmp_client,
            check_running: None,
        },
        // `upsc -L` (UpsService::is_available) already requires connecting
        // to a running `upsd`, so "installed" here already implies "the
        // NUT server is reachable" - no separate running check needed.
        DependencySpec {
            name: "upsc",
            package: "nut",
            category: "Monitoring",
            required: false,
            used_by: "UPS monitoring (NUT)",
            check_installed: crate::ups::UpsService::is_available,
            check_running: None,
        },
        // --- File / system tools (install.sh provisions all of these) ---
        DependencySpec {
            name: "git",
            package: "git",
            category: "System",
            required: false,
            used_by: "Docker App Catalog (community stores cloned from git repos)",
            check_installed: check_git,
            check_running: None,
        },
        DependencySpec {
            name: "unzip",
            package: "unzip",
            category: "System",
            required: false,
            used_by: "Explorer — extract .zip archives",
            check_installed: check_unzip,
            check_running: None,
        },
        DependencySpec {
            name: "zip",
            package: "zip",
            category: "System",
            required: false,
            used_by: "Browser — compress selection to .zip",
            check_installed: check_zip,
            check_running: None,
        },
        DependencySpec {
            name: "7z",
            package: "7zip or p7zip-full",
            category: "System",
            required: false,
            used_by: "Explorer — extract .7z archives",
            check_installed: check_7z,
            check_running: None,
        },
        DependencySpec {
            name: "lspci",
            package: "pciutils",
            category: "System",
            required: false,
            used_by: "Dashboard — GPU / hardware inventory",
            check_installed: check_lspci,
            check_running: None,
        },
        DependencySpec {
            name: "hostname",
            package: "hostname",
            category: "System",
            required: false,
            used_by: "Host identity (install.sh LAN-IP fallback, get_hostname)",
            check_installed: check_hostname,
            check_running: None,
        },
        DependencySpec {
            name: "python3",
            package: "python3",
            category: "System",
            required: false,
            used_by: "Linux-user / PAM login (src/auth/service.rs)",
            check_installed: check_python3,
            check_running: None,
        },
    ]
}

/// Runs every check and returns the current status of each dependency. Not
/// cached, so the result always reflects the host as it is right now — but
/// each individual check is a blocking subprocess call (`--version`-style,
/// a real connection attempt for a couple of them, or a `systemctl
/// is-active` query), so with ~24 entries running one after another this
/// took upwards of 25s end-to-end when first measured live. Every check
/// runs concurrently via `spawn_blocking` instead, cutting total wall time
/// down to roughly the single slowest check rather than their sum.
pub async fn check_all() -> Vec<DependencyStatus> {
    let handles: Vec<_> = specs()
        .into_iter()
        .map(|spec| {
            tokio::task::spawn_blocking(move || {
                let installed = (spec.check_installed)();
                // Only bother checking "running" if it's actually
                // installed - an absent package is never running.
                let running = spec.check_running.map(|f| installed && f());
                DependencyStatus {
                    name: spec.name,
                    package: spec.package,
                    category: spec.category,
                    required: spec.required,
                    used_by: spec.used_by,
                    installed,
                    running,
                }
            })
        })
        .collect();

    let mut results = Vec::with_capacity(handles.len());
    for handle in handles {
        if let Ok(status) = handle.await {
            results.push(status);
        }
    }
    results
}

#[cfg(test)]
mod tests {
    //! Consistency checks between this inventory and `installer/install.sh`,
    //! plus basic sanity on the spec table. Pure logic — no subprocesses,
    //! no network. The live per-host detection is exercised by the startup
    //! path and `tests/e2e/*` against a real server.

    use super::*;

    /// The set of apt package names `installer/install.sh` actually asks apt
    /// to install — parsed from `install_optional <pkg> ...` lines and from
    /// `install_packages "<label>" <required|optional> [<pkg>...]` calls
    /// (possibly spread over `\`-continued lines). The generic
    /// `apt-get install -y ... "${missing[@]}"` inside `install_packages`'s
    /// body is deliberately NOT parsed — its arguments come from the call
    /// sites this function reads. Deliberately does NOT scan quoted
    /// description strings (several of which happen to contain the package
    /// name), so a real typo in the package argument is caught.
    fn install_sh_packages() -> std::collections::HashSet<String> {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/installer/install.sh");
        let text =
            std::fs::read_to_string(path).unwrap_or_else(|e| panic!("cannot read {path}: {e}"));

        let mut pkgs = std::collections::HashSet::new();
        // True while reading a `\`-continued `install_packages` call.
        let mut in_pkg_call = false;
        for raw in text.lines() {
            let line = raw.trim();
            if line.starts_with('#') {
                continue;
            }

            if let Some(rest) = line.strip_prefix("install_optional ") {
                if let Some(pkg) = rest.split_whitespace().next() {
                    pkgs.insert(pkg.to_string());
                }
                continue;
            }

            if line.starts_with("install_packages ") || in_pkg_call {
                // Effective package-text of this line. For the opening line of
                // a call, skip the quoted `<label>` — the package list may
                // follow right after the closing quote on the same line, as
                // in `install_packages "Media player (FFmpeg)" required
                // ffmpeg`; cutting at the first `"` would silently drop
                // `ffmpeg`. A `\`-continued line below an opening call has no
                // quotes at all and is taken whole.
                let effective = if let Some(rest) = line.strip_prefix("install_packages ") {
                    if let Some(stripped) = rest.strip_prefix('"') {
                        match stripped.find('"') {
                            Some(offset) => &stripped[offset + 1..],
                            // Unterminated label — nothing parseable left.
                            None => "",
                        }
                    } else {
                        rest
                    }
                } else {
                    line
                };
                for tok in effective.split_whitespace() {
                    if matches!(
                        tok,
                        "install_packages" | "required" | "optional" | "\\"
                    ) || tok.is_empty()
                        // `install_packages "$name" optional "$pkg"` inside a
                        // helper body — `$name`/`$pkg` belong to no call site.
                        || tok.starts_with('$')
                        || tok.starts_with('"')
                    {
                        continue;
                    }
                    pkgs.insert(tok.trim_end_matches('\\').to_string());
                }
                // A trailing `\` means the package list continues on the next
                // line.
                in_pkg_call = line.ends_with('\\');
            }
        }
        pkgs
    }

    #[test]
    fn test_every_dependency_package_is_in_install_sh() {
        let installed = install_sh_packages();
        let mut missing = Vec::new();
        for spec in specs() {
            // Ollama isn't an apt package — `install.sh` installs it via its
            // own script (`install_ollama`), asserted separately below.
            if spec.package == "ollama" {
                continue;
            }
            // The email dependency is deliberately "any one of these".
            let candidates: Vec<&str> = spec
                .package
                .split([',', ' '])
                .map(|s| s.trim())
                .filter(|s| !s.is_empty() && *s != "or")
                .collect();
            if !candidates.iter().any(|p| installed.contains(*p)) {
                missing.push(format!("{} (package `{}`)", spec.name, spec.package));
            }
        }
        assert!(
            missing.is_empty(),
            "these runtime dependencies are NOT installed by installer/install.sh:\n  {}",
            missing.join("\n  ")
        );
    }

    #[test]
    fn test_install_sh_installs_ollama_for_the_chat_app() {
        // Ollama is exempt from the apt cross-check above (not an apt
        // package) — but `install.sh` must still actually set it up, or the
        // AI Assistant ships dead. Assert the dedicated step is present.
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/installer/install.sh");
        let text = std::fs::read_to_string(path).unwrap();
        assert!(
            text.contains("install_ollama") && text.contains("ollama.com/install.sh"),
            "install.sh no longer installs Ollama — the Chat app would be non-functional on a fresh deploy"
        );
        assert!(
            text.contains("systemctl enable --now ollama"),
            "install.sh should enable the Ollama daemon so Chat can reach it"
        );
        // Installing the binary is not enough — the Chat app talks to the
        // daemon's HTTP API on :11434 (Ollama's own Linux docs say to verify
        // `systemctl status ollama` + that the server answers). Both the
        // install step and the final verification must probe that, or a
        // fresh deploy ships Chat "offline" behind a green install log.
        assert!(
            text.contains("systemctl is-active --quiet ollama"),
            "install.sh should verify the Ollama unit is active, not just that the binary exists"
        );
        // Ollama ships its Linux release as `ollama-linux-<arch>.tar.zst`, and
        // its own installer aborts with "This version requires zstd for
        // extraction" when `zstd` is absent. zstd is Priority: optional, so a
        // fresh Debian/Ubuntu does not have it: without the explicit install
        // the AI assistant silently stayed offline on every new host.
        assert!(
            text.contains("install_optional zstd"),
            "install.sh must install zstd — Ollama's installer cannot unpack its .tar.zst without it"
        );
        assert!(
            text.contains("command -v zstd"),
            "install.sh must check for zstd before running Ollama's installer, with a clear message"
        );
        assert!(
            text.contains("http://127.0.0.1:11434/"),
            "install.sh should poll Ollama's HTTP API on :11434 — Chat needs the daemon answering, not merely installed"
        );
        // The model is deliberately NOT pulled here — it is the user's choice
        // (size vs. quality depends on the host) and is downloaded from
        // Settings > AI. Guard against a well-meaning re-add of an auto-pull.
        assert!(
            !text.contains("ollama pull"),
            "install.sh must not auto-download a model — the user picks it from Settings > AI"
        );
        // Ollama ships no 32-bit ARM build. Installing it anyway on armv7l
        // would leave a dead daemon that reads as an app bug; the installer
        // must skip it and say why.
        assert!(
            text.contains("armv7l|armv6l"),
            "install.sh must skip Ollama on 32-bit ARM (no upstream build)"
        );
    }

    /// Reads `installer/install.sh` as text for the interface-invariant tests
    /// below. Those assert the *entry* behaviour (how the script is launched,
    /// how it prompts) that a piped `curl | sudo bash` install silently
    /// breaks — no test covers the shell itself, so these are the guard rails.
    fn install_sh_text() -> String {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/installer/install.sh");
        std::fs::read_to_string(path).expect("cannot read installer/install.sh")
    }

    /// The installer is **English only** — its own messages, and (through
    /// `LC_ALL=C`) everything it relays: apt/dpkg warnings, systemctl errors
    /// and the whiptail button labels. An accented letter is the fingerprint
    /// of a localized string, so this test is a cheap drift detector for a
    /// translated prompt sneaking back in.
    #[test]
    fn test_install_sh_is_english_only() {
        let text = install_sh_text();
        let accented = "àâäéèêëîïôöùûüçÀÂÄÉÈÊËÎÏÔÖÙÛÜÇ";
        let offenders: Vec<(usize, &str)> = text
            .lines()
            .enumerate()
            .filter(|(_, l)| l.chars().any(|c| accented.contains(c)))
            .map(|(i, l)| (i + 1, l))
            .collect();
        assert!(
            offenders.is_empty(),
            "install.sh must stay English only; these lines look localized:\n{}",
            offenders
                .iter()
                .map(|(n, l)| format!("  {n}: {}", l.trim()))
                .collect::<Vec<_>>()
                .join("\n")
        );
    }

    #[test]
    fn test_install_sh_pins_an_english_locale() {
        // Writing English messages is not enough: apt, dpkg, systemctl and
        // whiptail all answer in the host's language. `C` makes the whole run
        // English and the parsing deterministic (`Candidate:` instead of a
        // localized `Candidat :`).
        let text = install_sh_text();
        assert!(
            text.contains("export LC_ALL=C") && text.contains("export LANG=C"),
            "install.sh must pin LC_ALL/LANG to C — otherwise apt and the whiptail buttons are localized"
        );
    }

    /// Binaries `Command::new(…)` invokes that are *not* probed with
    /// `binary_present` above, either because their module has its own
    /// `is_available()` predicate (clamav, docker, ufw, restic, VPN, UPS,
    /// SNMP, the email fallbacks) or because they come from a driver
    /// HomeVault cannot install (GPU vendor tools) — those degrade to
    /// `lspci` on their own.
    const BINARIES_WITH_DEDICATED_CHECK: &[&str] = &[
        "clamscan",
        "docker",
        "fusermount",
        "getfacl",
        "lsb_release",
        "msmtp",
        "mutt",
        "nvidia-smi",
        "openvpn",
        "restic",
        "rocm-smi",
        "sendmail",
        "snmpd",
        "snmpwalk",
        "testparm",
        "ufw",
        "upsc",
        "wg",
        "wg-quick",
    ];

    /// The two initramfs generators RAID creation may invoke — **as pure
    /// best-effort fallbacks, both wrapped in `let _ =`**
    /// (`src/disk.rs`, mdadm post-create): `update-initramfs` on
    /// initramfs-tools distros, `dracut` elsewhere. HomeVault deliberately
    /// installs NEITHER:
    /// - `initramfs-tools` is `Priority: important` and ships with every
    ///   kernel-capable Debian/Ubuntu base — and a system booted without any
    ///   generator does not exist;
    /// - installing `dracut` *alongside* it to satisfy an audit would risk
    ///   switching the kernel postinst regime on an apt-based host, a much
    ///   worse outcome than skipping an optional regeneration.
    /// Each base therefore already ships exactly one of the two (this is the
    /// same "cannot be missing on a bootable base" class BASE_BINARIES pins,
    /// kept out of it because the pair is an either/or, not a single package).
    const BEST_EFFORT_INITRAMFS_TOOLS: &[&str] = &["dracut", "update-initramfs"];

    /// Binaries provided by packages every supported base already ships:
    /// `Priority: required` / `Essential` (coreutils, util-linux, procps,
    /// passwd, debianutils, libc-bin, gzip, findutils, systemd, sudo,
    /// e2fsprogs), or requested by `install.sh`'s *required* group (iproute2,
    /// iputils-ping).
    const BASE_BINARIES: &[&str] = &[
        "blkid",
        // systemd, like `systemctl` / `journalctl` / `timedatectl` below: the
        // mDNS hostname is set and read back through `busctl` against
        // `org.freedesktop.Avahi` (`src/mdns.rs`).
        "busctl",
        "cat",
        "chmod",
        "chown",
        "chpasswd",
        "df",
        "find",
        "findmnt",
        "gpasswd",
        "getent",
        "groupadd",
        "groupdel",
        "groups",
        "gunzip",
        "hostnamectl",
        "id",
        "ip",
        "journalctl",
        "kill",
        "lsblk",
        "mkdir",
        "mkfs.ext4",
        "mount",
        "mountpoint",
        "pgrep",
        "ping",
        "pkill",
        "ps",
        "rm",
        "rmdir",
        "ss",
        "sudo",
        "systemctl",
        "tail",
        "tar",
        "tee",
        "test",
        // systemd, like `hostnamectl` above: the Settings "timezone" field is
        // applied with `timedatectl set-timezone` (ROADMAP 13.4).
        "timedatectl",
        "umount",
        "uname",
        "useradd",
        "usermod",
        "visudo",
        "which",
        "wipefs",
    ];

    /// Walks `src/` and collects every binary name passed as a *literal* to
    /// `Command::new("…")` (both `std::process` and `tokio::process`) and to
    /// `SetupService::resolve_bin("…", …)` — the latter is the sudoers
    /// contract, i.e. every tool HomeVault intends to run as root, which is
    /// exactly where a missing package hides. Tools launched through a
    /// one-argument helper (`explorer.rs`'s `run("avahi-browse", …)`) are
    /// pinned by their own `DependencySpec` instead.
    fn collect_commanded_binaries(
        dir: &std::path::Path,
        out: &mut std::collections::BTreeSet<String>,
    ) {
        const NEEDLES: [&str; 2] = ["Command::new(\"", "resolve_bin(\""];
        let entries = std::fs::read_dir(dir).unwrap_or_else(|e| panic!("cannot read {dir:?}: {e}"));
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                collect_commanded_binaries(&path, out);
                continue;
            }
            if path.extension().is_none_or(|e| e != "rs") {
                continue;
            }
            let text = std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("cannot read {path:?}: {e}"));
            // Audit **production** code only. Every module's tests live at the
            // bottom behind a column-0 `#[cfg(test)]`, and they build fixtures
            // such as `Command::new("homevault-no-such-binary")`, a pipe into
            // `sleep`, or their own copy of this scan's needle — auditing those
            // reported binaries HomeVault never runs (and this guard's own text
            // reported itself, which kept the whole audit red).
            let text = match text.find("\n#[cfg(test)]") {
                Some(i) => &text[..i],
                None => &text[..],
            };
            for needle in NEEDLES {
                let mut rest = text;
                while let Some(i) = rest.find(needle) {
                    let tail = &rest[i + needle.len()..];
                    let Some(end) = tail.find('"') else { break };
                    let name = &tail[..end];
                    if !name.is_empty() {
                        out.insert(name.to_string());
                    }
                    rest = &tail[end + 1..];
                }
            }
        }
    }

    /// Every binary this file probes with `binary_present("…", …)`.
    fn probed_binaries() -> std::collections::HashSet<String> {
        let text = include_str!("dependencies.rs");
        let needle = "binary_present(\"";
        let mut out = std::collections::HashSet::new();
        let mut rest = text;
        while let Some(i) = rest.find(needle) {
            let tail = &rest[i + needle.len()..];
            let Some(end) = tail.find('"') else { break };
            out.insert(tail[..end].to_string());
            rest = &tail[end + 1..];
        }
        out
    }

    /// The completeness audit: **anything HomeVault executes must have a
    /// package that installs it**, and that package must be in the inventory
    /// above (which `test_every_dependency_package_is_in_install_sh` then
    /// pins to `install.sh`).
    ///
    /// Regression (2026-10-19) — three real gaps this test was written for:
    /// `hdparm` (Power app: disk spindown) was invoked by `src/power.rs` but
    /// installed by nothing, so the page reported "hdparm not available" and
    /// the setting never applied; `mkfs.xfs` (RAID arrays) had no `xfsprogs`;
    /// and `mkfs.exfat` (Explorer format) was assumed to come with
    /// `exfat-fuse`, which does not ship it on Debian 12+.
    #[test]
    fn test_every_commanded_binary_has_an_install_source() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut commanded = std::collections::BTreeSet::new();
        collect_commanded_binaries(&root, &mut commanded);
        assert!(
            commanded.len() > 50,
            "the scan found only {} binaries — it is not walking src/",
            commanded.len()
        );

        let probed = probed_binaries();
        let uncovered: Vec<&str> = commanded
            .iter()
            .map(String::as_str)
            .filter(|bin| !probed.contains(*bin))
            .filter(|bin| !BASE_BINARIES.contains(bin))
            .filter(|bin| !BINARIES_WITH_DEDICATED_CHECK.contains(bin))
            .filter(|bin| !BEST_EFFORT_INITRAMFS_TOOLS.contains(bin))
            .collect();

        assert!(
            uncovered.is_empty(),
            "these binaries are executed by HomeVault but have no install source:\n  {}\n\
             Add a `DependencySpec` for each (package + `binary_present` check) so \
             install.sh installs it, or list it in BASE_BINARIES / \
             BINARIES_WITH_DEDICATED_CHECK / BEST_EFFORT_INITRAMFS_TOOLS with a reason.",
            uncovered.join("\n  ")
        );
    }

    #[test]
    fn test_install_sh_never_reads_prompts_from_stdin() {
        // Regression (2026-10-19): `curl … | sudo bash` puts the *script* on
        // stdin, so a bare `read -r -p` consumed the next line of the script
        // instead of the keyboard. The HTTPS prompt did exactly that: it ate
        // its own `if [[ ! "$REPLY" =~ ^[Nn]$ ]]; then` line and aborted the
        // install with "syntax error near unexpected token 'fi'" *after* the
        // binary had been extracted but before the service was enabled.
        // Every prompt must read the dedicated fd 3 instead.
        let text = install_sh_text();
        let offenders: Vec<&str> = text
            .lines()
            .filter(|l| l.contains("read -r -p") && !l.contains("-u 3"))
            .collect();
        assert!(
            offenders.is_empty(),
            "these prompts read stdin, which is the script itself under `curl | sudo bash`:\n  {}",
            offenders.join("\n  ")
        );
    }

    #[test]
    fn test_install_sh_binds_a_terminal_for_prompts() {
        // fd 3 must point at the controlling terminal when there is one, and
        // fall back to stdin otherwise (headless/cron), so `ask` can decide
        // between prompting and taking the default.
        let text = install_sh_text();
        assert!(
            text.contains("exec 3<&0") && text.contains("exec 3</dev/tty"),
            "install.sh must bind fd 3 to /dev/tty (with a stdin fallback) — prompts rely on it"
        );
        // `-r` is not a valid probe: /dev/tty is world-readable but opening it
        // fails with ENXIO when the process has no controlling terminal.
        assert!(
            text.contains(": </dev/tty"),
            "install.sh must probe /dev/tty by opening it, not with `[[ -r ]]`"
        );
    }

    #[test]
    fn test_install_sh_tui_keeps_answers_off_the_terminal() {
        // whiptail draws on stderr and returns the answer on stderr too. With
        // `2>&4` (fd 4 = /dev/tty) the drawing bypasses the log tee, and
        // `--output-fd 1` puts the answer on stdout where `$(…)` can capture
        // it without the escape sequences mixed in.
        let text = install_sh_text();
        assert!(
            text.contains("--output-fd 1"),
            "whiptail answers must go to stdout via --output-fd 1"
        );
        assert!(
            text.contains("exec 4>/dev/tty"),
            "install.sh must open fd 4 on /dev/tty for whiptail's drawing"
        );
        assert!(
            text.contains(">&4 2>&4"),
            "whiptail must draw its stdout/stderr TUI on fd 4, never on the log-redirected stdout/stderr"
        );
        // The engine's own output would paint over the whiptail screens, so in
        // TUI mode stdout/stderr go to the log file only.
        assert!(
            text.contains("exec >>\"$LOG_FILE\""),
            "in TUI mode install.sh must redirect its own output to the log file"
        );
    }

    #[test]
    fn test_install_sh_exposes_the_interface_flags() {
        // CLI, TUI and unattended runs are all supported from the same file.
        let text = install_sh_text();
        for flag in [
            "--cli)",
            "--tui)",
            "--yes|-y|--non-interactive)",
            "--help|-h)",
        ] {
            assert!(
                text.contains(flag),
                "install.sh no longer parses {flag} — the documented interface would be rejected"
            );
        }
        assert!(
            text.contains("usage() {"),
            "--help must print a usage block (usage() is missing)"
        );
        // `--yes` has to beat TUI detection, otherwise an unattended run
        // blocks on a whiptail dialog nobody can answer.
        assert!(
            text.contains("\"$YES\" -eq 0 && \"$DRY_RUN\" -eq 0 && \"$UI_REQUEST\" != \"cli\""),
            "the TUI must be skipped when --yes or --cli is given"
        );
    }

    #[test]
    fn test_install_sh_elevates_through_sudo_and_forwards_release_knobs() {
        // Double-clicking the file runs it as the logged-in user with no
        // terminal. The script has to reopen itself in a terminal and then
        // elevate — and `sudo` resets the environment, so the documented
        // HV_* knobs must be forwarded explicitly (`sudo -E` is rejected by
        // the default sudoers policy).
        let text = install_sh_text();
        assert!(
            text.contains("x-terminal-emulator"),
            "install.sh must relaunch inside a terminal emulator when double-clicked"
        );
        assert!(
            text.contains("exec sudo \"${_hv_envs[@]}\" bash \"$HV_SELF\""),
            "install.sh must re-exec itself through sudo to obtain root"
        );
        for knob in [
            "HV_RELEASE_URL",
            "HV_RELEASE_SHA256",
            "HV_RELEASE_TOKEN",
            "HV_RELEASE_BASE",
        ] {
            assert!(
                text.contains(knob),
                "{knob} is no longer forwarded through sudo — the documented override would be dropped"
            );
        }
        // Never `sudo -E`: it is refused by the default sudoers policy.
        // Comment lines are excluded — the file explains the choice above.
        let code_uses_sudo_e = text
            .lines()
            .filter(|l| !l.trim_start().starts_with('#'))
            .any(|l| l.contains("sudo -E"));
        assert!(
            !code_uses_sudo_e,
            "install.sh must not rely on `sudo -E` (refused by default sudoers)"
        );
        // --package and --dry-run must never elevate or pop a window.
        assert!(
            text.contains("--package|-p|--dry-run|--help|-h) HV_BOOTSTRAP=0"),
            "--package/--dry-run/--help must skip the terminal relaunch and sudo elevation"
        );
    }

    #[test]
    fn test_install_sh_reexec_cannot_loop() {
        // Two re-execs (terminal, then sudo) would ping-pong forever if either
        // one came back unprivileged or terminal-less. The stage variable
        // breaks the loop and the second attempt fails loudly instead.
        let text = install_sh_text();
        assert!(
            text.contains("HV_INSTALLER_STAGE"),
            "install.sh must carry a re-exec stage marker to break the bootstrap loop"
        );
        assert!(
            text.contains("== reexec"),
            "install.sh must refuse to elevate twice (sudo came back unprivileged)"
        );
        assert!(
            text.contains("!= terminal"),
            "install.sh must not reopen a terminal when it is already inside one"
        );
    }

    #[test]
    fn test_install_sh_docker_compose_never_breaks_apt() {
        // Regression (2026-10-19): the Docker repository was added with an
        // unguarded `curl … | gpg --dearmor` and an unguarded
        // `apt-get update -qq`. Under `set -euo pipefail` a transient network
        // failure aborted the whole installation at Step 2 — and a broken
        // `docker.list` left on disk breaks every later `apt-get update` on
        // the host. Both are now best-effort and the list file is removed on
        // failure.
        let text = install_sh_text();
        assert!(
            text.contains("add_docker_repo() {"),
            "the Docker repository must live in its own best-effort function"
        );
        assert!(
            text.contains("elif add_docker_repo; then"),
            "a failing Docker repository must not abort the install"
        );
        assert!(
            text.contains("rm -f \"$list\""),
            "a Docker list file that cannot be fetched must be removed again"
        );
        // The distribution package wins over the third-party repository.
        assert!(
            text.contains("pkg_available docker-compose-v2"),
            "prefer docker-compose-v2 (Ubuntu 24.04+/Debian 13+) over adding a third-party repo"
        );
        // A CLI without a reachable daemon must not read as installed.
        assert!(
            text.contains("check \"Docker daemon\"") && text.contains("docker info"),
            "Step 4 must verify the Docker daemon answers, not just that the CLI exists"
        );
        // The Incus repository follows the same rule.
        assert!(
            text.contains("The Incus repository is unreachable"),
            "a failing Incus repository must warn and remove its list file instead of aborting"
        );
    }

    #[test]
    fn test_install_sh_parser_finds_a_known_package_and_rejects_a_bogus_one() {
        // Guards the parser above from silently degrading to "matches
        // anything" (which would make the drift check worthless).
        let installed = install_sh_packages();
        assert!(
            installed.contains("samba"),
            "parser missed a plain install_optional line"
        );
        assert!(
            installed.contains("acl"),
            "parser missed an apt-get-block package"
        );
        assert!(
            installed.contains("ffmpeg"),
            "parser missed a single-line install_packages package listed after the closing label quote"
        );
        assert!(
            !installed.contains("HomeVault"),
            "parser leaked a word from a quoted description string"
        );
    }

    #[test]
    fn test_spec_table_is_well_formed() {
        let specs = specs();
        assert!(
            specs.len() >= 25,
            "the inventory looks truncated: {}",
            specs.len()
        );
        for spec in &specs {
            assert!(!spec.name.is_empty());
            assert!(!spec.package.is_empty());
            assert!(!spec.used_by.is_empty(), "{} has no `used_by`", spec.name);
            assert!(!spec.category.is_empty(), "{} has no category", spec.name);
        }
    }

    #[test]
    fn test_no_duplicate_dependency_names() {
        let specs = specs();
        let mut seen = std::collections::HashSet::new();
        for spec in &specs {
            assert!(
                seen.insert(spec.name),
                "duplicate dependency entry: {}",
                spec.name
            );
        }
    }

    #[test]
    fn test_binary_present_finds_a_universally_available_binary() {
        // `sh` is on every POSIX host at /bin/sh — a smoke test that the
        // detection helper itself works.
        assert!(binary_present("sh", "/bin/sh"));
        assert!(!binary_present(
            "hv-definitely-not-a-real-binary",
            "/hv/nope"
        ));
    }

    #[test]
    fn test_ufw_is_presence_only_not_a_running_daemon() {
        // Regression (2026-09-15): ufw's "active" flag is whether the
        // *firewall* is enabled — a state the admin controls from the
        // Firewall app, not a daemon that must be up. Giving it a `running`
        // check made every startup warn "installed but its service isn't
        // running" on the intended default (firewall off). ufw must stay
        // presence-only: the binary's existence is what gates the app.
        let ufw = specs()
            .into_iter()
            .find(|s| s.name == "ufw")
            .expect("ufw dependency entry is missing");
        assert!(
            ufw.check_running.is_none(),
            "ufw must not have a `running` check — an inactive firewall is the default"
        );
    }

    /// Reads a heredoc body out of `text`: every line after the first line
    /// whose trimmed form contains `start_marker`, up to (not including) the
    /// closing `EOF` line.
    fn heredoc_body<'a>(text: &'a str, start_marker: &str) -> Vec<&'a str> {
        let mut body = Vec::new();
        let mut capturing = false;
        for line in text.lines() {
            if !capturing {
                if line.trim().contains(start_marker) {
                    capturing = true;
                }
                continue;
            }
            if line.trim() == "EOF" {
                break;
            }
            body.push(line);
        }
        assert!(capturing, "no heredoc starting with `{start_marker}` found");
        assert!(!body.is_empty(), "the `{start_marker}` heredoc is empty");
        body
    }

    /// Directives only. Comments and blank lines are excluded: the heredocs
    /// are shell snippets whose comments carry no behaviour to keep in sync.
    fn directives<'a>(lines: impl IntoIterator<Item = &'a str>) -> Vec<&'a str> {
        lines
            .into_iter()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .collect()
    }

    fn installer_file(name: &str) -> String {
        let path = format!("{}/installer/{}", env!("CARGO_MANIFEST_DIR"), name);
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("cannot read {path}: {e}"))
    }

    #[test]
    fn test_install_sh_embeds_a_complete_systemd_unit() {
        // `install.sh` is the only installer file: the systemd unit it writes
        // inline (there is no sibling `homevault.service` any more) is the
        // single source of truth. Pin the load-bearing directives — a silently
        // dropped `ExecStart`/`EnvironmentFile`/... would otherwise only
        // surface on a fresh deploy.
        let install = installer_file("install.sh");
        let unit = directives(heredoc_body(&install, "cat > \"$SERVICE_FILE\" <<EOF"));
        for want in [
            "Description=HomeVault Server",
            "After=network.target",
            "Type=simple",
            "WorkingDirectory=$INSTALL_DIR",
            "ExecStart=$INSTALL_DIR/homevault",
            "Restart=on-failure",
            "Environment=\"PORT=$PORT\"",
            "Environment=\"DATA_DIRECTORY=$DATA_DIR\"",
            "EnvironmentFile=-$ENV_FILE",
            "WantedBy=multi-user.target",
        ] {
            assert!(
                unit.contains(&want),
                "the systemd unit install.sh writes is missing `{want}`"
            );
        }
        // HomeVault deliberately runs as root (see docs/ARCHITECTURE.md):
        // the unit must not grow a User=/Group= line.
        assert!(
            !unit
                .iter()
                .any(|l| l.starts_with("User=") || l.starts_with("Group=")),
            "the HomeVault unit must not pin User=/Group= — it runs as root by design"
        );
    }

    #[test]
    fn test_install_sh_embeds_the_automount_config() {
        // Same single-file contract as the unit above: the udiskie config and
        // daemon unit are written inline, so assert their load-bearing keys.
        let install = installer_file("install.sh");

        let conf = directives(heredoc_body(
            &install,
            "cat > /etc/homevault/udiskie.conf <<",
        ));
        for want in [
            "[General]",
            "automount = true",
            "ignore_internal = true",
            "[Mount Options]",
        ] {
            assert!(
                conf.contains(&want),
                "the udiskie config install.sh writes is missing `{want}`"
            );
        }

        let service = directives(heredoc_body(
            &install,
            "cat > /etc/systemd/system/udiskie.service <<",
        ));
        for want in [
            "After=udisks2.service",
            "ExecStart=/usr/bin/udiskie -c /etc/homevault/udiskie.conf -n",
        ] {
            assert!(
                service.contains(&want),
                "the udiskie unit install.sh writes is missing `{want}`"
            );
        }
    }

    #[test]
    fn test_install_sh_is_self_contained() {
        // The retired siblings (`homevault.service`, `setup-automount.sh`,
        // `optional-packages.conf`, the packaging `Makefile`) were folded into
        // install.sh. It must never grow a dependency on one of them again —
        // a piped `curl | sudo bash` install has no checkout on disk.
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/installer/install.sh");
        let install = std::fs::read_to_string(path).unwrap();
        for sibling in [
            "installer/homevault.service",
            "installer/setup-automount.sh",
            "installer/optional-packages.conf",
            "installer/Makefile",
            "SCRIPT_DIR",
        ] {
            assert!(
                !install.contains(sibling),
                "install.sh still references `{sibling}` — it must stay self-contained"
            );
        }
    }
}

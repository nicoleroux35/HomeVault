# HomeVaultOS — bootable, ready-to-use OS images

**Purpose:** turn HomeVault into an appliance, the way umbreLOS does for
Umbrel: boot a stick or burn a CD, and the machine is already a working NAS —
HomeVault, Docker and every companion package preinstalled, web UI on
`http://homevault.local:8080`. There is **no installer wizard**: the first
boot only provisions storage silently; everything a user configures happens
in the web Setup Wizard.

Everything lives in `os/` (`build.sh`, `test-qemu.sh`, `os/README.md`).

## Architecture

The images are Debian 12 (bookworm) live systems built with **live-build**.
A build does three things:

1. `installer/install.sh --package` produces the application tarball
   (`installer/release/homevault-<arch>.tar.gz`) — the same asset the
   `curl | sudo bash` installer publishes on GitHub Releases.
2. The tarball is extracted into the rootfs at `/opt/homevault/homevault`
   (the backend embeds `static/`, so the UI ships inside the binary — nothing
   is downloaded at boot).
3. live-build assembles the squashfs: packages from
   `os/config/package-lists/0100-homevaultos.list.chroot` (mirroring
   `install.sh`'s dependency groups, plus live-boot, firmware and the
   self-install tooling), hooks in `os/config/hooks/live/` (Docker
   repository for `docker-compose-plugin`, incus from bookworm-backports,
   ClamAV definitions baked in, unit enablement, appliance identity) and
   static files from `os/config/includes.chroot/` (systemd units,
   `hv-firstboot`, hostname, udiskie config).

The result is an iso-hybrid image (BIOS + UEFI, `dd`-able with Rufus/Ventoy/
balenaEtcher): `os/dist/homevaultos-<version>-<variant>-amd64.iso`.

## The two variants

Both share the same system and differ only in
`/etc/default/homevault-os` (`HV_OS_VARIANT`), consumed by
`/usr/local/sbin/hv-firstboot` (unit:
`etc/systemd/system/homevault-firstboot.service`, ordered before
`homevault.service`, `docker.service` and `udiskie.service`).

| | Variant A — `persistent` | Variant B — `selfinstall` |
|---|---|---|
| ISO | `homevaultos-<version>-persistent-amd64.iso` | `homevaultos-<version>-selfinstall-amd64.iso` |
| Boot | live from USB/CD, permanently | live once, then from disk |
| First boot | provisions an `HV-DATA` volume | flashes the **prebuilt disk image** embedded in the ISO onto a blank internal disk (≥ 32 GiB) with `dd` and powers off |
| OS updates | reflash the stick (data kept) | flash a new ISO as variant A, or plain `install.sh` upgrade of the binary |
| Best for | sticks, demo boxes, "the USB *is* the NAS" | fixed mini-PCs/NAS boxes, like umbreLOS x86 |

Variant B also publishes `homevaultos-<version>-amd64.img.xz` — the same
prebuilt image, flashable directly with balenaEtcher/Rufus.

Variant A additionally passes `persistence` on the kernel command line: if
the user creates a partition labelled `persistence` (live-boot format),
OS-level tweaks (hostname changes, extra config) survive reboots too.

### First-boot storage rules (safety-critical)

`hv-firstboot` never destroys existing data. Automatic provisioning only
happens on **completely blank disks**: no partition table (`lsblk PTTYPE`),
no partitions, no filesystem signature (`wipefs`), not read-only. On a live
boot it picks, in order:

1. an existing `HV-DATA` volume by label (mounted, not reformatted — this is
   how data survives across reboots in live mode),
2. the biggest blank disk ≥ 16 GiB,
3. ≥ 8 GiB of free space behind the ISO on the boot stick itself (what makes
   a plain stick self-sufficient in VMs / demo boxes).

On an installed system (variant B after the reboot) the script mounts an
existing `HV-DATA` by label but **never creates volumes** — any further
storage belongs to the Storage app. Docker's data-root is redirected to
`/var/lib/homevault/system/docker` and user homes
(`/home/homevault`) are bind-mounted to `/var/lib/homevault/system/home`
before `docker.service`/`homevault.service` start, so containers, apps and
accounts persist on the data volume.

The first boot also initializes the Incus daemon when the VM stack is
present (`incus admin init --auto` — a fresh daemon starts empty, which
would make the first VM creation fail; `docs/guides/vm.md` → *Host
provisioning*).

### Variant B — prebuilt image + flash (umbreLOS pattern)

The ISO embeds a **prebuilt raw disk image** — `os/build-disk-image.sh`, run
by `build.sh` between the live-build `chroot` and `binary` stages: GPT with a
1 MiB BIOS boot, a 512 MiB ESP and an 8 GiB ext4 root carrying the full
rootfs, with GRUB installed for **both BIOS and UEFI at build time** inside
a chroot of the image itself (grub-install on a live system fails with
`failed to get canonical path of 'overlay'` — the exact failure that killed
the earlier live-clone approach). fstab (FS UUIDs), the `homevault` hostname
and a blanked `machine-id` are baked in as well.

On first boot, `hv-firstboot::selfinstall` only streams bytes:
`xz --decompress --stdout image | dd` onto the biggest blank internal disk
(≥ 32 GiB — never a disk with any existing content) and **powers off**;
remove the media and power back on, like the umbreLOS USB installer. The
flashed system ships no data partition by design: on its first boot,
`prepare_data_storage` claims the free space **behind the OS partitions**
on the system disk as `HV-DATA` (ext4, label, fstab entry) before
`docker.service`/`homevault.service` start — `sgdisk --move-second-header`
first relocates the backup GPT header to the real end of the disk (the
flashed image's GPT still claims the image's size). The installed system's
kernel line also carries a serial console (`console=ttyS0,115200n8`) for
headless debugging, and the system sshd stays disabled — HomeVault's own
SFTP subsystem is the only SSH surface.

The `.img.xz` is also published as a standalone artifact
(`os/dist/homevaultos-<version>-amd64.img.xz`) for direct flashing with
balenaEtcher/Rufus — the third delivery path after both ISOs. After `systemctl
reboot` the box boots from disk; re-booting the same stick never re-clones
(the target disk is no longer blank). Secure Boot must be disabled in the
firmware — enabling signed shim/grub is future work.

## Building

```bash
sudo apt install live-build xorriso      # + debootstrap (pulled by live-build)
sudo ./os/build.sh                       # both variants (~15 GB, needs network)
sudo ./os/build.sh --variant persistent  # variant A only
sudo ./os/build.sh --variant selfinstall # variant B only
sudo ./os/build.sh --fresh-package       # rebuild the app tarball first
sudo ./os/build.sh --skip-clamav         # smaller ISO, no baked virus defs
```

Artifacts: `os/dist/homevaultos-<version>-<variant>-amd64.iso` + `.sha256`.
Build workspaces land in `os/build/` (both gitignored; `--keep` retains one
for debugging). Images are x86_64-only for now; an aarch64 profile would be a
sibling live-build config.

## Testing

```bash
./os/test-qemu.sh os/dist/<iso>              # BIOS
./os/test-qemu.sh os/dist/<iso> --uefi       # UEFI
./os/test-qemu.sh os/dist/<iso> --nographic  # serial console
```

The test VM always gets one blank disk, so:

* variant A: watch `hv-firstboot` format it as `HV-DATA`, then open
  `http://localhost:8080` (port-forward) or the console motd;
* variant B: watch the clone + reboot, then verify the second boot comes from
  the installed disk (`lsblk`, `findmnt /var/lib/homevault`).

Shell validation: `bash -n os/build.sh os/test-qemu.sh
os/config/includes.chroot/usr/local/sbin/hv-firstboot` and on every hook.

Smoke checklist after boot (console or SSH-less): `systemctl status
homevault docker hv-firstboot`, `curl -fsS http://127.0.0.1:8080`,
`findmnt /var/lib/homevault`, `docker info | grep "Docker Root Dir"`,
`avahi-resolve -n homevault.local`.

## Invariants

* Nothing is downloaded at boot: the binary, Docker, packages and (unless
  `--skip-clamav`) virus definitions are inside the image.
* The application layout is identical to `installer/install.sh`
  (`/opt/homevault`, `/var/lib/homevault`, port 8080, `/etc/homevault.env`,
  `homevault.service`) — either packaging path must keep working on the
  result.
* Only blank disks are ever auto-erased; the Storage app owns everything else
  on an installed system.
* `HV_OS_VARIANT` is the single difference between the two ISOs.

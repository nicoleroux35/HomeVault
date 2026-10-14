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
| First boot | provisions an `HV-DATA` volume | clones the system onto a blank internal disk (≥ 32 GiB), installs GRUB, reboots |
| OS updates | reflash the stick (data kept) | flash a new ISO as variant A, or plain `install.sh` upgrade of the binary |
| Best for | sticks, demo boxes, "the USB *is* the NAS" | fixed mini-PCs/NAS boxes, like umbreLOS x86 |

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

### Variant B clone details

The self-install partitions the target as GPT: 1 MiB BIOS boot, 512 MiB ESP,
16 GiB ext4 root, the rest as `HV-DATA`. The rootfs is the *pristine
squashfs* (loop-mounted, `rsync -aHAX`), plus the kernel/initrd from the live
medium. GRUB is installed for both firmware kinds with `--removable
--no-nvram` (fallback `EFI/BOOT/BOOTX64.EFI` path — no NVRAM writes), and a
minimal `boot/grub/grub.cfg` boots `root=UUID=… ro quiet`. `machine-id` and
SSH host keys are reset so every installation is unique. After `systemctl
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

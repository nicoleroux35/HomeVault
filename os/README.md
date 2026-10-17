# HomeVaultOS — bootable, ready-to-use images

HomeVaultOS is the appliance packaging of HomeVault (the umbreLOS model):
the ISO boots straight into a fully preinstalled system — HomeVault, Docker,
every companion package — and serves its UI at `http://homevault.local:8080`.
There is no installer wizard: the first boot only provisions storage, and
configuration happens in the web Setup Wizard.

Full documentation: [`docs/guides/os-image.md`](../docs/guides/os-image.md).

## Variants

| Variant | ISO | Behaviour |
|---------|-----|-----------|
| **A — persistent** (`persistent`) | `homevaultos-<version>-persistent-amd64.iso` | Runs live from the USB stick / CD. First boot provisions an `HV-DATA` data volume (biggest blank disk, or free space on the stick) and mounts it over `/var/lib/homevault`. Reflashing the stick = OS update, data kept. |
| **B — self-install** (`selfinstall`) | `homevaultos-<version>-selfinstall-amd64.iso` | Same ready-to-use live system, but the first boot **flashes the prebuilt disk image embedded in the ISO** (umbreLOS USB-installer pattern: `xz \| dd` onto the blank internal disk ≥ 32 GiB, then power off — remove the stick and power back on). A standalone `homevaultos-<version>-amd64.img.xz` is published alongside for direct flashing. |

Both variants only ever erase **completely blank** disks — existing data is
never touched automatically.

## Build

```bash
sudo apt install live-build xorriso
sudo ./os/build.sh                            # both variants
sudo ./os/build.sh --variant persistent       # A only
sudo ./os/build.sh --variant selfinstall      # B only
sudo ./os/build.sh --fresh-package            # rebuild the app tarball first
sudo ./os/build.sh --skip-clamav              # smaller ISO, no baked AV defs
```

The application tarball comes from `installer/install.sh --package`
(frontend + backend build). Artifacts land in `os/dist/` with a `.sha256`.

## Test in QEMU

```bash
./os/test-qemu.sh os/dist/<iso>               # BIOS boot
./os/test-qemu.sh os/dist/<iso> --uefi        # UEFI boot
./os/test-qemu.sh os/dist/<iso> --nographic   # serial console
```

The VM always gets one blank disk: variant A formats it as `HV-DATA`,
variant B installs itself onto it and reboots from disk.

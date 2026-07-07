# Installing Tendril

> ⚠️ **Pre-1.0.** For testers and contributors — expect rough edges. Tendril ships a bootable
> installer ISO (below), a **web control plane** with a create-station wizard, and a TrueNAS-style
> on-screen console.

## 0. Install from the release ISO (easiest)

Download the single-file installer, verify it, and flash it. `tendril-latest-installer-x86_64.iso`
always points at the newest release (CI rebuilds and republishes it on every push to `main`):

```bash
curl -LO https://dl.onetick.ninja/tendril-latest-installer-x86_64.iso
curl -LO https://dl.onetick.ninja/SHA256SUMS
sha256sum -c SHA256SUMS
sudo dd if=tendril-latest-installer-x86_64.iso of=/dev/sdX bs=4M status=progress
```

The download supports resume (`curl -C -`), so a dropped connection isn't fatal. A specific version is
also available as `tendril-<version>-installer-x86_64.iso` (e.g. `tendril-0.8.0-...`).

Boot the target from the USB stick, follow the installer, then jump to **step 4 (Verify)**. Prefer
to build it yourself? Continue below.

## 1. Prerequisites

- **A machine with an IOMMU** and **VT-d (Intel) / AMD-Vi (AMD) enabled in BIOS/UEFI.** This is a
  firmware toggle — no software (Tendril included) can enable it for you. Enable "VT-d", "AMD-Vi",
  "IOMMU", or "SVM + IOMMU" in your board's setup.
- **At least two GPUs**, *or* an iGPU + a discrete GPU: one drives the host, the rest are passed to
  VMs. (A single GPU can't run both the host and a passthrough VM.)
- A build host with **`podman`** (or Docker) to build the image.
- A target host running **Fedora bootc** (or a system you'll `bootc install` onto).

## 2. Build the host image

From the repo root:

```bash
podman build -f image/Containerfile -t tendril:dev .
```

This produces a Fedora bootc image containing:

- the virtualization stack — `libvirt`, `qemu-kvm`, `edk2-ovmf` (Secure Boot), `swtpm` (TPM 2.0);
- IOMMU kernel arguments (`intel_iommu=on amd_iommu=on iommu=pt`) + early `vfio-pci`;
- the VFIO modules loaded at boot;
- the `tendril-detect` / `tendril-plan` / `tendril-apply` binaries.

Push it to a registry your target can reach if you're not building on the target itself. The
official image is published to Tendril's own Gitea registry:

```bash
podman tag tendril:dev git.onetick.ninja/flan/tendril:latest
podman push git.onetick.ninja/flan/tendril:latest
```

## 2b. Build a bootable USB installer (easiest for end users)

Turn the image into a USB-flashable installer with `bootc-image-builder`:

```bash
scripts/build-installer.sh --type iso     # installer ISO -> flash to USB, boot the target, install
scripts/build-installer.sh --type raw     # raw disk image -> dd straight onto the target's disk
```

Then flash the ISO and boot the target machine from it:

```bash
sudo dd if=out/*.iso of=/dev/sdX bs=4M status=progress
```

> Building the installer needs a host with loopback devices and a privileged container (bare metal or
> a full VM) — it will not run inside an unprivileged LXC.

## 3. Deploy with bootc

**Switch an existing Fedora bootc host:**

```bash
sudo bootc switch git.onetick.ninja/flan/tendril:latest   # or a ref you pushed yourself
sudo reboot
```

**Fresh install to a disk** (from the image, e.g. via a live environment) — see the
[`bootc install`](https://bootc-dev.github.io/bootc/bootc-install.html) docs:

```bash
sudo bootc install to-disk /dev/sdX
```

Updates and rollback are then handled by bootc: `bootc upgrade`, and a bad boot rolls back
automatically (greenboot).

## 4. Verify

After reboot, the primary display auto-logs in and drops you into the **`tendril` console** — a menu
covering hardware, GPU binding, station creation/management, media, networking, and power. (Other
VTs and SSH give a normal shell; run `tendril` there to open the menu manually.) Its "Hardware &
capabilities" entry is the same as:

```bash
tendril-detect
```

- **GPUs listed with IOMMU groups** → IOMMU is working. Passthrough-capable GPUs are flagged.
- **No IOMMU groups** → VT-d / AMD-Vi is still disabled in BIOS; go back to step 1.

Preview the passthrough config for your GPUs (changes nothing):

```bash
tendril-plan          # what would be bound
tendril-apply         # dry-run: the exact sysfs writes
```

Only `tendril-apply --execute` actually binds a GPU to `vfio-pci` (detaching it from the host) — do
that when you're ready to hand it to a VM.

## 5. Create a Windows station (unattended)

Fetch the install media (once), then let `tendril-guest` build the disk and install Windows itself —
no clicking through Setup:

```bash
scripts/fetch-windows-media.sh --dest /var/lib/tendril/isos     # win11.iso + virtio-win.iso

sudo tendril-guest \
  --name station1 --create-disk --size-gib 128 \
  --iso /var/lib/tendril/isos/win11.iso \
  --virtio-iso /var/lib/tendril/isos/virtio-win.iso \
  --unattend --username player --password changeme \
  --start
```

`--unattend` builds a seed ISO carrying `autounattend.xml`, which injects the virtio storage driver
(so the disk is visible), auto-partitions, skips the OOBE / Microsoft-account screens, creates the
local account, and installs the virtio guest tools on first logon. Watch it on the VNC console
(`virsh domdisplay station1`). By default the station's GPU (its whole IOMMU group) is passed
through; add `--no-gpu` to install headless first and attach the GPU later.

Once Windows has installed itself and rebooted to the desktop, drop the install media so it boots
straight from disk:

```bash
sudo tendril-guest --name station1 --finalize --start
```

### SteamOS-style (Bazzite) station

Valve ships no generic-PC SteamOS installer (the Steam Deck recovery image is image-based and
AMD-only, so it can't drive an NVIDIA station). Until it does, Tendril's SteamOS station is
[Bazzite](https://bazzite.gg) — an atomic, Steam-gaming-mode image with a scriptable Anaconda ISO:

```bash
scripts/fetch-steamos-media.sh --dest /var/lib/tendril/isos   # Bazzite Deck/NVIDIA ISO

sudo tendril-guest \
  --steamos --name station2 --create-disk --size-gib 128 \
  --iso /var/lib/tendril/isos/bazzite-deck-nvidia.iso \
  --unattend --username player --password changeme \
  --start
```

`--unattend` builds a `ks.cfg` kickstart on an `OEMDRV`-labelled seed ISO, which Anaconda auto-loads:
it wipes the disk, installs the image, creates the user, enables SSH, and auto-logs into Steam gaming
mode. Override the image with `--image ghcr.io/ublue-os/bazzite-deck:stable` (e.g. AMD/Intel), or
`--no-ssh`. Then `--finalize --start` to boot from disk. `--native-hardware` applies the opt-in
fingerprint-reduction overlay (read the ToS warnings in the README first).

## What's not here yet

The graphical "create a gaming station" wizard and multi-seat peripheral binding are implemented (see
the README's "What works today"). **vGPU** (>1 VM per GPU, via mdev or SR-IOV) is implemented but
experimental and needs validation on real vGPU hardware — see **[docs/VGPU.md](VGPU.md)** for the
supported-card list and setup. **Clustering** is on the [roadmap](../README.md#roadmap) but not
implemented. The CLIs above already take a host from bare metal to a running, self-installed Windows
station.

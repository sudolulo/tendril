# Installing Tendril

> ⚠️ **Pre-1.0.** There's a bootable installer ISO (below), but no graphical VM wizard yet — for
> testers and contributors. Expect rough edges.

## 0. Install from the release ISO (easiest)

Download the installer from the [latest release](https://git.onetick.ninja/flan/tendril/releases).
It's split into `.part` files (the host caps release assets at 2 GiB); reassemble, verify, and flash:

```bash
cat tendril-*-installer-x86_64.iso.part* > tendril-installer.iso
sha256sum -c SHA256SUMS
sudo dd if=tendril-installer.iso of=/dev/sdX bs=4M status=progress
```

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

Push it to a registry your target can reach if you're not building on the target itself:

```bash
podman push tendril:dev registry.example/you/tendril:dev
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
sudo bootc switch localhost/tendril:dev   # or the registry ref you pushed
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

After reboot:

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

## What's not here yet

Creating and running the Windows/SteamOS VMs, the setup wizard, multi-seat peripherals, vGPU, and
clustering are on the [roadmap](../README.md#roadmap) but not implemented. For now Tendril gets the
*host* ready for passthrough; the VM layer is next.

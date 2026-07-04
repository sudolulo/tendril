# Installing Tendril

> ⚠️ **Pre-release.** Tendril does not yet ship a published image or a turnkey installer. This guide
> covers building the host image yourself and deploying it with [`bootc`](https://bootc-dev.github.io/bootc/).
> Expect rough edges; this is for testers and contributors.

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

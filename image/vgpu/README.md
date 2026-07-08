# vGPU host-driver image variants

Tendril's host is an **immutable bootc image**, so you don't install a GPU driver into the running
system — you **bake it into a derived image and reboot into it**. These variants layer a vGPU host
driver onto the base Tendril image; after `bootc switch` + reboot, the driver's mdev/SR-IOV profiles
appear and Tendril's vGPU features (the create-station profile picker, the Hardware SR-IOV control)
light up automatically.

> **Status: scaffolding — not yet validated on real hardware.** Kernel-module builds are
> version- and card-specific; treat these as a correct starting point to test against your gear, not a
> guaranteed one-shot. In particular verify: kernel-devel matches the image kernel, Secure Boot module
> signing (if host SB is on), and the exact driver/card support matrix.

## Build model

1. Build the base Tendril image first: `podman build -f image/Containerfile -t localhost/tendril:dev .`
2. Build a variant on top of it (see below), which produces e.g. `localhost/tendril:vgpu-nvidia`.
3. Deploy it: `sudo bootc switch localhost/tendril:vgpu-nvidia && sudo reboot` (or push it to your
   registry and switch to that).

The wrapper `scripts/build-vgpu-variant.sh nvidia|amd` builds the base (if needed) and the variant.

## AMD — MxGPU / GIM (fully automated)

`Containerfile.amd-gim` clones AMD's **open-source** GIM (GPU-IOV Module) and builds it as a kernel
module. Nothing to supply — it's redistributable, so this can even be published prebuilt.

```bash
scripts/build-vgpu-variant.sh amd
```

Supports AMD's SR-IOV-capable pro/datacenter cards (FirePro S7150, Instinct MI-series, …) — **not**
consumer Radeon (which has no vGPU at all).

## NVIDIA — vGPU + vgpu_unlock (you supply the driver)

The NVIDIA vGPU host driver is **licensed and non-redistributable**, so Tendril can't ship it. Get the
`.run` legitimately, then the build automates the rest (installing it, applying
[`vgpu_unlock-rs`](https://github.com/mbilker/vgpu_unlock-rs) for consumer GeForce cards, and enabling
the `nvidia-vgpud` / `nvidia-vgpu-mgr` services).

**Where to get the `.run` (legitimately):**
- **NVIDIA vGPU 90-day evaluation** — free signup at NVIDIA's site grants access to the vGPU software
  downloads (the host `.run`). This is the no-cost, lawful path for homelab use.
- Or your existing **NVIDIA Licensing Portal / Enterprise** account.

Tendril will **not** download it from unofficial mirrors — that would redistribute NVIDIA's licensed
binary against their terms.

```bash
# Drop your host driver here (git-ignored), then:
cp NVIDIA-Linux-x86_64-<ver>-vgpu-kvm.run image/vgpu/nvidia-vgpu.run
scripts/build-vgpu-variant.sh nvidia
# (or point the script at a URL you have access to: NVIDIA_VGPU_RUN_URL=https://… scripts/build-vgpu-variant.sh nvidia)
```

For **consumer GeForce** cards the build enables `vgpu_unlock` automatically (see
[docs/VGPU.md](../../docs/VGPU.md) for the supported-card list). For **datacenter** cards it's the
official path (you'll also point at your license server via the NVIDIA licensing client).

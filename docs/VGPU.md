# vGPU — splitting one GPU across multiple stations

Tendril can hand a single GPU to more than one station using **vGPU**. Two mechanisms are supported,
detected per-GPU straight from sysfs:

- **Mediated devices (mdev)** — the NVIDIA vGPU / `vgpu_unlock` / Intel GVT-g path. Tendril reads each
  GPU's `mdev_supported_types`, the create-station wizard lists the available profiles, and choosing
  one creates a persistent mediated device (`mdevctl define --auto` + `start`) that's attached to the
  station as `<hostdev type='mdev'>`.
- **SR-IOV** — for GPUs that advertise `sriov_totalvfs` (AMD MxGPU, Intel Data Center GPU). The
  **Hardware** page has an inline control to enable *N* virtual functions; the VFs then appear as their
  own GPUs and are passed through with the normal whole-GPU path.

**Getting the host driver on the box.** Because the host is an immutable bootc image, the vGPU driver is
**baked into a derived image and booted into**, not installed live — see
[image/vgpu/](../image/vgpu/README.md) for the build variants:
- **AMD MxGPU/GIM** — fully automated (open source), `scripts/build-vgpu-variant.sh amd`.
- **NVIDIA vGPU + `vgpu_unlock`** — you supply the licensed `.run` (from NVIDIA's free vGPU eval or
  licensing portal — Tendril won't fetch it from mirrors); the build applies `vgpu_unlock-rs` and
  enables the host services: `scripts/build-vgpu-variant.sh nvidia`.

Once the driver is in place (after reboot), whatever it exposes shows up automatically in the wizard's
profile picker or the SR-IOV control. Whole-GPU passthrough is unchanged and remains the reliable
default. See also the capability model in [docs/PLAN.md](PLAN.md).

> **Status:** implemented, but **not yet validated on real vGPU hardware** — the dev box has no
> mdev-capable driver. The sysfs paths, `mdevctl` calls, and mdev domain XML follow standard
> libvirt/kernel conventions; the first run on an actual card is the real test.

## Which cards support vGPU

### NVIDIA — mdev

**Official vGPU** (needs a licensed NVIDIA vGPU / GRID host driver) — datacenter / pro cards:

| Architecture | Cards |
|---|---|
| Maxwell | M10, M60 |
| Pascal | P4, P40, P100, P6 |
| Volta | V100 |
| Turing | T4, Quadro RTX 6000 / 8000 |
| Ampere | A100, A40, A30, A16, A10, A2 |
| Ada | L4, L40, L40S |

**Consumer GeForce via `vgpu_unlock`** (unofficial — this is the realistic *gaming* path). Works on
consumer cards whose silicon has a datacenter twin, using a patched vGPU host driver plus
[`vgpu_unlock-rs`](https://github.com/mbilker/vgpu_unlock-rs):

| Architecture | Cards | Notes |
|---|---|---|
| **Turing** | **RTX 20-series, GTX 16-series** | Best-supported — the classic target |
| Pascal | GTX 10-series | Supported |
| Maxwell | GTX 900-series | Supported |
| Ampere | RTX 30-series | Works with newer patches |
| Ada | RTX 40-series | Experimental / limited |

### Intel

| Mechanism | Hardware | Notes |
|---|---|---|
| **GVT-g** (mdev) | Integrated GPUs, ~Broadwell → Gen11 (5th–10th gen Core HD/UHD/Iris) | **Deprecated / removed** in recent kernels |
| **SR-IOV** | Data Center GPU **Flex**, **Arc** A-series, 12th/13th-gen+ iGPUs | The current path, on recent `i915` / `xe` drivers |

### AMD

| Mechanism | Hardware | Notes |
|---|---|---|
| **MxGPU** (SR-IOV) | FirePro S7150(x2), Radeon Pro V340 / V520 / V620, Instinct MI-series | Datacenter / pro only |
| — | **Consumer Radeon RX (any gaming card)** | **No vGPU at all** — not mdev, not SR-IOV. Whole-card passthrough only. |

## The short version for a gaming box

| You have | vGPU? | How |
|---|---|---|
| **NVIDIA RTX 20-series (Turing)** | ✅ best bet | mdev via `vgpu_unlock` |
| NVIDIA GTX 10 / 16, RTX 30 | ✅ likely | mdev via `vgpu_unlock` |
| NVIDIA RTX 40 | ⚠️ experimental | mdev via `vgpu_unlock` |
| NVIDIA datacenter (T4, A40 …) | ✅ official | mdev (licensed) |
| Intel Arc / Flex / recent iGPU | ✅ | SR-IOV |
| AMD Radeon RX (gaming) | ❌ | passthrough whole card only |

If you're buying specifically to split one card across stations, a used **Turing** card (RTX
2060/2070/2080) is the best-documented `vgpu_unlock` target.

## Using it in Tendril

1. Get the vGPU host driver onto the box by building + deploying a driver variant — AMD is automated,
   NVIDIA takes your supplied `.run` (see [image/vgpu/](../image/vgpu/README.md)) — then reboot.
2. **mdev:** open **New station** → the **GPU** dropdown lists each available vGPU profile
   ("… — vGPU: GRID RTX6000-4Q (2 free)") alongside whole-GPU options. Pick one; Tendril creates the
   mediated device and attaches it. Deleting the station tears the mdev down again.
3. **SR-IOV:** on the **Hardware** page, set the number of virtual functions on a capable GPU. The VFs
   then appear as their own GPUs and can be assigned to stations like any whole GPU.

The Hardware page's **vGPU** column shows each GPU's mdev profiles / SR-IOV VFs, and **Used by** shows
which stations hold a slice.

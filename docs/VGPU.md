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
**baked into a derived image and booted into**, not installed live:

- **NVIDIA (easiest — no repo checkout):** on the **System** page's **vGPU** panel, upload the
  licensed `NVIDIA-…-vgpu-kvm.run` (or give a URL you're entitled to), click **Build vGPU image** —
  the appliance builds the variant from its own running image — then
  `sudo bootc switch localhost/tendril:vgpu-nvidia && sudo reboot`. Get the `.run` from NVIDIA's
  free vGPU eval or your licensing portal; Tendril won't fetch it from mirrors.
- **AMD MxGPU/GIM** — fully automated (open source): switch to the published
  `git.onetick.ninja/flan/tendril:vgpu-amd` variant, or build it yourself.
- **From a repo checkout** — `scripts/build-vgpu-variant.sh amd|nvidia` builds either variant with
  podman; see [image/vgpu/](../image/vgpu/README.md). The NVIDIA build applies `vgpu_unlock-rs` and
  enables the host services.

Once the driver is in place (after reboot), whatever it exposes shows up automatically in the wizard's
profile picker or the SR-IOV control. Whole-GPU passthrough is unchanged and remains the reliable
default. See also the capability model in [docs/PLAN.md](PLAN.md).

> **Community reference (NVIDIA).** [wvthoog's Proxmox vGPU guide](https://wvthoog.nl/proxmox-vgpu-v3/)
> is a thorough walkthrough of the NVIDIA vGPU + `vgpu_unlock` process (driver install, consumer-card
> unlock, `mdevctl`, licensing). It targets Proxmox, but the driver/unlock steps map directly onto our
> image variant. **Caveat:** that guide auto-downloads the host `.run` from an unofficial mirror —
> don't. Get your `.run` from NVIDIA's free
> [90-day vGPU evaluation](https://www.nvidia.com/en-us/data-center/resources/vgpu-evaluation/) (or your
> licensing portal), which is the only lawful source, and supply that to the build.

## Guest licensing (NVIDIA) — FastAPI-DLS

The host `.run` makes vGPU *work*; NVIDIA's licensing makes it run *un-throttled*. Each guest's vGPU
driver leases a license on boot — unlicensed, it runs degraded and drops sessions (~24 h). The official
path is an NVIDIA DLS/CLS license server; the common self-hosted path is
[FastAPI-DLS](https://git.collinwebdesigns.de/oscar.krause/fastapi-dls), a minimal DLS re-implementation
guests lease from. Tendril can run it for you (opt-in) — see the licensing section of the
**vGPU** panel on the **System** page (it auto-starts once the host driver is active). It's separate from the `.run`: you need the driver regardless; DLS only removes the
throttle. Emulating NVIDIA's licensing is a gray area — enable it only with your own entitlement.

### Automatic guest setup — fully invisible

The guest also needs the **GRID guest driver installed inside it** — separate from the host `.run` and
part of the same licensed package. In Tendril this is **completely automatic**: staging the host
`.run` pins the vGPU release, and Tendril fetches BOTH matching guest installers (Windows `.exe` and
Linux `.run`) from **NVIDIA's own public distribution bucket** (`nvidia-drivers-us-public` — the same
source GCP pulls from). You never pick, upload, or see a guest driver.

When a hands-off station is bound to an NVIDIA vGPU (mdev) slice, the driver rides the seed disc and
installs on first logon/boot; if the built-in license server is running, the station also fetches its
licensing token automatically — a fresh vGPU station comes up **licensed and un-throttled with zero
manual steps**. The cache is keyed to the host driver branch, so upgrading the host driver
automatically re-fetches the matching guest drivers. Whole-GPU-passthrough stations are unaffected
(they get their driver from Windows Update / the vendor). An unlisted host branch or an air-gapped
box can point at a reachable copy via `TENDRIL_VGPU_GUEST_EXE_URL` / `TENDRIL_VGPU_GUEST_RUN_URL`.

Fetching from NVIDIA's own public URL is retrieval-from-source, not redistribution, but the vGPU
EULA still gates *use* — the same entitlement that got you the host `.run` covers the guests. The
SteamOS guest install is a first-boot oneshot; because Bazzite is atomic it's best-effort (the
durable path is layering the driver into the image) and **unvalidated on real hardware**.

The **New station** wizard can additionally bake in **Steam**, **Sunshine** (game-stream host — a
seatless station needs it), **Discord**, and a **Moonlight** receiver, fetched from their official
URLs on first boot. A vGPU station's slice can later be changed **without touching its disk** — the
station page's **GPU split** panel re-slices and the station boots into the new profile with
Windows, games, and saves intact.

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

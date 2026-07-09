# Tendril — Design & Build Plan

> An open-source, Fedora-bootc–based operating system that turns a
> single multi-GPU machine (or a cluster of them) into multiple plug-and-play gaming stations,
> each running Windows or SteamOS in a GPU-passthrough VM.

---

## 1. Vision & scope

**Goal:** Let a gamer take a box with N GPUs and, with as little manual work as possible, run up to
N independent gaming stations — each a Windows or SteamOS VM with a real GPU passed through, its own
monitor/keyboard/mouse. Detect hardware automatically, install the right drivers, and be
"can't-break-it" reliable for non-expert users.

**Primary primitive:** full **GPU passthrough** (1 GPU → 1 VM). This always works on consumer
hardware and is what every "N gamers, 1 PC" build uses.

**Future primitive:** **vGPU splitting** (>1 VM per GPU) — official (datacenter NVIDIA / AMD MxGPU)
and unofficial (`vgpu_unlock` on consumer NVIDIA), added behind a capability gate. Designed for now,
shipped later.

**Non-goals (initially):** live migration of running VMs (passthrough makes this impossible),
guaranteed anti-cheat bypass, and enterprise HA.

---

## 2. Decision log (locked)

| # | Decision | Choice | Why |
|---|---|---|---|
| D1 | Host base OS | **Fedora bootc / atomic** | Self-healing atomic rollback (greenboot) = can't-break-it appliance for non-expert gamers; CI-built versioned images for safe OTA updates; proven NVIDIA-on-atomic path (Bazzite/uBlue). Beats Debian for a *distributed* product. |
| D2 | VM engine | **Direct libvirt/QEMU (own orchestrator)** | Passthrough/vGPU/CPU-pinning/Secure-Boot/fingerprint overlays need low-level domain-XML control. Incus's abstraction ceiling caused TrueNAS to revert to libvirt (missing Secure Boot, VNC). Our needs are deeper than theirs. |
| D3 | Splitting strategy | **Passthrough-first, vGPU later** | Consumer GPUs don't officially support vGPU. Ship the reliable path; add vGPU as a detected bonus. |
| D4 | Display model | **Physical monitors first, streaming (Sunshine/Moonlight) later** | Lowest latency for local multi-seat now; streaming backend designed as a swappable module for later + for clustering. |
| D5 | Multi-machine | **Federation, not clustering** (superseded 2026-07-09; was: controller/agent clustering as a later phase) | Every node stays a fully self-managing control plane; peers aggregate over the token/mTLS JSON API (see docs/FEDERATION.md) — no consensus, no elected controller, no fencing. Placement is GPU-aware at create time; cold re-home covers node loss. NOT live migration. |
| D6 | Guests | **Windows + SteamOS** | The two headline gaming targets. SteamOS via a VM-friendly image (HoloISO/Bazzite-style). |
| D7 | Anti-cheat / "native hardware" | **Optional per-VM compatibility overlay, off by default, gated with warnings** | Generic VM-fingerprint reducer for picky software; honest ToS/ban warnings; not marketed as a bypass. |
| D8 | Distribution | **Open source, for gamers** | Drives the reliability + reproducible-update priorities above. |

---

## 3. High-level architecture

```
┌──────────────────────────────────────────────────────────────────────┐
│  HOST OS  — Fedora bootc (immutable root, A/B, greenboot auto-rollback)│
│   • kernel w/ IOMMU pre-flagged (intel_iommu / amd_iommu, iommu=pt)    │
│   • KVM + QEMU + libvirt + VFIO baked into the image                   │
│   • NVIDIA/AMD host drivers layered into image variants                │
├──────────────────────────────────────────────────────────────────────┤
│  CAPABILITY ENGINE  (Rust daemon)                                      │
│   enumerate GPUs → IOMMU-group analysis → capability matrix            │
│   {passthrough | vgpu-official | vgpu-unlock | host-only}             │
├──────────────────────────────────────────────────────────────────────┤
│  PROVISIONING LAYER   (trait: ProvisioningStrategy)                    │
│   passthrough.rs (v1) | vgpu_nvidia.rs (v2) | vgpu_unlock.rs (v3)      │
│   → drives image layering + reversible host state                     │
├──────────────────────────────────────────────────────────────────────┤
│  ORCHESTRATOR   controller  ⇄  per-node agent(s)   (libvirt)           │
│   station lifecycle · GPU-aware scheduling · domain-XML templating     │
│   overlays: cpu-pinning · secure-boot+TPM · native-hardware (opt-in)   │
├──────────────────────────────────────────────────────────────────────┤
│  OUTPUT BACKENDS   (trait: OutputBackend)                              │
│   physical.rs (v1: monitor + USB-controller/seat) | streaming.rs (fut) │
├──────────────────────────────────────────────────────────────────────┤
│  GUEST IMAGE BUILDER   Windows (virtio + autounattend) · SteamOS       │
├──────────────────────────────────────────────────────────────────────┤
│  CONTROL PLANE   Web UI (Proxmox-like, gaming-first) + first-boot wizard│
└──────────────────────────────────────────────────────────────────────┘
```

Two planes on disk:
- **OS plane** = the immutable bootc image (swappable, rollback-able).
- **Data plane** = writable storage pool (`/var` or a dedicated btrfs/ZFS/LVM-thin pool) holding VM
  disks, golden images, and mutable config. Host updates never touch it.

---

## 4. Controller / agent architecture (the orchestrator)

> **Superseded (2026-07-09):** multi-machine went the *federation* route instead (docs/FEDERATION.md)
> — independent nodes aggregating over the web layer's JSON API, with GPU-aware placement and cold
> re-home. No clustered controller/agent split is planned; the `tendrild`/`Role` scaffold has been
> removed. The diagram below is kept for the record of what was considered.

Single binary, two roles. Ship single-node now; the same design scales to a cluster with no rewrite.

```
                 ┌─────────────── CONTROLLER (one elected node) ───────────────┐
                 │  • REST/gRPC API  ← Web UI, CLI                             │
                 │  • cluster state store (embedded: SQLite→raft later)        │
                 │  • scheduler: place station on node w/ free compatible GPU  │
                 │  • golden-image registry / distribution                     │
                 └───────────────┬───────────────────────┬────────────────────┘
                                 │ (mTLS)                 │
                    ┌────────────▼─────────┐   ┌──────────▼───────────┐
                    │  AGENT (node A)      │   │  AGENT (node B)      │
                    │  • talks to libvirt  │   │  • talks to libvirt  │
                    │  • runs cap-engine   │   │  • runs cap-engine   │
                    │  • VFIO bind, USB    │   │                      │
                    │  • reports GPU state │   │                      │
                    └──────────────────────┘   └──────────────────────┘
```

- **Single-node mode:** controller + agent in the same process; no network, no cluster DB overhead.
- **Cluster mode:** one node runs the controller (leader-elected); every node runs an agent.
- **Scheduling is GPU-aware:** the controller knows each node's GPUs, their capability class, and
  which are free/assigned, and places a new station accordingly. Cold migration = re-place + move
  disk. **Live migration is explicitly unsupported** for passthrough VMs (documented, not hidden).
- **Storage rule:** working VM disks stay **local NVMe** per node (latency); only the **golden-image
  library** is shared/replicated.

---

## 5. Component breakdown

### 5.1 Capability engine (Rust)
- Enumerate PCI devices (sysfs / `lspci -nnk`): GPU vendor, model, PCI IDs, reset method
  (`function_level_reset`, vendor reset quirks — AMD reset bug).
- **IOMMU group analysis** — the make-or-break for passthrough. Flag GPUs sharing a group with
  critical devices (ACS-override needed → security caveat surfaced to user).
- Detect VT-d/AMD-Vi enabled; if disabled, **instruct user to enable it in BIOS** (host can't).
- Reserve one GPU/iGPU for the host console.
- Emit a **capability matrix**: PCI ID → `{passthrough | vgpu-official | vgpu-unlock | host-only}`.

### 5.2 Provisioning layer (trait `ProvisioningStrategy`)
- `passthrough` (v1): bind target GPUs to `vfio-pci` (image config + kernel cmdline + initramfs),
  blacklist native driver grabbing them.
- `vgpu_nvidia` (v2): vGPU host driver, `mdev`/SR-IOV vGPU types, licensing-server config.
- `vgpu_unlock` (v3, experimental): consumer NVIDIA via `vgpu_unlock-rs`, gated + warned.
- All host mutation is **image-layer based** (bootc) → atomic + auto-rollback for free. A generic
  base image + a per-hardware layer computed at first boot (see §6.1).

### 5.3 Orchestrator (libvirt domain templating)
- Base domain template + composable **overlays**:
  - `cpu-pinning` (vCPU pinning, isolated CPU sets, hugepages — big for gaming latency)
  - `secure-boot-tpm` (OVMF Secure Boot + swtpm — **required for Windows 11**)
  - `native-hardware` (opt-in fingerprint reducer — §8)
  - `gpu-passthrough` / `gpu-vgpu`
  - `usb-seat` (whole USB controller per station; evdev fallback)
- Golden image + per-VM overlay disks (qcow2 backing files or ZFS/LVM-thin clones) → near-instant
  new stations.

### 5.4 Output backends (trait `OutputBackend`)
- `physical` (v1): drive a real monitor off the passed-through GPU; route a USB controller (+ audio)
  per seat; hot-plug support.
- `streaming` (future): Sunshine in-guest + Moonlight clients; unlocks "play any station from
  anywhere" — especially powerful combined with clustering.

### 5.5 Guest image builder
- **Windows:** unattended `autounattend.xml`, virtio driver injection, GPU driver auto-install,
  Sunshine pre-staged for future streaming. Secure Boot + TPM enabled by default (Win11).
- **SteamOS:** VM-friendly image (HoloISO / Bazzite-style), boots to Gaming Mode.
- Golden images published to the controller's registry; nodes pull them.

### 5.6 Control plane (Web UI + wizard)
- Proxmox-like but **gaming-first**. Core flow = **"Create Gaming Station" wizard**:
  detected GPUs shown with green/yellow/red capability badges → pick GPU → pick OS → assign
  peripherals → launch. VFIO/IOMMU complexity hidden; "advanced" drawer for tinkerers.

---

## 6. Key flows

### 6.1 First-boot / two-stage hardware detection (the immutability resolver)
1. Machine boots the **generic base image**.
2. Capability engine enumerates GPUs + IOMMU groups → capability matrix.
3. Provisioning computes the **needed host layers** (which GPU drivers, VFIO binds, vGPU modules).
4. System **deploys the tailored bootc deployment and reboots** into it.
5. If the new deployment fails health checks, **greenboot auto-reverts** to the generic image — the
   user's box never bricks. This is how we get runtime hardware adaptation *and* immutability.

### 6.2 Create a gaming station
Wizard → controller schedules onto a node with a free compatible GPU → agent renders domain XML
(base + overlays) → clones golden image → binds GPU + USB seat → starts VM → monitor lights up.

### 6.3 Update & rollback
CI builds a new versioned host image → nodes pull + stage → A/B reboot → greenboot verifies →
auto-rollback on failure. VM data plane untouched throughout.

---

## 7. Cross-cutting concerns

- **Networking:** virtio-net + a host bridge per station; optional passed-through NIC for the
  native-hardware profile.
- **Storage:** local NVMe pool for VM disks; shared/replicated store only for golden images.
- **Security & honesty:**
  - `native-hardware` overlay **off by default**, wizard confirmation gate, explicit ToS/ban
    warning, no per-anti-cheat targeting, logged as user choice. Framed as *compatibility*, not
    *bypass*. Kernel-level anti-cheats (Vanguard, strict EAC/BattlEye) will still block — stated
    plainly.
  - ACS-override, if ever needed for bad IOMMU groups, surfaced with its security caveat.
- **Reversibility:** every host change is an image layer → atomic rollback is the default safety net.

---

## 8. Tech stack

| Layer | Choice |
|---|---|
| Host OS | Fedora bootc (Containerfile-defined, CI-built images) |
| Hypervisor | KVM + QEMU + libvirt + VFIO (mainline) |
| Capability engine / provisioning / orchestrator | Rust (per-node binaries; multi-node via web-layer federation) |
| State store | SQLite (single-node) → raft/embedded consensus (cluster) |
| API | REST/gRPC over mTLS |
| Web UI | (TBD framework) served by controller |
| Guest streaming (future) | Sunshine (host) + Moonlight (client) |
| Firmware | OVMF (Secure Boot) + swtpm (TPM 2.0) |

---

## 9. Phased roadmap

| Phase | Deliverable | Exit criteria |
|---|---|---|
| **0 — Spike** | Manual single-GPU passthrough to one Windows VM on plain Fedora | One Windows-11 VM with a real GPU + monitor + USB, booting reliably. Prove IOMMU/VFIO/reset on target HW *before* building anything. |
| **1 — Base image + single station** | Fedora bootc image (IOMMU pre-flagged, libvirt/VFIO baked) + capability engine v1 + passthrough provisioning + orchestrator single-node + physical output + Windows & SteamOS golden images | From a fresh install: auto-detect GPUs, create 1 station via wizard, play a game. Greenboot rollback works. |
| **2 — Multi-GPU multi-seat** | N GPUs → N stations; USB-controller seat routing; CPU pinning/hugepages; per-seat audio | 2+ simultaneous independent gaming stations on one box, each with own monitor/peripherals. |
| **3 — vGPU (>1 VM/GPU)** | `vgpu_nvidia` (official datacenter) then `vgpu_unlock` (consumer, experimental gate) | More stations than physical GPUs on supported hardware. |
| **4 — Clustering** | Controller/agent multi-node; GPU-aware scheduling; golden-image distribution; cold migration | Manage stations across ≥2 machines from one UI; place a new station on any node with a free GPU. |
| **5 — Streaming** | Sunshine/Moonlight `streaming` output backend | Play a station headless from another device; cluster + streaming = play any station from anywhere. |

**MVP = end of Phase 2:** multiple real gaming stations on one multi-GPU box, plug-and-play,
un-brickable. Everything after is expansion.

---

## 10. Top risks & mitigations

| Risk | Mitigation |
|---|---|
| GPU reset bugs (esp. AMD) prevent VM restart | Detect reset method in cap-engine; document unsupported cards; apply known reset quirks/vendor-reset. |
| Bad IOMMU grouping blocks passthrough | Analyze groups up front; surface ACS-override with security caveat; recommend compatible boards. |
| NVIDIA drivers on immutable root | Follow the proven Bazzite/uBlue akmod-layering pattern; bake per-variant images. |
| vGPU (esp. `vgpu_unlock`) fragile across driver updates | Pin driver+module versions per image variant; keep experimental, gated, opt-in. |
| Anti-cheat bans from native-hardware overlay | Off by default; explicit warnings; position product around the huge VM-friendly game library. |
| Immutability vs runtime detection | Solved by the two-stage first-boot pattern (§6.1) + greenboot auto-rollback. |
| Scope creep delays a usable product | MVP is Phase 2; clustering/vGPU/streaming are strictly later. |

---

## 11. Open questions (still to decide)

1. **Project name** (GameHost is a placeholder).
2. **Web UI framework** (and whether a local TUI ships alongside).
3. **Golden-image distribution mechanism** for clustering (OCI registry? built-in?).
4. **Peripheral routing default:** whole USB-controller passthrough vs. evdev — per-hardware policy.
5. **Which GPUs to officially "bless"** at launch (a supported-hardware list reduces support load).
6. ~~**Licensing** (GPL/Apache/etc.) and governance for the open-source project.~~ **Decided
   (2026-07-08): AGPL-3.0-only, dual-licensed commercially** — one open edition, no feature gating;
   revenue model = stable-channel assurance + commercial licenses. See [LICENSING.md](../LICENSING.md),
   [CLA.md](../CLA.md) (single-owner copyright), and [CHANNELS.md](CHANNELS.md) (dev/latest/stable).

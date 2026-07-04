# Tendril

**Tendril** is an open-source, [Fedora bootc](https://docs.fedoraproject.org/en-US/bootc/)–based
operating system that turns a single multi-GPU machine (or a cluster of them) into multiple
plug-and-play gaming stations — each running **Windows** or **SteamOS** in a GPU-passthrough VM. It
auto-detects your GPUs, sets up the drivers, and aims to be "can't-break-it" reliable for non-expert
users. Think of the DIY Proxmox passthrough build, but automated and gaming-first.

> One machine, many tendrils.

## What is it for?

One powerful box with several GPUs → several independent gaming setups at once. Two people gaming on
one tower, a handful of Steam Machines driven from a closet server, a Windows VM for the games that
need it next to a SteamOS VM for everything else. Tendril handles the hard parts — IOMMU, VFIO,
driver binding, VM setup — so you don't hand-edit GRUB and `vfio.conf` to get there.

- **Passthrough-first:** N GPUs → N independent stations (the reliable path on consumer hardware).
- **Self-healing host:** atomic bootc images with greenboot auto-rollback — a bad update can't brick the box.
- **Own libvirt orchestrator:** full control of passthrough, CPU pinning, and Secure Boot + TPM (for Windows 11).
- **vGPU & clustering later:** more VMs per GPU, and management across multiple machines.

## What works today

Tendril is **pre-release**. The host-side provisioning pipeline runs and is validated on real
hardware; there is not yet a turnkey installer or VM wizard. The building-block CLIs:

| Tool | What it does |
|---|---|
| `tendril-detect` | Enumerates GPUs + IOMMU groups, classifies each as passthrough / host-only |
| `tendril-plan` | Computes the exact `vfio-pci` bind set for a GPU (its whole IOMMU group) |
| `tendril-apply` | Binds a GPU to `vfio-pci` — **dry-run by default**, `--execute` to enact |

## Install

> ⚠️ **Pre-release.** No published image or one-click installer yet. Today you build the host image
> yourself and deploy it with `bootc`. See **[docs/INSTALL.md](docs/INSTALL.md)** for the full guide.

**Prerequisite:** enable **VT-d** (Intel) or **AMD-Vi / IOMMU** (AMD) in your motherboard's BIOS —
no software can turn this on for you.

Build the host image (from the repo root):

```bash
podman build -f image/Containerfile -t tendril:dev .
```

Deploy it to a machine with [`bootc`](https://containers.github.io/bootc/) — either a fresh install
to a disk, or switch an existing Fedora bootc system over:

```bash
# switch an existing Fedora bootc host to Tendril
sudo bootc switch localhost/tendril:dev
sudo reboot
```

After it reboots, confirm IOMMU came up and see your GPUs:

```bash
tendril-detect
```

If no IOMMU groups appear, VT-d / AMD-Vi is still disabled in your BIOS.

Just want to try the CLIs without installing the OS? On any Linux host with `/sys`:

```bash
cargo run --bin tendril-detect
```

## Roadmap

| Area | Capability | Status |
|---|---|---|
| Detection | GPU + IOMMU enumeration → capability matrix | ✅ Done |
| Provisioning · plan | Per-GPU VFIO bind config (whole IOMMU group) | ✅ Done |
| Provisioning · apply | Bind GPU to `vfio-pci` (dry-run + execute) | ✅ Done |
| Host image | Fedora bootc + virt stack + IOMMU kargs + binaries | 🔨 In progress |
| VM orchestration | libvirt domain templating; create/manage stations | 📋 Planned |
| Guest images | Automated Windows + SteamOS (Secure Boot/TPM, virtio) | 📋 Planned |
| Multi-seat | N GPUs → N stations; USB-controller + audio routing | 📋 Planned |
| Control plane | Web UI + "create gaming station" wizard | 📋 Planned |
| vGPU | >1 VM per GPU (official + `vgpu_unlock`) | 🔭 Future |
| Clustering | Manage stations across machines; GPU-aware scheduling | 🔭 Future |
| Streaming | Sunshine/Moonlight for headless / remote play | 🔭 Future |

Full architecture, decisions, and phase detail: **[docs/PLAN.md](docs/PLAN.md)**.

## Contributing

Trunk-based on `dev`, Conventional Commits, changelog per change — see
**[CONTRIBUTING.md](CONTRIBUTING.md)**.

## AI disclosure

Portions of this project — including design documents and code — were produced with the assistance
of AI tools. All output is reviewed by human maintainers before it lands. See [NOTICE](NOTICE).

## License

TBD — see the open questions in [docs/PLAN.md](docs/PLAN.md).

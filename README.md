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

**v0.5.0 — first bootable release.** There's a flashable installer ISO (built from the bootc image),
the full host-side provisioning pipeline, and libvirt orchestration — all validated on real hardware.
No graphical VM wizard yet; you create stations with the CLIs:

| Tool | What it does |
|---|---|
| `tendril-detect` | Enumerates GPUs + IOMMU groups, classifies each as passthrough / host-only |
| `tendril-plan` | Computes the exact `vfio-pci` bind set for a GPU (its whole IOMMU group) |
| `tendril-apply` | Binds a GPU to `vfio-pci` — **dry-run by default**, `--execute` to enact |
| `tendril-domain` | Renders a libvirt domain (Secure Boot + TPM, passthrough hostdevs) for a GPU |
| `tendril-vm` | Renders and (with `--define`) registers a station's VM with libvirt |
| `tendril-guest` | Creates the station disk + the OS-install domain (Windows/SteamOS) |
| `tendril-usb` | Lists USB controllers + devices for multi-seat assignment |

Plus `scripts/build-installer.sh` (build the ISO) and `scripts/fetch-windows-media.sh` (Win11 + virtio-win ISOs).

## Install

**Easiest:** grab the installer ISO from the
[latest release](https://git.onetick.ninja/flan/tendril/releases) (it's split into `.part` files to
fit the host's asset limit — `cat` them back per the release notes, verify against `SHA256SUMS`),
flash it to a USB stick, boot the target, and install. Or build the image yourself — see
**[docs/INSTALL.md](docs/INSTALL.md)** for both. Still pre-1.0; expect rough edges.

**Prerequisite:** enable **VT-d** (Intel) or **AMD-Vi / IOMMU** (AMD) in your motherboard's BIOS —
no software can turn this on for you.

Build the host image (from the repo root):

```bash
podman build -f image/Containerfile -t tendril:dev .
```

Deploy it to a machine with [`bootc`](https://containers.github.io/bootc/) — either a fresh install
to a disk, or switch an existing Fedora bootc system over:

```bash
# switch an existing Fedora bootc host to Tendril (published image)
sudo bootc switch git.onetick.ninja/flan/tendril:latest
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
| Host image + installer | Fedora bootc image + flashable installer ISO | ✅ Done |
| VM orchestration | libvirt domain templating + lifecycle (`virsh`) | ✅ Done |
| Guest disks & media | qcow2 disks, install ISOs, Win11 + virtio fetch | ✅ Done |
| Multi-seat USB | USB controller + per-device passthrough | ✅ Done |
| Guest OS install | Boot a Windows/SteamOS station end-to-end | 🔨 Next |
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

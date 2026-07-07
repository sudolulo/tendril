# Tendril

**Tendril** is an open-source, [Fedora bootc](https://docs.fedoraproject.org/en-US/bootc/)–based
operating system that turns a single multi-GPU machine (or a cluster of them) into multiple
plug-and-play gaming stations — each running **Windows** or **SteamOS** in a GPU-passthrough VM.

> One machine, many tendrils.

> **📍 This GitHub repository is a read-only mirror.** Tendril's canonical home is
> **[git.onetick.ninja/flan/tendril](https://git.onetick.ninja/flan/tendril)**. Please file **bug
> reports and issues** on the [Gitea issue tracker](https://git.onetick.ninja/flan/tendril/issues) —
> issues opened here on the mirror may go unseen.

![The Tendril web control plane — dashboard with stations and hardware](docs/images/dashboard.png)

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

**v0.11.0 — web control plane + console.** Tendril now has a **web UI** (Axum + HTMX) served on the
host: a dashboard, a create-station wizard, station management, a live in-browser console (noVNC),
GPU binding, media, and network — all over the same provisioning core as the CLI. There's also the
**`tendril` console**, a TrueNAS-style menu the OS launches on the primary display. Under both: a
flashable installer ISO, the full host-side provisioning pipeline, libvirt orchestration, and a
station that installs its guest OS **unattended** — Windows 11 (past the virtio "no drives" and
Microsoft-account walls) or a SteamOS-style Bazzite image (Anaconda kickstart, boots to Steam gaming
mode) — then boots from disk. This release adds **seats** (named USB device groups you assign to a
station in one pick), **live install progress**, and a **branded, simplified installer**.

Create a station from the browser — pick the OS, GPU, and unattended account, and Tendril builds the
disk, the answer-file/kickstart seed, and the VM, then installs it hands-off:

![The create-station wizard](docs/images/create-station.png)

You can drive everything from the web UI, the console menu, or the CLIs:

| Tool | What it does |
|---|---|
| `tendril-web` | Web control plane (Axum + HTMX) — dashboard, create/manage stations, live noVNC console, GPU/media/network |
| `tendril` | Interactive console — a menu over every function below (the OS launches it on the primary display) |
| `tendril-detect` | Enumerates GPUs + IOMMU groups, classifies each as passthrough / host-only |
| `tendril-plan` | Computes the exact `vfio-pci` bind set for a GPU (its whole IOMMU group) |
| `tendril-apply` | Binds a GPU to `vfio-pci` — **dry-run by default**, `--execute` to enact |
| `tendril-domain` | Renders a libvirt domain (Secure Boot + TPM, passthrough hostdevs) for a GPU |
| `tendril-vm` | Renders and (with `--define`) registers a station's VM with libvirt |
| `tendril-guest` | Creates the disk, builds a seed (Windows `autounattend.xml` or SteamOS/Bazzite kickstart), and installs the OS hands-off (`--unattend --start`), then boots from disk (`--finalize`) |
| `tendril-usb` | Lists USB controllers + devices for multi-seat assignment |

Plus `scripts/build-installer.sh` (build the ISO), `scripts/fetch-windows-media.sh` (Win11 +
virtio-win ISOs), and `scripts/fetch-steamos-media.sh` (Bazzite gaming-mode ISO).

## Install

**Easiest:** download the installer ISO — one file — from **https://dl.onetick.ninja/**, verify it
against `SHA256SUMS`, flash it to a USB stick, boot the target, and install. (It's also mirrored in
the [Gitea release](https://git.onetick.ninja/flan/tendril/releases), split into `.part` files to fit
the 2 GiB asset cap — reassemble per the release notes.) Or build the image yourself — see
**[docs/INSTALL.md](docs/INSTALL.md)**. Still pre-1.0; expect rough edges.

**Prerequisite:** enable **VT-d** (Intel) or **AMD-Vi / IOMMU** (AMD) in your motherboard's BIOS —
no software can turn this on for you.

Build the host image (from the repo root):

```bash
podman build -f image/Containerfile -t tendril:dev .
```

Or deploy the **published image** with [`bootc`](https://containers.github.io/bootc/) — it's pushed
to Tendril's own registry at `git.onetick.ninja/flan/tendril` (tags `latest` and `0.11.0`). Fresh
install to a disk, or switch an existing Fedora bootc system over:

```bash
# switch an existing Fedora bootc host to Tendril
sudo bootc switch git.onetick.ninja/flan/tendril:latest
sudo reboot
```

Once it's up, open the **web UI** at `http://<host-ip>/`, or use the `tendril` console on the
attached display. To confirm IOMMU came up and see your GPUs from a shell:

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
| Guest OS install | Unattended Windows (virtio + no-OOBE) **and** SteamOS/Bazzite (kickstart), boot from disk | ✅ Done |
| Console menu | Interactive `tendril` console over every function (OS boots into it) | ✅ Done |
| Web control plane | Axum + HTMX UI: dashboard, create/manage stations, live noVNC console, GPU/media/network | ✅ Done |
| Web polish | Auth, live install progress, richer host stats, per-seat USB (seats) | ✅ Done |
| Network config | Configure interfaces, DNS, and static IP from the web UI (DHCP ↔ static via NetworkManager) | ✅ Done |
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

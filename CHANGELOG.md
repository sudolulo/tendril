# Changelog

All notable changes to Tendril are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- **Friendly GPU names.** `tendril-detect` and the web UI now resolve the marketing model name from the
  system `pci.ids` database (`hwdata`) — e.g. `10de:1e84` shows as `TU104 [GeForce RTX 2070 SUPER]`
  instead of a bare vendor. `hwdata` is included in the image.
- **Automated release pipeline.** New `.gitea/workflows/release.yml` — on every push to `main` (image,
  crate, or installer-script change) the self-hosted runner builds the bootc image, pushes it to the
  Gitea registry (`:<version>` and `:latest`), builds the installer ISO, and publishes it to
  dl.onetick.ninja (with `tendril-latest-installer-x86_64.iso` always pointing at the newest). No
  stored credentials — checkout and registry login use the built-in Actions token.

## [0.8.0] - 2026-07-06

The web control plane — a full browser UI for the host, shipped in the image and served on `:80`.

### Added
- **Web control plane** (`crates/web`, `tendril-web`) — Axum + HTMX over the shared
  `orchestrator::provision` service, the same code path as the console and CLI. Pages:
  - a **dashboard** (host summary, live self-refreshing stations, hardware matrix);
  - a **create-station wizard** — choose OS, GPU, disk, and unattended account; it builds the disk,
    the answer-file/kickstart seed, and the VM, then installs hands-off;
  - **station management** (start / shut down / force off / delete) with HTMX swaps;
  - a **live in-browser console** — noVNC over a built-in WebSocket↔VNC proxy, to watch installs;
  - a **GPU/passthrough** page that binds a GPU (its whole IOMMU group) to `vfio-pci`;
  - **media** (list ISOs, background fetch) and **network** (interfaces/routes/DNS) pages.

  Server-rendered with Maud; htmx and the noVNC client are embedded in the binary, so the appliance
  serves everything offline.
- **Shipped in the image.** The OS runs `tendril-web` as a systemd service on `:80` — the address the
  console already advertises.

### Changed
- The `tendril` console banner now points at the live web UI (no longer "planned").

## [0.7.0] - 2026-07-06

An interactive console for the whole host — the stepping stone to the web UI. Boot the Tendril OS to
a monitor and you land in a menu that fronts every function.

### Added
- **`tendril` console.** A dependency-free, TrueNAS-style numbered menu covering everything: inspect
  hardware & capabilities, bind a GPU to `vfio-pci`, create a gaming station (guided: OS, GPU, disk,
  unattended account), manage stations (start/stop/force-off/delete), fetch install media, list USB
  devices, configure networking (`nmtui` + routes/DNS), open a shell, and reboot/shut down. The
  header shows the host name/address and where the web UI will live.
- **The OS boots into it.** The image auto-logs in the primary console (`tty1`) and launches the
  menu (appliance UX); other VTs and SSH still get a normal shell. Ships `NetworkManager-tui`,
  `genisoimage`, and the media-fetch scripts (`/usr/libexec/tendril`).
- **`orchestrator::provision` service layer.** A single `provision(StationRequest)` entry point that
  turns a resolved request into a running/defined station. `tendril-guest` (CLI), the console menu,
  and a future web UI all call it, so a station is created identically everywhere. Adds
  `Libvirt::list` for station enumeration.

### Changed
- `tendril-guest` now delegates to `orchestrator::provision` (no behavioural change; one code path).
- `InstallMedia.unattend_iso` was renamed to `seed_iso` in 0.6.0's API.

## [0.6.0] - 2026-07-06

Hands-off guest install for both station types: a station boots a stock Windows 11 ISO (or a
SteamOS-style Bazzite ISO) and installs itself unattended, then boots straight from disk.

### Added
- **Unattended Windows setup.** New `orchestrator::unattend` generates a Windows `autounattend.xml`
  that injects the virtio storage driver in WinPE (so the disk is visible), auto-partitions the disk,
  skips the OOBE / Microsoft-account screens, creates a local administrator, optionally auto-logs in,
  and installs the virtio guest tools (QEMU guest agent, balloon, network) on first logon.
- **Unattended SteamOS install.** New `orchestrator::kickstart` generates an Anaconda kickstart that
  wipes the disk, installs the OS image, creates a sudo user, enables SSH, and auto-logs into Steam
  gaming mode. Valve ships no generic-PC SteamOS installer (the Deck recovery image is image-based
  and AMD-only, so it can't drive an NVIDIA station), so the "SteamOS" station is
  [Bazzite](https://bazzite.gg) — an atomic, gaming-mode image with a scriptable Anaconda ISO. New
  `scripts/fetch-steamos-media.sh` grabs the Bazzite (Deck/NVIDIA) ISO.
- **Seed ISO builder.** New `orchestrator::guest::build_seed_iso` / `build_kickstart_seed` write the
  answer file onto a small ISO — `autounattend.xml` (any label, Windows scans all drives) or `ks.cfg`
  on an `OEMDRV`-labelled disc (Anaconda auto-loads it). The domain renderer attaches it as a third
  cdrom, and `InstallMedia` gains a `seed_iso` field.
- **End-to-end station install.** `tendril-guest` now composes the whole flow: create disk → build
  the seed ISO (`--unattend`) → render the install domain → `--define`/`--start` to register and
  launch it. Windows honors `--username`/`--password`/`--computer-name`/`--locale`/`--timezone`/
  `--edition`; SteamOS honors `--username`/`--password`/`--hostname`/`--image`/`--no-ssh`. `--steamos`
  selects the Bazzite path (and auto-taps Enter through the Windows CD boot prompt only where needed).
  `--finalize` re-renders the domain without install media so the station boots from disk; `--no-gpu`
  installs headless via VNC before a GPU is attached.

## [0.5.0] - 2026-07-05

First bootable release: a flashable installer ISO built from the bootc host image, plus VM lifecycle,
guest disks/install-media, USB multi-seat, and automated Windows-media fetching.

### Added
- **Bootable USB installer.** New `scripts/build-installer.sh` turns the bootc host image into a
  USB-flashable installer ISO (or a raw disk image) via `bootc-image-builder` — the easy "flash a
  stick and go" install path. Documented in `docs/INSTALL.md`.
- **Automated Windows media fetch.** New `scripts/fetch-windows-media.sh` grabs both ISOs a Windows
  station needs: the virtio-win driver ISO, and a genuine Windows 11 ISO assembled from Microsoft's
  Windows Update CDN via UUP dump (the consumer download page is anti-bot gated; this is the
  automatable path).
- **USB detection & passthrough (multi-seat).** New `capability-engine::usb` enumerates USB host
  controllers (with IOMMU group + passthrough viability, for assigning a whole controller to a seat)
  and connected USB devices; the `tendril-usb` binary lists both. Domains can now pass through
  individual USB devices by id (`<hostdev type='usb'>`) for a seat's keyboard/mouse.
- **VM lifecycle.** New `orchestrator::lifecycle::Libvirt` drives station VMs through `virsh`
  (`define`/`start`/`shutdown`/`destroy`/`undefine`/`state`). New `tendril-vm` binary renders a
  station's domain and, with `--define`, registers it with libvirt (validated, not started).
- **Guest disks & install media.** New `orchestrator::guest` creates a station's qcow2 disk via
  `qemu-img` and models `InstallMedia` (OS ISO + virtio-win). The domain renderer attaches install
  ISOs as cdroms with the right boot order, and the new `tendril-guest` binary creates the disk and
  renders the OS-install domain (`--steamos` for SteamOS, `--iso`/`--virtio-iso` for media).

### Fixed
- **Image build.** The host `Containerfile` now compiles the Rust binaries on the Fedora base (off
  Docker Hub, which rate-limits anonymous pulls) with the toolchain under `/usr/local` (bootc's
  `/root`→`/var/roothome` symlink tripped rustup's home/rc setup), and copies all seven `tendril-*`
  binaries into the image. `build-installer.sh` sets `--rootfs xfs` (bootc-image-builder needs it).
- Domain XML now emits `<smm state='on'/>`, which libvirt requires to match a Secure Boot firmware —
  without it, `virsh define` fails with "Unable to find 'efi' firmware". Verified against libvirt.

## [0.4.0] - 2026-07-04

First installable milestone: a bootc host image plus the full host-side pipeline
(detect → plan → apply) and libvirt domain templating.

### Added
- **Provisioning apply.** New `apply` module renders a `ProvisioningPlan` into ordered sysfs actions
  (unbind → `driver_override` → probe) that bind a GPU's IOMMU group to `vfio-pci`, with `DryRun` and
  `Execute` modes. New `tendril-apply` binary is dry-run by default (shows the exact writes and each
  device's current driver) and only mutates the host with `--execute`.
- **Bootc host image.** `image/Containerfile` builds a Fedora bootc host with the passthrough
  virtualization stack (`libvirt`, `qemu-kvm`, OVMF, `swtpm`), IOMMU kernel args + early `vfio-pci`,
  the VFIO modules, and the `tendril-*` binaries baked in — the first step toward an installable OS.
- **Install & roadmap docs.** `docs/INSTALL.md` (build the image + deploy with `bootc`) and an
  expanded `README.md` with a roadmap table and what-it's-for overview.
- **VM domain templating.** New `orchestrator::domain` renders a `StationSpec` into libvirt domain
  XML — OVMF Secure Boot + emulated TPM (Windows 11), `host-passthrough` CPU, virtio disk/net, and
  `<hostdev>` entries for the GPU's whole IOMMU group, plus the opt-in native-hardware fingerprint
  overlay. New `tendril-domain` binary renders a domain for a detected passthrough GPU.

### Changed
- Versioning now batches to user-meaningful milestones (first installable image, roadmap phases,
  `1.0.0` = production) instead of cutting a release per feature; changes accumulate under
  `[Unreleased]` between milestones.

## [0.3.0] - 2026-07-04

### Added
- **Provisioning plan (passthrough).** `PassthroughStrategy::plan` consumes a GPU's IOMMU group and
  emits the full set of PCI addresses to bind to `vfio-pci` — the GPU plus its audio/USB companion
  functions, since the IOMMU group is the smallest passable unit — with a caveat when no group is
  present. New `tendril-plan` binary prints the plan for each passthrough-capable GPU.

### Changed
- CI now runs on PRs into (and pushes to) `dev` as well as `main`, and installs the pinned Rust
  toolchain per run so it works on a sandboxed (Docker-mode) Gitea Actions runner.

## [0.2.0] - 2026-07-04

### Changed
- Development workflow now uses a long-lived `dev` integration branch (the repo default); `main` is
  release-only. Feature branches merge into `dev`; releases are PRs from `dev` into `main`, tagged on
  `main`. `scripts/setup-branch-protection.sh` now configures both branches.

### Added
- **Capability engine — live hardware detection.** `pci::enumerate` walks `/sys/bus/pci/devices` for
  display-class GPUs; `iommu` reads `/sys/kernel/iommu_groups` and assesses passthrough viability
  (isolated / shared-needs-ACS / no-IOMMU); `matrix::build` classifies each GPU
  (passthrough / host-only), exposed via a new `detect()` entry point and a `tendril-detect` binary.
  Fixture-based tests cover the isolated, shared, and no-IOMMU cases.

## [0.1.0] - 2026-07-04

Inaugural release: project foundation, development workflow, and the Rust workspace scaffold.

### Added
- **Rust Cargo workspace** with three crates reflecting the architecture:
  - `tendril-capability-engine` — GPU/IOMMU enumeration scaffolding (`pci`, `iommu`) and the
    `Capability` matrix types.
  - `tendril-provisioning` — `ProvisioningStrategy` trait and the VFIO `PassthroughStrategy`.
  - `tendril-orchestrator` — controller/agent `Role`, the `StationSpec` model, and the `tendrild`
    binary.
- **Pinned Rust toolchain** (1.84.0) via `rust-toolchain.toml`; committed `Cargo.lock`.
- **Development workflow** (`CONTRIBUTING.md`): trunk-based branching and Conventional Commits.
- **Version pinning policy** (`docs/VERSIONING.md`) with a central pin manifest (`versions.toml`).
- **Renovate** configuration for dependency pin bumps via PRs.
- **CI** via Gitea Actions (`fmt`, `clippy -D warnings`, `build`, `test`).
- **Branch-protection tooling** (`scripts/setup-branch-protection.sh`).
- **Design & build plan** (`docs/PLAN.md`), project `README.md`, and AI-disclosure `NOTICE`.

[Unreleased]: https://git.onetick.ninja/flan/tendril/compare/v0.5.0...HEAD
[0.5.0]: https://git.onetick.ninja/flan/tendril/compare/v0.4.0...v0.5.0
[0.4.0]: https://git.onetick.ninja/flan/tendril/compare/v0.3.0...v0.4.0
[0.3.0]: https://git.onetick.ninja/flan/tendril/compare/v0.2.0...v0.3.0
[0.2.0]: https://git.onetick.ninja/flan/tendril/compare/v0.1.0...v0.2.0
[0.1.0]: https://git.onetick.ninja/flan/tendril/src/tag/v0.1.0

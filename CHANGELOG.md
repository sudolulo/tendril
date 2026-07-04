# Changelog

All notable changes to Tendril are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
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

[Unreleased]: https://git.onetick.ninja/flan/tendril/compare/v0.4.0...HEAD
[0.4.0]: https://git.onetick.ninja/flan/tendril/compare/v0.3.0...v0.4.0
[0.3.0]: https://git.onetick.ninja/flan/tendril/compare/v0.2.0...v0.3.0
[0.2.0]: https://git.onetick.ninja/flan/tendril/compare/v0.1.0...v0.2.0
[0.1.0]: https://git.onetick.ninja/flan/tendril/src/tag/v0.1.0

# Changelog

All notable changes to Tendril are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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

[Unreleased]: https://git.onetick.ninja/flan/tendril/compare/v0.1.0...HEAD
[0.1.0]: https://git.onetick.ninja/flan/tendril/src/tag/v0.1.0

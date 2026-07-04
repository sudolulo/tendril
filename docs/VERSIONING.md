# Versioning & pinning policy

Tendril ships **immutable, reproducible host images**. That only works if every input is pinned, so a
given Tendril version always builds the same OS. This document is the contract for how we version and
what we pin.

## Release versioning — SemVer

Tendril releases follow [Semantic Versioning](https://semver.org): `MAJOR.MINOR.PATCH`.

- **PATCH** — fixes, driver/security bumps, no behavior change for users.
- **MINOR** — new capabilities (a new provisioning strategy, a new output backend, clustering, …).
- **MAJOR** — breaking changes to the on-disk layout, config schema, or public API.

Pre-1.0, minor releases may break; roadmap phases in [PLAN.md](PLAN.md) map to minor milestones.

Releases are annotated, signed git tags: `vMAJOR.MINOR.PATCH`. Every release corresponds to a section
in [CHANGELOG.md](../CHANGELOG.md) ([Keep a Changelog](https://keepachangelog.com/en/1.1.0/) format).
**Versions are cut only at user-meaningful milestones** — the first installable image, subsequent
roadmap-phase milestones, and `1.0.0` for production — **not per feature**. Between milestones,
changes accumulate under `[Unreleased]` (see the changelog rules in
[CONTRIBUTING.md](../CONTRIBUTING.md)).

## What is pinned, and where

Single source of truth: **[`../versions.toml`](../versions.toml)**. Nothing that affects a built
image floats.

| Input | Pinned via | Notes |
|---|---|---|
| Rust toolchain | `rust-toolchain.toml` + `versions.toml [toolchain]` | Exact channel, not `stable`. |
| Rust crates | `Cargo.lock` (committed) | Bins commit their lockfile. |
| Fedora bootc base | `versions.toml [host]` | **By digest** (`sha256:…`) before each release, not just a tag. |
| Hypervisor stack (libvirt/qemu/OVMF/swtpm) | inherited from pinned base image | Recorded in `versions.toml [hypervisor]` for visibility. |
| GPU drivers (NVIDIA/AMD, vGPU) | `versions.toml [drivers]`, per image-variant | Pinned exact; vGPU/`vgpu_unlock` are driver-version-locked. |

**Tags are for humans; digests are for reproducibility.** A release must pin the base image by digest.

## How pins change — Renovate

Pins are bumped **only through PRs**. [`renovate.json`](../renovate.json) opens grouped, labeled PRs
(`rangeStrategy: pin`, no auto-merge) so every bump goes through CI and review. Manual pin edits are
allowed but must be their own reviewed PR — never bundled into a feature change.

## Why this strictness

An immutable OS whose inputs float isn't reproducible — two builds of "the same version" could differ,
defeating the atomic-rollback guarantee that is Tendril's core reliability promise. Pinning is what
lets a user roll back to a *known* image, and lets us reproduce any reported build exactly.

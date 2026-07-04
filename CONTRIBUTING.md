# Contributing to Tendril

This is the **literal development workflow**. Follow it for every change.

## Golden rules

1. **Work targets `dev`; releases target `main`.** `dev` is the default integration branch; `main`
   is release-only and rejects direct pushes.
2. **Feature work happens on a branch and lands via a Pull Request** into `dev` that passes CI (and
   review, once the project has more than one maintainer).
3. **Pinned versions change only through a PR** (usually a Renovate PR). See [docs/VERSIONING.md](docs/VERSIONING.md).

## Branching model

Two long-lived branches:

- **`dev`** — the default integration branch; day-to-day work lands here. Protected (no force-push,
  no deletion); direct pushes are allowed for small changes, but feature work should still go through
  a PR.
- **`main`** — release/stable. Strictly protected: no direct pushes; changes arrive only via PR from
  `dev`. Every merge to `main` is a release and gets a `vX.Y.Z` tag.

Short-lived working branches off `dev`, named by type, deleted after merge:
  - `feat/<slug>` — a new feature
  - `fix/<slug>` — a bug fix
  - `chore/<slug>` — tooling, deps, docs, CI
  - `spike/<slug>` — throwaway experiment (e.g. `spike/single-gpu-passthrough`)

## Commit messages — Conventional Commits

```
<type>(<optional scope>): <short imperative summary>

<optional body: what & why>
```

Types: `feat`, `fix`, `chore`, `docs`, `refactor`, `perf`, `test`, `ci`, `build`.
Breaking changes: add `!` after the type/scope (`feat!:`) and a `BREAKING CHANGE:` footer.
This convention drives SemVer bumps and changelog generation.

Examples:
```
feat(orchestrator): render secure-boot + swtpm overlay for Windows 11 guests
fix(capability-engine): correct IOMMU group parse for multi-function GPUs
chore(deps): pin cargo dependencies (renovate)
```

## The step-by-step loop

```bash
# 1. Start from an up-to-date dev
git checkout dev && git pull

# 2. Branch
git checkout -b feat/gpu-capability-matrix

# 3. Work. Keep commits atomic and conventional.
git add -A
git commit -m "feat(capability-engine): emit per-GPU capability matrix"

# 4. Push the branch
git push -u origin feat/gpu-capability-matrix

# 5. Open a PR against dev (Gitea web UI or `tea pr create`).
#    CI runs automatically. Address review comments with follow-up commits.

# 6. Merge when green (+ approved): SQUASH merge, then delete the branch.
```

Rebase your branch on `dev` (don't merge `dev` into it) to keep history linear:
```bash
git fetch origin && git rebase origin/dev
```

## Local checks before you push

```bash
cargo fmt --all
cargo clippy --all-targets --all-features -- -D warnings
cargo test
```
CI runs the same checks; running them locally saves a round-trip.

## Releases — SemVer

- Versions follow [SemVer](https://semver.org): `MAJOR.MINOR.PATCH`.
- **Cutting a release:** open a PR from `dev` into `main`, merge it, then tag the merge on `main`:
  `git tag -s vX.Y.Z -m "vX.Y.Z" && git push origin vX.Y.Z`.
- Tagging triggers the release pipeline (build + publish the bootc image). Pre-1.0 the API/layout may
  change between minor versions; roadmap phases (see [docs/PLAN.md](docs/PLAN.md)) map roughly to
  minor milestones.

## Changelog & versioning

Every change updates [CHANGELOG.md](CHANGELOG.md), which follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/). Add a bullet under `## [Unreleased]` in the
right category (`Added`, `Changed`, `Deprecated`, `Removed`, `Fixed`, `Security`).

**Big changes cut a new version.** When a change is significant, roll `[Unreleased]` into a new
`## [X.Y.Z] - YYYY-MM-DD` section, bump `version` in the workspace `Cargo.toml`, and tag `vX.Y.Z`
(see *Releases* above). What counts as "big":

- a new crate, provisioning strategy, or output backend;
- a roadmap-phase milestone (see [docs/PLAN.md](docs/PLAN.md));
- any user-facing behavior, on-disk layout, or config-schema change.

Small fixes and chores just accumulate under `[Unreleased]` until the next version is cut.

## Version pinning

Reproducible images demand pinned inputs. The rules and the pin manifest (`versions.toml`) are
documented in [docs/VERSIONING.md](docs/VERSIONING.md). Don't hand-edit pins outside a PR.

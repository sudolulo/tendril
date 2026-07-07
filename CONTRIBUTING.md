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
- **`main` is release-only.** Advancing `main` **is** a release — don't do it between milestones
  (see *Changelog & versioning* below). Keep everyday work on `dev`.
- **Cutting a release** (one push, no PR dance):
  1. On `dev`: roll `[Unreleased]` into a `## [X.Y.Z] - YYYY-MM-DD` changelog section and bump
     `version` in the workspace `Cargo.toml`. Commit and `git push origin dev`.
  2. `git push origin dev:main` — a direct fast-forward push (the repo owner is whitelisted for
     `main`; force-push stays blocked). That push triggers `.gitea/workflows/release.yml`, which
     **builds + pushes** the bootc image (registry `:X.Y.Z` and `:latest`), **publishes the installer
     ISO** to dl.onetick.ninja, and **creates the stable Gitea release + `vX.Y.Z` tag** — automatically.
  The workflow is **idempotent by version**: pushing `main` when its `Cargo.toml` version is already
  released is a no-op, so only a version bump actually cuts a release.
- **Why a direct push, not a PR:** a *fast-forward PR merge* does not emit a `push` event in this Gitea,
  so it never triggers `release.yml` (it needed a manual `workflow_dispatch`). A direct push does. A PR
  still works if you prefer review — just run the release via `workflow_dispatch` afterward.
- **CI gotcha:** the release job authenticates to the container registry with the **`REGISTRY_TOKEN`**
  repo secret (a real PAT). Gitea's built-in Actions token is rejected by the registry even with
  `packages: write`, so that secret must exist (and be current) for releases to publish. If the PAT is
  rotated, update this secret too.
- Pre-1.0 the API/layout may change between minor versions; roadmap phases (see
  [docs/PLAN.md](docs/PLAN.md)) map roughly to minor milestones.

## Changelog & versioning

Every change updates [CHANGELOG.md](CHANGELOG.md), which follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/). Add a bullet under `## [Unreleased]` in the
right category (`Added`, `Changed`, `Deprecated`, `Removed`, `Fixed`, `Security`).

**Cut a version only at user-meaningful milestones — not per feature.** Features accumulate on `dev`
under `## [Unreleased]`; the changelog and git history track granular progress. A tagged release is
reserved for something a user can actually get value from:

- the **first installable** milestone — a bootc image that boots and detects hardware; this is the
  next version we cut;
- subsequent roadmap-phase milestones (multi-seat, vGPU, clustering — see [docs/PLAN.md](docs/PLAN.md));
- **`1.0.0`** = production / stable.

Everything below the next milestone just accumulates under `[Unreleased]`. Pre-1.0 tags are
development previews. To cut a release, follow *Releases — SemVer* above (roll the changelog, bump the
workspace `Cargo.toml` version, `dev → main`).

> **Cadence discipline.** It's tempting during rapid iteration to cut a release per feature batch —
> resist it. Batch changes on `dev` and cut at a milestone. (We drifted to several releases in a day
> once; the version number should track maturity, not commits.)

## Public demo

`tendril-web` run with `TENDRIL_DEMO=1` is a **read-only showcase**: no login, a DEMO badge, every
mutating action disabled behind a banner, and **self-contained canned data** (from
`crates/web/src/demo.rs`) — it touches no real host state, so a demo can run beside a real instance on
the same box. `scripts/demo-setup.sh` installs it as `tendril-demo.service` (binds all interfaces for
LAN by default; override with `ADDR=127.0.0.1:PORT` for proxy-only) and opens the firewall port. Keep
demo content canned — never make demo mode depend on real libvirt/media/host state.

## Version pinning

Reproducible images demand pinned inputs. The rules and the pin manifest (`versions.toml`) are
documented in [docs/VERSIONING.md](docs/VERSIONING.md). Don't hand-edit pins outside a PR.

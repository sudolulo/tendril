# Release channels

Tendril publishes one codebase through three channels. A bootc host tracks a channel and rolls
forward with `bootc upgrade`; greenboot auto-rollback protects every channel equally.

| Channel | Image tag | Published by | Cadence | Who it's for |
|---|---|---|---|---|
| **dev** | `:dev` | every push to `dev` ([deploy-dev.yml](../.gitea/workflows/deploy-dev.yml)) | rolling | contributors, testers |
| **release** | `:latest` + `:X.Y.Z` | every release ([release.yml](../.gitea/workflows/release.yml)) | per milestone | enthusiasts who want current features |
| **stable** | `:stable` | manual promotion ([promote-stable.yml](../.gitea/workflows/promote-stable.yml)) | when a release has proven itself | anyone running Tendril for others — venues, fleets, the risk-averse |

Subscribe:

```bash
sudo bootc switch git.onetick.ninja/flan/tendril:stable   # or :latest, :dev
```

Installer ISOs: `tendril-latest-installer-x86_64.iso`, `tendril-stable-installer-x86_64.iso`, and
`tendril-dev-installer-x86_64.iso` at https://dl.onetick.ninja/ (verify against `SHA256SUMS`).

Released images are cosign-signed (from the first release after signing was enabled). Verify with
the repository's [`cosign.pub`](../cosign.pub):

```bash
cosign verify --key cosign.pub git.onetick.ninja/flan/tendril:stable
```

## What "stable" means

**Promotion moves bits, never rebuilds.** The `:stable` tag is repointed at the *exact image digest*
already released as `:X.Y.Z` — what was validated is what ships. The promotion workflow refuses to
build anything.

A release is promoted only when **all** of the following hold:

1. **Bake time** — released for **≥ 14 days** with no regression reports affecting core flows
   (install, station create, passthrough, update/rollback).
2. **Hardware validation** — on the reference matrix (at minimum: one Intel + one AMD IOMMU
   platform, one NVIDIA + one AMD GPU): fresh ISO install, unattended Windows 11 **and** Bazzite
   station install, `bootc upgrade` *onto* the candidate from the previous stable, and a forced
   greenboot rollback.
3. **No open release-blockers** — no issue labeled `release-blocker` against the candidate.
4. **Security review** — the changelog's `Security` entries for the candidate are shipped, and no
   unfixed vulnerability reported under [SECURITY.md](../SECURITY.md) affects it.

Security fixes invert the flow: a patch release containing a security fix for stable is promoted
**out of band**, as soon as the hardware pass completes — bake time doesn't apply.

Run the promotion from Gitea → Actions → `promote-stable` → *Run workflow*, entering the version
(e.g. `0.18.0`). The job logs the promoted digest; record it in the release notes if you edit them.

## Experimental features on stable

Features flagged 🧪 in the [README](../README.md) (vGPU, federation, streaming) are present on every
channel — there is one edition of Tendril — but the stable promise (the checklist above) covers the
core flows. Experimental features graduate by being added to the hardware-validation matrix.

## Maintainer notes

- **Signing key custody.** The cosign private key and its password live only with the maintainer
  (dev-box `~/.tendril-cosign/` + password vault) and as the repo Actions secrets
  `COSIGN_PRIVATE_KEY` / `COSIGN_PASSWORD` — never in the repo. The public half is committed as
  [`cosign.pub`](../cosign.pub). To rotate: `cosign generate-key-pair`, replace both secrets,
  commit the new `cosign.pub`, and note the rotation in the changelog. If the secrets are absent
  the release job skips signing and says so — signing is optional by design, never release-blocking.
- **Promotion cadence.** Each release's 14-day bake clock starts at its release date; a newer
  release restarts the clock on itself (promote the newest candidate that has baked, not several).
  Set a reminder for release date + 14 days when cutting a release.

## Looking ahead

The stable channel is also the foundation for a future paid **assurance subscription** (stable
channel + support SLA, served from an authenticated registry). If that happens, the public channels
above — including everything in the codebase — remain free and complete; see
[LICENSING.md](../LICENSING.md).

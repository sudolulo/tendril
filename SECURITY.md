# Security Policy

## Reporting a vulnerability

**Do not open a public issue for security problems.** Email **holden@arch.fyi** with a description,
reproduction steps, and the Tendril version / channel (`:dev`, `:latest`, `:stable`).

You'll get an acknowledgment within **72 hours**. Tendril is maintained by a small team, so fix
timelines are honest rather than contractual: critical issues (remote compromise of the host or
control plane, VM escape paths Tendril introduces, fleet mTLS/auth bypass) take priority over
everything else; lower-severity issues are batched into the next release. You'll be kept informed,
and credited in the changelog unless you prefer otherwise.

Please allow a fix to ship before public disclosure; 90 days is a reasonable outer bound.

## Supported versions

| Channel | Security posture |
|---|---|
| `:stable` | Promoted releases; security fixes trigger an out-of-band promotion |
| `:latest` (current release) | Fixed in the next release; critical issues cut an immediate patch release |
| `:dev` (rolling) | Fixed in-place; roll forward with `bootc upgrade` |
| Older tags | Not patched — bootc hosts should track a channel, not a pinned version |

## Scope

In scope: the Tendril host image, orchestrator, provisioning, web control plane and console,
fleet/federation (join codes, mTLS, peer control), the installer, and update/rollback machinery.

Out of scope: guest operating systems and games, the NVIDIA vGPU driver, and vulnerabilities in
upstream components (Fedora, libvirt, QEMU) without a Tendril-specific exposure — though we'll ship
upstream fixes promptly via image rebuilds, and reports that Tendril's *configuration* of an
upstream component is unsafe are very much in scope.

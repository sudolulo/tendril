# Licensing

Tendril is **dual-licensed**: open source under the AGPL for everyone, with commercial licenses
available for organizations that need different terms. There is **one edition** of Tendril — every
feature is in the open-source version; nothing is held back.

## Open-source license — AGPL-3.0-only

Tendril is licensed under the [GNU Affero General Public License v3.0](LICENSE)
(SPDX: `AGPL-3.0-only`).

In practice:

- **Use it freely, including commercially.** Running Tendril — at home, in a LAN center, in a
  business — costs nothing and carries no obligation beyond keeping the license and notices intact.
  Simply *using* Tendril to operate gaming stations you charge for is fine.
- **Modify it freely.** If you distribute a modified version, or let others interact with a modified
  version **over a network**, the AGPL requires you to offer them your modified source under the
  same license. This is the AGPL's one addition over the regular GPL, and it is deliberate: it keeps
  improvements to Tendril open.
- **Unmodified hosting is trivially compliant** — the corresponding source is this repository.

## Commercial license

If your organization wants to embed Tendril in a proprietary product, build a hosted service on a
modified Tendril without publishing the modifications, or otherwise can't work with the AGPL,
commercial licenses are available. Contact **holden@arch.fyi**.

Dual licensing is possible because all Tendril copyright is held by a single owner; contributions
are accepted under a [Contributor License Agreement](CLA.md) that preserves this
(see [CONTRIBUTING.md](CONTRIBUTING.md)).

## What is *not* covered by Tendril's license

- **Guest operating systems** (Windows, Bazzite/SteamOS) and the games you run — licensed by you
  from their respective vendors.
- **NVIDIA vGPU host driver** — licensed and non-redistributable; Tendril never ships it. The
  optional vGPU image variant is built only from a driver package *you* are licensed for, into a
  private repository for your own use (see `image/vgpu/README.md`).
- **Third-party dependencies** — under their own licenses, as recorded in `Cargo.lock` /
  `versions.toml`.

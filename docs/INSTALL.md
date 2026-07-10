# Installing Tendril

> ⚠️ **Pre-1.0.** For testers and contributors — expect rough edges. The fastest path is:
> flash the release ISO → boot it → open the web UI → click **+ New station**.

## 0. Install from the release ISO (easiest)

Download the single-file installer, verify it, and flash it. `tendril-latest-installer-x86_64.iso`
always points at the newest release (rebuilt on every release; `-stable-` and `-dev-` variants track
the other [channels](CHANNELS.md)):

```bash
curl -LO https://dl.onetick.ninja/tendril-latest-installer-x86_64.iso
curl -LO https://dl.onetick.ninja/SHA256SUMS
sha256sum -c SHA256SUMS --ignore-missing
sudo dd if=tendril-latest-installer-x86_64.iso of=/dev/sdX bs=4M status=progress
```

The download supports resume (`curl -C -`). A specific version is also available as
`tendril-<version>-installer-x86_64.iso`.

Boot the target from the USB stick and follow the installer (you pick the disk, login, and
language). Then jump to **step 4 (First station)**. Prefer to build the image yourself? Continue
below.

> Already have one Tendril box and a rack of empty machines? Its **Fleet → Provision a room (PXE)**
> panel netboots them straight into the unattended installer — no USB sticks. See
> [FEDERATION.md](FEDERATION.md).

## 1. Prerequisites

- **A machine with an IOMMU** and **VT-d (Intel) / AMD-Vi (AMD) enabled in BIOS/UEFI.** This is a
  firmware toggle — no software (Tendril included) can enable it for you. Enable "VT-d", "AMD-Vi",
  "IOMMU", or "SVM + IOMMU" in your board's setup.
- **At least two GPUs**, *or* an iGPU + a discrete GPU: one drives the host, the rest are passed to
  VMs. (A single GPU can't run both the host and a passthrough VM.)
- To build the image yourself: a host with **`podman`** (or Docker).

## 2. Build the host image (optional)

From the repo root:

```bash
podman build -f image/Containerfile -t tendril:dev .
```

This produces a Fedora bootc image containing the virtualization stack (`libvirt`, `qemu-kvm`,
`edk2-ovmf` for Secure Boot, `swtpm` for TPM 2.0), IOMMU kernel arguments + early `vfio-pci`, the
web control plane, the console, and all the CLIs.

Push it to a registry your target can reach if you're not building on the target itself. The
official image is published to Tendril's own Gitea registry
(`git.onetick.ninja/flan/tendril` — `:dev`, `:latest`, `:stable`).

### 2b. Build a bootable USB installer

Turn the image into a USB-flashable installer with `bootc-image-builder`:

```bash
scripts/build-installer.sh --type iso     # installer ISO -> flash to USB, boot the target, install
scripts/build-installer.sh --type raw     # raw disk image -> dd straight onto the target's disk
scripts/build-installer.sh --unattended   # opt-in TOUCHLESS install (CI/test VMs, fleet provisioning)
```

The default ISO is **guided** (you pick the disk, admin login, and language). `--unattended` builds a
**touchless** variant that installs hands-off — safe single-disk partitioning (targets one real disk;
never a blind wipe) and a seeded **must-change** default web admin password (you're forced to set a
new one on first sign-in). Use it for repeatable test VMs and fleet provisioning, not as the shipping
media.

> Building the installer needs a host with loopback devices and a privileged container (bare metal or
> a full VM) — it will not run inside an unprivileged LXC.

## 3. Deploy with bootc (alternative to the ISO)

**Switch an existing Fedora bootc host:**

```bash
sudo bootc switch git.onetick.ninja/flan/tendril:latest   # or :stable / :dev, or a ref you pushed
sudo reboot
```

**Fresh install to a disk** (from the image, e.g. via a live environment) — see the
[`bootc install`](https://bootc-dev.github.io/bootc/bootc-install.html) docs:

```bash
sudo bootc install to-disk /dev/sdX
```

Updates and rollback are then handled by bootc: one click on the **System** page (or
`bootc upgrade`), and a boot that fails the health check rolls back automatically (greenboot).

## 4. First station (web UI)

After the install reboots, the primary display drops into the **`tendril` console**, which shows the
box's address. From another device, open **`https://<host-ip>`** — the certificate is self-signed
(accept the warning; the **System** page can install a real cert or run behind your own
TLS-terminating proxy with `TENDRIL_TLS=off`). Set the admin password when prompted.

Check **Hardware**: your GPUs should be listed with IOMMU groups, the boot GPU reserved for the
host, the rest passthrough-ready. (An IOMMU warning banner means VT-d/AMD-Vi is still off — back to
step 1.)

Then **Stations → + New station**: pick the OS (Windows 11 or SteamOS-style Bazzite), the GPU, and
an account, and create. Tendril fetches and checksum-verifies the install media itself (several GB,
first time only), builds the disk and the answer-file seed, and installs the guest **completely
unattended** — watch it live in the in-browser console. When it reaches the desktop, the monitor on
that GPU is a gaming station.

Day-2 things worth knowing about on day 1: **snapshots** before risky guest updates, **Save as
image** to clone an installed station instantly, **persistent data volumes** that survive
reinstalls, and **Remote play** (Sunshine/Moonlight) for playing from another device — all on the
station's page.

## 5. First station (CLI alternative)

Everything the wizard does is scriptable. Fetch media once, then let `tendril-guest` build the disk
and install Windows itself:

```bash
scripts/fetch-windows-media.sh --dest /var/lib/tendril/isos     # win11.iso + virtio-win.iso

sudo tendril-guest \
  --name station1 --create-disk --size-gib 128 \
  --iso /var/lib/tendril/isos/win11.iso \
  --virtio-iso /var/lib/tendril/isos/virtio-win.iso \
  --unattend --username player --password changeme \
  --start
```

`--unattend` builds a seed ISO carrying `autounattend.xml`, which injects the virtio storage driver,
auto-partitions, skips the OOBE / Microsoft-account screens, creates the local account, and installs
the virtio guest tools on first logon. By default the station's GPU (its whole IOMMU group) is
passed through; add `--no-gpu` to install headless and attach a GPU later. Once Windows reaches the
desktop:

```bash
sudo tendril-guest --name station1 --finalize --start   # drop install media, boot from disk
```

For a SteamOS-style station, swap in `--steamos` and the Bazzite ISO
(`scripts/fetch-steamos-media.sh`); the kickstart seed wipes the disk, installs the image, creates
the user, and auto-logs into Steam gaming mode. `--native-hardware` applies the opt-in
fingerprint-reduction overlay (read the ToS caveats first). Inspection tools: `tendril-detect`
(GPUs + IOMMU), `tendril-plan` (the vfio bind set), `tendril-apply` (dry-run by default),
`tendril-vm` (render/define a domain) — see [CLI.md](CLI.md).

## Next steps

- **More boxes?** [FEDERATION.md](FEDERATION.md) — join codes, fleet placement, PXE room provisioning.
- **More stations than GPUs?** [VGPU.md](VGPU.md) — split a GPU with mdev/SR-IOV.
- **Lots of games?** [STEAM-GAMES.md](STEAM-GAMES.md) — golden images and the shared Steam library.
- **Validating on new hardware?** [HARDWARE-TESTING.md](HARDWARE-TESTING.md) is the checklist.

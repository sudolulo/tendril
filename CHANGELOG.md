# Changelog

All notable changes to Tendril are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- **Save & clone station images (clustering groundwork).** Capture an installed station's disk as a
  reusable **golden image** ("Save as image" on the station page — flattened + compressed into
  `/var/lib/tendril/images`, listed under **Media → Station images**), then **create a new station from
  it** via the wizard's **Base image** picker. Clones are qcow2 **copy-on-write overlays** — instant
  and deduplicated (the base is shared, not copied) — and boot straight to the installed OS with no
  install step. This is the basis for shipping a built station to other machines (clustering).
- **Public demo mode.** Running `tendril-web` with `TENDRIL_DEMO=1` serves a read-only showcase: no
  login, a DEMO badge in the header, and every mutating action disabled (returns a friendly banner).
  It shows **self-contained canned data** (stations, media, seats) that touches no real host state, so
  a demo can run **alongside a real Tendril instance on the same box without colliding**.
  `scripts/demo-setup.sh` installs it as a LAN-reachable systemd service. Running stations show a
  representative **console preview** (an original Steam gaming-mode / Windows desktop mock). Live at demo.onetick.ninja.

## [0.14.0] - 2026-07-07

Marginal-buffer station sizing, network/media polish, and a release pipeline that finally auto-publishes.

### Added
- **Media provenance tooltips.** Each known install ISO (Windows/virtio/Bazzite) carries an "ⓘ source"
  tooltip explaining where it came from and how it's verified, so the media never looks like it
  appeared from nowhere. Removed the now-redundant footer note.
- **Previewable OS updates on non-bootc hosts.** The System page's OS-image panel shows a labelled
  **demo** (sample `bootc status`, Check/Update buttons) instead of being hidden, so the control is
  visible on test builds.

### Changed
- **Station resource defaults are now proportional to GPU count with only a marginal host buffer.**
  RAM/CPU/disk split evenly across one station per GPU, keeping just what the host OS needs to run
  (~2 GiB RAM, 1 thread, ~20 GiB disk) instead of a large proportional reserve. On an 8-thread / 8 GB
  box a single station now defaults to ~7 vCPU and ~5–6 GB rather than 6 vCPU / 4 GB.
- **Network:** on a DHCP connection the form now shows the box's **real current address/gateway/DNS**
  as placeholders (instead of generic examples), and the System page puts **OS image above Host**.
- System page: automatic-updates toggle sits **under OS image** (with the update controls); logs now
  **drop SELinux `audit`/AVC spam** from both the All and Stations-only views.
- **Automated releases work.** `release.yml` authenticates to the container registry with a stored PAT
  (`REGISTRY_TOKEN`) — the built-in Actions token is rejected there even with `packages: write`.
- **Windows media fetch only downloads what's missing.** If you already have a valid `win11.iso` or
  `virtio-win.iso`, the fetcher skips it and grabs only the other (pass `--force` to re-fetch both).
  Downloads are written atomically (temp + move), so a "present" file is always complete — an
  interrupted download won't be mistaken for a good copy. When win11 is already present, the fetch no
  longer requires the whole UUP toolchain just to grab virtio.

## [0.13.1] - 2026-07-07

Safety fix for the network trial, plus System/wizard polish.

### Changed
- **Auto-update toggle moved next to the power buttons** on the System page (under "Automatic OS
  updates"). On a non-bootc host it now renders as a clickable **demo** (labelled as such) instead of
  being hidden, so the control is visible everywhere; on a real bootc host it drives the systemd timer.
- **Unattended-install toggle moved into Advanced** in the create-station wizard (grouped with
  native-overlay and start-now), keeping the default view minimal. Unattended and **Start-now are
  both on by default**, so a station installs hands-off as soon as you create it.
- Release CI: the workflow now grants the job `packages: write` so the registry login succeeds.

### Fixed
- **Network test-and-revert race.** The 60-second trial now reserves its backup *before* touching the
  connection profile, so two overlapping applies (a double-submit or a second tab) can no longer
  snapshot the half-applied config as the "original" — the auto-revert always restores the true prior
  settings, preserving the safety guarantee for the link you're reconfiguring.

## [0.13.0] - 2026-07-07

Auto-fetched media, hardware-usage visibility, better logs, and much saner station defaults.

### Added
- **Auto-fetch install media on station create.** If you create a station whose default install ISO
  isn't downloaded yet (Windows 11 + virtio-win, or Bazzite), Tendril fetches it in the background and
  then provisions the station automatically once it's ready. Media is **verified against the
  publisher's checksum as it downloads**, and a station is **never created from media that fails
  verification** (a checksum mismatch). Media with no upstream checksum (e.g. the locally-assembled
  Windows ISO) is fine to use.

- **Hardware usage & warnings.** The Hardware page shows which station each GPU and USB device is
  **used by** (or "free"), and the station list flags any station with **no GPU** passed through with a
  red ⚠. The System **logs** gain a **Stations-only filter** and a **Download** button.

### Changed
- Network page: dropped the "may drop the link, use the console" warning (the 60-second test-and-revert
  makes it safe) and now lists only physical NICs — podman/docker/libvirt bridges are hidden.
- **Smarter station defaults.** RAM/vCPU/disk now reserve real host headroom before splitting per GPU
  (previously they handed the host's *entire* RAM/CPU/disk to the stations). The **Start now** and
  **native-hardware** options moved into **Advanced**, and Start is now **off by default** so a new
  station isn't left running an empty VM.
- Dashboard: the live Host panel moved below Stations/Hardware to keep the top focused.

## [0.12.0] - 2026-07-07

Configure the network from the browser — with a TrueNAS-style safety net.

### Added
- **Configurable network from the web UI.** The Network page is now editable: each active
  NetworkManager connection can be switched between **DHCP and a static** address/gateway/DNS and
  applied live (`nmcli`). Changes are applied **on a 60-second trial (TrueNAS-style)** — a
  server-side timer automatically reverts to the previous config unless you click **Keep these
  settings**, so a bad change that cuts off your own access heals itself. Non-essential controls
  (gateway, DNS, and the read-only interface/route/DNS view) are tucked behind **Advanced** toggles.

## [0.11.0] - 2026-07-07

Per-seat USB, live install progress, a branded installer, and a release pipeline that always builds.

### Added
- **Seats — named USB device groups.** Group a player's keyboard/mouse/controller under a friendly
  name (managed under **Hardware → Seats**), then assign the whole seat to a station in one pick from
  the create-station wizard. Persisted at `/etc/tendril/seats.conf`; a station still supports picking
  individual USB devices when you don't want a saved group.
- **Live install progress.** A station installing its guest OS shows bytes written to the guest disk,
  polling every 5 seconds — so you can watch an unattended Windows/Bazzite install make headway.
- **Branded installer.** The OS now identifies as **Tendril** (os-release), and the installer ISO
  ships a simplified, guided kickstart (`image/installer/config.toml`): keyboard/timezone/networking
  are pre-answered while **language, destination disk, and the admin password stay interactive**, and
  the login banner points at the web UI. `build-installer.sh` gains `--rootfs xfs|btrfs` (the
  installed system's root filesystem) and `--config` to select the installer config.

### Changed
- **Reproducible, reliable ISO builds.** `build-installer.sh` now pins `bootc-image-builder` to a
  verified-good digest (override with `$BIB`), so a surprise `:latest` push can't break the release
  build, and the release workflow **creates the matching Gitea release** automatically (linking the
  container image and the verified ISO). The prior ISO-build failures were disk-exhaustion during the
  compose, not the builder; the `grub2-probe: … /dev/mapper/fedora-root` line in the log is a
  harmless RPM-scriptlet warning and the ISO completes regardless.

## [0.10.0] - 2026-07-07

The web control plane hardens up: login-gated, smarter defaults, and live host stats.

### Added
- **Live host stats** on the dashboard — memory and disk usage bars, load, and uptime, auto-refreshing.
- **Smarter, simpler create-station wizard.** Non-essential fields are tucked behind an **Advanced**
  toggle; RAM, vCPUs, and disk size now **default to the host total ÷ number of GPUs** (one station
  per GPU); and the install ISO defaults to the right image for the chosen OS when left blank.
- **Web UI authentication.** A single admin password (Argon2-hashed at `/etc/tendril/webauth`),
  server-side sessions via an HttpOnly cookie, and a first-run `/setup` page to create it — every
  route is gated. An optional reverse-proxy trust mode (`TENDRIL_TRUST_PROXY_HEADER`, e.g.
  `Remote-User`) lets an SSO front-end (Authelia/NPM) authenticate instead. Set or reset the password
  from the console (`tendril` → *Set web admin password*) or `tendril-web --set-password`.

## [0.9.0] - 2026-07-06

Web UI depth pass: USB passthrough, station delete, an OS-updates + system page, install-media
checksum verification, friendly GPU names, a live in-browser console, and an automated release
pipeline that builds and publishes the image + ISO on every push to `main`.

### Added
- **Install-media checksum verification.** `scripts/verify-media.sh` verifies a fetched ISO against
  its upstream-published SHA-256 (Bazzite publishes one; Windows is assembled by UUP dump from
  hash-verified components, so its ISO gets a recorded local hash). The fetch scripts run it
  automatically, and the **Media page shows each ISO's verification state** — verified / mismatch /
  local — with a **Verify** button whose result appears **live (HTMX polling, no refresh)**.
- **System page controls.** Reboot and shut down the host, a **live journal log tail** (last 200
  lines, auto-refreshing), and host info (uptime, load, memory, disk, kernel) — alongside the OS
  update panel.

### Fixed
- **USB passthrough list excludes hubs** (device class 0x09). Root hubs aren't passthrough targets and
  couldn't be attached by vendor:product anyway (`Multiple USB devices for 1d6b:3` errored).
- **Web console rendered black** even while the VM had output — the noVNC canvas container collapsed
  to zero height. It now fills the console panel and shows the live screen, with a status overlay
  ("Connecting…" / connection errors) so a genuinely blank VM is distinguishable from a problem.
- **Delete is now available on running stations** too (it forces the VM off first), from both the
  list and the station page.
- Station **VNC now listens on all interfaces**, so a native viewer on the LAN can reach it (the
  in-browser console worked regardless). Added a **"Send Enter"** button and a longer automatic
  key-tap window to clear the Windows "press any key to boot from CD" prompt on slow-booting hosts.

### Added
- **USB passthrough in the web UI.** Assign a seat's keyboard/mouse/controller to a station by
  friendly name — both in the create wizard and afterward on the station page (hot-plugged live via
  `virsh attach-device`/`detach-device`, persisted to the config). `StationRequest`/`provision` gained
  a `usb_devices` field; `Libvirt` gained `attach_usb`/`detach_usb`/`usb_devices`.
- **Delete stations in the web UI** — from the stations list and the station page (removes the VM
  definition; the disk image is kept).
- **OS updates page (bootc).** A System page shows the current image and can check for, and stage,
  OS updates (`bootc upgrade`); a toggle turns automatic updates on/off
  (`bootc-fetch-apply-updates.timer`). A top-bar "Update ready" badge appears when a new image is
  staged and pending reboot.
- **Console shows the VNC endpoint**, and the dashboard's host-capacity stat is now labelled
  (e.g. "8 threads · 8 GB RAM" instead of "8t").
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

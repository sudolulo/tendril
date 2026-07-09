# Changelog

All notable changes to Tendril are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

A correctness-audit round following 0.22.0's security rounds: a full-codebase review for logic bugs
and cleanups (not security), then a second pass over the fixes themselves. No feature or API changes.

### Fixed
- **Kickstart passwords with backslashes.** pykickstart tokenizes with shlex, so a trailing `\`
  aborted the whole unattended SteamOS install and `\\` silently changed the account password;
  backslashes are now stripped alongside quotes (the Windows path was unaffected).
- Media fetch/verify background jobs are reaped — each click no longer leaves a zombie process in
  `tendril-web` until the next service restart.
- A failed fetch of the shared sums script no longer aborts a half-done stable promote (it warns
  and continues).
- **Demo privacy.** The public demo no longer serves the real host's journal, nor the (possibly
  co-located real instance's) audit log — the logs/audit views and downloads return placeholders.
- **Greenboot vs reverse-proxy.** The required post-boot health check only spoke HTTPS; with the
  documented `TENDRIL_TLS=off` proxy setup every boot failed the check, silently rolling back all
  OS updates. It now accepts plain HTTP on :443 too.
- **Empty wizard account fields.** A cleared Username/Password no longer renders an empty
  `<Name>`/AutoLogon into `autounattend.xml` (Windows Setup rejects it hours into the install);
  the local path applies the same `player`/`tendril` defaults as the federation path.
- **Image capture/verify markers** (`.qcow2.partial` / `.qcow2.verifying`) orphaned by a service
  restart no longer wedge the images panel or block re-capturing a name — markers are mtime-aged
  and reaped, which stays safe for a peer's live capture on a shared store.
- **PXE hardening of the previous round's own fixes:** backticks in a kickstart-heredoc comment
  were command-substituted (executing a stray `oci` binary as root and corrupting the generated
  kickstart); the full-extraction fallback now also covers a missing EFI tree; a failed clone
  provision undefines the domain it defined (no ghost station pointing at a deleted overlay); and
  the installer-ISO fetch uses an in-process guard plus pid-unique temp so a stalled download and a
  retry can't corrupt each other.
- `versions.toml` rust pin matches `rust-toolchain.toml` (1.85.0).
- **PXE net-install payload.** The PXE kickstart pointed `ostreecontainer --transport=oci` at the
  ISO over HTTP; the `oci` transport takes a local OCI-layout path, so every net-install died at
  the payload step. It now installs the container embedded in the ISO (`/run/install/repo/container`).
- **Seeded default password.** Both installer configs ran `/usr/local/bin/tendril-web` to seed the
  default admin password, but the appliance ships binaries in `/usr/bin` — the seed unit failed on
  every unattended install and the documented first-login flow never worked.
- **Shared-store mount detection** used `findmnt --target`, which matches the filesystem
  *containing* the path — a configured-but-unmounted store still resolved as live, silently writing
  ISOs/images/registry/CA material to the local disk under the mountpoint. Now an exact-mountpoint
  check; disconnecting a store also now surfaces a failed `umount` instead of forgetting a
  still-mounted share, and store fields that land in `/etc/fstab` reject embedded spaces.
- **vGPU guest-driver cache** (version-less filenames) is invalidated when the staged host-driver
  branch changes — a host upgrade no longer hands new stations the previous release's guest
  drivers — and removing the staged driver also drops the recorded branch + cached guest drivers.
- **Stale-marker recovery.** A `tendril-web` restart mid-vGPU-build or mid-PXE-ISO-download no
  longer wedges those panels forever; the PXE pidfile is verified against the process cmdline so a
  reboot-recycled PID can't be killed as root or shown as "serving".
- **Clone wizard.** Cloning a golden image now honors the low-latency (CPU pinning + hugepages)
  checkbox; a failed clone provision removes its orphaned overlay so retrying the name works; a
  custom disk path without `.qcow2` no longer silently drops the requested data volume.
- **Guest IPs on Windows.** `virsh domifaddr` parsing took a fixed column, so interface names with
  spaces ("Ethernet 2") hid the station's IP from the Guest/Remote-play panels.
- **Media verification.** The base-variant Bazzite ISO checksum URL was derived wrongly (404), so
  its verification silently degraded to "no upstream checksum".
- **Release pipeline.** `SHA256SUMS` is regenerated for every served ISO on release instead of
  clobbering the stable/older entries; `setup-branch-protection.sh` now fails on non-2xx API
  responses instead of reporting success with unprotected branches.

### Removed
- **The `tendrild`/`Role` clustering scaffold** — multi-machine is served by federation (decision
  2026-07-09): independent self-managing nodes aggregating over the token/mTLS API, not an elected
  controller with per-node agents. `docs/PLAN.md` D5/§4 updated to record the supersession.
- **`tendril-domain`** — a strict subset of `tendril-vm`'s dry-run mode. `tendril-vm` (no flags)
  renders the same domain XML; its dry run also adopts `tendril-domain`'s graceful no-GPU handling
  (informational message, exit 0), while `--define` still fails loudly without a GPU.

### Changed
- CI hygiene now asserts the Containerfile `FROM` lines match `versions.toml`'s
  `fedora_bootc_base` and that the rust pins in `rust-toolchain.toml`/`versions.toml` agree —
  "single source of truth" is enforced, not aspirational.
- Refactor pass (zero behavior change): one shared XML escaper, sysfs reader module, and
  detect/plan helpers replace copy-pasted scaffolding across the five CLI binaries (which now build
  a `StationRequest` and call `provision()` like the web layer); the web crate gains shared
  meminfo/size/LAN-IP/secret-write/curl-to-peer helpers replacing ~30 duplicated blocks; dead code
  dropped (unused `/stats` cluster, `StationSpec.gpu_address`, `Libvirt::session`, and friends);
  `scripts/refresh-dl-sums.sh` is the single SHA256SUMS regenerator for both release workflows.
  Two deliberate improvements: auth password files are 0600 from the first byte, and the console
  no longer reads sysfs twice per menu. Net ~350 lines removed.
- The PXE server no longer extracts the whole ISO into `/tmp` (tmpfs) nor copies the multi-GB ISO
  there — only the boot trees are extracted and the ISO is symlinked into the HTTP root.
- One-pass libvirt usage sweep for the Hardware page and station wizard (one `dumpxml` per domain
  instead of one per usage map); peer panels/actions on the Stations page refetch just that node
  instead of the whole fleet.
- Boot-prompt console messages derive from the real key-tap duration (was hardcoded "~18s", actual
  45s); shared helpers replace duplicated cert-SAN, URL-encoding, IOMMU-group-lookup, and
  shell-block-markup code; the appliance image drops dev/admin-host-only scripts.

## [0.22.0] - 2026-07-09

A security-hardening release. Four iterative multi-agent audit rounds swept the web app,
orchestrator, capability engine, provisioning, and the installer/PXE scripts; the final round came
back with no high- or medium-severity findings across three independent passes. This release fixes
everything the earlier rounds turned up plus the notable defense-in-depth items from the last one.
No feature or API changes — safe to update in place.

### Security
- **Federation input validation.** Every request-body URL that reaches a `curl` invocation or a
  config file is now validated as a real `http(s)` URL and the `curl` calls are `--`-terminated:
  the token-authed `/api/image-pull` source, and the `fed` (mTLS endpoint) fields on fleet-register
  and join-code — closing a leading-dash curl-option injection reachable by a shared-token holder.
- **`is_http_url` rejects shell metacharacters** (`$`, backtick, `;`, quotes, …) so a validated URL
  can't smuggle a command into the guest-side `sh -c` that fetches the vGPU licensing token.
- **Read-only enforcement.** Demo mode now blocks the VNC console WebSocket (live guest control is
  not "read-only"), and the viewer role is refused the full audit-log and journal downloads.
- **Private-key permissions.** TLS, federation CA + node, and DLS webserver keys are pre-created
  `0600` before `openssl` writes them (and the installed TLS key is written `0600` from the first
  byte) — no window where a key is world-readable.
- **Console scripts** JSON-encode and URL-encode the station/node names instead of splicing them
  into an inline `<script>`, and `reimage` re-validates the station name on the peer path.
- **Installer/kickstart.** The username is clamped to a POSIX-safe charset and quotes are stripped
  from the seed password before they reach Anaconda's `%post` shell / `user` directive; the
  removable-disk skip filter in the unattended/PXE installers is applied at install time.

## [0.21.0] - 2026-07-09

A gaming-completeness pass closing the biggest gaps against the design: low-latency tuning,
anti-cheat/VM-hiding, snapshots, can't-break-it rollback, a balancing fleet scheduler, RBAC + audit,
an in-guest agent, one-command room provisioning, and remote play — plus slimmer, more invisible
vGPU and fleet panels.

### Added
- **Low-latency mode** (F1): a station wizard toggle pins each vCPU 1:1 to a dedicated host core
  (reserving cores for the host + QEMU emulator, and avoiding cores other stations already hold) and
  uses hugepages when a pool exists — so gaming frame-times don't jitter from host scheduling.
- **Anti-cheat / VM-fingerprint hiding** (F2): the native-hardware overlay now spoofs SMBIOS/DMI
  (OEM strings + per-station serials) on top of hiding KVM and the hypervisor CPUID flag, so software
  that blocks VMs sees a plausible desktop. (Kernel-level anti-cheats still won't run — stated plainly.)
- **Station snapshots** (F10): point-in-time restore points (create / restore / delete) from the
  station page — snapshot before a risky Windows update or anti-cheat change, roll back instantly.
- **greenboot health-gated rollback** (F3): after any bootc OS update a required health check
  (libvirt + control plane up and answering) must pass, or the bootloader falls back to the previous
  deployment — the can't-break-it net the design promised.
- **Boot-time hardware adaptation** (F7): a per-boot service snapshots the GPU capability matrix and
  detects whether IOMMU is actually active; when it's off in firmware the Hardware page shows a banner
  telling you to enable VT-d/AMD-Vi — the one passthrough blocker only you can fix.
- **Read-only viewer role + audit log** (F9): an optional viewer password grants look-but-don't-touch
  access; every admin change is recorded (timestamp, actor, action, status) in a downloadable audit
  log. Both live in a new System "Access & audit" panel.
- **In-guest agent** (F8): wired the QEMU guest-agent channel (the Windows unattend already installs
  the agent), so a station's OS, hostname and IP surface in a new "Guest" panel — the basis for health
  and graceful shutdown.
- **Provision a room over PXE** (F6): `tendril-pxe.sh` turns a node into a proxy-DHCP + TFTP + HTTP
  PXE server that net-boots a rack of bare-metal PCs straight into the unattended installer — a room
  images itself hands-off. Safe on a live LAN (proxy-DHCP). The command is shown on the Fleet page.
- **Remote play** (F4): a station "Remote play" panel gives the exact Moonlight *Add PC* address (from
  the guest agent's IP) + pairing steps, plus WAN guidance (mesh-VPN first, else the Sunshine ports).

### Changed
- **Balancing GPU-aware scheduler** (F5): auto-placement across the fleet now picks the node with the
  most free passthrough GPUs (spreading onto the emptiest hardware), tie-broken by fewest existing
  stations then lowest load — instead of first-fit.
- **Slimmer, more invisible panels**: the vGPU section collapses all driver/guest/licensing machinery
  behind a "Set up GPU splitting" disclosure (whole-GPU passthrough needs none of it); fleet setup's
  four overlapping onboarding paths reduce to two clear actions ("Add a machine" / "Join a fleet") with
  the rest under Advanced.

## [0.20.0] - 2026-07-08

Invisible-by-default NVIDIA vGPU: one **vGPU** panel (driver → guest driver → licensing, all automatic),
the Windows guest driver auto-fetches like Linux, and licensing is automatic (a built-in server, or
bring your own). Plus AGPL relicensing, a stable release channel, and optionally-signed images.

### Added
- **NVIDIA vGPU licensing is automatic.** The built-in FastAPI-DLS license server auto-starts with
  sane defaults as soon as the vGPU host driver is active, and station provisioning installs the token
  into each guest — nothing is pasted into VMs. It's a single one-time opt-in (the gray-area note shows
  only on this path), after which driver + license + guest driver are all automatic and silent.
- **Bring your own NVIDIA license server.** If you run a real on-prem DLS/NLS appliance or CLS, point
  Tendril at its client-token URL and it **never runs the built-in emulated server** — guests are
  licensed by your legitimate server. A valid license means you don't need the emulation.
- **Dev-channel installs auto-enable OS updates.** A machine installed from the dev-channel ISO enables
  `bootc-fetch-apply-updates.timer` at first boot (only when it tracks `:dev`), so dev boxes roll
  forward with the channel automatically. Stable installs are unaffected (operator opts in).
- **License: AGPL-3.0-only, dual-licensed.** The project's license is no longer TBD — full text in
  `LICENSE`, the dual-licensing model (one open edition, commercial licenses available) in
  `LICENSING.md`, SPDX `license` in the Cargo workspace, and copyright in `NOTICE`.
- **Contributor License Agreement** (`CLA.md`) — required with a first PR so single-owner copyright
  (and thus dual licensing) survives outside contributions; process documented in `CONTRIBUTING.md`.
- **Security policy** (`SECURITY.md`) — private reporting, response expectations, per-channel
  support matrix, scope.
- **Stable release channel.** `promote-stable.yml` repoints `:stable` (and `:vgpu-amd-stable`) at an
  already-released image digest — promotion moves bits, never rebuilds — and publishes the
  `tendril-stable-installer-x86_64.iso` name. Channels, promotion checklist (bake time, hardware
  matrix, blockers, security review), and subscription commands in `docs/CHANNELS.md`.
- **Optional image signing** — `release.yml` signs the released image digest with cosign when
  `COSIGN_PRIVATE_KEY`/`COSIGN_PASSWORD` secrets are configured; non-fatal and off by default. The
  workflow fetches a pinned, checksum-verified cosign itself if the runner lacks one; the signing
  public key is committed as `cosign.pub`, with verification steps in `docs/CHANNELS.md`.

### Changed
- **One vGPU panel.** The System page's separate "vGPU host driver" and "vGPU licensing" panels are
  merged into a single **vGPU** panel — host driver → guest driver → licensing, top to bottom, all
  automatic. Licensing no longer has its own opt-in step: the built-in server is the default and
  auto-starts the moment the host driver is active (staging the licensed `.run` is the single gate).
- **The Windows vGPU guest driver is now invisible too.** It's auto-fetched from NVIDIA's own public
  bucket (paired per vGPU release, verified for 15.4–18.4) to match the staged host driver branch —
  exactly like the Linux `.run`. The old "NVIDIA doesn't publish it, upload it yourself" claim was
  wrong. Removed the guest-driver upload form, release picker, and per-driver entitlement checkbox; the
  panel is read-only status. Air-gapped escape hatch: `TENDRIL_VGPU_GUEST_EXE_URL` / `_RUN_URL`.

### Fixed
- **Green `dev` CI restored** — `cargo fmt` drift across five files from the 0.18 cycle and a
  clippy `useless_conversion` in the federation WebSocket bridge failed `ci.yml` on every push.
- **Demo System-page OS-image sample** was showing a stale `0.13.1`; updated to the current version.

## [0.18.0] - 2026-07-08

Fleet-wide station control (including live console) from any node, invisible vGPU guest drivers,
data-preserving re-splits, persistent data volumes, and a one-ISO guided/unattended installer.

### Added
- **Control any station from any node.** A peer's stations on the Stations page now have full
  lifecycle parity — start / stop / **force-off** / delete, dispatched to the owning node — plus a
  **self-refreshing** panel and state pills matching local. **Open** a peer station for its **live
  console**: this node bridges the browser's noVNC WebSocket to the owner's token/mTLS-authed
  `/api/station/:name/vnc`.
- **Invisible vGPU guest driver.** A vGPU station's guest driver is selected and installed
  automatically to match the host driver branch — there's no per-VM driver step. The host branch is
  captured when the host `.run` is staged; the matching Linux guest driver is fetched from NVIDIA's
  official bucket on demand (Windows installer supplied once from the licensed package). Golden images
  record their built-with branch, and the Media list flags a **branch mismatch** so a stale driver on
  a moved/pushed image is never silent.
- **Non-destructive GPU re-split.** Change a station's vGPU profile from its detail page **without
  recreating the disk** — the qcow2 (Windows / games / saves) is kept; only the mediated device swaps.
- **Persistent data volume.** An optional per-station data disk (`vdb`) — set via the wizard's
  *"Persistent data volume (GiB)"* field and auto-mounted in the guest (Bazzite + Windows) — that
  **survives OS reinstalls, base-image pushes, and re-splits** (a reimage replaces only the boot disk
  `vda`, never `vdb`).
- **One ISO, guided or unattended.** The installer ISO now carries a second GRUB entry, **"Install
  Tendril — Unattended (ERASES THIS DISK)"**: a visible ERASE countdown you can abort by powering off,
  then a hands-off single-disk install. The same `tendril.unattended` boot path is what the PXE
  "provision the room" flow will use. The guided install stays the default.
- **Dev-channel installer ISO** — an on-demand + nightly workflow builds an ISO from the rolling
  `:dev` image, so testers can install the dev channel directly and roll forward via `bootc`.

## [0.17.0] - 2026-07-08

Easier fleet-building, a touchless installer, and gaming provisioning (games + streaming).

### Added
- **Fleet-building without hand-typed IPs.** A store-less **join code** (base64 carrying the founder's
  URLs, the shared token, and the fleet CA) lets a new node join with no shared store: paste it on the
  node's **Join a fleet** screen and it adopts the CA + token, adds the founder as a peer, and
  **registers back** so membership is mutual (`/api/fleet/register`). **mDNS LAN discovery**
  (`_tendril._tcp`) surfaces nearby Tendril machines in Fleet setup — discovery finds them, the code
  grants trust (no auto-join). Peer entries can now carry an mTLS URL (`name=<ui>|<fed>`).
- **Touchless installer** (`scripts/build-installer.sh --unattended`, opt-in — the shipping ISO stays
  guided). Hands-off install for CI/test VMs and fleet provisioning: safe single-disk partitioning
  (targets one real disk — never a blind wipe, and skips zram/loop/rom), and a seeded **must-change**
  default web admin password.
- **Change the admin password in the UI** (System → Admin password), and a **force-change** flow: a
  seeded default password forces `/setup` on first use before the console is usable. New
  `tendril-web --seed-default-password`.
- **Load games into stations.** A **Shared Steam library** toggle shares the fleet store's
  `steam-library/` folder into a station over **virtio-fs** (games installed once, read by many),
  auto-mounted in-guest; plus a golden-image workflow guide ([docs/STEAM-GAMES.md](docs/STEAM-GAMES.md)).
- **Moonlight receiver** on stations (Windows via winget, Bazzite via Flathub) so a station can also
  *receive* a stream — the client to Sunshine's host.
- **Windows station auto-provisioning:** NVIDIA vGPU **guest driver** + FastAPI-DLS licensing token
  injected into the unattended install, plus optional **Steam / Sunshine / Discord** from official
  URLs on first logon.
- **Dark/light theme toggle** in the top bar (persisted, pre-paint to avoid flash).
- **CI: auto-deploy + rolling `:dev` edge image.** Every push to `dev` redeploys the local test/demo
  instances and publishes `git.onetick.ninja/flan/tendril:dev` (a bootc VM tracks it via
  `bootc switch …:dev`). `:latest` stays the stable main-release tag.

### Changed
- **Stations vs Fleet, re-cut along workloads vs infrastructure.** Stations is the single station
  surface (grouped by node in a fleet; **control a peer's stations** from it); Fleet is the
  infrastructure view (nodes/GPUs/health/setup). **Dashboard folded into Stations** as a summary strip
  (one fewer tab); the **Fleet tab is always visible** with a create/join empty state for a lone node.
- vGPU driver + licensing panels moved up under OS updates on the System page.

### Fixed
- Touchless installer: the disk-picker no longer selects Fedora's `zram0` swap device (Anaconda
  aborted with "Disk zram0 … does not exist"); it keeps root usable for the tty1 console autologin
  (a locked root dead-ended the login).

## [0.16.0] - 2026-07-07

vGPU, fleet federation, and hardened golden images.

### Added
- **Federation: manage a fleet of nodes from one UI.** A new **Fleet** tab (appears once peers are
  configured) aggregates every node over a JSON API (`/api/node`, shared-token authed), showing each
  node's stations, GPUs, and health, with reachable/unreachable status. **Create a station on the
  fleet** with GPU-aware placement — pick a node (or "auto") with a free compatible GPU and it's
  provisioned there (`/api/provision`). A shared **station registry** on the store lets a **down node's
  image-backed stations be re-homed** onto a healthy node in one click (human-confirmed). Nodes stay
  fully independent — no shared consensus, quorum, or fencing (see [docs/FEDERATION.md](docs/FEDERATION.md)).
- **Golden-image integrity.** Each captured image records a **SHA-256** (a sidecar that travels with it
  on the shared store); the **Station images** panel shows the hash and a **verify** action that
  recomputes and flags a **mismatch** — so a corrupt or tampered image isn't silently cloned into a new
  station (or re-homed across machines).
- **vGPU: split one GPU across multiple stations.** Two mechanisms, detected per-GPU from sysfs:
  - **Mediated devices (mdev)** — the NVIDIA vGPU / `vgpu_unlock` / Intel GVT-g path. The capability
    engine reads each GPU's `mdev_supported_types`; the create-station wizard lists every available
    profile ("… — vGPU: GRID RTX6000-4Q (2 free)"), and choosing one creates a persistent mediated
    device (`mdevctl define --auto` + `start`) and attaches it to the station as
    `<hostdev type='mdev'>`. The mdev is torn down when the station is deleted (or if provisioning
    fails), and the **Hardware** page's "Used by" shows which stations hold a slice.
  - **SR-IOV** — for GPUs that advertise `sriov_totalvfs` (AMD MxGPU, Intel Data Center GPU). An inline
    control on the **Hardware** page enables *N* virtual functions; the VFs then appear as their own
    GPUs and are passed through with the existing whole-GPU path.

  Tendril **detects and guides** — it consumes an mdev/SR-IOV-capable host driver but doesn't install
  proprietary vGPU drivers. **Station disks and whole-GPU passthrough are unchanged.**

### Changed
- **Golden images record their OS and capture atomically.** A capture now runs in the background and
  writes to a hidden temp, publishing the final image only when complete — so a half-written image is
  never listed or cloneable. The guest OS is recorded on capture and shown in the **Station images**
  list and the create-station pickers.
- **Streamlined storage config.** The NFS/SMB form is type-aware (the server/share placeholder switches
  with NFS vs SMB, SMB credentials appear only for SMB) and tucks mount point + options behind an
  Advanced toggle with sensible per-type defaults.

### Fixed
- **Install media isn't usable until fully downloaded.** Fetch scripts download to a `.part` file and
  rename on completion (the SteamOS fetcher joined the Windows one), and the Media page shows in-flight
  downloads as "downloading…" — so provisioning can't pick up a partial ISO.
- **Station creation can't corrupt a golden image, and clone/install are exclusive.** Cloning derives
  the guest OS from the image (no more mismatched pairing), the wizard hides install-only fields when a
  base image is chosen, and disk paths/names that would land in the images directory are rejected.
- **vGPU safety guards.** SR-IOV virtual-function changes are refused while a VF is assigned to a
  station (which would yank it out from under a running VM), and whole-GPU passthrough vs vGPU on the
  same physical card are mutually exclusive in the wizard.

## [0.15.0] - 2026-07-07

Shared storage and reusable golden images — the groundwork for clustering — plus a public demo.

### Added
- **Remote media storage (NFS / SMB).** Point Tendril's ISOs and golden images at a mounted **NFS or
  SMB/CIFS** share from **Media → Storage**, so every node — and every station-image clone — sees the
  same media and templates (the shared store behind clustering). The mount is persisted to
  `/etc/fstab` (`nofail,_netdev`) so it reconnects on boot; SMB credentials are written to a root-only
  file. `iso_dir()`/`image_dir()` resolve to the share when it's mounted and fall back to local
  (`/var/lib/tendril/{isos,images}`) otherwise. **Station disks stay local** (fast, per-node); only
  media and images move to shared storage.
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

[Unreleased]: https://git.onetick.ninja/flan/tendril/compare/v0.22.0...HEAD
[0.22.0]: https://git.onetick.ninja/flan/tendril/compare/v0.21.0...v0.22.0
[0.21.0]: https://git.onetick.ninja/flan/tendril/compare/v0.20.0...v0.21.0
[0.20.0]: https://git.onetick.ninja/flan/tendril/compare/v0.18.0...v0.20.0
[0.18.0]: https://git.onetick.ninja/flan/tendril/compare/v0.17.0...v0.18.0
[0.17.0]: https://git.onetick.ninja/flan/tendril/compare/v0.16.0...v0.17.0
[0.16.0]: https://git.onetick.ninja/flan/tendril/compare/v0.15.0...v0.16.0
[0.15.0]: https://git.onetick.ninja/flan/tendril/compare/v0.14.0...v0.15.0
[0.5.0]: https://git.onetick.ninja/flan/tendril/compare/v0.4.0...v0.5.0
[0.4.0]: https://git.onetick.ninja/flan/tendril/compare/v0.3.0...v0.4.0
[0.3.0]: https://git.onetick.ninja/flan/tendril/compare/v0.2.0...v0.3.0
[0.2.0]: https://git.onetick.ninja/flan/tendril/compare/v0.1.0...v0.2.0
[0.1.0]: https://git.onetick.ninja/flan/tendril/src/tag/v0.1.0

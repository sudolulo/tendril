# Clustering plan — federation

Status: **design; integrity tracking in progress.** Tendril "clustering" is **federation**: one web UI
over a fleet of otherwise-independent nodes, with GPU-aware placement, human-confirmed recovery, and
verified golden images shared between machines. There is deliberately **no distributed control plane** —
no shared consensus, quorum, leader election, or fencing.

## Why federation (and not a real cluster)

The reasoning, following Tendril's own constraints to their conclusion:

1. **A station *is* its GPU.** Stations own physical GPUs (or vGPU slices), which are node-local, so a
   running station **cannot live-migrate**. Proxmox's headline feature is simply unavailable.
2. **We dropped guaranteed auto-failover** (gaming sessions are ephemeral — when a node dies the player
   is already disconnected; cold-restarting the VM ~a minute faster, automatically, isn't worth
   fencing/quorum complexity or reserved spare-GPU cost). See "Deferred" below.
3. **Once the cluster never acts autonomously, it needs no shared consensus state.** Quorum, Raft,
   distributed locks, and fencing exist to make *autonomous* decisions (auto-failover, auto-rebalance)
   safe. We don't do those, so none of that machinery is needed.
4. **Each Tendril node is already an independent, API-driven control plane.** It fully manages its own
   stations via its own HTTP API + libvirt. The fleet layer is a thin aggregator on top — not a new
   system that owns the nodes.

Reusing a heavyweight platform (KubeVirt, Incus, oVirt, Nomad) was rejected: each either **owns the VM
lifecycle** — surrendering the low-level control that is Tendril's product (Secure Boot + TPM, the
noVNC console, USB seats, the native-hardware overlay) — or is far too heavy for a bootc appliance, or
(Nomad/Consul) is BSL-licensed. The framework we "reuse" is **Tendril itself**: each node's existing API
and web stack.

## Architecture

1. **Nodes are unchanged.** Every node runs the same Tendril today: its own web UI, API, and libvirt
   orchestrator, fully functional standalone. Federation adds nothing a single node depends on.

2. **Aggregator / management view.** One node (or any node) runs a fleet view that calls each node's
   existing API and presents the whole fleet in one place — stations, GPUs, health — read-only to start.
   No shared mutable state; each node remains the source of truth for its own stations.

3. **Stateless GPU-aware placement.** To create a station on the fleet, the management layer queries each
   node's live capability matrix (`capability-engine::detect`), picks a node with a **compatible free
   GPU** (vendor match required for a given golden image, model preferred), and calls **that node's
   existing create-station endpoint**. The "scheduler" is a query-and-choose each time — nothing to keep
   consistent.

4. **Lightweight station registry on the shared store.** So the fleet knows what stations exist (and can
   recover a node that's currently down), each station writes a small JSON record — `{ name, node,
   gpu/vgpu, golden image, disk location, seat }` — to the shared media/image store. This is read-mostly
   config, **not** a live consensus state machine, so it needs no consistency protocol.

5. **Verified golden images shared between machines.** Golden images live on the shared store and carry a
   recorded **SHA-256** (sidecar). Any node can verify an image's integrity before cloning or re-homing
   from it — catching corruption or truncation on the shared NFS/SMB path, or a tampered image. See
   "Golden-image integrity" below.

6. **Assisted re-home (human-confirmed).** Nodes publish periodic heartbeats (a timestamp file on the
   shared store); a stale heartbeat marks a node **down** (reachability only, no quorum vote). The
   management UI then lists that node's stations (from the registry) and offers **one-click cold
   re-home** to a node with a compatible free GPU — the human confirms the dead node is actually off.
   - **Local-disk stations (default):** safe — independent disks; state resets to the golden image.
   - **Shared-disk stations:** re-home requires a "confirm the node is powered off" step to avoid a
     shared-disk double-mount. Lightweight human-fencing, not automatic fencing.

## Golden-image integrity

Golden images are the one thing genuinely shared and *executed* across machines, so their integrity is
verified rather than assumed:

- **On capture**, after the image is finalized, its **SHA-256 is computed and recorded** in a sidecar
  next to the image on the shared store (`<name>.qcow2.sha256`). The sidecar travels with the image, so
  every node sees the same expected hash.
- **Verify** (on demand, and recommended before a cross-node clone / re-home) recomputes the image's
  hash and compares it to the recorded value; a mismatch marks the image (`<name>.qcow2.mismatch`) and
  the UI flags it, so a corrupt/tampered image isn't silently cloned into a new station. Verification
  runs in the background (multi-GB) with a status the UI polls.
- Because cloning uses a **copy-on-write overlay backing onto the image**, a corrupted base would
  silently corrupt every station cloned from it — which is exactly why the recorded hash + verify exist.

This mirrors the existing **install-media** verification (ISOs carry `.verified`/`.sha256`/`.mismatch`
markers), extended to golden images and framed for the multi-machine case.

## The shared store

Everything the fleet shares lives on the existing NFS/SMB media/image store (`storage` module):
golden images + their SHA-256 sidecars + OS/GPU metadata, the station registry, and node heartbeats.
It is **read-mostly** and needs no consistency protocol. Existing stations keep running even if the
store is briefly unreachable; only fleet management/placement/re-home pause.

## Host GPU requirements

- **Per node:** at least one passthrough-capable GPU (IOMMU isolation, clean-ish ACS groups — already
  assessed by the capability engine).
- **Heterogeneous pool supported.** Mixed vendors/models allowed; a station can only run/re-home on a
  **compatible** GPU (vendor match required, model preferred), so golden images record the **GPU
  vendor/model** they were built on (alongside guest OS and SHA-256) for the scheduler to match.
- **Seats (USB) are node-local** — a re-homed station gets its VM back, not its physical peripherals,
  until a seat is reassigned on the new node.
- **No reserved spare GPU** is required — recovery is best-effort against whatever compatible GPU is free
  at the time, chosen by a human.

## Phased build order

| Phase | Delivers |
|---|---|
| **A — Fleet view** | Aggregator management view over the per-node APIs (stations, GPUs, health) + node heartbeats. Read-only. |
| **B — Placement + remote provision** | Stateless GPU-aware placement: pick a compatible node and call its create endpoint. Station registry on the shared store. |
| **C — Integrity + assisted re-home** | Golden-image SHA-256 record + verify (started now, standalone); node-down detection; one-click human-confirmed cold re-home (with the shared-disk power-off guard). |

## Deferred: autonomous operation

Guaranteed unattended failover or automatic rebalancing — the only features that would need shared
consensus state and safe fencing — are **out of scope**. Revisit only if Tendril becomes an always-on
service (paid cloud-gaming / remote workstations) on a **homogeneous** GPU pool with **reserved spare
capacity** (N+1 hot spare, vGPU headroom, or priority preemption). At that point, reuse a proven
substrate — **etcd** (Apache-2.0, sidecar) or **openraft** (embedded Rust Raft) for consistent state +
health, or go all-in on **Nomad** (accepting its BSL license + a custom libvirt task driver) — layered
*on top of* federation, not instead of it. Do not hand-roll consensus.

## Open questions / risks

- **Shared store is a dependency** (already is, for images). Unreachable → management pauses; running
  stations are unaffected.
- **Registry writes** are low-contention config, but still need a stale-lock-safe write (bounded lease),
  not a hand-rolled forever-lock.
- **Auth across nodes** — the aggregator needs credentials/tokens to call each node's API.
- **Shared-disk re-home** must enforce the power-off/confirm-dead step; local-disk stations (default)
  are unaffected.

See the broader roadmap in [PLAN.md](PLAN.md) and the vGPU capability model in [VGPU.md](VGPU.md).

# Clustering plan

Status: **design / not implemented.** This is the plan for turning Tendril from a single-box appliance
into a fleet of boxes managed as one. It records the target architecture, the reasoning that shaped it
(including what we deliberately *aren't* building), and a phased build order. Nothing here ships yet.

## Goal

Manage a fleet of Tendril nodes (target scale: **5–20+ boxes**) from one web UI, and place each station
on whichever node has a **compatible free GPU**. When a node goes down, make recovering its stations a
**one-click, human-confirmed** operation.

Explicitly **not** a goal: guaranteed *unattended automatic failover*. See the reasoning below — for
Tendril's workload and hardware assumptions, it's high cost and risk for low real value, so it's
deferred (with the conditions to revisit it spelled out at the end).

## The reasoning (why this shape, not Proxmox's)

We started by looking at Proxmox (compute cluster: corosync quorum + pmxcfs replicated config + watchdog
fencing + shared-storage live-migration/HA) and TrueNAS (storage appliance: dual-controller failover;
TrueCommand single-pane management). It's tempting to copy Proxmox. We're not, and here's the chain:

1. **A station *is* its GPU.** A Tendril station exists to own a physical GPU (or a vGPU slice). GPUs
   are physical and node-local, so a running station **cannot live-migrate** — you can't copy a GPU's
   state over the wire. That removes Proxmox's headline feature outright.

2. **Failover, if we did it, is inherently cold and GPU-constrained.** A re-homed station *restarts*
   (no saved state) and only where a **compatible free GPU** exists. GPUs — not nodes — are the unit of
   redundancy, and they're the expensive part of the box, so spare GPU capacity is a real cost.

3. **The workload is ephemeral gaming sessions.** When a node dies mid-session the player is already
   disconnected and unsaved progress is gone *regardless*. So the only thing automatic failover buys
   over a human clicking "re-home" is ~30–90s of unattended recovery on a session that's already broken.
   That's a poor trade against fencing/quorum complexity and reserved-GPU cost.

4. **Heterogeneous GPUs make *automatic* placement fragile.** We're assuming a mixed pool of cards (see
   below). A golden image has its GPU vendor's drivers baked in, so a station can only land on a
   compatible GPU; reliable *unattended* placement across arbitrary mixed hardware is exactly the case
   that breaks. A human resolving "which compatible GPU" at re-home time is far more robust.

5. **Therefore the heavy machinery loses its justification.** Raft quorum and watchdog self-fencing
   exist almost entirely to make *automatic* failover safe. Drop that guarantee and there's nothing left
   for them to protect — the design collapses to something much simpler.

Net: Tendril clustering is **fleet management + GPU-aware placement + assisted recovery**, built on the
shared storage we already ship — *not* a consensus-and-fencing cluster.

## What's already in place

Two of the three substrates are shipped:
- **Shared store** — NFS/SMB media/image storage (`storage` module) so every node sees the same media
  and golden images. This doubles as the cluster's coordination substrate (below).
- **Portable golden images** — compressed standalone qcow2 templates + copy-on-write cloning
  (`images` module, `orchestrator::guest::create_overlay`), with recorded guest-OS metadata.

Missing: the **control plane** — shared cluster state, a scheduler, per-node reconcile, and assisted
re-home.

## Target architecture

No consensus protocol, no fencing. The shared store *is* the coordination layer.

1. **Shared cluster state.** Desired-state lives on the shared store (a small SQLite db or a directory
   of files that every node already mounts): nodes + heartbeats, each node's **GPU capability matrix**
   (from `capability-engine::detect`, incl. vendor/model and vGPU profiles), and **station objects**
   `{ name, home_node, gpu/vgpu assignment, golden image, disk location, seat }`. Any node reads it
   locally.

2. **Write serialization via file locks — no elected coordinator, no quorum.** Cluster mutations
   (create/move a station) are infrequent, so a writer takes a lock on the shared store, writes, and
   releases. Any node can do this, so there's **no single point of failure and no leader election** to
   build. Split-brain is naturally bounded: a station is GPU-pinned, so two nodes can't both run the
   same station's GPU.

3. **Per-node reconcile loop.** Each node watches shared state and drives its local `libvirt` /
   `provision()` to match its assigned stations. Reuses the entire existing provisioning path unchanged.

4. **GPU-aware scheduler (heterogeneous-first).** On "create station," pick a node with a compatible
   free GPU/vGPU profile. Compatibility rules for a heterogeneous pool:
   - **Same GPU vendor is mandatory** to run a given golden image (drivers are baked in).
   - **Same model is preferred**; a different model of the same vendor usually works but is ranked lower.
   - vGPU profiles are matched by parent-GPU capability.
   The scheduler tracks each node's GPUs and their assignment, and refuses placements with no compatible
   free GPU.

5. **Health + assisted re-home.** Nodes write periodic heartbeats to the shared store; a stale heartbeat
   marks a node **down** (reachability only — no quorum vote). The UI then lists that node's stations
   and offers **one-click cold re-home** to a node with a compatible free GPU, **human-confirmed**. The
   human is the "fencing": they confirm the node is actually dead before re-homing.
   - **Local-disk stations (the default):** safe to re-home — the two nodes have independent disks, and
     the dead node is unreachable anyway. State resets to the golden image (or the last local disk if
     recoverable).
   - **Shared-disk stations:** re-home must include a **"confirm the node is powered off"** step to
     avoid a double-mount of the shared disk. This is lightweight human-fencing, not automatic fencing.

## Host GPU requirements

- **Per node:** at least one **passthrough-capable GPU** — IOMMU isolation, clean-ish ACS groups
  (already assessed by the capability engine).
- **Heterogeneous pool is supported.** Mixed vendors and models are allowed. The cost is that a station
  can only run/re-home on a **compatible** GPU (vendor match required, model match preferred), so the
  scheduler and golden-image metadata must track GPU compatibility. Golden images already record their
  guest OS; they'll also need to record the **GPU vendor/model** they were built on so placement can
  match.
- **Seats (USB) are node-local.** A re-homed station gets its VM back, not its physical keyboard/mouse,
  until a seat is reassigned on the new node.
- **No reserved spare GPU is required** in this design — recovery is best-effort against whatever
  compatible GPU is free at the time, decided by a human. (A guaranteed-capacity policy only becomes
  necessary if we later add automatic failover; see below.)

## Phased build order

Each phase is independently useful and testable.

| Phase | Delivers |
|---|---|
| **A — Fleet state + read-only view** | Shared cluster-state store, node heartbeats + GPU inventory, and a web UI that shows the whole fleet (nodes, GPUs, stations) read-only. |
| **B — GPU-aware scheduler + remote provision** | Create a station targeted at (or auto-placed on) a node; that node's reconcile loop provisions it. Heterogeneous GPU compatibility matching. |
| **C — Assisted re-home** | Node-down detection; one-click, human-confirmed cold re-home of a down node's stations to a compatible node (with the shared-disk power-off guard). |

## Deferred: hard automatic failover — when to revisit

Guaranteed unattended failover (and the Raft/quorum + watchdog-fencing + reserved-GPU-capacity machinery
it needs) is **out of scope** for the reasons above. Revisit it only if the constraints change:

- Tendril becomes an **always-on service** (paid cloud-gaming, remote workstations) where unattended
  uptime has real value — not casual gaming where the session is already broken.
- The deployment moves to a **homogeneous GPU pool** (identical cards), which is what makes automatic
  placement reliable and any card able to take any station.
- The operator is willing to **reserve spare GPU capacity** — via one of: an N+1 hot-spare GPU, **vGPU
  headroom** (run cards under capacity and absorb an orphaned station as an extra, lower-performance
  slice — cheapest, leans on the shipped vGPU support), or **priority preemption** (stop a lower-priority
  station to free its GPU).

At that point the plan grows a consensus layer (`openraft` for quorum + a replicated state machine),
watchdog **self-fencing** (a node that loses quorum stops its stations within T seconds so survivors can
safely restart them), and a **failover-capacity policy** — essentially the Proxmox-shaped design, added
*on top of* the simpler control plane, not instead of it.

## Open questions / risks

- **Shared store is a hard dependency** (it already is for images). If it's unreachable, scheduling and
  re-home pause — though existing stations keep running locally.
- **Write-lock correctness.** Serializing mutations on a network filesystem must handle stale locks
  (a crashed writer) — bounded lease + takeover, not a hand-rolled forever-lock.
- **Auth across nodes.** Sessions are per-node in-memory today; a fleet needs shared credentials and
  either replicated sessions or stateless tokens.
- **Manual re-home of shared-disk stations** must enforce the power-off/confirm-dead step to avoid a
  double-mount; local-disk stations (the default) are unaffected.

See the broader roadmap in [PLAN.md](PLAN.md) and the vGPU capability model in [VGPU.md](VGPU.md).

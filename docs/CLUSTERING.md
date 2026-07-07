# Clustering plan

Status: **design / not implemented.** This is the plan for turning Tendril from a single-box appliance
into a cluster of boxes managed as one. It records the target architecture, the constraints that make
Tendril different from Proxmox, and a phased build order. Nothing here ships yet.

## Goal

Manage a fleet of Tendril nodes from one web UI, place stations on whichever node has a suitable free
GPU, and — where the operator opts in — **cold-fail a dead node's stations onto survivors**.

Chosen shape (see the trade-offs below): **peer / multi-master** control plane with **cold-failover
HA**. No single point of failure; any node can manage the cluster.

## Why Tendril isn't Proxmox

The defining constraint: **a passed-through GPU is physical and node-local, so a running station
cannot live-migrate.** You can't copy a GPU's state over the wire. That removes Proxmox's headline
feature and reshapes what "cluster" means for us.

| Capability | Proxmox | Tendril |
|---|---|---|
| Live-migrate a running VM | ✅ | ❌ — GPU-bound, impossible |
| Cold re-home a station to another node | ✅ | ✅ *only if* its disk is on shared storage (else re-clone from a golden image) |
| Shared golden images across nodes | via storage | ✅ **already shipped** (NFS/SMB store + portable qcow2) |
| GPU-aware placement | generic scheduler | **the core of it** |
| One UI managing all nodes | multi-master (pmxcfs) | this plan |

Consequences to design around:
- **HA is always cold and capacity-constrained.** A station only comes back if a surviving node has a
  compatible *free* GPU, and it *restarts* — no live state.
- **Seats (USB) and GPUs are node-local.** A re-homed station may boot without its seat until one is
  reassigned on the new node.
- **Local NVMe vs shared disk is a real tension.** Gaming wants fast local disks; re-homing needs the
  disk reachable elsewhere. Resolution: **shared disk is a per-station opt-in** ("make this station
  re-homeable"); everything else stays on fast local storage.

## Reference: how the two systems we're borrowing from work

**Proxmox (compute cluster).**
- **Corosync** — membership + messaging over a low-latency link; provides **quorum** (a node acts only
  if it sees >50% of the cluster) to prevent split-brain. Even-node clusters add a **QDevice**
  tiebreaker.
- **pmxcfs** (`/etc/pve`) — a SQLite-backed FUSE filesystem **replicated to every node** by corosync;
  holds VM/storage/user config. This is what makes it multi-master.
- **Shared storage** (Ceph/NFS/iSCSI/ZFS-replication) is what enables live migration and HA restart.
- **HA** — a cluster resource manager elects actions, a local manager executes them, and a **watchdog
  fences** a dead node so its VMs can restart elsewhere safely.

**TrueNAS (storage appliance — for contrast).**
- **Dual-controller HA** — two controller heads over shared SAS disks, active/passive; the standby
  imports the ZFS pool on failure. Storage-controller failover, not scale-out compute.
- Scale-out clustered SMB (Gluster + CTDB) was added then **removed**. TrueCommand is a single-pane
  manager for many boxes, not a cluster.

Takeaway: Proxmox is the right mental model; Tendril is "Proxmox minus live-migration, plus
GPU-awareness."

## Target architecture

1. **Membership + quorum.** Embed a Rust Raft library (**`openraft`**) rather than corosync/C: it gives
   leader election, a replicated log, and quorum in-process — no external daemon, which suits the bootc
   appliance. Even-node clusters need a lightweight **witness** (QDevice equivalent) as tiebreaker.

2. **Replicated cluster state** (the pmxcfs equivalent) — a Raft state machine holding:
   - nodes and their liveness,
   - each node's **GPU capability matrix** (from `capability-engine::detect`, incl. vGPU profiles),
   - **station objects**: `{ name, home_node, gpu/vgpu assignment, golden image, disk location,
     ha_flag, seat }`.

   Reads are served locally on any node; writes go through the Raft leader — so it *feels*
   multi-master (any node's UI accepts a request and forwards it to the leader).

3. **Per-node reconcile loop** (the Proxmox LRM equivalent) — each node watches desired state and drives
   its local `libvirt` / `provision()` to match: "station X should run here → define + start it."
   Reuses the entire existing provisioning path unchanged.

4. **GPU-aware scheduler** — on "create station," pick a node with a compatible **free** GPU / vGPU
   profile, write the assignment; the target node's loop provisions it. Placement is the scheduling
   problem, since GPUs are the scarce, node-pinned resource.

5. **Cold-failover HA** — the hard part:
   - **Self-fencing watchdog.** A node that loses quorum must **stop its stations within T seconds**
     (watchdog reboot, like Proxmox). Only then can survivors safely assume a partitioned node isn't
     still running a station — otherwise a double-run or shared-disk double-mount corrupts data.
   - **Re-home.** The leader reassigns a dead node's HA-flagged stations to a survivor with (a) a
     compatible free GPU and (b) access to the station's disk. State survives **only** for stations on
     shared storage; a local-disk station can only be re-cloned fresh from its golden image.

## What's already in place

Two of the three substrates are shipped:
- **Shared store** — NFS/SMB media/image storage (`storage` module) so every node sees the same media
  and golden images.
- **Portable golden images** — compressed standalone qcow2 templates + copy-on-write cloning
  (`images` module, `orchestrator::guest::create_overlay`).

Missing: the **control plane** (membership, replicated state, reconcile loop, scheduler, fencing).

## Phased build order

Each phase is independently useful and testable; the distributed-systems risk is concentrated in D, so
it lands last.

| Phase | Delivers | Risk |
|---|---|---|
| **A — Membership + read-only cluster view** | `openraft` cluster, node join/leave, replicated node + GPU inventory + station objects; the web UI shows the whole cluster (read-only). | Medium |
| **B — Remote provision + scheduler** | Create a station targeted at a node; that node's reconcile loop provisions it. GPU-aware placement picks the node. | Medium |
| **C — Shared-disk opt-in** | A per-station "re-homeable" flag puts its disk on the shared store instead of local NVMe. | Low |
| **D — HA / fencing** | Watchdog self-fence on quorum loss; health detection; automatic cold re-home of HA stations to a survivor with a free compatible GPU. | **High** |

## Open questions / risks

- **Fencing correctness is the scary part.** Self-fencing + quorum is where distributed systems bite; a
  bug means double-run or corruption. Proxmox leans on a battle-tested watchdog for exactly this — ours
  needs careful design and hard testing (fault injection, partition tests) before D is trusted.
- **Even-node split-brain** needs a witness/arbiter.
- **Auth across nodes.** Sessions are per-node in-memory today; multi-master needs replicated
  credentials and either replicated sessions or stateless tokens.
- **Shared-disk performance.** HA stations on NFS/shared storage pay a latency cost vs local NVMe —
  acceptable because it's opt-in per station.
- **Scheduler capacity.** "HA" only works if survivors keep a spare compatible GPU; the scheduler may
  need to reserve headroom, or HA is explicitly best-effort.

See the broader roadmap in [PLAN.md](PLAN.md) and the vGPU capability model in [VGPU.md](VGPU.md).

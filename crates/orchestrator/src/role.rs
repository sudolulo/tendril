//! Process role: single-node runs both roles; a cluster elects one controller.

/// The role(s) this process performs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// Controller + agent in one process (single machine).
    SingleNode,
    /// Cluster controller (scheduling, state, API).
    Controller,
    /// Per-node agent (libvirt, VFIO, GPU state).
    Agent,
}

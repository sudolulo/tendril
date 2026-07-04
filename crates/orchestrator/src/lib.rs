//! Tendril orchestrator.
//!
//! Runs as a controller and/or per-node agent. The controller schedules gaming stations onto nodes
//! with a free, compatible GPU; the agent talks to libvirt on each node. Single-node mode runs both
//! roles in one process; cluster mode elects one controller.

pub mod domain;
pub mod lifecycle;
pub mod role;
pub mod station;

pub use domain::DomainSpec;
pub use lifecycle::{DomainState, Libvirt};
pub use role::Role;
pub use station::{GuestOs, StationSpec};

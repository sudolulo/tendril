//! Tendril orchestrator.
//!
//! Runs as a controller and/or per-node agent. The controller schedules gaming stations onto nodes
//! with a free, compatible GPU; the agent talks to libvirt on each node. Single-node mode runs both
//! roles in one process; cluster mode elects one controller.

pub mod domain;
pub mod guest;
pub mod kickstart;
pub mod lifecycle;
pub mod provision;
pub mod role;
pub mod station;
pub mod unattend;

pub use domain::{DomainSpec, UsbPassthrough};
pub use guest::{build_kickstart_seed_with, build_seed_iso_with, InstallMedia};
pub use kickstart::{render_kickstart, KickstartSpec};
pub use lifecycle::{DomainState, Libvirt};
pub use provision::{provision, ProvisionReport, StationRequest};
pub use role::Role;
pub use station::{GuestOs, StationSpec};
pub use unattend::{render_autounattend, GuestApp, UnattendSpec};

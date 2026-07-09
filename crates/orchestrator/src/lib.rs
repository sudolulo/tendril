//! Tendril orchestrator.
//!
//! Station provisioning and lifecycle over libvirt for a single node. Multi-node setups use the
//! web layer's federation (each node stays fully self-managing; peers aggregate over the JSON API)
//! rather than a clustered controller/agent split — see docs/FEDERATION.md.

pub mod domain;
pub mod guest;
pub mod kickstart;
pub mod lifecycle;
pub mod provision;
pub mod station;
pub mod unattend;
mod xml;

pub use domain::{CpuPinning, DomainSpec, UsbPassthrough};
pub use guest::{build_kickstart_seed_with, build_seed_iso_with, InstallMedia};
pub use kickstart::{render_kickstart, KickstartSpec};
pub use lifecycle::{
    parse_pci_hostdevs, parse_usb_hostdevs, DomainState, GuestAgentInfo, Libvirt, Snapshot,
};
pub use provision::{provision, ProvisionReport, StationRequest};
pub use station::{GuestOs, StationSpec};
pub use unattend::{render_autounattend, GuestApp, UnattendSpec};

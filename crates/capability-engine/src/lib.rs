//! Tendril capability engine.
//!
//! Enumerates the host's GPUs and IOMMU groups and classifies each GPU into a
//! [`matrix::Capability`] so the provisioning layer knows what is possible on this hardware.
//!
//! Everything here is scaffolding; the real sysfs parsing lands in Phase 1 (see `TODO`s).

pub mod iommu;
pub mod matrix;
pub mod pci;

pub use matrix::{Capability, CapabilityMatrix, GpuCapability};
pub use pci::{GpuDevice, GpuVendor};

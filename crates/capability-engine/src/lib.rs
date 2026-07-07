//! Tendril capability engine.
//!
//! Enumerates the host's GPUs and IOMMU groups and classifies each GPU into a
//! [`matrix::Capability`] so the provisioning layer knows what is possible on this hardware.
//!
//! The public entry point is [`detect`], which reads the live host (`/sys`). The `*_from` variants
//! take an explicit sysfs root so the logic can be unit-tested against fixtures.

pub mod iommu;
pub mod matrix;
pub mod pci;
pub mod usb;
pub mod vgpu;

pub use iommu::{IommuGroup, PassthroughViability};
pub use matrix::{Capability, CapabilityMatrix, GpuCapability};
pub use pci::{GpuDevice, GpuVendor};
pub use usb::{UsbController, UsbDevice};
pub use vgpu::{MdevType, VgpuSupport};

/// Detect all display devices on the live host and classify each into a [`CapabilityMatrix`].
pub fn detect() -> CapabilityMatrix {
    let gpus = pci::enumerate();
    let groups = iommu::read_groups();
    matrix::build(gpus, &groups)
}

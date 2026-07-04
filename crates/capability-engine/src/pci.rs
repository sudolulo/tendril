//! PCI enumeration of GPU devices.

/// GPU silicon vendor, resolved from the PCI vendor id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GpuVendor {
    Nvidia,
    Amd,
    Intel,
    Unknown,
}

impl GpuVendor {
    /// Classify a PCI vendor id.
    pub fn from_vendor_id(id: u16) -> Self {
        match id {
            0x10de => GpuVendor::Nvidia,
            0x1002 => GpuVendor::Amd,
            0x8086 => GpuVendor::Intel,
            _ => GpuVendor::Unknown,
        }
    }
}

/// A GPU discovered on the PCI bus.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GpuDevice {
    /// PCI address, e.g. `0000:01:00.0`.
    pub address: String,
    /// PCI vendor id (e.g. `0x10de` for NVIDIA).
    pub vendor_id: u16,
    /// PCI device id.
    pub device_id: u16,
    /// Resolved vendor.
    pub vendor: GpuVendor,
    /// Human-readable model string, if resolvable.
    pub model: Option<String>,
}

/// Enumerate all GPU-class PCI devices on the host.
///
/// TODO(phase-1): parse `/sys/bus/pci/devices/*` for display-controller classes (`0x0300xx`).
pub fn enumerate() -> Vec<GpuDevice> {
    Vec::new()
}

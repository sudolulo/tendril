//! PCI enumeration of GPU devices from sysfs.

use std::fs;
use std::path::Path;

/// Default sysfs path holding one directory per PCI device.
const PCI_DEVICES: &str = "/sys/bus/pci/devices";

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

/// Enumerate all display-controller PCI devices on the live host.
pub fn enumerate() -> Vec<GpuDevice> {
    enumerate_from(Path::new(PCI_DEVICES))
}

/// Enumerate display-controller devices under an explicit sysfs `devices` directory.
///
/// Display controllers are PCI class `0x03xxxx` (VGA, 3D, other display). Non-display functions of a
/// GPU (its audio/USB companions) are intentionally excluded here — IOMMU grouping ([`crate::iommu`])
/// is what ties those together for passthrough.
pub fn enumerate_from(devices_dir: &Path) -> Vec<GpuDevice> {
    let Ok(entries) = fs::read_dir(devices_dir) else {
        return Vec::new();
    };

    let mut gpus: Vec<GpuDevice> = entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            let class = read_hex(&path.join("class"))?;
            // Top byte of the 24-bit class is the base class; 0x03 == display controller.
            if class >> 16 != 0x03 {
                return None;
            }
            let vendor_id = read_hex(&path.join("vendor"))? as u16;
            let device_id = read_hex(&path.join("device"))? as u16;
            Some(GpuDevice {
                address: entry.file_name().to_string_lossy().into_owned(),
                vendor_id,
                device_id,
                vendor: GpuVendor::from_vendor_id(vendor_id),
                model: None,
            })
        })
        .collect();

    gpus.sort_by(|a, b| a.address.cmp(&b.address));
    gpus
}

/// Read a sysfs file containing a `0x`-prefixed hex value (e.g. `class`, `vendor`, `device`).
fn read_hex(path: &Path) -> Option<u32> {
    let raw = fs::read_to_string(path).ok()?;
    let trimmed = raw.trim();
    let hex = trimmed.strip_prefix("0x").unwrap_or(trimmed);
    u32::from_str_radix(hex, 16).ok()
}

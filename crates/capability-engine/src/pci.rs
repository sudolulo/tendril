//! PCI enumeration of GPU devices from sysfs.

use std::fs;
use std::path::Path;

/// Default sysfs path holding one directory per PCI device.
const PCI_DEVICES: &str = "/sys/bus/pci/devices";

/// Where the `hwdata` package installs the PCI id database (the one `lspci` reads).
const PCI_IDS_PATHS: &[&str] = &["/usr/share/hwdata/pci.ids", "/usr/share/misc/pci.ids"];

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

/// Enumerate all display-controller PCI devices on the live host, with friendly model names
/// resolved from the system `pci.ids` database.
pub fn enumerate() -> Vec<GpuDevice> {
    let mut gpus = enumerate_from(Path::new(PCI_DEVICES));
    if let Some(ids) = PCI_IDS_PATHS
        .iter()
        .find_map(|p| fs::read_to_string(p).ok())
    {
        for g in &mut gpus {
            g.model = lookup_model(&ids, g.vendor_id, g.device_id);
        }
    }
    gpus
}

/// Look up a device's friendly name in a `pci.ids` database, e.g. `10de:1e84` →
/// `"TU104 [GeForce RTX 2070 SUPER]"`. Returns `None` if the vendor/device isn't listed.
///
/// `pci.ids` is indentation-structured: vendor lines start at column 0, their device lines are
/// indented one tab, and subsystem lines two tabs — all as `<4-hex-id>  <name>`.
pub(crate) fn lookup_model(ids: &str, vendor_id: u16, device_id: u16) -> Option<String> {
    let vhex = format!("{vendor_id:04x}");
    let dhex = format!("{device_id:04x}");
    let mut in_vendor = false;
    for line in ids.lines() {
        if line.starts_with('#') || line.trim().is_empty() {
            continue;
        }
        if !line.starts_with('\t') {
            // A vendor line. If we were already inside the target vendor, its block has ended.
            if in_vendor {
                break;
            }
            // `get(..4)` (not `[..4]`) so a line whose 4th byte is inside a multi-byte UTF-8 char
            // doesn't panic on a malformed pci.ids.
            in_vendor = line.get(..4).is_some_and(|p| p.eq_ignore_ascii_case(&vhex));
        } else if in_vendor && !line.starts_with("\t\t") {
            // A device line under the target vendor (skip the two-tab subsystem lines).
            let entry = line.trim_start();
            if entry
                .get(..4)
                .is_some_and(|p| p.eq_ignore_ascii_case(&dhex))
            {
                return Some(entry[4..].trim().to_string());
            }
        }
    }
    None
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

/// Read a sysfs file containing a hex value, with or without a `0x` prefix
/// (e.g. PCI `class`/`vendor`, or USB `idVendor`).
pub(crate) fn read_hex(path: &Path) -> Option<u32> {
    let raw = fs::read_to_string(path).ok()?;
    let trimmed = raw.trim();
    let hex = trimmed.strip_prefix("0x").unwrap_or(trimmed);
    u32::from_str_radix(hex, 16).ok()
}

#[cfg(test)]
mod tests {
    use super::lookup_model;

    // A trimmed pci.ids sample (tabs are significant): two vendors, devices, and a subsystem line.
    const IDS: &str = "\
# comment
1002  Advanced Micro Devices, Inc. [AMD/ATI]
\t744c  Navi 31 [Radeon RX 7900 XTX]
10de  NVIDIA Corporation
\t1e84  TU104 [GeForce RTX 2070 SUPER]
\t\t1462 3733  RTX 2070 SUPER GAMING
\t2482  GA104 [GeForce RTX 3070 Ti]
8086  Intel Corporation
";

    #[test]
    fn resolves_device_name_within_vendor() {
        assert_eq!(
            lookup_model(IDS, 0x10de, 0x1e84).as_deref(),
            Some("TU104 [GeForce RTX 2070 SUPER]")
        );
        assert_eq!(
            lookup_model(IDS, 0x10de, 0x2482).as_deref(),
            Some("GA104 [GeForce RTX 3070 Ti]")
        );
        assert_eq!(
            lookup_model(IDS, 0x1002, 0x744c).as_deref(),
            Some("Navi 31 [Radeon RX 7900 XTX]")
        );
    }

    #[test]
    fn unknown_device_or_vendor_is_none() {
        assert_eq!(lookup_model(IDS, 0x10de, 0xffff), None); // vendor present, device not
        assert_eq!(lookup_model(IDS, 0x1234, 0x1e84), None); // vendor absent
                                                             // A device id that only exists under a *different* vendor must not cross over.
        assert_eq!(lookup_model(IDS, 0x8086, 0x1e84), None);
    }
}

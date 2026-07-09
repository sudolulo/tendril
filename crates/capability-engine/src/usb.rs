//! USB enumeration for multi-seat setups: USB host controllers (for whole-controller passthrough to
//! a seat) and connected USB devices (for per-device passthrough of a seat's keyboard/mouse).

use std::fs;
use std::path::Path;

use crate::iommu::{self, IommuGroup, PassthroughViability};
use crate::sysfs::{read_hex, PCI_DEVICES};

const USB_DEVICES: &str = "/sys/bus/usb/devices";

/// A USB host controller (PCI class `0x0c03`) — a candidate for whole-controller passthrough,
/// giving a seat every port on that controller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsbController {
    pub address: String,
    pub vendor_id: u16,
    pub device_id: u16,
    /// The controller's IOMMU group (its passthrough unit); empty when there is no IOMMU.
    pub iommu_group: Vec<String>,
    pub viability: PassthroughViability,
}

/// A connected USB device — a candidate for per-device passthrough (e.g. a seat's keyboard/mouse).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsbDevice {
    /// sysfs port name, e.g. `1-1.3`.
    pub port: String,
    pub vendor_id: u16,
    pub product_id: u16,
    /// Product string, if the device reports one.
    pub product: Option<String>,
}

/// Enumerate USB host controllers on the live host.
pub fn controllers() -> Vec<UsbController> {
    controllers_from(Path::new(PCI_DEVICES), &iommu::read_groups())
}

/// Enumerate USB host controllers under an explicit sysfs `devices` dir, with the IOMMU groups.
pub fn controllers_from(devices_dir: &Path, groups: &[IommuGroup]) -> Vec<UsbController> {
    let Ok(entries) = fs::read_dir(devices_dir) else {
        return Vec::new();
    };
    let mut out: Vec<UsbController> = entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            // Class `0x0c03xx` == USB controller (the prog-if byte distinguishes UHCI/EHCI/XHCI).
            if read_hex(&path.join("class"))? >> 8 != 0x0c03 {
                return None;
            }
            let address = entry.file_name().to_string_lossy().into_owned();
            let viability = iommu::viability_for(&address, groups);
            let iommu_group = iommu::group_of(&address, groups)
                .map(|g| g.device_addresses.clone())
                .unwrap_or_default();
            Some(UsbController {
                vendor_id: read_hex(&path.join("vendor"))? as u16,
                device_id: read_hex(&path.join("device"))? as u16,
                address,
                iommu_group,
                viability,
            })
        })
        .collect();
    out.sort_by(|a, b| a.address.cmp(&b.address));
    out
}

/// Enumerate connected USB devices on the live host.
pub fn devices() -> Vec<UsbDevice> {
    devices_from(Path::new(USB_DEVICES))
}

/// Enumerate connected USB devices under an explicit `usb/devices` dir. Interfaces (which have no
/// `idVendor`) are skipped — only real devices are returned.
pub fn devices_from(usb_dir: &Path) -> Vec<UsbDevice> {
    let Ok(entries) = fs::read_dir(usb_dir) else {
        return Vec::new();
    };
    let mut out: Vec<UsbDevice> = entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            let vendor_id = read_hex(&path.join("idVendor"))? as u16;
            let product_id = read_hex(&path.join("idProduct"))? as u16;
            // Skip hubs (USB device class 0x09) — root hubs and external hubs aren't passthrough
            // targets, and their duplicate ids can't be attached by vendor:product alone.
            if read_hex(&path.join("bDeviceClass")).unwrap_or(0) == 0x09 {
                return None;
            }
            let product = fs::read_to_string(path.join("product"))
                .ok()
                .map(|s| s.trim().to_string());
            Some(UsbDevice {
                port: entry.file_name().to_string_lossy().into_owned(),
                vendor_id,
                product_id,
                product,
            })
        })
        .collect();
    out.sort_by(|a, b| a.port.cmp(&b.port));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn fixture(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name)
    }

    #[test]
    fn finds_usb_controllers() {
        let ctrls = controllers_from(&fixture("usb/pci"), &[]);
        assert_eq!(ctrls.len(), 1);
        assert_eq!(ctrls[0].address, "0000:00:1a.0");
        assert_eq!(ctrls[0].vendor_id, 0x8086);
        assert_eq!(ctrls[0].device_id, 0x1d2d);
        assert_eq!(ctrls[0].viability, PassthroughViability::NoIommu); // no groups passed
    }

    #[test]
    fn lists_usb_devices_skipping_interfaces() {
        let devs = devices_from(&fixture("usb/devices"));
        assert_eq!(devs.len(), 1); // the interface dir (no idVendor) is skipped
        assert_eq!(devs[0].vendor_id, 0x1d6b);
        assert_eq!(devs[0].product_id, 0x0002);
        assert_eq!(devs[0].product.as_deref(), Some("Test Hub"));
    }
}

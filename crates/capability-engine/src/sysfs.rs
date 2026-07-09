//! Shared sysfs constants and small file readers used by the enumeration modules.

use std::fs;
use std::path::Path;

/// Default sysfs path holding one directory per PCI device.
pub(crate) const PCI_DEVICES: &str = "/sys/bus/pci/devices";

/// Read a sysfs file containing a hex value, with or without a `0x` prefix
/// (e.g. PCI `class`/`vendor`, or USB `idVendor`).
pub(crate) fn read_hex(path: &Path) -> Option<u32> {
    let raw = fs::read_to_string(path).ok()?;
    let trimmed = raw.trim();
    let hex = trimmed.strip_prefix("0x").unwrap_or(trimmed);
    u32::from_str_radix(hex, 16).ok()
}

/// Read a sysfs file as a base-10 unsigned integer; 0 (and absent) both read as 0.
pub(crate) fn read_uint(path: &Path) -> u32 {
    fs::read_to_string(path)
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0)
}

/// Read a sysfs file, trimmed, dropping empty/missing to `None`.
pub(crate) fn read_trimmed(path: &Path) -> Option<String> {
    let s = fs::read_to_string(path).ok()?;
    let t = s.trim();
    (!t.is_empty()).then(|| t.to_string())
}

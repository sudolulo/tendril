//! vGPU host actions: create/remove **mediated devices** (mdev) and toggle **SR-IOV** virtual
//! functions. Detection lives in `capability-engine::vgpu` (pure sysfs read); this module performs
//! the host mutations the web layer drives.
//!
//! mdev instances are managed with **`mdevctl`** so they persist across reboots (`define --auto`
//! writes a config the systemd `mdev@.service` re-creates on boot). SR-IOV VFs are enabled by writing
//! `sriov_numvfs`; the resulting VFs show up as ordinary PCI GPUs and flow through the existing
//! whole-GPU passthrough picker, so nothing else here is needed for them.

use crate::ui;

/// Encoded wizard selection for a vGPU profile: `mdev:<parent-pci>:<type-id>`.
pub const MDEV_PREFIX: &str = "mdev:";

/// A fresh UUID from the kernel (`/proc/sys/kernel/random/uuid`).
fn new_uuid() -> Option<String> {
    std::fs::read_to_string("/proc/sys/kernel/random/uuid")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Create a mediated device of `type_id` on `parent` (a PCI address) and return its UUID.
///
/// Uses `mdevctl define --auto` + `start` so the vGPU is both live now and recreated on boot. Returns
/// an error string suitable for surfacing in the UI.
pub fn create_mdev(parent: &str, type_id: &str) -> Result<String, String> {
    let uuid = new_uuid().ok_or("couldn't allocate a UUID for the vGPU")?;
    // Persist (auto-start on boot) then start now. `define` writes the config; `start` instantiates.
    ui::run_result(
        "mdevctl",
        &[
            "define", "--uuid", &uuid, "--parent", parent, "--type", type_id, "--auto",
        ],
    )
    .map_err(|e| format!("mdevctl define failed: {e}"))?;
    if let Err(e) = ui::run_result("mdevctl", &["start", "--uuid", &uuid]) {
        // Roll back the definition so we don't leave a half-created, unstarted vGPU behind.
        let _ = ui::run_result("mdevctl", &["undefine", "--uuid", &uuid]);
        return Err(format!("mdevctl start failed: {e}"));
    }
    Ok(uuid)
}

/// Tear down a mediated device by UUID (best-effort: stop the live instance, drop its definition).
pub fn remove_mdev(uuid: &str) {
    let _ = ui::run_result("mdevctl", &["stop", "--uuid", uuid]);
    let _ = ui::run_result("mdevctl", &["undefine", "--uuid", uuid]);
}

/// Map an mdev UUID to its parent PCI address (for "used by" on the hardware page).
/// Reads the sysfs symlink `/sys/bus/mdev/devices/<uuid>` which points under the parent PCI device.
pub fn mdev_parent(uuid: &str) -> Option<String> {
    let link = std::fs::read_link(format!("/sys/bus/mdev/devices/{uuid}")).ok()?;
    // …/devices/pci0000:00/0000:00:03.1/0000:01:00.0/<uuid> → the parent is the last PCI-address
    // component before the uuid.
    link.components()
        .filter_map(|c| c.as_os_str().to_str())
        .filter(|s| is_pci_address(s))
        .next_back()
        .map(String::from)
}

fn is_pci_address(s: &str) -> bool {
    // dddd:dd:dd.d
    let b = s.as_bytes();
    s.len() == 12
        && b[4] == b':'
        && b[7] == b':'
        && b[10] == b'.'
        && s.chars()
            .enumerate()
            .all(|(i, c)| matches!(i, 4 | 7 | 10) || c.is_ascii_hexdigit())
}

/// Enable (or change) SR-IOV virtual functions on `parent`. Writing a new count requires zeroing
/// first, so we always reset to 0 before setting `n`.
pub fn set_sriov_numvfs(parent: &str, n: u32) -> Result<(), String> {
    let path = format!("/sys/bus/pci/devices/{parent}/sriov_numvfs");
    std::fs::write(&path, "0").map_err(|e| format!("reset VFs: {e}"))?;
    if n > 0 {
        std::fs::write(&path, n.to_string()).map_err(|e| format!("set {n} VFs: {e}"))?;
    }
    Ok(())
}

/// Parse a wizard GPU selection into an mdev `(parent, type_id)` if it encodes a vGPU profile.
/// The value is `mdev:<12-char PCI address>:<type-id>`, so the parent is a fixed-width slice.
pub fn parse_mdev_selection(sel: &str) -> Option<(String, String)> {
    let rest = sel.strip_prefix(MDEV_PREFIX)?;
    let (parent, rest) = rest.split_at_checked(12)?;
    let type_id = rest.strip_prefix(':')?;
    (is_pci_address(parent) && !type_id.is_empty())
        .then(|| (parent.to_string(), type_id.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_pci_addresses() {
        assert!(is_pci_address("0000:01:00.0"));
        assert!(is_pci_address("0000:83:00.1"));
        assert!(!is_pci_address("not-a-pci"));
        assert!(!is_pci_address("0000:01:00")); // no function
    }

    #[test]
    fn parses_mdev_selection() {
        assert_eq!(
            parse_mdev_selection("mdev:0000:01:00.0:nvidia-256"),
            Some(("0000:01:00.0".to_string(), "nvidia-256".to_string()))
        );
        assert_eq!(
            parse_mdev_selection("mdev:0000:83:00.0:i915-GVTg_V5_4"),
            Some(("0000:83:00.0".to_string(), "i915-GVTg_V5_4".to_string()))
        );
        assert_eq!(parse_mdev_selection("0000:01:00.0"), None); // plain passthrough
        assert_eq!(parse_mdev_selection(""), None);
    }
}

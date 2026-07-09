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

pub fn is_pci_address(s: &str) -> bool {
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

/// The PCI addresses of a GPU's current SR-IOV virtual functions (`virtfn0`, `virtfn1`, … symlinks).
pub fn sriov_vfs(parent: &str) -> Vec<String> {
    let base = format!("/sys/bus/pci/devices/{parent}");
    let mut out = Vec::new();
    for i in 0..256 {
        match std::fs::read_link(format!("{base}/virtfn{i}")) {
            Ok(p) => {
                if let Some(n) = p.file_name().and_then(|s| s.to_str()) {
                    out.push(n.to_string());
                }
            }
            Err(_) => break, // virtfn indices are contiguous from 0
        }
    }
    out
}

/// Enable (or change) SR-IOV virtual functions on `parent`. Writing a new count requires zeroing
/// first, so we always reset to 0 before setting `n` — which means **any** change tears down the
/// current VFs (see the in-use guard in the handler).
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

/// Parse an NVIDIA vGPU profile's framebuffer (GB) and series letter from its name, e.g.
/// `"GRID RTX6000-4Q"` → `(4, 'Q')`. `None` when the name doesn't follow the `…-<GB><letter>` form.
fn profile_fb_gb(name: &str) -> Option<(u32, char)> {
    let tail = name.rsplit('-').next()?.trim(); // "4Q"
    let letter = tail.chars().last().filter(|c| c.is_ascii_alphabetic())?;
    let digits: String = tail.chars().take_while(|c| c.is_ascii_digit()).collect();
    let gb: u32 = digits.parse().ok()?;
    (gb > 0).then_some((gb, letter))
}

/// The **recommended** profile for a gaming station among a GPU's mdev types (its index), encoding
/// Tendril's default guidance: real games need ≥4 GB of vGPU framebuffer, and within that you want the
/// smallest such profile so the card yields the most stations — preferring the **Q** (workstation /
/// full-graphics) series. Falls back to the largest available profile when none reach 4 GB, then to the
/// first usable one. `None` if no profile has capacity.
///
/// e.g. an 8 GB card offering 1/2/4/8 GB Q-profiles → the 4 GB one (two gaming-capable stations).
pub fn recommended_mdev(types: &[tendril_capability_engine::MdevType]) -> Option<usize> {
    types
        .iter()
        .enumerate()
        .filter(|(_, t)| t.available > 0)
        .max_by_key(|(_, t)| match t.name.as_deref().and_then(profile_fb_gb) {
            // ≥4 GB: prefer the *smallest* (more stations) → rank descends with size; +Q bonus.
            Some((gb, letter)) if gb >= 4 => {
                1000 - gb as i32
                    + if letter.eq_ignore_ascii_case(&'Q') {
                        100
                    } else {
                        0
                    }
            }
            // <4 GB: too small for AAA, so pick the *largest* of these; +Q bonus.
            Some((gb, letter)) => {
                gb as i32
                    + if letter.eq_ignore_ascii_case(&'Q') {
                        100
                    } else {
                        0
                    }
            }
            // Unknown framebuffer: usable but unranked.
            None => 1,
        })
        .map(|(i, _)| i)
}

/// The single default vGPU selection key for the station wizard: the recommended profile of the first
/// vGPU-capable GPU that isn't already passed through whole. `None` when no vGPU profile is available.
pub fn default_mdev_key(
    matrix: &tendril_capability_engine::CapabilityMatrix,
    whole_used: &std::collections::HashMap<String, String>,
) -> Option<String> {
    matrix
        .vgpu_capable()
        .filter(|g| !whole_used.contains_key(&g.gpu.address))
        .find_map(|g| {
            recommended_mdev(&g.vgpu.mdev_types).map(|i| {
                format!(
                    "{}{}:{}",
                    MDEV_PREFIX, g.gpu.address, g.vgpu.mdev_types[i].id
                )
            })
        })
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

    #[test]
    fn parses_profile_framebuffer() {
        assert_eq!(profile_fb_gb("GRID RTX6000-4Q"), Some((4, 'Q')));
        assert_eq!(profile_fb_gb("GRID RTX6000-8Q"), Some((8, 'Q')));
        assert_eq!(profile_fb_gb("NVIDIA RTXA6000-2B"), Some((2, 'B')));
        assert_eq!(profile_fb_gb("some-weird-name"), None);
    }

    fn mdev(id: &str, name: &str, available: u32) -> tendril_capability_engine::MdevType {
        tendril_capability_engine::MdevType {
            id: id.to_string(),
            name: Some(name.to_string()),
            description: None,
            available,
            device_api: None,
        }
    }

    #[test]
    fn recommends_gaming_profile() {
        // 8 GB card, Q-series 1/2/4/8 GB → 4 GB (real gaming VRAM, two stations).
        let types = vec![
            mdev("n-1", "GRID RTX6000-1Q", 8),
            mdev("n-2", "GRID RTX6000-2Q", 4),
            mdev("n-4", "GRID RTX6000-4Q", 2),
            mdev("n-8", "GRID RTX6000-8Q", 1),
        ];
        assert_eq!(recommended_mdev(&types), Some(2)); // the 4Q profile

        // None ≥4 GB → pick the largest available small one (2 GB).
        let small = vec![mdev("a", "GRID X-1Q", 4), mdev("b", "GRID X-2Q", 2)];
        assert_eq!(recommended_mdev(&small), Some(1));

        // Skip profiles with no capacity.
        let full = vec![mdev("a", "GRID X-4Q", 0), mdev("b", "GRID X-2Q", 3)];
        assert_eq!(recommended_mdev(&full), Some(1));

        assert_eq!(recommended_mdev(&[]), None);
    }
}

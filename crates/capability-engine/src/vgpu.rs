//! vGPU capability probing: **mediated devices (mdev)** and **SR-IOV** virtual functions.
//!
//! A GPU can be split across several stations two ways, each with its own libvirt wiring:
//!
//! - **mdev** — the host driver advertises profiles under
//!   `/sys/bus/pci/devices/<addr>/mdev_supported_types/<type>/`. You create an instance (write a UUID
//!   to the type's `create` file) and attach it as `<hostdev type='mdev'>`. This is the NVIDIA vGPU /
//!   `vgpu_unlock` / Intel GVT-g path — the realistic "one gaming GPU, many stations".
//! - **SR-IOV** — the GPU advertises `sriov_totalvfs`; writing `sriov_numvfs` spawns virtual functions
//!   that appear as ordinary PCI devices and are passed through with the existing whole-GPU path. This
//!   is the AMD MxGPU / Intel Data Center GPU route (datacenter silicon, not consumer gaming cards).
//!
//! Both are probed straight from sysfs; nothing here mutates the host (that's the provisioning layer).

use std::fs;
use std::path::Path;

/// Default sysfs path holding one directory per PCI device.
const PCI_DEVICES: &str = "/sys/bus/pci/devices";

/// One mdev profile a GPU can hand out (e.g. `nvidia-256` → "GRID RTX6000-4Q").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MdevType {
    /// The type id — the sysfs directory name, e.g. `nvidia-256` or `i915-GVTg_V5_4`.
    pub id: String,
    /// Friendly profile name from the `name` file, if present.
    pub name: Option<String>,
    /// Longer description from the `description` file, if present.
    pub description: Option<String>,
    /// How many more instances of this type can currently be created (`available_instances`).
    pub available: u32,
    /// The device API the profile presents (`device_api`), typically `vfio-pci`.
    pub device_api: Option<String>,
}

/// What vGPU mechanisms a single GPU supports right now.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VgpuSupport {
    /// mdev profiles the GPU advertises (empty when mdev isn't available).
    pub mdev_types: Vec<MdevType>,
    /// Maximum SR-IOV virtual functions (`sriov_totalvfs`); 0 when not SR-IOV capable.
    pub sriov_totalvfs: u32,
    /// SR-IOV virtual functions currently enabled (`sriov_numvfs`).
    pub sriov_numvfs: u32,
}

impl VgpuSupport {
    /// True if the GPU can be split by *either* mechanism.
    pub fn is_capable(&self) -> bool {
        !self.mdev_types.is_empty() || self.sriov_totalvfs > 0
    }
    /// True if any mdev profile still has capacity to hand out.
    pub fn mdev_available(&self) -> bool {
        self.mdev_types.iter().any(|t| t.available > 0)
    }
}

/// Probe a GPU's vGPU support on the live host (reads `/sys`).
pub fn probe(address: &str) -> VgpuSupport {
    probe_from(Path::new(PCI_DEVICES), address)
}

/// Probe under an explicit sysfs `devices` directory (so the logic is unit-testable).
pub fn probe_from(devices_dir: &Path, address: &str) -> VgpuSupport {
    let dev = devices_dir.join(address);
    VgpuSupport {
        mdev_types: read_mdev_types(&dev.join("mdev_supported_types")),
        sriov_totalvfs: read_uint(&dev.join("sriov_totalvfs")),
        sriov_numvfs: read_uint(&dev.join("sriov_numvfs")),
    }
}

/// Enumerate the mdev profile directories under `<dev>/mdev_supported_types`.
fn read_mdev_types(dir: &Path) -> Vec<MdevType> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut types: Vec<MdevType> = entries
        .flatten()
        .filter(|e| e.path().is_dir())
        .map(|e| {
            let p = e.path();
            MdevType {
                id: e.file_name().to_string_lossy().into_owned(),
                name: read_trimmed(&p.join("name")),
                description: read_trimmed(&p.join("description")),
                available: read_uint(&p.join("available_instances")),
                device_api: read_trimmed(&p.join("device_api")),
            }
        })
        .collect();
    types.sort_by(|a, b| a.id.cmp(&b.id));
    types
}

/// Read a sysfs file as a base-10 unsigned integer; 0 (and absent) both read as 0.
fn read_uint(path: &Path) -> u32 {
    fs::read_to_string(path)
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0)
}

/// Read a sysfs file, trimmed, dropping empty/missing to `None`.
fn read_trimmed(path: &Path) -> Option<String> {
    let s = fs::read_to_string(path).ok()?;
    let t = s.trim();
    (!t.is_empty()).then(|| t.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write(p: &Path, s: &str) {
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, s).unwrap();
    }

    #[test]
    fn probes_mdev_types_sorted_with_metadata() {
        let tmp = std::env::temp_dir().join(format!("tendril-vgpu-mdev-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        let addr = "0000:01:00.0";
        let base = tmp.join(addr).join("mdev_supported_types");
        write(&base.join("nvidia-256").join("name"), "GRID RTX6000-4Q\n");
        write(&base.join("nvidia-256").join("available_instances"), "2\n");
        write(&base.join("nvidia-256").join("device_api"), "vfio-pci\n");
        write(&base.join("nvidia-255").join("name"), "GRID RTX6000-8Q\n");
        write(&base.join("nvidia-255").join("available_instances"), "0\n");

        let s = probe_from(&tmp, addr);
        assert_eq!(s.mdev_types.len(), 2);
        // sorted by id → nvidia-255 first
        assert_eq!(s.mdev_types[0].id, "nvidia-255");
        assert_eq!(s.mdev_types[1].id, "nvidia-256");
        assert_eq!(s.mdev_types[1].name.as_deref(), Some("GRID RTX6000-4Q"));
        assert_eq!(s.mdev_types[1].available, 2);
        assert!(s.is_capable());
        assert!(s.mdev_available()); // nvidia-256 has 2 free
        assert_eq!(s.sriov_totalvfs, 0);
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn probes_sriov_counts() {
        let tmp = std::env::temp_dir().join(format!("tendril-vgpu-sriov-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        let addr = "0000:03:00.0";
        write(&tmp.join(addr).join("sriov_totalvfs"), "16\n");
        write(&tmp.join(addr).join("sriov_numvfs"), "4\n");
        let s = probe_from(&tmp, addr);
        assert_eq!(s.sriov_totalvfs, 16);
        assert_eq!(s.sriov_numvfs, 4);
        assert!(s.is_capable());
        assert!(s.mdev_types.is_empty());
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn absent_paths_are_not_capable() {
        let tmp = std::env::temp_dir().join(format!("tendril-vgpu-none-{}", std::process::id()));
        let s = probe_from(&tmp, "0000:09:00.0");
        assert!(!s.is_capable());
    }
}

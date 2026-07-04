//! IOMMU group analysis — the gate for safe GPU passthrough.

use std::fs;
use std::path::Path;

use crate::pci::GpuDevice;

/// Default sysfs path holding one directory per IOMMU group.
const IOMMU_GROUPS: &str = "/sys/kernel/iommu_groups";

/// An IOMMU group and the device addresses bound within it.
#[derive(Debug, Clone)]
pub struct IommuGroup {
    pub id: u32,
    pub device_addresses: Vec<String>,
}

/// Whether a GPU can be safely passed through, based on its IOMMU grouping.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PassthroughViability {
    /// GPU sits alone (only its own functions) in its group — clean passthrough.
    Isolated,
    /// GPU shares a group with unrelated devices — needs ACS override (security caveat).
    SharedGroup,
    /// IOMMU disabled/unavailable, or the GPU is in no group — impossible until enabled in firmware.
    NoIommu,
}

/// Read all IOMMU groups from the live host.
pub fn read_groups() -> Vec<IommuGroup> {
    read_groups_from(Path::new(IOMMU_GROUPS))
}

/// Read IOMMU groups under an explicit `iommu_groups` directory.
pub fn read_groups_from(groups_dir: &Path) -> Vec<IommuGroup> {
    let Ok(entries) = fs::read_dir(groups_dir) else {
        return Vec::new();
    };

    let mut groups: Vec<IommuGroup> = entries
        .flatten()
        .filter_map(|entry| {
            let id = entry.file_name().to_string_lossy().parse::<u32>().ok()?;
            let mut device_addresses: Vec<String> = fs::read_dir(entry.path().join("devices"))
                .into_iter()
                .flatten()
                .flatten()
                .map(|d| d.file_name().to_string_lossy().into_owned())
                .collect();
            device_addresses.sort();
            Some(IommuGroup {
                id,
                device_addresses,
            })
        })
        .collect();

    groups.sort_by_key(|g| g.id);
    groups
}

/// Assess passthrough viability for a GPU given the discovered IOMMU groups.
///
/// A group is [`PassthroughViability::Isolated`] when every device in it belongs to the GPU's own
/// PCI slot (its functions, e.g. `0000:83:00.{0,1,2,3}`). Any foreign device makes it a
/// [`PassthroughViability::SharedGroup`] (passthrough still possible, but only with an ACS override
/// and its security caveat). No group / no IOMMU is [`PassthroughViability::NoIommu`].
///
/// TODO(phase-1+): treat PCIe root ports/bridges in the group specially rather than as foreign.
pub fn assess(gpu: &GpuDevice, groups: &[IommuGroup]) -> PassthroughViability {
    let Some(group) = groups
        .iter()
        .find(|g| g.device_addresses.iter().any(|a| a == &gpu.address))
    else {
        return PassthroughViability::NoIommu;
    };

    let gpu_slot = slot_of(&gpu.address);
    let isolated = group
        .device_addresses
        .iter()
        .all(|addr| slot_of(addr) == gpu_slot);

    if isolated {
        PassthroughViability::Isolated
    } else {
        PassthroughViability::SharedGroup
    }
}

/// Drop the PCI function from an address: `0000:83:00.1` -> `0000:83:00`.
fn slot_of(address: &str) -> &str {
    address.rsplit_once('.').map_or(address, |(slot, _fn)| slot)
}

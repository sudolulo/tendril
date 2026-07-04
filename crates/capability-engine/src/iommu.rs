//! IOMMU group analysis — the gate for safe GPU passthrough.

use crate::pci::GpuDevice;

/// An IOMMU group and the device addresses bound within it.
#[derive(Debug, Clone)]
pub struct IommuGroup {
    pub id: u32,
    pub device_addresses: Vec<String>,
}

/// Whether a GPU can be safely passed through, based on its IOMMU grouping.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PassthroughViability {
    /// GPU sits alone (with its own functions) in its group — clean passthrough.
    Isolated,
    /// GPU shares a group with unrelated devices — needs ACS override (security caveat).
    SharedGroup,
    /// IOMMU disabled or unavailable — impossible until enabled in firmware.
    NoIommu,
}

/// Assess passthrough viability for a GPU given the discovered IOMMU groups.
///
/// TODO(phase-1): read `/sys/kernel/iommu_groups/*` and correlate with the GPU address.
pub fn assess(_gpu: &GpuDevice, _groups: &[IommuGroup]) -> PassthroughViability {
    PassthroughViability::NoIommu
}

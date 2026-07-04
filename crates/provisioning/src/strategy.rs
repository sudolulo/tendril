//! The provisioning strategy trait.

use tendril_capability_engine::{GpuDevice, IommuGroup};

/// The host changes required to provision a GPU for a usage mode. Pure data; rendered into bootc
/// image layers by a later step.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProvisioningPlan {
    /// Host driver every listed device is bound to (e.g. `vfio-pci`).
    pub driver: String,
    /// PCI addresses to bind — the GPU plus every other function in its IOMMU group.
    pub bind_addresses: Vec<String>,
    /// Human-readable summary of what will change.
    pub summary: String,
    /// A caveat about this plan, if any (e.g. IOMMU disabled).
    pub note: Option<String>,
}

/// A strategy that provisions the host for a particular GPU usage mode.
pub trait ProvisioningStrategy {
    /// Stable identifier, e.g. `"passthrough"`.
    fn name(&self) -> &'static str;

    /// Compute the host changes needed to provision `gpu`, given its IOMMU `group` (if any).
    /// Pure — performs no mutation.
    fn plan(&self, gpu: &GpuDevice, group: Option<&IommuGroup>) -> ProvisioningPlan;
}

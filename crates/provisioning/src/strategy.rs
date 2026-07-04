//! The provisioning strategy trait.

use tendril_capability_engine::GpuDevice;

/// The host changes required to provision a GPU. Rendered into bootc image layers; pure data.
#[derive(Debug, Clone, Default)]
pub struct ProvisioningPlan {
    /// Kernel module to bind the device to (e.g. `vfio-pci`).
    pub driver: Option<String>,
    /// Kernel command-line additions (e.g. `vfio-pci.ids=10de:xxxx`).
    pub kernel_cmdline: Vec<String>,
    /// Human-readable summary of what will change.
    pub summary: String,
}

/// A strategy that provisions the host for a particular GPU usage mode.
pub trait ProvisioningStrategy {
    /// Stable identifier, e.g. `"passthrough"`.
    fn name(&self) -> &'static str;

    /// Compute the host changes needed to provision `gpu`. Pure — performs no mutation.
    fn plan(&self, gpu: &GpuDevice) -> ProvisioningPlan;
}

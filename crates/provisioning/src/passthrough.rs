//! VFIO full-GPU passthrough — the reliable default path (1 GPU -> 1 VM).

use crate::strategy::{ProvisioningPlan, ProvisioningStrategy};
use tendril_capability_engine::GpuDevice;

/// Binds a GPU to `vfio-pci` for exclusive passthrough to a single VM.
#[derive(Debug, Default)]
pub struct PassthroughStrategy;

impl ProvisioningStrategy for PassthroughStrategy {
    fn name(&self) -> &'static str {
        "passthrough"
    }

    fn plan(&self, gpu: &GpuDevice) -> ProvisioningPlan {
        ProvisioningPlan {
            driver: Some("vfio-pci".to_string()),
            kernel_cmdline: vec![format!(
                "vfio-pci.ids={:04x}:{:04x}",
                gpu.vendor_id, gpu.device_id
            )],
            summary: format!("Bind {} to vfio-pci for passthrough", gpu.address),
        }
    }
}

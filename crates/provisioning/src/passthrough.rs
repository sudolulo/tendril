//! VFIO full-GPU passthrough — the reliable default path (1 GPU -> 1 VM).

use crate::strategy::{ProvisioningPlan, ProvisioningStrategy};
use tendril_capability_engine::{GpuDevice, IommuGroup};

/// Binds a GPU — and every other function in its IOMMU group — to `vfio-pci` for exclusive
/// passthrough to a single VM.
#[derive(Debug, Default)]
pub struct PassthroughStrategy;

impl ProvisioningStrategy for PassthroughStrategy {
    fn name(&self) -> &'static str {
        "passthrough"
    }

    fn plan(&self, gpu: &GpuDevice, group: Option<&IommuGroup>) -> ProvisioningPlan {
        // A GPU's audio/USB companion functions share its IOMMU group and must be bound with it —
        // the IOMMU group is the smallest unit that can be passed through.
        let (bind_addresses, note) = match group {
            Some(g) if !g.device_addresses.is_empty() => (g.device_addresses.clone(), None),
            _ => (
                vec![gpu.address.clone()],
                Some(format!(
                    "No IOMMU group found for {}; binding the GPU function only (is IOMMU enabled?).",
                    gpu.address
                )),
            ),
        };

        ProvisioningPlan {
            summary: format!(
                "Bind {} device(s) in {}'s IOMMU group to vfio-pci for passthrough",
                bind_addresses.len(),
                gpu.address
            ),
            driver: "vfio-pci".to_string(),
            bind_addresses,
            note,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tendril_capability_engine::GpuVendor;

    fn nvidia_gpu() -> GpuDevice {
        GpuDevice {
            address: "0000:83:00.0".to_string(),
            vendor_id: 0x10de,
            device_id: 0x1e84,
            vendor: GpuVendor::Nvidia,
            model: None,
            boot_vga: false,
        }
    }

    #[test]
    fn binds_every_function_in_the_group() {
        let group = IommuGroup {
            id: 13,
            device_addresses: vec![
                "0000:83:00.0".to_string(),
                "0000:83:00.1".to_string(),
                "0000:83:00.2".to_string(),
                "0000:83:00.3".to_string(),
            ],
        };
        let plan = PassthroughStrategy.plan(&nvidia_gpu(), Some(&group));
        assert_eq!(plan.driver, "vfio-pci");
        assert_eq!(plan.bind_addresses.len(), 4);
        assert!(plan.bind_addresses.contains(&"0000:83:00.2".to_string()));
        assert!(plan.note.is_none());
    }

    #[test]
    fn without_a_group_binds_gpu_only_and_warns() {
        let plan = PassthroughStrategy.plan(&nvidia_gpu(), None);
        assert_eq!(plan.bind_addresses, vec!["0000:83:00.0".to_string()]);
        assert!(plan.note.is_some());
    }
}

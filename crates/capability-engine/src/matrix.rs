//! Capability matrix: maps each GPU to what Tendril can do with it.

use crate::iommu::{self, IommuGroup, PassthroughViability};
use crate::pci::{GpuDevice, GpuVendor};

/// What a given GPU supports on this host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Capability {
    /// Full GPU passthrough (1 GPU -> 1 VM). The reliable default.
    Passthrough,
    /// Officially supported vGPU splitting (datacenter NVIDIA / AMD MxGPU).
    VgpuOfficial,
    /// Experimental consumer vGPU via `vgpu_unlock`.
    VgpuUnlock,
    /// Reserved for the host (console / no passthrough).
    HostOnly,
}

/// A GPU paired with its assessed capability and passthrough viability.
#[derive(Debug, Clone)]
pub struct GpuCapability {
    pub gpu: GpuDevice,
    pub capability: Capability,
    /// Detail behind the capability (e.g. whether an ACS override is needed).
    pub viability: PassthroughViability,
}

/// The full set of GPU capabilities for the host.
#[derive(Debug, Clone, Default)]
pub struct CapabilityMatrix {
    pub gpus: Vec<GpuCapability>,
}

impl CapabilityMatrix {
    /// GPUs that can serve as an independent gaming station via passthrough.
    pub fn passthrough_capable(&self) -> impl Iterator<Item = &GpuCapability> {
        self.gpus
            .iter()
            .filter(|g| matches!(g.capability, Capability::Passthrough))
    }
}

/// Build a capability matrix from enumerated GPUs and IOMMU groups.
pub fn build(gpus: Vec<GpuDevice>, groups: &[IommuGroup]) -> CapabilityMatrix {
    let gpus = gpus
        .into_iter()
        .map(|gpu| {
            let viability = iommu::assess(&gpu, groups);
            let capability = classify(&gpu, viability);
            GpuCapability {
                gpu,
                capability,
                viability,
            }
        })
        .collect();
    CapabilityMatrix { gpus }
}

/// Classify a single GPU into a [`Capability`].
///
/// Non-GPU-vendor display devices (e.g. a Matrox/ASPEED BMC console, or an unrecognized adapter) are
/// treated as [`Capability::HostOnly`]. Recognized GPUs are passthrough-capable when the IOMMU
/// permits it; without IOMMU they fall back to host-only.
///
/// TODO(phase-1+): vGPU classification (official vs `vgpu_unlock`) and host-GPU reservation policy
/// (which display device the host keeps) belong here / one layer up.
fn classify(gpu: &GpuDevice, viability: PassthroughViability) -> Capability {
    if gpu.vendor == GpuVendor::Unknown {
        return Capability::HostOnly;
    }
    match viability {
        PassthroughViability::Isolated | PassthroughViability::SharedGroup => {
            Capability::Passthrough
        }
        PassthroughViability::NoIommu => Capability::HostOnly,
    }
}

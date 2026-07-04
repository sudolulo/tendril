//! Capability matrix: maps each GPU to what Tendril can do with it.

use crate::pci::GpuDevice;

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

/// A GPU paired with its assessed capability.
#[derive(Debug, Clone)]
pub struct GpuCapability {
    pub gpu: GpuDevice,
    pub capability: Capability,
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

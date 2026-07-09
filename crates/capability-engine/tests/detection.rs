//! Detection tests against checked-in fixture sysfs trees under `tests/fixtures/`.

use std::path::{Path, PathBuf};

use tendril_capability_engine::{
    iommu, matrix, pci, Capability, GpuVendor, PassthroughViability, VgpuSupport,
};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

#[test]
fn enumerates_only_display_devices() {
    let gpus = pci::enumerate_from(&fixture("pci"));
    // NVIDIA dGPU + Intel iGPU are display-class; the audio function is not.
    assert_eq!(gpus.len(), 2, "expected 2 display devices, got {gpus:?}");

    let nvidia = gpus
        .iter()
        .find(|g| g.vendor == GpuVendor::Nvidia)
        .expect("nvidia GPU present");
    assert_eq!(nvidia.address, "0000:83:00.0");
    assert_eq!(nvidia.device_id, 0x1e84);

    assert!(gpus.iter().any(|g| g.vendor == GpuVendor::Intel));
}

#[test]
fn nvidia_in_own_group_is_isolated_passthrough() {
    let gpus = pci::enumerate_from(&fixture("pci"));
    let groups = iommu::read_groups_from(&fixture("iommu_isolated"));
    let nvidia = gpus.iter().find(|g| g.vendor == GpuVendor::Nvidia).unwrap();

    assert_eq!(
        iommu::assess(nvidia, &groups),
        PassthroughViability::Isolated
    );

    let m = matrix::build_with(gpus, &groups, |_| VgpuSupport::default());
    let nv = m
        .gpus
        .iter()
        .find(|c| c.gpu.vendor == GpuVendor::Nvidia)
        .unwrap();
    assert_eq!(nv.capability, Capability::Passthrough);
    assert_eq!(m.passthrough_capable().count(), 2); // nvidia + intel, both isolated
}

#[test]
fn shared_group_needs_acs() {
    let gpus = pci::enumerate_from(&fixture("pci"));
    let groups = iommu::read_groups_from(&fixture("iommu_shared"));
    let nvidia = gpus.iter().find(|g| g.vendor == GpuVendor::Nvidia).unwrap();

    assert_eq!(
        iommu::assess(nvidia, &groups),
        PassthroughViability::SharedGroup
    );
}

#[test]
fn no_iommu_means_host_only() {
    let gpus = pci::enumerate_from(&fixture("pci"));
    let groups: Vec<iommu::IommuGroup> = Vec::new();
    let nvidia = gpus.iter().find(|g| g.vendor == GpuVendor::Nvidia).unwrap();

    assert_eq!(
        iommu::assess(nvidia, &groups),
        PassthroughViability::NoIommu
    );

    let m = matrix::build_with(gpus, &groups, |_| VgpuSupport::default());
    let nv = m
        .gpus
        .iter()
        .find(|c| c.gpu.vendor == GpuVendor::Nvidia)
        .unwrap();
    assert_eq!(nv.capability, Capability::HostOnly);
}

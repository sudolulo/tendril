//! `tendril-domain` — render a libvirt domain for the first passthrough-capable GPU.
//!
//! Demonstrates the pipeline end to end: detect → plan → domain XML. Prints XML only; it does not
//! define or start anything with libvirt.

use tendril_capability_engine::{iommu, matrix, pci};
use tendril_orchestrator::domain::{render, DomainSpec};
use tendril_orchestrator::{GuestOs, InstallMedia, StationSpec};
use tendril_provisioning::{PassthroughStrategy, ProvisioningStrategy};

fn main() {
    let gpus = pci::enumerate();
    let groups = iommu::read_groups();
    let matrix = matrix::build(gpus, &groups);

    let Some(cap) = matrix.passthrough_capable().next() else {
        println!("No passthrough-capable GPU to build a station for.");
        return;
    };

    let group = groups
        .iter()
        .find(|g| g.device_addresses.iter().any(|a| a == &cap.gpu.address));
    let plan = PassthroughStrategy.plan(&cap.gpu, group);

    let station = StationSpec {
        name: "station1".to_string(),
        guest: GuestOs::Windows,
        gpu_address: cap.gpu.address.clone(),
        native_hardware: false,
    };
    let spec = DomainSpec {
        station: &station,
        vcpus: 8,
        memory_mib: 16384,
        disk_path: "/var/lib/tendril/station1.qcow2".to_string(),
        passthrough_addresses: plan.bind_addresses,
        mdev_uuid: None,
        media: InstallMedia::none(),
        usb_devices: Vec::new(),
    };

    print!("{}", render(&spec));
}

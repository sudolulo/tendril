//! `tendril-vm` — render a station's domain and (optionally) define it with libvirt.
//!
//! Dry-run by default: prints the domain XML. `--define` registers it with libvirt (validated, not
//! started). Starting is left to a human — that's when the GPU is detached from the host.

use tendril_capability_engine::{iommu, matrix, pci};
use tendril_orchestrator::domain::{render, DomainSpec};
use tendril_orchestrator::{GuestOs, InstallMedia, Libvirt, StationSpec};
use tendril_provisioning::{PassthroughStrategy, ProvisioningStrategy};

fn main() {
    let define = std::env::args().any(|a| a == "--define");

    let gpus = pci::enumerate();
    let groups = iommu::read_groups();
    let matrix = matrix::build(gpus, &groups);

    let Some(cap) = matrix.passthrough_capable().next() else {
        eprintln!("No passthrough-capable GPU to build a station for.");
        std::process::exit(1);
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
        media: InstallMedia::none(),
    };
    let xml = render(&spec);

    if !define {
        print!("{xml}");
        eprintln!(
            "(dry-run — pass --define to register the domain with libvirt; it will not be started)"
        );
        return;
    }

    match Libvirt::system().define(&station.name, &xml) {
        Ok(()) => println!("defined domain '{}' (not started)", station.name),
        Err(e) => {
            eprintln!("define failed: {e}");
            std::process::exit(1);
        }
    }
}

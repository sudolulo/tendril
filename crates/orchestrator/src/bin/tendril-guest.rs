//! `tendril-guest` — create a station's disk and render its OS-install domain.
//!
//! `--create-disk` makes the qcow2 (default 128 GiB). `--iso <path>` / `--virtio-iso <path>` attach
//! install media (the domain then boots from the ISO). `--steamos` selects SteamOS (default Windows).
//! Prints the install domain XML; it does not start anything.

use std::path::Path;

use tendril_capability_engine::{iommu, matrix, pci};
use tendril_orchestrator::domain::{render, DomainSpec};
use tendril_orchestrator::guest::create_disk;
use tendril_orchestrator::{GuestOs, InstallMedia, StationSpec};
use tendril_provisioning::{PassthroughStrategy, ProvisioningStrategy};

fn arg_value(flag: &str) -> Option<String> {
    let args: Vec<String> = std::env::args().collect();
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1).cloned())
}

fn has_flag(flag: &str) -> bool {
    std::env::args().any(|a| a == flag)
}

fn main() {
    let disk = arg_value("--disk").unwrap_or_else(|| "/var/lib/tendril/station1.qcow2".to_string());
    let size_gib: u32 = arg_value("--size-gib")
        .and_then(|s| s.parse().ok())
        .unwrap_or(128);
    let guest = if has_flag("--steamos") {
        GuestOs::SteamOs
    } else {
        GuestOs::Windows
    };

    if has_flag("--create-disk") {
        match create_disk(Path::new(&disk), size_gib) {
            Ok(()) => eprintln!("created {size_gib} GiB disk at {disk}"),
            Err(e) => {
                eprintln!("create disk failed: {e}");
                std::process::exit(1);
            }
        }
    }

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
        guest,
        gpu_address: cap.gpu.address.clone(),
        native_hardware: false,
    };
    let spec = DomainSpec {
        station: &station,
        vcpus: 8,
        memory_mib: 16384,
        disk_path: disk,
        passthrough_addresses: plan.bind_addresses,
        media: InstallMedia {
            install_iso: arg_value("--iso"),
            virtio_iso: arg_value("--virtio-iso"),
        },
        usb_devices: Vec::new(),
    };
    print!("{}", render(&spec));
}

//! `tendril-vm` — render a station's domain and (optionally) define it with libvirt.
//!
//! Dry-run by default: prints the domain XML. `--define` registers it with libvirt (validated, not
//! started). Starting is left to a human — that's when the GPU is detached from the host.

use tendril_capability_engine::detect_with_groups;
use tendril_orchestrator::{provision, GuestOs, InstallMedia, Libvirt, StationRequest};
use tendril_provisioning::plan_for;

fn main() {
    let define = std::env::args().any(|a| a == "--define");

    let (matrix, groups) = detect_with_groups();

    let Some(cap) = matrix.passthrough_capable().next() else {
        eprintln!("No passthrough-capable GPU to build a station for.");
        std::process::exit(1);
    };

    let req = StationRequest {
        name: "station1".to_string(),
        guest: GuestOs::Windows,
        disk_path: "/var/lib/tendril/station1.qcow2".to_string(),
        size_gib: 128,
        create_disk: false,
        vcpus: 8,
        memory_mib: 16384,
        native_hardware: false,
        passthrough_addresses: plan_for(&cap.gpu, &groups).bind_addresses,
        mdev_uuid: None,
        media: InstallMedia::none(),
        usb_devices: Vec::new(),
        define,
        start: false,
        steam_library_dir: None,
        data_disk: None,
        cpu_pinning: None,
        hugepages: false,
    };

    match provision(&req, &Libvirt::system()) {
        Ok(report) if !define => {
            print!("{}", report.xml);
            eprintln!(
                "(dry-run — pass --define to register the domain with libvirt; it will not be started)"
            );
        }
        Ok(_) => println!("defined domain '{}' (not started)", req.name),
        Err(e) => {
            eprintln!("define failed: {e}");
            std::process::exit(1);
        }
    }
}

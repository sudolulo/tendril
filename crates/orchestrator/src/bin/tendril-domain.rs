//! `tendril-domain` — render a libvirt domain for the first passthrough-capable GPU.
//!
//! Demonstrates the pipeline end to end: detect → plan → domain XML. Prints XML only; it does not
//! define or start anything with libvirt.

use tendril_capability_engine::detect_with_groups;
use tendril_orchestrator::{provision, GuestOs, InstallMedia, Libvirt, StationRequest};
use tendril_provisioning::plan_for;

fn main() {
    let (matrix, groups) = detect_with_groups();

    let Some(cap) = matrix.passthrough_capable().next() else {
        println!("No passthrough-capable GPU to build a station for.");
        return;
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
        define: false,
        start: false,
        steam_library_dir: None,
        data_disk: None,
        cpu_pinning: None,
        hugepages: false,
    };

    match provision(&req, &Libvirt::system()) {
        Ok(report) => print!("{}", report.xml),
        Err(e) => {
            eprintln!("provision failed: {e}");
            std::process::exit(1);
        }
    }
}

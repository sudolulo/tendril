//! `tendril-apply` — bind passthrough-capable GPUs to `vfio-pci`.
//!
//! Dry-run by default: it prints the exact sysfs writes it *would* make. Pass `--execute` to
//! actually perform them — this DETACHES the GPU from the host driver, so only do it when you mean
//! to hand the card to a VM.

use tendril_capability_engine::{iommu, matrix, pci};
use tendril_provisioning::{apply, PassthroughStrategy, ProvisioningStrategy};

fn main() {
    let execute = std::env::args().any(|a| a == "--execute");
    let mode = if execute {
        apply::Mode::Execute
    } else {
        apply::Mode::DryRun
    };

    let gpus = pci::enumerate();
    let groups = iommu::read_groups();
    let matrix = matrix::build(gpus, &groups);
    let strategy = PassthroughStrategy;

    let mut any = false;
    for cap in matrix.passthrough_capable() {
        any = true;
        let group = groups
            .iter()
            .find(|g| g.device_addresses.iter().any(|a| a == &cap.gpu.address));
        let plan = strategy.plan(&cap.gpu, group);

        println!(
            "{} [{:04x}:{:04x}] — {}",
            cap.gpu.address, cap.gpu.vendor_id, cap.gpu.device_id, plan.summary
        );
        for addr in &plan.bind_addresses {
            let cur = apply::current_driver(addr).unwrap_or_else(|| "(none)".to_string());
            println!("  {addr}  currently bound to: {cur}");
        }
        if let Err(e) = apply::execute(&apply::render(&plan), mode) {
            eprintln!("  ERROR: {e}");
            std::process::exit(1);
        }
        println!();
    }

    if !any {
        println!("No passthrough-capable GPUs detected.");
    } else if !execute {
        println!("(dry-run — re-run with --execute to actually bind. This detaches the GPU from the host driver.)");
    }
}

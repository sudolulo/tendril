//! `tendril-plan` — for each passthrough-capable GPU, print its VFIO provisioning plan.

use tendril_capability_engine::{iommu, matrix, pci};
use tendril_provisioning::{PassthroughStrategy, ProvisioningStrategy};

fn main() {
    let gpus = pci::enumerate();
    let groups = iommu::read_groups();
    let matrix = matrix::build(gpus, &groups);
    let strategy = PassthroughStrategy;

    let mut any = false;
    for cap in matrix.passthrough_capable() {
        any = true;
        let group = iommu::group_of(&cap.gpu.address, &groups);
        let plan = strategy.plan(&cap.gpu, group);

        println!(
            "GPU {} [{:04x}:{:04x}] — {}",
            cap.gpu.address, cap.gpu.vendor_id, cap.gpu.device_id, plan.summary
        );
        println!("  driver: {}", plan.driver);
        println!("  bind:   {}", plan.bind_addresses.join("  "));
        if let Some(note) = &plan.note {
            println!("  note:   {note}");
        }
    }

    if !any {
        println!("No passthrough-capable GPUs detected.");
    }
}

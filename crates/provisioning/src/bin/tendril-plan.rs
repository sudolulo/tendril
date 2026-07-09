//! `tendril-plan` — for each passthrough-capable GPU, print its VFIO provisioning plan.

use tendril_capability_engine::detect_with_groups;
use tendril_provisioning::plan_for;

fn main() {
    let (matrix, groups) = detect_with_groups();

    let mut any = false;
    for cap in matrix.passthrough_capable() {
        any = true;
        let plan = plan_for(&cap.gpu, &groups);

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

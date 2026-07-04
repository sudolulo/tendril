//! `tendril-detect` — print the host's GPU capability matrix.

use tendril_capability_engine::{detect, Capability};

fn main() {
    let matrix = detect();

    if matrix.gpus.is_empty() {
        println!("No display devices found (is this running on the host, with /sys available?).");
        return;
    }

    println!("Detected {} display device(s):\n", matrix.gpus.len());
    for cap in &matrix.gpus {
        let model = cap.gpu.model.as_deref().unwrap_or("");
        println!(
            "  {addr}  [{vid:04x}:{did:04x}]  {vendor:<7?}  {capability:?} ({viability:?}) {model}",
            addr = cap.gpu.address,
            vid = cap.gpu.vendor_id,
            did = cap.gpu.device_id,
            vendor = cap.gpu.vendor,
            capability = cap.capability,
            viability = cap.viability,
        );
    }

    let stations = matrix.passthrough_capable().count();
    println!("\n{stations} GPU(s) can drive an independent gaming station via passthrough.");
    if matrix
        .gpus
        .iter()
        .any(|c| matches!(c.capability, Capability::HostOnly))
    {
        println!("(Host-only devices are kept for the host console / not passthrough-capable.)");
    }
}

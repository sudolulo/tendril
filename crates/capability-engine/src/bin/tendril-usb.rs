//! `tendril-usb` — list USB host controllers (for whole-controller passthrough to a seat) and
//! connected USB devices (for per-device passthrough of a seat's keyboard/mouse).

use tendril_capability_engine::usb;

fn main() {
    let controllers = usb::controllers();
    println!("USB host controllers ({}):", controllers.len());
    for c in &controllers {
        println!(
            "  {}  [{:04x}:{:04x}]  {:?}  (IOMMU group: {} device(s))",
            c.address,
            c.vendor_id,
            c.device_id,
            c.viability,
            c.iommu_group.len()
        );
    }

    let devices = usb::devices();
    println!("\nConnected USB devices ({}):", devices.len());
    for d in &devices {
        let name = d.product.as_deref().unwrap_or("");
        println!(
            "  {}  [{:04x}:{:04x}]  {}",
            d.port, d.vendor_id, d.product_id, name
        );
    }
}

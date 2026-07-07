//! Hardware & passthrough: the GPU/IOMMU matrix (with a "bind to vfio-pci" action) and USB devices.

use std::collections::HashMap;

use axum::extract::Path;
use maud::{html, Markup};

use tendril_capability_engine::{detect, iommu, pci, usb, Capability};
use tendril_orchestrator::Libvirt;
use tendril_provisioning::{apply, Mode, PassthroughStrategy, ProvisioningStrategy};

use crate::ui;

/// Which station (if any) passes through each GPU PCI address.
pub fn gpu_users() -> HashMap<String, String> {
    let lv = Libvirt::system();
    let mut m = HashMap::new();
    for name in lv.list() {
        for addr in lv.pci_hostdevs(&name) {
            m.insert(addr, name.clone());
        }
    }
    m
}

/// Which station (if any) passes through each USB `(vendor, product)`.
fn usb_users() -> HashMap<(u16, u16), String> {
    let lv = Libvirt::system();
    let mut m = HashMap::new();
    for name in lv.list() {
        for id in lv.usb_devices(&name) {
            m.insert(id, name.clone());
        }
    }
    m
}

/// A small "in use by <station>" / "free" cell.
fn used_by(station: Option<&String>) -> Markup {
    html! {
        @match station {
            Some(s) => span.badge title="Passed through to this station" { "▶ " (s) },
            None => span.sub { "free" },
        }
    }
}

pub async fn page() -> Markup {
    ui::page(
        "hardware",
        "Hardware",
        html! {
            (ui::panel("GPUs & passthrough", None, gpu_fragment(None)))
            (ui::panel("USB devices", None, usb_panel()))
            (ui::panel("Seats", Some("USB device groups a station passes through as one"), crate::seats::panel()))
        },
    )
}

/// The GPU table (swapped in place after a bind). `note` shows the result of the last action.
fn gpu_fragment(note: Option<Markup>) -> Markup {
    let matrix = detect();
    let users = gpu_users();
    html! {
        div #gpus {
            @if let Some(n) = note { div.pad style="padding-bottom:0" { (n) } }
            div.scroll {
                table {
                    thead { tr { th { "GPU" } th { "Address" } th { "Capability" } th { "Passthrough" } th { "Driver" } th { "Used by" } th.right { "" } } }
                    tbody {
                        @for g in &matrix.gpus {
                            @let addr = g.gpu.address.clone();
                            @let bindable = matches!(g.capability, Capability::Passthrough);
                            @let driver = apply::current_driver(&addr).unwrap_or_else(|| "—".into());
                            tr {
                                td { div.name { (ui::vendor(g.gpu.vendor)) " " (g.gpu.model.as_deref().unwrap_or("GPU")) } }
                                td.addr.mono { (addr) }
                                td { (format!("{:?}", g.capability)) }
                                td { span class=(if matches!(g.viability, tendril_capability_engine::PassthroughViability::Isolated) { "via clean" } else { "via" }) { (ui::viability(g.viability)) } }
                                td.mono.sub { (driver) }
                                td { (used_by(users.get(&addr))) }
                                td.right {
                                    @if bindable && driver != "vfio-pci" {
                                        button.btn.sm
                                            hx-post=(format!("/hardware/{addr}/bind"))
                                            hx-target="#gpus" hx-swap="outerHTML"
                                            hx-confirm=(format!("Bind {addr} (its whole IOMMU group) to vfio-pci? This detaches the GPU from the host now.")) {
                                            "Bind to vfio-pci"
                                        }
                                    } @else if driver == "vfio-pci" {
                                        span.badge { "bound" }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            p.sub.pad style="padding-top:12px" { (matrix.passthrough_capable().count()) " GPU(s) ready for passthrough." }
        }
    }
}

pub async fn bind(Path(addr): Path<String>) -> Markup {
    let gpus = pci::enumerate();
    let groups = iommu::read_groups();
    let Some(gpu) = gpus.iter().find(|g| g.address == addr) else {
        return gpu_fragment(Some(html! { div.banner.error { "No such GPU: " (addr) } }));
    };
    let plan = PassthroughStrategy.plan(gpu, iommu::group_of(&addr, &groups));
    let actions = apply::render(&plan);
    let note = match apply::execute(&actions, Mode::Execute) {
        Ok(()) => {
            html! { div.banner.ok { "Bound " (addr) " (" (plan.bind_addresses.len()) " devices) to vfio-pci." } }
        }
        Err(e) => html! { div.banner.error { "Bind failed: " (e) } },
    };
    gpu_fragment(Some(note))
}

fn usb_panel() -> Markup {
    let controllers = usb::controllers();
    let devices = usb::devices();
    html! {
        div.scroll {
            table {
                thead { tr { th { "USB controller" } th { "Passthrough" } th.right { "Group devices" } } }
                tbody {
                    @for c in &controllers {
                        tr {
                            td.mono { (c.address) " " (format!("{:04x}:{:04x}", c.vendor_id, c.device_id)) }
                            td { (ui::viability(c.viability)) }
                            td.right.num { (c.iommu_group.len()) }
                        }
                    }
                }
            }
        }
        div.pad {
            p.sub style="margin:0 0 8px" { (devices.len()) " connected device(s):" }
            @let users = usb_users();
            @for d in &devices {
                div style="display:flex; gap:8px; align-items:center; justify-content:space-between" {
                    div.sub.mono { (format!("{:04x}:{:04x}", d.vendor_id, d.product_id)) " — " (d.product.as_deref().unwrap_or("device")) }
                    (used_by(users.get(&(d.vendor_id, d.product_id))))
                }
            }
        }
    }
}

//! Dashboard, Media, and Network pages.

use std::path::Path as FsPath;
use std::process::Command;

use maud::{html, Markup};

use tendril_capability_engine::detect;
use tendril_orchestrator::{DomainState, Libvirt};

use crate::stations;
use crate::ui;

const ISO_DIR: &str = "/var/lib/tendril/isos";

// ── dashboard ───────────────────────────────────────────────────────────────────────────────

pub async fn dashboard() -> Markup {
    let lv = Libvirt::system();
    let names = lv.list();
    let running = names
        .iter()
        .filter(|n| matches!(lv.state(n), DomainState::Running))
        .count();
    let matrix = detect();
    let ready = matrix.passthrough_capable().count();
    let (threads, mem_gib) = host_capacity();

    ui::page(
        "dashboard",
        "Dashboard",
        html! {
            section.summary {
                (stat("Stations", &names.len().to_string(), false, None))
                (stat("Running", &running.to_string(), true, None))
                (stat("GPUs · passthrough-ready", &ready.to_string(), false, Some(&format!("/ {}", matrix.gpus.len()))))
                (stat("Host capacity", &threads.to_string(), false, Some(&format!("threads · {mem_gib} GB RAM"))))
            }
            (ui::panel("Stations", None, stations::fragment(&lv)))
            (ui::panel("Hardware", Some(&format!("{ready} of {} GPUs ready", matrix.gpus.len())), html! {
                div.scroll {
                    table {
                        thead { tr { th { "GPU" } th { "Address" } th { "Capability" } th { "Passthrough" } } }
                        tbody {
                            @for g in &matrix.gpus {
                                tr {
                                    td.name { (ui::vendor(g.gpu.vendor)) " " (g.gpu.model.as_deref().unwrap_or("GPU")) }
                                    td.addr.mono { (g.gpu.address) }
                                    td { (format!("{:?}", g.capability)) }
                                    td.sub { (ui::viability(g.viability)) }
                                }
                            }
                        }
                    }
                }
            }))
        },
    )
}

fn stat(k: &str, v: &str, accent: bool, small: Option<&str>) -> Markup {
    html! {
        div.stat {
            div.k { (k) }
            div class=(if accent { "v num accent" } else { "v num" }) {
                (v) @if let Some(s) = small { " " small { (s) } }
            }
        }
    }
}

fn host_capacity() -> (usize, u64) {
    let threads = ui::run_stdout("nproc", &[])
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0);
    let mem_gib = std::fs::read_to_string("/proc/meminfo")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("MemTotal:"))
                .and_then(|l| l.split_whitespace().nth(1))
                .and_then(|kb| kb.parse::<u64>().ok())
        })
        .map(|kb| (kb + 512 * 1024) / (1024 * 1024))
        .unwrap_or(0);
    (threads, mem_gib)
}

// ── media ───────────────────────────────────────────────────────────────────────────────────

pub async fn media() -> Markup {
    media_page(None)
}

fn media_page(note: Option<Markup>) -> Markup {
    ui::page(
        "media",
        "Media",
        html! {
            @if let Some(n) = note { (n) }
            (ui::panel("Install media", Some(ISO_DIR), html! {
                div.pad {
                    @let isos = list_isos();
                    @if isos.is_empty() {
                        p.sub { "No ISOs yet. Fetch one below, or drop files into " span.mono { (ISO_DIR) } "." }
                    } @else {
                        table {
                            thead { tr { th { "File" } th.right { "Size" } } }
                            tbody { @for (f, sz) in &isos { tr { td.mono { (f) } td.right.num { (sz) } } } }
                        }
                    }
                    div.btnrow style="margin-top:16px" {
                        button.btn hx-post="/media/fetch/windows" hx-target="#media-note" hx-swap="innerHTML" { "Fetch Windows 11 + virtio" }
                        button.btn hx-post="/media/fetch/steamos" hx-target="#media-note" hx-swap="innerHTML" { "Fetch SteamOS (Bazzite)" }
                    }
                    div #media-note style="margin-top:12px" {}
                }
            }))
        },
    )
}

pub async fn fetch(axum::extract::Path(which): axum::extract::Path<String>) -> Markup {
    let script = match which.as_str() {
        "windows" => "fetch-windows-media.sh",
        "steamos" => "fetch-steamos-media.sh",
        _ => return html! { div.banner.error { "Unknown media type." } },
    };
    match locate_script(script) {
        Some(path) => match Command::new(&path).arg("--dest").arg(ISO_DIR).spawn() {
            Ok(_) => {
                html! { div.banner.ok { "Started downloading in the background (several GB). Refresh this page to see files appear in " span.mono { (ISO_DIR) } "." } }
            }
            Err(e) => html! { div.banner.error { "Could not start " (path) ": " (e) } },
        },
        None => {
            html! { div.banner.error { "Fetch script not found (" (script) "). Run it from the console instead." } }
        }
    }
}

fn list_isos() -> Vec<(String, String)> {
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(ISO_DIR) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.ends_with(".iso") {
                let sz = e.metadata().map(|m| human(m.len())).unwrap_or_default();
                out.push((name, sz));
            }
        }
    }
    out.sort();
    out
}

fn human(bytes: u64) -> String {
    let (v, u) = if bytes >= 1 << 30 {
        (bytes as f64 / (1u64 << 30) as f64, "GB")
    } else {
        (bytes as f64 / (1u64 << 20) as f64, "MB")
    };
    format!("{v:.1} {u}")
}

fn locate_script(name: &str) -> Option<String> {
    for base in ["/usr/libexec/tendril", "scripts", "./scripts"] {
        let p = format!("{base}/{name}");
        if FsPath::new(&p).exists() {
            return Some(p);
        }
    }
    None
}

// ── network ─────────────────────────────────────────────────────────────────────────────────

pub async fn network() -> Markup {
    ui::page(
        "network",
        "Network",
        html! {
            (ui::panel("Interfaces", None, html! {
                pre.pad.mono style="margin:0; overflow-x:auto" { (ui::run_stdout("ip", &["-brief", "addr"]).unwrap_or_default()) }
            }))
            (ui::panel("Routes", None, html! {
                pre.pad.mono style="margin:0; overflow-x:auto" { (ui::run_stdout("ip", &["route"]).unwrap_or_default()) }
            }))
            (ui::panel("DNS", None, html! {
                pre.pad.mono style="margin:0; overflow-x:auto" { (dns()) }
            }))
            p.sub { "Editing the network from the browser is intentionally disabled (you're likely connected over it). Change it from the console menu (" span.mono { "tendril" } " → Configure network → nmtui)." }
        },
    )
}

fn dns() -> String {
    std::fs::read_to_string("/etc/resolv.conf")
        .unwrap_or_default()
        .lines()
        .filter(|l| l.starts_with("nameserver") || l.starts_with("search"))
        .collect::<Vec<_>>()
        .join("\n")
}

// ── system / OS updates (bootc) ───────────────────────────────────────────────────────────────

/// The bootc auto-update timer that periodically fetches and stages new images.
const AUTO_TIMER: &str = "bootc-fetch-apply-updates.timer";

pub async fn system() -> Markup {
    let status = ui::run_stdout("bootc", &["status"]);
    ui::page(
        "system",
        "System",
        html! {
            @if status.is_none() {
                (ui::panel("OS updates", None, html! {
                    div.pad { p.muted {
                        "This host isn't running bootc, so OS updates aren't managed here. On a Tendril "
                        "install this shows your image version and lets you update the whole OS atomically."
                    } }
                }))
            } @else {
                (ui::panel("OS image", None, html! {
                    div.pad {
                        pre.mono style="margin:0; overflow-x:auto; white-space:pre-wrap; font-size:12.5px" { (status.unwrap_or_default().trim()) }
                        div.btnrow style="margin-top:14px" {
                            button.btn hx-post="/system/check" hx-target="#update-result" hx-swap="innerHTML" { "Check for updates" }
                            button.btn.primary hx-post="/system/update" hx-target="#update-result" hx-swap="innerHTML"
                                hx-confirm="Download and stage the latest OS image? It applies on the next reboot." { "Update now" }
                        }
                        div #update-result style="margin-top:12px" {}
                    }
                }))
                (ui::panel("Automatic updates", None, auto_fragment()))
            }
        },
    )
}

pub async fn system_check() -> Markup {
    match ui::run_stdout("bootc", &["upgrade", "--check"]) {
        Some(out) if !out.trim().is_empty() => {
            html! { div.banner.ok { pre.mono style="margin:0; white-space:pre-wrap" { (out.trim()) } } }
        }
        Some(_) => html! { div.banner.ok { "You're on the latest image." } },
        None => {
            html! { div.banner.error { "Update check failed — no network, or this isn't a bootc host." } }
        }
    }
}

pub async fn system_update() -> Markup {
    match Command::new("bootc").arg("upgrade").output() {
        Ok(o) if o.status.success() => html! {
            div.banner.ok { "Update staged. Reboot to apply it (System stays on the current image until you do)." }
        },
        Ok(o) => {
            let msg = String::from_utf8_lossy(&o.stderr).trim().to_string();
            html! { div.banner.error { "Update failed: " (msg) } }
        }
        Err(e) => html! { div.banner.error { "Could not run bootc: " (e.to_string()) } },
    }
}

/// Toggle the auto-update timer, then re-render the panel.
pub async fn system_auto() -> Markup {
    let action = if auto_enabled() { "disable" } else { "enable" };
    let _ = Command::new("systemctl")
        .args([action, "--now", AUTO_TIMER])
        .status();
    auto_fragment()
}

fn auto_enabled() -> bool {
    ui::run_stdout("systemctl", &["is-enabled", AUTO_TIMER])
        .map(|s| s.trim() == "enabled")
        .unwrap_or(false)
}

fn auto_fragment() -> Markup {
    let on = auto_enabled();
    html! {
        div #autoupd {
            div.pad {
                div style="display:flex; align-items:center; gap:12px" {
                    @if on {
                        span.pill.running { span.led {} "on" }
                        button.btn hx-post="/system/auto" hx-target="#autoupd" hx-swap="outerHTML" { "Disable" }
                    } @else {
                        span.pill.off { span.led {} "off" }
                        button.btn.primary hx-post="/system/auto" hx-target="#autoupd" hx-swap="outerHTML" { "Enable" }
                    }
                }
                p.sub style="margin:10px 0 0" {
                    "When on, the host fetches and stages new OS images on a timer ("
                    span.mono { (AUTO_TIMER) } "); they apply on the next reboot."
                }
            }
        }
    }
}

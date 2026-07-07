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
                        div.scroll {
                            table {
                                thead { tr { th { "File" } th { "Verification" } th.right { "Size" } } }
                                tbody { @for (f, sz) in &isos {
                                    tr {
                                        td.mono { (f) }
                                        td { (verify_cell(f)) }
                                        td.right.num { (sz) }
                                    }
                                } }
                            }
                        }
                    }
                    div.btnrow style="margin-top:16px" {
                        button.btn hx-post="/media/fetch/windows" hx-target="#media-note" hx-swap="innerHTML" { "Fetch Windows 11 + virtio" }
                        button.btn hx-post="/media/fetch/steamos" hx-target="#media-note" hx-swap="innerHTML" { "Fetch SteamOS (Bazzite)" }
                    }
                    div #media-note style="margin-top:12px" {}
                    p.sub style="margin-top:10px" {
                        "Bazzite ISOs are checked against Bazzite's published SHA-256. Windows is assembled by UUP "
                        "dump from hash-verified components (no single upstream checksum for the built ISO)."
                    }
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

/// Verification state of a media ISO, from the marker files `verify-media.sh` writes.
enum VerifyState {
    /// A background verification is in progress.
    Verifying,
    /// SHA-256 matched the upstream-published checksum (short hash).
    Verified(String),
    /// SHA-256 did NOT match the published checksum (short hash).
    Mismatch(String),
    /// A local hash was recorded but there's no upstream checksum to compare (short hash).
    Local(String),
    /// Not checked yet.
    Unchecked,
}

fn list_isos() -> Vec<(String, String)> {
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(ISO_DIR) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.ends_with(".iso") {
                out.push((
                    name,
                    e.metadata().map(|m| human(m.len())).unwrap_or_default(),
                ));
            }
        }
    }
    out.sort();
    out
}

fn verify_state(name: &str) -> VerifyState {
    let read = |ext: &str| std::fs::read_to_string(format!("{ISO_DIR}/{name}.{ext}")).ok();
    let short = |c: String| {
        c.split_whitespace()
            .next()
            .unwrap_or("")
            .chars()
            .take(12)
            .collect::<String>()
    };
    if read("verifying").is_some() {
        VerifyState::Verifying
    } else if let Some(c) = read("verified") {
        VerifyState::Verified(short(c))
    } else if let Some(c) = read("mismatch") {
        VerifyState::Mismatch(short(c))
    } else if let Some(c) = read("sha256") {
        VerifyState::Local(short(c))
    } else {
        VerifyState::Unchecked
    }
}

/// A CSS-selector-safe element id for an ISO (its name has dots, which break `#id` selectors).
fn cell_id(iso: &str) -> String {
    format!("v-{}", iso.replace(['.', ' '], "-"))
}

/// One ISO's verification cell — badge plus a Verify button, or (while a check runs) a self-polling
/// "verifying…" that swaps itself for the result when done. No page refresh needed.
fn verify_cell(iso: &str) -> Markup {
    let st = verify_state(iso);
    let id = cell_id(iso);
    html! {
        div id=(id) {
            @match &st {
                VerifyState::Verifying => {
                    span.sub { "verifying\u{2026}" }
                    span hx-get=(format!("/media/verifystatus/{iso}")) hx-trigger="every 2s"
                        hx-target=(format!("#{id}")) hx-swap="outerHTML" {}
                }
                VerifyState::Verified(h) => {
                    span.pill.running { span.led {} "verified" } " " span.sub.mono { (h) "\u{2026}" }
                }
                VerifyState::Mismatch(h) => {
                    span.pill.off style="background:var(--crit-soft);color:var(--crit)" { span.led {} "MISMATCH" } " " span.sub.mono { (h) "\u{2026}" }
                }
                _ => {
                    @if let VerifyState::Local(h) = &st {
                        span.sub.mono { "sha256 " (h) "\u{2026}" } " " span.sub { "· no upstream" } " "
                    } @else {
                        span.sub { "unverified " }
                    }
                    button.btn.sm hx-post=(format!("/media/verify/{iso}"))
                        hx-target=(format!("#{id}")) hx-swap="outerHTML" { "Verify" }
                }
            }
        }
    }
}

fn guard_iso(iso: &str) -> Option<String> {
    if iso.contains('/') || !iso.ends_with(".iso") {
        return None;
    }
    let path = format!("{ISO_DIR}/{iso}");
    FsPath::new(&path).exists().then_some(path)
}

/// Kick off a background verification and return the (now self-polling) cell.
pub async fn verify(axum::extract::Path(iso): axum::extract::Path<String>) -> Markup {
    if let (Some(path), Some(script)) = (guard_iso(&iso), locate_script("verify-media.sh")) {
        // Mark in-progress, run the (slow) hash+compare detached, clear the marker when done.
        let _ = std::fs::write(format!("{path}.verifying"), "");
        let cmd = format!(
            "{s} {p}; rm -f {p}.verifying",
            s = shq(&script),
            p = shq(&path)
        );
        let _ = Command::new("sh").arg("-c").arg(cmd).spawn();
    }
    verify_cell(&iso)
}

/// Poll target: re-render the verification cell.
pub async fn verify_status(axum::extract::Path(iso): axum::extract::Path<String>) -> Markup {
    verify_cell(&iso)
}

/// Minimal single-quote shell escaping for a trusted-but-punctuated path.
fn shq(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
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
            (ui::panel("Power", None, html! {
                div.pad {
                    div.btnrow {
                        button.btn hx-post="/system/reboot" hx-target="#power-result" hx-swap="innerHTML"
                            hx-confirm="Reboot the host now? All running stations will be stopped." { "Reboot" }
                        button.btn.danger hx-post="/system/shutdown" hx-target="#power-result" hx-swap="innerHTML"
                            hx-confirm="Shut down the host now? All running stations will be stopped." { "Shut down" }
                    }
                    div #power-result style="margin-top:10px" {}
                }
            }))
            (ui::panel("Host", None, host_info()))
            @if let Some(s) = status {
                (ui::panel("OS image", None, html! {
                    div.pad {
                        pre.mono style="margin:0; overflow-x:auto; white-space:pre-wrap; font-size:12.5px" { (s.trim()) }
                        div.btnrow style="margin-top:14px" {
                            button.btn hx-post="/system/check" hx-target="#update-result" hx-swap="innerHTML" { "Check for updates" }
                            button.btn.primary hx-post="/system/update" hx-target="#update-result" hx-swap="innerHTML"
                                hx-confirm="Download and stage the latest OS image? It applies on the next reboot." { "Update now" }
                        }
                        div #update-result style="margin-top:12px" {}
                    }
                }))
                (ui::panel("Automatic updates", None, auto_fragment()))
            } @else {
                (ui::panel("OS updates", None, html! {
                    div.pad { p.muted {
                        "This host isn't running bootc, so atomic OS updates aren't managed here. On a Tendril "
                        "install this shows the image version and lets you update the whole OS."
                    } }
                }))
            }
            (ui::panel("Logs", Some("last 200 journal lines · live"), logs_fragment()))
        },
    )
}

fn host_info() -> Markup {
    let line = |k: &str, v: String| html! { tr { td.sub style="white-space:nowrap" { (k) } td.mono { (v) } } };
    let load = std::fs::read_to_string("/proc/loadavg")
        .ok()
        .map(|s| s.split_whitespace().take(3).collect::<Vec<_>>().join("  "))
        .unwrap_or_default();
    html! {
        div.pad {
            table {
                tbody {
                    (line("Hostname", ui::run_stdout("hostname", &[]).unwrap_or_default().trim().to_string()))
                    (line("Uptime", ui::run_stdout("uptime", &["-p"]).unwrap_or_default().trim().to_string()))
                    (line("Load (1/5/15m)", load))
                    (line("Memory", meminfo()))
                    (line("Disk (/)", ui::run_stdout("df", &["-h", "--output=used,size,pcent", "/"]).map(|s| s.lines().nth(1).unwrap_or("").split_whitespace().collect::<Vec<_>>().join(" / ")).unwrap_or_default()))
                    (line("Kernel", ui::run_stdout("uname", &["-r"]).unwrap_or_default().trim().to_string()))
                }
            }
        }
    }
}

fn meminfo() -> String {
    let read = |k: &str| {
        std::fs::read_to_string("/proc/meminfo").ok().and_then(|s| {
            s.lines()
                .find(|l| l.starts_with(k))
                .and_then(|l| l.split_whitespace().nth(1))
                .and_then(|kb| kb.parse::<u64>().ok())
        })
    };
    match (read("MemTotal:"), read("MemAvailable:")) {
        (Some(t), Some(a)) => format!(
            "{:.1} / {:.1} GB used",
            (t - a) as f64 / 1048576.0,
            t as f64 / 1048576.0
        ),
        _ => String::new(),
    }
}

/// Recent journal lines. Polled by the Logs panel for a live tail.
pub async fn logs() -> Markup {
    logs_fragment()
}

fn logs_fragment() -> Markup {
    let out = ui::run_stdout(
        "journalctl",
        &["--no-pager", "-n", "200", "-o", "short-iso"],
    )
    .unwrap_or_else(|| "journalctl unavailable".to_string());
    html! {
        div #logs hx-get="/system/logs" hx-trigger="every 5s" hx-swap="outerHTML" {
            pre.mono style="margin:0; padding:14px 18px; max-height:420px; overflow:auto; font-size:12px; line-height:1.5" { (out) }
        }
    }
}

pub async fn system_reboot() -> Markup {
    let _ = Command::new("systemctl").arg("reboot").spawn();
    html! { div.banner.ok { "Rebooting… this connection will drop." } }
}

pub async fn system_shutdown() -> Markup {
    let _ = Command::new("systemctl").arg("poweroff").spawn();
    html! { div.banner.ok { "Shutting down… this connection will drop." } }
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

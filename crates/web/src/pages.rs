//! Dashboard, Media, and Network pages.

use std::path::Path as FsPath;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};

use axum::extract::Query;
use axum::http::header;
use axum::response::IntoResponse;
use maud::{html, Markup};
use serde::Deserialize;

use tendril_capability_engine::detect;
use tendril_orchestrator::{DomainState, Libvirt};

use crate::ui;

// ISO/image storage locations resolve through `storage` (local, or a mounted NFS/SMB share).

// ── overview strip (Stations landing) ───────────────────────────────────────────────────────

/// The at-a-glance stat strip shown at the top of the Stations landing page — folded in from the
/// former standalone Dashboard (the Hardware/Host detail it used to duplicate lives on their tabs).
pub fn overview_strip() -> Markup {
    let (n_stations, running) = if ui::is_demo() {
        crate::demo::counts()
    } else {
        let lv = Libvirt::system();
        let names = lv.list();
        let r = names
            .iter()
            .filter(|n| matches!(lv.state(n), DomainState::Running))
            .count();
        (names.len(), r)
    };
    let matrix = detect();
    let ready = matrix.passthrough_capable().count();
    let (threads, mem_gib) = host_capacity();
    html! {
        section.summary {
            (stat("Stations", &n_stations.to_string(), false, None))
            (stat("Running", &running.to_string(), true, None))
            (stat("GPUs · passthrough-ready", &ready.to_string(), false, Some(&format!("/ {}", matrix.gpus.len()))))
            (stat("Host capacity", &threads.to_string(), false, Some(&format!("threads · {mem_gib} GB RAM"))))
        }
    }
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
    let mem_gib = ui::meminfo_kb("MemTotal:")
        .map(|kb| (kb + 512 * 1024) / (1024 * 1024))
        .unwrap_or(0);
    (threads, mem_gib)
}

// ── media ───────────────────────────────────────────────────────────────────────────────────

pub async fn media() -> Markup {
    ui::page(
        "media",
        "Media",
        html! {
            @let iso_dir = crate::storage::iso_dir();
            (ui::panel("Storage", Some("where ISOs and station images live (local or a remote share)"), crate::storage::panel()))
            (ui::panel("Install media", Some(iso_dir.as_str()), media_isos_fragment()))
            (ui::panel("Station images", Some("golden templates you can clone into new stations"), crate::images::panel()))
        },
    )
}

pub async fn fetch(axum::extract::Path(which): axum::extract::Path<String>) -> Markup {
    let script = match which.as_str() {
        "windows" => "fetch-windows-media.sh",
        "steamos" => "fetch-steamos-media.sh",
        _ => return html! { div.banner.error { "Unknown media type." } },
    };
    let iso_dir = crate::storage::iso_dir();
    match locate_script(script) {
        Some(path) => match Command::new(&path).arg("--dest").arg(&iso_dir).spawn() {
            Ok(child) => {
                reap(child);
                html! { div.banner.ok { "Started downloading in the background (several GB). Refresh this page to see files appear in " span.mono { (iso_dir) } "." } }
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

/// Where a known install ISO comes from and how it's trustworthy — shown on the Media page so the
/// media never looks like it appeared from nowhere.
pub fn provenance(iso: &str) -> Option<&'static str> {
    let n = iso.to_lowercase();
    if n.starts_with("win11") {
        Some("Source: assembled by UUP dump from Microsoft's Windows Update CDN. Every component is \
              SHA-verified against Microsoft's own hashes as it downloads, so the ISO is built from \
              genuine Microsoft parts. Microsoft publishes no whole-ISO checksum, so the hash shown \
              is recorded locally (no upstream to compare).")
    } else if n.contains("virtio") {
        Some("Source: Red Hat's official virtio-win driver ISO (fedorapeople.org). The drivers inside \
              are WHQL-signed by Microsoft and GPG-signed by Red Hat; Red Hat's published CHECKSUM \
              covers the signed RPM the ISO ships in (not a bare-ISO hash), so the hash shown is local.")
    } else if n.contains("bazzite") {
        Some("Source: Bazzite (SteamOS-style) image from bazzite.gg, verified against the publisher's \
              SHA-256 CHECKSUM.")
    } else {
        None
    }
}

/// GET handler so the install-media list can self-refresh while a download is in progress.
pub async fn media_isos() -> Markup {
    media_isos_fragment()
}

/// Target names of downloads currently in progress (a hidden `.<name>.part` temp exists in the ISO
/// dir). Fetch scripts download to a `.part` and only rename to the final `.iso` when complete, so a
/// partial download is never listed or usable.
fn downloads_in_progress() -> Vec<String> {
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(crate::storage::iso_dir()) {
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().into_owned();
            if let Some(mid) = n.strip_prefix('.').and_then(|s| s.strip_suffix(".part")) {
                out.push(mid.to_string());
            }
        }
    }
    out.sort();
    out
}

/// The install-media table + fetch buttons, as a fragment that polls while a download runs so a
/// finished ISO appears (and its "downloading…" row disappears) on its own.
fn media_isos_fragment() -> Markup {
    let iso_dir = crate::storage::iso_dir();
    let isos = if ui::is_demo() {
        Vec::new()
    } else {
        list_isos()
    };
    let dls = if ui::is_demo() {
        Vec::new()
    } else {
        downloads_in_progress()
    };
    let poll = !dls.is_empty();
    html! {
        div #media-isos hx-get=[poll.then_some("/media/isos")] hx-trigger=[poll.then_some("every 3s")] hx-swap="outerHTML" {
            div.pad {
                @if ui::is_demo() {
                    (crate::demo::media_table())
                } @else if isos.is_empty() && dls.is_empty() {
                    p.sub { "No ISOs yet. Fetch one below, or drop files into " span.mono { (iso_dir) } "." }
                } @else {
                    div.scroll {
                        table {
                            thead { tr { th { "File" } th { "Verification" } th.right { "Size" } } }
                            tbody {
                                @for (f, sz) in &isos {
                                    tr {
                                        td {
                                            span.mono { (f) }
                                            @if let Some(p) = provenance(f) {
                                                span.info title=(p) style="margin-left:6px; cursor:help; color:var(--muted); border-bottom:1px dotted var(--muted)" { "\u{24D8} source" }
                                            }
                                        }
                                        td { (verify_cell(f)) }
                                        td.right.num { (sz) }
                                    }
                                }
                                @for f in &dls {
                                    tr {
                                        td { span.mono { (f) } " " span.sub { "(downloading…)" } }
                                        td { span.sub { "waiting for download" } }
                                        td.right.num { span.sub { "—" } }
                                    }
                                }
                            }
                        }
                    }
                }
                div.btnrow style="margin-top:16px" {
                    button.btn hx-post="/media/fetch/windows" hx-target="#media-note" hx-swap="innerHTML" { "Fetch Windows 11 + virtio" }
                    button.btn hx-post="/media/fetch/steamos" hx-target="#media-note" hx-swap="innerHTML" { "Fetch SteamOS (Bazzite)" }
                }
                div #media-note style="margin-top:12px" {}
            }
        }
    }
}

fn list_isos() -> Vec<(String, String)> {
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(crate::storage::iso_dir()) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.ends_with(".iso") {
                out.push((
                    name,
                    e.metadata()
                        .map(|m| ui::human_size(m.len()))
                        .unwrap_or_default(),
                ));
            }
        }
    }
    out.sort();
    out
}

fn verify_state(name: &str) -> VerifyState {
    let dir = crate::storage::iso_dir();
    let read = |ext: &str| std::fs::read_to_string(format!("{dir}/{name}.{ext}")).ok();
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
    // `iso` builds a state-file path — reject `/`/`..` (path traversal) before any read.
    if guard_iso(iso).is_none() {
        return html! {};
    }
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
    let path = format!("{}/{iso}", crate::storage::iso_dir());
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
        if let Ok(child) = Command::new("sh").arg("-c").arg(cmd).spawn() {
            reap(child);
        }
    }
    verify_cell(&iso)
}

/// Reap a detached background child off-thread — a dropped `std::process::Child` is never waited
/// on, so each finished job would otherwise linger as a zombie until the service restarts.
pub(crate) fn reap(mut child: std::process::Child) {
    std::thread::spawn(move || {
        let _ = child.wait();
    });
}

/// Poll target: re-render the verification cell.
pub async fn verify_status(axum::extract::Path(iso): axum::extract::Path<String>) -> Markup {
    // Same path-traversal guard as `verify` — `iso` is used to build a state-file path, so reject
    // `/` and `..` before reading.
    match guard_iso(&iso) {
        Some(_) => verify_cell(&iso),
        None => html! {},
    }
}

/// Minimal single-quote shell escaping for a trusted-but-punctuated path.
fn shq(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

pub fn locate_script(name: &str) -> Option<String> {
    for base in ["/usr/libexec/tendril", "scripts", "./scripts"] {
        let p = format!("{base}/{name}");
        if FsPath::new(&p).exists() {
            return Some(p);
        }
    }
    None
}

// Network configuration lives in the `network` module (nmcli-backed, editable).

// ── system / OS updates (bootc) ───────────────────────────────────────────────────────────────

/// The bootc auto-update timer that periodically fetches and stages new images.
const AUTO_TIMER: &str = "bootc-fetch-apply-updates.timer";

/// Sample `bootc status` output shown on the System page's OS-image panel when this host isn't a
/// bootc system, so the control is previewable on test builds.
const DUMMY_BOOTC_STATUS: &str = "\
Current booted image: git.onetick.ninja/flan/tendril:latest
        Digest: sha256:9f3c…a1b2  (version 0.23.0, 2026-07-09)
Current staged image: none
    Available update: none — you're on the latest image
Rollback image: git.onetick.ninja/flan/tendril:0.22.0  (bootable fallback)";

pub async fn system(headers: axum::http::HeaderMap) -> Markup {
    let is_admin = crate::auth::is_admin(&headers);
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
            (ui::panel("Admin password", None, crate::auth::password_panel()))
            (crate::auth::access_panel())
            (crate::notify::panel(is_admin))
            (crate::apitokens::panel())
            (crate::backup::panel())
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
                        div style="margin-top:16px; padding-top:14px; border-top:1px solid var(--line)" {
                            div.sub style="font-weight:600; margin-bottom:8px" { "Automatic updates" }
                            (auto_fragment())
                        }
                    }
                }))
            } @else {
                (ui::panel("OS image", None, html! {
                    div.pad {
                        div style="display:flex; gap:10px; align-items:center; margin-bottom:10px; flex-wrap:wrap" {
                            span.badge title="This host isn't bootc — shown as a preview" { "demo" }
                            span.sub { "Preview of what a real Tendril (bootc) install shows here." }
                        }
                        pre.mono style="margin:0; overflow-x:auto; white-space:pre-wrap; font-size:12.5px" { (DUMMY_BOOTC_STATUS) }
                        div.btnrow style="margin-top:14px" {
                            button.btn hx-post="/system/check" hx-target="#update-result" hx-swap="innerHTML" { "Check for updates" }
                            button.btn.primary hx-post="/system/update" hx-target="#update-result" hx-swap="innerHTML" { "Update now" }
                        }
                        div #update-result style="margin-top:12px" {}
                        div style="margin-top:16px; padding-top:14px; border-top:1px solid var(--line)" {
                            div.sub style="font-weight:600; margin-bottom:8px" { "Automatic updates" }
                            (auto_fragment())
                        }
                    }
                }))
            }
            @if let Some(vgpu) = crate::hardware::vgpu_system_panels() { (vgpu) }
            (ui::panel("Host", None, host_info()))
            (crate::tls::panel())
            (ui::panel("Logs", Some("live · filterable · downloadable"), logs_fragment(false)))
        },
    )
}

fn host_info() -> Markup {
    let line = |k: &str, v: String| html! { tr { td.sub style="white-space:nowrap" { (k) } td.mono { (v) } } };
    let load = ui::loadavg("  ");
    html! {
        div.pad {
            table {
                tbody {
                    (line("Hostname", ui::run_stdout("hostname", &[]).unwrap_or_default().trim().to_string()))
                    (line("Uptime", ui::uptime()))
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
    match ui::mem_used_total_gb() {
        Some((used, total)) => format!("{used:.1} / {total:.1} GB used"),
        None => String::new(),
    }
}

/// Recent journal lines. Polled by the Logs panel for a live tail.
#[derive(Deserialize, Default)]
pub struct LogQuery {
    #[serde(default)]
    stations: bool,
}

pub async fn logs(Query(q): Query<LogQuery>) -> Markup {
    logs_fragment(q.stations)
}

/// Download the current log view as a text file (more lines than the live view shows).
pub async fn logs_download(Query(q): Query<LogQuery>) -> impl IntoResponse {
    let body = journal_text(q.stations, "5000");
    let fname = if q.stations {
        "tendril-station-logs.txt"
    } else {
        "tendril-logs.txt"
    };
    (
        [
            (
                header::CONTENT_TYPE,
                "text/plain; charset=utf-8".to_string(),
            ),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{fname}\""),
            ),
        ],
        body,
    )
}

/// Journal text. Always drops SELinux `audit`/AVC spam (never actionable here); when `stations_only`
/// is set, further narrows to station-relevant sources (libvirt/qemu/vfio/tendril).
fn journal_text(stations_only: bool, lines: &str) -> String {
    // The public demo must never expose the real host's journal (IPs, sshd, co-located services) —
    // everything else it shows is synthetic, and the middleware only gates POSTs, not this GET.
    if ui::is_demo() {
        return "(demo — host logs are hidden)".to_string();
    }
    // Fetch extra lines before de-noising so the view still fills after audit spam is dropped.
    let fetch: u32 = lines.parse::<u32>().unwrap_or(300).saturating_mul(2);
    let denoise = r"grep -avE 'audit\[[0-9]+\]:|avc:  denied|audit: type=1[0-9]{3}'";
    let pipeline = if stations_only {
        format!(
            "journalctl --no-pager -n {fetch} -o short-iso | {denoise} | grep -iE 'libvirt|qemu|virt|vfio|hostdev|tendril|station' | tail -n {lines} || true"
        )
    } else {
        format!(
            "journalctl --no-pager -n {fetch} -o short-iso | {denoise} | tail -n {lines} || true"
        )
    };
    ui::run_stdout("sh", &["-c", &pipeline]).unwrap_or_default()
}

fn logs_fragment(stations_only: bool) -> Markup {
    let out = journal_text(stations_only, "300");
    let out = if out.is_empty() {
        "(no matching log lines)".to_string()
    } else {
        out
    };
    let all_cls = if stations_only {
        "btn sm"
    } else {
        "btn sm primary"
    };
    let sta_cls = if stations_only {
        "btn sm primary"
    } else {
        "btn sm"
    };
    html! {
        div #logs hx-get=(format!("/system/logs?stations={stations_only}")) hx-trigger="every 5s" hx-swap="outerHTML" {
            div.btnrow style="padding:10px 14px; gap:8px; align-items:center" {
                button class=(all_cls) hx-get="/system/logs?stations=false" hx-target="#logs" hx-swap="outerHTML" { "All" }
                button class=(sta_cls) hx-get="/system/logs?stations=true" hx-target="#logs" hx-swap="outerHTML" { "Stations only" }
                a.btn.sm href=(format!("/system/logs/download?stations={stations_only}")) download="tendril-logs.txt" { "⬇ Download" }
            }
            pre.mono style="margin:0; padding:0 18px 14px; max-height:420px; overflow:auto; font-size:12px; line-height:1.5" { (out) }
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
    if !is_bootc() {
        return html! { div.banner.ok { span.badge { "demo" } " On a real bootc host this checks the registry for a newer image. This host isn't bootc, so there's nothing to check." } };
    }
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
    if !is_bootc() {
        return html! { div.banner.ok { span.badge { "demo" } " On a real bootc host this stages the latest image and applies it on reboot. Nothing to update on this non-bootc host." } };
    }
    match Command::new("bootc").arg("upgrade").output() {
        Ok(o) if o.status.success() => {
            crate::notify::notify("Tendril OS update", "OS update staged — reboot to apply.");
            html! {
                div.banner.ok { "Update staged. Reboot to apply it (System stays on the current image until you do)." }
            }
        }
        Ok(o) => {
            let msg = String::from_utf8_lossy(&o.stderr).trim().to_string();
            html! { div.banner.error { "Update failed: " (msg) } }
        }
        Err(e) => html! { div.banner.error { "Could not run bootc: " (e.to_string()) } },
    }
}

/// Toggle the auto-update timer, then re-render the panel.
/// Demo auto-update state — only consulted when this host isn't bootc, so the toggle is still
/// visible and clickable on non-appliance test builds. On a real bootc host the systemd timer is
/// authoritative and this is ignored.
static DEMO_AUTO_UPDATE: AtomicBool = AtomicBool::new(false);

/// Whether this host is a bootc system (so OS updates are real).
fn is_bootc() -> bool {
    ui::run_stdout("bootc", &["status"]).is_some()
}

pub async fn system_auto() -> Markup {
    if is_bootc() {
        let action = if auto_enabled() { "disable" } else { "enable" };
        let _ = Command::new("systemctl")
            .args([action, "--now", AUTO_TIMER])
            .status();
    } else {
        // No bootc timer to flip — just toggle the in-memory demo state so the UI responds.
        let cur = DEMO_AUTO_UPDATE.load(Ordering::Relaxed);
        DEMO_AUTO_UPDATE.store(!cur, Ordering::Relaxed);
    }
    auto_fragment()
}

fn auto_enabled() -> bool {
    ui::run_stdout("systemctl", &["is-enabled", AUTO_TIMER])
        .map(|s| s.trim() == "enabled")
        .unwrap_or(false)
}

// ── Prometheus metrics ────────────────────────────────────────────────────────────────────────

/// Escape a Prometheus label value: `\` → `\\`, `"` → `\"`, newline → `\n`. Station names are
/// already charset-constrained, but escape anyway — the exposition format is easy to corrupt.
fn escape_label(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}

/// Parse `df -B1 --output=used,size <mount>` output into `(used, total)` bytes.
fn parse_df_bytes(out: &str) -> Option<(u64, u64)> {
    let mut it = out.lines().nth(1)?.split_whitespace();
    Some((it.next()?.parse().ok()?, it.next()?.parse().ok()?))
}

/// GET /metrics — Prometheus text exposition (version 0.0.4). Lives inside the auth-gated router,
/// so a session or a bearer API token both work; viewers are allowed (it's read-only).
pub async fn metrics() -> impl IntoResponse {
    // Gathering shells out (virsh, df) — off the async worker.
    let body = tokio::task::spawn_blocking(metrics_text)
        .await
        .unwrap_or_default();
    (
        [(
            header::CONTENT_TYPE,
            "text/plain; version=0.0.4; charset=utf-8",
        )],
        body,
    )
}

fn metrics_text() -> String {
    use std::fmt::Write as _;
    let mut m = String::new();
    let _ = writeln!(m, "# TYPE tendril_build_info gauge");
    let _ = writeln!(
        m,
        "tendril_build_info{{version=\"{}\"}} 1",
        escape_label(env!("CARGO_PKG_VERSION"))
    );
    // The demo stops at build_info: its stations/GPUs are synthetic (scraping them as real would be
    // misleading), and the host stats would leak the real machine the demo is co-located on.
    if ui::is_demo() {
        return m;
    }
    let lv = Libvirt::system();
    let names = lv.list();
    let _ = writeln!(m, "# TYPE tendril_stations_total gauge");
    let _ = writeln!(m, "tendril_stations_total {}", names.len());
    let _ = writeln!(m, "# TYPE tendril_station_state gauge");
    for n in &names {
        let state = format!("{:?}", lv.state(n)).to_lowercase();
        let _ = writeln!(
            m,
            "tendril_station_state{{name=\"{}\",state=\"{}\"}} 1",
            escape_label(n),
            escape_label(&state)
        );
    }
    let matrix = detect();
    let usage = crate::hardware::usage();
    let free = matrix
        .passthrough_capable()
        .filter(|g| {
            !usage.gpu.contains_key(&g.gpu.address) && !usage.mdev.contains_key(&g.gpu.address)
        })
        .count();
    let _ = writeln!(m, "# TYPE tendril_gpus_total gauge");
    let _ = writeln!(m, "tendril_gpus_total {}", matrix.gpus.len());
    let _ = writeln!(m, "# TYPE tendril_gpus_passthrough_free gauge");
    let _ = writeln!(m, "tendril_gpus_passthrough_free {free}");
    if let Some((used, total)) = ui::mem_used_total_gb() {
        let _ = writeln!(m, "# TYPE tendril_host_mem_used_gb gauge");
        let _ = writeln!(m, "tendril_host_mem_used_gb {used:.2}");
        let _ = writeln!(m, "# TYPE tendril_host_mem_total_gb gauge");
        let _ = writeln!(m, "tendril_host_mem_total_gb {total:.2}");
    }
    if let Some(load1) = ui::loadavg(" ")
        .split_whitespace()
        .next()
        .and_then(|s| s.parse::<f64>().ok())
    {
        let _ = writeln!(m, "# TYPE tendril_host_load1 gauge");
        let _ = writeln!(m, "tendril_host_load1 {load1}");
    }
    if let Some((used, total)) = ui::run_stdout("df", &["-B1", "--output=used,size", "/"])
        .as_deref()
        .and_then(parse_df_bytes)
    {
        let _ = writeln!(m, "# TYPE tendril_host_disk_used_bytes gauge");
        let _ = writeln!(m, "tendril_host_disk_used_bytes {used}");
        let _ = writeln!(m, "# TYPE tendril_host_disk_total_bytes gauge");
        let _ = writeln!(m, "tendril_host_disk_total_bytes {total}");
    }
    m
}

fn auto_fragment() -> Markup {
    let bootc = is_bootc();
    let on = if bootc {
        auto_enabled()
    } else {
        DEMO_AUTO_UPDATE.load(Ordering::Relaxed)
    };
    html! {
        div #autoupd {
            div style="display:flex; align-items:center; gap:12px; flex-wrap:wrap" {
                @if on {
                    span.pill.running { span.led {} "on" }
                    button.btn hx-post="/system/auto" hx-target="#autoupd" hx-swap="outerHTML" { "Disable" }
                } @else {
                    span.pill.off { span.led {} "off" }
                    button.btn.primary hx-post="/system/auto" hx-target="#autoupd" hx-swap="outerHTML" { "Enable" }
                }
                @if !bootc { span.badge title="This host isn't bootc — the toggle is a preview and changes no real timer" { "demo" } }
            }
            p.sub style="margin:10px 0 0" {
                "When on, the host fetches and stages new OS images on a timer ("
                span.mono { (AUTO_TIMER) } "); they apply on the next reboot."
                @if !bootc { " — preview only on this non-bootc host." }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn label_escaping() {
        assert_eq!(escape_label("plain-name_1.2"), "plain-name_1.2");
        assert_eq!(escape_label(r#"a\b"#), r#"a\\b"#);
        assert_eq!(escape_label(r#"say "hi""#), r#"say \"hi\""#);
        assert_eq!(escape_label("two\nlines"), r"two\nlines");
    }

    #[test]
    fn df_bytes_parsing() {
        let out = "    Used  1B-blocks\n42949672960 107374182400\n";
        assert_eq!(parse_df_bytes(out), Some((42_949_672_960, 107_374_182_400)));
        // Header only / garbage → None, so the disk metrics are simply omitted.
        assert_eq!(parse_df_bytes("Used 1B-blocks\n"), None);
        assert_eq!(parse_df_bytes(""), None);
        assert_eq!(parse_df_bytes("x\nnot numbers here\n"), None);
    }
}

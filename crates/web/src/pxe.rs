//! PXE room-provisioner control: start/stop the `tendril-pxe.sh` server from the web UI so a rack of
//! bare-metal PCs can net-boot into the unattended installer without touching a shell.
//!
//! The script (baked at `/usr/libexec/tendril/tendril-pxe.sh`) runs dnsmasq proxy-DHCP + TFTP + HTTP in
//! the foreground and cleans up its children on TERM. We launch it in its own session (`setsid`) so
//! it's a process group we can stop cleanly, record its PID, and stream its output to a log.

use maud::{html, Markup};

use crate::ui;

const SCRIPT: &str = "/usr/libexec/tendril/tendril-pxe.sh";
const LATEST_ISO: &str = "tendril-latest-installer-x86_64.iso";
const LATEST_URL: &str = "https://dl.onetick.ninja/tendril-latest-installer-x86_64.iso";

fn state_dir() -> String {
    std::env::var("TENDRIL_PXE_DIR").unwrap_or_else(|_| "/var/lib/tendril".to_string())
}
fn pidfile() -> String {
    format!("{}/pxe.pid", state_dir())
}
fn logfile() -> String {
    format!("{}/pxe.log", state_dir())
}
fn iso_marker() -> String {
    format!("{}/pxe.iso", state_dir())
}

/// The running PXE server's PID, if one is alive (else clears a stale pidfile).
fn running_pid() -> Option<i32> {
    let pid: i32 = std::fs::read_to_string(pidfile())
        .ok()?
        .trim()
        .parse()
        .ok()?;
    // Alive AND actually our script — a pidfile surviving a reboot can point at a recycled PID, and
    // Stop would then `kill -TERM` an unrelated process group as root.
    let cmdline = std::fs::read_to_string(format!("/proc/{pid}/cmdline")).unwrap_or_default();
    if cmdline.replace('\0', " ").contains("tendril-pxe.sh") {
        Some(pid)
    } else {
        let _ = std::fs::remove_file(pidfile());
        let _ = std::fs::remove_file(iso_marker());
        None
    }
}

/// Installer ISOs present in the media dir (basenames), newest-looking first.
fn available_isos() -> Vec<String> {
    let dir = crate::storage::iso_dir();
    let mut v: Vec<String> = std::fs::read_dir(&dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| {
            let l = n.to_lowercase();
            l.ends_with(".iso") && l.contains("tendril") && l.contains("installer")
        })
        .collect();
    v.sort();
    v.reverse();
    v
}

/// In-process guard: exactly one fetch thread per process. The mtime heuristic below only covers
/// orphans from a *previous* process — without this, two quick Fetch clicks (or a stalled >10-min
/// download plus a retry) could race two curls.
static FETCH_RUNNING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Whether a background ISO fetch is in progress: this process has a fetch thread, or a pid-suffixed
/// `.part.<pid>` temp is still growing (fresh mtime — covers a fetch owned by a previous process).
/// A temp orphaned by a mid-download restart would otherwise hide the fetch button forever.
fn fetching() -> bool {
    if FETCH_RUNNING.load(std::sync::atomic::Ordering::SeqCst) {
        return true;
    }
    let prefix = format!("{LATEST_ISO}.part");
    let Ok(rd) = std::fs::read_dir(crate::storage::iso_dir()) else {
        return false;
    };
    rd.flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with(prefix.as_str()))
        .any(|e| {
            e.metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.elapsed().ok())
                .map(|age| age.as_secs() < 600)
                .unwrap_or(false)
        })
}

/// Reject anything that isn't a plain basename (defends the shell command below from injection/paths).
fn safe_basename(name: &str) -> bool {
    !name.is_empty()
        && !name.contains('/')
        && !name.contains("..")
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
}

#[derive(serde::Deserialize)]
pub struct StartForm {
    #[serde(default)]
    iso: String,
}

/// Start the PXE server serving the chosen installer ISO (a basename in the media dir).
pub async fn start(axum::Form(f): axum::Form<StartForm>) -> Markup {
    if ui::is_demo() {
        return panel_body(Some(
            html! { div.banner.warn { "Disabled in the live demo." } },
        ));
    }
    if running_pid().is_some() {
        return panel_body(Some(
            html! { div.banner.warn { "PXE is already running." } },
        ));
    }
    let iso = f.iso.trim().to_string();
    if !safe_basename(&iso) {
        return panel_body(Some(
            html! { div.banner.error { "Choose an installer ISO." } },
        ));
    }
    let iso_path = format!("{}/{}", crate::storage::iso_dir(), iso);
    if !std::path::Path::new(&iso_path).is_file() {
        return panel_body(Some(
            html! { div.banner.error { "That ISO isn't on this node any more." } },
        ));
    }
    let _ = std::fs::create_dir_all(state_dir());
    // Launch in its own session, backgrounded, PID recorded — sh exits immediately, the server keeps
    // running (reparented to init). `iso` is validated as a safe basename above.
    // Single-quote every interpolation so a state-dir/media-dir path with spaces doesn't split the
    // command. `iso` is a validated basename; the dirs come from config, not the request.
    let cmd = format!(
        "setsid '{SCRIPT}' --iso '{iso_path}' >'{log}' 2>&1 & echo $! > '{pid}'",
        log = logfile(),
        pid = pidfile()
    );
    match tokio::task::spawn_blocking(move || {
        std::process::Command::new("sh")
            .arg("-c")
            .arg(&cmd)
            .status()
    })
    .await
    {
        Ok(Ok(st)) if st.success() => {
            let _ = std::fs::write(iso_marker(), &iso);
            panel_body(Some(html! { div.banner.ok {
                "PXE server starting — boot the target machines to network-boot. They'll ERASE their disk and install Tendril unattended."
            } }))
        }
        _ => panel_body(Some(
            html! { div.banner.error { "Couldn't start the PXE server (is dnsmasq installed and is this running as root?)." } },
        )),
    }
}

/// Stop the PXE server (kills its whole process group, so dnsmasq + the HTTP server go too).
pub async fn stop() -> Markup {
    if ui::is_demo() {
        return panel_body(Some(
            html! { div.banner.warn { "Disabled in the live demo." } },
        ));
    }
    if let Some(pid) = running_pid() {
        // Negative pid → the process group (the setsid session leader's group).
        let _ = tokio::task::spawn_blocking(move || {
            let _ = std::process::Command::new("kill")
                .args(["-TERM", &format!("-{pid}")])
                .status();
        })
        .await;
        let _ = std::fs::remove_file(pidfile());
        let _ = std::fs::remove_file(iso_marker());
    }
    panel_body(Some(html! { div.banner.ok { "PXE server stopped." } }))
}

/// Fetch the latest installer ISO from dl.onetick.ninja into the media dir (background — it's ~2.4 GB).
pub async fn fetch() -> Markup {
    if ui::is_demo() {
        return panel_body(Some(
            html! { div.banner.warn { "Disabled in the live demo." } },
        ));
    }
    use std::sync::atomic::Ordering;
    if !fetching()
        && FETCH_RUNNING
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
    {
        let dir = crate::storage::iso_dir();
        std::thread::spawn(move || {
            let _ = std::fs::create_dir_all(&dir);
            // Pid-suffixed temp: a leftover curl from a dead previous process can't interleave
            // writes with this one, and each cleanup only ever removes its own temp.
            let tmp = format!("{dir}/{LATEST_ISO}.part.{}", std::process::id());
            if ui::run_result(
                "curl",
                &["-fL", "--max-time", "3600", "-o", &tmp, LATEST_URL],
            )
            .is_ok()
            {
                let _ = std::fs::rename(&tmp, format!("{dir}/{LATEST_ISO}"));
            } else {
                let _ = std::fs::remove_file(&tmp);
            }
            FETCH_RUNNING.store(false, Ordering::SeqCst);
        });
    }
    panel_body(Some(html! { div.banner.ok {
        "Downloading the latest installer ISO… reload in a minute (it's ~2.4 GB)."
    } }))
}

/// The Fleet-page "Provision a room (PXE)" panel — now with Start/Stop controls.
pub fn panel() -> Markup {
    ui::panel(
        "Provision a room (PXE)",
        Some("net-boot many machines into the unattended installer"),
        panel_body(None),
    )
}

fn panel_body(banner: Option<Markup>) -> Markup {
    let running = running_pid();
    let isos = available_isos();
    let is_fetching = fetching();
    html! {
        div.pad #pxe-panel {
            @if let Some(b) = banner { (b) }
            p.sub style="margin-top:0" {
                "Turn this node into a PXE server so a rack of bare-metal PCs images itself hands-off: each "
                "net-boots, ERASES its disk, and installs Tendril unattended. Uses proxy-DHCP, so it's safe "
                "on a live network. UEFI targets — set them to network-boot first."
            }
            @if let Some(pid) = running {
                @let iso = std::fs::read_to_string(iso_marker()).ok().unwrap_or_default();
                div style="display:flex; align-items:center; gap:12px; flex-wrap:wrap" {
                    span.pill.running { span.led {} "serving" }
                    @if !iso.trim().is_empty() { span.sub { "ISO: " code { (iso.trim()) } } }
                    span.sub { "pid " (pid) }
                    button.btn.danger
                        hx-post="/fleet/pxe/stop" hx-target="#pxe-panel" hx-swap="outerHTML"
                        hx-confirm="Stop the PXE server? Machines already installing are unaffected." { "Stop" }
                }
                p.sub style="margin:10px 0 0" { "Boot the target machines now. They pick up the net-boot and install unattended." }
            } @else if isos.is_empty() {
                p.sub { b { "No installer ISO on this node yet." } " Fetch the latest, or copy a " code { "tendril-*-installer-x86_64.iso" } " into " code { (crate::storage::iso_dir()) } "." }
                @if is_fetching {
                    div.sub { span.pill.running { span.led {} "downloading" } " Fetching the installer ISO… reload in a minute." }
                } @else {
                    button.btn.primary hx-post="/fleet/pxe/fetch" hx-target="#pxe-panel" hx-swap="outerHTML" { "Fetch latest installer ISO" }
                }
            } @else {
                form hx-post="/fleet/pxe/start" hx-target="#pxe-panel" hx-swap="outerHTML"
                    hx-confirm="Start a PXE server on your LAN? Machines set to network-boot will ERASE their disk and install Tendril."
                    style="display:flex; gap:8px; align-items:center; flex-wrap:wrap" {
                    label.sub { "Installer ISO" }
                    select name="iso" style="font-size:12.5px" {
                        @for iso in &isos { option value=(iso) { (iso) } }
                    }
                    button.btn.primary type="submit" { "Start PXE server" }
                }
                @if !is_fetching {
                    p.sub style="margin:8px 0 0" { "Or " a href="#" hx-post="/fleet/pxe/fetch" hx-target="#pxe-panel" hx-swap="outerHTML" { "fetch the latest installer ISO" } " first." }
                }
            }
        }
    }
}

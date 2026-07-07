//! Stations: list, the create wizard (form → `provision`), detail with a live noVNC console, and
//! lifecycle actions. Everything routes through the shared `orchestrator::provision` service.

use std::path::Path as FsPath;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::Path;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};
use axum::Form;
use maud::{html, Markup};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use tendril_capability_engine::{detect, iommu, pci, usb};
use tendril_orchestrator::guest::{build_kickstart_seed, build_seed_iso};
use tendril_orchestrator::{
    provision, DomainState, GuestOs, InstallMedia, KickstartSpec, Libvirt, StationRequest,
    UnattendSpec, UsbPassthrough,
};
use tendril_provisioning::{PassthroughStrategy, ProvisioningStrategy};

use crate::ui;

const ISO_DIR: &str = "/var/lib/tendril/isos";
const DISK_DIR: &str = "/var/lib/tendril";

// ── list & dashboard fragment ───────────────────────────────────────────────────────────────

/// The self-refreshing stations panel (HTMX polls it; actions swap it).
pub fn fragment(lv: &Libvirt) -> Markup {
    let names = lv.list();
    html! {
        div #stations hx-get="/stations/fragment" hx-trigger="every 6s" hx-swap="outerHTML" {
            @if names.is_empty() {
                div.emptybox { "No stations yet. " a href="/stations/new" { "Create one." } }
            } @else {
                div.scroll {
                    table {
                        thead { tr { th { "Station" } th { "State" } th.right { "Actions" } } }
                        tbody { @for n in &names { (row(lv, n)) } }
                    }
                }
            }
        }
    }
}

fn row(lv: &Libvirt, name: &str) -> Markup {
    let state = lv.state(name);
    let running = matches!(state, DomainState::Running);
    html! {
        tr {
            td { a href=(format!("/stations/{name}")) { (name) } }
            td { (ui::state_pill(state)) }
            td.right {
                div.actions {
                    a.btn.sm href=(format!("/stations/{name}")) { "Open" }
                    @if running {
                        (action_btn(name, "stop", "Shut down", true))
                    } @else {
                        (action_btn(name, "start", "Start", false))
                    }
                    (delete_btn(name))
                }
            }
        }
    }
}

fn action_btn(name: &str, action: &str, label: &str, danger: bool) -> Markup {
    html! {
        button class=(if danger { "btn sm danger" } else { "btn sm" })
            hx-post=(format!("/stations/{name}/{action}"))
            hx-target="#stations" hx-swap="outerHTML" { (label) }
    }
}

/// Delete button with a typed browser confirm (removes the VM definition; the disk file is kept).
fn delete_btn(name: &str) -> Markup {
    html! {
        button.btn.sm.danger
            hx-post=(format!("/stations/{name}/delete"))
            hx-target="#stations" hx-swap="outerHTML"
            hx-confirm=(format!("Delete station '{name}'? If it's running it will be forced off. This removes the VM definition; the disk image is left on disk.")) {
            "Delete"
        }
    }
}

pub async fn list_page() -> Markup {
    ui::page(
        "stations",
        "Stations",
        html! {
            div.btnrow style="margin-bottom:16px" {
                a.btn.primary href="/stations/new" { "+ New station" }
            }
            (ui::panel("Stations", None, fragment(&Libvirt::system())))
        },
    )
}

pub async fn fragment_route() -> Markup {
    fragment(&Libvirt::system())
}

// ── lifecycle actions (return the refreshed fragment) ───────────────────────────────────────

pub async fn start(Path(n): Path<String>) -> Markup {
    act(|lv| lv.start(&n))
}
pub async fn stop(Path(n): Path<String>) -> Markup {
    act(|lv| lv.shutdown(&n))
}
pub async fn forceoff(Path(n): Path<String>) -> Markup {
    act(|lv| lv.destroy(&n))
}
pub async fn delete(Path(n): Path<String>) -> Markup {
    act(|lv| {
        let _ = lv.destroy(&n); // force off if running (ignored if already stopped)
        lv.undefine(&n)
    })
}

fn act(f: impl FnOnce(&Libvirt) -> std::io::Result<()>) -> Markup {
    let lv = Libvirt::system();
    let err = f(&lv).err().map(|e| e.to_string());
    html! {
        @if let Some(e) = err { div.banner.error { (e) } }
        (fragment(&lv))
    }
}

// ── create wizard ───────────────────────────────────────────────────────────────────────────

pub async fn new_form() -> Markup {
    create_form(None)
}

fn create_form(error: Option<&str>) -> Markup {
    let matrix = detect();
    ui::page(
        "stations",
        "New station",
        html! {
            @if let Some(e) = error { div.banner.error { (e) } }
            (ui::panel("Create a gaming station", None, html! {
                @let (ram, vcpus, disk) = resource_defaults();
                form.grid.pad method="post" action="/stations" {
                    div.field { label { "Station name" } input name="name" value="station1" required; }
                    div.field {
                        label { "Guest OS" }
                        select name="os" {
                            option value="windows" { "Windows 11" }
                            option value="steamos" { "SteamOS (Bazzite)" }
                        }
                    }
                    div.field.wide {
                        label { "GPU (passed through whole IOMMU group)" }
                        select name="gpu" {
                            option value="" { "None — headless / attach later" }
                            @for g in matrix.passthrough_capable() {
                                option value=(g.gpu.address) {
                                    (ui::vendor(g.gpu.vendor)) " "
                                    (g.gpu.model.as_deref().unwrap_or("GPU")) " [" (g.gpu.address) "]"
                                }
                            }
                        }
                    }
                    div.field.check.wide { input type="checkbox" name="unattend" id="unattend" checked; label for="unattend" { "Install unattended (hands-off)" } }
                    div.field { label { "Username" } input name="username" value="player"; }
                    div.field { label { "Password" } input name="password" value="tendril"; }
                    @let usb_list = usb::devices();
                    @if !usb_list.is_empty() {
                        div.field.wide {
                            label { "Pass through USB devices (keyboard, mouse, controller)" }
                            @for d in &usb_list {
                                @let id = format!("{:04x}:{:04x}", d.vendor_id, d.product_id);
                                @let uid = format!("usb-{id}");
                                div.check {
                                    input type="checkbox" name="usb" value=(id) id=(uid);
                                    label for=(uid) { (d.product.as_deref().unwrap_or("USB device")) " " span.sub.mono { "(" (id) ")" } }
                                }
                            }
                            span.hint { "You can also add or remove these after the station is created." }
                        }
                    }
                    div.field.check.wide { input type="checkbox" name="start" id="start" checked; label for="start" { "Start now (begins the install)" } }
                    details.advanced.wide {
                        summary { "Advanced options" }
                        div.grid style="margin-top:14px" {
                            div.field { label { "Memory (MiB)" } input name="memory_mib" value=(ram) inputmode="numeric"; span.hint { "Default: host RAM ÷ GPUs" } }
                            div.field { label { "vCPUs" } input name="vcpus" value=(vcpus) inputmode="numeric"; span.hint { "Default: threads ÷ GPUs" } }
                            div.field { label { "Disk size (GiB)" } input name="size_gib" value=(disk) inputmode="numeric"; span.hint { "Default: free disk ÷ GPUs" } }
                            div.field { label { "Disk image path" } input name="disk" placeholder=(format!("{DISK_DIR}/<name>.qcow2")); }
                            div.field.wide { label { "Install ISO (blank = the OS default)" } input name="iso" placeholder=(format!("{ISO_DIR}/win11.iso · bazzite-deck-nvidia.iso")); }
                            div.field.wide { label { "virtio-win ISO (Windows; blank = default)" } input name="virtio_iso" placeholder=(format!("{ISO_DIR}/virtio-win.iso")); }
                            div.field.wide { label { "Computer name / hostname" } input name="hostname" placeholder="defaults to the station name"; }
                            div.field.check { input type="checkbox" name="native" id="native"; label for="native" { "Native-hardware overlay (anti-cheat; may violate ToS)" } }
                        }
                    }
                    div.field.wide { div.btnrow { button.btn.primary type="submit" { "Create station" } a.btn href="/stations" { "Cancel" } } }
                }
            }))
        },
    )
}

/// The create form is parsed as raw pairs so repeated `usb` checkboxes are all captured.
pub async fn create(Form(form): Form<Vec<(String, String)>>) -> Response {
    let get = |k: &str| -> String {
        form.iter()
            .rev()
            .find(|(kk, _)| kk == k)
            .map(|(_, v)| v.trim().to_string())
            .unwrap_or_default()
    };
    let checked = |k: &str| form.iter().any(|(kk, _)| kk == k);
    let usb_devices: Vec<UsbPassthrough> = form
        .iter()
        .filter(|(k, _)| k == "usb")
        .filter_map(|(_, v)| parse_usb_id(v))
        .collect();

    let guest = if get("os") == "steamos" {
        GuestOs::SteamOs
    } else {
        GuestOs::Windows
    };
    let name = get("name");
    if name.is_empty() {
        return create_form(Some("Station name is required.")).into_response();
    }
    let disk = {
        let d = get("disk");
        if d.is_empty() {
            format!("{DISK_DIR}/{name}.qcow2")
        } else {
            d
        }
    };

    let seed_iso = if checked("unattend") {
        match build_seed(
            guest,
            &name,
            &disk,
            &get("username"),
            &get("password"),
            &get("hostname"),
        ) {
            Ok(p) => Some(p),
            Err(e) => {
                return create_form(Some(&format!("Building the seed ISO failed: {e}")))
                    .into_response()
            }
        }
    } else {
        None
    };

    let (dram, dvcpus, ddisk) = resource_defaults();
    let install_iso = {
        let v = get("iso");
        Some(if v.is_empty() { default_iso(guest) } else { v })
    };
    let virtio_iso = if matches!(guest, GuestOs::Windows) {
        let v = get("virtio_iso");
        Some(if v.is_empty() {
            format!("{ISO_DIR}/virtio-win.iso")
        } else {
            v
        })
    } else {
        None
    };
    let req = StationRequest {
        name: name.clone(),
        guest,
        disk_path: disk.clone(),
        size_gib: get("size_gib").parse().unwrap_or(ddisk),
        create_disk: !FsPath::new(&disk).exists(),
        vcpus: get("vcpus").parse().unwrap_or(dvcpus),
        memory_mib: get("memory_mib").parse().unwrap_or(dram),
        native_hardware: checked("native"),
        passthrough_addresses: passthrough_for(&get("gpu")),
        media: InstallMedia {
            install_iso,
            virtio_iso,
            seed_iso,
        },
        usb_devices,
        define: true,
        start: checked("start"),
    };

    let lv = Libvirt::system();
    match provision(&req, &lv) {
        Ok(report) => {
            if report.started && req.needs_boot_prompt_clear() {
                // Clear the Windows CD prompt without blocking the response.
                let n = name.clone();
                tokio::task::spawn_blocking(move || Libvirt::system().clear_boot_prompt(&n));
            }
            Redirect::to(&format!("/stations/{name}")).into_response()
        }
        Err(e) => create_form(Some(&format!("Provisioning failed: {e}"))).into_response(),
    }
}

fn build_seed(
    guest: GuestOs,
    name: &str,
    disk: &str,
    username: &str,
    password: &str,
    hostname: &str,
) -> std::io::Result<String> {
    let dir = FsPath::new(disk)
        .parent()
        .map(|p| p.to_string_lossy().into_owned())
        .filter(|p| !p.is_empty())
        .unwrap_or_else(|| ".".to_string());
    let seed = format!("{dir}/{name}-seed.iso");
    let path = FsPath::new(&seed);
    match guest {
        GuestOs::Windows => build_seed_iso(
            &UnattendSpec {
                computer_name: if hostname.trim().is_empty() {
                    name.to_uppercase()
                } else {
                    hostname.trim().to_string()
                },
                username: username.to_string(),
                password: password.to_string(),
                ..UnattendSpec::default()
            },
            path,
        )?,
        GuestOs::SteamOs => build_kickstart_seed(
            &KickstartSpec {
                hostname: if hostname.trim().is_empty() {
                    name.to_string()
                } else {
                    hostname.trim().to_string()
                },
                username: username.to_string(),
                password: password.to_string(),
                ..KickstartSpec::default()
            },
            path,
        )?,
    }
    Ok(seed)
}

fn passthrough_for(addr: &str) -> Vec<String> {
    if addr.is_empty() {
        return Vec::new();
    }
    let gpus = pci::enumerate();
    let groups = iommu::read_groups();
    match gpus.iter().find(|g| g.address == addr) {
        Some(gpu) => {
            PassthroughStrategy
                .plan(gpu, iommu::group_of(addr, &groups))
                .bind_addresses
        }
        None => vec![addr.to_string()],
    }
}

/// Sensible per-station resource defaults: split the host's RAM, CPU threads, and free disk evenly
/// across the passthrough-capable GPUs (one station per GPU). Returns (memory_mib, vcpus, disk_gib).
fn resource_defaults() -> (u64, u32, u32) {
    let num = detect().passthrough_capable().count().max(1) as u64;
    let total_ram_mib = std::fs::read_to_string("/proc/meminfo")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("MemTotal:"))
                .and_then(|l| l.split_whitespace().nth(1))
                .and_then(|kb| kb.parse::<u64>().ok())
        })
        .map(|kb| kb / 1024)
        .unwrap_or(16384);
    let threads = std::thread::available_parallelism()
        .map(|n| n.get() as u64)
        .unwrap_or(8);
    let free_disk_gib = ui::run_stdout("df", &["-B1", "--output=avail", "/"])
        .and_then(|s| s.lines().nth(1).and_then(|l| l.trim().parse::<u64>().ok()))
        .map(|b| b / 1_000_000_000)
        .unwrap_or(256);

    let ram = ((total_ram_mib / num) / 1024).max(2) * 1024; // whole GiB, min 2
    let vcpus = (threads / num).max(1) as u32;
    let disk = (free_disk_gib / num).clamp(32, 1024) as u32;
    (ram, vcpus, disk)
}

/// Default install ISO for a guest OS (used when the create form's ISO field is left blank).
fn default_iso(guest: GuestOs) -> String {
    match guest {
        GuestOs::Windows => format!("{ISO_DIR}/win11.iso"),
        GuestOs::SteamOs => format!("{ISO_DIR}/bazzite-deck-nvidia.iso"),
    }
}

/// Parse a `"vvvv:pppp"` hex USB id into a passthrough spec.
fn parse_usb_id(s: &str) -> Option<UsbPassthrough> {
    let (v, p) = s.split_once(':')?;
    Some(UsbPassthrough {
        vendor_id: u16::from_str_radix(v.trim(), 16).ok()?,
        product_id: u16::from_str_radix(p.trim(), 16).ok()?,
    })
}

// ── detail + console ────────────────────────────────────────────────────────────────────────

pub async fn detail(Path(name): Path<String>) -> Response {
    let lv = Libvirt::system();
    let state = lv.state(&name);
    if matches!(state, DomainState::Absent) {
        return ui::page(
            "stations",
            &name,
            html! {
                div.banner.error { "No station named “" (name) "”." }
                a.btn href="/stations" { "← Back to stations" }
            },
        )
        .into_response();
    }
    let running = matches!(state, DomainState::Running);
    ui::page("stations", &name, html! {
        div style="display:flex; align-items:center; gap:12px; margin-bottom:16px" {
            a.btn.sm href="/stations" { "←" }
            h1 style="margin:0; font-size:1.3rem" { (name) }
            (ui::state_pill(state))
            div.spacer style="flex:1" {}
            div.actions {
                @if running {
                    (action_btn(&name, "stop", "Shut down", false))
                    (action_btn(&name, "forceoff", "Force off", true))
                } @else {
                    (action_btn(&name, "start", "Start", false))
                }
                (delete_btn(&name))
            }
        }
        (ui::panel("Console", None, html! {
            @if running {
                @let ep = vnc_endpoint(&name);
                div.pad {
                    div.console style="position:relative" {
                        div id="screen" {}
                        div id="console-status" style="position:absolute; inset:0; display:flex; align-items:center; justify-content:center; color:#8b97a6; font-size:14px; pointer-events:none" { "Connecting to console\u{2026}" }
                    }
                    div style="margin:.7rem 0 0; display:flex; gap:12px; flex-wrap:wrap; align-items:center" {
                        button.btn.sm hx-post=(format!("/stations/{name}/sendenter")) hx-swap="none"
                            title="Nudges a Windows installer past the 'press any key to boot from CD' prompt" { "Send Enter" }
                        span.sub { "A station with no OS (or stuck at firmware) shows black — this is live VNC, not an error." }
                        @if let Some((host, disp, port)) = &ep {
                            span.badge.mono { "VNC " (host) ":" (port) " (display " (disp) ")" }
                        }
                    }
                }
                script type="module" { (maud::PreEscaped(console_script(&name))) }
            } @else {
                div.emptybox { "The station is not running. Start it to open the console." }
            }
        }))
        (ui::panel("USB devices", None, usb_fragment(&lv, &name)))
    })
    .into_response()
}

/// The USB passthrough panel: what's attached (with Remove), and what's available to add. Swapped in
/// place by the add/remove actions.
fn usb_fragment(lv: &Libvirt, name: &str) -> Markup {
    let attached = lv.usb_devices(name);
    let connected = usb::devices();
    let friendly = |v: u16, p: u16| -> Option<String> {
        connected
            .iter()
            .find(|d| d.vendor_id == v && d.product_id == p)
            .and_then(|d| d.product.clone())
    };
    let addable: Vec<&usb::UsbDevice> = connected
        .iter()
        .filter(|d| {
            !attached
                .iter()
                .any(|(v, p)| *v == d.vendor_id && *p == d.product_id)
        })
        .collect();
    html! {
        div #usb {
            div.pad {
                p.sub style="margin:0 0 8px" { "Passed through to this station:" }
                @if attached.is_empty() {
                    p.muted { "None." }
                } @else {
                    @for (v, p) in &attached {
                        @let id = format!("{v:04x}:{p:04x}");
                        div style="display:flex; align-items:center; gap:10px; padding:5px 0; border-bottom:1px solid var(--line)" {
                            span { (friendly(*v, *p).as_deref().unwrap_or("USB device")) " " span.sub.mono { "(" (id) ")" } }
                            div style="flex:1" {}
                            button.btn.sm.danger hx-post=(format!("/stations/{name}/usb/remove/{id}")) hx-target="#usb" hx-swap="outerHTML" { "Remove" }
                        }
                    }
                }
                @if !addable.is_empty() {
                    p.sub style="margin:16px 0 8px" { "Available on the host — add one:" }
                    @for d in &addable {
                        @let id = format!("{:04x}:{:04x}", d.vendor_id, d.product_id);
                        div style="display:flex; align-items:center; gap:10px; padding:5px 0" {
                            span { (d.product.as_deref().unwrap_or("USB device")) " " span.sub.mono { "(" (id) ")" } }
                            div style="flex:1" {}
                            button.btn.sm hx-post=(format!("/stations/{name}/usb/add/{id}")) hx-target="#usb" hx-swap="outerHTML" { "Add" }
                        }
                    }
                }
            }
        }
    }
}

/// Tap Enter on a station's console (e.g. to clear the "press any key to boot from CD" prompt).
pub async fn send_enter(Path(name): Path<String>) -> StatusCode {
    let _ = Libvirt::system().send_key(&name, 28);
    StatusCode::NO_CONTENT
}

pub async fn usb_add(Path((name, id)): Path<(String, String)>) -> Markup {
    usb_op(&name, &id, true)
}

pub async fn usb_remove(Path((name, id)): Path<(String, String)>) -> Markup {
    usb_op(&name, &id, false)
}

fn usb_op(name: &str, id: &str, add: bool) -> Markup {
    let lv = Libvirt::system();
    let err = match parse_usb_id(id) {
        Some(u) if add => lv.attach_usb(name, u.vendor_id, u.product_id).err(),
        Some(u) => lv.detach_usb(name, u.vendor_id, u.product_id).err(),
        None => Some(std::io::Error::other("invalid USB id")),
    };
    html! {
        @if let Some(e) = err { div.banner.error { (e.to_string()) } }
        (usb_fragment(&lv, name))
    }
}

fn console_script(name: &str) -> String {
    format!(
        r#"import RFB from '/assets/novnc/core/rfb.js';
const screen = document.getElementById('screen');
const statusEl = document.getElementById('console-status');
const say = (m) => {{ if (statusEl) statusEl.textContent = m; }};
try {{
  const proto = location.protocol === 'https:' ? 'wss://' : 'ws://';
  const rfb = new RFB(screen, proto + location.host + '/stations/{name}/vnc');
  rfb.scaleViewport = true;
  rfb.background = '#000';
  rfb.addEventListener('connect', () => say(''));
  rfb.addEventListener('disconnect', (e) =>
    say((e.detail && e.detail.clean) ? 'Console closed.' : 'Console connection lost — reload to reconnect.'));
  rfb.addEventListener('securityfailure', (e) =>
    say('Auth failed: ' + ((e.detail && e.detail.reason) || 'unknown')));
}} catch (err) {{
  say('Console failed to start: ' + (err && err.message ? err.message : err));
}}
"#
    )
}

/// WebSocket ↔ VNC TCP proxy (a minimal websockify) so the browser noVNC client can reach the
/// domain's localhost VNC server.
pub async fn vnc_ws(Path(name): Path<String>, ws: WebSocketUpgrade) -> Response {
    match vnc_port(&name) {
        Some(port) => ws.on_upgrade(move |sock| relay(sock, port)),
        None => (StatusCode::NOT_FOUND, "no VNC display for this domain").into_response(),
    }
}

fn vnc_port(name: &str) -> Option<u16> {
    let out = ui::run_stdout("virsh", &["-c", "qemu:///system", "vncdisplay", name])?;
    let disp = out.trim();
    // ":0", "127.0.0.1:0", "127.0.0.1:0,tls-port"... take the display number after the last ':'.
    let n: u16 = disp
        .rsplit(':')
        .next()?
        .split(',')
        .next()?
        .trim()
        .parse()
        .ok()?;
    Some(5900 + n)
}

/// The VNC endpoint for display in the console: (host address, `:N` display, TCP port).
/// The server binds VNC to the host's loopback by default, so a native viewer needs an SSH tunnel
/// (`ssh -L PORT:127.0.0.1:PORT host`); the in-browser console proxies it for you.
fn vnc_endpoint(name: &str) -> Option<(String, String, u16)> {
    let port = vnc_port(name)?;
    let host = ui::run_stdout("hostname", &["-I"])
        .unwrap_or_default()
        .split_whitespace()
        .next()
        .unwrap_or("127.0.0.1")
        .to_string();
    Some((host, format!(":{}", port - 5900), port))
}

async fn relay(mut socket: WebSocket, port: u16) {
    let mut tcp = match TcpStream::connect(("127.0.0.1", port)).await {
        Ok(t) => t,
        Err(_) => {
            let _ = socket.send(Message::Close(None)).await;
            return;
        }
    };
    let mut buf = vec![0u8; 32 * 1024];
    loop {
        tokio::select! {
            msg = socket.recv() => match msg {
                Some(Ok(Message::Binary(b))) => { if tcp.write_all(&b).await.is_err() { break; } }
                Some(Ok(Message::Text(t))) => { if tcp.write_all(t.as_bytes()).await.is_err() { break; } }
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                Some(Ok(_)) => {}
            },
            read = tcp.read(&mut buf) => match read {
                Ok(0) | Err(_) => break,
                Ok(n) => { if socket.send(Message::Binary(buf[..n].to_vec())).await.is_err() { break; } }
            },
        }
    }
}

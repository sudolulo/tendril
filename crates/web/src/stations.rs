//! Stations: list, the create wizard (form → `provision`), detail with a live noVNC console, and
//! lifecycle actions. Everything routes through the shared `orchestrator::provision` service.

use std::path::Path as FsPath;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::Path;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};
use axum::Form;
use maud::{html, Markup};
use serde::Deserialize;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use tendril_capability_engine::{detect, iommu, pci};
use tendril_orchestrator::guest::{build_kickstart_seed, build_seed_iso};
use tendril_orchestrator::{
    provision, DomainState, GuestOs, InstallMedia, KickstartSpec, Libvirt, StationRequest,
    UnattendSpec,
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
    act(|lv| lv.undefine(&n))
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
                    div.field { label { "Disk image" } input name="disk" value=(format!("{DISK_DIR}/station1.qcow2")); }
                    div.field { label { "Disk size (GiB)" } input name="size_gib" value="128" inputmode="numeric"; }
                    div.field { label { "vCPUs" } input name="vcpus" value="8" inputmode="numeric"; }
                    div.field { label { "Memory (MiB)" } input name="memory_mib" value="16384" inputmode="numeric"; }
                    div.field.wide { label { "Install ISO" } input name="iso" value=(format!("{ISO_DIR}/win11.iso")); span.hint { "Windows: win11.iso · SteamOS: bazzite-deck-nvidia.iso" } }
                    div.field.wide { label { "virtio-win ISO (Windows only)" } input name="virtio_iso" value=(format!("{ISO_DIR}/virtio-win.iso")); }
                    div.field.check.wide { input type="checkbox" name="unattend" id="unattend" checked; label for="unattend" { "Install unattended (hands-off)" } }
                    div.field { label { "Username" } input name="username" value="player"; }
                    div.field { label { "Password" } input name="password" value="tendril"; }
                    div.field.wide { label { "Computer name / hostname" } input name="hostname" value="STATION1"; }
                    div.field.check { input type="checkbox" name="native" id="native"; label for="native" { "Native-hardware overlay (anti-cheat; may violate ToS)" } }
                    div.field.check { input type="checkbox" name="start" id="start" checked; label for="start" { "Start now (begins the install)" } }
                    div.field.wide { div.btnrow { button.btn.primary type="submit" { "Create station" } a.btn href="/stations" { "Cancel" } } }
                }
            }))
        },
    )
}

#[derive(Deserialize)]
pub struct CreateForm {
    name: String,
    os: String,
    gpu: String,
    disk: String,
    size_gib: String,
    vcpus: String,
    memory_mib: String,
    iso: String,
    virtio_iso: String,
    #[serde(default)]
    unattend: Option<String>,
    username: String,
    password: String,
    hostname: String,
    #[serde(default)]
    native: Option<String>,
    #[serde(default)]
    start: Option<String>,
}

pub async fn create(Form(f): Form<CreateForm>) -> Response {
    let guest = if f.os == "steamos" {
        GuestOs::SteamOs
    } else {
        GuestOs::Windows
    };
    let name = f.name.trim().to_string();
    if name.is_empty() {
        return create_form(Some("Station name is required.")).into_response();
    }
    let disk = if f.disk.trim().is_empty() {
        format!("{DISK_DIR}/{name}.qcow2")
    } else {
        f.disk.trim().to_string()
    };

    let seed_iso = if f.unattend.is_some() {
        match build_seed(guest, &name, &disk, &f.username, &f.password, &f.hostname) {
            Ok(p) => Some(p),
            Err(e) => {
                return create_form(Some(&format!("Building the seed ISO failed: {e}")))
                    .into_response()
            }
        }
    } else {
        None
    };

    let req = StationRequest {
        name: name.clone(),
        guest,
        disk_path: disk.clone(),
        size_gib: f.size_gib.trim().parse().unwrap_or(128),
        create_disk: !FsPath::new(&disk).exists(),
        vcpus: f.vcpus.trim().parse().unwrap_or(8),
        memory_mib: f.memory_mib.trim().parse().unwrap_or(16384),
        native_hardware: f.native.is_some(),
        passthrough_addresses: passthrough_for(f.gpu.trim()),
        media: InstallMedia {
            install_iso: nonempty(&f.iso),
            virtio_iso: if matches!(guest, GuestOs::Windows) {
                nonempty(&f.virtio_iso)
            } else {
                None
            },
            seed_iso,
        },
        define: true,
        start: f.start.is_some(),
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

fn nonempty(s: &str) -> Option<String> {
    let s = s.trim();
    if s.is_empty() {
        None
    } else {
        Some(s.to_string())
    }
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
                    (action_btn(&name, "delete", "Delete", true))
                }
            }
        }
        (ui::panel("Console", None, html! {
            @if running {
                div.pad {
                    div.console { div id="screen" {} }
                    p.sub style="margin:.6rem 0 0" { "Live VNC. During an unattended install this is where you watch it run." }
                }
                script type="module" { (maud::PreEscaped(console_script(&name))) }
            } @else {
                div.emptybox { "The station is not running. Start it to open the console." }
            }
        }))
    })
    .into_response()
}

fn console_script(name: &str) -> String {
    format!(
        r#"import RFB from '/assets/novnc/core/rfb.js';
const proto = location.protocol === 'https:' ? 'wss://' : 'ws://';
const url = proto + location.host + '/stations/{name}/vnc';
const rfb = new RFB(document.getElementById('screen'), url);
rfb.scaleViewport = true;
rfb.addEventListener('disconnect', () => {{
  document.getElementById('screen').innerHTML =
    '<div style=\"color:#8b97a6;padding:40px\">Console disconnected.</div>';
}});
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

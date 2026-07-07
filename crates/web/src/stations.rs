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
use tendril_orchestrator::guest::{build_kickstart_seed, build_seed_iso, create_overlay};
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
    if ui::is_demo() {
        return crate::demo::stations_fragment();
    }
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
    let has_gpu = !lv.pci_hostdevs(name).is_empty();
    html! {
        tr {
            td {
                a href=(format!("/stations/{name}")) { (name) }
                @if !has_gpu {
                    span title="No GPU passed through — this station has no graphics acceleration"
                        style="color:var(--crit); margin-left:6px; cursor:help" { "⚠" }
                }
            }
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
                    @let img_list = crate::images::list();
                    @if !img_list.is_empty() {
                        div.field.wide {
                            label { "Base image (clone a ready-to-play station instantly)" }
                            select name="base_image" {
                                option value="" { "None — install the OS fresh" }
                                @for (n, sz) in &img_list { option value=(n) { (n) " (" (sz) ")" } }
                            }
                            span.hint { "Pick a saved image to clone it (copy-on-write, instant); leave as None to install from media below." }
                        }
                    }
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
                    div.field { label { "Username" } input name="username" value="player"; }
                    div.field { label { "Password" } input name="password" value="tendril"; }
                    @let seat_list = crate::seats::load();
                    @if !seat_list.is_empty() {
                        div.field.wide {
                            label { "Seat (a saved group of USB devices — manage under Hardware)" }
                            select name="seat" {
                                option value="" { "None" }
                                @for s in &seat_list { option value=(s.name) { (s.name) " (" (s.devices.len()) " devices)" } }
                            }
                        }
                    }
                    @let usb_list = usb::devices();
                    @if !usb_list.is_empty() {
                        div.field.wide {
                            label { "Or pick individual USB devices (keyboard, mouse, controller)" }
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
                    details.advanced.wide {
                        summary { "Advanced options" }
                        div style="margin-top:14px; display:flex; flex-direction:column; gap:10px" {
                            div.field.check { input type="checkbox" name="unattend" id="unattend" checked; label for="unattend" { "Install unattended (hands-off)" } span.hint { "On by default — installs the guest OS without prompts using the account above. Uncheck for a manual install." } }
                            div.field.check { input type="checkbox" name="native" id="native"; label for="native" { "Native-hardware overlay (anti-cheat; may violate ToS)" } }
                            div.field.check { input type="checkbox" name="start" id="start" checked; label for="start" { "Start now (begins the install immediately)" } }
                        }
                        div.grid style="margin-top:12px" {
                            div.field { label { "Memory (MiB)" } input name="memory_mib" value=(ram) inputmode="numeric"; span.hint { "Auto: (host RAM − ~2 GiB host reserve) ÷ GPUs" } }
                            div.field { label { "vCPUs" } input name="vcpus" value=(vcpus) inputmode="numeric"; span.hint { "Auto: (host threads − 1) ÷ GPUs" } }
                            div.field { label { "Disk size (GiB)" } input name="size_gib" value=(disk) inputmode="numeric"; span.hint { "Auto: (free disk − ~20 GiB) ÷ GPUs" } }
                            div.field { label { "Disk image path" } input name="disk" placeholder=(format!("{DISK_DIR}/<name>.qcow2")); }
                            div.field.wide { label { "Install ISO (blank = the OS default)" } input name="iso" placeholder=(format!("{ISO_DIR}/win11.iso · bazzite-deck-nvidia.iso")); }
                            div.field.wide { label { "virtio-win ISO (Windows; blank = default)" } input name="virtio_iso" placeholder=(format!("{ISO_DIR}/virtio-win.iso")); }
                            div.field.wide { label { "Computer name / hostname" } input name="hostname" placeholder="defaults to the station name"; }
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
    let mut usb_devices: Vec<UsbPassthrough> = form
        .iter()
        .filter(|(k, _)| k == "usb")
        .filter_map(|(_, v)| parse_usb_id(v))
        .collect();
    // A chosen seat contributes its whole USB group.
    let seat = form
        .iter()
        .find(|(k, _)| k == "seat")
        .map(|(_, v)| v.trim().to_string())
        .unwrap_or_default();
    if !seat.is_empty() {
        usb_devices.extend(crate::seats::devices_of(&seat));
    }

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

    // Clone-from-image path: if a base image is chosen, the disk is a copy-on-write overlay of it and
    // there's no install step — just define the VM (which boots straight from the cloned disk).
    let base_image = get("base_image");
    if !base_image.is_empty() {
        let Some(base_path) = crate::images::path_of(&base_image) else {
            return create_form(Some("The selected base image no longer exists.")).into_response();
        };
        if FsPath::new(&disk).exists() {
            return create_form(Some(&format!("A disk already exists at {disk}."))).into_response();
        }
        if let Err(e) = create_overlay(FsPath::new(&disk), FsPath::new(&base_path)) {
            return create_form(Some(&format!("Cloning the image failed: {e}"))).into_response();
        }
        let (dram, dvcpus, _) = resource_defaults();
        let req = StationRequest {
            name: name.clone(),
            guest,
            disk_path: disk.clone(),
            size_gib: 0,
            create_disk: false,
            vcpus: get("vcpus").parse().unwrap_or(dvcpus),
            memory_mib: get("memory_mib").parse().unwrap_or(dram),
            native_hardware: checked("native"),
            passthrough_addresses: passthrough_for(&get("gpu")),
            media: InstallMedia::default(), // no install media — the domain boots from the cloned disk
            usb_devices,
            define: true,
            start: checked("start"),
        };
        let lv = Libvirt::system();
        return match provision(&req, &lv) {
            Ok(_) => Redirect::to(&format!("/stations/{name}")).into_response(),
            Err(e) => create_form(Some(&format!("Provisioning failed: {e}"))).into_response(),
        };
    }

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

    // Refuse install media that failed checksum verification (a `.mismatch` marker). Media with no
    // upstream checksum — only a recorded `.sha256`, e.g. the locally-assembled Windows ISO — is fine.
    for p in [
        req.media.install_iso.as_deref(),
        req.media.virtio_iso.as_deref(),
    ]
    .into_iter()
    .flatten()
    {
        if FsPath::new(p).exists() && verification_failed(p) {
            return create_form(Some(&format!(
                "{p} failed checksum verification — not using it. Re-fetch it from the Media page."
            )))
            .into_response();
        }
    }

    // Auto-fetch the OS install media if the default ISO(s) aren't downloaded yet. The fetch runs in
    // the background (it can be several GB and verifies as it goes); the station is provisioned only
    // once the media has arrived AND passed verification.
    if using_default_media(&req.media, guest) && media_missing(&req.media, guest) {
        let Some(script) = crate::pages::locate_script(fetch_script(guest)) else {
            return create_form(Some(
                "The install media isn't downloaded yet and the fetch script wasn't found — \
                 download it from the Media page first.",
            ))
            .into_response();
        };
        let req = req.clone();
        std::thread::spawn(move || fetch_then_provision(script, req, guest));
        return fetching_page(&name, guest).into_response();
    }

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

/// A media file whose checksum verification failed (has a `.mismatch` marker). No marker, or a
/// `.verified` / `.sha256` marker, is acceptable — it's fine to use media with no upstream checksum.
fn verification_failed(path: &str) -> bool {
    FsPath::new(&format!("{path}.mismatch")).exists()
}

fn fetch_script(guest: GuestOs) -> &'static str {
    match guest {
        GuestOs::Windows => "fetch-windows-media.sh",
        GuestOs::SteamOs => "fetch-steamos-media.sh",
    }
}

/// True when the request's install media are the OS defaults, so we know which fetcher produces them.
fn using_default_media(media: &InstallMedia, guest: GuestOs) -> bool {
    media.install_iso.as_deref() == Some(default_iso(guest).as_str())
}

/// True if a required install-media file isn't on disk yet.
fn media_missing(media: &InstallMedia, guest: GuestOs) -> bool {
    let absent = |p: &Option<String>| {
        p.as_deref()
            .map(|p| !FsPath::new(p).exists())
            .unwrap_or(false)
    };
    absent(&media.install_iso) || (matches!(guest, GuestOs::Windows) && absent(&media.virtio_iso))
}

/// Background worker: download the OS media (the fetch script verifies as it goes), then provision
/// the station — but only if every media file arrived and none is flagged as a checksum mismatch.
fn fetch_then_provision(script: String, req: StationRequest, guest: GuestOs) {
    let _ = std::process::Command::new(&script)
        .arg("--dest")
        .arg(ISO_DIR)
        .status();
    for p in [
        req.media.install_iso.as_deref(),
        req.media.virtio_iso.as_deref(),
    ]
    .into_iter()
    .flatten()
    {
        if !FsPath::new(p).exists() || verification_failed(p) {
            eprintln!(
                "station {}: install media {p} missing or failed verification — not provisioning",
                req.name
            );
            return;
        }
    }
    let _ = guest; // media set already reflects the guest; kept for signature symmetry
    let lv = Libvirt::system();
    if let Ok(report) = provision(&req, &lv) {
        if report.started && req.needs_boot_prompt_clear() {
            Libvirt::system().clear_boot_prompt(&req.name);
        }
    }
}

/// Shown after an auto-download kicks off: the station is created once its media is ready & verified.
fn fetching_page(name: &str, guest: GuestOs) -> Markup {
    let os = match guest {
        GuestOs::Windows => "Windows 11 + virtio-win",
        GuestOs::SteamOs => "Bazzite (SteamOS-style)",
    };
    ui::page(
        "stations",
        "Downloading media",
        html! {
            (ui::panel("Preparing station", None, html! {
                div.pad {
                    div.banner.ok { "Downloading " (os) " install media for station " strong { (name) } " — several GB." }
                    p { "The media is checked against the publisher's checksum as it downloads. Once it's ready "
                        "and verified, " strong { (name) } " is created automatically (and started, if you chose that). "
                        "If verification fails, the station is not created." }
                    p.sub { "Track progress on the " a href="/media" { "Media" } " page; the station appears under "
                        a href="/stations" { "Stations" } " when it's ready." }
                }
            }))
        },
    )
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
        .map(|b| b / (1 << 30)) // GiB (binary), matching the RAM/threads units
        .unwrap_or(256);

    // Split resources across one station per GPU, keeping only a *marginal* buffer for the host OS to
    // run (a lean bootc host plus libvirt/qemu overhead) — not a large proportional slice. Flat
    // reserves so a big host isn't over-reserved. Each station keeps a sane minimum.
    let ram = ((total_ram_mib.saturating_sub(2048) / num) / 1024).max(2) * 1024; // ~2 GiB for the host
    let vcpus = (threads.saturating_sub(1) / num).max(2) as u32; // 1 thread for the host
    let disk = (free_disk_gib.saturating_sub(20) / num).clamp(32, 1024) as u32; // ~20 GiB for the host
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
    if ui::is_demo() {
        return crate::demo::station_detail(&name).into_response();
    }
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
                        (progress_fragment(&name))
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
        (ui::panel("Save as image", Some("capture this station's disk as a reusable golden image"), html! {
            div.pad {
                @if running {
                    p.muted { "Shut the station down first — a consistent image can only be captured from a stopped disk." }
                } @else {
                    form hx-post=(format!("/stations/{name}/save-image")) hx-target="#save-result" hx-swap="innerHTML" {
                        div.field { label { "Image name" } input name="image_name" value=(format!("{name}-image")) required; }
                        button.btn.primary type="submit" style="margin-top:8px" { "Save as image" }
                    }
                    div #save-result style="margin-top:10px" {}
                    p.sub style="margin-top:8px" { "Flattened + compressed into " span.mono { (crate::images::images_dir()) } ". New stations can then clone it instantly from the create wizard." }
                }
            }
        }))
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

/// The station's OS disk path, from libvirt (the first `device='disk'` in its block list).
fn disk_path(name: &str) -> Option<String> {
    let out = ui::run_stdout(
        "virsh",
        &["-c", "qemu:///system", "domblklist", "--details", name],
    )?;
    out.lines().find_map(|l| {
        let c: Vec<&str> = l.split_whitespace().collect();
        (c.len() >= 4 && c[1] == "disk").then(|| c[3].to_string())
    })
}

/// Live install-progress: how much has been written to the station's disk so far. Polls itself.
pub async fn progress(Path(name): Path<String>) -> Markup {
    progress_fragment(&name)
}

fn progress_fragment(name: &str) -> Markup {
    let written = disk_path(name)
        .and_then(|p| std::fs::metadata(p).ok())
        .map(|m| m.len())
        .unwrap_or(0);
    html! {
        span #progress hx-get=(format!("/stations/{name}/progress")) hx-trigger="every 5s" hx-swap="outerHTML" {
            span.badge { "disk written: " span.mono { (format!("{:.1} GB", written as f64 / 1e9)) } }
        }
    }
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

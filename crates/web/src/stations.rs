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
use tendril_orchestrator::guest::{build_kickstart_seed_with, build_seed_iso_with, create_overlay};
use tendril_orchestrator::{
    provision, CpuPinning, DomainState, GuestAgentInfo, GuestApp, GuestOs, InstallMedia,
    KickstartSpec, Libvirt, StationRequest, UnattendSpec, UsbPassthrough,
};

// ── low-latency CPU pinning + hugepages (opt-in) ────────────────────────────────────────────────

/// Number of online host CPUs.
fn host_cpu_count() -> u32 {
    std::thread::available_parallelism()
        .map(|n| n.get() as u32)
        .unwrap_or(0)
}

/// Host CPUs already pinned by other defined domains (parsed from their `<vcpupin>`/`<emulatorpin>`
/// cpusets), so a new low-latency station doesn't share cores with an existing one.
fn cpus_taken_by_other_domains() -> std::collections::HashSet<u32> {
    let mut taken = std::collections::HashSet::new();
    let Some(list) = ui::run_stdout("virsh", &["list", "--all", "--name"]) else {
        return taken;
    };
    for dom in list.lines().map(str::trim).filter(|s| !s.is_empty()) {
        let Some(xml) = ui::run_stdout("virsh", &["dumpxml", dom]) else {
            continue;
        };
        for cs in xml.split("cpuset='").skip(1) {
            if let Some(end) = cs.find('\'') {
                parse_cpuset(&cs[..end], &mut taken);
            }
        }
    }
    taken
}

/// Parse a libvirt cpuset (`"4"`, `"4-7"`, `"0,2,4-6"`, with `^N`/`^A-B` exclusions) into the set of
/// CPUs it actually covers. Exclusions are applied after the additive tokens (libvirt semantics), so a
/// `^N` core is correctly treated as NOT taken rather than being counted.
fn parse_cpuset(spec: &str, out: &mut std::collections::HashSet<u32>) {
    let mut add = std::collections::HashSet::new();
    let mut remove = std::collections::HashSet::new();
    let expand = |tok: &str, set: &mut std::collections::HashSet<u32>| {
        if let Some((a, b)) = tok.split_once('-') {
            if let (Ok(a), Ok(b)) = (a.trim().parse::<u32>(), b.trim().parse::<u32>()) {
                for c in a..=b {
                    set.insert(c);
                }
            }
        } else if let Ok(c) = tok.parse::<u32>() {
            set.insert(c);
        }
    };
    for tok in spec.split(',') {
        let tok = tok.trim();
        if let Some(excl) = tok.strip_prefix('^') {
            expand(excl.trim(), &mut remove);
        } else {
            expand(tok, &mut add);
        }
    }
    out.extend(add.difference(&remove).copied());
}

/// Whether the host has a static hugepage pool allocated (else enabling `<hugepages/>` would stop the
/// VM from starting).
fn hugepages_available() -> bool {
    std::fs::read_to_string("/proc/meminfo")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("HugePages_Total"))
                .and_then(|l| l.split_whitespace().nth(1))
                .and_then(|n| n.parse::<u64>().ok())
        })
        .map(|n| n > 0)
        .unwrap_or(false)
}

/// Plan low-latency resources for a `vcpus`-wide station: pin its vCPUs 1:1 to free host cores
/// (reserving cores 0–1 for the host + QEMU emulator threads) and enable hugepages if a pool exists.
/// Returns `(None, hp)` when the host is too small or already fully pinned — the station then runs
/// unpinned rather than oversubscribing a core.
fn plan_low_latency(vcpus: u32) -> (Option<CpuPinning>, bool) {
    let hugepages = hugepages_available();
    let total = host_cpu_count();
    // Need the 2 reserved host cores plus one dedicated core per vCPU. saturating_add so an absurd
    // `vcpus` from the form can't overflow.
    if total < vcpus.saturating_add(2) {
        return (None, hugepages);
    }
    let taken = cpus_taken_by_other_domains();
    let free: Vec<u32> = (2..total).filter(|c| !taken.contains(c)).collect();
    if (free.len() as u32) < vcpus {
        return (None, hugepages);
    }
    let cores = &free[..vcpus as usize];
    (CpuPinning::new(vcpus, cores, &[0, 1]), hugepages)
}
use tendril_provisioning::{PassthroughStrategy, ProvisioningStrategy};

use crate::ui;

const DISK_DIR: &str = "/var/lib/tendril"; // station disks stay local (per-node, fast)
                                           // ISO paths resolve through `storage::iso_dir()` (local, or a mounted NFS/SMB share).

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
    use crate::federation as fed;
    // Single node: the classic flat list.
    if !fed::enabled() {
        return ui::page(
            "stations",
            "Stations",
            html! {
                (crate::pages::overview_strip())
                div.btnrow style="margin-bottom:16px" {
                    a.btn.primary href="/stations/new" { "+ New station" }
                }
                (ui::panel("Stations", None, fragment(&Libvirt::system())))
            },
        );
    }
    // Fleet: every station across the fleet, grouped by node. This node is interactive; each peer's
    // stations are shown read-only (they're managed on that peer). The demo uses its synthetic fleet.
    let local = fed::node_name();
    let peers: Vec<_> = if ui::is_demo() {
        fed::demo_fleet()
    } else {
        tokio::task::spawn_blocking(fed::fleet)
            .await
            .unwrap_or_default()
            .into_iter()
            .filter(|n| n.name != local)
            .collect()
    };
    ui::page(
        "stations",
        "Stations",
        html! {
            (crate::pages::overview_strip())
            div.btnrow style="margin-bottom:16px" {
                a.btn.primary href="/stations/new" { "+ New station" }
            }
            p.sub style="margin-bottom:16px" {
                "Every station across the fleet — start, stop, and delete any of them from here, on this "
                "node or any peer. Machines and health live on the "
                a href="/fleet" { "Fleet" } " page."
            }
            (ui::panel(&format!("{local} · this node"), None, fragment(&Libvirt::system())))
            @for n in &peers { (fed::stations_peer_panel(n)) }
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
    let err = lifecycle(&n, "delete").err();
    html! {
        @if let Some(e) = err { div.banner.error { (e) } }
        (fragment(&Libvirt::system()))
    }
}

/// Perform a lifecycle action on a **local** station by name — the shared core behind both the local
/// UI handlers and the federation API (so a peer can drive it from its Stations page). `delete` runs
/// the full teardown (force off, undefine, release any vGPU mdev, forget it in the shared registry).
pub(crate) fn lifecycle(name: &str, action: &str) -> Result<(), String> {
    // The name becomes a bare `virsh <sub> <name>` argv token; reject a leading-dash / out-of-charset
    // name so it can't be parsed as an option (this path is reachable from a token-authed peer via
    // api_station_action, where the name isn't otherwise validated).
    if !valid_station_name(name) {
        return Err("invalid station name".into());
    }
    let lv = Libvirt::system();
    let e = |r: std::io::Result<()>| r.map_err(|e| e.to_string());
    match action {
        "start" => e(lv.start(name)),
        "stop" => e(lv.shutdown(name)),
        "forceoff" => e(lv.destroy(name)),
        "delete" => {
            // Capture any vGPU mdev before the definition is gone, then tear it down too.
            let mdev = station_mdev_uuid(name);
            let _ = lv.destroy(name); // force off if running (ignored if already stopped)
            let r = e(lv.undefine(name));
            if let Some(uuid) = mdev {
                crate::vgpu::remove_mdev(&uuid);
            }
            crate::federation::forget_station(&crate::federation::node_name(), name);
            r
        }
        _ => Err(format!("unknown station action: {action}")),
    }
}

/// The UUID of a station's attached mediated device (vGPU), if it has one, read from its domain XML.
pub(crate) fn station_mdev_uuid(name: &str) -> Option<String> {
    let xml = ui::run_stdout("virsh", &["-c", "qemu:///system", "dumpxml", name])?;
    mdev_uuid_from_xml(&xml)
}

/// Find `<address uuid='...'/>` inside the mdev hostdev of an already-fetched domain XML.
pub(crate) fn mdev_uuid_from_xml(xml: &str) -> Option<String> {
    let after = xml.split("type='mdev'").nth(1)?;
    let start = after.find("uuid='")? + "uuid='".len();
    let end = after[start..].find('\'')? + start;
    Some(after[start..end].to_string())
}

/// Change a station's vGPU split **without touching its disk** — the qcow2 (Windows/games/saves) is
/// kept as-is. The station must be stopped: we create the new slice on the host, repoint the persistent
/// domain definition at it (swapping just the mdev UUID — everything else, including the disk, stays
/// identical), and it boots into the new split. The guest driver already matches the host driver branch,
/// which a re-split does not change, so no driver reinstall is needed. On failure nothing is lost: the
/// new slice is rolled back and the old definition is untouched.
///
/// NOTE: needs validation on real vGPU hardware (this is the experimental vGPU path).
pub(crate) fn resplit(name: &str, new_gpu_sel: &str) -> Result<(), String> {
    let lv = Libvirt::system();
    if matches!(lv.state(name), DomainState::Running | DomainState::Paused) {
        return Err("Shut the station down before changing its GPU split.".into());
    }
    let (parent, type_id) = crate::vgpu::parse_mdev_selection(new_gpu_sel)
        .ok_or("Pick a vGPU profile to change to.")?;
    let old_uuid = station_mdev_uuid(name)
        .ok_or("This station has no vGPU slice to change (it's whole-GPU or has none).")?;

    // New slice on the host first, so a failure here changes nothing on the station.
    let new_uuid = crate::vgpu::create_mdev(&parent, &type_id)?;

    // Repoint the persistent definition at the new mdev — swap only the UUID, keeping the disk and
    // every other device identical (no disk recreation ⇒ user data preserved).
    let xml = match ui::run_stdout("virsh", &["-c", "qemu:///system", "dumpxml", name]) {
        Some(x) if x.contains(&format!("uuid='{old_uuid}'")) => x,
        _ => {
            crate::vgpu::remove_mdev(&new_uuid);
            return Err("couldn't locate the current vGPU slice in the station definition".into());
        }
    };
    let new_xml = xml.replace(&format!("uuid='{old_uuid}'"), &format!("uuid='{new_uuid}'"));
    if let Err(e) = lv.define(name, &new_xml) {
        crate::vgpu::remove_mdev(&new_uuid); // roll back the freshly-created slice
        return Err(format!("couldn't update the station definition: {e}"));
    }

    // Committed — release the old slice.
    if old_uuid != new_uuid {
        crate::vgpu::remove_mdev(&old_uuid);
    }
    Ok(())
}

/// POST handler: re-split a station into a new vGPU profile (data-preserving).
pub async fn resplit_action(
    Path(name): Path<String>,
    Form(form): Form<std::collections::HashMap<String, String>>,
) -> Markup {
    if ui::is_demo() {
        return html! { div.banner.warn { "Actions are disabled in the live demo." } };
    }
    let sel = form.get("gpu").cloned().unwrap_or_default();
    let n = name.clone();
    let res = tokio::task::spawn_blocking(move || resplit(&n, &sel))
        .await
        .unwrap_or_else(|_| Err("re-split task panicked".into()));
    match res {
        Ok(()) => html! { div.banner.ok {
            "GPU split changed — your disk and data are untouched. Start the station to boot into the new split."
        } },
        Err(e) => html! { div.banner.error { (e) } },
    }
}

// ── in-guest agent status ───────────────────────────────────────────────────────────────────────

/// The "Guest" panel: what the in-VM QEMU agent reports (OS, hostname, IPs) — health/telemetry for a
/// running station. Empty/hint when the agent isn't connected yet.
fn guest_panel(info: &GuestAgentInfo, running: bool) -> Markup {
    ui::panel(
        "Guest",
        Some("in-VM agent — OS, hostname, IP"),
        html! {
            div.pad {
                @if !running {
                    p.sub style="margin:0" { "Start the station to see guest details." }
                } @else if info.connected {
                    table { tbody {
                        tr { td.sub style="white-space:nowrap" { "Agent" } td { span.pill.running { span.led {} "connected" } } }
                        @if let Some(os) = &info.os { tr { td.sub { "OS" } td { (os) } } }
                        @if let Some(h) = &info.hostname { tr { td.sub { "Hostname" } td { (h) } } }
                        @if !info.ips.is_empty() {
                            tr { td.sub { "IP" } td { @for ip in &info.ips { span.mono { (ip) } " " } } }
                        }
                    } }
                } @else {
                    p.sub style="margin:0" {
                        "No guest agent response yet. New stations install it automatically (QEMU guest agent); "
                        "once it's up the guest reports its OS, hostname and IP here and can be shut down gracefully."
                    }
                }
            }
        },
    )
}

/// The "Remote play" panel: how to stream this station to another device with Moonlight. Seatless
/// stations run Sunshine by default; this surfaces the station's IP (from the guest agent) + the
/// pairing steps, and WAN guidance. This is the "play any station from anywhere" entry point.
fn remote_play_panel(info: &GuestAgentInfo, running: bool) -> Markup {
    ui::panel(
        "Remote play",
        Some("stream this station to another device (Moonlight)"),
        html! {
            div.pad {
                @if !running {
                    p.sub style="margin:0" { "Start the station to stream it." }
                } @else {
                    p.sub style="margin-top:0" {
                        "This station streams over " b { "Sunshine" } " (installed by default on seatless stations). "
                        "On the device you want to play from, install " b { "Moonlight" } " and add this PC:"
                    }
                    @if let Some(ip) = info.ips.first() {
                        pre.mono style="margin:0; padding:8px 10px; background:var(--bg2,#0002); border-radius:6px; font-size:13px" { (ip) }
                        p.sub style="margin:8px 0 0" {
                            "Moonlight → " b { "Add PC" } " → " code { (ip) } ". Sunshine shows a PIN — enter it to pair, then "
                            "launch Desktop or a game."
                        }
                    } @else {
                        p.sub style="margin:0" { "Waiting for the station's IP (the guest agent reports it once the VM is up)." }
                    }
                    details style="margin-top:12px" {
                        summary.sub style="cursor:pointer" { "Play over the internet (WAN)" }
                        div style="margin-top:8px" {
                            p.sub style="margin:0" {
                                "Easiest + safest: put the playing device and this station on the same "
                                b { "mesh VPN" } " (Tailscale / WireGuard) and use the station's VPN IP above — no ports opened. "
                                "Otherwise forward Sunshine's ports on your router to " code { (info.ips.first().map(String::as_str).unwrap_or("the station IP")) }
                                ": TCP 47984/47989/48010 and UDP 47998–48000, then add your public IP in Moonlight."
                            }
                        }
                    }
                }
            }
        },
    )
}

// ── snapshots (restore points) ──────────────────────────────────────────────────────────────────

/// Sanitize a user-supplied snapshot name to something virsh accepts (alnum, dash, underscore, dot).
fn clean_snap_name(s: &str) -> String {
    s.trim()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || "-_.".contains(c) {
                c
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_string()
}

fn snapshots_fragment(lv: &Libvirt, name: &str, banner: Option<Markup>) -> Markup {
    let snaps = lv.snapshots(name);
    let running = matches!(lv.state(name), DomainState::Running | DomainState::Paused);
    html! {
        div #snap-list {
            @if let Some(b) = banner { (b) }
            div.pad {
                p.sub style="margin-top:0" {
                    "A restore point for this station's disk — snapshot before a risky change (a Windows "
                    "update, a new anti-cheat) and roll back instantly if it breaks."
                }
                form hx-post=(format!("/stations/{name}/snapshot")) hx-target="#snap-list" hx-swap="outerHTML"
                    style="display:flex; gap:8px; align-items:center; flex-wrap:wrap; margin-bottom:10px" {
                    input name="snap" placeholder="restore-point name" required style="width:16em";
                    button.btn.primary type="submit" { "Take snapshot" }
                    @if running { span.sub { "Tip: shut the station down first for a clean snapshot." } }
                }
                @if snaps.is_empty() {
                    div.emptybox { "No snapshots yet." }
                } @else {
                    table.list { tbody {
                        @for s in &snaps {
                            tr {
                                td { b { (s.name) } }
                                td.sub { (s.created) }
                                td.sub { (s.state) }
                                td style="text-align:right; white-space:nowrap" {
                                    form.inline style="display:inline" hx-post=(format!("/stations/{name}/snapshot/revert"))
                                        hx-target="#snap-list" hx-swap="outerHTML"
                                        hx-confirm=(format!("Roll {name} back to “{}”? Changes since the snapshot are lost.", s.name)) {
                                        input type="hidden" name="snap" value=(s.name);
                                        button.btn.sm type="submit" { "Restore" }
                                    }
                                    " "
                                    form.inline style="display:inline" hx-post=(format!("/stations/{name}/snapshot/delete"))
                                        hx-target="#snap-list" hx-swap="outerHTML"
                                        hx-confirm=(format!("Delete snapshot “{}”?", s.name)) {
                                        input type="hidden" name="snap" value=(s.name);
                                        button.btn.sm.danger type="submit" { "Delete" }
                                    }
                                }
                            }
                        }
                    } }
                }
            }
        }
    }
}

/// The "Snapshots" panel on a station's detail page.
fn snapshots_panel(lv: &Libvirt, name: &str) -> Markup {
    ui::panel(
        "Snapshots",
        Some("restore points — roll back a bad update instantly"),
        snapshots_fragment(lv, name, None),
    )
}

#[derive(serde::Deserialize)]
pub struct SnapForm {
    #[serde(default)]
    snap: String,
}

/// Run a snapshot action off-thread and re-render the list with a success/error banner.
async fn snapshot_action(name: String, snap: String, verb: &'static str) -> Markup {
    if ui::is_demo() {
        return html! { div.banner.warn { "Actions are disabled in the live demo." } };
    }
    let snap = clean_snap_name(&snap);
    let lv = Libvirt::system();
    if snap.is_empty() {
        return snapshots_fragment(
            &lv,
            &name,
            Some(html! { div.banner.error { "Enter a name." } }),
        );
    }
    let (n, s) = (name.clone(), snap.clone());
    let res = tokio::task::spawn_blocking(move || {
        let lv = Libvirt::system();
        match verb {
            "create" => lv.snapshot_create(&n, &s),
            "revert" => lv.snapshot_revert(&n, &s),
            "delete" => lv.snapshot_delete(&n, &s),
            _ => Ok(()),
        }
    })
    .await
    .unwrap_or_else(|_| Err(std::io::Error::other("snapshot task panicked")));
    let banner = match (&res, verb) {
        (Ok(()), "create") => html! { div.banner.ok { "Snapshot “" (snap) "” taken." } },
        (Ok(()), "revert") => html! { div.banner.ok { "Restored to “" (snap) "”." } },
        (Ok(()), _) => html! { div.banner.ok { "Snapshot “" (snap) "” deleted." } },
        (Err(e), _) => html! { div.banner.error { (e.to_string()) } },
    };
    snapshots_fragment(&lv, &name, Some(banner))
}

pub async fn snapshot_create(Path(name): Path<String>, Form(f): Form<SnapForm>) -> Markup {
    snapshot_action(name, f.snap, "create").await
}
pub async fn snapshot_revert(Path(name): Path<String>, Form(f): Form<SnapForm>) -> Markup {
    snapshot_action(name, f.snap, "revert").await
}
pub async fn snapshot_delete(Path(name): Path<String>, Form(f): Form<SnapForm>) -> Markup {
    snapshot_action(name, f.snap, "delete").await
}

/// The "GPU split" panel on a station's detail page — only for stations bound to a vGPU (mdev) slice.
/// Lets you re-slice the same GPU without recreating the disk. Returns None for non-vGPU stations.
fn resplit_panel(name: &str, running: bool) -> Option<Markup> {
    let uuid = station_mdev_uuid(name)?;
    let parent = crate::vgpu::mdev_parent(&uuid)?; // no parent found ⇒ don't offer a re-split
                                                   // Precompute the selectable profiles (value, label) so the markup stays simple.
    let sup = tendril_capability_engine::vgpu::probe(&parent);
    let opts: Vec<(String, String)> = sup
        .mdev_types
        .iter()
        .filter(|m| m.available > 0)
        .map(|m| {
            let label = m.name.clone().unwrap_or_else(|| m.id.clone());
            (format!("mdev:{parent}:{}", m.id), label)
        })
        .collect();
    Some(ui::panel(
        "GPU split",
        Some("change how this station's GPU is sliced — the disk and its data are kept"),
        html! {
            div.pad {
                @if running {
                    p.muted { "Shut the station down first — the split changes on the next boot." }
                } @else if opts.is_empty() {
                    p.sub { "No other vGPU profiles are currently available on the parent GPU." }
                } @else {
                    form hx-post=(format!("/stations/{name}/resplit")) hx-target="#resplit-result" hx-swap="innerHTML" {
                        div.field { label { "New profile" }
                            select name="gpu" {
                                @for (val, label) in &opts {
                                    option value=(val) { (label) }
                                }
                            }
                        }
                        button.btn.primary type="submit" style="margin-top:8px"
                            hx-confirm="Change this station's GPU split? Its disk and data are kept; it boots into the new split." { "Change split" }
                    }
                    div #resplit-result style="margin-top:10px" {}
                    p.sub style="margin-top:8px" { "Only the GPU slice changes — Windows, games, and saves on the disk are untouched." }
                }
            }
        },
    ))
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

/// Remote placement targets for the create form (names only, no network): a fleet's peer nodes.
/// Empty when there's no fleet, so the Placement selector doesn't appear on a lone node.
fn fleet_target_names() -> Vec<String> {
    if !crate::federation::enabled() {
        return Vec::new();
    }
    if ui::is_demo() {
        return crate::federation::demo_fleet()
            .into_iter()
            .filter(|n| n.reachable)
            .map(|n| n.name)
            .collect();
    }
    crate::federation::peers()
        .into_iter()
        .map(|p| p.name)
        .collect()
}

fn create_form(error: Option<&str>) -> Markup {
    let matrix = detect();
    // Whole-GPU passthrough and vGPU on the same physical GPU are mutually exclusive at the driver
    // level, so don't offer a GPU both ways: hide whole-GPU for a card already handing out vGPU
    // slices, and hide vGPU profiles for a card already passed through whole.
    let u = crate::hardware::usage();
    let (whole_used, vgpu_used) = (u.gpu, u.mdev);
    ui::page(
        "stations",
        "New station",
        html! {
            @if let Some(e) = error { div.banner.error { (e) } }
            (ui::panel("Create a gaming station", None, html! {
                @let (ram, vcpus, disk) = resource_defaults();
                @let peers = fleet_target_names();
                form #newstation.grid.pad method="post" action="/stations" {
                    div.field { label { "Station name" } input name="name" value="station1" required; }
                    // Placement: only shown once a fleet exists. Picking a peer switches the form to the
                    // fleet dispatcher (a peer auto-assigns a whole free GPU; local-only fields are hidden).
                    @if !peers.is_empty() {
                        div.field.wide {
                            label { "Placement" }
                            select #placement name="target" onchange="tendrilPlace()" {
                                option value="" { "This node — full options (specific GPU / vGPU, USB, seats)" }
                                @for nm in &peers { option value=(nm) { (nm) } }
                            }
                            span.hint.remote-note style="display:none" { "On another node a whole free GPU is auto-assigned; local-only options (specific GPU/vGPU, USB, seats, disk path, ISO) don't apply." }
                        }
                    }
                    @let img_list = crate::images::list();
                    @if !img_list.is_empty() {
                        div.field.wide {
                            label { "Base image (clone a ready-to-play station instantly)" }
                            select #base-image name="base_image" onchange="tendrilClone()" {
                                option value="" { "None — install the OS fresh" }
                                @for (n, sz) in &img_list {
                                    @let os = crate::images::image_os(n);
                                    @let osa = match os { Some(GuestOs::Windows) => "windows", Some(GuestOs::SteamOs) => "steamos", None => "" };
                                    @let oslabel = match os { Some(GuestOs::Windows) => " · Windows", Some(GuestOs::SteamOs) => " · SteamOS", None => "" };
                                    option value=(n) data-os=(osa) { (n) " (" (sz) ")" (oslabel) }
                                }
                            }
                            span.hint { "Pick a saved image to clone it (copy-on-write, instant) — the OS and install options come from the image. Leave as None to install fresh from media below." }
                        }
                    }
                    div.field.install-only {
                        label { "Guest OS" }
                        select #os-select name="os" {
                            option value="windows" { "Windows 11" }
                            option value="steamos" { "SteamOS (Bazzite)" }
                        }
                    }
                    div.field.wide.remote-hide {
                        label { "GPU" }
                        select name="gpu" {
                            option value="" { "None — headless / attach later" }
                            @for g in matrix.passthrough_capable() {
                                @if !vgpu_used.contains_key(&g.gpu.address) {
                                    option value=(g.gpu.address) {
                                        (ui::vendor(g.gpu.vendor)) " "
                                        (g.gpu.model.as_deref().unwrap_or("GPU")) " [" (g.gpu.address) "] — whole GPU"
                                    }
                                }
                            }
                            // vGPU: one option per available mdev profile (a slice of a shared GPU). The
                            // profile Tendril recommends for gaming (≥4 GB, most stations) is badged and
                            // pre-selected as the logical default (see vgpu::recommended_mdev).
                            @let rec_key = crate::vgpu::default_mdev_key(&matrix, &whole_used);
                            @for g in matrix.vgpu_capable() {
                                @if !whole_used.contains_key(&g.gpu.address) {
                                @let rec_i = crate::vgpu::recommended_mdev(&g.vgpu.mdev_types);
                                @for (i, t) in g.vgpu.mdev_types.iter().enumerate() {
                                    @if t.available > 0 {
                                        @let val = format!("{}{}:{}", crate::vgpu::MDEV_PREFIX, g.gpu.address, t.id);
                                        option value=(val) selected[rec_key.as_deref() == Some(val.as_str())] {
                                            (ui::vendor(g.gpu.vendor)) " "
                                            (g.gpu.model.as_deref().unwrap_or("GPU"))
                                            " — vGPU: " (t.name.as_deref().unwrap_or(t.id.as_str()))
                                            " (" (t.available) " free)"
                                            @if rec_i == Some(i) { " · recommended" }
                                        }
                                    }
                                }
                                }
                            }
                        }
                        span.hint { "Pick a whole GPU for full passthrough, or a vGPU profile to hand a station one slice of a shared GPU (requires an mdev-capable driver, e.g. NVIDIA vGPU). The " b { "recommended" } " vGPU profile is the gaming sweet spot for your card — ≥4 GB per station while splitting the card as far as it sensibly goes. SR-IOV virtual functions appear here as whole GPUs once enabled on the Hardware page." }
                    }
                    div.field.install-only { label { "Username" } input name="username" value="player"; }
                    div.field.install-only { label { "Password" } input name="password" value="tendril"; }
                    @let seat_list = crate::seats::load();
                    @if !seat_list.is_empty() {
                        div.field.wide.remote-hide {
                            label { "Seat (a saved group of USB devices — manage under Hardware)" }
                            select name="seat" {
                                option value="" { "None" }
                                @for s in &seat_list { option value=(s.name) { (s.name) " (" (s.devices.len()) " devices)" } }
                            }
                        }
                    }
                    @let usb_list = usb::devices();
                    @if !usb_list.is_empty() {
                        div.field.wide.remote-hide {
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
                            div.field.check.install-only { input type="checkbox" name="unattend" id="unattend" checked; label for="unattend" { "Install unattended (hands-off)" } span.hint { "On by default — installs the guest OS without prompts using the account above. Uncheck for a manual install." } }
                            div.field.check { input type="checkbox" name="native" id="native"; label for="native" { "Native-hardware overlay (anti-cheat; may violate ToS)" } }
                            div.field.check { input type="checkbox" name="low_latency" id="low_latency" checked; label for="low_latency" { "Low-latency (pin CPU cores + hugepages)" } span.hint { "Pins this station's vCPUs to dedicated host cores so gaming frame-times don't jitter. Skips pinning automatically if the host is too small." } }
                            div.field.check { input type="checkbox" name="start" id="start" checked; label for="start" { "Start now (begins the install immediately)" } }
                            div.field.check.install-only { input type="checkbox" name="app_steam" id="app_steam" checked; label for="app_steam" { "Install Steam" } }
                            div.field.check.install-only { input type="checkbox" name="app_sunshine" id="app_sunshine" checked; label for="app_sunshine" { "Sunshine — stream to Moonlight" } span.hint { "Recommended for a seatless station — otherwise there's no low-latency way to see it. Installs on Windows, enables Bazzite's on Linux." } }
                            div.field.check.install-only { input type="checkbox" name="app_discord" id="app_discord" checked; label for="app_discord" { "Install Discord" } }
                            div.field.check.install-only { input type="checkbox" name="app_moonlight" id="app_moonlight"; label for="app_moonlight" { "Moonlight — receive streams" } span.hint { "Installs the Moonlight client so this station can also play another station's games over the LAN (the receiver to Sunshine's host)." } }
                            @if crate::storage::store_root().is_some() {
                                div.field.check.install-only { input type="checkbox" name="steam_library" id="steam_library"; label for="steam_library" { "Shared Steam library (experimental)" } span.hint { "Shares the fleet store's steam-library/ folder into this station over virtio-fs — install games once, read from many. Update from one station at a time. See docs/STEAM-GAMES.md." } }
                            }
                        }
                        div.grid style="margin-top:12px" {
                            div.field { label { "Memory (MiB)" } input name="memory_mib" value=(ram) inputmode="numeric"; span.hint { "Auto: (host RAM − ~2 GiB host reserve) ÷ GPUs" } }
                            div.field { label { "vCPUs" } input name="vcpus" value=(vcpus) inputmode="numeric"; span.hint { "Auto: (host threads − 1) ÷ GPUs" } }
                            div.field.install-only { label { "Disk size (GiB)" } input name="size_gib" value=(disk) inputmode="numeric"; span.hint { "Auto: (free disk − ~20 GiB) ÷ GPUs" } }
                            div.field.install-only { label { "Persistent data volume (GiB)" } input name="data_gib" value="0" inputmode="numeric"; span.hint { "0 = none. A separate disk for games/saves that survives OS reinstalls and GPU re-splits." } }
                            div.field.install-only.remote-hide { label { "Disk image path" } input name="disk" placeholder=(format!("{DISK_DIR}/<name>.qcow2")); }
                            div.field.wide.install-only.remote-hide { label { "Install ISO (blank = the OS default)" } input name="iso" placeholder=(format!("{}/win11.iso · bazzite-deck-nvidia.iso", crate::storage::iso_dir())); }
                            div.field.wide.install-only.remote-hide { label { "virtio-win ISO (Windows; blank = default)" } input name="virtio_iso" placeholder=(format!("{}/virtio-win.iso", crate::storage::iso_dir())); }
                            div.field.wide.install-only { label { "Computer name / hostname" } input name="hostname" placeholder="defaults to the station name"; }
                        }
                    }
                    div.field.wide { div.btnrow { button.btn.primary type="submit" { "Create station" } a.btn href="/stations" { "Cancel" } } }
                    (maud::PreEscaped(
                        "<script>\
                         window.tendrilPlace=function(){\
                         var p=document.getElementById('placement');var f=document.getElementById('newstation');\
                         var remote=p&&p.value!=='';\
                         if(f){f.setAttribute('action',remote?'/fleet/create':'/stations');}\
                         document.querySelectorAll('.remote-hide').forEach(function(e){e.style.display=remote?'none':'';});\
                         var n=document.querySelector('.remote-note');if(n){n.style.display=remote?'':'none';}\
                         if(!remote&&window.tendrilClone){tendrilClone();}\
                         };\
                         window.tendrilClone=function(){\
                         var b=document.getElementById('base-image');if(!b)return;\
                         var o=b.options[b.selectedIndex];var cloning=b.value!=='';\
                         var p=document.getElementById('placement');var remote=p&&p.value!=='';\
                         document.querySelectorAll('.install-only').forEach(function(e){\
                           if(remote&&e.classList.contains('remote-hide'))return;\
                           e.style.display=cloning?'none':'';});\
                         var os=o&&o.getAttribute('data-os');var s=document.getElementById('os-select');\
                         if(cloning&&os&&s){s.value=os;}\
                         if(cloning&&!os&&s){var ff=s.closest('.install-only');if(ff)ff.style.display='';}\
                         };\
                         tendrilPlace();</script>"
                    ))
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
    if !valid_station_name(&name) {
        return create_form(Some(
            "Station name must be non-empty and contain only letters, numbers, - _ . (no slashes).",
        ))
        .into_response();
    }
    // The guest account fields are interpolated into the autounattend.xml / kickstart (and a root-run
    // %post shell) — validate them (same rules as the federation provision path).
    if let Err(e) = valid_guest_fields(&get("username"), &get("hostname"), &get("password")) {
        return create_form(Some(&e)).into_response();
    }
    let disk = {
        let d = get("disk");
        if d.is_empty() {
            format!("{DISK_DIR}/{name}.qcow2")
        } else {
            d
        }
    };
    // Never let a station's disk land in the golden-images directory — it would corrupt an image.
    if !disk_target_ok(&disk) {
        return create_form(Some(
            "The disk path can't be inside the images directory — that's reserved for golden images.",
        ))
        .into_response();
    }

    // Clone-from-image path: if a base image is chosen, the disk is a copy-on-write overlay of it and
    // there's no install step — just define the VM (which boots straight from the cloned disk). The
    // guest OS comes from the image (recorded at save time), not the wizard, so it can't be mismatched.
    // Shared Steam library (experimental): when on and a store is configured, share <store>/steam-library
    // into the station over virtio-fs (games installed once, read by many). See docs/STEAM-GAMES.md.
    let steam_lib = if checked("steam_library") {
        crate::storage::store_root().map(|r| {
            let d = format!("{r}/steam-library");
            let _ = std::fs::create_dir_all(&d);
            d
        })
    } else {
        None
    };

    let base_image = get("base_image");
    if !base_image.is_empty() {
        let Some(base_path) = crate::images::path_of(&base_image) else {
            return create_form(Some("The selected base image no longer exists.")).into_response();
        };
        let guest = crate::images::image_os(&base_image).unwrap_or(guest);
        if FsPath::new(&disk).exists() {
            return create_form(Some(&format!("A disk already exists at {disk}."))).into_response();
        }
        if let Err(e) = create_overlay(FsPath::new(&disk), FsPath::new(&base_path)) {
            return create_form(Some(&format!("Cloning the image failed: {e}"))).into_response();
        }
        let assign = match assign_gpu(&get("gpu")) {
            Ok(a) => a,
            Err(e) => {
                // Don't strand the freshly-created overlay — it would block a retry under the
                // same name with "a disk already exists".
                let _ = std::fs::remove_file(&disk);
                return create_form(Some(&e)).into_response();
            }
        };
        let (dram, dvcpus, _) = resource_defaults();
        let mut req = StationRequest {
            name: name.clone(),
            guest,
            disk_path: disk.clone(),
            size_gib: 0,
            create_disk: false,
            vcpus: get("vcpus").parse().unwrap_or(dvcpus),
            memory_mib: get("memory_mib").parse().unwrap_or(dram),
            native_hardware: checked("native"),
            passthrough_addresses: assign.passthrough.clone(),
            mdev_uuid: assign.mdev_uuid.clone(),
            media: InstallMedia::default(), // no install media — the domain boots from the cloned disk
            usb_devices,
            define: true,
            start: checked("start"),
            steam_library_dir: steam_lib.clone(),
            data_disk: None,
            cpu_pinning: None,
            hugepages: false,
        };
        // Low-latency applies to clones too — the checkbox is visible (and on by default) in
        // clone mode, so honoring it only on fresh installs would silently drop it here.
        if checked("low_latency") {
            let (pinning, hugepages) = plan_low_latency(req.vcpus);
            req.cpu_pinning = pinning;
            req.hugepages = hugepages;
        }
        let lv = Libvirt::system();
        return match provision(&req, &lv) {
            Ok(_) => {
                record_local(&name, guest, Some(&base_image));
                Redirect::to(&format!("/stations/{name}")).into_response()
            }
            Err(e) => {
                assign.cleanup();
                let _ = std::fs::remove_file(&disk);
                create_form(Some(&format!("Provisioning failed: {e}"))).into_response()
            }
        };
    }

    let seed_iso = if checked("unattend") {
        let mut apps = Vec::new();
        if checked("app_steam") {
            apps.push(GuestApp::Steam);
        }
        if checked("app_sunshine") {
            apps.push(GuestApp::Sunshine);
        }
        if checked("app_discord") {
            apps.push(GuestApp::Discord);
        }
        if checked("app_moonlight") {
            apps.push(GuestApp::Moonlight);
        }
        match build_seed(
            guest,
            &name,
            &disk,
            &get("username"),
            &get("password"),
            &get("hostname"),
            &get("gpu"),
            &apps,
            get("data_gib").parse::<u32>().unwrap_or(0) > 0,
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
            format!("{}/virtio-win.iso", crate::storage::iso_dir())
        } else {
            v
        })
    } else {
        None
    };
    let mut req = StationRequest {
        name: name.clone(),
        guest,
        disk_path: disk.clone(),
        size_gib: get("size_gib").parse().unwrap_or(ddisk),
        create_disk: !FsPath::new(&disk).exists(),
        vcpus: get("vcpus").parse().unwrap_or(dvcpus),
        memory_mib: get("memory_mib").parse().unwrap_or(dram),
        native_hardware: checked("native"),
        // GPU is resolved below, after media validation, so a vGPU mdev isn't created before a
        // possible early return.
        passthrough_addresses: Vec::new(),
        mdev_uuid: None,
        media: InstallMedia {
            install_iso,
            virtio_iso,
            seed_iso,
        },
        usb_devices,
        define: true,
        start: checked("start"),
        steam_library_dir: steam_lib.clone(),
        // Optional persistent data volume: a `<disk>-data.qcow2` sized by the wizard (0 = none). It
        // survives OS/base-image swaps and re-splits, so games/saves aren't lost when the OS is replaced.
        data_disk: {
            let gib: u32 = get("data_gib").parse().unwrap_or(0);
            // A custom disk path may not end in .qcow2 — still honor the requested volume rather
            // than silently dropping it.
            (gib > 0).then(|| {
                let base = disk.strip_suffix(".qcow2").unwrap_or(&disk);
                (format!("{base}-data.qcow2"), gib)
            })
        },
        // Low-latency pinning/hugepages resolved just below (needs vcpus, and to see cores other
        // stations already hold).
        cpu_pinning: None,
        hugepages: false,
    };
    // Low-latency mode (opt-in): pin this station's vCPUs to dedicated host cores and use hugepages
    // when a pool exists, so gaming frame-times don't jitter from host scheduling.
    if checked("low_latency") {
        let (pinning, hugepages) = plan_low_latency(req.vcpus);
        req.cpu_pinning = pinning;
        req.hugepages = hugepages;
    }

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

    // Resolve the GPU now that everything that could fail cheaply has passed — creating a vGPU mdev
    // (if chosen) just before we commit to provisioning.
    let assign = match assign_gpu(&get("gpu")) {
        Ok(a) => a,
        Err(e) => return create_form(Some(&e)).into_response(),
    };
    req.passthrough_addresses = assign.passthrough.clone();
    req.mdev_uuid = assign.mdev_uuid.clone();

    // Auto-fetch the OS install media if the default ISO(s) aren't downloaded yet. The fetch runs in
    // the background (it can be several GB and verifies as it goes); the station is provisioned only
    // once the media has arrived AND passed verification.
    if using_default_media(&req.media, guest) && media_missing(&req.media, guest) {
        let Some(script) = crate::pages::locate_script(fetch_script(guest)) else {
            assign.cleanup();
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
            record_local(&name, guest, None);
            if report.started && req.needs_boot_prompt_clear() {
                // Clear the Windows CD prompt without blocking the response.
                let n = name.clone();
                tokio::task::spawn_blocking(move || Libvirt::system().clear_boot_prompt(&n));
            }
            Redirect::to(&format!("/stations/{name}")).into_response()
        }
        Err(e) => {
            assign.cleanup();
            create_form(Some(&format!("Provisioning failed: {e}"))).into_response()
        }
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
        .arg(crate::storage::iso_dir())
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
            // Don't strand a vGPU mdev we created for this station.
            if let Some(u) = &req.mdev_uuid {
                crate::vgpu::remove_mdev(u);
            }
            return;
        }
    }
    let _ = guest; // media set already reflects the guest; kept for signature symmetry
    let lv = Libvirt::system();
    match provision(&req, &lv) {
        Ok(report) => {
            record_local(&req.name, guest, None);
            if report.started && req.needs_boot_prompt_clear() {
                Libvirt::system().clear_boot_prompt(&req.name);
            }
        }
        Err(e) => {
            eprintln!("station {}: provisioning failed: {e}", req.name);
            if let Some(u) = &req.mdev_uuid {
                crate::vgpu::remove_mdev(u);
            }
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

/// Build the hands-off install seed for a station.
///
/// `gpu` is the wizard's raw GPU selection (an `mdev:…` value marks an NVIDIA vGPU slice); `apps` are
/// the apps to bake in. For a Windows station bound to an NVIDIA vGPU with a guest driver staged, the
/// driver is copied onto the seed disc and the answer file installs it on first logon, plus the DLS
/// licensing token if the license server is running. Whole-GPU passthrough and non-Windows guests are
/// unaffected.
#[allow(clippy::too_many_arguments)] // a flat seed spec; the fields are all distinct scalars
fn build_seed(
    guest: GuestOs,
    name: &str,
    disk: &str,
    username: &str,
    password: &str,
    hostname: &str,
    gpu: &str,
    apps: &[GuestApp],
    data_volume: bool,
) -> std::io::Result<String> {
    let dir = FsPath::new(disk)
        .parent()
        .map(|p| p.to_string_lossy().into_owned())
        .filter(|p| !p.is_empty())
        .unwrap_or_else(|| ".".to_string());
    let seed = format!("{dir}/{name}-seed.iso");
    let path = FsPath::new(&seed);
    match guest {
        GuestOs::Windows => {
            // Inject the NVIDIA vGPU guest driver only when this station is bound to a vGPU (mdev)
            // slice — a whole-GPU-passthrough station gets its driver from Windows Update / the vendor
            // instead. Selection is automatic: `auto_windows_exe` picks the Windows guest `.exe`
            // matching the host driver branch, fetching it from NVIDIA's public bucket if not cached.
            let is_vgpu = gpu.starts_with(crate::vgpu::MDEV_PREFIX);
            let staged = is_vgpu.then(crate::vgpuguest::auto_windows_exe).flatten();
            let mut extras: Vec<(&str, &FsPath)> = Vec::new();
            let staged_path = staged.as_deref().map(FsPath::new);
            if let Some(p) = staged_path {
                extras.push((crate::vgpuguest::DISC_NAME, p));
            }
            let spec = UnattendSpec {
                computer_name: if hostname.trim().is_empty() {
                    name.to_uppercase()
                } else {
                    hostname.trim().to_string()
                },
                username: username.to_string(),
                password: password.to_string(),
                vgpu_driver_exe: staged
                    .as_ref()
                    .map(|_| crate::vgpuguest::DISC_NAME.to_string()),
                // The token is only useful once the guest driver is present.
                dls_token_url: staged
                    .as_ref()
                    .and_then(|_| crate::licensing::guest_token_url()),
                apps: apps.to_vec(),
                data_volume,
                ..UnattendSpec::default()
            };
            build_seed_iso_with(&spec, &extras, path)?
        }
        GuestOs::SteamOs => {
            // Inject the Linux vGPU guest driver for a station on a vGPU (mdev) slice. Selection is
            // automatic: `auto_linux_run` picks the release matching the host driver branch, fetching
            // it from NVIDIA's public bucket if it isn't already staged — the user never picks a driver.
            // It rides the OEMDRV kickstart seed and a first-boot service installs it.
            let is_vgpu = gpu.starts_with(crate::vgpu::MDEV_PREFIX);
            let staged = is_vgpu.then(crate::vgpuguest::auto_linux_run).flatten();
            let mut extras: Vec<(&str, &FsPath)> = Vec::new();
            let staged_path = staged.as_deref().map(FsPath::new);
            if let Some(p) = staged_path {
                extras.push((crate::vgpuguest::LINUX_DISC_NAME, p));
            }
            let spec = KickstartSpec {
                hostname: if hostname.trim().is_empty() {
                    name.to_string()
                } else {
                    hostname.trim().to_string()
                },
                username: username.to_string(),
                password: password.to_string(),
                vgpu_guest_run: staged
                    .as_ref()
                    .map(|_| crate::vgpuguest::LINUX_DISC_NAME.to_string()),
                dls_token_url: staged
                    .as_ref()
                    .and_then(|_| crate::licensing::guest_token_url()),
                // Same Sunshine toggle as Windows — here it enables Bazzite's Sunshine rather than
                // installing an .exe. Steam/Discord aren't wired: Bazzite already ships Steam+gaming mode.
                enable_sunshine: apps.contains(&GuestApp::Sunshine),
                enable_moonlight: apps.contains(&GuestApp::Moonlight),
                data_volume,
                ..KickstartSpec::default()
            };
            build_kickstart_seed_with(&spec, &extras, path)?
        }
    }
    Ok(seed)
}

/// Reject station names that could escape the disk directory — a name becomes both a qcow2 file name
/// (`{DISK_DIR}/{name}.qcow2`) and the libvirt domain name.
/// Validate a station's guest-account fields (used by both the local create handler and the federation
/// `provision_spec`): username + hostname to a safe charset (they become bare shell/OS tokens in the
/// kickstart `%post` / autounattend), password just free of control chars (it's XML-escaped downstream).
pub(crate) fn valid_guest_fields(
    username: &str,
    hostname: &str,
    password: &str,
) -> Result<(), String> {
    let safe_name = |s: &str| {
        s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    };
    for (label, v) in [("Username", username), ("Hostname", hostname)] {
        if !v.is_empty() && !safe_name(v) {
            return Err(format!("{label} may only contain letters, numbers, - _ ."));
        }
    }
    if !password.is_empty() && !crate::ui::safe_field(password) {
        return Err("Password can't contain control characters (newlines/tabs).".into());
    }
    Ok(())
}

pub(crate) fn valid_station_name(name: &str) -> bool {
    !name.is_empty()
        && !name.contains("..")
        // No leading '-': the name becomes a bare virsh/libvirt argv token, so `-x`/`--all` would be
        // parsed as an option (argument injection).
        && !name.starts_with('-')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// Refuse a station disk path inside the golden-images directory. Running or installing a station
/// there writes over an image (overlays back onto images read-only, but a *disk* placed there is
/// written directly), corrupting it.
fn disk_target_ok(disk: &str) -> bool {
    let images = crate::images::images_dir();
    let images = images.trim_end_matches('/');
    let d = disk.trim();
    d != images && !d.starts_with(&format!("{images}/"))
}

/// Provision a station on THIS node from a compact federation spec (used by the remote-provision API
/// and the fleet create flow). Supports cloning a golden image (the federation-primary path — images
/// live on the shared store) or a fresh install (optionally unattended). Seats/USB and vGPU are left
/// to the node's own wizard for now.
pub(crate) fn provision_spec(s: &crate::federation::ProvisionSpec) -> Result<(), String> {
    if !valid_station_name(&s.name) {
        return Err("invalid station name (letters, numbers, - _ . only)".into());
    }
    // Same guest-field validation as the local create handler — this path is reachable from the local
    // fleet UI and from token-authed peers (api_provision), and these fields are interpolated into the
    // autounattend.xml / kickstart / a root-run %post.
    valid_guest_fields(&s.username, &s.hostname, &s.password)?;
    let disk = format!("{DISK_DIR}/{}.qcow2", s.name);
    if !disk_target_ok(&disk) {
        return Err("disk path resolves into the images directory".into());
    }
    let (dram, dvcpus, ddisk) = resource_defaults();
    let guest;
    let create_disk;
    let size_gib;
    let media;
    if let Some(img) = s.base_image.as_deref().filter(|i| !i.is_empty()) {
        let base = crate::images::path_of(img).ok_or("base image not found on this node")?;
        if FsPath::new(&disk).exists() {
            return Err(format!("a disk already exists at {disk}"));
        }
        create_overlay(FsPath::new(&disk), FsPath::new(&base)).map_err(|e| e.to_string())?;
        guest = crate::images::image_os(img).unwrap_or(GuestOs::Windows);
        create_disk = false;
        size_gib = 0;
        media = InstallMedia::default();
    } else {
        guest = if s.os == "steamos" {
            GuestOs::SteamOs
        } else {
            GuestOs::Windows
        };
        create_disk = !FsPath::new(&disk).exists();
        size_gib = s.size_gib.unwrap_or(ddisk);
        let install_iso = Some(default_iso(guest));
        let virtio_iso = matches!(guest, GuestOs::Windows)
            .then(|| format!("{}/virtio-win.iso", crate::storage::iso_dir()));
        // Unattended install: build the answer-file/kickstart seed from the account fields.
        let seed_iso = if s.unattend {
            let user = if s.username.trim().is_empty() {
                "player"
            } else {
                s.username.trim()
            };
            let pass = if s.password.trim().is_empty() {
                "tendril"
            } else {
                s.password.trim()
            };
            // The federation/API path leaves vGPU + app selection to the node's own wizard for now,
            // so no guest-driver injection or apps here.
            Some(
                build_seed(
                    guest,
                    &s.name,
                    &disk,
                    user,
                    pass,
                    s.hostname.trim(),
                    "",
                    &[],
                    false, // fleet-placed: data volume is a follow-up
                )
                .map_err(|e| e.to_string())?,
            )
        } else {
            None
        };
        media = InstallMedia {
            install_iso,
            virtio_iso,
            seed_iso,
        };
    }
    let req = StationRequest {
        name: s.name.clone(),
        guest,
        disk_path: disk,
        size_gib,
        create_disk,
        vcpus: s.vcpus.unwrap_or(dvcpus),
        memory_mib: s.memory_mib.unwrap_or(dram),
        native_hardware: s.native,
        passthrough_addresses: passthrough_for(s.gpu.as_deref().unwrap_or("")),
        mdev_uuid: None,
        media,
        usb_devices: Vec::new(),
        define: true,
        start: s.start,
        steam_library_dir: None, // fleet-placed stations: shared library is a follow-up
        data_disk: None,         // fleet-placed stations: data volume is a follow-up
        cpu_pinning: None,
        hugepages: false,
    };
    provision(&req, &Libvirt::system()).map_err(|e| e.to_string())?;
    record_local(
        &s.name,
        guest,
        s.base_image.as_deref().filter(|b| !b.is_empty()),
    );
    Ok(())
}

/// Record a station this node just created into the shared fleet registry (so it can be re-homed if
/// this node later goes down). No-op-safe when there's no shared store (writes locally).
fn record_local(name: &str, guest: GuestOs, base_image: Option<&str>) {
    let os = match guest {
        GuestOs::Windows => "windows",
        GuestOs::SteamOs => "steamos",
    };
    crate::federation::record_station(&crate::federation::node_name(), name, os, base_image);
}

pub(crate) fn passthrough_for(addr: &str) -> Vec<String> {
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

/// How a station is wired to a GPU: a whole-GPU (or SR-IOV VF) passthrough group, and/or a freshly
/// created vGPU mediated device. Resolving a vGPU selection **creates the mdev as a side effect**, so
/// call [`GpuAssignment::cleanup`] if provisioning then fails.
#[derive(Default)]
struct GpuAssignment {
    passthrough: Vec<String>,
    mdev_uuid: Option<String>,
}

impl GpuAssignment {
    /// Tear down anything this assignment created on the host (the mdev), on a failed provision.
    fn cleanup(&self) {
        if let Some(u) = &self.mdev_uuid {
            crate::vgpu::remove_mdev(u);
        }
    }
}

/// Resolve the wizard's `gpu` selection into a [`GpuAssignment`], creating a vGPU mdev if one was
/// chosen. Returns an error string (already user-facing) if mdev creation fails.
fn assign_gpu(sel: &str) -> Result<GpuAssignment, String> {
    if let Some((parent, type_id)) = crate::vgpu::parse_mdev_selection(sel) {
        let uuid = crate::vgpu::create_mdev(&parent, &type_id)?;
        Ok(GpuAssignment {
            passthrough: Vec::new(),
            mdev_uuid: Some(uuid),
        })
    } else {
        Ok(GpuAssignment {
            passthrough: passthrough_for(sel),
            mdev_uuid: None,
        })
    }
}

/// Sensible per-station resource defaults: split the host's RAM, CPU threads, and free disk evenly
/// across the passthrough-capable GPUs (one station per GPU). Returns (memory_mib, vcpus, disk_gib).
pub(crate) fn resource_defaults() -> (u64, u32, u32) {
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
        GuestOs::Windows => format!("{}/win11.iso", crate::storage::iso_dir()),
        GuestOs::SteamOs => format!("{}/bazzite-deck-nvidia.iso", crate::storage::iso_dir()),
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
    // Query the in-guest agent once (used by both the Guest and Remote-play panels).
    let agent = if running {
        lv.guest_agent(&name)
    } else {
        GuestAgentInfo::default()
    };
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
        (guest_panel(&agent, running))
        (remote_play_panel(&agent, running))
        (ui::panel("USB devices", None, usb_fragment(&lv, &name)))
        (snapshots_panel(&lv, &name))
        @if let Some(p) = resplit_panel(&name, running) { (p) }
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
    // JSON-encode the name into a JS string literal (rather than splice it raw into inline
    // `<script>`) and URL-encode it into the path — so a name with JS/URL-special characters can
    // never break out of the string, even though `valid_station_name` already constrains the charset.
    let name_js = serde_json::to_string(name).unwrap_or_else(|_| "\"\"".to_string());
    format!(
        r#"import RFB from '/assets/novnc/core/rfb.js';
const screen = document.getElementById('screen');
const statusEl = document.getElementById('console-status');
const say = (m) => {{ if (statusEl) statusEl.textContent = m; }};
const stationName = {name_js};
try {{
  const proto = location.protocol === 'https:' ? 'wss://' : 'ws://';
  const rfb = new RFB(screen, proto + location.host + '/stations/' + encodeURIComponent(stationName) + '/vnc');
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

#[cfg(test)]
mod tests {
    use super::{disk_target_ok, valid_station_name};

    #[test]
    fn station_names_reject_path_escapes() {
        assert!(valid_station_name("station1"));
        assert!(valid_station_name("win11-gaming.v2"));
        assert!(!valid_station_name("")); // empty
        assert!(!valid_station_name("images/test-golden")); // slash
        assert!(!valid_station_name("../images/x")); // traversal
        assert!(!valid_station_name("a b")); // space
    }

    #[test]
    fn disk_target_rejects_images_dir() {
        // Default images dir is /var/lib/tendril/images (no store mounted in tests).
        assert!(disk_target_ok("/var/lib/tendril/station1.qcow2"));
        assert!(disk_target_ok("/data/vms/s1.qcow2"));
        assert!(!disk_target_ok("/var/lib/tendril/images/test-golden.qcow2"));
        assert!(!disk_target_ok("/var/lib/tendril/images"));
    }
}

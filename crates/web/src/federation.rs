//! Federation: aggregate a fleet of independent Tendril nodes into one view.
//!
//! Each node stays a fully self-managing control plane; federation only **reads** peers over their JSON
//! API (`GET /api/node`) and shows the fleet together — no shared consensus, no quorum, no fencing (see
//! docs/FEDERATION.md). Peers are configured explicitly; inter-node calls carry a shared federation token.
//!
//! Config comes from env (`TENDRIL_NODE_NAME`, `TENDRIL_FEDERATION_TOKEN`, `TENDRIL_PEERS`) or
//! `/etc/tendril/federation.conf` (`key=value` lines: `name=…`, `token=…`, and repeatable `peer=…`).

use maud::{html, Markup};
use serde::{Deserialize, Serialize};

use tendril_capability_engine::{detect, GpuVendor};
use tendril_orchestrator::Libvirt;

use crate::ui;

fn conf_path() -> String {
    std::env::var("TENDRIL_FEDERATION_CONF")
        .unwrap_or_else(|_| "/etc/tendril/federation.conf".to_string())
}

/// Parsed federation.conf: (this node's name, shared token, peer entries).
fn conf() -> (Option<String>, Option<String>, Vec<String>) {
    let mut name = None;
    let mut token = None;
    let mut peers = Vec::new();
    if let Ok(txt) = std::fs::read_to_string(conf_path()) {
        for line in txt.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some((k, v)) = line.split_once('=') {
                let v = v.trim().to_string();
                match k.trim() {
                    "name" => name = Some(v),
                    "token" => token = Some(v),
                    "peer" => peers.push(v),
                    _ => {}
                }
            }
        }
    }
    (name, token, peers)
}

/// A configured peer node.
pub struct Peer {
    pub name: String,
    pub url: String,
}

/// Parse a peer entry: `name=http://host:port` or bare `http://host:port` (name derived from host).
fn parse_peer(entry: &str) -> Option<Peer> {
    let entry = entry.trim();
    if entry.is_empty() {
        return None;
    }
    let (name, url) = match entry.split_once('=') {
        Some((n, u)) => (n.trim().to_string(), u.trim().to_string()),
        None => (host_of(entry), entry.to_string()),
    };
    (!url.is_empty()).then_some(Peer { name, url })
}

/// A display name from a URL: the host part, e.g. `http://10.0.0.2:8080/` → `10.0.0.2`.
fn host_of(url: &str) -> String {
    url.rsplit("://")
        .next()
        .unwrap_or(url)
        .split(['/', ':'])
        .next()
        .unwrap_or(url)
        .to_string()
}

/// Configured peers (from `TENDRIL_PEERS` env, else `federation.conf`).
pub fn peers() -> Vec<Peer> {
    let entries: Vec<String> = match std::env::var("TENDRIL_PEERS") {
        Ok(v) => v.split(',').map(|s| s.to_string()).collect(),
        Err(_) => conf().2,
    };
    entries.iter().filter_map(|e| parse_peer(e)).collect()
}

/// True when this node is part of a fleet (any peers configured) — gates the Fleet nav/view.
pub fn enabled() -> bool {
    !peers().is_empty()
}

/// This node's name in the fleet.
pub fn node_name() -> String {
    std::env::var("TENDRIL_NODE_NAME")
        .ok()
        .or_else(|| conf().0)
        .or_else(|| ui::run_stdout("hostname", &[]).map(|s| s.trim().to_string()))
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "tendril".to_string())
}

/// The shared token peers present to each other (env, else conf, else a token file).
fn federation_token() -> String {
    std::env::var("TENDRIL_FEDERATION_TOKEN")
        .ok()
        .or_else(|| conf().1)
        .or_else(|| std::fs::read_to_string("/etc/tendril/federation-token").ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

/// True if `presented` matches the configured token (and a token is set).
pub fn token_ok(presented: &str) -> bool {
    let t = federation_token();
    !t.is_empty() && presented == t
}

// ── node info (the federation API payload) ──────────────────────────────────────────────────────

#[derive(Serialize, Deserialize, Clone)]
pub struct StationInfo {
    pub name: String,
    pub state: String,
    pub gpu: bool,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct GpuInfo {
    pub address: String,
    pub label: String,
    pub capability: String,
    pub used_by: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Default)]
pub struct Health {
    pub uptime: String,
    pub load: String,
    pub mem_used_gb: f64,
    pub mem_total_gb: f64,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct NodeInfo {
    pub name: String,
    #[serde(default)]
    pub reachable: bool,
    pub stations: Vec<StationInfo>,
    pub gpus: Vec<GpuInfo>,
    #[serde(default)]
    pub health: Health,
}

fn vendor_name(v: GpuVendor) -> &'static str {
    match v {
        GpuVendor::Nvidia => "NVIDIA",
        GpuVendor::Amd => "AMD",
        GpuVendor::Intel => "Intel",
        GpuVendor::Unknown => "GPU",
    }
}

fn local_health() -> Health {
    let read_mem = |k: &str| {
        std::fs::read_to_string("/proc/meminfo").ok().and_then(|s| {
            s.lines()
                .find(|l| l.starts_with(k))
                .and_then(|l| l.split_whitespace().nth(1))
                .and_then(|kb| kb.parse::<f64>().ok())
        })
    };
    let (used, total) = match (read_mem("MemTotal:"), read_mem("MemAvailable:")) {
        (Some(t), Some(a)) => ((t - a) / 1048576.0, t / 1048576.0),
        _ => (0.0, 0.0),
    };
    Health {
        uptime: ui::run_stdout("uptime", &["-p"])
            .unwrap_or_default()
            .trim()
            .to_string(),
        load: std::fs::read_to_string("/proc/loadavg")
            .ok()
            .map(|s| s.split_whitespace().take(3).collect::<Vec<_>>().join(" "))
            .unwrap_or_default(),
        mem_used_gb: used,
        mem_total_gb: total,
    }
}

/// This node's info, built from libvirt + the capability engine + host stats.
pub fn local_node_info() -> NodeInfo {
    let lv = Libvirt::system();
    let users = crate::hardware::gpu_users();
    let stations = lv
        .list()
        .into_iter()
        .map(|n| StationInfo {
            state: format!("{:?}", lv.state(&n)),
            gpu: !lv.pci_hostdevs(&n).is_empty(),
            name: n,
        })
        .collect();
    let matrix = detect();
    let gpus = matrix
        .gpus
        .iter()
        .map(|g| GpuInfo {
            address: g.gpu.address.clone(),
            label: format!(
                "{} {}",
                vendor_name(g.gpu.vendor),
                g.gpu.model.as_deref().unwrap_or("GPU")
            ),
            capability: format!("{:?}", g.capability),
            used_by: users.get(&g.gpu.address).cloned(),
        })
        .collect();
    NodeInfo {
        name: node_name(),
        reachable: true,
        stations,
        gpus,
        health: local_health(),
    }
}

/// Fetch a peer's info over its API (via `curl`, short timeout), or a down stub if unreachable.
fn fetch_peer(p: &Peer) -> NodeInfo {
    let url = format!("{}/api/node", p.url.trim_end_matches('/'));
    let auth = format!("X-Tendril-Federation: {}", federation_token());
    let parsed = ui::run_result("curl", &["-s", "--max-time", "5", "-H", &auth, &url])
        .ok()
        .and_then(|s| serde_json::from_str::<NodeInfo>(&s).ok());
    match parsed {
        Some(mut ni) => {
            ni.reachable = true;
            ni
        }
        None => NodeInfo {
            name: p.name.clone(),
            reachable: false,
            stations: Vec::new(),
            gpus: Vec::new(),
            health: Health::default(),
        },
    }
}

/// The whole fleet: this node first, then each peer (fetched concurrently).
pub fn fleet() -> Vec<NodeInfo> {
    let mut out = vec![local_node_info()];
    let handles: Vec<_> = peers()
        .into_iter()
        .map(|p| std::thread::spawn(move || fetch_peer(&p)))
        .collect();
    for h in handles {
        if let Ok(ni) = h.join() {
            out.push(ni);
        }
    }
    out
}

// ── handlers + UI ───────────────────────────────────────────────────────────────────────────────

/// JSON API consumed by peers' aggregators.
pub async fn api_node() -> axum::Json<NodeInfo> {
    axum::Json(local_node_info())
}

/// The aggregated fleet page.
pub async fn page() -> Markup {
    // Peer fetches shell out with a timeout; run off the async worker.
    let nodes = tokio::task::spawn_blocking(fleet).await.unwrap_or_default();
    fleet_page(nodes, None)
}

fn fleet_page(nodes: Vec<NodeInfo>, note: Option<Markup>) -> Markup {
    let up = nodes.iter().filter(|n| n.reachable).count();
    let total_stations: usize = nodes.iter().map(|n| n.stations.len()).sum();
    ui::page(
        "fleet",
        "Fleet",
        html! {
            @if let Some(n) = note { (n) }
            div.btnrow style="margin-bottom:16px" {
                a.btn.primary href="/fleet/new" { "+ New fleet station" }
            }
            p.sub style="margin-bottom:16px" {
                "Every node manages itself; this view aggregates the fleet over each node's API. "
                strong { (up) "/" (nodes.len()) } " node(s) reachable · " (total_stations) " station(s)."
            }
            @for n in &nodes { (node_card(n)) }
        },
    )
}

// ── remote provision + GPU-aware placement (Phase B) ────────────────────────────────────────────

#[derive(Serialize, Deserialize, Clone)]
pub struct ProvisionSpec {
    pub name: String,
    #[serde(default)]
    pub os: String,
    #[serde(default)]
    pub base_image: Option<String>,
    #[serde(default)]
    pub gpu: Option<String>,
    #[serde(default)]
    pub memory_mib: Option<u64>,
    #[serde(default)]
    pub vcpus: Option<u32>,
    #[serde(default)]
    pub size_gib: Option<u32>,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub password: String,
    #[serde(default)]
    pub hostname: String,
    #[serde(default)]
    pub unattend: bool,
    #[serde(default)]
    pub native: bool,
    #[serde(default)]
    pub start: bool,
}

#[derive(Serialize, Deserialize)]
pub struct ProvisionResult {
    pub ok: bool,
    #[serde(default)]
    pub error: Option<String>,
}

/// Remote-provision API: create a station on THIS node (called by the fleet aggregator with the
/// federation token). Provisioning is blocking libvirt work, so it runs off the async worker.
pub async fn api_provision(
    axum::Json(spec): axum::Json<ProvisionSpec>,
) -> axum::Json<ProvisionResult> {
    let res = tokio::task::spawn_blocking(move || crate::stations::provision_spec(&spec))
        .await
        .unwrap_or_else(|_| Err("provision task panicked".into()));
    axum::Json(match res {
        Ok(()) => ProvisionResult {
            ok: true,
            error: None,
        },
        Err(e) => ProvisionResult {
            ok: false,
            error: Some(e),
        },
    })
}

/// Post a provision spec to a peer's `/api/provision` over HTTP (curl, federation token).
fn remote_provision(url: &str, spec: &ProvisionSpec) -> Result<(), String> {
    let body = serde_json::to_string(spec).map_err(|e| e.to_string())?;
    let ep = format!("{}/api/provision", url.trim_end_matches('/'));
    let auth = format!("X-Tendril-Federation: {}", federation_token());
    let out = ui::run_result(
        "curl",
        &[
            "-s",
            "--max-time",
            "120",
            "-H",
            &auth,
            "-H",
            "Content-Type: application/json",
            "-d",
            &body,
            &ep,
        ],
    )?;
    let res: ProvisionResult =
        serde_json::from_str(&out).map_err(|e| format!("bad response from peer: {e}"))?;
    if res.ok {
        Ok(())
    } else {
        Err(res
            .error
            .unwrap_or_else(|| "remote provision failed".into()))
    }
}

/// A node's first free passthrough-capable GPU address, if any.
fn free_gpu(n: &NodeInfo) -> Option<String> {
    n.gpus
        .iter()
        .find(|g| g.used_by.is_none() && g.capability == "Passthrough")
        .map(|g| g.address.clone())
}

/// GPU-aware placement: resolve `target` ("" = auto) to a (node, gpu-address) with a free GPU.
fn place(nodes: &[NodeInfo], target: &str) -> Result<(String, String), String> {
    if !target.is_empty() {
        let n = nodes
            .iter()
            .find(|n| n.name == target && n.reachable)
            .ok_or("target node is not reachable")?;
        let g = free_gpu(n).ok_or("target node has no free passthrough GPU")?;
        Ok((n.name.clone(), g))
    } else {
        nodes
            .iter()
            .filter(|n| n.reachable)
            .find_map(|n| free_gpu(n).map(|g| (n.name.clone(), g)))
            .ok_or_else(|| "no node in the fleet has a free passthrough GPU".to_string())
    }
}

/// Dispatch a provision to the chosen node — locally if it's us, else over the peer's API.
fn dispatch(target: &str, spec: &ProvisionSpec) -> Result<(), String> {
    if target == node_name() {
        crate::stations::provision_spec(spec)
    } else {
        let url = peers()
            .into_iter()
            .find(|p| p.name == target)
            .map(|p| p.url)
            .ok_or("unknown peer node")?;
        remote_provision(&url, spec)
    }
}

#[derive(Deserialize)]
pub struct FleetCreateForm {
    name: String,
    #[serde(default)]
    os: String,
    #[serde(default)]
    base_image: String,
    #[serde(default)]
    target: String,
    #[serde(default)]
    username: String,
    #[serde(default)]
    password: String,
    #[serde(default)]
    hostname: String,
    #[serde(default)]
    memory_mib: String,
    #[serde(default)]
    vcpus: String,
    #[serde(default)]
    size_gib: String,
    #[serde(default)]
    unattend: Option<String>,
    #[serde(default)]
    native: Option<String>,
    #[serde(default)]
    start: Option<String>,
}

/// The "create a station on the fleet" form — mirrors the standard station wizard (same options,
/// sensible defaults), with the non-essential ones behind an Advanced toggle. GPU is auto-assigned by
/// placement rather than picked; seats/USB and vGPU are set on the node's own wizard.
pub async fn new_page() -> Markup {
    let nodes = tokio::task::spawn_blocking(fleet).await.unwrap_or_default();
    let images = crate::images::list();
    ui::page(
        "fleet",
        "New fleet station",
        html! {
            (ui::panel("Create a station on the fleet", None, html! {
                @let (ram, vcpus, disk) = crate::stations::resource_defaults();
                form.grid.pad method="post" action="/fleet/create" {
                    div.field { label { "Station name" } input name="name" value="station1" required; }
                    div.field {
                        label { "Placement" }
                        select name="target" {
                            option value="" { "Auto — any node with a free GPU" }
                            @for n in &nodes { @if n.reachable {
                                option value=(n.name) { (n.name) " (" (free_gpu(n).map(|_| "free GPU").unwrap_or("no free GPU")) ")" }
                            } }
                        }
                        span.hint { "A whole GPU is auto-assigned on the chosen node." }
                    }
                    @if !images.is_empty() {
                        div.field.wide {
                            label { "Base image (clone a ready-to-play station instantly)" }
                            select #fleet-base name="base_image" onchange="fleetClone()" {
                                option value="" { "None — install the OS fresh" }
                                @for (n, sz) in &images {
                                    @let osa = crate::images::image_os_short(n);
                                    option value=(n) data-os=(osa) { (n) " (" (sz) ") · " (crate::images::os_display(n)) }
                                }
                            }
                            span.hint { "Golden images on the shared store are visible to every node — cloning is instant, needs no install media, and the OS comes from the image." }
                        }
                    }
                    div.field.fleet-install-only {
                        label { "Guest OS" }
                        select #fleet-os name="os" {
                            option value="windows" { "Windows 11" }
                            option value="steamos" { "SteamOS (Bazzite)" }
                        }
                    }
                    div.field.fleet-install-only { label { "Username" } input name="username" value="player"; }
                    div.field.fleet-install-only { label { "Password" } input name="password" value="tendril"; }
                    details.advanced.wide {
                        summary { "Advanced options" }
                        div style="margin-top:14px; display:flex; flex-direction:column; gap:10px" {
                            div.field.check.fleet-install-only { input type="checkbox" name="unattend" id="f-unattend" checked; label for="f-unattend" { "Install unattended (hands-off)" } span.hint { "Installs the guest OS without prompts using the account above." } }
                            div.field.check { input type="checkbox" name="native" id="f-native"; label for="f-native" { "Native-hardware overlay (anti-cheat; may violate ToS)" } }
                            div.field.check { input type="checkbox" name="start" id="f-start" checked; label for="f-start" { "Start now" } }
                        }
                        div.grid style="margin-top:12px" {
                            div.field { label { "Memory (MiB)" } input name="memory_mib" value=(ram) inputmode="numeric"; span.hint { "Default sized to the chosen node." } }
                            div.field { label { "vCPUs" } input name="vcpus" value=(vcpus) inputmode="numeric"; }
                            div.field.fleet-install-only { label { "Disk size (GiB)" } input name="size_gib" value=(disk) inputmode="numeric"; }
                            div.field.wide.fleet-install-only { label { "Computer name / hostname" } input name="hostname" placeholder="defaults to the station name"; }
                        }
                    }
                    div.field.wide { div.btnrow { button.btn.primary type="submit" { "Create on fleet" } a.btn href="/fleet" { "Cancel" } } }
                    (maud::PreEscaped(
                        "<script>window.fleetClone=function(){\
                         var b=document.getElementById('fleet-base');if(!b)return;\
                         var o=b.options[b.selectedIndex];var cloning=b.value!=='';\
                         document.querySelectorAll('.fleet-install-only').forEach(function(e){e.style.display=cloning?'none':'';});\
                         var os=o&&o.getAttribute('data-os');var s=document.getElementById('fleet-os');\
                         if(cloning&&os&&s){s.value=os;}\
                         if(cloning&&!os&&s){var f=s.closest('.fleet-install-only');if(f)f.style.display='';}\
                         };fleetClone();</script>"
                    ))
                }
            }))
        },
    )
}

/// Resolve placement and dispatch the provision to the chosen node.
pub async fn create(axum::extract::Form(f): axum::extract::Form<FleetCreateForm>) -> Markup {
    let nodes = tokio::task::spawn_blocking(fleet).await.unwrap_or_default();
    if f.name.trim().is_empty() {
        return fleet_page(nodes, Some(banner(false, "Station name is required.")));
    }
    let (target, gpu) = match place(&nodes, f.target.trim()) {
        Ok(x) => x,
        Err(e) => return fleet_page(nodes, Some(banner(false, &e))),
    };
    let spec = ProvisionSpec {
        name: f.name.trim().to_string(),
        os: f.os.clone(),
        base_image: (!f.base_image.trim().is_empty()).then(|| f.base_image.trim().to_string()),
        gpu: Some(gpu.clone()),
        memory_mib: f.memory_mib.trim().parse().ok(),
        vcpus: f.vcpus.trim().parse().ok(),
        size_gib: f.size_gib.trim().parse().ok(),
        username: f.username.clone(),
        password: f.password.clone(),
        hostname: f.hostname.clone(),
        unattend: f.unattend.is_some(),
        native: f.native.is_some(),
        start: f.start.is_some(),
    };
    let (t, sp) = (target.clone(), spec.clone());
    let res = tokio::task::spawn_blocking(move || dispatch(&t, &sp))
        .await
        .unwrap_or_else(|_| Err("dispatch task panicked".into()));
    let nodes = tokio::task::spawn_blocking(fleet).await.unwrap_or_default();
    match res {
        Ok(()) => fleet_page(
            nodes,
            Some(banner(
                true,
                &format!(
                    "Created \u{201c}{}\u{201d} on {target} (GPU {gpu}).",
                    spec.name
                ),
            )),
        ),
        Err(e) => fleet_page(
            nodes,
            Some(banner(false, &format!("Failed on {target}: {e}"))),
        ),
    }
}

fn banner(ok: bool, msg: &str) -> Markup {
    html! { div class=(if ok { "banner ok" } else { "banner error" }) { (msg) } }
}

fn os_pretty(os: &str) -> &'static str {
    match os {
        "windows" => "Windows 11",
        "steamos" => "SteamOS",
        _ => "—",
    }
}

// ── station registry (Phase C) — shared record so a down node's stations are recoverable ─────────

#[derive(Serialize, Deserialize, Clone)]
pub struct StationRecord {
    pub node: String,
    pub name: String,
    #[serde(default)]
    pub os: String,
    /// The golden image a station was cloned from, if any — the key to re-homing it elsewhere.
    #[serde(default)]
    pub base_image: Option<String>,
}

fn safe_component(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect()
}

fn record_file(node: &str, name: &str) -> String {
    format!(
        "{}/{}__{}.json",
        crate::storage::registry_dir(),
        safe_component(node),
        safe_component(name)
    )
}

/// Record (or update) a station in the shared registry so it's known even if its node goes down. The
/// provisioning node calls this for its own stations (`node` is this node's name).
pub fn record_station(node: &str, name: &str, os: &str, base_image: Option<&str>) {
    let _ = std::fs::create_dir_all(crate::storage::registry_dir());
    let rec = StationRecord {
        node: node.to_string(),
        name: name.to_string(),
        os: os.to_string(),
        base_image: base_image.map(str::to_string),
    };
    if let Ok(j) = serde_json::to_string(&rec) {
        let _ = std::fs::write(record_file(node, name), j);
    }
}

/// Drop a station's registry record.
pub fn forget_station(node: &str, name: &str) {
    let _ = std::fs::remove_file(record_file(node, name));
}

fn all_records() -> Vec<StationRecord> {
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(crate::storage::registry_dir()) {
        for e in rd.flatten() {
            if e.path().extension().is_some_and(|x| x == "json") {
                if let Ok(txt) = std::fs::read_to_string(e.path()) {
                    if let Ok(r) = serde_json::from_str::<StationRecord>(&txt) {
                        out.push(r);
                    }
                }
            }
        }
    }
    out
}

fn records_for(node: &str) -> Vec<StationRecord> {
    let mut v: Vec<StationRecord> = all_records()
        .into_iter()
        .filter(|r| r.node == node)
        .collect();
    v.sort_by(|a, b| a.name.cmp(&b.name));
    v
}

#[derive(Deserialize)]
pub struct RehomeForm {
    name: String,
    from: String,
}

/// Cold re-home a down node's station onto a healthy one: recreate it from its golden image, place it
/// on a survivor with a free GPU, and move its registry record. Human-confirmed in the UI.
pub async fn rehome(axum::extract::Form(f): axum::extract::Form<RehomeForm>) -> Markup {
    let nodes = tokio::task::spawn_blocking(fleet).await.unwrap_or_default();
    let Some(rec) = records_for(&f.from).into_iter().find(|r| r.name == f.name) else {
        return fleet_page(
            nodes,
            Some(banner(false, "No registry record for that station.")),
        );
    };
    let Some(base) = rec.base_image.clone().filter(|b| !b.is_empty()) else {
        return fleet_page(
            nodes,
            Some(banner(
                false,
                &format!(
                    "\u{201c}{}\u{201d} has no golden image recorded — its disk was local to {}, so it can't be re-homed. Recreate it fresh.",
                    rec.name, f.from
                ),
            )),
        );
    };
    // Survivors: reachable nodes other than the down one.
    let survivors: Vec<NodeInfo> = nodes
        .iter()
        .filter(|n| n.reachable && n.name != f.from)
        .cloned()
        .collect();
    let (target, gpu) = match place(&survivors, "") {
        Ok(x) => x,
        Err(e) => return fleet_page(nodes, Some(banner(false, &format!("Can't re-home: {e}")))),
    };
    let spec = ProvisionSpec {
        name: rec.name.clone(),
        os: rec.os.clone(),
        base_image: Some(base),
        gpu: Some(gpu.clone()),
        memory_mib: None,
        vcpus: None,
        size_gib: None,
        username: String::new(),
        password: String::new(),
        hostname: String::new(),
        unattend: false,
        native: false,
        start: true,
    };
    let (t, sp) = (target.clone(), spec.clone());
    let res = tokio::task::spawn_blocking(move || dispatch(&t, &sp))
        .await
        .unwrap_or_else(|_| Err("dispatch task panicked".into()));
    let nodes = tokio::task::spawn_blocking(fleet).await.unwrap_or_default();
    match res {
        Ok(()) => {
            // The target recorded the station under its own name; drop the old node's record.
            forget_station(&f.from, &rec.name);
            fleet_page(
                nodes,
                Some(banner(
                    true,
                    &format!(
                        "Re-homed \u{201c}{}\u{201d} from {} onto {target} (GPU {gpu}).",
                        rec.name, f.from
                    ),
                )),
            )
        }
        Err(e) => fleet_page(
            nodes,
            Some(banner(false, &format!("Re-home failed on {target}: {e}"))),
        ),
    }
}

fn node_card(n: &NodeInfo) -> Markup {
    let free_gpus = n
        .gpus
        .iter()
        .filter(|g| g.used_by.is_none() && g.capability == "Passthrough")
        .count();
    ui::panel(
        &n.name,
        Some(if n.reachable { "online" } else { "unreachable" }),
        html! {
            div.pad {
                @if !n.reachable {
                    p.sub { "Node is not responding. Its stations keep running if it's only a network blip; if the node is dead, re-home its image-backed stations onto a healthy node below." }
                    @let recs = records_for(&n.name);
                    @if recs.is_empty() {
                        p.sub { "No stations recorded for this node in the shared registry." }
                    } @else {
                        div.scroll { table {
                            thead { tr { th { "Station" } th { "OS" } th { "Golden image" } th.right { "" } } }
                            tbody { @for r in &recs {
                                @let recoverable = r.base_image.as_deref().map(|b| !b.is_empty()).unwrap_or(false);
                                tr {
                                    td.mono { (r.name) }
                                    td { (os_pretty(&r.os)) }
                                    td.mono.sub { (r.base_image.as_deref().unwrap_or("—")) }
                                    td.right {
                                        @if recoverable {
                                            form method="post" action="/fleet/rehome" style="display:inline"
                                                onsubmit=(format!("return confirm('Re-home \"{}\" onto a healthy node? It is recreated from its golden image and started. If {} comes back, delete the duplicate there.')", r.name, n.name)) {
                                                input type="hidden" name="name" value=(r.name);
                                                input type="hidden" name="from" value=(n.name);
                                                button.btn.sm type="submit" { "Re-home" }
                                            }
                                        } @else {
                                            span.sub title="No golden image recorded and the disk was local to this node — its state can't be recovered; recreate it fresh." { "not re-homeable" }
                                        }
                                    }
                                }
                            } }
                        } }
                    }
                } @else {
                    p.sub style="margin:0 0 10px" {
                        (n.stations.len()) " station(s) · " (n.gpus.len()) " GPU(s), " (free_gpus) " free for passthrough"
                        @if !n.health.uptime.is_empty() { " · up " (n.health.uptime) }
                        @if n.health.mem_total_gb > 0.0 { " · " (format!("{:.0}/{:.0} GB RAM", n.health.mem_used_gb, n.health.mem_total_gb)) }
                    }
                    @if n.stations.is_empty() {
                        p.sub { "No stations." }
                    } @else {
                        div.scroll { table {
                            thead { tr { th { "Station" } th { "State" } th.right { "GPU" } } }
                            tbody { @for s in &n.stations {
                                tr {
                                    td.mono { (s.name) }
                                    td { (s.state) }
                                    td.right { @if s.gpu { "✓" } @else { span.sub { "—" } } }
                                }
                            } }
                        } }
                    }
                    @if !n.gpus.is_empty() {
                        p.sub style="margin:10px 0 0" {
                            "GPUs: "
                            @for (i, g) in n.gpus.iter().enumerate() {
                                @if i > 0 { " · " }
                                span title=(format!("{} [{}] — {}", g.label, g.address, g.capability)) {
                                    (g.label)
                                    @if let Some(u) = &g.used_by { " (" (u) ")" } @else { " (free)" }
                                }
                            }
                        }
                    }
                }
            }
        },
    )
}

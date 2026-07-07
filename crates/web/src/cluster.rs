//! Federation: aggregate a fleet of independent Tendril nodes into one view.
//!
//! Each node stays a fully self-managing control plane; federation only **reads** peers over their JSON
//! API (`GET /api/node`) and shows the fleet together — no shared consensus, no quorum, no fencing (see
//! docs/CLUSTERING.md). Peers are configured explicitly; inter-node calls carry a shared cluster token.
//!
//! Config comes from env (`TENDRIL_NODE_NAME`, `TENDRIL_CLUSTER_TOKEN`, `TENDRIL_PEERS`) or
//! `/etc/tendril/cluster.conf` (`key=value` lines: `name=…`, `token=…`, and repeatable `peer=…`).

use maud::{html, Markup};
use serde::{Deserialize, Serialize};

use tendril_capability_engine::{detect, GpuVendor};
use tendril_orchestrator::Libvirt;

use crate::ui;

fn conf_path() -> String {
    std::env::var("TENDRIL_CLUSTER_CONF")
        .unwrap_or_else(|_| "/etc/tendril/cluster.conf".to_string())
}

/// Parsed cluster.conf: (this node's name, shared token, peer entries).
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

/// Configured peers (from `TENDRIL_PEERS` env, else `cluster.conf`).
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
fn cluster_token() -> String {
    std::env::var("TENDRIL_CLUSTER_TOKEN")
        .ok()
        .or_else(|| conf().1)
        .or_else(|| std::fs::read_to_string("/etc/tendril/cluster-token").ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

/// True if `presented` matches the configured token (and a token is set).
pub fn token_ok(presented: &str) -> bool {
    let t = cluster_token();
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
    let auth = format!("X-Tendril-Cluster: {}", cluster_token());
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
    let up = nodes.iter().filter(|n| n.reachable).count();
    let total_stations: usize = nodes.iter().map(|n| n.stations.len()).sum();
    ui::page(
        "fleet",
        "Fleet",
        html! {
            p.sub style="margin-bottom:16px" {
                "Every node manages itself; this view aggregates the fleet over each node's API. "
                strong { (up) "/" (nodes.len()) } " node(s) reachable · " (total_stations) " station(s)."
            }
            @for n in &nodes { (node_card(n)) }
        },
    )
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
                    p.sub { "Node is not responding. Existing stations keep running; it's just not reachable for management right now." }
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

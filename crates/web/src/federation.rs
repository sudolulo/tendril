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
    /// The peer's mTLS federation endpoint, if it advertises one (auto-discovered peers only).
    pub fed: Option<String>,
}

/// Parse a peer entry: `name=<ui-url>` or bare `<ui-url>` (name derived from host). The URL half may
/// also carry the peer's mTLS endpoint after a `|`: `name=<ui-url>|<fed-url>` (written by a join).
fn parse_peer(entry: &str) -> Option<Peer> {
    let entry = entry.trim();
    if entry.is_empty() {
        return None;
    }
    let (name, rest) = match entry.split_once('=') {
        Some((n, u)) => (n.trim().to_string(), u.trim().to_string()),
        None => (host_of(entry), entry.to_string()),
    };
    let (url, fed) = match rest.split_once('|') {
        Some((u, f)) => {
            let f = f.trim();
            (u.trim().to_string(), (!f.is_empty()).then(|| f.to_string()))
        }
        None => (rest, None),
    };
    (!url.is_empty()).then_some(Peer { name, url, fed })
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

/// The shared-store directory holding each node's presence file (auto-discovery).
fn nodes_dir() -> Option<String> {
    crate::storage::store_root().map(|r| format!("{r}/nodes"))
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// This node's presence record, published to the shared store so peers auto-discover it.
#[derive(Serialize, Deserialize, Clone)]
struct Presence {
    name: String,
    url: String,
    ts: u64,
    /// The node's mTLS federation endpoint, when it serves one (absent on token-only nodes).
    #[serde(default)]
    fed: Option<String>,
}

/// The base URL peers should reach this node at: explicit override, else `http://<lan-ip>:<port>`.
pub(crate) fn advertise_url() -> String {
    if let Ok(u) = std::env::var("TENDRIL_ADVERTISE_URL") {
        let u = u.trim().trim_end_matches('/');
        if !u.is_empty() {
            return u.to_string();
        }
    }
    let port = std::env::var("TENDRIL_WEB_ADDR")
        .ok()
        .and_then(|a| a.rsplit(':').next().map(str::to_string))
        .unwrap_or_else(|| "8080".to_string());
    let ip = ui::run_stdout("hostname", &["-I"])
        .and_then(|s| s.split_whitespace().next().map(str::to_string))
        .unwrap_or_else(|| "127.0.0.1".to_string());
    let scheme = if crate::tls::enabled() {
        "https"
    } else {
        "http"
    };
    format!("{scheme}://{ip}:{port}")
}

/// Publish/refresh this node's presence on the shared store (called periodically). No-op without a
/// shared store (then federation falls back to manually configured peers).
pub fn heartbeat() {
    let Some(dir) = nodes_dir() else { return };
    let _ = std::fs::create_dir_all(&dir);
    let rec = Presence {
        name: node_name(),
        url: advertise_url(),
        ts: now_secs(),
        fed: crate::fedtls::available().then(crate::fedtls::fed_advertise_url),
    };
    if let Ok(j) = serde_json::to_string(&rec) {
        let _ = std::fs::write(format!("{dir}/{}.json", safe_component(&node_name())), j);
    }
}

/// Fleet peers: **auto-discovered** from the shared store's presence files (zero config — every node
/// that heartbeats there is found), unioned with any manually configured peers (env/conf), deduped by
/// name and excluding self.
pub fn peers() -> Vec<Peer> {
    let me = node_name();
    let mut seen = std::collections::HashSet::new();
    seen.insert(me.clone());
    let mut out: Vec<Peer> = Vec::new();
    if let Some(dir) = nodes_dir() {
        if let Ok(rd) = std::fs::read_dir(&dir) {
            for e in rd.flatten() {
                if e.path().extension().is_some_and(|x| x == "json") {
                    if let Ok(txt) = std::fs::read_to_string(e.path()) {
                        if let Ok(p) = serde_json::from_str::<Presence>(&txt) {
                            if seen.insert(p.name.clone()) {
                                out.push(Peer {
                                    name: p.name,
                                    url: p.url,
                                    fed: p.fed,
                                });
                            }
                        }
                    }
                }
            }
        }
    }
    let entries: Vec<String> = match std::env::var("TENDRIL_PEERS") {
        Ok(v) => v.split(',').map(str::to_string).collect(),
        Err(_) => conf().2,
    };
    for e in entries {
        if let Some(p) = parse_peer(&e) {
            if seen.insert(p.name.clone()) {
                out.push(p);
            }
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// True when this node is part of a fleet (any peers discovered/configured) — gates the Fleet nav/view.
/// The public demo always shows a fleet (synthetic — see `demo_fleet`).
pub fn enabled() -> bool {
    ui::is_demo() || !peers().is_empty()
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

/// A strong random token (two kernel UUIDs, dashes stripped → 64 hex chars).
fn new_random_token() -> Option<String> {
    let a = std::fs::read_to_string("/proc/sys/kernel/random/uuid").ok()?;
    let b = std::fs::read_to_string("/proc/sys/kernel/random/uuid").ok()?;
    Some(format!(
        "{}{}",
        a.trim().replace('-', ""),
        b.trim().replace('-', "")
    ))
}

/// The shared token peers present to each other. Precedence: env → conf → **auto-managed on the shared
/// store** (`<store>/fleet-token`, generated once, 0600) → legacy token file. Auto-generation on the
/// store is what makes membership zero-config: every node that mounts the store reads the same token.
fn federation_token() -> String {
    if let Ok(t) = std::env::var("TENDRIL_FEDERATION_TOKEN") {
        let t = t.trim();
        if !t.is_empty() {
            return t.to_string();
        }
    }
    if let Some(t) = conf().1.filter(|t| !t.is_empty()) {
        return t;
    }
    if let Some(root) = crate::storage::store_root() {
        let p = format!("{root}/fleet-token");
        if let Ok(t) = std::fs::read_to_string(&p) {
            let t = t.trim();
            if !t.is_empty() {
                return t.to_string();
            }
        }
        // Generate once; create_new loses the race gracefully (the winner's token is then read).
        if let Some(tok) = new_random_token() {
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&p)
            {
                Ok(mut f) => {
                    use std::io::Write as _;
                    let _ = f.write_all(tok.as_bytes());
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        let _ =
                            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600));
                    }
                    return tok;
                }
                Err(_) => {
                    if let Ok(t) = std::fs::read_to_string(&p) {
                        let t = t.trim();
                        if !t.is_empty() {
                            return t.to_string();
                        }
                    }
                }
            }
        }
    }
    std::fs::read_to_string("/etc/tendril/federation-token")
        .ok()
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

/// True if `presented` matches the configured token (and a token is set).
pub fn token_ok(presented: &str) -> bool {
    let t = federation_token();
    !t.is_empty() && presented == t
}

/// Persist a federation token to `federation.conf` (replacing any existing `token=` line). Used when
/// founding a **store-less** fleet, where there's no shared store to auto-generate/hold the token.
fn set_conf_token(tok: &str) -> Result<(), String> {
    let p = conf_path();
    let mut lines: Vec<String> = std::fs::read_to_string(&p)
        .ok()
        .map(|t| {
            t.lines()
                .filter(|l| !l.trim_start().starts_with("token="))
                .map(String::from)
                .collect()
        })
        .unwrap_or_default();
    lines.push(format!("token={tok}"));
    if let Some(d) = std::path::Path::new(&p).parent() {
        let _ = std::fs::create_dir_all(d);
    }
    std::fs::write(&p, lines.join("\n") + "\n").map_err(|e| e.to_string())
}

/// This node's federation token, generating and persisting one to `federation.conf` if none exists
/// yet — so a store-less founder always has a token to hand out in its join code.
fn ensure_founder_token() -> Option<String> {
    let t = federation_token();
    if !t.is_empty() {
        return Some(t);
    }
    let tok = new_random_token()?;
    set_conf_token(&tok).ok()?;
    Some(tok)
}

/// A fleet join code: everything a new node needs to join this fleet with **no shared store** — this
/// node's reachable URLs, the shared token, and the fleet CA (cert + key) so the joiner trusts the
/// fleet and self-issues its cert. It's a one-time trust-bearing secret (like a Proxmox join blob).
#[derive(Serialize, Deserialize)]
struct JoinCode {
    /// This (founding) node's name.
    name: String,
    /// This node's plain-TLS UI URL (fallback transport).
    ui: String,
    /// This node's mTLS federation endpoint.
    fed: String,
    /// The shared federation token.
    token: String,
    /// The fleet CA certificate (PEM).
    ca: String,
    /// The fleet CA private key (PEM) — lets the joiner self-issue its node cert store-lessly.
    cakey: String,
}

/// Produce a copy-paste join code for this fleet (see [`JoinCode`]). Founds a store-less CA + token
/// on demand, so "Create fleet" works on a lone node with no shared store. `None` if CA material
/// can't be produced (e.g. `openssl` missing).
pub fn make_join_code() -> Option<String> {
    use base64::Engine as _;
    let token = ensure_founder_token()?;
    let (ca, cakey) = crate::fedtls::ca_material()?;
    let jc = JoinCode {
        name: node_name(),
        ui: advertise_url(),
        fed: crate::fedtls::fed_advertise_url(),
        token,
        ca,
        cakey,
    };
    let json = serde_json::to_vec(&jc).ok()?;
    Some(base64::engine::general_purpose::STANDARD.encode(json))
}

/// Append a peer to `federation.conf` (replacing any existing entry with the same name). Entry format
/// is `name=<ui-url>|<fed-url>` (see [`parse_peer`]).
fn add_conf_peer(entry: &str) -> Result<(), String> {
    let p = conf_path();
    let new_name = entry
        .split_once('=')
        .map(|(n, _)| n.trim())
        .unwrap_or(entry);
    let mut lines: Vec<String> = std::fs::read_to_string(&p)
        .unwrap_or_default()
        .lines()
        .filter(|l| {
            l.trim()
                .strip_prefix("peer=")
                .and_then(|r| r.split_once('='))
                .map(|(n, _)| n.trim() != new_name)
                .unwrap_or(true)
        })
        .map(String::from)
        .collect();
    lines.push(format!("peer={entry}"));
    if let Some(d) = std::path::Path::new(&p).parent() {
        let _ = std::fs::create_dir_all(d);
    }
    std::fs::write(&p, lines.join("\n") + "\n").map_err(|e| e.to_string())
}

/// A node introducing itself to a fleet member (join reverse-registration payload).
#[derive(Serialize, Deserialize)]
pub struct RegisterReq {
    name: String,
    ui: String,
    #[serde(default)]
    fed: String,
}

/// Founder-side register API: a joining node calls this (token-authed) so the founder adds it as a
/// peer too — the store-less path to *mutual* membership (no shared presence dir to discover it).
pub async fn api_fleet_register(
    axum::Json(r): axum::Json<RegisterReq>,
) -> axum::Json<ProvisionResult> {
    if ui::is_demo() {
        return axum::Json(ProvisionResult {
            ok: false,
            error: Some("disabled in the demo".into()),
        });
    }
    let name = safe_component(r.name.trim());
    if name.is_empty() || r.ui.trim().is_empty() {
        return axum::Json(ProvisionResult {
            ok: false,
            error: Some("register requires a node name and UI url".into()),
        });
    }
    let entry = format!("{}={}|{}", name, r.ui.trim(), r.fed.trim());
    axum::Json(match add_conf_peer(&entry) {
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

/// Tell the founder about this node so it adds us as a peer (mutual membership). Uses the founder's
/// plain-TLS UI endpoint with the shared token — works before either side's mTLS listener is up.
fn register_with(jc: &JoinCode) -> Result<(), String> {
    let ep = format!("{}/api/fleet/register", jc.ui.trim_end_matches('/'));
    let body = serde_json::to_string(&RegisterReq {
        name: node_name(),
        ui: advertise_url(),
        fed: crate::fedtls::fed_advertise_url(),
    })
    .map_err(|e| e.to_string())?;
    let auth = format!("X-Tendril-Federation: {}", jc.token);
    let out = ui::run_result(
        "curl",
        &[
            "-sk",
            "--max-time",
            "30",
            "-X",
            "POST",
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
        serde_json::from_str(&out).map_err(|e| format!("bad response from founder: {e}"))?;
    if res.ok {
        Ok(())
    } else {
        Err(res
            .error
            .unwrap_or_else(|| "founder rejected registration".into()))
    }
}

/// Apply a join code on a new node: install the fleet CA, adopt the shared token, add the founder as a
/// peer (with its mTLS URL), and register back so membership is mutual. Federation works immediately
/// over token+TLS; mTLS fully engages once this node's service restarts to start its mTLS listener.
pub fn apply_join_code(code: &str) -> Result<String, String> {
    use base64::Engine as _;
    let raw = base64::engine::general_purpose::STANDARD
        .decode(code.trim())
        .map_err(|_| "invalid join code (not valid base64)".to_string())?;
    let jc: JoinCode = serde_json::from_slice(&raw)
        .map_err(|_| "invalid join code (unrecognized contents)".to_string())?;
    if jc.name.trim().is_empty() || jc.token.trim().is_empty() || jc.ca.trim().is_empty() {
        return Err("join code is missing the fleet name, token, or CA".into());
    }
    crate::fedtls::install_ca(&jc.ca, &jc.cakey)?;
    set_conf_token(&jc.token)?;
    add_conf_peer(&format!(
        "{}={}|{}",
        safe_component(jc.name.trim()),
        jc.ui,
        jc.fed
    ))?;
    match register_with(&jc) {
        Ok(()) => Ok(format!(
            "Joined the fleet via {}. Both nodes now see each other. Restart this node's tendril-web \
             service to bring up its mTLS endpoint (federation already works over the shared token).",
            jc.name
        )),
        Err(e) => Ok(format!(
            "Joined the fleet via {} — you can see it now. Reverse registration didn't complete ({e}); \
             add this node on {} manually, or it will be picked up once both nodes are on a shared store.",
            jc.name, jc.name
        )),
    }
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
    // mTLS to the peer's federation endpoint when both sides support it (verifies the peer via our
    // shared CA); else plain TLS (`-k`) + the shared token — still encrypted.
    let (base, sec) = crate::fedtls::transport(&p.url, p.fed.as_deref());
    let url = format!("{base}/api/node");
    let auth = format!("X-Tendril-Federation: {}", federation_token());
    let mut args: Vec<&str> = sec.iter().map(String::as_str).collect();
    args.extend(["--max-time", "5", "-H", &auth, &url]);
    let parsed = ui::run_result("curl", &args)
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
    if ui::is_demo() {
        return fleet_page(demo_fleet(), None);
    }
    // Peer fetches shell out with a timeout; run off the async worker.
    let nodes = tokio::task::spawn_blocking(fleet).await.unwrap_or_default();
    fleet_page(nodes, None)
}

/// A synthetic multi-node fleet for the public demo: a few heterogeneous boxes with stations and GPUs,
/// including a vGPU-split node and one that's offline (to show the re-home story).
pub(crate) fn demo_fleet() -> Vec<NodeInfo> {
    let station = |name: &str, state: &str, gpu: bool| StationInfo {
        name: name.to_string(),
        state: state.to_string(),
        gpu,
    };
    let gpu = |addr: &str, label: &str, cap: &str, used: Option<&str>| GpuInfo {
        address: addr.to_string(),
        label: label.to_string(),
        capability: cap.to_string(),
        used_by: used.map(str::to_string),
    };
    let health = |uptime: &str, used: f64, total: f64| Health {
        uptime: uptime.to_string(),
        load: String::new(),
        mem_used_gb: used,
        mem_total_gb: total,
    };
    vec![
        NodeInfo {
            name: "aurora".into(),
            reachable: true,
            stations: vec![
                station("win-arcade", "Running", true),
                station("steam-den", "Running", true),
                station("test-bench", "Shutoff", true),
            ],
            gpus: vec![
                gpu(
                    "0000:01:00.0",
                    "NVIDIA RTX 4090",
                    "Passthrough",
                    Some("win-arcade"),
                ),
                gpu(
                    "0000:41:00.0",
                    "NVIDIA RTX 3080",
                    "Passthrough",
                    Some("steam-den"),
                ),
            ],
            health: health("6 days", 38.0, 64.0),
        },
        NodeInfo {
            name: "nebula".into(),
            reachable: true,
            stations: vec![
                station("couch-coop", "Running", true),
                station("guest-1", "Shutoff", true),
            ],
            gpus: vec![
                gpu(
                    "0000:01:00.0",
                    "NVIDIA RTX 4080",
                    "Passthrough",
                    Some("couch-coop"),
                ),
                gpu("0000:81:00.0", "NVIDIA RTX 4080", "Passthrough", None),
            ],
            health: health("13 days", 22.0, 32.0),
        },
        NodeInfo {
            name: "quasar".into(),
            reachable: true,
            stations: vec![
                station("vgpu-a", "Running", true),
                station("vgpu-b", "Running", true),
                station("vgpu-c", "Running", true),
            ],
            gpus: vec![
                gpu(
                    "0000:01:00.0",
                    "NVIDIA A40",
                    "VgpuOfficial",
                    Some("vgpu-a, vgpu-b, vgpu-c"),
                ),
                gpu("0000:c1:00.0", "NVIDIA RTX 4070", "Passthrough", None),
            ],
            health: health("2 days", 51.0, 128.0),
        },
        NodeInfo {
            name: "eclipse".into(),
            reachable: false,
            stations: Vec::new(),
            gpus: Vec::new(),
            health: Health::default(),
        },
    ]
}

fn fleet_page(nodes: Vec<NodeInfo>, note: Option<Markup>) -> Markup {
    let up = nodes.iter().filter(|n| n.reachable).count();
    let total_stations: usize = nodes.iter().map(|n| n.stations.len()).sum();
    // A lone node (just this one, not the demo) gets a create/join empty state instead of the
    // infrastructure framing — the Fleet tab is always visible now, so this is the common first view.
    let lone = !ui::is_demo() && nodes.len() <= 1;
    ui::page(
        "fleet",
        "Fleet",
        html! {
            @if let Some(n) = note { (n) }
            @if lone {
                p.sub style="margin-bottom:16px" {
                    strong { "This machine isn't in a fleet yet." }
                    " A fleet manages multiple Tendril machines together and lets you place stations "
                    "across them. Create one below to get a join code, or join an existing fleet by "
                    "pasting its code."
                }
            } @else {
                p.sub style="margin-bottom:16px" {
                    "Infrastructure view — the machines in your fleet, their GPUs and health. Every node "
                    "manages itself; create and control stations from the "
                    a href="/stations" { "Stations" } " page. "
                    strong { (up) "/" (nodes.len()) } " node(s) reachable · " (total_stations) " station(s)."
                }
            }
            @for n in &nodes { (node_card(n)) }
            @if !ui::is_demo() { (setup_panel()) (pxe_panel()) }
        },
    )
}

/// "Provision a room" — PXE net-boot a rack of bare-metal machines into the unattended installer.
/// Running it affects the LAN, so this shows the command rather than firing it from a button.
fn pxe_panel() -> Markup {
    ui::panel(
        "Provision a room (PXE)",
        Some("net-boot many machines into the unattended installer"),
        html! {
            div.pad {
                p.sub style="margin-top:0" {
                    "Turn this node into a PXE server so a rack of bare-metal PCs images itself hands-off: "
                    "each net-boots, ERASES its disk, and installs Tendril unattended. Uses proxy-DHCP, so "
                    "it's safe on a live network (your router keeps handing out IPs). UEFI targets; set them "
                    "to network-boot first."
                }
                pre.mono style="margin:0; padding:8px 10px; background:var(--bg2,#0002); border-radius:6px; overflow-x:auto; font-size:12.5px" {
                    "sudo /usr/libexec/tendril/tendril-pxe.sh --iso /path/to/tendril-installer-x86_64.iso"
                }
                p.sub style="margin:8px 0 0" { "Grab the ISO from " a href="https://dl.onetick.ninja/" { "dl.onetick.ninja" } ". Ctrl-C to stop serving." }
            }
        },
    )
}

// ── fleet setup / onboarding ─────────────────────────────────────────────────────────────────

/// A node-name override forced by env (then the setup panel shows it read-only).
fn env_node_name_override() -> Option<String> {
    std::env::var("TENDRIL_NODE_NAME")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// True when the join token is auto-managed on the shared store (so it can be rotated from the UI),
/// rather than pinned by env/conf.
fn token_is_store_managed() -> bool {
    std::env::var("TENDRIL_FEDERATION_TOKEN").is_err()
        && conf().1.is_none()
        && crate::storage::store_root().is_some()
}

/// Update the node name in `federation.conf`, preserving other keys.
fn set_conf_name(new: &str) -> Result<(), String> {
    let p = conf_path();
    let mut lines: Vec<String> = std::fs::read_to_string(&p)
        .ok()
        .map(|t| {
            t.lines()
                .filter(|l| !l.trim_start().starts_with("name="))
                .map(String::from)
                .collect()
        })
        .unwrap_or_default();
    lines.push(format!("name={new}"));
    if let Some(d) = std::path::Path::new(&p).parent() {
        let _ = std::fs::create_dir_all(d);
    }
    std::fs::write(&p, lines.join("\n") + "\n").map_err(|e| e.to_string())
}

#[derive(Deserialize)]
pub struct NameForm {
    name: String,
}

/// Rename this node (writes `federation.conf`), drop its old presence file, and re-advertise.
pub async fn setup_name(axum::Form(f): axum::Form<NameForm>) -> Markup {
    if ui::is_demo() {
        return setup_body(Some(html! { div.banner.warn { "Disabled in the demo." } }));
    }
    if env_node_name_override().is_some() {
        return setup_body(Some(
            html! { div.banner.warn { "This node's name is set via the " code { "TENDRIL_NODE_NAME" } " env var — change it there." } },
        ));
    }
    let new = safe_component(f.name.trim());
    if new.is_empty() {
        return setup_body(Some(html! { div.banner.error { "Enter a node name." } }));
    }
    let old = node_name();
    if let Err(e) = set_conf_name(&new) {
        return setup_body(Some(
            html! { div.banner.error { "Couldn't save the name: " (e) } },
        ));
    }
    if new != old {
        if let Some(dir) = nodes_dir() {
            let _ = std::fs::remove_file(format!("{dir}/{}.json", safe_component(&old)));
        }
    }
    heartbeat();
    setup_body(Some(
        html! { div.banner.ok { "Node renamed to " b { (new) } "." } },
    ))
}

/// Rotate the shared join token (only when it's store-managed). Every node on the shared store picks the
/// new token up automatically.
pub async fn rotate_token() -> Markup {
    if ui::is_demo() {
        return setup_body(Some(html! { div.banner.warn { "Disabled in the demo." } }));
    }
    if !token_is_store_managed() {
        return setup_body(Some(
            html! { div.banner.warn { "The join token is pinned via env/conf — rotate it there, not here." } },
        ));
    }
    let Some(root) = crate::storage::store_root() else {
        return setup_body(Some(
            html! { div.banner.error { "No shared store to hold the token." } },
        ));
    };
    let Some(tok) = new_random_token() else {
        return setup_body(Some(
            html! { div.banner.error { "Couldn't generate a new token." } },
        ));
    };
    let p = format!("{root}/fleet-token");
    if let Err(e) = std::fs::write(&p, &tok) {
        return setup_body(Some(
            html! { div.banner.error { "Couldn't write the token: " (e) } },
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600));
    }
    setup_body(Some(
        html! { div.banner.ok { "Join token rotated. Other nodes on the shared store pick it up automatically." } },
    ))
}

/// The Fleet setup / onboarding panel — how to form and grow a fleet, shown wherever a lone node needs
/// to discover federation (System page) and on the Fleet page itself.
/// Generate and reveal this fleet's join code (store-less onboarding). Runs the CA/token setup off
/// the async worker since it shells out to `openssl` and touches the filesystem.
pub async fn join_code() -> Markup {
    if ui::is_demo() {
        return html! { div.banner.warn { "Disabled in the demo." } };
    }
    match tokio::task::spawn_blocking(make_join_code)
        .await
        .ok()
        .flatten()
    {
        Some(code) => html! {
            p.sub style="margin:0 0 6px" {
                "Paste this on a new node's " b { "Join fleet" } " screen. It stays valid until you rotate the token."
            }
            pre.mono style="margin:0; padding:8px 10px; background:var(--bg2,#0002); border-radius:6px; overflow-x:auto; font-size:12px; word-break:break-all" { (code) }
        },
        None => {
            html! { div.banner.error { "Couldn't generate a join code — is " code { "openssl" } " available on this node?" } }
        }
    }
}

/// The paste-a-join-code form (its own fn so the handler can re-render it after an error).
fn join_form() -> Markup {
    html! {
        p.sub style="margin:0 0 6px" {
            "Paste a join code from another node's Fleet setup. This node adopts that fleet's CA + token, "
            "and both nodes start seeing each other."
        }
        form hx-post="/fleet/join" hx-target="#join-box" hx-swap="innerHTML" {
            textarea name="code" rows="3" required
                style="width:100%; font-family:monospace; font-size:12px" placeholder="paste join code\u{2026}" {}
            div.btnrow style="margin-top:8px" { button.btn.sm.primary type="submit" { "Join fleet" } }
        }
    }
}

#[derive(Deserialize)]
pub struct JoinForm {
    code: String,
}

/// Apply a pasted join code (store-less join). Off the async worker — it shells to openssl/curl.
pub async fn join(axum::Form(f): axum::Form<JoinForm>) -> Markup {
    if ui::is_demo() {
        return html! { div.banner.warn { "Disabled in the live demo." } };
    }
    let code = f.code.clone();
    let res = tokio::task::spawn_blocking(move || apply_join_code(&code))
        .await
        .unwrap_or_else(|_| Err("join task panicked".into()));
    match res {
        Ok(msg) => html! { div.banner.ok { (msg) } },
        Err(e) => html! {
            div.banner.error style="margin-bottom:8px" { (e) }
            (join_form())
        },
    }
}

pub fn setup_panel() -> Markup {
    ui::panel("Fleet setup", Some("form & grow a fleet"), setup_body(None))
}

fn setup_body(banner: Option<Markup>) -> Markup {
    let name = node_name();
    let env_name = env_node_name_override().is_some();
    let store = crate::storage::store_root();
    let mtls = crate::fedtls::available();
    let reach = if mtls {
        crate::fedtls::fed_advertise_url()
    } else {
        advertise_url()
    };
    let token = federation_token();
    let peers = peers();
    let row =
        |k: &str, v: Markup| html! { tr { td.sub style="white-space:nowrap" { (k) } td { (v) } } };
    html! {
        div.pad #fleet-setup {
            @if let Some(b) = banner { (b) }
            p.sub style="margin-top:0" {
                "A fleet lets you see and control every machine from any one of them. To add a machine, "
                "generate a " b { "join code" } " here and paste it on that machine — that's the whole flow."
            }

            // Identity + fleet size — the only always-visible facts.
            table { tbody {
                (row("This node", html! {
                    @if env_name {
                        b { (name) } " " span.sub { "(set via TENDRIL_NODE_NAME)" }
                    } @else {
                        form.inline hx-post="/fleet/setup/name" hx-target="#fleet-setup" hx-swap="outerHTML"
                            style="display:inline-flex; gap:6px; align-items:center" {
                            input name="name" value=(name) style="width:12em";
                            button.btn.sm type="submit" { "Rename" }
                        }
                    }
                }))
                (row("Machines", html! {
                    b { (peers.len() + 1) } (if peers.is_empty() { " (just this one)" } else { "" })
                }))
            } }

            @if !peers.is_empty() {
                ul style="margin:8px 0 0; padding-left:18px" {
                    li.sub { b { (name) } " (this node)" }
                    @for p in &peers {
                        li.sub { (p.name) " — " span.mono { (p.url) }
                            @if p.fed.is_some() { " " span.badge title="secured with mTLS" { "mTLS" } }
                        }
                    }
                }
            }

            // Primary action 1 — add a machine (one path: a join code).
            div style="margin-top:16px; padding-top:12px; border-top:1px solid var(--line)" {
                div.sub style="font-weight:600; margin-bottom:4px" { "Add a machine" }
                p.sub style="margin:0 0 8px" { "Generate a code, then paste it into " b { "Join a fleet" } " on the machine you want to add. The code carries this node's address, trust, and security — treat it as a secret." }
                div #join-code-box { button.btn hx-get="/fleet/join-code" hx-target="#join-code-box" hx-swap="innerHTML" { "Generate join code" } }
            }

            // Primary action 2 — join someone else's fleet.
            div style="margin-top:16px; padding-top:12px; border-top:1px solid var(--line)" {
                div.sub style="font-weight:600; margin-bottom:4px" { "Join a fleet" }
                p.sub style="margin:0 0 8px" { "Paste a join code from another machine to join its fleet." }
                div #join-box { (join_form()) }
            }

            // Everything else — shared-store auto-membership, raw token, manual peering, discovery.
            details style="margin-top:16px; padding-top:12px; border-top:1px solid var(--line)" {
                summary.sub style="cursor:pointer" { "Advanced" }
                div style="margin-top:10px" {
                    table { tbody {
                        (row("Reachable at", html! { span.mono { (reach) } }))
                        (row("Shared store", match &store {
                            Some(p) => html! { span.mono { (p) } " " span.badge title="Nodes on this store auto-join" { "auto-membership" } },
                            None => html! { "none — " a href="/storage" { "add an NFS/SMB store" } " to auto-federate instead of using join codes" },
                        }))
                        (row("Secure transport", if mtls {
                            html! { span.badge { "mTLS" } " — CA auto-managed on the shared store" }
                        } else {
                            html! { "token + TLS — a join code still sets up mTLS; a shared store or " code { "TENDRIL_FED_CA_DIR" } " does too" }
                        }))
                    } }

                    div style="margin-top:12px" {
                        div.sub style="font-weight:600; margin-bottom:4px" { "Raw join token" }
                        p.sub style="margin:0 0 6px" { "The shared secret a join code wraps. Only needed for manual peering (below) or a shared store — treat it as a secret." }
                        pre.mono style="margin:0; padding:8px 10px; background:var(--bg2,#0002); border-radius:6px; overflow-x:auto; font-size:12px; word-break:break-all" { (token) }
                        @if token_is_store_managed() {
                            button.btn.sm style="margin-top:8px"
                                hx-post="/fleet/setup/rotate-token" hx-target="#fleet-setup" hx-swap="outerHTML"
                                hx-confirm="Rotate the join token? Nodes on the shared store update automatically; any manually-configured node must be given the new token." { "Rotate token" }
                        }
                    }

                    div style="margin-top:12px" {
                        div.sub style="font-weight:600; margin-bottom:4px" { "Manual peering (no join code)" }
                        p.sub style="margin:0 0 6px" { "Set these on the other node and restart it (then point this node back the same way):" }
                        pre.mono style="margin:0; padding:8px 10px; background:var(--bg2,#0002); border-radius:6px; overflow-x:auto; font-size:12px" {
                            "TENDRIL_PEERS=" (name) "=" (reach) "\nTENDRIL_FEDERATION_TOKEN=" (token)
                        }
                    }

                    @let discovered = crate::mdns::nearby();
                    @if !discovered.is_empty() {
                        div style="margin-top:12px" {
                            div.sub style="font-weight:600; margin-bottom:4px" { "Nearby on the LAN" }
                            p.sub style="margin:0 0 6px" { "Discovered Tendril machines. To add one, generate a join code on it and paste it into " b { "Join a fleet" } " above." }
                            ul style="margin:0; padding-left:18px" {
                                @for d in &discovered {
                                    li.sub {
                                        b { (d.name) } " — " a href=(d.url) target="_blank" rel="noreferrer" { span.mono { (d.url) } }
                                        @if d.fed.is_some() { " " span.badge title="Advertises an mTLS federation endpoint" { "mTLS" } }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
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

/// Valid remote station lifecycle actions (guards the path before it's proxied to a peer).
fn valid_action(a: &str) -> bool {
    matches!(a, "start" | "stop" | "forceoff" | "delete")
}

/// Remote station lifecycle API: perform an action on a station **on this node**, called by a peer
/// controlling it from its Stations page. Token-authed by the auth middleware (X-Tendril-Federation).
pub async fn api_station_action(
    axum::extract::Path((name, action)): axum::extract::Path<(String, String)>,
) -> axum::Json<ProvisionResult> {
    if ui::is_demo() {
        return axum::Json(ProvisionResult {
            ok: false,
            error: Some("disabled in the demo".into()),
        });
    }
    if !valid_action(&action) {
        return axum::Json(ProvisionResult {
            ok: false,
            error: Some(format!("unknown action: {action}")),
        });
    }
    let res = tokio::task::spawn_blocking(move || crate::stations::lifecycle(&name, &action))
        .await
        .unwrap_or_else(|_| Err("station action task panicked".into()));
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

/// Call a peer's station-action API over the federation transport (mTLS, or token + plain TLS).
fn remote_station_action(
    url: &str,
    fed: Option<&str>,
    name: &str,
    action: &str,
) -> Result<(), String> {
    let (base, sec) = crate::fedtls::transport(url, fed);
    let ep = format!("{base}/api/station/{name}/{action}");
    let auth = format!("X-Tendril-Federation: {}", federation_token());
    let mut args: Vec<&str> = sec.iter().map(String::as_str).collect();
    args.extend(["--max-time", "60", "-X", "POST", "-H", &auth, &ep]);
    let out = ui::run_result("curl", &args)?;
    let res: ProvisionResult =
        serde_json::from_str(&out).map_err(|e| format!("bad response from peer: {e}"))?;
    if res.ok {
        Ok(())
    } else {
        Err(res.error.unwrap_or_else(|| "remote action failed".into()))
    }
}

/// Dispatch a station lifecycle action to the node that owns the station — locally if it's us, else
/// over the peer's API. Rejects unknown actions before touching anything.
pub fn station_action_dispatch(node: &str, name: &str, action: &str) -> Result<(), String> {
    if !valid_action(action) {
        return Err(format!("unknown action: {action}"));
    }
    if node == node_name() {
        crate::stations::lifecycle(name, action)
    } else {
        let peer = peers()
            .into_iter()
            .find(|p| p.name == node)
            .ok_or("unknown peer node")?;
        remote_station_action(&peer.url, peer.fed.as_deref(), name, action)
    }
}

/// Post a provision spec to a peer's `/api/provision` over HTTP (curl, federation token).
fn remote_provision(url: &str, fed: Option<&str>, spec: &ProvisionSpec) -> Result<(), String> {
    let body = serde_json::to_string(spec).map_err(|e| e.to_string())?;
    let (base, sec) = crate::fedtls::transport(url, fed);
    let ep = format!("{base}/api/provision");
    let auth = format!("X-Tendril-Federation: {}", federation_token());
    let mut args: Vec<&str> = sec.iter().map(String::as_str).collect();
    args.extend([
        "--max-time",
        "120",
        "-H",
        &auth,
        "-H",
        "Content-Type: application/json",
        "-d",
        &body,
        &ep,
    ]);
    let out = ui::run_result("curl", &args)?;
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

/// How many free passthrough GPUs a node has.
fn free_gpu_count(n: &NodeInfo) -> usize {
    n.gpus
        .iter()
        .filter(|g| g.used_by.is_none() && g.capability == "Passthrough")
        .count()
}

/// A node's 1-minute load average (for tie-breaking), or a large value if unknown.
fn load1(n: &NodeInfo) -> f64 {
    n.health
        .load
        .split_whitespace()
        .next()
        .and_then(|s| s.parse().ok())
        .unwrap_or(f64::MAX)
}

/// GPU-aware placement: resolve `target` ("" = auto) to a (node, gpu-address) with a free GPU.
///
/// Auto scheduling balances the fleet: among reachable nodes that have a free passthrough GPU, pick the
/// one with the **most** free GPUs (so stations spread onto the emptiest hardware), breaking ties by
/// **fewest existing stations**, then **lowest load**. An explicit `target` places there or errors.
fn place(nodes: &[NodeInfo], target: &str) -> Result<(String, String), String> {
    if !target.is_empty() {
        let n = nodes
            .iter()
            .find(|n| n.name == target && n.reachable)
            .ok_or("target node is not reachable")?;
        let g = free_gpu(n).ok_or("target node has no free passthrough GPU")?;
        return Ok((n.name.clone(), g));
    }
    let best = nodes
        .iter()
        .filter(|n| n.reachable && free_gpu_count(n) > 0)
        .max_by(|a, b| {
            free_gpu_count(a)
                .cmp(&free_gpu_count(b))
                // fewer existing stations ranks higher
                .then(b.stations.len().cmp(&a.stations.len()))
                // lower load ranks higher
                .then(
                    load1(b)
                        .partial_cmp(&load1(a))
                        .unwrap_or(std::cmp::Ordering::Equal),
                )
        })
        .ok_or("no node in the fleet has a free passthrough GPU")?;
    let g = free_gpu(best).ok_or("no free passthrough GPU")?;
    Ok((best.name.clone(), g))
}

/// Dispatch a provision to the chosen node — locally if it's us, else over the peer's API.
fn dispatch(target: &str, spec: &ProvisionSpec) -> Result<(), String> {
    if target == node_name() {
        crate::stations::provision_spec(spec)
    } else {
        let peer = peers()
            .into_iter()
            .find(|p| p.name == target)
            .ok_or("unknown peer node")?;
        remote_provision(&peer.url, peer.fed.as_deref(), spec)
    }
}

// ── reimage (push a golden image to stations across the fleet) ───────────────────────────────────

#[derive(Serialize, Deserialize, Clone)]
pub struct ReimageSpec {
    pub station: String,
    pub image: String,
}

/// Remote-reimage API: reset a station on THIS node to the named golden image, resolved from this
/// node's image dir (the shared store — so the overlay backs onto it in place, nothing transferred).
pub async fn api_reimage(axum::Json(spec): axum::Json<ReimageSpec>) -> axum::Json<ProvisionResult> {
    let res = tokio::task::spawn_blocking(move || {
        let path = crate::images::path_of(&spec.image)
            .ok_or("image not found on this node".to_string())?;
        crate::images::reimage_station(&spec.station, &path)
    })
    .await
    .unwrap_or_else(|_| Err("reimage task panicked".into()));
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

/// Reimage a station on `node` — locally (overlay onto the local/shared image), or via the peer's API.
pub fn reimage_dispatch(node: &str, station: &str, image: &str) -> Result<(), String> {
    if node == node_name() {
        let path = crate::images::path_of(image).ok_or("image not found on this node")?;
        crate::images::reimage_station(station, &path)
    } else {
        let peer = peers()
            .into_iter()
            .find(|p| p.name == node)
            .ok_or("unknown peer node")?;
        remote_reimage(
            &peer.url,
            peer.fed.as_deref(),
            &ReimageSpec {
                station: station.to_string(),
                image: image.to_string(),
            },
        )
    }
}

fn remote_reimage(url: &str, fed: Option<&str>, spec: &ReimageSpec) -> Result<(), String> {
    let body = serde_json::to_string(spec).map_err(|e| e.to_string())?;
    let (base, sec) = crate::fedtls::transport(url, fed);
    let ep = format!("{base}/api/reimage");
    let auth = format!("X-Tendril-Federation: {}", federation_token());
    let mut args: Vec<&str> = sec.iter().map(String::as_str).collect();
    args.extend([
        "--max-time",
        "60",
        "-H",
        &auth,
        "-H",
        "Content-Type: application/json",
        "-d",
        &body,
        &ep,
    ]);
    let out = ui::run_result("curl", &args)?;
    let res: ProvisionResult =
        serde_json::from_str(&out).map_err(|e| format!("bad response from peer: {e}"))?;
    if res.ok {
        Ok(())
    } else {
        Err(res.error.unwrap_or_else(|| "remote reimage failed".into()))
    }
}

// ── distribute a golden image to every node's store (once per node; then overlays, no per-station copy) ──

/// Serve a golden image's bytes to a peer pulling it (token-authed). 404 if this node lacks it.
pub async fn api_image(
    axum::extract::Path(name): axum::extract::Path<String>,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    match crate::images::path_of(&name) {
        Some(p) => match tokio::fs::File::open(&p).await {
            Ok(f) => (
                [(axum::http::header::CONTENT_TYPE, "application/octet-stream")],
                axum::body::Body::from_stream(tokio_util::io::ReaderStream::new(f)),
            )
                .into_response(),
            Err(_) => axum::http::StatusCode::NOT_FOUND.into_response(),
        },
        None => axum::http::StatusCode::NOT_FOUND.into_response(),
    }
}

#[derive(Serialize, Deserialize)]
pub struct ImagePull {
    name: String,
    from: String,
}

/// Pull a golden image from a peer into THIS node's store (once; then reimaging uses overlays). No-op
/// if already present (e.g. a shared store).
pub async fn api_image_pull(
    axum::Json(pull): axum::Json<ImagePull>,
) -> axum::Json<ProvisionResult> {
    let tok = federation_token();
    let res =
        tokio::task::spawn_blocking(move || crate::images::pull_from(&pull.name, &pull.from, &tok))
            .await
            .unwrap_or_else(|_| Err("image-pull task panicked".into()));
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

/// Distribute a golden image to `node`'s store: it pulls from `source_url`. The local node is the
/// source (no-op if it already has it).
pub fn distribute_dispatch(node: &str, name: &str, source_url: &str) -> Result<(), String> {
    if node == node_name() {
        return if crate::images::path_of(name).is_some() {
            Ok(())
        } else {
            Err("image not present on the source node".into())
        };
    }
    let peer = peers()
        .into_iter()
        .find(|p| p.name == node)
        .ok_or("unknown peer node")?;
    let body = serde_json::to_string(&ImagePull {
        name: name.to_string(),
        from: source_url.to_string(),
    })
    .map_err(|e| e.to_string())?;
    let (base, sec) = crate::fedtls::transport(&peer.url, peer.fed.as_deref());
    let ep = format!("{base}/api/image-pull");
    let auth = format!("X-Tendril-Federation: {}", federation_token());
    let mut args: Vec<&str> = sec.iter().map(String::as_str).collect();
    args.extend([
        "--max-time",
        "3600",
        "-H",
        &auth,
        "-H",
        "Content-Type: application/json",
        "-d",
        &body,
        &ep,
    ]);
    let out = ui::run_result("curl", &args)?;
    let res: ProvisionResult =
        serde_json::from_str(&out).map_err(|e| format!("bad response from peer: {e}"))?;
    if res.ok {
        Ok(())
    } else {
        Err(res
            .error
            .unwrap_or_else(|| "remote image-pull failed".into()))
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

/// Resolve placement and dispatch the provision to the chosen node. The station spec comes from the
/// unified Stations wizard, which switches its POST target here when a peer node is chosen.
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

// ── cross-node VNC console proxy ───────────────────────────────────────────────────────────────

/// Cross-node console: relay the browser's noVNC WebSocket to a PEER station's VNC. This node opens an
/// mTLS WebSocket to the peer's `/api/station/:name/vnc` (fed client cert + shared token) and pipes
/// bytes both ways, so a station on any node can be opened and watched from here. Session-authed on the
/// browser side; the peer side is authed by the fed cert + token.
///
/// SCAFFOLDING: compiles + fully wired, but needs two real nodes with a running station's VNC to
/// validate end to end.
pub async fn peer_vnc_ws(
    axum::extract::Path((node, name)): axum::extract::Path<(String, String)>,
    ws: axum::extract::ws::WebSocketUpgrade,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    let Some(peer) = peers().into_iter().find(|p| p.name == node) else {
        return (axum::http::StatusCode::NOT_FOUND, "unknown fleet node").into_response();
    };
    let Some(fed) = peer.fed.clone() else {
        return (
            axum::http::StatusCode::BAD_GATEWAY,
            "peer has no mTLS console endpoint yet (it must restart to bring up its cert listener)",
        )
            .into_response();
    };
    let Some(tls) = crate::fedtls::client_config() else {
        return (
            axum::http::StatusCode::BAD_GATEWAY,
            "no federation identity for the mTLS console",
        )
            .into_response();
    };
    let ws_url = format!(
        "{}/api/station/{}/vnc",
        fed.trim_end_matches('/').replacen("https://", "wss://", 1),
        name
    );
    let token = federation_token();
    ws.on_upgrade(move |browser| peer_vnc_relay(browser, ws_url, token, tls))
}

/// Pump bytes between the browser WebSocket (axum) and the peer's mTLS WebSocket (tokio-tungstenite).
async fn peer_vnc_relay(
    browser: axum::extract::ws::WebSocket,
    url: String,
    token: String,
    tls: std::sync::Arc<rustls::ClientConfig>,
) {
    use axum::extract::ws::Message as AxMsg;
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    use tokio_tungstenite::tungstenite::Message as TgMsg;

    let mut req = match url.into_client_request() {
        Ok(r) => r,
        Err(_) => return,
    };
    if let Ok(v) = token.parse() {
        req.headers_mut().insert("X-Tendril-Federation", v);
    }
    let connector = tokio_tungstenite::Connector::Rustls(tls);
    let peer_ws =
        match tokio_tungstenite::connect_async_tls_with_config(req, None, false, Some(connector))
            .await
        {
            Ok((s, _)) => s,
            Err(_) => {
                let mut b = browser;
                let _ = b.send(AxMsg::Close(None)).await;
                return;
            }
        };
    let (mut peer_tx, mut peer_rx) = peer_ws.split();
    let (mut b_tx, mut b_rx) = browser.split();

    let to_peer = async {
        while let Some(Ok(msg)) = b_rx.next().await {
            let out = match msg {
                AxMsg::Binary(b) => TgMsg::Binary(b),
                AxMsg::Text(t) => TgMsg::Text(t.as_str().into()),
                AxMsg::Close(_) => break,
                _ => continue,
            };
            if peer_tx.send(out).await.is_err() {
                break;
            }
        }
    };
    let to_browser = async {
        while let Some(Ok(msg)) = peer_rx.next().await {
            let out = match msg {
                TgMsg::Binary(b) => AxMsg::Binary(b.to_vec()),
                TgMsg::Text(t) => AxMsg::Text(t.to_string()),
                TgMsg::Close(_) => break,
                _ => continue,
            };
            if b_tx.send(out).await.is_err() {
                break;
            }
        }
    };
    tokio::select! { _ = to_peer => {}, _ = to_browser => {} }
}

/// "Open" a peer station: a detail page served by this node with the station's live console proxied
/// from the owning node (via [`peer_vnc_ws`]). Full control of a station from any node.
pub async fn peer_station_detail(
    axum::extract::Path((node, name)): axum::extract::Path<(String, String)>,
) -> Markup {
    let (nd, st) = (node.clone(), name.clone());
    let info = tokio::task::spawn_blocking(move || {
        fleet()
            .into_iter()
            .find(|x| x.name == nd)
            .and_then(|x| x.stations.into_iter().find(|s| s.name == st))
    })
    .await
    .ok()
    .flatten();
    let running = info
        .as_ref()
        .map(|s| s.state.eq_ignore_ascii_case("running"))
        .unwrap_or(false);
    ui::page(
        "stations",
        &name,
        html! {
            div style="display:flex; align-items:center; gap:12px; margin-bottom:16px" {
                a.btn.sm href="/stations" { "\u{2190}" }
                h1 style="margin:0; font-size:1.3rem" { (name) }
                span.sub { "on " (node) }
            }
            (ui::panel("Console", Some(&format!("live console proxied from {node} over the fleet's secure channel")), html! {
                @if running {
                    div.pad {
                        div.console style="position:relative" {
                            div id="screen" {}
                            div id="console-status" style="position:absolute; inset:0; display:flex; align-items:center; justify-content:center; color:#8b97a6; font-size:14px; pointer-events:none" { "Connecting to " (node) " console\u{2026}" }
                        }
                    }
                    script type="module" { (maud::PreEscaped(peer_console_script(&node, &name))) }
                } @else {
                    div.emptybox { "The station isn't running. Start it from the " a href="/stations" { "Stations" } " page, then open its console." }
                }
            }))
        },
    )
}

fn peer_console_script(node: &str, name: &str) -> String {
    format!(
        r#"import RFB from '/assets/novnc/core/rfb.js';
const screen=document.getElementById('screen');const s=document.getElementById('console-status');const say=(m)=>{{if(s)s.textContent=m;}};
try{{
  const proto=location.protocol==='https:'?'wss://':'ws://';
  const rfb=new RFB(screen,proto+location.host+'/fleet/{node}/station/{name}/vnc');
  rfb.scaleViewport=true;rfb.background='#000';
  rfb.addEventListener('connect',()=>say(''));
  rfb.addEventListener('disconnect',(e)=>say((e.detail&&e.detail.clean)?'Console closed.':'Console connection lost — reload to reconnect.'));
  rfb.addEventListener('securityfailure',(e)=>say('Auth failed: '+((e.detail&&e.detail.reason)||'unknown')));
}}catch(err){{say('Console failed to start: '+(err&&err.message?err.message:err));}}
"#
    )
}

/// A peer node's stations panel — rendered on the Stations page for every node other than this one.
/// Its lifecycle controls (start/stop/force-off/delete) dispatch to the owning node over the
/// federation API, so a peer's stations are fully controllable from here, not just visible.
pub fn stations_peer_panel(n: &NodeInfo) -> Markup {
    peer_panel(n, None)
}

/// A peer node's stations panel with per-station controls (start/stop/delete dispatched to that node
/// over the federation API). Wrapped in a stable id so an action swaps just this panel. `err` renders
/// a banner inside the wrapper (used when re-rendering after a failed action).
fn peer_panel(n: &NodeInfo, err: Option<&str>) -> Markup {
    let free = n
        .gpus
        .iter()
        .filter(|g| g.used_by.is_none() && g.capability == "Passthrough")
        .count();
    let wrap = format!("peer-{}", safe_component(&n.name));
    let body = html! {
        div.pad {
            @if let Some(e) = err { div.banner.error style="margin-bottom:10px" { (e) } }
            // One-line node context: reachability · stations · GPUs free · uptime.
            p.sub style="margin:0 0 10px" {
                @if n.reachable { "online" } @else { "unreachable" }
                " · " (n.stations.len()) " station(s) · " (n.gpus.len()) " GPU(s), " (free) " free"
                @if !n.health.uptime.is_empty() { " · up " (n.health.uptime) }
            }
            @if !n.reachable {
                p.sub { "Node not responding. Recover its image-backed stations from the " a href="/fleet" { "Fleet" } " page." }
            } @else if n.stations.is_empty() {
                p.sub { "No stations on this node." }
            } @else {
                div.scroll { table {
                    thead { tr { th { "Station" } th { "State" } th.right { "Actions" } } }
                    tbody { @for s in &n.stations {
                        @let running = s.state.eq_ignore_ascii_case("running");
                        tr {
                            td.mono {
                                (s.name)
                                @if !s.gpu { span.sub title="no GPU passed through" { " \u{26a0}" } }
                            }
                            td { (crate::ui::state_pill_str(&s.state)) }
                            td.right { div.actions {
                                a.btn.sm href=(format!("/fleet/{}/station/{}", n.name, s.name)) { "Open" }
                                @if running {
                                    (peer_action_btn(&n.name, &s.name, "stop", "Shut down", false, &wrap))
                                    (peer_action_btn(&n.name, &s.name, "forceoff", "Force off", true, &wrap))
                                } @else {
                                    (peer_action_btn(&n.name, &s.name, "start", "Start", false, &wrap))
                                }
                                (peer_action_btn(&n.name, &s.name, "delete", "Delete", true, &wrap))
                            } }
                        }
                    } }
                } }
            }
        }
    };
    html! {
        // Self-refresh: re-fetch just this peer every 8s (like the local panel's 6s poll), swapping
        // the whole wrapper. Actions also target #wrap, so they compose with the poll.
        div id=(wrap)
            hx-get=(format!("/fleet/{}/panel", n.name))
            hx-trigger="every 8s" hx-target="this" hx-swap="outerHTML" {
            (ui::panel(&n.name, Some(if n.reachable { "peer · online" } else { "peer · unreachable" }), body))
        }
    }
}

/// UI poll: re-fetch a single peer's fresh state and re-render just its panel (self-refresh).
pub async fn peer_panel_fragment(axum::extract::Path(node): axum::extract::Path<String>) -> Markup {
    if ui::is_demo() {
        return match demo_fleet().into_iter().find(|x| x.name == node) {
            Some(x) => peer_panel(&x, None),
            None => html! {},
        };
    }
    let nd = node.clone();
    let fresh = tokio::task::spawn_blocking(move || {
        peers()
            .into_iter()
            .find(|p| p.name == nd)
            .map(|p| fetch_peer(&p))
    })
    .await
    .ok()
    .flatten();
    match fresh {
        Some(x) => peer_panel(&x, None),
        None => peer_panel(
            &NodeInfo {
                name: node,
                reachable: false,
                stations: Vec::new(),
                gpus: Vec::new(),
                health: Health::default(),
            },
            None,
        ),
    }
}

/// A control button that dispatches a station action to a peer node and swaps this peer's panel.
fn peer_action_btn(
    node: &str,
    station: &str,
    action: &str,
    label: &str,
    danger: bool,
    wrap: &str,
) -> Markup {
    let confirm = (action == "delete").then(|| {
        format!("Delete station '{station}' on {node}? It's forced off and its VM definition removed on that node.")
    });
    html! {
        button class=(if danger { "btn sm danger" } else { "btn sm" })
            hx-post=(format!("/fleet/{node}/station/{station}/{action}"))
            hx-target=(format!("#{wrap}")) hx-swap="outerHTML"
            hx-confirm=[confirm.as_deref()] { (label) }
    }
}

/// UI proxy: run a lifecycle action on a peer's station, then re-render that peer's panel.
pub async fn peer_station_action(
    axum::extract::Path((node, name, action)): axum::extract::Path<(String, String, String)>,
) -> Markup {
    if ui::is_demo() {
        return html! { div.banner.warn { "Actions are disabled in the live demo." } };
    }
    let (nd, st, ac) = (node.clone(), name.clone(), action.clone());
    let res = tokio::task::spawn_blocking(move || station_action_dispatch(&nd, &st, &ac))
        .await
        .unwrap_or_else(|_| Err("dispatch task panicked".into()));
    // Re-fetch this peer's fresh state so the panel reflects the action.
    let nd2 = node.clone();
    let fresh = tokio::task::spawn_blocking(move || fleet().into_iter().find(|x| x.name == nd2))
        .await
        .ok()
        .flatten();
    match fresh {
        Some(x) => peer_panel(&x, res.err().as_deref()),
        None => peer_panel(
            &NodeInfo {
                name: node,
                reachable: false,
                stations: Vec::new(),
                gpus: Vec::new(),
                health: Health::default(),
            },
            res.err().as_deref(),
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

#[cfg(test)]
mod scheduler_tests {
    use super::*;

    fn gpu(addr: &str, free: bool) -> GpuInfo {
        GpuInfo {
            address: addr.to_string(),
            label: "GPU".to_string(),
            capability: "Passthrough".to_string(),
            used_by: if free { None } else { Some("s".to_string()) },
        }
    }
    fn node(
        name: &str,
        reachable: bool,
        free_gpus: usize,
        stations: usize,
        load: &str,
    ) -> NodeInfo {
        NodeInfo {
            name: name.to_string(),
            reachable,
            stations: (0..stations)
                .map(|i| StationInfo {
                    name: format!("st{i}"),
                    state: "running".to_string(),
                    gpu: true,
                })
                .collect(),
            gpus: (0..free_gpus)
                .map(|i| gpu(&format!("{name}:{i}"), true))
                .collect(),
            health: Health {
                uptime: String::new(),
                load: load.to_string(),
                mem_used_gb: 0.0,
                mem_total_gb: 0.0,
            },
        }
    }

    #[test]
    fn auto_prefers_most_free_gpus_then_fewest_stations() {
        // A and B both have 2 free GPUs, but B has fewer stations -> B wins. C has only 1 free GPU.
        let nodes = vec![
            node("A", true, 2, 3, "1.0"),
            node("B", true, 2, 1, "2.0"),
            node("C", true, 1, 0, "0.1"),
        ];
        let (n, _g) = place(&nodes, "").unwrap();
        assert_eq!(n, "B");
    }

    #[test]
    fn auto_skips_unreachable_and_full_nodes() {
        let nodes = vec![
            node("down", false, 4, 0, "0.0"), // unreachable, ignored
            node("full", true, 0, 2, "0.0"),  // no free GPU, ignored
            node("ok", true, 1, 5, "9.0"),    // the only candidate
        ];
        assert_eq!(place(&nodes, "").unwrap().0, "ok");
        // No candidate at all -> error.
        let none = vec![node("full", true, 0, 1, "0.0")];
        assert!(place(&none, "").is_err());
    }

    #[test]
    fn explicit_target_must_be_reachable_and_have_a_gpu() {
        let nodes = vec![node("A", true, 1, 0, "0.0"), node("B", false, 2, 0, "0.0")];
        assert_eq!(place(&nodes, "A").unwrap().0, "A");
        assert!(place(&nodes, "B").is_err()); // unreachable
        assert!(place(&nodes, "ghost").is_err()); // unknown
    }
}

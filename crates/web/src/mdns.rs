//! LAN discovery of Tendril nodes over mDNS (`_tendril._tcp`). This node advertises itself and browses
//! for peers, so nearby machines show up in Fleet setup for one-click joining — no hand-typed IPs on a
//! flat LAN (the convention-floor case). Best-effort: any mDNS failure is silently non-fatal, and the
//! rest of federation (join codes, shared store) works without it.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};

const SERVICE_TYPE: &str = "_tendril._tcp.local.";

/// A Tendril node discovered on the LAN.
#[derive(Clone)]
pub struct Nearby {
    pub name: String,
    /// The node's browser UI / advertise URL.
    pub url: String,
    /// The node's mTLS federation endpoint, if it serves one.
    pub fed: Option<String>,
}

fn seen() -> &'static Mutex<HashMap<String, Nearby>> {
    static SEEN: OnceLock<Mutex<HashMap<String, Nearby>>> = OnceLock::new();
    SEEN.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Keep the daemon alive for the process — dropping it stops advertising/browsing.
fn daemon_slot() -> &'static Mutex<Option<ServiceDaemon>> {
    static D: OnceLock<Mutex<Option<ServiceDaemon>>> = OnceLock::new();
    D.get_or_init(|| Mutex::new(None))
}

/// Advertise this node and start browsing for peers. Call once at startup. No-op in the demo.
pub fn start() {
    if crate::ui::is_demo() {
        return;
    }
    let Ok(mdns) = ServiceDaemon::new() else {
        return;
    };

    let name = crate::federation::node_name();
    let url = crate::federation::advertise_url();
    let fed = crate::fedtls::available()
        .then(crate::fedtls::fed_advertise_url)
        .unwrap_or_default();
    let ip = url_host(&url);
    let port = url_port(&url).unwrap_or(443);
    let host = format!("{name}.local.");
    let props: [(&str, &str); 3] = [
        ("name", name.as_str()),
        ("url", url.as_str()),
        ("fed", fed.as_str()),
    ];
    if let Ok(info) = ServiceInfo::new(SERVICE_TYPE, &name, &host, ip.as_str(), port, &props[..]) {
        let _ = mdns.register(info);
    }

    if let Ok(rx) = mdns.browse(SERVICE_TYPE) {
        std::thread::spawn(move || {
            while let Ok(event) = rx.recv() {
                match event {
                    ServiceEvent::ServiceResolved(info) => {
                        let n = info.get_property_val_str("name").unwrap_or("").to_string();
                        let u = info.get_property_val_str("url").unwrap_or("").to_string();
                        // Keep the paired federation endpoint only if it too is a real http(s) URL —
                        // it's consumed by the federation/mTLS layer, so don't store a LAN-advertised
                        // `fed` with a bogus scheme or shell-special characters.
                        let f = info
                            .get_property_val_str("fed")
                            .filter(|s| crate::ui::is_http_url(s))
                            .map(str::to_string);
                        // Only keep an advert with a real http(s) URL — a LAN attacker can advertise
                        // anything, and the URL is later rendered as a link (block `javascript:` etc.).
                        if !n.is_empty() && crate::ui::is_http_url(&u) {
                            seen().lock().unwrap().insert(
                                info.get_fullname().to_string(),
                                Nearby {
                                    name: n,
                                    url: u,
                                    fed: f,
                                },
                            );
                        }
                    }
                    ServiceEvent::ServiceRemoved(_ty, fullname) => {
                        seen().lock().unwrap().remove(&fullname);
                    }
                    _ => {}
                }
            }
        });
    }
    *daemon_slot().lock().unwrap() = Some(mdns);
}

/// Nearby Tendril nodes on the LAN, excluding this node and any already-configured peers (those show
/// up under "Nodes in the fleet" already).
pub fn nearby() -> Vec<Nearby> {
    let me = crate::federation::node_name();
    let peers: std::collections::HashSet<String> = crate::federation::peers()
        .into_iter()
        .map(|p| p.name)
        .collect();
    let mut out: Vec<Nearby> = seen()
        .lock()
        .unwrap()
        .values()
        .filter(|n| n.name != me && !peers.contains(&n.name))
        .cloned()
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out.dedup_by(|a, b| a.name == b.name);
    out
}

fn url_host(url: &str) -> String {
    url.rsplit("://")
        .next()
        .unwrap_or(url)
        .split(['/', ':'])
        .next()
        .unwrap_or("")
        .to_string()
}

fn url_port(url: &str) -> Option<u16> {
    url.rsplit("://")
        .next()
        .unwrap_or(url)
        .split('/')
        .next()
        .unwrap_or("")
        .rsplit(':')
        .next()
        .and_then(|p| p.parse().ok())
}

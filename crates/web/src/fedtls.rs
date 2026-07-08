//! Mutual TLS for federation (node-to-node).
//!
//! The web UI's HTTPS is for *browsers* (self-signed or a user cert). Federation needs the opposite:
//! nodes authenticating *each other* by certificate, with no browser in the loop. So mTLS runs on its
//! **own listener** (`TENDRIL_FED_ADDR`, default `:8444`) with a rustls config that **requires a client
//! certificate** signed by a shared **federation CA** — and presents a server cert from that same CA,
//! so `curl` can verify the peer instead of `-k`.
//!
//! The CA is auto-managed on the **shared store** (`<store>/ca/`), exactly like the fleet token: every
//! node that mounts the store trusts the same CA and self-issues its own node cert from it (the CA key
//! is readable there — the shared store *is* the trust boundary). No store / no CA → mTLS is
//! unavailable and federation falls back to the token + plain-TLS (`-k`) path, still encrypted.

use std::io::BufReader;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};

use crate::ui;

/// Local dir holding this node's private key + issued cert (the key never leaves the node). Overridable
/// so co-located instances (e.g. a demo peer) don't share one identity.
fn identity_dir() -> String {
    std::env::var("TENDRIL_FED_IDENTITY_DIR")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "/var/lib/tendril/fedtls".to_string())
}

/// The CA directory: explicit override, else the shared store's `ca/`, else a **local** CA dir that
/// exists only once this node founded or joined a fleet via a join code (store-less trust). `None`
/// (→ mTLS off, token fallback) on a lone node that has done neither.
fn ca_dir() -> Option<String> {
    if let Ok(d) = std::env::var("TENDRIL_FED_CA_DIR") {
        let d = d.trim();
        if !d.is_empty() {
            return Some(d.to_string());
        }
    }
    if let Some(r) = crate::storage::store_root() {
        return Some(format!("{r}/ca"));
    }
    let local = local_ca_dir();
    Path::new(&format!("{local}/ca.pem"))
        .exists()
        .then_some(local)
}

/// Local CA dir for a store-less fleet — the founder generates the CA here (via a join code) and
/// joiners install the received CA here, so mTLS works with no shared store in the loop.
pub(crate) fn local_ca_dir() -> String {
    std::env::var("TENDRIL_FED_LOCAL_CA_DIR")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "/var/lib/tendril/fedtls-ca".to_string())
}

/// The federation mTLS listener address (its own port so it never collides with the browser UI).
pub fn fed_addr() -> String {
    std::env::var("TENDRIL_FED_ADDR").unwrap_or_else(|_| "0.0.0.0:8444".to_string())
}

/// The URL peers should reach this node's mTLS endpoint at (published in presence).
pub fn fed_advertise_url() -> String {
    if let Ok(u) = std::env::var("TENDRIL_FED_ADVERTISE_URL") {
        let u = u.trim().trim_end_matches('/');
        if !u.is_empty() {
            return u.to_string();
        }
    }
    let port = fed_addr().rsplit(':').next().unwrap_or("8444").to_string();
    let ip = ui::run_stdout("hostname", &["-I"])
        .and_then(|s| s.split_whitespace().next().map(str::to_string))
        .unwrap_or_else(|| "127.0.0.1".to_string());
    format!("https://{ip}:{port}")
}

#[derive(Clone)]
pub struct Identity {
    pub ca: String,
    pub cert: String,
    pub key: String,
}

/// Hostnames/IPs for this node's cert SANs (so peers can verify it however they reach it).
fn sans() -> Vec<String> {
    let mut v = vec!["localhost".to_string(), "127.0.0.1".to_string()];
    if let Some(h) = ui::run_stdout("hostname", &[]) {
        let h = h.trim();
        if !h.is_empty() {
            v.push(h.to_string());
        }
    }
    if let Some(ips) = ui::run_stdout("hostname", &["-I"]) {
        for ip in ips.split_whitespace() {
            v.push(ip.to_string());
        }
    }
    v.sort();
    v.dedup();
    v
}

fn san_ext(sans: &[String]) -> String {
    let parts: Vec<String> = sans
        .iter()
        .map(|s| {
            if s.parse::<std::net::IpAddr>().is_ok() {
                format!("IP:{s}")
            } else {
                format!("DNS:{s}")
            }
        })
        .collect();
    format!("subjectAltName={}", parts.join(","))
}

#[cfg(unix)]
fn chmod_600(path: &str) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
}
#[cfg(not(unix))]
fn chmod_600(_: &str) {}

/// Ensure the fleet CA exists in this node's active CA dir (generate once, race-safe on a shared
/// store). Returns the CA cert + key paths, or `None` if there's no CA dir.
fn ensure_ca() -> Option<(String, String)> {
    ensure_ca_in(&ca_dir()?)
}

/// Force-create (once) a CA in the local dir so this node can **found** a store-less fleet — the CA
/// it hands out in join codes. Returns the cert + key paths.
pub fn ensure_local_ca() -> Option<(String, String)> {
    ensure_ca_in(&local_ca_dir())
}

/// The active fleet CA's cert + key **contents**, for embedding in a join code (store-less trust
/// transfer). Founds a local CA when there's no store and none exists yet.
pub fn ca_material() -> Option<(String, String)> {
    let (cert, key) = ensure_ca().or_else(ensure_local_ca)?;
    Some((
        std::fs::read_to_string(&cert).ok()?,
        std::fs::read_to_string(&key).ok()?,
    ))
}

/// Ensure a CA exists in `dir` (generate once, race-safe). Returns the CA cert + key paths.
fn ensure_ca_in(dir: &str) -> Option<(String, String)> {
    let _ = std::fs::create_dir_all(dir);
    let ca_cert = format!("{dir}/ca.pem");
    let ca_key = format!("{dir}/ca.key");
    if Path::new(&ca_cert).exists() {
        return Some((ca_cert, ca_key));
    }
    // Claim generation by creating the key file exclusively; losers wait for ca.pem to appear.
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&ca_key)
    {
        Ok(_) => {
            let ok = ui::run_result(
                "openssl",
                &[
                    "req",
                    "-x509",
                    "-newkey",
                    "ec",
                    "-pkeyopt",
                    "ec_paramgen_curve:prime256v1",
                    "-nodes",
                    "-days",
                    "3650",
                    "-subj",
                    "/CN=Tendril Federation CA",
                    "-addext",
                    "basicConstraints=critical,CA:TRUE",
                    "-addext",
                    "keyUsage=critical,keyCertSign,cRLSign",
                    "-keyout",
                    &ca_key,
                    "-out",
                    &ca_cert,
                ],
            );
            chmod_600(&ca_key);
            if ok.is_err() {
                let _ = std::fs::remove_file(&ca_key); // let another node retry
                return None;
            }
        }
        Err(_) => {
            // Another node is generating it — wait briefly for the cert to land.
            for _ in 0..50 {
                if Path::new(&ca_cert).exists() {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
        }
    }
    Path::new(&ca_cert).exists().then_some((ca_cert, ca_key))
}

/// Build (once) this node's identity: a local keypair with a cert signed by the shared CA, usable as
/// both a server (on the mTLS listener) and a client (calling peers). Cached for the process.
fn build_identity() -> Option<Identity> {
    let (ca_cert, ca_key) = ensure_ca()?;
    let id_dir = identity_dir();
    let _ = std::fs::create_dir_all(&id_dir);
    let ca_local = format!("{id_dir}/ca.pem");
    let node_key = format!("{id_dir}/node.key");
    let node_crt = format!("{id_dir}/node.crt");
    // Keep a local copy of the CA so curl --cacert points at a stable path (store may unmount).
    let _ = std::fs::copy(&ca_cert, &ca_local);

    if !(Path::new(&node_key).exists() && Path::new(&node_crt).exists()) {
        let csr = format!("{id_dir}/node.csr");
        let ext = format!("{id_dir}/node.ext");
        let name = crate::federation::node_name();
        // Key + CSR.
        ui::run_result(
            "openssl",
            &[
                "req",
                "-newkey",
                "ec",
                "-pkeyopt",
                "ec_paramgen_curve:prime256v1",
                "-nodes",
                "-keyout",
                &node_key,
                "-out",
                &csr,
                "-subj",
                &format!("/CN={name}"),
            ],
        )
        .ok()?;
        chmod_600(&node_key);
        // Extensions: SANs + both server/client EKU (this cert serves and calls).
        let extfile = format!(
            "{}\nextendedKeyUsage=serverAuth,clientAuth\n",
            san_ext(&sans())
        );
        std::fs::write(&ext, extfile).ok()?;
        // Sign with the CA.
        ui::run_result(
            "openssl",
            &[
                "x509",
                "-req",
                "-in",
                &csr,
                "-CA",
                &ca_cert,
                "-CAkey",
                &ca_key,
                "-CAcreateserial",
                "-days",
                "3650",
                "-extfile",
                &ext,
                "-out",
                &node_crt,
            ],
        )
        .ok()?;
        let _ = std::fs::remove_file(&csr);
    }
    Some(Identity {
        ca: ca_local,
        cert: node_crt,
        key: node_key,
    })
}

/// This node's federation identity, if a CA is reachable (cached once built).
pub fn identity() -> Option<Identity> {
    static CACHE: OnceLock<Mutex<Option<Identity>>> = OnceLock::new();
    let cell = CACHE.get_or_init(|| Mutex::new(None));
    let mut guard = cell.lock().unwrap();
    if guard.is_none() {
        *guard = build_identity();
    }
    guard.clone()
}

/// Whether this node can do federation mTLS (has an identity to present). When false, federation calls
/// fall back to plain TLS + the shared token.
pub fn available() -> bool {
    identity().is_some()
}

/// How to reach a peer's federation API: prefer its mTLS endpoint (verified via our CA, presenting our
/// client cert) when both sides support it; else its plain-TLS UI URL with `-k`. Returns the base URL
/// and the `curl` transport-security arguments to splice in before the endpoint.
pub fn transport(ui_url: &str, peer_fed: Option<&str>) -> (String, Vec<String>) {
    match (identity(), peer_fed) {
        (Some(id), Some(fed)) if !fed.trim().is_empty() => (
            fed.trim_end_matches('/').to_string(),
            vec![
                "--cacert".into(),
                id.ca,
                "--cert".into(),
                id.cert,
                "--key".into(),
                id.key,
                "-s".into(),
            ],
        ),
        _ => (ui_url.trim_end_matches('/').to_string(), vec!["-sk".into()]),
    }
}

/// `curl` transport-security args for calling an already-chosen federation endpoint: our client cert +
/// CA when we have an identity (the endpoint is then expected to be an mTLS URL), else `-sk`.
pub fn client_args() -> Vec<String> {
    match identity() {
        Some(id) => vec![
            "--cacert".into(),
            id.ca,
            "--cert".into(),
            id.cert,
            "--key".into(),
            id.key,
            "-s".into(),
        ],
        None => vec!["-sk".into()],
    }
}

// ── rustls server config for the mTLS listener ────────────────────────────────────────────────

fn load_certs(path: &str) -> Option<Vec<rustls::pki_types::CertificateDer<'static>>> {
    let f = std::fs::File::open(path).ok()?;
    let mut rd = BufReader::new(f);
    let certs: Result<Vec<_>, _> = rustls_pemfile::certs(&mut rd).collect();
    certs.ok().filter(|v| !v.is_empty())
}

fn load_key(path: &str) -> Option<rustls::pki_types::PrivateKeyDer<'static>> {
    let f = std::fs::File::open(path).ok()?;
    let mut rd = BufReader::new(f);
    rustls_pemfile::private_key(&mut rd).ok().flatten()
}

/// A rustls server config that requires a client cert signed by the federation CA and presents this
/// node's cert. `None` when this node has no identity (→ don't start the mTLS listener).
pub fn server_config() -> Option<Arc<rustls::ServerConfig>> {
    let id = identity()?;
    let mut roots = rustls::RootCertStore::empty();
    for c in load_certs(&id.ca)? {
        roots.add(c).ok()?;
    }
    let verifier = rustls::server::WebPkiClientVerifier::builder(Arc::new(roots))
        .build()
        .ok()?;
    let certs = load_certs(&id.cert)?;
    let key = load_key(&id.key)?;
    let cfg = rustls::ServerConfig::builder()
        .with_client_cert_verifier(verifier)
        .with_single_cert(certs, key)
        .ok()?;
    Some(Arc::new(cfg))
}

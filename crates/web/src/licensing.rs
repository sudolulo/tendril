//! NVIDIA vGPU guest licensing via **FastAPI-DLS** (opt-in).
//!
//! The host `.run` makes vGPU *work*; NVIDIA licensing makes it run *un-throttled*. Each guest's vGPU
//! driver leases a license on boot — unlicensed it runs degraded and drops sessions (~24h). This runs
//! a self-hosted [FastAPI-DLS](https://git.collinwebdesigns.de/oscar.krause/fastapi-dls) — a minimal
//! Delegated License Service the guests lease from — as a podman container Tendril manages, and shows
//! the per-guest setup snippets.
//!
//! It is **separate from** the driver (you need the `.run` regardless) and **off by default**.
//! Emulating NVIDIA's licensing is a gray area — enable it only with your own vGPU entitlement.

use axum::extract::Form;
use maud::{html, Markup};
use serde::Deserialize;

use crate::ui;

const CONTAINER: &str = "tendril-dls";
const IMAGE: &str = "collinwebdesigns/fastapi-dls:latest";
const DATA_DIR: &str = "/var/lib/tendril/dls";

fn conf_path() -> String {
    std::env::var("TENDRIL_DLS_CONF").unwrap_or_else(|_| "/etc/tendril/dls.conf".to_string())
}

/// Configured DLS settings (with sane defaults). `url`/`port` are what **guests** reach the server at,
/// so they must be routable from the VMs (default: this host's LAN IP, a non-443 port to avoid the web
/// UI).
struct DlsConf {
    url: String,
    port: u16,
    lease_days: u32,
}

fn default_url() -> String {
    ui::run_stdout("hostname", &["-I"])
        .and_then(|s| s.split_whitespace().next().map(str::to_string))
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "127.0.0.1".to_string())
}

fn read_conf() -> DlsConf {
    let mut url = default_url();
    let mut port = 8443u16;
    let mut lease_days = 90u32;
    if let Ok(txt) = std::fs::read_to_string(conf_path()) {
        for line in txt.lines() {
            if let Some((k, v)) = line.split_once('=') {
                let v = v.trim();
                match k.trim() {
                    "url" if !v.is_empty() => url = v.to_string(),
                    "port" => {
                        if let Ok(p) = v.parse() {
                            port = p;
                        }
                    }
                    "lease_days" => {
                        if let Ok(d) = v.parse() {
                            lease_days = d;
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    DlsConf {
        url,
        port,
        lease_days,
    }
}

fn write_conf(c: &DlsConf) -> Result<(), String> {
    let p = conf_path();
    if let Some(d) = std::path::Path::new(&p).parent() {
        let _ = std::fs::create_dir_all(d);
    }
    std::fs::write(
        &p,
        format!(
            "url={}\nport={}\nlease_days={}\n",
            c.url, c.port, c.lease_days
        ),
    )
    .map_err(|e| e.to_string())
}

/// Whether the DLS container is currently running (`podman inspect` state). `false` if podman is
/// missing or the container doesn't exist.
fn running() -> bool {
    ui::run_stdout(
        "podman",
        &["inspect", "-f", "{{.State.Running}}", CONTAINER],
    )
    .map(|s| s.trim() == "true")
    .unwrap_or(false)
}

/// Generate the DLS web-server TLS cert (guests connect over HTTPS) if absent. FastAPI-DLS creates its
/// own lease-signing instance keys on first run.
fn ensure_cert(url: &str) -> Result<String, String> {
    let cert_dir = format!("{DATA_DIR}/cert");
    std::fs::create_dir_all(&cert_dir).map_err(|e| e.to_string())?;
    let crt = format!("{cert_dir}/webserver.crt");
    let key = format!("{cert_dir}/webserver.key");
    if std::path::Path::new(&crt).exists() && std::path::Path::new(&key).exists() {
        return Ok(cert_dir);
    }
    let san = if url.parse::<std::net::IpAddr>().is_ok() {
        format!("subjectAltName=IP:{url}")
    } else {
        format!("subjectAltName=DNS:{url}")
    };
    ui::run_result(
        "openssl",
        &[
            "req",
            "-x509",
            "-nodes",
            "-days",
            "3650",
            "-newkey",
            "rsa:2048",
            "-subj",
            &format!("/CN={url}"),
            "-addext",
            &san,
            "-keyout",
            &key,
            "-out",
            &crt,
        ],
    )
    .map_err(|e| format!("openssl (DLS webserver cert) failed: {e}"))?;
    Ok(cert_dir)
}

/// Start (or restart) the FastAPI-DLS container with the given settings.
fn start(c: &DlsConf) -> Result<(), String> {
    let cert_dir = ensure_cert(&c.url)?;
    let db_dir = format!("{DATA_DIR}/db");
    std::fs::create_dir_all(&db_dir).map_err(|e| e.to_string())?;
    // Replace any prior container so settings changes take effect.
    let _ = ui::run_result("podman", &["rm", "-f", CONTAINER]);
    let port_map = format!("{}:443", c.port);
    let e_url = format!("DLS_URL={}", c.url);
    let e_port = format!("DLS_PORT={}", c.port);
    let e_lease = format!("LEASE_EXPIRE_DAYS={}", c.lease_days);
    let cert_vol = format!("{cert_dir}:/app/cert:Z");
    let db_vol = format!("{db_dir}:/app/database:Z");
    ui::run_result(
        "podman",
        &[
            "run",
            "-d",
            "--name",
            CONTAINER,
            "--restart=always",
            "-p",
            &port_map,
            "-e",
            &e_url,
            "-e",
            &e_port,
            "-e",
            &e_lease,
            "-e",
            "DATABASE=sqlite:////app/database/db.sqlite",
            "-e",
            "TZ=UTC",
            "-v",
            &cert_vol,
            "-v",
            &db_vol,
            IMAGE,
        ],
    )
    .map(|_| ())
    .map_err(|e| format!("podman run (fastapi-dls) failed: {e}"))
}

#[derive(Deserialize)]
pub struct DlsForm {
    url: String,
    port: u16,
    lease_days: u32,
}

pub async fn enable(Form(f): Form<DlsForm>) -> Markup {
    if ui::is_demo() {
        return panel_with(Some(html! { div.banner.warn { "Disabled in the demo." } }));
    }
    let url = f.url.trim().to_string();
    if url.is_empty() || f.port == 0 {
        return panel_with(Some(
            html! { div.banner.error { "Provide the address guests will reach the license server at, and a port." } },
        ));
    }
    let c = DlsConf {
        url,
        port: f.port,
        lease_days: f.lease_days.max(1),
    };
    if let Err(e) = write_conf(&c) {
        return panel_with(Some(
            html! { div.banner.error { "Couldn't save settings: " (e) } },
        ));
    }
    match start(&c) {
        Ok(()) => panel_with(Some(
            html! { div.banner.ok { "License server started. Point your NVIDIA vGPU guests at it with the steps below." } },
        )),
        Err(e) => panel_with(Some(html! { div.banner.error { (e) } })),
    }
}

pub async fn disable() -> Markup {
    if ui::is_demo() {
        return panel_with(Some(html! { div.banner.warn { "Disabled in the demo." } }));
    }
    match ui::run_result("podman", &["rm", "-f", CONTAINER]) {
        Ok(_) => panel_with(Some(html! { div.banner.ok { "License server stopped." } })),
        Err(e) => panel_with(Some(
            html! { div.banner.error { "Couldn't stop it: " (e) } },
        )),
    }
}

pub fn panel() -> Markup {
    panel_with(None)
}

fn panel_with(banner: Option<Markup>) -> Markup {
    let c = read_conf();
    let on = running();
    let token_url = format!("https://{}:{}/-/client-token", c.url, c.port);
    ui::panel(
        "vGPU licensing (NVIDIA)",
        Some(if on { "running" } else { "off" }),
        html! {
            div.pad #dls-panel {
                @if let Some(b) = banner { (b) }
                p.sub style="margin-top:0" {
                    "NVIDIA vGPU guests run throttled and drop sessions (~24h) unless they lease a license. "
                    "This runs a self-hosted "
                    a href="https://git.collinwebdesigns.de/oscar.krause/fastapi-dls" { "FastAPI-DLS" }
                    " server the guests lease from — full performance, no NVIDIA license server. It's separate "
                    "from the driver (you still need the " code { ".run" } "). Emulating NVIDIA's licensing is a "
                    "gray area — enable it only with your own vGPU entitlement."
                }
                form hx-post="/hardware/dls/enable" hx-target="#dls-panel" hx-swap="outerHTML" {
                    div style="display:grid; grid-template-columns:1fr 1fr 1fr; gap:10px; align-items:end" {
                        div.field style="margin:0" {
                            label { "Address guests reach it at" }
                            input type="text" name="url" value=(c.url) required
                                title="Hostname or IP routable from the VMs — must match the server cert";
                        }
                        div.field style="margin:0" {
                            label { "Port" }
                            input type="number" name="port" min="1" max="65535" value=(c.port)
                                title="Host port for the DLS (kept off 443 so it doesn't clash with this web UI)";
                        }
                        div.field style="margin:0" {
                            label { "Lease days" }
                            input type="number" name="lease_days" min="1" max="365" value=(c.lease_days);
                        }
                    }
                    div.btnrow style="margin-top:12px" {
                        button.btn.primary type="submit" { @if on { "Apply / restart" } @else { "Start license server" } }
                        @if on {
                            button.btn.danger type="button"
                                hx-post="/hardware/dls/disable" hx-target="#dls-panel" hx-swap="outerHTML"
                                hx-confirm="Stop the license server? Guests will fall back to unlicensed (throttled) after their lease expires." { "Stop" }
                        }
                    }
                }
                @if on {
                    div style="margin-top:16px; padding-top:14px; border-top:1px solid var(--line)" {
                        div.sub style="font-weight:600; margin-bottom:6px" { "Point a guest at the license server" }
                        p.sub style="margin:0 0 8px" { "Run inside each NVIDIA-vGPU station after its guest driver is installed. Token URL: " code { (token_url) } }
                        div.sub { b { "Windows" } " (PowerShell, as admin):" }
                        (cmd(&format!(
                            "$d = \"C:\\Program Files\\NVIDIA Corporation\\vGPU Licensing\\ClientConfigToken\"\ncurl.exe --insecure -L \"{token_url}\" -o \"$d\\client_config_token_$(Get-Date -f dd-MM-yy-HH-mm-ss).tok\"\nRestart-Service NVDisplay.ContainerLocalSystem"
                        )))
                        div.sub { b { "Linux" } " (SteamOS / Ubuntu guest):" }
                        (cmd(&format!(
                            "sudo curl --insecure -L \"{token_url}\" \\\n  -o /etc/nvidia/ClientConfigToken/client_configuration_token_$(date '+%d-%m-%Y-%H-%M-%S').tok\nsudo sed -i 's/^#*FeatureType=.*/FeatureType=1/' /etc/nvidia/gridd.conf\nsudo systemctl restart nvidia-gridd"
                        )))
                    }
                }
            }
        },
    )
}

/// A shell-command block (matches the vGPU driver guide styling).
fn cmd(text: &str) -> Markup {
    html! { pre.mono style="margin:6px 0; padding:8px 10px; background:var(--bg2,#0002); border-radius:6px; overflow-x:auto; font-size:12.5px" { (text) } }
}

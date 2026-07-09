//! NVIDIA vGPU guest licensing — automatic, folded into the vGPU host-driver panel.
//!
//! The host `.run` makes vGPU *work*; NVIDIA licensing makes it run *un-throttled*. Each guest's vGPU
//! driver leases a license on boot — unlicensed it runs degraded and drops sessions (~24h). So licensing
//! isn't optional for NVIDIA vGPU, and it's not a separate step:
//!
//! - **Built-in (default):** Tendril runs a self-hosted
//!   [FastAPI-DLS](https://git.collinwebdesigns.de/oscar.krause/fastapi-dls) container the guests lease
//!   from. It **auto-starts** (sane defaults) as soon as the vGPU host driver is active — staging the
//!   licensed `.run` (only obtainable with a vGPU entitlement) is the single gate — and provisioning
//!   auto-installs the token into each guest. The admin pastes nothing.
//! - **Your own license server:** if you already run a real NVIDIA license server (on-prem DLS/NLS
//!   appliance or CLS), point Tendril at its client-token URL and it **won't run the built-in one at
//!   all** — a valid license means you don't need Tendril's emulation.
//!
//! This module renders a *fragment* embedded in the vGPU panel ([`crate::hardware`]), not its own panel.

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

/// Which licensing path is in effect. Built-in is the default (automatic); external means the admin
/// runs their own real NVIDIA license server and Tendril runs none.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Builtin,
    External,
}

/// Configured DLS settings (with sane defaults). For [`Mode::Builtin`], `url`/`port` are what **guests**
/// reach the server at, so they must be routable from the VMs (default: this host's LAN IP, a non-443
/// port to avoid the web UI). For [`Mode::External`], `external` is the admin's own client-token URL.
struct DlsConf {
    mode: Mode,
    external: String,
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
    // Built-in is the default — licensing needs no opt-in; staging the host driver is the entitlement
    // gate. External only when the admin explicitly points at their own server.
    let mut mode = Mode::Builtin;
    let mut external = String::new();
    let mut url = default_url();
    let mut port = 8443u16;
    let mut lease_days = 90u32;
    if let Ok(txt) = std::fs::read_to_string(conf_path()) {
        for line in txt.lines() {
            if let Some((k, v)) = line.split_once('=') {
                let v = v.trim();
                match k.trim() {
                    "mode" if v == "external" => mode = Mode::External,
                    "external" if !v.is_empty() => external = v.to_string(),
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
    // A stored external URL implies external mode even if the mode line is stale/missing.
    if !external.is_empty() {
        mode = Mode::External;
    }
    DlsConf {
        mode,
        external,
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
    let mode = match c.mode {
        Mode::Builtin => "builtin",
        Mode::External => "external",
    };
    std::fs::write(
        &p,
        format!(
            "mode={}\nexternal={}\nurl={}\nport={}\nlease_days={}\n",
            mode, c.external, c.url, c.port, c.lease_days
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
    // openssl writes the key with the default umask (usually 0644) — lock it down to 0600.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o600));
    }
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

/// Idempotently ensure the built-in license server is running — the automatic path. No-op unless the
/// built-in mode is in effect (i.e. the admin hasn't pointed at their own server), this host's vGPU is
/// actually active, and it isn't already running. Never touches anything in demo mode. Called at startup
/// (and after settings change) so licensing "just happens" once the vGPU host driver loads.
pub fn ensure_auto_started() {
    if ui::is_demo() {
        return;
    }
    let c = read_conf();
    if c.mode == Mode::External || running() {
        return;
    }
    if !crate::hardware::nvidia_vgpu_active() {
        return;
    }
    let _ = start(&c);
}

/// The client-token URL to hand a new NVIDIA-vGPU guest so it leases a license automatically, or `None`
/// if licensing isn't ready. For [`Mode::External`] this is the admin's own server; for [`Mode::Builtin`]
/// it's the running FastAPI-DLS. Station provisioning consumes this so the admin pastes nothing.
pub fn guest_token_url() -> Option<String> {
    let c = read_conf();
    match c.mode {
        Mode::External if !c.external.is_empty() => Some(c.external),
        Mode::Builtin if running() => Some(format!("https://{}:{}/-/client-token", c.url, c.port)),
        _ => None,
    }
}

#[derive(Deserialize)]
pub struct BuiltinForm {
    #[serde(default)]
    port: Option<u16>,
    #[serde(default)]
    lease_days: Option<u32>,
}

/// Switch to / re-apply the built-in license server (also the way back from an external server): record
/// built-in mode, then start it now if vGPU is already active, else it auto-starts when the driver loads.
pub async fn use_builtin(Form(f): Form<BuiltinForm>) -> Markup {
    if ui::is_demo() {
        return fragment_with(Some(html! { div.banner.warn { "Disabled in the demo." } }));
    }
    let mut c = read_conf();
    c.mode = Mode::Builtin;
    c.external.clear();
    if let Some(p) = f.port {
        if p > 0 {
            c.port = p;
        }
    }
    if let Some(d) = f.lease_days {
        c.lease_days = d.max(1);
    }
    if let Err(e) = write_conf(&c) {
        return fragment_with(Some(
            html! { div.banner.error { "Couldn't save settings: " (e) } },
        ));
    }
    let banner = if crate::hardware::nvidia_vgpu_active() {
        match start(&c) {
            Ok(()) => {
                html! { div.banner.ok { "Built-in license server running — new NVIDIA-vGPU stations are licensed automatically." } }
            }
            Err(e) => html! { div.banner.error { (e) } },
        }
    } else {
        html! { div.banner.ok { "Using the built-in license server — it starts automatically once the vGPU host driver is active." } }
    };
    fragment_with(Some(banner))
}

#[derive(Deserialize)]
pub struct ExternalForm {
    url: String,
}

/// Point Tendril at the admin's own real NVIDIA license server and stop/never-run the built-in one.
pub async fn use_external(Form(f): Form<ExternalForm>) -> Markup {
    if ui::is_demo() {
        return fragment_with(Some(html! { div.banner.warn { "Disabled in the demo." } }));
    }
    let url = f.url.trim().to_string();
    // Written line-by-line into dls.conf and later fetched by guests — require a clean http(s) URL so a
    // newline can't inject config lines and the scheme is a real one.
    if !crate::ui::is_http_url(&url) {
        return fragment_with(Some(
            html! { div.banner.error { "Enter your license server's client-token URL as an http(s):// address." } },
        ));
    }
    let mut c = read_conf();
    c.mode = Mode::External;
    c.external = url;
    if let Err(e) = write_conf(&c) {
        return fragment_with(Some(
            html! { div.banner.error { "Couldn't save settings: " (e) } },
        ));
    }
    // Tear down the built-in server — with a real license they don't need it.
    let _ = ui::run_result("podman", &["rm", "-f", CONTAINER]);
    fragment_with(Some(html! { div.banner.ok {
        "Using your license server. Tendril's built-in server is off — new vGPU stations will be pointed at yours."
    } }))
}

/// The licensing block, embedded inside the vGPU panel (not its own panel). Read-only status by default;
/// the built-in server is automatic, with an Advanced drawer to tune it or switch to your own server.
pub fn fragment() -> Markup {
    fragment_with(None)
}

fn fragment_with(banner: Option<Markup>) -> Markup {
    let c = read_conf();
    let active = crate::hardware::nvidia_vgpu_active();
    let on = running();
    html! {
        div #dls-panel {
            @if let Some(b) = banner { (b) }
            @match c.mode {
                Mode::External => (external_body(&c)),
                Mode::Builtin => (builtin_body(&c, active, on)),
            }
        }
    }
}

/// Steady state for the built-in server: read-only status + Advanced tweaks / switch to your own.
fn builtin_body(c: &DlsConf, active: bool, on: bool) -> Markup {
    let token_url = format!("https://{}:{}/-/client-token", c.url, c.port);
    html! {
        @if on {
            p.sub style="margin:0" { "Licensing: automatic — built-in server running ✓, guests licensed on first boot." }
        } @else if active {
            p.sub style="margin:0" { "Licensing: automatic — built-in server starting (reload, or re-apply under Advanced)." }
        } @else {
            p.sub style="margin:0" { "Licensing: automatic — the built-in server starts once vGPU is active." }
        }
        details style="margin-top:10px" {
            summary.sub style="cursor:pointer" { "Advanced" }
            div style="margin-top:10px" {
                form hx-post="/hardware/dls/builtin" hx-target="#dls-panel" hx-swap="outerHTML" {
                    p.sub style="margin:0 0 8px" { "Guests reach the built-in server at this host's IP (" code { (c.url) } ") — change the port or lease length only if needed." }
                    div style="display:grid; grid-template-columns:1fr 1fr; gap:10px; align-items:end" {
                        div.field style="margin:0" {
                            label { "Port" }
                            input type="number" name="port" min="1" max="65535" value=(c.port);
                        }
                        div.field style="margin:0" {
                            label { "Lease days" }
                            input type="number" name="lease_days" min="1" max="365" value=(c.lease_days);
                        }
                    }
                    button.btn type="submit" style="margin-top:10px" { "Apply / restart" }
                }
                div style="margin-top:14px; padding-top:12px; border-top:1px solid var(--line)" {
                    p.sub style="margin:0 0 6px" { b { "Have your own NVIDIA license server?" } " Point Tendril at it (an on-prem DLS/NLS appliance or CLS) and the built-in one is turned off — a valid license means you don't need Tendril's:" }
                    form hx-post="/hardware/dls/external" hx-target="#dls-panel" hx-swap="outerHTML" {
                        div.field style="margin:0 0 8px" {
                            label { "Your client-token URL" }
                            input type="url" name="url" required placeholder="https://nls.example.internal/-/client-token";
                        }
                        button.btn type="submit" { "Use my license server" }
                    }
                }
                @if on {
                    div style="margin-top:14px; padding-top:12px; border-top:1px solid var(--line)" {
                        div.sub style="font-weight:600; margin-bottom:6px" { "Manual token (troubleshooting)" }
                        p.sub style="margin:0 0 8px" { "Only if a guest didn't auto-license. Run inside the station after its guest driver is installed." }
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
        }
    }
}

/// Steady state for an external (real) license server.
fn external_body(c: &DlsConf) -> Markup {
    html! {
        p.sub style="margin:0" {
            "New NVIDIA-vGPU stations are pointed at your license server (" code { (c.external) } ") on first "
            "boot. Tendril's built-in server is off — with a valid license you don't need it."
        }
        details style="margin-top:10px" {
            summary.sub style="cursor:pointer" { "Change" }
            div style="margin-top:10px" {
                form hx-post="/hardware/dls/external" hx-target="#dls-panel" hx-swap="outerHTML" {
                    div.field style="margin:0 0 8px" {
                        label { "Your client-token URL" }
                        input type="url" name="url" value=(c.external) required;
                    }
                    button.btn type="submit" { "Update" }
                }
                div style="margin-top:14px; padding-top:12px; border-top:1px solid var(--line)" {
                    p.sub style="margin:0 0 6px" { "No license server of your own? Switch to Tendril's built-in one (auto-starts when vGPU is active):" }
                    form hx-post="/hardware/dls/builtin" hx-target="#dls-panel" hx-swap="outerHTML" {
                        button.btn type="submit" { "Use built-in license server" }
                    }
                }
            }
        }
    }
}

/// A shell-command block (matches the vGPU driver guide styling).
fn cmd(text: &str) -> Markup {
    html! { pre.mono style="margin:6px 0; padding:8px 10px; background:var(--bg2,#0002); border-radius:6px; overflow-x:auto; font-size:12.5px" { (text) } }
}

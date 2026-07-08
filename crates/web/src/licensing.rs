//! NVIDIA vGPU guest licensing — automatic, with a "bring your own license server" escape.
//!
//! The host `.run` makes vGPU *work*; NVIDIA licensing makes it run *un-throttled*. Each guest's vGPU
//! driver leases a license on boot — unlicensed it runs degraded and drops sessions (~24h). So licensing
//! isn't optional for NVIDIA vGPU, and Tendril makes it invisible:
//!
//! - **Built-in (default):** Tendril runs a self-hosted
//!   [FastAPI-DLS](https://git.collinwebdesigns.de/oscar.krause/fastapi-dls) container the guests lease
//!   from. It **auto-starts** (sane defaults) as soon as the vGPU host driver is active, and station
//!   provisioning auto-installs the token into each guest — the user pastes nothing. Emulating NVIDIA's
//!   license server is a gray area, so this is for admins who hold their own vGPU entitlement.
//! - **Your own license server:** if you already run a real NVIDIA license server (on-prem DLS/NLS
//!   appliance or CLS), point Tendril at its client-token URL and it **won't run the built-in one at
//!   all** — guests are licensed by your legitimate server. Having a valid license means you don't need
//!   Tendril's emulation.
//!
//! The choice is made once (a single informed opt-in); after that driver + license + guest driver are
//! automatic and silent. See the `tendril-vgpu-guest-driver-invisible` design note.

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

/// Which licensing path the admin has chosen for this host.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// Not chosen yet — prompt once (the single informed opt-in).
    Unset,
    /// Tendril's built-in FastAPI-DLS, auto-started when vGPU is active.
    Builtin,
    /// The admin's own real NVIDIA license server — Tendril never runs its own.
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
    let mut mode = Mode::Unset;
    let mut external = String::new();
    let mut url = default_url();
    let mut port = 8443u16;
    let mut lease_days = 90u32;
    if let Ok(txt) = std::fs::read_to_string(conf_path()) {
        for line in txt.lines() {
            if let Some((k, v)) = line.split_once('=') {
                let v = v.trim();
                match k.trim() {
                    "mode" => {
                        mode = match v {
                            "builtin" => Mode::Builtin,
                            "external" => Mode::External,
                            _ => Mode::Unset,
                        }
                    }
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
        Mode::Unset => "",
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
/// admin chose the built-in server, this host's vGPU is actually active, and it isn't already running.
/// Never touches anything in demo mode or when the admin runs their own license server. Called at
/// startup (and after the mode is chosen) so licensing "just happens" once the vGPU host driver loads.
pub fn ensure_auto_started() {
    if ui::is_demo() {
        return;
    }
    let c = read_conf();
    if c.mode != Mode::Builtin || running() {
        return;
    }
    if !crate::hardware::nvidia_vgpu_active() {
        return;
    }
    let _ = start(&c);
}

/// The client-token URL to hand a new NVIDIA-vGPU guest so it leases a license automatically, or `None`
/// if licensing isn't set up. For [`Mode::External`] this is the admin's own server; for [`Mode::Builtin`]
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

/// Choose (or re-apply) the built-in license server: record the mode, then start it now if vGPU is
/// already active, else it auto-starts when the host driver loads.
pub async fn use_builtin(Form(f): Form<BuiltinForm>) -> Markup {
    if ui::is_demo() {
        return panel_with(Some(html! { div.banner.warn { "Disabled in the demo." } }));
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
        return panel_with(Some(
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
        html! { div.banner.ok { "Built-in licensing selected — it starts automatically once the vGPU host driver is active." } }
    };
    panel_with(Some(banner))
}

#[derive(Deserialize)]
pub struct ExternalForm {
    url: String,
}

/// Point Tendril at the admin's own real NVIDIA license server and stop/never-run the built-in one.
pub async fn use_external(Form(f): Form<ExternalForm>) -> Markup {
    if ui::is_demo() {
        return panel_with(Some(html! { div.banner.warn { "Disabled in the demo." } }));
    }
    let url = f.url.trim().to_string();
    if url.is_empty() {
        return panel_with(Some(
            html! { div.banner.error { "Enter your license server's client-token URL (the endpoint guests fetch their token from)." } },
        ));
    }
    let mut c = read_conf();
    c.mode = Mode::External;
    c.external = url;
    if let Err(e) = write_conf(&c) {
        return panel_with(Some(
            html! { div.banner.error { "Couldn't save settings: " (e) } },
        ));
    }
    // Tear down the built-in server — with a real license they don't need it.
    let _ = ui::run_result("podman", &["rm", "-f", CONTAINER]);
    panel_with(Some(html! { div.banner.ok {
        "Using your license server. Tendril's built-in server is off — new vGPU stations will be pointed at yours."
    } }))
}

pub fn panel() -> Markup {
    panel_with(None)
}

fn panel_with(banner: Option<Markup>) -> Markup {
    let c = read_conf();
    let active = crate::hardware::nvidia_vgpu_active();
    let on = running();
    let status = match c.mode {
        Mode::External => "your server",
        Mode::Builtin if on => "running",
        Mode::Builtin => "starts when active",
        Mode::Unset => "set up",
    };
    ui::panel(
        "vGPU licensing (NVIDIA)",
        Some(status),
        html! {
            div.pad #dls-panel {
                @if let Some(b) = banner { (b) }
                @match c.mode {
                    Mode::External => (external_body(&c)),
                    Mode::Builtin => (builtin_body(&c, active, on)),
                    Mode::Unset => (choose_body(active)),
                }
            }
        },
    )
}

/// First-run: the single informed opt-in. NVIDIA vGPU can't run un-throttled without a license, so the
/// admin picks one path once. Either records the mode and, from then on, licensing is automatic.
fn choose_body(active: bool) -> Markup {
    html! {
        p.sub style="margin-top:0" {
            "NVIDIA vGPU guests throttle and drop sessions (~24h) unless they lease a license — so this "
            "isn't optional. Pick once; after that it's automatic and you paste nothing into guests."
        }
        @if !active {
            p.sub { "vGPU isn't active on this host yet — set this up after the vGPU host driver is loaded, or go ahead and choose now and it'll take effect then." }
        }
        div style="display:grid; gap:12px; margin-top:10px" {
            div style="padding:10px 12px; border:1px solid var(--line); border-radius:8px" {
                div.sub style="font-weight:600; margin-bottom:4px" { "I have my own NVIDIA license server" }
                p.sub style="margin:0 0 8px" { "Running a real on-prem DLS/NLS appliance or CLS? Point guests at it — Tendril won't run its own. A valid license means you don't need Tendril's." }
                form hx-post="/hardware/dls/external" hx-target="#dls-panel" hx-swap="outerHTML" {
                    div.field style="margin:0 0 8px" {
                        label { "Your client-token URL" }
                        input type="url" name="url" required placeholder="https://nls.example.internal/-/client-token";
                    }
                    button.btn.primary type="submit" { "Use my license server" }
                }
            }
            div style="padding:10px 12px; border:1px solid var(--line); border-radius:8px" {
                div.sub style="font-weight:600; margin-bottom:4px" { "Use Tendril's built-in license server" }
                p.sub style="margin:0 0 8px" {
                    "Tendril runs a self-hosted "
                    a href="https://git.collinwebdesigns.de/oscar.krause/fastapi-dls" { "FastAPI-DLS" }
                    " server with sensible defaults and licenses guests automatically. Emulating NVIDIA's "
                    "licensing is a gray area — choose this only if you hold your own NVIDIA vGPU entitlement."
                }
                form hx-post="/hardware/dls/builtin" hx-target="#dls-panel" hx-swap="outerHTML" {
                    button.btn type="submit" { "Use built-in (I hold a vGPU entitlement)" }
                }
            }
        }
    }
}

/// Steady state for the built-in server: read-only status + Advanced tweaks / switch to your own.
fn builtin_body(c: &DlsConf, active: bool, on: bool) -> Markup {
    let token_url = format!("https://{}:{}/-/client-token", c.url, c.port);
    html! {
        div.sub style="font-weight:600; margin:0 0 4px" { "Automatic — built-in license server" }
        @if on {
            p.sub style="margin:0" {
                "Running at " code { (token_url) } ". New NVIDIA-vGPU stations are licensed automatically on "
                "first boot — nothing to paste into guests."
            }
        } @else if active {
            p.sub style="margin:0" { "Selected, but not running yet — it should come up automatically; reload, or re-apply under Advanced." }
        } @else {
            p.sub style="margin:0" { "Selected. It starts automatically as soon as the vGPU host driver is active on this host." }
        }
        details style="margin-top:12px" {
            summary.sub style="cursor:pointer" { "Advanced" }
            div style="margin-top:10px" {
                form hx-post="/hardware/dls/builtin" hx-target="#dls-panel" hx-swap="outerHTML" {
                    p.sub style="margin:0 0 8px" { "Guests reach the server at this host's IP (" code { (c.url) } ") — change the port or lease length only if needed." }
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
                    p.sub style="margin:0 0 6px" { "Have your own NVIDIA license server instead? Switch to it (turns the built-in one off):" }
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
        div.sub style="font-weight:600; margin:0 0 4px" { "Using your license server" }
        p.sub style="margin:0" {
            "New NVIDIA-vGPU stations are pointed at " code { (c.external) } " on first boot. Tendril's "
            "built-in server is off — with a valid license you don't need it."
        }
        details style="margin-top:12px" {
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
                    p.sub style="margin:0 0 6px" { "No license server of your own? Switch to Tendril's built-in one:" }
                    form hx-post="/hardware/dls/builtin" hx-target="#dls-panel" hx-swap="outerHTML" {
                        button.btn type="submit" { "Use built-in (I hold a vGPU entitlement)" }
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

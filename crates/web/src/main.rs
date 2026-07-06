//! `tendril-web` — the Tendril web control plane.
//!
//! Axum + HTMX over the same services the console and CLI use (`capability-engine::detect`,
//! `orchestrator::provision`, `lifecycle::Libvirt`). Server-rendered HTML with small HTMX swaps; the
//! only vendored asset is `htmx.min.js`, embedded in the binary so the appliance serves everything
//! offline. This first slice is the dashboard: live hardware and stations with start/stop controls.

use axum::extract::Path;
use axum::http::header;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::Router;
use maud::{html, Markup, PreEscaped, DOCTYPE};

use tendril_capability_engine::{detect, GpuVendor, PassthroughViability};
use tendril_orchestrator::{DomainState, Libvirt};

/// htmx, embedded so the appliance needs no CDN.
const HTMX_JS: &str = include_str!("../assets/htmx.min.js");

#[tokio::main]
async fn main() {
    let app = Router::new()
        .route("/", get(dashboard))
        .route("/assets/htmx.min.js", get(htmx_js))
        .route("/stations", get(stations_fragment))
        .route("/stations/:name/start", post(action_start))
        .route("/stations/:name/stop", post(action_stop))
        .route("/stations/:name/forceoff", post(action_forceoff));

    let addr = std::env::var("TENDRIL_WEB_ADDR").unwrap_or_else(|_| "0.0.0.0:8080".to_string());
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .unwrap_or_else(|e| panic!("bind {addr}: {e}"));
    println!("Tendril web UI listening on http://{addr}");
    axum::serve(listener, app).await.expect("serve");
}

// ── routes ────────────────────────────────────────────────────────────────────────────────────

async fn dashboard() -> Markup {
    page(html! {
        section.card {
            h2 { "Hardware & capabilities" }
            (hardware_table())
        }
        section.card {
            h2 { "Stations" }
            (stations_fragment().await)
        }
    })
}

async fn htmx_js() -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "application/javascript")], HTMX_JS)
}

/// The self-refreshing stations panel — HTMX polls it and swaps it after each action.
async fn stations_fragment() -> Markup {
    stations_markup(&Libvirt::system())
}

async fn action_start(Path(name): Path<String>) -> Markup {
    act(&name, |lv| lv.start(&name))
}

async fn action_stop(Path(name): Path<String>) -> Markup {
    act(&name, |lv| lv.shutdown(&name))
}

async fn action_forceoff(Path(name): Path<String>) -> Markup {
    act(&name, |lv| lv.destroy(&name))
}

/// Run a lifecycle action and re-render the stations panel (with an error banner if it failed).
fn act(_name: &str, f: impl FnOnce(&Libvirt) -> std::io::Result<()>) -> Markup {
    let lv = Libvirt::system();
    let err = f(&lv).err().map(|e| e.to_string());
    html! {
        @if let Some(e) = err {
            div.banner.error { (e) }
        }
        (stations_markup(&lv))
    }
}

// ── views ─────────────────────────────────────────────────────────────────────────────────────

fn hardware_table() -> Markup {
    let matrix = detect();
    html! {
        @if matrix.gpus.is_empty() {
            p.muted { "No display devices found." }
        } @else {
            table {
                thead { tr { th { "GPU" } th { "Address" } th { "Capability" } th { "Passthrough" } } }
                tbody {
                    @for g in &matrix.gpus {
                        tr {
                            td { (vendor(g.gpu.vendor)) " " (g.gpu.model.as_deref().unwrap_or("GPU")) }
                            td.mono { (g.gpu.address) }
                            td { (format!("{:?}", g.capability)) }
                            td { (viability(g.viability)) }
                        }
                    }
                }
            }
            p.muted { (matrix.passthrough_capable().count()) " GPU(s) ready for passthrough." }
        }
    }
}

fn stations_markup(lv: &Libvirt) -> Markup {
    let names = lv.list();
    html! {
        // Polls itself every 5s and is replaced wholesale after each action.
        div #stations hx-get="/stations" hx-trigger="every 5s" hx-swap="outerHTML" {
            @if names.is_empty() {
                p.muted { "No stations defined yet." }
            } @else {
                table {
                    thead { tr { th { "Station" } th { "State" } th { "Actions" } } }
                    tbody {
                        @for name in &names {
                            (station_row(lv, name))
                        }
                    }
                }
            }
        }
    }
}

fn station_row(lv: &Libvirt, name: &str) -> Markup {
    let state = lv.state(name);
    let running = matches!(state, DomainState::Running);
    html! {
        tr {
            td { (name) }
            td { span.state.(state_class(state)) { (state_label(state)) } }
            td.actions {
                @if running {
                    (btn(name, "stop", "Shut down"))
                    (btn(name, "forceoff", "Force off"))
                } @else {
                    (btn(name, "start", "Start"))
                }
            }
        }
    }
}

fn btn(name: &str, action: &str, label: &str) -> Markup {
    html! {
        button.btn
            hx-post=(format!("/stations/{name}/{action}"))
            hx-target="#stations"
            hx-swap="outerHTML" { (label) }
    }
}

fn page(body: Markup) -> Markup {
    html! {
        (DOCTYPE)
        html lang="en" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                title { "Tendril" }
                script src="/assets/htmx.min.js" {}
                style { (PreEscaped(CSS)) }
            }
            body {
                header.topbar {
                    div.brand { "TENDRIL" }
                    div.sub { "GPU-passthrough gaming stations" }
                }
                main { (body) }
            }
        }
    }
}

fn vendor(v: GpuVendor) -> &'static str {
    match v {
        GpuVendor::Nvidia => "NVIDIA",
        GpuVendor::Amd => "AMD",
        GpuVendor::Intel => "Intel",
        GpuVendor::Unknown => "GPU",
    }
}

fn viability(v: PassthroughViability) -> &'static str {
    match v {
        PassthroughViability::Isolated => "isolated (clean)",
        PassthroughViability::SharedGroup => "shared group (ACS override)",
        PassthroughViability::NoIommu => "no IOMMU",
    }
}

fn state_label(s: DomainState) -> &'static str {
    match s {
        DomainState::Running => "running",
        DomainState::Paused => "paused",
        DomainState::ShutOff => "shut off",
        DomainState::Absent => "absent",
        DomainState::Other => "other",
    }
}

fn state_class(s: DomainState) -> &'static str {
    match s {
        DomainState::Running => "ok",
        DomainState::Paused => "warn",
        _ => "off",
    }
}

const CSS: &str = r#"
:root { --bg:#0f1216; --card:#171c22; --line:#262d36; --fg:#e6e9ee; --muted:#8b95a3;
        --accent:#5aa9ff; --ok:#3fb950; --warn:#d29922; --off:#6e7681; }
* { box-sizing:border-box; }
body { margin:0; background:var(--bg); color:var(--fg);
       font:15px/1.5 system-ui,-apple-system,Segoe UI,Roboto,sans-serif; }
.topbar { display:flex; align-items:baseline; gap:.75rem; padding:1rem 1.5rem;
          border-bottom:1px solid var(--line); background:#12161b; }
.brand { font-weight:700; letter-spacing:.25em; }
.sub { color:var(--muted); font-size:.85rem; }
main { max-width:960px; margin:1.5rem auto; padding:0 1rem; display:grid; gap:1.25rem; }
.card { background:var(--card); border:1px solid var(--line); border-radius:10px; padding:1rem 1.25rem; }
.card h2 { margin:.2rem 0 .8rem; font-size:1.05rem; }
table { width:100%; border-collapse:collapse; }
th,td { text-align:left; padding:.5rem .6rem; border-bottom:1px solid var(--line); }
th { color:var(--muted); font-weight:600; font-size:.8rem; text-transform:uppercase; letter-spacing:.04em; }
.mono { font-family:ui-monospace,SFMono-Regular,Menlo,monospace; color:var(--muted); }
.muted { color:var(--muted); }
.actions { display:flex; gap:.4rem; }
.btn { background:#20262e; color:var(--fg); border:1px solid var(--line); border-radius:6px;
       padding:.35rem .7rem; cursor:pointer; font-size:.85rem; }
.btn:hover { border-color:var(--accent); }
.state { font-size:.8rem; padding:.15rem .5rem; border-radius:999px; border:1px solid var(--line); }
.state.ok { color:var(--ok); } .state.warn { color:var(--warn); } .state.off { color:var(--off); }
.banner.error { background:#3d1418; border:1px solid #6e2630; color:#ffb4bb;
                padding:.6rem .8rem; border-radius:8px; margin-bottom:.8rem; }
"#;

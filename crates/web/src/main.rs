//! `tendril-web` — the Tendril web control plane.
//!
//! Axum + HTMX over the same services the console and CLI use (`capability-engine::detect`,
//! `orchestrator::provision`, `lifecycle::Libvirt`). Server-rendered HTML (Maud), HTMX for in-page
//! swaps, and a WebSocket↔VNC proxy driving an embedded noVNC console. All assets — htmx and noVNC —
//! are baked into the binary, so the appliance serves everything offline.

mod auth;
mod demo;
mod hardware;
mod network;
mod pages;
mod seats;
mod stations;
mod ui;

use axum::extract::Path;
use axum::http::{header, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{middleware, Router};

/// htmx, embedded so the appliance needs no CDN.
const HTMX_JS: &str = include_str!("../assets/htmx.min.js");
/// The noVNC client (ES modules + its zlib vendor), embedded and served under /assets/novnc/.
static NOVNC: include_dir::Dir = include_dir::include_dir!("$CARGO_MANIFEST_DIR/assets/novnc");

#[tokio::main]
async fn main() {
    // `tendril-web --set-password` (used by the console) reads a password from stdin and stores it.
    if std::env::args().any(|a| a == "--set-password") {
        set_password_cli();
        return;
    }

    let app = Router::new()
        .route("/", get(pages::dashboard))
        .route("/stats", get(pages::stats))
        // stations
        .route("/stations", get(stations::list_page).post(stations::create))
        .route("/stations/fragment", get(stations::fragment_route))
        .route("/stations/new", get(stations::new_form))
        .route("/stations/:name", get(stations::detail))
        .route("/stations/:name/start", post(stations::start))
        .route("/stations/:name/stop", post(stations::stop))
        .route("/stations/:name/forceoff", post(stations::forceoff))
        .route("/stations/:name/delete", post(stations::delete))
        .route("/stations/:name/usb/add/:id", post(stations::usb_add))
        .route("/stations/:name/usb/remove/:id", post(stations::usb_remove))
        .route("/stations/:name/sendenter", post(stations::send_enter))
        .route("/stations/:name/progress", get(stations::progress))
        .route("/stations/:name/vnc", get(stations::vnc_ws))
        // hardware
        .route("/hardware", get(hardware::page))
        .route("/hardware/:addr/bind", post(hardware::bind))
        .route("/seats", post(seats::create))
        .route("/seats/delete", post(seats::delete))
        // media + network
        .route("/media", get(pages::media))
        .route("/media/fetch/:which", post(pages::fetch))
        .route("/media/verify/:iso", post(pages::verify))
        .route("/media/verifystatus/:iso", get(pages::verify_status))
        .route("/network", get(network::page))
        .route("/network/apply", post(network::apply))
        .route("/network/confirm", post(network::confirm))
        .route("/network/revert", post(network::revert))
        // system / OS updates
        .route("/system", get(pages::system))
        .route("/system/check", post(pages::system_check))
        .route("/system/update", post(pages::system_update))
        .route("/system/auto", post(pages::system_auto))
        .route("/system/reboot", post(pages::system_reboot))
        .route("/system/shutdown", post(pages::system_shutdown))
        .route("/system/logs", get(pages::logs))
        .route("/system/logs/download", get(pages::logs_download))
        // auth
        .route("/login", get(auth::login_page).post(auth::login))
        .route("/logout", post(auth::logout))
        .route("/setup", get(auth::setup_page).post(auth::setup))
        // assets
        .route("/assets/htmx.min.js", get(htmx_js))
        .route("/assets/novnc/*path", get(novnc_asset))
        // gate everything above behind auth (the middleware lets the auth/asset paths through)
        .layer(middleware::from_fn(auth::require_auth));

    let addr = std::env::var("TENDRIL_WEB_ADDR").unwrap_or_else(|_| "0.0.0.0:8080".to_string());
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .unwrap_or_else(|e| panic!("bind {addr}: {e}"));
    println!("Tendril web UI listening on http://{addr}");
    axum::serve(listener, app).await.expect("serve");
}

/// Read a password from stdin and store it as the admin password (called via `--set-password`).
fn set_password_cli() {
    use std::io::Write as _;
    print!("New Tendril web admin password: ");
    let _ = std::io::stdout().flush();
    let mut pw = String::new();
    if std::io::stdin().read_line(&mut pw).is_err() {
        eprintln!("could not read password");
        std::process::exit(1);
    }
    let pw = pw.trim();
    if pw.chars().count() < 6 {
        eprintln!("password must be at least 6 characters");
        std::process::exit(1);
    }
    match auth::set_password(pw) {
        Ok(()) => println!("admin password set."),
        Err(e) => {
            eprintln!("failed to set password: {e}");
            std::process::exit(1);
        }
    }
}

async fn htmx_js() -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "application/javascript")], HTMX_JS)
}

async fn novnc_asset(Path(path): Path<String>) -> impl IntoResponse {
    match NOVNC.get_file(&path) {
        Some(f) => ([(header::CONTENT_TYPE, mime_for(&path))], f.contents()).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

fn mime_for(path: &str) -> &'static str {
    if path.ends_with(".js") || path.ends_with(".mjs") {
        "text/javascript"
    } else if path.ends_with(".css") {
        "text/css"
    } else if path.ends_with(".json") {
        "application/json"
    } else {
        "application/octet-stream"
    }
}

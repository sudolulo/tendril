//! `tendril-web` — the Tendril web control plane.
//!
//! Axum + HTMX over the same services the console and CLI use (`capability-engine::detect`,
//! `orchestrator::provision`, `lifecycle::Libvirt`). Server-rendered HTML (Maud), HTMX for in-page
//! swaps, and a WebSocket↔VNC proxy driving an embedded noVNC console. All assets — htmx and noVNC —
//! are baked into the binary, so the appliance serves everything offline.

mod auth;
mod demo;
mod federation;
mod fedtls;
mod hardware;
mod images;
mod licensing;
mod mdns;
mod network;
mod pages;
mod pxe;
mod seats;
mod stations;
mod storage;
mod tls;
mod ui;
mod vgpu;
mod vgpudrv;
mod vgpuguest;

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
    // `tendril-web --seed-default-password` (used by the unattended installer) reads a password from
    // stdin, stores it, and flags it as a must-change default — so the web console forces the user to
    // replace it on first sign-in.
    if std::env::args().any(|a| a == "--seed-default-password") {
        set_password_cli();
        auth::mark_password_default();
        return;
    }
    // `tendril-web --set-password` (used by the console) reads a password from stdin and stores it.
    if std::env::args().any(|a| a == "--set-password") {
        set_password_cli();
        return;
    }

    // Install the process-wide rustls crypto provider once, up front — both the federation mTLS
    // listener and the browser HTTPS server build rustls configs that require it.
    let _ = rustls::crypto::ring::default_provider().install_default();

    // A vGPU-build `.building` marker surviving into a fresh process is stale (the build thread died
    // with the old process) and would otherwise block builds forever.
    vgpudrv::clear_stale_build_marker();

    // vGPU guest licensing is automatic: if the admin chose the built-in license server and this host's
    // vGPU is active, make sure it's running. No-op in demo, when the admin runs their own license
    // server, or when vGPU isn't active yet (then it starts once the host driver loads). Off-thread
    // because it may pull a container image + generate a cert.
    std::thread::spawn(licensing::ensure_auto_started);

    let app = Router::new()
        // Stations is the landing page (the former Dashboard folded into it as a summary strip).
        .route("/", get(stations::list_page))
        .route("/stats", get(pages::stats))
        // federation
        .route("/fleet", get(federation::page))
        // Legacy fleet-create URL now folds into the unified Stations wizard (placement selector).
        .route(
            "/fleet/new",
            get(|| async { axum::response::Redirect::permanent("/stations/new") }),
        )
        .route("/fleet/create", post(federation::create))
        .route("/fleet/rehome", post(federation::rehome))
        .route("/fleet/setup/name", post(federation::setup_name))
        .route("/fleet/setup/rotate-token", post(federation::rotate_token))
        .route("/fleet/join-code", get(federation::join_code))
        .route("/fleet/join", post(federation::join))
        .route("/fleet/pxe/start", post(pxe::start))
        .route("/fleet/pxe/stop", post(pxe::stop))
        .route("/fleet/pxe/fetch", post(pxe::fetch))
        .route("/api/fleet/register", post(federation::api_fleet_register))
        // Control a peer's station from the Stations page (UI proxy → dispatches to the owning node).
        .route(
            "/fleet/:node/station/:name/:action",
            post(federation::peer_station_action),
        )
        // Self-refresh poll for a single peer's stations panel.
        .route("/fleet/:node/panel", get(federation::peer_panel_fragment))
        // Open a peer station (detail page with its console) + the cross-node console WS proxy.
        .route(
            "/fleet/:node/station/:name",
            get(federation::peer_station_detail),
        )
        .route(
            "/fleet/:node/station/:name/vnc",
            get(federation::peer_vnc_ws),
        )
        .route("/api/node", get(federation::api_node))
        .route("/api/provision", post(federation::api_provision))
        .route(
            "/api/station/:name/:action",
            post(federation::api_station_action),
        )
        // Peer-facing VNC bridge: exposes this node's local station console over the token/mTLS-authed
        // fed API (reuses the same relay as the browser console). It's the owning-node half of the
        // cross-node console — a fleet peer with the fed token can reach a station's VNC through here.
        // The browser-side proxy that consumes it (this node ↔ peer WebSocket) is a follow-up: it needs
        // a WS client + the fed client cert, and real cross-node hardware to validate.
        .route("/api/station/:name/vnc", get(stations::vnc_ws))
        .route("/api/reimage", post(federation::api_reimage))
        .route("/api/image/:name", get(federation::api_image))
        .route("/api/image-pull", post(federation::api_image_pull))
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
        .route("/stations/:name/save-image", post(images::save))
        .route("/stations/:name/resplit", post(stations::resplit_action))
        .route("/stations/:name/snapshot", post(stations::snapshot_create))
        .route(
            "/stations/:name/snapshot/revert",
            post(stations::snapshot_revert),
        )
        .route(
            "/stations/:name/snapshot/delete",
            post(stations::snapshot_delete),
        )
        .route("/stations/:name/vnc", get(stations::vnc_ws))
        .route("/images/delete", post(images::delete))
        .route("/images/panel", get(images::panel_route))
        .route("/images/verify", post(images::verify))
        .route("/images/verifystatus", get(images::verify_status))
        .route("/images/push", get(images::push_form).post(images::push))
        .route("/images/distribute", post(images::distribute))
        // hardware
        .route("/hardware", get(hardware::page))
        .route("/hardware/:addr/bind", post(hardware::bind))
        .route("/hardware/:addr/sriov", post(hardware::sriov))
        .route("/hardware/dls/builtin", post(licensing::use_builtin))
        .route("/hardware/dls/external", post(licensing::use_external))
        // Staging the NVIDIA .run is a multi-hundred-MB upload — lift the body limit on this route.
        .route(
            "/hardware/vgpu/run",
            post(vgpudrv::stage).layer(axum::extract::DefaultBodyLimit::disable()),
        )
        .route("/hardware/vgpu/run/clear", post(vgpudrv::clear))
        .route("/hardware/vgpu/build", post(vgpudrv::build))
        .route("/hardware/vgpu/buildstatus", get(vgpudrv::build_status))
        // The guest driver is fully automatic (fetched to match the host branch) — no staging routes.
        .route("/seats", post(seats::create))
        .route("/seats/delete", post(seats::delete))
        // media + network
        .route("/media", get(pages::media))
        .route("/media/isos", get(pages::media_isos))
        .route("/media/fetch/:which", post(pages::fetch))
        .route("/media/verify/:iso", post(pages::verify))
        .route("/media/verifystatus/:iso", get(pages::verify_status))
        .route("/storage/configure", post(storage::configure))
        .route("/storage/unmount", post(storage::unmount))
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
        .route("/system/password", post(auth::change_password))
        .route("/system/viewer", post(auth::set_viewer))
        .route("/system/audit/download", get(auth::audit_download))
        .route("/system/tls/upload", post(tls::upload))
        .route("/system/tls/regenerate", post(tls::regenerate))
        // auth
        .route("/login", get(auth::login_page).post(auth::login))
        .route("/logout", post(auth::logout))
        .route("/setup", get(auth::setup_page).post(auth::setup))
        // assets
        .route("/assets/htmx.min.js", get(htmx_js))
        .route("/assets/novnc/*path", get(novnc_asset))
        // gate everything above behind auth (the middleware lets the auth/asset paths through)
        .layer(middleware::from_fn(auth::require_auth));

    // Fleet heartbeat: publish this node's presence to the shared store every 30s so peers
    // auto-discover it. No-op without a shared store (federation then uses manual peers).
    std::thread::spawn(|| loop {
        federation::heartbeat();
        std::thread::sleep(std::time::Duration::from_secs(30));
    });

    // Advertise this node + browse for peers over mDNS so nearby machines appear in Fleet setup for
    // one-click joining (best-effort; no-op in the demo or where multicast is unavailable).
    mdns::start();

    // Federation mTLS listener: a separate port that requires a client cert signed by the shared
    // federation CA (and presents this node's cert). Runs alongside the browser UI so node-to-node
    // calls are mutually authenticated. Only starts when this node has a CA-issued identity (a shared
    // store, or TENDRIL_FED_CA_DIR); otherwise federation falls back to token + plain TLS.
    if let Some(fed_cfg) = fedtls::server_config() {
        let fed_app = app.clone();
        let fed_addr = fedtls::fed_addr();
        tokio::spawn(async move {
            match fed_addr.parse::<std::net::SocketAddr>() {
                Ok(sock) => {
                    let cfg = axum_server::tls_rustls::RustlsConfig::from_config(fed_cfg);
                    println!("Tendril federation mTLS on https://{fed_addr}");
                    if let Err(e) = axum_server::bind_rustls(sock, cfg)
                        .serve(fed_app.into_make_service())
                        .await
                    {
                        eprintln!("federation mTLS server error: {e}");
                    }
                }
                Err(e) => eprintln!("bad TENDRIL_FED_ADDR {fed_addr}: {e}"),
            }
        });
    }

    let addr = std::env::var("TENDRIL_WEB_ADDR").unwrap_or_else(|_| "0.0.0.0:8080".to_string());

    // HTTPS (opt-in via TENDRIL_TLS=on): terminate TLS in-app with a self-signed (or provided) cert.
    if tls::enabled() {
        match tls::ensure() {
            Ok((cert, key)) => {
                let config = axum_server::tls_rustls::RustlsConfig::from_pem_file(&cert, &key)
                    .await
                    .unwrap_or_else(|e| panic!("load TLS cert {cert}: {e}"));
                // Stash the live config so the UI can hot-swap the cert without a restart.
                tls::set_live_config(config.clone());
                let sock: std::net::SocketAddr =
                    addr.parse().unwrap_or_else(|e| panic!("addr {addr}: {e}"));
                // Optional HTTP→HTTPS redirect (e.g. :80 → :443) so bare-hostname visits still land.
                if let Ok(redir) = std::env::var("TENDRIL_HTTP_REDIRECT_ADDR") {
                    let https_port = sock.port();
                    tokio::spawn(async move {
                        let redirect = axum::Router::new().fallback(
                            move |headers: axum::http::HeaderMap, uri: axum::http::Uri| async move {
                                let host = headers
                                    .get(axum::http::header::HOST)
                                    .and_then(|v| v.to_str().ok())
                                    .unwrap_or("")
                                    .split(':')
                                    .next()
                                    .unwrap_or("")
                                    .to_string();
                                let pq = uri.path_and_query().map(|p| p.as_str()).unwrap_or("/");
                                let target = if https_port == 443 {
                                    format!("https://{host}{pq}")
                                } else {
                                    format!("https://{host}:{https_port}{pq}")
                                };
                                axum::response::Redirect::permanent(&target)
                            },
                        );
                        match tokio::net::TcpListener::bind(&redir).await {
                            Ok(l) => {
                                println!("HTTP→HTTPS redirect on http://{redir}");
                                if let Err(e) = axum::serve(l, redirect).await {
                                    eprintln!("redirect server error: {e}");
                                }
                            }
                            Err(e) => eprintln!("redirect bind {redir} failed: {e}"),
                        }
                    });
                }
                println!("Tendril web UI listening on https://{addr}");
                axum_server::bind_rustls(sock, config)
                    .serve(app.into_make_service())
                    .await
                    .expect("serve https");
                return;
            }
            Err(e) => eprintln!("TLS setup failed ({e}); serving plain HTTP"),
        }
    }

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

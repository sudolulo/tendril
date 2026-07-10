//! Fleet-staggered OS updates: stage + reboot every **reachable peer**, one node at a time, waiting
//! for each to come back before touching the next — so a bad image or a node that never returns
//! stops costing you one machine, not the whole fleet at once.
//!
//! This node is deliberately **not** in the rotation: updating it would reboot the web console
//! driving the orchestration. Update it last from its own System page.
//!
//! Progress lives in a state file (`/var/lib/tendril/fleet-update.state`, override
//! `TENDRIL_FLEET_UPDATE_STATE`): the background thread rewrites it before every step, the Fleet
//! panel polls it, and its **mtime** is the liveness signal — a line older than 30 minutes with no
//! progress counts as a dead run (e.g. tendril-web restarted mid-orchestration), so the panel
//! degrades to "no update running" instead of wedging the button forever.

use std::time::{Duration, Instant};

use maud::{html, Markup};
use serde::Serialize;

use crate::federation::{self, NodeInfo, Peer, ProvisionResult};
use crate::ui;

/// A state line older than this with no progress is a dead run. Matches the longest single blocking
/// step (the peer's `bootc upgrade`, capped at 30 min below) — every other step re-touches the file.
const STALE_AFTER_SECS: u64 = 30 * 60;
/// How long the peer's `bootc upgrade` may run (curl `--max-time`, seconds).
const UPDATE_MAX_TIME: &str = "1800";
/// Reboot polling: probe the peer's `/api/node` every 20s, for up to 10 minutes.
const REBOOT_POLL_SECS: u64 = 20;
const REBOOT_WAIT_MAX_SECS: u64 = 10 * 60;

// ── state file ───────────────────────────────────────────────────────────────────────────────

fn state_path() -> String {
    std::env::var("TENDRIL_FLEET_UPDATE_STATE")
        .unwrap_or_else(|_| "/var/lib/tendril/fleet-update.state".to_string())
}

/// Rewrite the state line (fresh mtime = the run is alive). Best-effort.
fn write_state(s: &str) {
    let p = state_path();
    if let Some(dir) = std::path::Path::new(&p).parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(&p, s);
}

/// The state line + its age, if the file exists (a future mtime reads as `None` → not running).
fn read_state() -> Option<(String, Duration)> {
    let p = state_path();
    let age = std::fs::metadata(&p)
        .ok()?
        .modified()
        .ok()?
        .elapsed()
        .ok()?;
    let line = std::fs::read_to_string(&p).ok()?;
    Some((line.trim().to_string(), age))
}

/// A terminal state line (the summary a finished run leaves behind for the panel to show).
fn is_done(s: &str) -> bool {
    s.starts_with("done:")
}

/// Pure liveness rule: a run is live while its line is non-terminal and fresh. Stale-but-present
/// covers a tendril-web restart mid-run — the orphaned marker must not block future runs.
fn state_live(line: &str, age_secs: u64) -> bool {
    age_secs < STALE_AFTER_SECS && !is_done(line)
}

/// Whether an orchestration is currently running (per the state file).
fn running() -> bool {
    read_state()
        .map(|(s, age)| state_live(&s, age.as_secs()))
        .unwrap_or(false)
}

fn summary(updated: usize, failed: usize, skipped: usize) -> String {
    format!("done: {updated} updated, {failed} failed, {skipped} skipped")
}

// ── federation API endpoints (this node as the update *target*) ─────────────────────────────

/// Run `bootc upgrade` to completion — the blocking core of the update endpoint, same call as the
/// System page's "Update now" but with the error text surfaced to the JSON envelope.
fn run_bootc_upgrade() -> Result<(), String> {
    match std::process::Command::new("bootc").arg("upgrade").output() {
        Ok(o) if o.status.success() => Ok(()),
        Ok(o) => {
            let e = String::from_utf8_lossy(&o.stderr).trim().to_string();
            Err(if e.is_empty() {
                format!("bootc exited {}", o.status)
            } else {
                e
            })
        }
        Err(e) => Err(format!("could not run bootc: {e}")),
    }
}

/// POST /api/system/update — stage the latest OS image on THIS node (token-authed; called by a
/// peer's fleet orchestrator). Blocks until `bootc upgrade` finishes, so the caller knows the image
/// is staged before it asks for the reboot.
pub async fn api_update() -> axum::Json<ProvisionResult> {
    if ui::is_demo() {
        return axum::Json(ProvisionResult::err("disabled in the demo"));
    }
    let res = tokio::task::spawn_blocking(run_bootc_upgrade)
        .await
        .unwrap_or_else(|_| Err("update task panicked".into()));
    axum::Json(res.into())
}

/// POST /api/system/reboot — reboot THIS node. The reboot is spawned detached (and reaped), so the
/// response goes out before the machine drops.
pub async fn api_reboot() -> axum::Json<ProvisionResult> {
    if ui::is_demo() {
        return axum::Json(ProvisionResult::err("disabled in the demo"));
    }
    match std::process::Command::new("systemctl")
        .arg("reboot")
        .spawn()
    {
        Ok(child) => {
            crate::pages::reap(child);
            axum::Json(ProvisionResult::ok())
        }
        Err(e) => axum::Json(ProvisionResult::err(format!(
            "could not run systemctl: {e}"
        ))),
    }
}

#[derive(Serialize)]
pub struct VersionInfo {
    version: &'static str,
    booted: String,
}

/// GET /api/system/version — this node's tendril-web version + `bootc status` text (empty on a
/// non-bootc host). A cheap identity check after a fleet-update reboot.
pub async fn api_version() -> axum::Json<VersionInfo> {
    let booted = if ui::is_demo() {
        String::new()
    } else {
        tokio::task::spawn_blocking(|| {
            ui::run_stdout("bootc", &["status"])
                .map(|s| s.trim().to_string())
                .unwrap_or_default()
        })
        .await
        .unwrap_or_default()
    };
    axum::Json(VersionInfo {
        version: env!("CARGO_PKG_VERSION"),
        booted,
    })
}

// ── orchestration (this node as the *driver*) ────────────────────────────────────────────────

/// Update one peer end to end: stage the image, reboot, wait for it to answer `/api/node` again.
/// Rewrites the state line before every step so the run reads as alive.
fn update_peer(p: &Peer, i: usize, n: usize) -> Result<(), String> {
    let (base, sec) = crate::fedtls::transport(&p.url, p.fed.as_deref());
    let token = federation::federation_token();
    write_state(&format!("updating {} ({}/{})", p.name, i + 1, n));
    federation::post_fed(
        &base,
        &sec,
        "/api/system/update",
        &token,
        None,
        UPDATE_MAX_TIME,
        &p.name,
        "update failed",
    )?;
    write_state(&format!("rebooting {} ({}/{})", p.name, i + 1, n));
    federation::post_fed(
        &base,
        &sec,
        "/api/system/reboot",
        &token,
        None,
        "30",
        &p.name,
        "reboot failed",
    )?;
    // Poll until the peer answers its node API again. The first probe waits a full interval so we
    // don't mistake the pre-reboot instance (still up for a moment) for "back".
    let deadline = Instant::now() + Duration::from_secs(REBOOT_WAIT_MAX_SECS);
    loop {
        write_state(&format!(
            "waiting for {} to come back ({}/{})",
            p.name,
            i + 1,
            n
        ));
        std::thread::sleep(Duration::from_secs(REBOOT_POLL_SECS));
        if federation::fetch_peer(p).reachable {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "did not come back within {} minutes of the reboot",
                REBOOT_WAIT_MAX_SECS / 60
            ));
        }
    }
}

/// The background orchestration: reachable peers in order, one at a time; a broken node is recorded
/// and **skipped past** — it must not block the rest of the fleet. Runs on a plain thread (it's all
/// blocking curl + sleeps) and leaves a `done:` summary for the panel.
fn run_fleet_update() {
    let peers = federation::peers();
    // Reachability snapshot up front: only nodes answering /api/node now get updated; the rest are
    // counted as skipped (they may be powered off — a reboot command to them would go nowhere).
    write_state("checking peer reachability…");
    let (reachable, unreachable): (Vec<Peer>, Vec<Peer>) = peers
        .into_iter()
        .partition(|p| federation::fetch_peer(p).reachable);
    let skipped = unreachable.len();
    let n = reachable.len();
    let (mut updated, mut failed) = (0usize, 0usize);
    for (i, p) in reachable.iter().enumerate() {
        match update_peer(p, i, n) {
            Ok(()) => {
                updated += 1;
                crate::notify::notify("Fleet update", &format!("{} updated and back", p.name));
            }
            Err(e) => {
                failed += 1;
                crate::notify::notify("Fleet update", &format!("{} failed: {e}", p.name));
            }
        }
    }
    let line = summary(updated, failed, skipped);
    write_state(&line);
    crate::notify::notify("Fleet update", &line);
}

// ── handlers + panel ─────────────────────────────────────────────────────────────────────────

/// POST /fleet/update-all — start the staggered fleet update. Refused while a run is live (per the
/// state file, so the guard also holds across a tendril-web restart).
pub async fn update_all(headers: axum::http::HeaderMap) -> Markup {
    let is_admin = crate::auth::is_admin(&headers);
    if ui::is_demo() {
        return status_body(
            is_admin,
            Some(
                html! { div.banner.warn style="margin:0 0 10px" { "Disabled in the live demo." } },
            ),
        );
    }
    if running() {
        return status_body(
            is_admin,
            Some(
                html! { div.banner.warn style="margin:0 0 10px" { "A fleet update is already running." } },
            ),
        );
    }
    // Mark the run live before answering so a double-click (or a second admin) is refused.
    write_state("starting…");
    std::thread::spawn(run_fleet_update);
    status_body(
        is_admin,
        Some(
            html! { div.banner.ok style="margin:0 0 10px" { "Fleet update started — progress below." } },
        ),
    )
}

/// GET /fleet/update-status — the self-polling progress fragment.
pub async fn update_status(headers: axum::http::HeaderMap) -> Markup {
    status_body(crate::auth::is_admin(&headers), None)
}

/// The polled fragment: the state line while it's fresh (live runs and recent summaries), plus the
/// "Update fleet" button when idle. Poller + button both swap this wrapper, so they compose.
fn status_body(is_admin: bool, banner: Option<Markup>) -> Markup {
    let state = read_state().filter(|(_, age)| age.as_secs() < STALE_AFTER_SECS);
    let live = state
        .as_ref()
        .map(|(s, age)| state_live(s, age.as_secs()))
        .unwrap_or(false);
    html! {
        div #fleet-update-status hx-get="/fleet/update-status" hx-trigger="every 5s"
            hx-target="this" hx-swap="outerHTML" {
            @if let Some(b) = banner { (b) }
            @if let Some((s, _)) = &state {
                @if live {
                    span.pill.running { span.led {} "running" } " " span.mono { (s) }
                } @else {
                    span.sub { "Last run — " span.mono { (s) } }
                }
            } @else {
                span.sub { "No fleet update running." }
            }
            @if is_admin && !live {
                div.btnrow style="margin-top:10px" {
                    button.btn.primary hx-post="/fleet/update-all"
                        hx-target="#fleet-update-status" hx-swap="outerHTML"
                        hx-confirm="Update the fleet? Every REACHABLE peer is updated and rebooted one at a time, waiting for each to come back healthy before the next. This node is NOT updated — do it last from its System page, so you don't reboot the console you're driving." {
                        "Update fleet"
                    }
                }
            }
        }
    }
}

/// The "Fleet OS updates" panel for the Fleet page (rendered after Fleet setup). `nodes` is the
/// already-fetched fleet (this node included — it's listed but never updated from here).
pub fn panel(nodes: &[NodeInfo], is_admin: bool) -> Markup {
    let me = federation::node_name();
    let peers: Vec<&NodeInfo> = nodes.iter().filter(|n| n.name != me).collect();
    ui::panel(
        "Fleet OS updates",
        Some("stage + reboot every reachable peer, one node at a time"),
        html! {
            div.pad {
                p.sub style="margin:0 0 10px" {
                    "Updates the OS image on each " b { "reachable" } " peer and reboots it, one node "
                    "at a time — the next node only starts once the previous one is back. Unreachable "
                    "peers are skipped, and one failing node doesn't block the rest. This node ("
                    b { (me) } ") is never touched from here: update it last from its "
                    a href="/system" { "System" } " page so you don't reboot the console you're driving."
                }
                @if peers.is_empty() {
                    p.sub style="margin:0 0 10px" { "No peers yet — this panel updates the other machines in the fleet." }
                } @else {
                    div.scroll style="margin:0 0 12px" { table {
                        thead { tr { th { "Peer" } th { "Status" } } }
                        tbody { @for p in &peers {
                            tr {
                                td.mono { (p.name) }
                                td {
                                    @if p.reachable {
                                        span.pill.running { span.led {} "reachable" }
                                    } @else {
                                        span.pill.off { span.led {} "unreachable — will be skipped" }
                                    }
                                }
                            }
                        } }
                    } }
                }
                (status_body(is_admin, None))
            }
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_is_terminal() {
        let s = summary(2, 1, 3);
        assert_eq!(s, "done: 2 updated, 1 failed, 3 skipped");
        assert!(is_done(&s));
        assert!(!is_done("updating aurora (1/3)"));
        assert!(!is_done("starting…"));
        assert!(!is_done("waiting for nebula to come back (2/3)"));
    }

    #[test]
    fn liveness_rule() {
        // Fresh + non-terminal → live.
        assert!(state_live("updating aurora (1/3)", 0));
        assert!(state_live("updating aurora (1/3)", STALE_AFTER_SECS - 1));
        // Stale (e.g. tendril-web restarted mid-run) → dead, never wedges the button.
        assert!(!state_live("updating aurora (1/3)", STALE_AFTER_SECS));
        assert!(!state_live("updating aurora (1/3)", STALE_AFTER_SECS + 1));
        // A finished summary is not a live run however fresh it is.
        assert!(!state_live(&summary(1, 0, 0), 0));
    }
}

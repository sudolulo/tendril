//! Saved station images — capture an installed station's disk as a reusable golden image (stored in
//! media), then clone new stations from it. Cloning uses a qcow2 overlay (see
//! `orchestrator::guest::create_overlay`): instant and deduplicated. Saving flattens + compresses the
//! disk into a standalone, portable image — the basis for shipping a built station to other machines
//! (federation).

use std::path::Path as FsPath;

use axum::extract::{Form, Path, Query};
use maud::{html, Markup};
use serde::Deserialize;

use tendril_orchestrator::guest::create_overlay;
use tendril_orchestrator::{DomainState, GuestOs, Libvirt};

use crate::ui;

/// Where golden images live — resolves to a mounted remote store's `images/` when configured, else
/// local (see `storage::image_dir`).
pub fn images_dir() -> String {
    crate::storage::image_dir()
}

/// Saved images as (name, human-readable size). Names are the `.qcow2` basename.
pub fn list() -> Vec<(String, String)> {
    if ui::is_demo() {
        return crate::demo::images();
    }
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(images_dir()) {
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().into_owned();
            if let Some(base) = n.strip_suffix(".qcow2") {
                let sz = e
                    .metadata()
                    .map(|m| ui::human_size(m.len()))
                    .unwrap_or_default();
                out.push((base.to_string(), sz));
            }
        }
    }
    out.sort();
    out
}

/// Full path of a saved image, guarding against traversal; `None` if it doesn't exist.
pub fn path_of(name: &str) -> Option<String> {
    let clean = sanitize(name);
    if clean.is_empty() {
        return None;
    }
    let p = format!("{}/{clean}.qcow2", images_dir());
    FsPath::new(&p).exists().then_some(p)
}

/// Sidecar file recording the guest OS a golden image was captured from, so cloning re-uses the
/// right OS (Windows vs SteamOS) instead of trusting a possibly-mismatched wizard selection.
fn os_sidecar(name: &str) -> String {
    format!("{}/{}.os", images_dir(), sanitize(name))
}

/// The guest OS a golden image holds, if recorded. `None` for older images with no sidecar.
pub fn image_os(name: &str) -> Option<GuestOs> {
    let raw = std::fs::read_to_string(os_sidecar(name)).ok()?;
    match raw.trim() {
        "windows" => Some(GuestOs::Windows),
        "steamos" => Some(GuestOs::SteamOs),
        _ => None,
    }
}

fn branch_sidecar(name: &str) -> String {
    format!("{}/{}.vgpu-branch", images_dir(), sanitize(name))
}

/// The NVIDIA vGPU host-driver branch a golden image's baked guest driver was built for, if recorded.
/// Used to reconcile the guest driver when the image is deployed onto a node running a different host
/// branch (see the reconcile-on-arrival flow). `None` for non-vGPU images or ones captured pre-metadata.
pub fn image_vgpu_branch(name: &str) -> Option<String> {
    let v = std::fs::read_to_string(branch_sidecar(name)).ok()?;
    let v = v.trim();
    (!v.is_empty()).then(|| v.to_string())
}

/// If deploying this golden image onto the current node would leave a mismatched vGPU guest driver,
/// returns `(image_branch, host_branch)`. That happens when the image's baked driver was built for a
/// different host branch than this node runs — i.e. it needs reconciling. `None` when they match, when
/// the image carries no branch (non-vGPU), or when the host branch is unknown.
pub fn image_vgpu_mismatch(name: &str) -> Option<(String, String)> {
    let img = image_vgpu_branch(name)?;
    let host = crate::vgpudrv::host_vgpu_branch()?;
    (img != host).then_some((img, host))
}

/// Short label for a guest OS, for the sidecar and the UI.
pub(crate) fn os_label(os: GuestOs) -> &'static str {
    match os {
        GuestOs::Windows => "windows",
        GuestOs::SteamOs => "steamos",
    }
}

/// Human OS label for the images list; "—" when an image has no recorded OS.
pub(crate) fn os_display(name: &str) -> &'static str {
    match image_os(name) {
        Some(GuestOs::Windows) => "Windows 11",
        Some(GuestOs::SteamOs) => "SteamOS",
        None => "—",
    }
}

/// A small badge showing a vGPU image's baked driver branch, and — when it differs from this host's
/// branch — that the guest driver will reconcile on deploy. Empty for non-vGPU images.
pub(crate) fn vgpu_badge(name: &str) -> Markup {
    let Some(img) = image_vgpu_branch(name) else {
        return html! {};
    };
    match image_vgpu_mismatch(name) {
        Some((i, h)) => html! {
            span.badge.warn style="margin-left:6px"
                title=(format!("Baked for vGPU driver {i}; this host runs {h} — the guest driver reconciles to {h} on deploy.")) {
                "vGPU " (i) " → " (h)
            }
        },
        None => html! {
            span.sub style="margin-left:6px" title=(format!("Baked for vGPU driver {img}")) { "vGPU " (img) }
        },
    }
}

/// A station's guest OS, inferred from its domain XML clock (Windows uses localtime, SteamOS UTC).
fn station_guest(name: &str) -> Option<GuestOs> {
    let xml = ui::virsh(&["dumpxml", name])?;
    if xml.contains("offset='localtime'") {
        Some(GuestOs::Windows)
    } else if xml.contains("offset='utc'") {
        Some(GuestOs::SteamOs)
    } else {
        None
    }
}

/// Keep image names to a safe charset (they become file names and query values).
fn sanitize(name: &str) -> String {
    name.trim()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        .collect()
}

/// A station's primary disk path, via virsh.
/// The station's **boot** disk source path (target `vda`). Explicitly `vda`, not just the first disk,
/// so a reimage/base-push replaces only the OS disk and NEVER the persistent data volume (`vdb`).
fn station_disk(name: &str) -> Option<String> {
    crate::stations::domblk_source(name, Some("vda"))
}

// ── integrity (SHA-256) ─────────────────────────────────────────────────────────────────────────

fn sha_path(name: &str) -> String {
    format!("{}/{}.qcow2.sha256", images_dir(), sanitize(name))
}
fn verifying_path(name: &str) -> String {
    format!("{}/{}.qcow2.verifying", images_dir(), sanitize(name))
}
fn mismatch_path(name: &str) -> String {
    format!("{}/{}.qcow2.mismatch", images_dir(), sanitize(name))
}

/// A file's SHA-256 (first token of `sha256sum` output).
fn sha256_of(path: &str) -> Option<String> {
    let out = ui::run_result("sha256sum", &[path]).ok()?;
    out.split_whitespace().next().map(str::to_string)
}

/// [`sha256_of`], re-touching `marker` while the hash runs. The verifying marker is mtime-aged
/// ([`fresh_or_reap`]), so a hash legitimately outlasting the staleness window (a huge image on a
/// slow shared store) must keep its marker fresh or a panel poll — ours or a peer's — reaps it
/// mid-run and offers a redundant concurrent verify.
fn sha256_watched(path: &str, marker: &str) -> Option<String> {
    let mut child = std::process::Command::new("sha256sum")
        .arg(path)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;
    let mut ticks = 0u32;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {
                std::thread::sleep(std::time::Duration::from_secs(1));
                ticks += 1;
                if ticks % 30 == 0 {
                    let _ = std::fs::write(marker, "");
                }
            }
            Err(_) => break,
        }
    }
    let out = child
        .wait_with_output()
        .ok()
        .filter(|o| o.status.success())?;
    String::from_utf8_lossy(&out.stdout)
        .split_whitespace()
        .next()
        .map(str::to_string)
}

/// The recorded SHA-256 of an image, if any.
fn image_sha(name: &str) -> Option<String> {
    std::fs::read_to_string(sha_path(name))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

enum Integrity {
    None,
    Recorded(String),
    Verifying,
    Mismatch,
}

/// How long a capture temp / verifying marker may sit unmodified before it's considered orphaned
/// (its worker thread died with a service restart). A live capture writes its temp continuously;
/// a verify of even a large image hashes in well under this.
const STALE_MARKER_SECS: u64 = 30 * 60;

/// True when `p` exists and was modified within [`STALE_MARKER_SECS`]. A stale file is removed —
/// self-healing, and safe on a shared store where a *peer's* live marker stays fresh.
fn fresh_or_reap(p: &str) -> bool {
    let Ok(md) = std::fs::metadata(p) else {
        return false;
    };
    let age = md
        .modified()
        .ok()
        .and_then(|t| t.elapsed().ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);
    if age > STALE_MARKER_SECS {
        let _ = std::fs::remove_file(p);
        return false;
    }
    true
}

fn integrity(name: &str) -> Integrity {
    if fresh_or_reap(&verifying_path(name)) {
        Integrity::Verifying
    } else if FsPath::new(&mismatch_path(name)).exists() {
        Integrity::Mismatch
    } else if let Some(h) = image_sha(name) {
        Integrity::Recorded(h)
    } else {
        Integrity::None
    }
}

fn short(h: &str) -> String {
    h.chars().take(12).collect()
}

fn cell_id(name: &str) -> String {
    format!("iv-{}", sanitize(name).replace('.', "-"))
}

/// Kick off a background integrity check: recompute the image's SHA-256 and compare it to the recorded
/// value. A mismatch flags the image so it isn't silently cloned. If no hash was recorded yet (e.g. an
/// image dropped in by hand), this records the current hash.
pub async fn verify(Query(q): Query<NameQuery>) -> Markup {
    if path_of(&q.name).is_some() {
        let name = sanitize(&q.name);
        let _ = std::fs::write(verifying_path(&name), "");
        let nm = name.clone();
        std::thread::spawn(move || {
            let dest = format!("{}/{}.qcow2", images_dir(), nm);
            match (image_sha(&nm), sha256_watched(&dest, &verifying_path(&nm))) {
                (Some(e), Some(a)) if e == a => {
                    let _ = std::fs::remove_file(mismatch_path(&nm));
                }
                (Some(_), Some(_)) => {
                    let _ = std::fs::write(mismatch_path(&nm), "");
                }
                (None, Some(a)) => {
                    // Backfill a missing hash.
                    let _ = std::fs::write(sha_path(&nm), a);
                    let _ = std::fs::remove_file(mismatch_path(&nm));
                }
                _ => {}
            }
            let _ = std::fs::remove_file(verifying_path(&nm));
        });
    }
    integrity_cell(&q.name)
}

pub async fn verify_status(Query(q): Query<NameQuery>) -> Markup {
    integrity_cell(&q.name)
}

/// The integrity cell for one image: current state + a verify button; polls while verifying.
fn integrity_cell(name: &str) -> Markup {
    let st = integrity(name);
    let id = cell_id(name);
    let poll = matches!(st, Integrity::Verifying);
    html! {
        span id=(id)
            hx-get=[poll.then(|| format!("/images/verifystatus?name={}", crate::ui::urlencode(name)))]
            hx-trigger=[poll.then_some("every 2s")] hx-swap="outerHTML" {
            @match &st {
                Integrity::Verifying => span.sub { "verifying…" },
                Integrity::Mismatch => span.badge title="No longer matches its recorded hash — corrupt or tampered. Don't clone it." style="color:var(--crit)" { "⚠ mismatch" },
                Integrity::Recorded(h) => span.sub.mono title=(h) { "✓ " (short(h)) },
                Integrity::None => span.sub { "—" },
            }
            @if !poll {
                " "
                button.btn.sm hx-post=(format!("/images/verify?name={}", crate::ui::urlencode(name)))
                    hx-target=(format!("#{id}")) hx-swap="outerHTML" { "verify" }
            }
        }
    }
}

// ── handlers ──────────────────────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct SaveForm {
    image_name: String,
}

/// Hidden temp path a capture writes to before it's atomically renamed to `<name>.qcow2`. The leading
/// dot and non-`.qcow2` suffix keep it out of [`list`] / [`path_of`], so a half-written image is never
/// listed or cloneable.
fn partial_path(dir: &str, name: &str) -> String {
    format!("{dir}/.{name}.qcow2.partial")
}

/// Names of captures currently in progress: a `.qcow2.partial` temp exists AND is still being
/// written (fresh mtime — `qemu-img convert` writes continuously). A temp orphaned by a service
/// restart is reaped instead of showing a phantom "capturing…" row forever.
fn in_progress() -> Vec<String> {
    let dir = images_dir();
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().into_owned();
            if let Some(mid) = n
                .strip_prefix('.')
                .and_then(|s| s.strip_suffix(".qcow2.partial"))
            {
                if fresh_or_reap(&format!("{dir}/{n}")) {
                    out.push(mid.to_string());
                }
            }
        }
    }
    out.sort();
    out
}

/// Capture a (shut-off) station's disk as a compressed standalone golden image. Runs in the
/// background (a multi-GB compress takes minutes): it writes to a hidden temp and only renames it to
/// the final name once complete, so the image can't be listed or cloned mid-capture.
pub async fn save(Path(station): Path<String>, Form(f): Form<SaveForm>) -> Markup {
    let name = sanitize(&f.image_name);
    if name.is_empty() {
        return note(false, "Image name required (letters, numbers, - _ .).");
    }
    let lv = Libvirt::system();
    if matches!(lv.state(&station), DomainState::Running) {
        return note(
            false,
            "Shut the station down first so its disk is captured consistently.",
        );
    }
    let Some(src) = station_disk(&station) else {
        return note(false, "Couldn't find the station's disk.");
    };
    let dir = images_dir();
    let dest = format!("{dir}/{name}.qcow2");
    let tmp = partial_path(&dir, &name);
    if FsPath::new(&dest).exists() {
        return note(false, "An image with that name already exists.");
    }
    // Only a *live* temp (still being written) blocks a re-capture; an orphaned one is reaped so
    // the name isn't refused forever after a restart killed its capture thread.
    if fresh_or_reap(&tmp) {
        return note(false, "A capture with that name is already in progress.");
    }
    let _ = std::fs::create_dir_all(&dir);
    // Record the guest OS now (needs the still-defined domain), applied after a successful capture.
    let guest = station_guest(&station);
    // If this is a vGPU station, record the host driver branch its baked guest driver matches, so the
    // driver can be reconciled when the image is later deployed onto a different-branch node.
    let vgpu_branch = crate::stations::station_mdev_uuid(&station)
        .and_then(|_| crate::vgpudrv::host_vgpu_branch());
    let nm = name.clone();
    std::thread::spawn(move || {
        // Flatten + compress into a portable standalone image (no backing chain), to a temp path.
        match ui::run_result("qemu-img", &["convert", "-c", "-O", "qcow2", &src, &tmp]) {
            Ok(_) => {
                // Atomic publish: the image only becomes visible/cloneable at this rename.
                if std::fs::rename(&tmp, &dest).is_ok() {
                    if let Some(os) = guest {
                        let _ = std::fs::write(os_sidecar(&nm), os_label(os));
                    }
                    if let Some(b) = &vgpu_branch {
                        let _ = std::fs::write(branch_sidecar(&nm), b);
                    }
                    // Record the SHA-256 so any node can verify the image's integrity before cloning
                    // or re-homing from it (the sidecar travels with the image on the shared store).
                    if let Some(h) = sha256_of(&dest) {
                        let _ = std::fs::write(sha_path(&nm), h);
                    }
                } else {
                    let _ = std::fs::remove_file(&tmp);
                    eprintln!("image {nm}: could not finalize capture");
                }
            }
            Err(e) => {
                let _ = std::fs::remove_file(&tmp);
                eprintln!("image {nm}: capture failed: {e}");
            }
        }
    });
    note(
        true,
        &format!("Capturing \u{201c}{name}\u{201d}\u{2026} it'll appear under Station images once complete."),
    )
}

#[derive(Deserialize)]
pub struct NameQuery {
    name: String,
}

pub async fn delete(Query(q): Query<NameQuery>) -> Markup {
    if let Some(p) = path_of(&q.name) {
        let _ = std::fs::remove_file(p);
        let _ = std::fs::remove_file(os_sidecar(&q.name));
        let _ = std::fs::remove_file(branch_sidecar(&q.name));
        let _ = std::fs::remove_file(sha_path(&q.name));
        let _ = std::fs::remove_file(mismatch_path(&q.name));
        let _ = std::fs::remove_file(verifying_path(&q.name));
    }
    panel()
}

fn note(ok: bool, msg: &str) -> Markup {
    html! { div class=(if ok { "banner ok" } else { "banner error" }) style="margin:0" { (msg) } }
}

// ── push a golden image to stations (bulk reimage) ──────────────────────────────────────────────

/// Reset a station's disk to a fresh **copy-on-write overlay** of `image_path` — a "reimage". The
/// overlay backs onto the existing golden image (instant, ~KB; **no full-image copy or transfer**),
/// so on a shared store every node overlays the same base in place. The station is forced off (its
/// disk is being replaced), the overlay recreated at the same path, and restarted if it was running.
///
/// Only the OS boot disk (`vda`) is replaced — a station's persistent **data volume** (`vdb`), if it
/// has one, is left untouched, so games/saves survive a base-image push.
pub(crate) fn reimage_station(name: &str, image_path: &str) -> Result<(), String> {
    // Reachable from the token-authed /api/reimage peer path — re-check the name (as the other
    // federation entry points do) so a `-`-leading value can't reach virsh as an argv option.
    if !crate::stations::valid_station_name(name) {
        return Err("invalid station name".into());
    }
    let lv = Libvirt::system();
    let was_running = matches!(lv.state(name), DomainState::Running);
    let disk = station_disk(name).ok_or("couldn't find the station's disk")?;
    if was_running {
        let _ = lv.destroy(name); // force off — the disk is about to be replaced
    }
    let _ = std::fs::remove_file(&disk);
    create_overlay(FsPath::new(&disk), FsPath::new(image_path)).map_err(|e| e.to_string())?;
    if was_running {
        lv.start(name).map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[derive(Deserialize)]
pub struct PushQuery {
    name: String,
}

/// The fleet-wide "push image to stations" picker — pick stations across all nodes to reset to this
/// golden image.
pub async fn push_form(Query(q): Query<PushQuery>) -> Markup {
    let image = sanitize(&q.name);
    let nodes = crate::federation::fleet_async().await;
    let total: usize = nodes.iter().map(|n| n.stations.len()).sum();
    html! {
        div #images {
            div.pad {
                @if path_of(&image).is_none() {
                    div.banner.error { "That image no longer exists." }
                    button.btn.sm hx-get="/images/panel" hx-target="#images" hx-swap="outerHTML" { "Back" }
                } @else if total == 0 {
                    p.muted { "No stations in the fleet to push to." }
                    button.btn.sm hx-get="/images/panel" hx-target="#images" hx-swap="outerHTML" { "Back" }
                } @else {
                    p { "Push " strong { (image) } " to stations — each selected station is reset to a fresh copy-on-write overlay of it (instant, no image transfer)." }
                    p.sub style="color:var(--crit)" { "This wipes each selected station's current disk (forced off, re-cloned, restarted). Push images matching the station's OS; remote nodes must have the image (e.g. on the shared store)." }
                    form hx-post=(format!("/images/push?name={}", crate::ui::urlencode(&image))) hx-target="#images" hx-swap="outerHTML"
                        hx-confirm="Reset the selected stations to this golden image? Their current disks are wiped." {
                        div.check { input type="checkbox" id="push-all" onclick="var c=this.checked;document.querySelectorAll('.push-st:not(:disabled)').forEach(function(e){e.checked=c});"; label for="push-all" { strong { "Select all" } } }
                        @for n in &nodes {
                            @if !n.stations.is_empty() {
                                p.sub style="margin:8px 0 2px" { strong { (n.name) } @if !n.reachable { " (unreachable — skipped)" } }
                                @for s in &n.stations {
                                    @let id = format!("push-{}-{}", n.name, s.name);
                                    div.check {
                                        input.push-st type="checkbox" name="station" value=(format!("{}|{}", n.name, s.name)) id=(id) disabled[!n.reachable];
                                        label for=(id) { (s.name) " " span.sub { "(" (s.state) ")" } }
                                    }
                                }
                            }
                        }
                        div.btnrow style="margin-top:10px" {
                            button.btn.primary.danger type="submit" { "Push to selected" }
                            button.btn type="button" hx-get="/images/panel" hx-target="#images" hx-swap="outerHTML" { "Cancel" }
                        }
                    }
                }
            }
        }
    }
}

/// Reimage each selected station (across the fleet) to the golden image, dispatching to the station's
/// node (locally, or over the peer's API).
pub async fn push(Query(q): Query<PushQuery>, Form(form): Form<Vec<(String, String)>>) -> Markup {
    let image = sanitize(&q.name);
    if path_of(&image).is_none() {
        return panel_with(Some(
            html! { div.banner.error { "That image no longer exists." } },
        ));
    }
    let targets: Vec<(String, String)> = form
        .iter()
        .filter(|(k, _)| k == "station")
        .filter_map(|(_, v)| {
            v.split_once('|')
                .map(|(n, s)| (n.to_string(), s.to_string()))
        })
        .collect();
    if targets.is_empty() {
        return panel_with(Some(html! { div.banner.warn { "No stations selected." } }));
    }
    let img = image.clone();
    let results = tokio::task::spawn_blocking(move || {
        targets
            .into_iter()
            .map(|(node, st)| {
                let r = crate::federation::reimage_dispatch(&node, &st, &img);
                (node, st, r)
            })
            .collect::<Vec<_>>()
    })
    .await
    .unwrap_or_default();
    let ok = results.iter().filter(|(_, _, r)| r.is_ok()).count();
    let errs: Vec<String> = results
        .iter()
        .filter_map(|(n, s, r)| r.as_ref().err().map(|e| format!("{n}/{s}: {e}")))
        .collect();
    let note = html! {
        @if errs.is_empty() {
            div.banner.ok { "Pushed " (image) " to " (ok) " station(s)." }
        } @else {
            div.banner.error { "Pushed to " (ok) " station(s); failed — " (errs.join("; ")) }
        }
    };
    panel_with(Some(note))
}

/// Pull a golden image from `from_url`'s API into this node's store, **unless already present** (e.g. a
/// shared store — then it's a no-op, nothing transferred). Atomic (temp + rename); records the
/// SHA-256 sidecar for the pulled copy.
pub(crate) fn pull_from(name: &str, from_url: &str, token: &str) -> Result<(), String> {
    let clean = sanitize(name);
    if clean.is_empty() {
        return Err("invalid image name".into());
    }
    // The source is a token-authed input (POST /api/image-pull); require a real http(s) URL so a
    // `-`-leading value can't become a curl option (a `--` terminator backs this up below).
    if !crate::ui::is_http_url(from_url) {
        return Err("image source must be an http(s) URL".into());
    }
    let dir = images_dir();
    let dest = format!("{dir}/{clean}.qcow2");
    if FsPath::new(&dest).exists() {
        return Ok(()); // already present (shared store, or previously distributed)
    }
    let _ = std::fs::create_dir_all(&dir);
    let tmp = format!("{dir}/.{clean}.qcow2.pulling");
    let _ = std::fs::remove_file(&tmp);
    let auth = format!("X-Tendril-Federation: {token}");
    let src = format!(
        "{}/api/image/{}",
        from_url.trim_end_matches('/'),
        crate::ui::urlencode(&clean)
    );
    // mTLS (our client cert + CA) when this node has a federation identity; else `-sk`. The caller
    // passes the source's mTLS endpoint to match (see `distribute`).
    let sec = crate::fedtls::client_args();
    let mut args: Vec<&str> = sec.iter().map(String::as_str).collect();
    args.extend([
        "--fail",
        "--proto",
        "=https,http",
        "--max-time",
        "3600",
        "-H",
        &auth,
        "-o",
        &tmp,
        "--",
        &src,
    ]);
    ui::run_result("curl", &args).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("pull failed: {e}")
    })?;
    std::fs::rename(&tmp, &dest).map_err(|e| e.to_string())?;
    if let Some(h) = sha256_of(&dest) {
        let _ = std::fs::write(sha_path(&clean), h);
    }
    Ok(())
}

/// Distribute a golden image to every reachable fleet node's image store (each pulls it once from this
/// node; nodes that already have it — e.g. via a shared store — skip). Then reimaging on any node uses
/// overlays, so only the base moves, once per node.
pub async fn distribute(Query(q): Query<PushQuery>) -> Markup {
    let image = sanitize(&q.name);
    if path_of(&image).is_none() {
        return panel_with(Some(
            html! { div.banner.error { "That image isn't on this node — distribute from a node that has it." } },
        ));
    }
    // Peers pull from our mTLS endpoint when we have an identity (so the transfer is mutually
    // authenticated), else from the plain-TLS UI URL.
    let source = if crate::fedtls::available() {
        crate::fedtls::fed_advertise_url()
    } else {
        crate::federation::advertise_url()
    };
    let nodes = crate::federation::fleet_async().await;
    let (img, src) = (image.clone(), source.clone());
    let results = tokio::task::spawn_blocking(move || {
        nodes
            .iter()
            .filter(|n| n.reachable)
            .map(|n| {
                (
                    n.name.clone(),
                    crate::federation::distribute_dispatch(&n.name, &img, &src),
                )
            })
            .collect::<Vec<_>>()
    })
    .await
    .unwrap_or_default();
    let ok = results.iter().filter(|(_, r)| r.is_ok()).count();
    let errs: Vec<String> = results
        .iter()
        .filter_map(|(n, r)| r.as_ref().err().map(|e| format!("{n}: {e}")))
        .collect();
    let note = html! {
        @if errs.is_empty() {
            div.banner.ok { "Distributed " (image) " to " (ok) " node(s)." }
        } @else {
            div.banner.error { "Distributed to " (ok) " node(s); failed — " (errs.join("; ")) }
        }
    };
    panel_with(Some(note))
}

// ── UI ──────────────────────────────────────────────────────────────────────────────────────

/// GET handler so the panel can self-refresh while a capture is running.
pub async fn panel_route() -> Markup {
    panel()
}

/// The saved-images panel for the Media page.
pub fn panel() -> Markup {
    panel_with(None)
}

fn panel_with(note: Option<Markup>) -> Markup {
    let imgs = list();
    let caps = if ui::is_demo() {
        Vec::new()
    } else {
        in_progress()
    };
    // Poll while a capture is in progress so it flips to a real image (or vanishes) when done.
    let poll = !caps.is_empty();
    html! {
        div #images hx-get=[poll.then_some("/images/panel")] hx-trigger=[poll.then_some("every 4s")] hx-swap="outerHTML" {
            div.pad {
                @if let Some(n) = note { (n) }
                @if imgs.is_empty() && caps.is_empty() {
                    p.muted { "No saved images yet. Open a station that's shut off and use " strong { "Save as image" } " to capture its installed disk as a reusable template." }
                } @else {
                    div.scroll { table {
                        thead { tr { th { "Image" } th { "OS" } th { "Integrity" } th.right { "Size" } th.right { "" } } }
                        tbody {
                            @for (n, sz) in &imgs {
                                tr {
                                    td.mono { (n) }
                                    td { (os_display(n)) (vgpu_badge(n)) }
                                    td { (integrity_cell(n)) }
                                    td.right.num { (sz) }
                                    td.right {
                                        button.btn.sm
                                            hx-get=(format!("/images/push?name={}", crate::ui::urlencode(n)))
                                            hx-target="#images" hx-swap="outerHTML"
                                            title="Reset stations across the fleet to a fresh copy of this image (each station's persistent data volume is kept)" { "Push…" }
                                        " "
                                        @if crate::federation::enabled() {
                                            button.btn.sm
                                                hx-post=(format!("/images/distribute?name={}", crate::ui::urlencode(n)))
                                                hx-target="#images" hx-swap="outerHTML"
                                                hx-confirm=(format!("Copy '{n}' to every fleet node's image store? (Nodes that already have it — e.g. via a shared store — are skipped. Large images transfer once per node.)"))
                                                title="Copy this image to every node's store so it can be used fleet-wide" { "Distribute…" }
                                            " "
                                        }
                                        button.btn.sm.danger
                                            hx-post=(format!("/images/delete?name={}", crate::ui::urlencode(n)))
                                            hx-target="#images" hx-swap="outerHTML"
                                            hx-confirm=(format!("Delete image '{n}'? Stations cloned from it (overlays) depend on it and will break.")) { "Delete" }
                                    }
                                }
                            }
                            @for n in &caps {
                                tr {
                                    td.mono { (n) " " span.sub { "(capturing…)" } }
                                    td { span.sub { "—" } }
                                    td { span.sub { "—" } }
                                    td.right.num { span.sub { "—" } }
                                    td.right { span.sub { "capturing…" } }
                                }
                            }
                        }
                    } }
                }
                p.sub style="margin-top:10px" { "Golden images are qcow2 templates. New stations clone them as copy-on-write overlays — instant and deduplicated (the base is shared, not copied) — which is the groundwork for shipping a built station to other machines. A capture only becomes usable once fully written." }
            }
        }
    }
}

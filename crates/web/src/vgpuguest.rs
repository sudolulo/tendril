//! NVIDIA vGPU **guest** driver staging.
//!
//! A station bound to an NVIDIA vGPU (mdev) slice needs the matching GRID **guest** driver installed
//! inside it — without it the vGPU is inert, and there's nothing for the [licensing][crate::licensing]
//! (FastAPI-DLS) token to un-throttle. The guest driver is part of NVIDIA's licensed vGPU package, so
//! it's staged under the Tendril state dir and copied onto the hands-off seed disc; provisioning then
//! installs it on first boot ([`tendril_orchestrator::UnattendSpec::vgpu_driver_exe`] for Windows,
//! [`tendril_orchestrator::KickstartSpec::vgpu_guest_run`] for SteamOS).
//!
//! Two flavours:
//! - **Windows** (`.exe`) — not published on the public bucket, so the admin **supplies** it (upload
//!   or a URL they can reach), like the host `.run` ([crate::vgpudrv]).
//! - **Linux** (`.run`) — published on NVIDIA's *official* public bucket, so it can be **auto-fetched**
//!   from there behind an entitlement attestation (retrieval-from-source, not redistribution), or
//!   supplied by URL.
//!
//! Free/redistributable extras (Steam, Sunshine, Discord) are *not* staged here — those are fetched
//! straight from their official URLs on first boot.

use axum::extract::{Form, Multipart};
use maud::{html, Markup};
use serde::Deserialize;

use crate::ui;

/// Filename the Windows guest driver is given on the seed disc; the answer file's first-logon command
/// runs it by this name.
pub const DISC_NAME: &str = "nvidia-vgpu-guest.exe";
/// Filename the Linux guest driver `.run` is given on the SteamOS kickstart seed disc.
pub const LINUX_DISC_NAME: &str = "nvidia-vgpu-guest.run";

/// Curated Linux vGPU **guest** drivers on NVIDIA's *official* public bucket (`nvidia-drivers-us-public`
/// — what GCP itself pulls from), one recent release per branch, each verified reachable. Fetching from
/// NVIDIA's own public URL is retrieval-from-source, not redistribution — but the vGPU EULA still gates
/// *use*, so auto-fetch requires the admin to attest they hold a vGPU entitlement. Pick the release that
/// matches the host driver branch you baked. (`label`, `url`)
const LINUX_DRIVERS: &[(&str, &str)] = &[
    ("vGPU 18.4 — 570.172.08", "https://storage.googleapis.com/nvidia-drivers-us-public/GRID/vGPU18.4/NVIDIA-Linux-x86_64-570.172.08-grid.run"),
    ("vGPU 17.4 — 550.127.05", "https://storage.googleapis.com/nvidia-drivers-us-public/GRID/vGPU17.4/NVIDIA-Linux-x86_64-550.127.05-grid.run"),
    ("vGPU 17.3 — 550.90.07", "https://storage.googleapis.com/nvidia-drivers-us-public/GRID/vGPU17.3/NVIDIA-Linux-x86_64-550.90.07-grid.run"),
    ("vGPU 16.3 — 535.154.05", "https://storage.googleapis.com/nvidia-drivers-us-public/GRID/vGPU16.3/NVIDIA-Linux-x86_64-535.154.05-grid.run"),
    ("vGPU 16.2 — 535.129.03", "https://storage.googleapis.com/nvidia-drivers-us-public/GRID/vGPU16.2/NVIDIA-Linux-x86_64-535.129.03-grid.run"),
    ("vGPU 15.4 — 525.147.05", "https://storage.googleapis.com/nvidia-drivers-us-public/GRID/vGPU15.4/NVIDIA-Linux-x86_64-525.147.05-grid.run"),
];

fn dir() -> String {
    std::env::var("TENDRIL_VGPU_GUEST_DIR")
        .unwrap_or_else(|_| "/var/lib/tendril/vgpu/guest".to_string())
}
fn exe_path() -> String {
    format!("{}/{}", dir(), DISC_NAME)
}
fn run_path() -> String {
    format!("{}/{}", dir(), LINUX_DISC_NAME)
}

/// Path to the staged Windows guest-driver installer, if present and non-empty. Consumed by station
/// provisioning to decide whether to bake the driver into an NVIDIA-vGPU Windows station.
pub fn staged_installer() -> Option<String> {
    non_empty(&exe_path())
}

/// Path to the staged Linux guest-driver `.run`, if present and non-empty. Consumed by SteamOS station
/// provisioning.
pub fn staged_linux_run() -> Option<String> {
    non_empty(&run_path())
}

fn non_empty(p: &str) -> Option<String> {
    std::fs::metadata(p)
        .ok()
        .filter(|m| m.len() > 0)
        .map(|_| p.to_string())
}

// ── automatic selection: the split implies the host branch, which implies the guest driver ─────────

/// The curated Linux bucket release (`label`, `url`) whose driver version matches `branch` exactly. For
/// a given vGPU release the Linux **guest** version equals the **host** version, so an exact match on
/// the version string is the correct pairing.
fn linux_release_for_branch(branch: &str) -> Option<(&'static str, &'static str)> {
    let b = branch.trim();
    if b.is_empty() {
        return None;
    }
    LINUX_DRIVERS.iter().find(|(_, u)| u.contains(b)).copied()
}
fn linux_url_for_branch(branch: &str) -> Option<&'static str> {
    linux_release_for_branch(branch).map(|(_, u)| u)
}

/// Auto-select the Linux guest `.run` for the host driver branch — the staged file if present, else the
/// matching release fetched from NVIDIA's official public bucket. This is what makes the guest driver
/// invisible for SteamOS stations: nothing to pick, nothing to stage. None only if no host branch is
/// known or no matching public release exists (then a whole-GPU or unlicensed path — driver skipped).
pub fn auto_linux_run() -> Option<String> {
    if let Some(p) = staged_linux_run() {
        return Some(p);
    }
    let branch = crate::vgpudrv::host_vgpu_branch()?;
    let url = linux_url_for_branch(&branch)?;
    let _ = std::fs::create_dir_all(dir());
    fetch_to(url, &run_path()).ok().map(|_| run_path())
}

/// Best-effort background prefetch of the Linux guest `.run` matching `branch`, so it's already staged
/// by the time a vGPU SteamOS station is created (keeps create fast). Called when the host driver is
/// staged. No-op if already staged or no matching public release.
pub fn prefetch_linux(branch: &str) {
    if staged_linux_run().is_some() {
        return;
    }
    let Some(url) = linux_url_for_branch(branch) else {
        return;
    };
    let url = url.to_string();
    std::thread::spawn(move || {
        let _ = std::fs::create_dir_all(dir());
        let _ = fetch_to(&url, &run_path());
    });
}

fn staged_size() -> Option<u64> {
    std::fs::metadata(exe_path())
        .ok()
        .map(|m| m.len())
        .filter(|n| *n > 0)
}

fn linux_size() -> Option<u64> {
    std::fs::metadata(run_path())
        .ok()
        .map(|m| m.len())
        .filter(|n| *n > 0)
}

fn human(n: u64) -> String {
    if n >= 1 << 30 {
        format!("{:.1} GB", n as f64 / (1u64 << 30) as f64)
    } else {
        format!("{:.0} MB", n as f64 / (1u64 << 20) as f64)
    }
}

/// Accept the guest driver as a multipart upload (`guestfile`) or a URL (`url`). Streamed to disk
/// (it's hundreds of MB) then atomically renamed into place. Mirrors [`crate::vgpudrv::stage`].
pub async fn stage(mut mp: Multipart) -> Markup {
    if ui::is_demo() {
        return section(Some(html! { div.banner.warn { "Disabled in the demo." } }));
    }
    let _ = std::fs::create_dir_all(dir());
    let tmp = format!("{}/.{}.part", dir(), DISC_NAME);
    let mut url = String::new();
    let mut wrote = 0u64;

    while let Ok(Some(mut field)) = mp.next_field().await {
        match field.name().unwrap_or("") {
            "guestfile" => {
                use std::io::Write as _;
                let Ok(mut f) = std::fs::File::create(&tmp) else {
                    return section(Some(
                        html! { div.banner.error { "Couldn't open a temp file to write the driver." } },
                    ));
                };
                loop {
                    match field.chunk().await {
                        Ok(Some(bytes)) => {
                            if f.write_all(&bytes).is_err() {
                                let _ = std::fs::remove_file(&tmp);
                                return section(Some(
                                    html! { div.banner.error { "Write failed while saving the driver." } },
                                ));
                            }
                            wrote += bytes.len() as u64;
                        }
                        Ok(None) => break,
                        Err(_) => {
                            let _ = std::fs::remove_file(&tmp);
                            return section(Some(
                                html! { div.banner.error { "Upload was interrupted." } },
                            ));
                        }
                    }
                }
            }
            "url" => {
                url = field.text().await.unwrap_or_default().trim().to_string();
            }
            _ => {}
        }
    }

    if wrote == 0 && !url.is_empty() {
        match ui::run_result("curl", &["-fL", "--max-time", "3600", "-o", &tmp, &url]) {
            Ok(_) => wrote = std::fs::metadata(&tmp).map(|m| m.len()).unwrap_or(0),
            Err(e) => {
                let _ = std::fs::remove_file(&tmp);
                return section(Some(html! { div.banner.error { "Download failed: " (e) } }));
            }
        }
    }

    if wrote == 0 {
        let _ = std::fs::remove_file(&tmp);
        return section(Some(
            html! { div.banner.error { "Provide the guest driver installer or a URL to download it from." } },
        ));
    }
    if wrote < 1 << 20 {
        let _ = std::fs::remove_file(&tmp);
        return section(Some(
            html! { div.banner.error { "That doesn't look like an NVIDIA vGPU guest driver (too small). Expected the multi-hundred-MB Windows GRID guest " code { ".exe" } "." } },
        ));
    }
    if let Err(e) = std::fs::rename(&tmp, exe_path()) {
        return section(Some(
            html! { div.banner.error { "Couldn't store the driver: " (e) } },
        ));
    }
    section(Some(
        html! { div.banner.ok { "Guest driver staged (" (human(wrote)) "). New NVIDIA-vGPU Windows stations will install it automatically." } },
    ))
}

pub async fn clear() -> Markup {
    if ui::is_demo() {
        return section(Some(html! { div.banner.warn { "Disabled in the demo." } }));
    }
    let _ = std::fs::remove_file(exe_path());
    section(Some(
        html! { div.banner.ok { "Staged Windows guest driver removed." } },
    ))
}

pub async fn clear_linux() -> Markup {
    if ui::is_demo() {
        return section(Some(html! { div.banner.warn { "Disabled in the demo." } }));
    }
    let _ = std::fs::remove_file(run_path());
    section(Some(
        html! { div.banner.ok { "Staged Linux guest driver removed." } },
    ))
}

#[derive(Deserialize)]
pub struct LinuxFetchForm {
    #[serde(default)]
    preset: String,
    #[serde(default)]
    custom_url: String,
    /// Present ("on") when the entitlement box is ticked.
    #[serde(default)]
    entitlement: Option<String>,
}

/// Fetch the Linux guest `.run`. A `preset` (an official-bucket URL from [`LINUX_DRIVERS`]) requires
/// the entitlement attestation, since it pulls NVIDIA's licensed binary from its public bucket; a
/// `custom_url` (your own portal/eval download) is the plain "supply it" path and needs no attestation.
pub async fn autofetch(Form(f): Form<LinuxFetchForm>) -> Markup {
    if ui::is_demo() {
        return section(Some(html! { div.banner.warn { "Disabled in the demo." } }));
    }
    let custom = f.custom_url.trim();
    let preset = f.preset.trim();
    let (url, is_preset) = if !custom.is_empty() {
        (custom, false)
    } else if !preset.is_empty() {
        (preset, true)
    } else {
        return section(Some(
            html! { div.banner.error { "Choose a release to auto-fetch, or paste a URL you can reach it at." } },
        ));
    };
    // Only allow presets from our vetted official-bucket list through the entitlement-gated path.
    if is_preset && !LINUX_DRIVERS.iter().any(|(_, u)| *u == url) {
        return section(Some(html! { div.banner.error { "Unknown release." } }));
    }
    if is_preset && f.entitlement.as_deref() != Some("on") {
        return section(Some(
            html! { div.banner.error { "Tick the box confirming you hold an NVIDIA vGPU entitlement to auto-fetch the licensed driver." } },
        ));
    }
    let _ = std::fs::create_dir_all(dir());
    match fetch_to(url, &run_path()) {
        Ok(n) => section(Some(html! { div.banner.ok {
        "Linux guest driver staged (" (human(n)) "). New NVIDIA-vGPU SteamOS stations will install it on first boot." } })),
        Err(e) => section(Some(html! { div.banner.error { "Download failed: " (e) } })),
    }
}

/// Stream `url` to `dest` via curl (multi-hundred-MB), atomically renaming into place. Returns the size.
fn fetch_to(url: &str, dest: &str) -> Result<u64, String> {
    let tmp = format!("{dest}.part");
    ui::run_result("curl", &["-fL", "--max-time", "3600", "-o", &tmp, url])
        .map_err(|e| e.to_string())?;
    let n = std::fs::metadata(&tmp).map(|m| m.len()).unwrap_or(0);
    if n < 1 << 20 {
        let _ = std::fs::remove_file(&tmp);
        return Err("that doesn't look like a driver (too small)".into());
    }
    std::fs::rename(&tmp, dest).map_err(|e| e.to_string())?;
    Ok(n)
}

/// The guest-driver staging UI, embedded in the vGPU panel under the host-driver section. Two parts:
/// the Windows guest driver (supplied) and the Linux guest driver (auto-fetchable from NVIDIA's bucket).
pub fn section(banner: Option<Markup>) -> Markup {
    let win = staged_size();
    let lin = linux_size();
    let host_branch = crate::vgpudrv::host_vgpu_branch();
    let lin_match = host_branch.as_deref().and_then(linux_release_for_branch);
    html! {
        div #vgpu-guest style="margin-top:8px" {
            @if let Some(b) = banner { (b) }
            p.sub style="margin:0 0 6px" {
                "vGPU " b { "guest" } " driver — installed " b { "inside" } " each NVIDIA-vGPU station so it can use its slice. "
                "Tendril picks it " b { "automatically" } " to match your host driver branch"
                @if let Some(hb) = &host_branch { " (" code { (hb) } ")" }
                " — you don't choose a driver. Linux stations fetch the matching release on demand; Windows needs its installer supplied once (below), since NVIDIA doesn't publish it."
            }

            // ── Windows guest driver (supplied) ──
            div.sub style="font-weight:600; margin:10px 0 4px" { "Windows stations" }
            @if let Some(n) = win {
                div.sub { "Staged " code { ".exe" } ": " b { (human(n)) } " ✓ "
                    button.btn.sm style="margin-left:8px"
                        hx-post="/hardware/vgpu/guest/clear" hx-target="#vgpu-guest" hx-swap="outerHTML"
                        hx-confirm="Remove the staged Windows guest driver?" { "Remove" }
                }
            } @else {
                p.sub style="margin:0 0 6px" {
                    "Supply it from your NVIDIA "
                    a href="https://www.nvidia.com/en-us/data-center/resources/vgpu-evaluation/" { "vGPU download" }
                    " (the " code { "Guest_Drivers" } " folder) — not published on the public bucket."
                }
                form hx-post="/hardware/vgpu/guest" hx-encoding="multipart/form-data"
                    hx-target="#vgpu-guest" hx-swap="outerHTML" {
                    div.field style="margin:0 0 8px" {
                        label { "Windows installer (" code { ".exe" } ")" }
                        input type="file" name="guestfile" accept=".exe" style="font-size:12.5px";
                    }
                    div.field style="margin:0 0 8px" {
                        label { "…or a URL to download it from" }
                        input type="url" name="url" placeholder="https://…/…grid_win10_win11_dch_64bit_international.exe";
                    }
                    button.btn.primary type="submit" { "Stage Windows driver" }
                }
            }

            // ── Linux guest driver (automatic; manual supply only as a fallback) ──
            div.sub style="font-weight:600; margin:14px 0 4px" { "SteamOS / Linux stations" }
            @if let Some(n) = lin {
                div.sub { "Ready " code { ".run" } ": " b { (human(n)) } " ✓ (auto-matched to your host driver) "
                    button.btn.sm style="margin-left:8px"
                        hx-post="/hardware/vgpu/guest/linux/clear" hx-target="#vgpu-guest" hx-swap="outerHTML"
                        hx-confirm="Remove the staged Linux guest driver? It will be re-fetched automatically." { "Remove" }
                }
            } @else if let Some((label, _)) = lin_match {
                p.sub {
                    "Automatic — " b { (label) } " matches your host driver and is fetched from NVIDIA's "
                    "official bucket when a vGPU SteamOS station is created. Nothing to do."
                }
            } @else {
                p.sub style="margin:0 0 6px" {
                    "No matching release is known for your host driver "
                    @if let Some(hb) = &host_branch { "(" code { (hb) } ") " } @else { "(host branch not detected yet) " }
                    "— supply the Linux " code { ".run" } " once (advanced):"
                }
                form hx-post="/hardware/vgpu/guest/linux" hx-target="#vgpu-guest" hx-swap="outerHTML" {
                    p.sub style="margin:0 0 6px" {
                        "Auto-fetch the Linux " code { ".run" } " from NVIDIA's " b { "official public bucket" }
                        " (the same source GCP uses). Pick the release matching your host driver branch."
                    }
                    div.field style="margin:0 0 8px" {
                        label { "Release (official bucket)" }
                        select name="preset" style="font-size:12.5px" {
                            option value="" { "— choose a release —" }
                            @for (label, url) in LINUX_DRIVERS {
                                option value=(url) { (label) }
                            }
                        }
                    }
                    div.field.check style="margin:0 0 8px" {
                        input type="checkbox" name="entitlement" id="vgpu-entitle" value="on";
                        label for="vgpu-entitle" { "I hold an NVIDIA vGPU entitlement (required to fetch the licensed driver)" }
                    }
                    div.field style="margin:0 0 8px" {
                        label { "…or a URL you can reach it at (your portal/eval download)" }
                        input type="url" name="custom_url" placeholder="https://…/NVIDIA-Linux-x86_64-…-grid.run";
                    }
                    button.btn.primary type="submit" { "Fetch Linux driver" }
                }
            }
        }
    }
}

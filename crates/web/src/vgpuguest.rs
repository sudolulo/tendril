//! NVIDIA vGPU **guest** driver — fully automatic, invisible to the user.
//!
//! A station bound to an NVIDIA vGPU (mdev) slice needs the matching GRID **guest** driver installed
//! inside it; without it the slice is inert and there's nothing for the [licensing][crate::licensing]
//! (FastAPI-DLS) token to un-throttle. The user never picks, uploads, or even sees this driver:
//!
//! - The **host** driver branch — captured when the licensed host `.run` is staged ([crate::vgpudrv]),
//!   which itself requires an NVIDIA vGPU entitlement — pins the exact vGPU release.
//! - For that release Tendril fetches BOTH guest installers (Windows `.exe`, Linux `.run`) from
//!   NVIDIA's own public distribution bucket (`nvidia-drivers-us-public` — the bucket GCP itself pulls
//!   from). That's retrieval-from-source, not redistribution, and the same entitlement that let the
//!   admin obtain the host driver covers guest use — so there's no separate upload or attestation.
//!
//! Fetched drivers are cached under the Tendril state dir and copied onto the hands-off seed disc;
//! provisioning installs them on first boot ([`tendril_orchestrator::UnattendSpec::vgpu_driver_exe`]
//! for Windows, [`tendril_orchestrator::KickstartSpec::vgpu_guest_run`] for SteamOS). An unlisted host
//! branch (rare) or an air-gapped box can point at a reachable copy via `TENDRIL_VGPU_GUEST_EXE_URL` /
//! `TENDRIL_VGPU_GUEST_RUN_URL` — still no UI.

use maud::{html, Markup};

use crate::ui;

/// Filename the Windows guest driver is given on the seed disc; the answer file's first-logon command
/// runs it by this name.
pub const DISC_NAME: &str = "nvidia-vgpu-guest.exe";
/// Filename the Linux guest driver `.run` is given on the SteamOS kickstart seed disc.
pub const LINUX_DISC_NAME: &str = "nvidia-vgpu-guest.run";

/// One curated vGPU release: the host (== Linux-guest) driver version, the human label, and the exact
/// public-bucket URLs for both guest installers. The Linux guest version equals the host version; the
/// Windows guest version differs and is paired by vGPU release. Every URL is verified reachable on
/// `storage.googleapis.com/nvidia-drivers-us-public/GRID/` (the bucket only serves known object paths —
/// it doesn't allow listing — so these are pinned, not discovered).
struct Release {
    /// Host / Linux-guest driver version, e.g. `550.127.05` — matched against the staged host branch.
    branch: &'static str,
    /// Human label for the release, e.g. `vGPU 17.4`.
    label: &'static str,
    /// Linux guest `.run` URL (guest version == host `branch`).
    run_url: &'static str,
    /// Windows guest `.exe` URL (Windows version differs; paired by vGPU release).
    exe_url: &'static str,
}

const RELEASES: &[Release] = &[
    Release {
        branch: "570.172.08",
        label: "vGPU 18.4",
        run_url: "https://storage.googleapis.com/nvidia-drivers-us-public/GRID/vGPU18.4/NVIDIA-Linux-x86_64-570.172.08-grid.run",
        exe_url: "https://storage.googleapis.com/nvidia-drivers-us-public/GRID/vGPU18.4/573.48_grid_win10_win11_server2022_dch_64bit_international.exe",
    },
    Release {
        branch: "550.127.05",
        label: "vGPU 17.4",
        run_url: "https://storage.googleapis.com/nvidia-drivers-us-public/GRID/vGPU17.4/NVIDIA-Linux-x86_64-550.127.05-grid.run",
        exe_url: "https://storage.googleapis.com/nvidia-drivers-us-public/GRID/vGPU17.4/553.24_grid_win10_win11_server2022_dch_64bit_international.exe",
    },
    Release {
        branch: "550.90.07",
        label: "vGPU 17.3",
        run_url: "https://storage.googleapis.com/nvidia-drivers-us-public/GRID/vGPU17.3/NVIDIA-Linux-x86_64-550.90.07-grid.run",
        exe_url: "https://storage.googleapis.com/nvidia-drivers-us-public/GRID/vGPU17.3/552.74_grid_win10_win11_server2022_dch_64bit_international.exe",
    },
    Release {
        branch: "535.154.05",
        label: "vGPU 16.3",
        run_url: "https://storage.googleapis.com/nvidia-drivers-us-public/GRID/vGPU16.3/NVIDIA-Linux-x86_64-535.154.05-grid.run",
        exe_url: "https://storage.googleapis.com/nvidia-drivers-us-public/GRID/vGPU16.3/538.15_grid_win10_win11_server2019_server2022_dch_64bit_international.exe",
    },
    Release {
        branch: "535.129.03",
        label: "vGPU 16.2",
        run_url: "https://storage.googleapis.com/nvidia-drivers-us-public/GRID/vGPU16.2/NVIDIA-Linux-x86_64-535.129.03-grid.run",
        exe_url: "https://storage.googleapis.com/nvidia-drivers-us-public/GRID/vGPU16.2/537.70_grid_win10_win11_server2019_server2022_dch_64bit_international.exe",
    },
    Release {
        branch: "525.147.05",
        label: "vGPU 15.4",
        run_url: "https://storage.googleapis.com/nvidia-drivers-us-public/GRID/vGPU15.4/NVIDIA-Linux-x86_64-525.147.05-grid.run",
        exe_url: "https://storage.googleapis.com/nvidia-drivers-us-public/GRID/vGPU15.4/529.19_grid_win10_win11_server2019_server2022_dch_64bit_international.exe",
    },
];

fn release_for_branch(branch: &str) -> Option<&'static Release> {
    let b = branch.trim();
    if b.is_empty() {
        return None;
    }
    RELEASES.iter().find(|r| r.branch == b)
}

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

fn non_empty(p: &str) -> Option<String> {
    std::fs::metadata(p)
        .ok()
        .filter(|m| m.len() > 0)
        .map(|_| p.to_string())
}

// ── automatic selection: the host branch implies the vGPU release, which implies the guest driver ──

/// Auto-provide the Windows guest `.exe` matching the host driver branch: a cached copy if present, an
/// explicit `TENDRIL_VGPU_GUEST_EXE_URL` override, else the paired release fetched from NVIDIA's public
/// bucket. `None` only when no host branch is known or no curated release matches (then the station is
/// built without a guest driver, as for whole-GPU passthrough). Never prompts the user.
pub fn auto_windows_exe() -> Option<String> {
    auto_fetch(&exe_path(), "TENDRIL_VGPU_GUEST_EXE_URL", |r| r.exe_url)
}

/// Auto-provide the Linux guest `.run` matching the host driver branch (guest version == host version).
/// Same policy as [`auto_windows_exe`]. This is what makes the guest driver invisible for SteamOS.
pub fn auto_linux_run() -> Option<String> {
    auto_fetch(&run_path(), "TENDRIL_VGPU_GUEST_RUN_URL", |r| r.run_url)
}

/// Shared resolution: cached file → env-override URL → curated public-bucket URL for the host branch.
fn auto_fetch(dest: &str, env_key: &str, pick: fn(&Release) -> &'static str) -> Option<String> {
    if let Some(p) = non_empty(dest) {
        return Some(p);
    }
    let _ = std::fs::create_dir_all(dir());
    if let Ok(url) = std::env::var(env_key) {
        let url = url.trim().to_string();
        if !url.is_empty() {
            return fetch_to(&url, dest).ok().map(|_| dest.to_string());
        }
    }
    let branch = crate::vgpudrv::host_vgpu_branch()?;
    let url = pick(release_for_branch(&branch)?);
    fetch_to(url, dest).ok().map(|_| dest.to_string())
}

/// Best-effort background prefetch of BOTH guest drivers matching `branch`, so they're already cached by
/// the time a vGPU station is created (keeps create fast). Called when the host driver is staged. No-ops
/// for already-cached files or an unlisted branch.
pub fn prefetch(branch: &str) {
    let Some(r) = release_for_branch(branch) else {
        return;
    };
    let jobs = [(run_path(), r.run_url), (exe_path(), r.exe_url)];
    std::thread::spawn(move || {
        let _ = std::fs::create_dir_all(dir());
        for (dest, url) in jobs {
            if non_empty(&dest).is_none() {
                let _ = fetch_to(url, &dest);
            }
        }
    });
}

/// Stream `url` to `dest` via curl (multi-hundred-MB), atomically renaming into place. Returns the size.
fn fetch_to(url: &str, dest: &str) -> Result<u64, String> {
    let tmp = format!("{dest}.part");
    if !crate::ui::is_http_url(url) {
        return Err("driver URL must be http(s)".into());
    }
    ui::run_result(
        "curl",
        &[
            "-fL",
            "--proto",
            "=https,http",
            "--max-time",
            "3600",
            "-o",
            &tmp,
            "--",
            url,
        ],
    )
    .map_err(|e| e.to_string())?;
    let n = std::fs::metadata(&tmp).map(|m| m.len()).unwrap_or(0);
    if n < 1 << 20 {
        let _ = std::fs::remove_file(&tmp);
        return Err("that doesn't look like a driver (too small)".into());
    }
    std::fs::rename(&tmp, dest).map_err(|e| e.to_string())?;
    Ok(n)
}

/// Read-only status shown under the host-driver panel. There are no forms: the guest driver is
/// automatic. Reports the matched release (and whether the installers are already cached) or, for the
/// rare unlisted branch, how to point at one — but never asks the user to pick, fetch, or upload.
pub fn section() -> Markup {
    let host_branch = crate::vgpudrv::host_vgpu_branch();
    let matched = host_branch.as_deref().and_then(release_for_branch);
    let exe_cached = std::fs::metadata(exe_path())
        .ok()
        .map(|m| m.len())
        .filter(|n| *n > 0);
    let run_cached = std::fs::metadata(run_path())
        .ok()
        .map(|m| m.len())
        .filter(|n| *n > 0);
    let cached = exe_cached.is_some() || run_cached.is_some();
    html! {
        div #vgpu-guest {
            @match (&host_branch, matched) {
                (Some(_), Some(r)) => {
                    p.sub style="margin:0" {
                        "Guest driver: automatic — " b { (r.label) } " installed per station"
                        @if cached { ", cached ✓" } @else { "" } "."
                    }
                }
                (Some(hb), None) => {
                    p.sub style="margin:0" {
                        "Guest driver: no curated release for host " code { (hb) }
                        " — set " code { "TENDRIL_VGPU_GUEST_EXE_URL" } " / " code { "_RUN_URL" } " to a reachable copy."
                    }
                }
                _ => {
                    p.sub style="margin:0" { "Guest driver: automatic once the host driver is staged." }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BUCKET: &str = "https://storage.googleapis.com/nvidia-drivers-us-public/GRID";

    #[test]
    fn every_release_pairs_a_run_and_exe_on_the_public_bucket() {
        for r in RELEASES {
            assert!(
                r.run_url.starts_with(BUCKET) && r.run_url.ends_with("-grid.run"),
                "{}",
                r.label
            );
            assert!(
                r.exe_url.starts_with(BUCKET) && r.exe_url.ends_with(".exe"),
                "{}",
                r.label
            );
            // The Linux guest version equals the host branch, so the branch appears in its .run URL.
            assert!(
                r.run_url.contains(r.branch),
                "run url must embed the host branch for {}",
                r.label
            );
        }
    }

    #[test]
    fn branch_lookup_is_exact() {
        assert_eq!(
            release_for_branch("550.127.05").map(|r| r.label),
            Some("vGPU 17.4")
        );
        assert!(release_for_branch("").is_none());
        assert!(release_for_branch("999.99.99").is_none());
    }
}

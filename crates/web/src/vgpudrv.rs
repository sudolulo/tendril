//! NVIDIA vGPU host-driver staging + on-appliance build.
//!
//! The licensed NVIDIA `.run` can't ship with Tendril, but the admin shouldn't have to SSH in and `cp`
//! it either. This lets them **upload the file or give a URL** in the web UI; the driver is staged under
//! the Tendril state dir, and — since the appliance is a bootc host with podman and the vGPU build
//! assets baked in — Tendril can **build the variant image right here** from the running image + the
//! staged driver, then the admin `bootc switch`es into it.

use axum::extract::Multipart;
use maud::{html, Markup};

use crate::ui;

fn dir() -> String {
    std::env::var("TENDRIL_VGPU_DIR").unwrap_or_else(|_| "/var/lib/tendril/vgpu".to_string())
}
fn run_path() -> String {
    std::env::var("TENDRIL_VGPU_RUN").unwrap_or_else(|_| format!("{}/nvidia-vgpu.run", dir()))
}
/// Where we record the staged host driver's version (e.g. "550.127.05"). bootc-image-builder renames
/// the `.run` to a version-less name, discarding the branch — so we capture it from the original
/// filename / download URL at stage time. This is the key that makes guest-driver selection automatic.
fn branch_path() -> String {
    format!("{}/host-branch", dir())
}

/// Best-effort extract an NVIDIA driver version like `550.127.05` (or `550.127`) from a filename/URL.
/// vGPU host driver majors are 3 digits (470/535/550/…), which lets us ignore noise like `x86_64`.
pub fn parse_nvidia_version(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if !b[i].is_ascii_digit() {
            i += 1;
            continue;
        }
        let start = i;
        while i < b.len()
            && (b[i].is_ascii_digit()
                || (b[i] == b'.' && i + 1 < b.len() && b[i + 1].is_ascii_digit()))
        {
            i += 1;
        }
        let tok = &s[start..i];
        let major = tok.split('.').next().unwrap_or("");
        if tok.contains('.') && major.len() >= 3 {
            return Some(tok.to_string());
        }
    }
    None
}

/// The installed/staged NVIDIA vGPU host driver version, or None. Prefers what we captured at stage
/// time; falls back to the live host (`/proc/driver/nvidia/version`) when the driver is actually loaded.
pub fn host_vgpu_branch() -> Option<String> {
    if let Ok(v) = std::fs::read_to_string(branch_path()) {
        let v = v.trim();
        if !v.is_empty() {
            return Some(v.to_string());
        }
    }
    let proc = std::fs::read_to_string("/proc/driver/nvidia/version").ok()?;
    parse_nvidia_version(&proc)
}
fn log_path() -> String {
    format!("{}/build.log", dir())
}
fn building_marker() -> String {
    format!("{}/.building", dir())
}
/// The baked appliance build script (present only on a real Tendril image).
fn build_script() -> Option<String> {
    let p = "/usr/libexec/tendril/appliance-vgpu-build.sh";
    std::path::Path::new(p).exists().then(|| p.to_string())
}

/// Size of the staged `.run`, if present.
fn staged() -> Option<u64> {
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

fn building() -> bool {
    std::path::Path::new(&building_marker()).exists()
}

// ── staging: upload a file or fetch a URL ─────────────────────────────────────────────────────

/// Accept the driver as a multipart upload (`runfile`) or a URL (`url`). The file is streamed to disk
/// (it's hundreds of MB) then atomically renamed into place.
pub async fn stage(mut mp: Multipart) -> Markup {
    if ui::is_demo() {
        return section(Some(html! { div.banner.warn { "Disabled in the demo." } }));
    }
    let _ = std::fs::create_dir_all(dir());
    let tmp = format!("{}/.nvidia-vgpu.run.part", dir());
    let mut url = String::new();
    let mut src_name = String::new(); // original filename — carries the version we want to capture
    let mut wrote = 0u64;

    while let Ok(Some(mut field)) = mp.next_field().await {
        match field.name().unwrap_or("") {
            "runfile" => {
                src_name = field.file_name().map(|s| s.to_string()).unwrap_or_default();
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

    // A URL was given and no file was uploaded → fetch it. Require an absolute http(s) URL so a value
    // like `file:///…` (SSRF/local read) or a `-K`/`--config` token (curl argument injection) can't be
    // fetched server-side.
    if wrote == 0 && !url.is_empty() {
        if !ui::is_http_url(&url) {
            return section(Some(
                html! { div.banner.error { "Provide an http(s):// URL to download the driver from." } },
            ));
        }
        match ui::run_result(
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
                &url,
            ],
        ) {
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
            html! { div.banner.error { "Provide the driver file or a URL to download it from." } },
        ));
    }
    if wrote < 1 << 20 {
        let _ = std::fs::remove_file(&tmp);
        return section(Some(
            html! { div.banner.error { "That doesn't look like an NVIDIA vGPU driver (too small). Expected the multi-hundred-MB " code { "-vgpu-kvm.run" } "." } },
        ));
    }
    if let Err(e) = std::fs::rename(&tmp, run_path()) {
        return section(Some(
            html! { div.banner.error { "Couldn't store the driver: " (e) } },
        ));
    }
    // Capture the version (from the original filename or URL) so guest-driver selection can match it
    // automatically later — the file itself is now renamed to a version-less name.
    match parse_nvidia_version(&src_name).or_else(|| parse_nvidia_version(&url)) {
        Some(v) => {
            let _ = std::fs::write(branch_path(), &v);
            // Eagerly fetch BOTH matching guest drivers (Windows + Linux) in the background, so vGPU
            // stations install with zero user steps and no create-time wait.
            crate::vgpuguest::prefetch(&v);
        }
        None => {
            let _ = std::fs::remove_file(branch_path());
        }
    }
    section(Some(
        html! { div.banner.ok { "Driver staged (" (human(wrote)) "). Build the vGPU image below." } },
    ))
}

pub async fn clear() -> Markup {
    if ui::is_demo() {
        return section(Some(html! { div.banner.warn { "Disabled in the demo." } }));
    }
    let _ = std::fs::remove_file(run_path());
    section(Some(html! { div.banner.ok { "Staged driver removed." } }))
}

// ── build ─────────────────────────────────────────────────────────────────────────────────────

/// Kick off the appliance build in the background, streaming output to the build log.
pub async fn build() -> Markup {
    if ui::is_demo() {
        return section(Some(html! { div.banner.warn { "Disabled in the demo." } }));
    }
    let Some(script) = build_script() else {
        return section(Some(
            html! { div.banner.error { "This host doesn't have the vGPU build assets (needs a Tendril bootc image)." } },
        ));
    };
    if staged().is_none() {
        return section(Some(
            html! { div.banner.error { "Stage the NVIDIA driver first." } },
        ));
    }
    if building() {
        return section(Some(
            html! { div.banner.warn { "A build is already running." } },
        ));
    }
    let _ = std::fs::write(building_marker(), "");
    let _ = std::fs::write(log_path(), "Starting vGPU image build…\n");
    let marker = building_marker();
    let log = log_path();
    std::thread::spawn(move || {
        use std::process::{Command, Stdio};
        let out = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log)
            .ok();
        let (o, e) = match out {
            Some(f) => (Stdio::from(f.try_clone().unwrap()), Stdio::from(f)),
            None => (Stdio::null(), Stdio::null()),
        };
        let _ = Command::new(script)
            .arg("nvidia")
            .stdout(o)
            .stderr(e)
            .status();
        let _ = std::fs::remove_file(&marker);
    });
    section(None)
}

/// The live build status/log fragment (polled while a build runs).
pub async fn build_status() -> Markup {
    let on = building();
    let tail = std::fs::read_to_string(log_path())
        .ok()
        .map(|s| {
            s.lines()
                .rev()
                .take(40)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default();
    html! {
        div #vgpu-build
            hx-get="/hardware/vgpu/buildstatus"
            hx-trigger=[on.then_some("load delay:2s")]
            hx-swap="outerHTML"
        {
            @if on { div.sub { span.led {} " Building the vGPU image… (this takes several minutes)" } }
            @if !tail.is_empty() {
                pre.mono style="margin:8px 0 0; max-height:220px; overflow:auto; padding:8px 10px; background:var(--bg2,#0002); border-radius:6px; font-size:12px" { (tail) }
            }
            @if !on && tail.contains("bootc switch localhost/tendril:vgpu-nvidia") {
                div.banner.ok style="margin-top:8px" { "Build finished. Deploy it: "
                    code { "sudo bootc switch localhost/tendril:vgpu-nvidia && sudo reboot" }
                }
            }
        }
    }
}

// ── panel section (embedded in the Hardware vGPU driver guide) ─────────────────────────────────

/// The NVIDIA driver staging + build UI, embedded in the vGPU driver panel's "NVIDIA / not installed"
/// branch. `banner` shows the result of the last action.
pub fn section(banner: Option<Markup>) -> Markup {
    let sz = staged();
    let can_build = build_script().is_some();
    html! {
        div #vgpu-run style="margin-top:8px" {
            @if let Some(b) = banner { (b) }
            @if let Some(n) = sz {
                div.sub { "Staged driver: " b { (human(n)) } " ✓ "
                    button.btn.sm style="margin-left:8px"
                        hx-post="/hardware/vgpu/run/clear" hx-target="#vgpu-run" hx-swap="outerHTML"
                        hx-confirm="Remove the staged NVIDIA driver?" { "Remove" }
                }
                @if can_build {
                    div.btnrow style="margin-top:10px" {
                        button.btn.primary
                            hx-post="/hardware/vgpu/build" hx-target="#vgpu-run" hx-swap="outerHTML"
                            hx-confirm="Build the NVIDIA vGPU image on this host now? It compiles the driver against the running kernel and takes several minutes." {
                            "Build vGPU image"
                        }
                    }
                    (build_status_placeholder())
                } @else {
                    div.sub style="margin-top:8px" { "Build it on a host with the repo + podman: " code { "scripts/build-vgpu-variant.sh nvidia" } " (it uses this staged driver via " code { "TENDRIL_VGPU_RUN" } ")." }
                }
            } @else {
                p.sub style="margin:0 0 6px" { "Add the licensed NVIDIA vGPU host driver — upload the " code { ".run" } " or give a URL you can reach it at." }
                form hx-post="/hardware/vgpu/run" hx-encoding="multipart/form-data"
                    hx-target="#vgpu-run" hx-swap="outerHTML" {
                    div.field style="margin:0 0 8px" {
                        label { "Driver file (" code { "NVIDIA-Linux-x86_64-<ver>-vgpu-kvm.run" } ")" }
                        input type="file" name="runfile" accept=".run" style="font-size:12.5px";
                    }
                    div.field style="margin:0 0 8px" {
                        label { "…or a URL to download it from" }
                        input type="url" name="url" placeholder="https://…/NVIDIA-…-vgpu-kvm.run";
                        span.hint {
                            "Use a URL you're authorized to reach (your NVIDIA "
                            a href="https://www.nvidia.com/en-us/data-center/resources/vgpu-evaluation/" { "vGPU eval" }
                            " download). This community guide walks through obtaining the driver: "
                            a href="https://wvthoog.nl/proxmox-vgpu-v3/" { "wvthoog's NVIDIA vGPU guide" } "."
                        }
                    }
                    button.btn.primary type="submit" { "Stage driver" }
                }
            }
        }
    }
}

/// Empty build-status container that immediately polls once (so a running build shows up).
fn build_status_placeholder() -> Markup {
    html! {
        div #vgpu-build hx-get="/hardware/vgpu/buildstatus" hx-trigger="load" hx-swap="outerHTML" {}
    }
}

#[cfg(test)]
mod tests {
    use super::parse_nvidia_version;

    #[test]
    fn parses_driver_versions_ignoring_noise() {
        assert_eq!(
            parse_nvidia_version("NVIDIA-Linux-x86_64-550.127.05-vgpu-kvm.run").as_deref(),
            Some("550.127.05")
        );
        assert_eq!(
            parse_nvidia_version("https://host/vGPU17.4/NVIDIA-Linux-x86_64-550.127.05-grid.run")
                .as_deref(),
            Some("550.127.05")
        );
        assert_eq!(
            parse_nvidia_version("535.161.08").as_deref(),
            Some("535.161.08")
        );
        assert_eq!(parse_nvidia_version("470.256").as_deref(), Some("470.256"));
        assert_eq!(
            parse_nvidia_version("x86_64 only, no version").as_deref(),
            None
        );
        assert_eq!(parse_nvidia_version("vGPU 17.4").as_deref(), None); // 2-digit major = release, not driver
    }
}

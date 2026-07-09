//! Guest disk provisioning: create the backing disk a station's OS installs onto.
//!
//! Uses `qemu-img` (from `qemu-utils`) rather than linking anything, keeping the workspace
//! dependency-free.

use std::io;
use std::path::Path;
use std::process::Command;

use crate::kickstart::{render_kickstart, KickstartSpec};
use crate::unattend::{render_autounattend, UnattendSpec};

/// Run `cmd` to completion; a non-zero exit becomes an `io::Error` carrying the trimmed stderr.
fn run_checked(cmd: &mut Command) -> io::Result<()> {
    let out = cmd.output()?;
    if out.status.success() {
        Ok(())
    } else {
        Err(io::Error::other(
            String::from_utf8_lossy(&out.stderr).trim().to_string(),
        ))
    }
}

/// Create a qcow2 disk of `size_gib` gigabytes at `path` (fails if it already exists).
pub fn create_disk(path: &Path, size_gib: u32) -> io::Result<()> {
    if path.exists() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("disk already exists: {}", path.display()),
        ));
    }
    run_checked(Command::new("qemu-img").args([
        "create",
        "-f",
        "qcow2",
        &path.to_string_lossy(),
        &format!("{size_gib}G"),
    ]))
}

/// Create a qcow2 **overlay** at `path` backed by `base` (copy-on-write). The base image is shared,
/// not copied — new stations cloned from a golden image cost only their own writes. Fails if `path`
/// exists or `base` is missing. NOTE: the overlay references `base` by path, so the base must stay put
/// (and, for clustering later, exist at the same path on each node).
pub fn create_overlay(path: &Path, base: &Path) -> io::Result<()> {
    if path.exists() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("disk already exists: {}", path.display()),
        ));
    }
    if !base.exists() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("base image not found: {}", base.display()),
        ));
    }
    run_checked(Command::new("qemu-img").args([
        "create",
        "-f",
        "qcow2",
        "-F",
        "qcow2",
        "-b",
        &base.to_string_lossy(),
        &path.to_string_lossy(),
    ]))
}

/// The install media a station needs to bring up its guest OS.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InstallMedia {
    /// The OS install ISO (Windows, or a SteamOS-style Bazzite ISO).
    pub install_iso: Option<String>,
    /// The virtio-win driver ISO — needed so Windows can see the virtio disk during setup.
    pub virtio_iso: Option<String>,
    /// A "seed" ISO driving the install hands-off: `autounattend.xml` for Windows, or a
    /// `ks.cfg` kickstart (on an `OEMDRV`-labelled disc) for a Bazzite station.
    pub seed_iso: Option<String>,
}

impl InstallMedia {
    /// No install media (a station booting an already-installed disk).
    pub fn none() -> Self {
        Self::default()
    }
}

/// Build a seed ISO carrying `autounattend.xml` for a hands-off Windows install.
///
/// Windows Setup scans every attached removable/optical drive for `autounattend.xml` during WinPE, so
/// this seed disc — attached alongside the Windows and virtio ISOs — drives the whole install without
/// a keypress.
pub fn build_seed_iso(spec: &UnattendSpec, out_path: &Path) -> io::Result<()> {
    build_seed_iso_with(spec, &[], out_path)
}

/// Like [`build_seed_iso`] but also copies `extras` (disc filename → source path) onto the seed disc —
/// used to carry the licensed NVIDIA vGPU **guest** driver installer, which the answer file's
/// first-logon commands then run from the disc. The binaries are large (hundreds of MB), so they're
/// copied by path rather than held in memory.
pub fn build_seed_iso_with(
    spec: &UnattendSpec,
    extras: &[(&str, &Path)],
    out_path: &Path,
) -> io::Result<()> {
    // Any volume label works — Windows scans all drives for the file by name.
    build_media_iso(
        &[("autounattend.xml", render_autounattend(spec))],
        extras,
        "TENDRIL_SEED",
        out_path,
    )
}

/// Build a seed ISO carrying a `ks.cfg` kickstart for a hands-off Bazzite (SteamOS-style) install.
///
/// The disc is labelled `OEMDRV`, which Anaconda auto-detects and reads `ks.cfg` from — no installer
/// kernel argument or media modification needed.
pub fn build_kickstart_seed(spec: &KickstartSpec, out_path: &Path) -> io::Result<()> {
    build_kickstart_seed_with(spec, &[], out_path)
}

/// Like [`build_kickstart_seed`] but also copies `extras` (disc filename → source path) onto the seed —
/// used to carry the NVIDIA vGPU **guest** `.run`, which the kickstart's first-boot service then
/// installs. Binaries are copied by path (they're hundreds of MB).
pub fn build_kickstart_seed_with(
    spec: &KickstartSpec,
    extras: &[(&str, &Path)],
    out_path: &Path,
) -> io::Result<()> {
    build_media_iso(
        &[("ks.cfg", render_kickstart(spec))],
        extras,
        "OEMDRV",
        out_path,
    )
}

/// Write `files` (name → text content) plus `extras` (disc filename → source path, copied verbatim) to
/// the root of a fresh ISO with volume `label`, at `out_path`. Uses `genisoimage` (or `mkisofs`),
/// already needed by `fetch-windows-media.sh`.
fn build_media_iso(
    files: &[(&str, String)],
    extras: &[(&str, &Path)],
    label: &str,
    out_path: &Path,
) -> io::Result<()> {
    let staging = out_path.with_extension("seed.d");
    // Fresh staging dir each time so a re-run reflects the current spec.
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging)?;
    for (name, content) in files {
        std::fs::write(staging.join(name), content)?;
    }
    for (name, src) in extras {
        std::fs::copy(src, staging.join(name)).map_err(|e| {
            io::Error::new(
                e.kind(),
                format!("staging {} onto seed disc: {e}", src.display()),
            )
        })?;
    }

    let mkisofs = which_mkisofs().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "neither genisoimage nor mkisofs found (install genisoimage)",
        )
    })?;
    let result = run_checked(Command::new(mkisofs).args([
        "-quiet",
        "-J", // Joliet + Rock Ridge, so the long filename survives
        "-r",
        "-V",
        label,
        "-o",
        &out_path.to_string_lossy(),
        &staging.to_string_lossy(),
    ]));
    // Clean up the staging dir on success and error alike.
    let _ = std::fs::remove_dir_all(&staging);
    result
}

fn which_mkisofs() -> Option<&'static str> {
    ["genisoimage", "mkisofs"].into_iter().find(|tool| {
        Command::new(tool)
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    })
}

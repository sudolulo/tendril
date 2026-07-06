//! Guest disk provisioning: create the backing disk a station's OS installs onto.
//!
//! Uses `qemu-img` (from `qemu-utils`) rather than linking anything, keeping the workspace
//! dependency-free.

use std::io;
use std::path::Path;
use std::process::Command;

use crate::unattend::{render_autounattend, UnattendSpec};

/// Create a qcow2 disk of `size_gib` gigabytes at `path` (fails if it already exists).
pub fn create_disk(path: &Path, size_gib: u32) -> io::Result<()> {
    if path.exists() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("disk already exists: {}", path.display()),
        ));
    }
    let out = Command::new("qemu-img")
        .args([
            "create",
            "-f",
            "qcow2",
            &path.to_string_lossy(),
            &format!("{size_gib}G"),
        ])
        .output()?;
    if out.status.success() {
        Ok(())
    } else {
        Err(io::Error::other(
            String::from_utf8_lossy(&out.stderr).trim().to_string(),
        ))
    }
}

/// The install media a station needs to bring up its guest OS.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InstallMedia {
    /// The OS install ISO (Windows or SteamOS/HoloISO/Bazzite).
    pub install_iso: Option<String>,
    /// The virtio-win driver ISO — needed so Windows can see the virtio disk during setup.
    pub virtio_iso: Option<String>,
    /// A "seed" ISO carrying `autounattend.xml` — makes the Windows install hands-off.
    pub unattend_iso: Option<String>,
}

impl InstallMedia {
    /// No install media (a station booting an already-installed disk).
    pub fn none() -> Self {
        Self::default()
    }
}

/// Build a small ISO carrying `autounattend.xml` at its root, from `spec`, at `out_path`.
///
/// Windows Setup scans every attached removable/optical drive for `autounattend.xml` during WinPE, so
/// this seed disc — attached alongside the Windows and virtio ISOs — drives the whole install without
/// a keypress. Uses `genisoimage` (or `mkisofs`), already needed by `fetch-windows-media.sh`.
pub fn build_seed_iso(spec: &UnattendSpec, out_path: &Path) -> io::Result<()> {
    let staging = out_path.with_extension("seed.d");
    // Fresh staging dir each time so a re-run reflects the current spec.
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging)?;
    std::fs::write(staging.join("autounattend.xml"), render_autounattend(spec))?;

    let mkisofs = which_mkisofs().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "neither genisoimage nor mkisofs found (install genisoimage)",
        )
    })?;
    let out = Command::new(mkisofs)
        .args([
            "-quiet",
            "-J", // Joliet, so Windows reads the filename
            "-r",
            "-V",
            "TENDRIL_SEED",
            "-o",
            &out_path.to_string_lossy(),
            &staging.to_string_lossy(),
        ])
        .output()?;
    let _ = std::fs::remove_dir_all(&staging);
    if out.status.success() {
        Ok(())
    } else {
        Err(io::Error::other(
            String::from_utf8_lossy(&out.stderr).trim().to_string(),
        ))
    }
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

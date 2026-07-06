//! Guest disk provisioning: create the backing disk a station's OS installs onto.
//!
//! Uses `qemu-img` (from `qemu-utils`) rather than linking anything, keeping the workspace
//! dependency-free.

use std::io;
use std::path::Path;
use std::process::Command;

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
}

impl InstallMedia {
    /// No install media (a station booting an already-installed disk).
    pub fn none() -> Self {
        Self::default()
    }
}

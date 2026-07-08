//! Station provisioning service — the single code path that turns a resolved request into a running
//! (or just-defined) station VM.
//!
//! `tendril-guest` (CLI), the `tendril` console menu, and a future web UI all call [`provision`], so
//! a station is created identically no matter the front-end. Inputs are owned, front-end-agnostic
//! data ([`StationRequest`]) — a CLI fills it from flags, the menu from prompts, a web handler from a
//! request body.

use std::io;
use std::path::Path;

use crate::domain::{render, DomainSpec, UsbPassthrough};
use crate::guest::{create_disk, InstallMedia};
use crate::lifecycle::Libvirt;
use crate::station::{GuestOs, StationSpec};

/// A fully-resolved request to provision one station.
#[derive(Debug, Clone)]
pub struct StationRequest {
    pub name: String,
    pub guest: GuestOs,
    pub disk_path: String,
    pub size_gib: u32,
    /// Create the qcow2 disk (fails if it already exists).
    pub create_disk: bool,
    pub vcpus: u32,
    pub memory_mib: u64,
    pub native_hardware: bool,
    /// The GPU's whole IOMMU group to pass through (empty = headless / no GPU). Also carries an
    /// SR-IOV virtual function, which is just another PCI address.
    pub passthrough_addresses: Vec<String>,
    /// A pre-created mediated device (vGPU) to attach, by UUID (empty = none). Set instead of
    /// `passthrough_addresses` when the station gets a vGPU slice rather than a whole GPU.
    pub mdev_uuid: Option<String>,
    /// Install media (install ISO, virtio-win, unattended seed) — already resolved/built.
    pub media: InstallMedia,
    /// USB devices to pass through (a seat's keyboard/mouse/controller), by vendor/product id.
    pub usb_devices: Vec<UsbPassthrough>,
    /// Register the domain with libvirt.
    pub define: bool,
    /// Start the domain (implies `define`).
    pub start: bool,
    /// Host directory to share into the guest as a shared Steam library (virtio-fs). Resolved by the
    /// caller (it knows the store); `None` = no shared library. See docs/STEAM-GAMES.md.
    pub steam_library_dir: Option<String>,
    /// Optional persistent data volume as `(qcow2 path, size GiB)` — a separate disk for user data
    /// (games/saves) that survives OS/base-image swaps and re-splits. Created when `create_disk` is
    /// also set. `None` = no data volume.
    pub data_disk: Option<(String, u32)>,
}

impl StationRequest {
    /// True when this run boots an install ISO (rather than a finalized boot-from-disk station).
    pub fn is_installing(&self) -> bool {
        self.media.install_iso.is_some()
    }

    /// Whether starting this run needs the firmware "press any key to boot from CD" prompt cleared —
    /// only the Windows installer stalls on it (the Bazzite boot menu auto-selects).
    pub fn needs_boot_prompt_clear(&self) -> bool {
        self.is_installing() && matches!(self.guest, GuestOs::Windows)
    }
}

/// What [`provision`] did.
#[derive(Debug, Clone)]
pub struct ProvisionReport {
    /// The rendered libvirt domain XML.
    pub xml: String,
    pub disk_created: bool,
    pub defined: bool,
    pub started: bool,
}

/// Provision `req`: create the disk (if asked), render the domain, and (optionally) define + start
/// it. Does not clear the Windows boot prompt itself — the caller decides that after `start` so it
/// can surface progress (see [`StationRequest::needs_boot_prompt_clear`] and
/// [`Libvirt::clear_boot_prompt`]).
pub fn provision(req: &StationRequest, lv: &Libvirt) -> io::Result<ProvisionReport> {
    let mut report = ProvisionReport {
        xml: String::new(),
        disk_created: false,
        defined: false,
        started: false,
    };

    if req.create_disk {
        create_disk(Path::new(&req.disk_path), req.size_gib)?;
        report.disk_created = true;
        // Create the persistent data volume alongside the boot disk when requested.
        if let Some((data_path, gib)) = &req.data_disk {
            create_disk(Path::new(data_path), *gib)?;
        }
    }

    // A GPU address is required by the spec even when nothing is passed through; the renderer only
    // emits hostdevs for `passthrough_addresses`, so this placeholder is inert when that's empty.
    let gpu_address = req
        .passthrough_addresses
        .first()
        .cloned()
        .unwrap_or_else(|| "0000:00:00.0".to_string());
    let station = StationSpec {
        name: req.name.clone(),
        guest: req.guest,
        gpu_address,
        native_hardware: req.native_hardware,
    };
    let spec = DomainSpec {
        station: &station,
        vcpus: req.vcpus,
        memory_mib: req.memory_mib,
        disk_path: req.disk_path.clone(),
        passthrough_addresses: req.passthrough_addresses.clone(),
        mdev_uuid: req.mdev_uuid.clone(),
        media: req.media.clone(),
        usb_devices: req.usb_devices.clone(),
        steam_library_dir: req.steam_library_dir.clone(),
        data_disk_path: req.data_disk.as_ref().map(|(p, _)| p.clone()),
    };
    report.xml = render(&spec);

    if req.define || req.start {
        lv.define(&req.name, &report.xml)?;
        report.defined = true;
    }
    if req.start {
        lv.start(&req.name)?;
        report.started = true;
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_request() -> StationRequest {
        StationRequest {
            name: "s1".to_string(),
            guest: GuestOs::Windows,
            disk_path: "/var/lib/tendril/s1.qcow2".to_string(),
            size_gib: 128,
            create_disk: false,
            vcpus: 8,
            memory_mib: 16384,
            native_hardware: false,
            passthrough_addresses: vec![],
            mdev_uuid: None,
            media: InstallMedia::none(),
            usb_devices: vec![],
            define: false,
            start: false,
            steam_library_dir: None,
            data_disk: None,
        }
    }

    #[test]
    fn data_volume_renders_a_second_disk() {
        let mut req = base_request();
        req.data_disk = Some(("/var/lib/tendril/s1-data.qcow2".to_string(), 256));
        let xml = provision(&req, &Libvirt::system()).unwrap().xml;
        assert!(
            xml.contains("dev='vdb'"),
            "data volume should attach as vdb"
        );
        assert!(xml.contains("s1-data.qcow2"));
        // Without a data disk, no vdb.
        let plain = provision(&base_request(), &Libvirt::system()).unwrap().xml;
        assert!(!plain.contains("dev='vdb'"));
    }

    #[test]
    fn renders_without_side_effects_when_not_defining() {
        // define=false, start=false, create_disk=false -> pure render, no system calls.
        let report = provision(&base_request(), &Libvirt::system()).unwrap();
        assert!(report.xml.contains("<name>s1</name>"));
        assert!(!report.disk_created && !report.defined && !report.started);
    }

    #[test]
    fn boot_prompt_clear_only_for_windows_install() {
        let mut req = base_request();
        req.media.install_iso = Some("/isos/win11.iso".to_string());
        assert!(req.needs_boot_prompt_clear());
        req.guest = GuestOs::SteamOs;
        assert!(!req.needs_boot_prompt_clear());
        req.guest = GuestOs::Windows;
        req.media.install_iso = None; // finalized boot-from-disk
        assert!(!req.needs_boot_prompt_clear());
    }
}

//! Render a [`StationSpec`] into a libvirt domain XML.
//!
//! Base domain plus composable overlays: GPU passthrough (the station's whole IOMMU group), OVMF
//! Secure Boot + an emulated TPM (both required by Windows 11), and the opt-in "native-hardware"
//! fingerprint reducer. CPU pinning and hugepages are TODO.

use std::fmt::Write as _;

use crate::guest::InstallMedia;
use crate::station::{GuestOs, StationSpec};

/// A USB device to pass through by vendor/product id (a seat's keyboard/mouse).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsbPassthrough {
    pub vendor_id: u16,
    pub product_id: u16,
}

/// A station's VM resources, paired with its [`StationSpec`], ready to render.
#[derive(Debug, Clone)]
pub struct DomainSpec<'a> {
    pub station: &'a StationSpec,
    /// Number of vCPUs.
    pub vcpus: u32,
    /// Guest memory in MiB.
    pub memory_mib: u64,
    /// Path to the VM's disk image (qcow2).
    pub disk_path: String,
    /// PCI addresses to pass through — the GPU's whole IOMMU group.
    pub passthrough_addresses: Vec<String>,
    /// Install media (OS ISO, plus virtio-win for Windows); empty once the disk is installed.
    pub media: InstallMedia,
    /// USB devices to pass through by id (per-seat keyboard/mouse); may be empty.
    pub usb_devices: Vec<UsbPassthrough>,
}

/// Render `spec` into a libvirt domain XML document.
pub fn render(spec: &DomainSpec) -> String {
    let s = spec.station;
    // Windows expects the RTC in local time; Linux/SteamOS expects UTC.
    let clock = match s.guest {
        GuestOs::Windows => "localtime",
        GuestOs::SteamOs => "utc",
    };

    let mut xml = String::new();
    let _ = writeln!(xml, "<domain type='kvm'>");
    let _ = writeln!(xml, "  <name>{}</name>", s.name);
    let _ = writeln!(xml, "  <memory unit='MiB'>{}</memory>", spec.memory_mib);
    let _ = writeln!(xml, "  <vcpu>{}</vcpu>", spec.vcpus);

    // Firmware: OVMF with Secure Boot (Windows 11 requires it).
    xml.push_str("  <os firmware='efi'>\n");
    xml.push_str("    <type arch='x86_64' machine='q35'>hvm</type>\n");
    xml.push_str("    <firmware>\n");
    xml.push_str("      <feature enabled='yes' name='secure-boot'/>\n");
    xml.push_str("    </firmware>\n");
    xml.push_str("  </os>\n");

    // Features (+ native-hardware fingerprint reduction).
    xml.push_str("  <features>\n");
    xml.push_str("    <acpi/>\n");
    xml.push_str("    <apic/>\n");
    // Secure Boot requires SMM; libvirt can't match a secure-boot firmware without it.
    xml.push_str("    <smm state='on'/>\n");
    if s.native_hardware {
        xml.push_str("    <kvm>\n      <hidden state='on'/>\n    </kvm>\n");
        xml.push_str(
            "    <hyperv>\n      <vendor_id state='on' value='0123456789ab'/>\n    </hyperv>\n",
        );
    }
    xml.push_str("  </features>\n");

    // CPU: host-passthrough for gaming; hide the hypervisor flag under native-hardware.
    if s.native_hardware {
        xml.push_str("  <cpu mode='host-passthrough' check='none' migratable='off'>\n");
        xml.push_str("    <feature policy='disable' name='hypervisor'/>\n");
        xml.push_str("  </cpu>\n");
    } else {
        xml.push_str("  <cpu mode='host-passthrough' check='none' migratable='off'/>\n");
    }

    let _ = writeln!(xml, "  <clock offset='{clock}'/>");

    xml.push_str("  <devices>\n");
    // Boot disk. Boots first unless install media is present.
    let disk_boot = if spec.media.install_iso.is_some() {
        2
    } else {
        1
    };
    xml.push_str("    <disk type='file' device='disk'>\n");
    xml.push_str("      <driver name='qemu' type='qcow2'/>\n");
    let _ = writeln!(xml, "      <source file='{}'/>", spec.disk_path);
    xml.push_str("      <target dev='vda' bus='virtio'/>\n");
    let _ = writeln!(xml, "      <boot order='{disk_boot}'/>");
    xml.push_str("    </disk>\n");
    // OS install ISO (cdrom) — boots first while present.
    if let Some(iso) = &spec.media.install_iso {
        xml.push_str("    <disk type='file' device='cdrom'>\n");
        xml.push_str("      <driver name='qemu' type='raw'/>\n");
        let _ = writeln!(xml, "      <source file='{iso}'/>");
        xml.push_str("      <target dev='sda' bus='sata'/>\n");
        xml.push_str("      <readonly/>\n");
        xml.push_str("      <boot order='1'/>\n");
        xml.push_str("    </disk>\n");
    }
    // virtio-win drivers (second cdrom) so Windows can see the virtio disk during setup.
    if let Some(iso) = &spec.media.virtio_iso {
        xml.push_str("    <disk type='file' device='cdrom'>\n");
        xml.push_str("      <driver name='qemu' type='raw'/>\n");
        let _ = writeln!(xml, "      <source file='{iso}'/>");
        xml.push_str("      <target dev='sdb' bus='sata'/>\n");
        xml.push_str("      <readonly/>\n");
        xml.push_str("    </disk>\n");
    }
    // Unattended-setup seed (third cdrom): autounattend.xml (Windows) or a kickstart (Bazzite).
    if let Some(iso) = &spec.media.seed_iso {
        xml.push_str("    <disk type='file' device='cdrom'>\n");
        xml.push_str("      <driver name='qemu' type='raw'/>\n");
        let _ = writeln!(xml, "      <source file='{iso}'/>");
        xml.push_str("      <target dev='sdc' bus='sata'/>\n");
        xml.push_str("      <readonly/>\n");
        xml.push_str("    </disk>\n");
    }
    // Network.
    xml.push_str("    <interface type='network'>\n");
    xml.push_str("      <source network='default'/>\n");
    xml.push_str("      <model type='virtio'/>\n");
    xml.push_str("    </interface>\n");
    // TPM 2.0 (Windows 11).
    xml.push_str("    <tpm model='tpm-crb'>\n");
    xml.push_str("      <backend type='emulator' version='2.0'/>\n");
    xml.push_str("    </tpm>\n");
    // A console for setup, before the passed-through GPU drives the real monitor. Listens on all
    // interfaces so the web console's proxy — and a native VNC viewer on the LAN — can reach it.
    xml.push_str("    <graphics type='vnc' port='-1' listen='0.0.0.0'/>\n");
    xml.push_str("    <video>\n      <model type='virtio'/>\n    </video>\n");
    // GPU passthrough: the whole IOMMU group as one unit.
    for addr in &spec.passthrough_addresses {
        if let Some(src) = pci_address_xml(addr) {
            xml.push_str("    <hostdev mode='subsystem' type='pci' managed='yes'>\n");
            let _ = writeln!(xml, "      <source>\n        {src}\n      </source>");
            xml.push_str("    </hostdev>\n");
        }
    }
    // Per-device USB passthrough (a seat's keyboard/mouse), by vendor/product id.
    for usb in &spec.usb_devices {
        xml.push_str("    <hostdev mode='subsystem' type='usb' managed='yes'>\n");
        let _ = writeln!(
            xml,
            "      <source>\n        <vendor id='0x{:04x}'/>\n        <product id='0x{:04x}'/>\n      </source>",
            usb.vendor_id, usb.product_id
        );
        xml.push_str("    </hostdev>\n");
    }
    xml.push_str("  </devices>\n");
    xml.push_str("</domain>\n");
    xml
}

/// Convert a PCI address like `0000:83:00.0` into a libvirt `<address .../>` element.
fn pci_address_xml(address: &str) -> Option<String> {
    let (bdf, function) = address.rsplit_once('.')?;
    let [domain, bus, slot] = bdf.split(':').collect::<Vec<_>>()[..] else {
        return None;
    };
    Some(format!(
        "<address domain='0x{domain}' bus='0x{bus}' slot='0x{slot}' function='0x{function}'/>"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::station::{GuestOs, StationSpec};

    fn station(native_hardware: bool, guest: GuestOs) -> StationSpec {
        StationSpec {
            name: "s1".to_string(),
            guest,
            gpu_address: "0000:83:00.0".to_string(),
            native_hardware,
        }
    }

    #[test]
    fn renders_core_domain_with_passthrough_group() {
        let st = station(false, GuestOs::Windows);
        let spec = DomainSpec {
            station: &st,
            vcpus: 8,
            memory_mib: 16384,
            disk_path: "/var/lib/tendril/s1.qcow2".to_string(),
            passthrough_addresses: vec!["0000:83:00.0".to_string(), "0000:83:00.1".to_string()],
            media: InstallMedia::none(),
            usb_devices: vec![],
        };
        let xml = render(&spec);
        assert!(xml.contains("<name>s1</name>"));
        assert!(xml.contains("<boot order='1'/>")); // disk boots first (no install media)
        assert!(xml.contains("<memory unit='MiB'>16384</memory>"));
        assert!(xml.contains("secure-boot"));
        assert!(xml.contains("<smm state='on'/>")); // required for Secure Boot
        assert!(xml.contains("<tpm"));
        assert!(xml.contains("offset='localtime'")); // Windows
        assert!(xml.contains("bus='0x83' slot='0x00' function='0x0'"));
        assert!(xml.contains("function='0x1'"));
        assert_eq!(xml.matches("<hostdev").count(), 2);
        assert!(!xml.contains("<hidden state='on'")); // native-hardware off
    }

    #[test]
    fn native_hardware_overlay_hides_the_hypervisor() {
        let st = station(true, GuestOs::SteamOs);
        let spec = DomainSpec {
            station: &st,
            vcpus: 4,
            memory_mib: 8192,
            disk_path: "/d.qcow2".to_string(),
            passthrough_addresses: vec![],
            media: InstallMedia::none(),
            usb_devices: vec![],
        };
        let xml = render(&spec);
        assert!(xml.contains("<hidden state='on'/>"));
        assert!(xml.contains("policy='disable' name='hypervisor'"));
        assert!(xml.contains("offset='utc'")); // SteamOS
    }

    #[test]
    fn install_media_adds_cdroms_and_boots_from_iso() {
        let st = station(false, GuestOs::Windows);
        let spec = DomainSpec {
            station: &st,
            vcpus: 4,
            memory_mib: 8192,
            disk_path: "/d.qcow2".to_string(),
            passthrough_addresses: vec![],
            media: InstallMedia {
                install_iso: Some("/isos/win11.iso".to_string()),
                virtio_iso: Some("/isos/virtio-win.iso".to_string()),
                seed_iso: Some("/isos/station1-seed.iso".to_string()),
            },
            usb_devices: vec![],
        };
        let xml = render(&spec);
        assert_eq!(xml.matches("device='cdrom'").count(), 3);
        assert!(xml.contains("/isos/win11.iso"));
        assert!(xml.contains("/isos/virtio-win.iso"));
        assert!(xml.contains("/isos/station1-seed.iso"));
        assert!(xml.contains("dev='sdc'")); // seed on the third cdrom
        assert!(xml.contains("<boot order='1'/>")); // cdrom first
        assert!(xml.contains("<boot order='2'/>")); // disk second
    }

    #[test]
    fn usb_devices_render_as_usb_hostdevs() {
        let st = station(false, GuestOs::Windows);
        let spec = DomainSpec {
            station: &st,
            vcpus: 4,
            memory_mib: 8192,
            disk_path: "/d.qcow2".to_string(),
            passthrough_addresses: vec![],
            media: InstallMedia::none(),
            usb_devices: vec![UsbPassthrough {
                vendor_id: 0x046d,
                product_id: 0xc52b,
            }],
        };
        let xml = render(&spec);
        assert!(xml.contains("type='usb'"));
        assert!(xml.contains("<vendor id='0x046d'/>"));
        assert!(xml.contains("<product id='0xc52b'/>"));
    }

    #[test]
    fn parses_pci_address() {
        assert_eq!(
            pci_address_xml("0000:83:00.0").unwrap(),
            "<address domain='0x0000' bus='0x83' slot='0x00' function='0x0'/>"
        );
        assert!(pci_address_xml("nonsense").is_none());
    }
}

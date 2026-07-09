//! Render a [`StationSpec`] into a libvirt domain XML.
//!
//! Base domain plus composable overlays: GPU passthrough (the station's whole IOMMU group), OVMF
//! Secure Boot + an emulated TPM (both required by Windows 11), low-latency CPU pinning + hugepages,
//! and the opt-in "native-hardware" fingerprint reducer (hides KVM/hypervisor + spoofs SMBIOS/DMI).

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
    /// PCI addresses to pass through — the GPU's whole IOMMU group, or an SR-IOV virtual function.
    pub passthrough_addresses: Vec<String>,
    /// A pre-created mediated device (vGPU) to attach, by UUID. Mutually exclusive with a whole-GPU
    /// `passthrough_addresses` group in practice, though the XML permits both.
    pub mdev_uuid: Option<String>,
    /// Install media (OS ISO, plus virtio-win for Windows); empty once the disk is installed.
    pub media: InstallMedia,
    /// USB devices to pass through by id (per-seat keyboard/mouse); may be empty.
    pub usb_devices: Vec<UsbPassthrough>,
    /// Host directory to share into the guest over virtio-fs as a shared Steam library (games
    /// installed once, read by many stations). `None` = no shared library. Requires shared-memory
    /// backing, which is emitted automatically when this is set. The guest mounts it by the tag
    /// `tendril-steamlib`.
    pub steam_library_dir: Option<String>,
    /// Optional persistent data volume — a second qcow2 attached as `vdb`, separate from the OS boot
    /// disk (`vda`). It survives OS/base-image swaps and re-splits, so user data (games, saves) is
    /// preserved when the boot disk is replaced. `None` = no data volume.
    pub data_disk_path: Option<String>,
    /// Optional low-latency CPU pinning: pin each vCPU 1:1 to a dedicated host physical CPU and keep
    /// the QEMU emulator/IO threads on separate host CPUs, so a gaming guest isn't rescheduled across
    /// the host's cores mid-frame. `None` = the host scheduler places vCPUs freely (default).
    pub cpu_pinning: Option<CpuPinning>,
    /// Back the guest's RAM with hugepages (fewer TLB misses → lower frame-time jitter). Requires a
    /// host hugepage pool; the caller only sets this when one exists, so the VM still starts.
    pub hugepages: bool,
}

/// A low-latency CPU pinning plan: which host physical CPU each vCPU pins to, plus the host CPUs the
/// QEMU emulator/IO threads run on (kept off the vCPU set so housekeeping never steals a game core).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CpuPinning {
    /// `(vcpu, host_cpu)` pairs — one per vCPU, 1:1.
    pub vcpupins: Vec<(u32, u32)>,
    /// Host CPUs (libvirt cpuset syntax, e.g. `0-1`) the emulator threads are pinned to.
    pub emulator_cpuset: String,
}

impl CpuPinning {
    /// Pin `vcpus` vCPUs 1:1 to the first `vcpus` entries of `host_cpus`, with emulator threads on
    /// `emulator_cpus`. `None` if there aren't enough host CPUs or no emulator CPU was given.
    pub fn new(vcpus: u32, host_cpus: &[u32], emulator_cpus: &[u32]) -> Option<Self> {
        if (host_cpus.len() as u32) < vcpus || vcpus == 0 || emulator_cpus.is_empty() {
            return None;
        }
        let vcpupins = (0..vcpus).map(|v| (v, host_cpus[v as usize])).collect();
        let emulator_cpuset = emulator_cpus
            .iter()
            .map(|c| c.to_string())
            .collect::<Vec<_>>()
            .join(",");
        Some(Self {
            vcpupins,
            emulator_cpuset,
        })
    }
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
    let _ = writeln!(xml, "  <name>{}</name>", xesc(&s.name));
    let _ = writeln!(xml, "  <memory unit='MiB'>{}</memory>", spec.memory_mib);
    // `placement='static'` when we pin, so libvirt honours the explicit vcpupin map below.
    if spec.cpu_pinning.is_some() {
        let _ = writeln!(xml, "  <vcpu placement='static'>{}</vcpu>", spec.vcpus);
    } else {
        let _ = writeln!(xml, "  <vcpu>{}</vcpu>", spec.vcpus);
    }
    // memoryBacking: hugepages (lower frame-time jitter) and/or shared memfd (virtio-fs needs the
    // guest memory shared with the vhost-user device). Emit one block covering whichever apply.
    if spec.hugepages || spec.steam_library_dir.is_some() {
        xml.push_str("  <memoryBacking>\n");
        if spec.hugepages {
            xml.push_str("    <hugepages/>\n");
        }
        if spec.steam_library_dir.is_some() {
            xml.push_str("    <source type='memfd'/>\n    <access mode='shared'/>\n");
        }
        xml.push_str("  </memoryBacking>\n");
    }
    // CPU pinning: pin each vCPU to its dedicated host core, emulator threads to the reserved set.
    if let Some(p) = &spec.cpu_pinning {
        xml.push_str("  <cputune>\n");
        for (vcpu, hostcpu) in &p.vcpupins {
            let _ = writeln!(xml, "    <vcpupin vcpu='{vcpu}' cpuset='{hostcpu}'/>");
        }
        let _ = writeln!(xml, "    <emulatorpin cpuset='{}'/>", p.emulator_cpuset);
        xml.push_str("  </cputune>\n");
    }

    // Firmware: OVMF with Secure Boot (Windows 11 requires it).
    xml.push_str("  <os firmware='efi'>\n");
    xml.push_str("    <type arch='x86_64' machine='q35'>hvm</type>\n");
    xml.push_str("    <firmware>\n");
    xml.push_str("      <feature enabled='yes' name='secure-boot'/>\n");
    xml.push_str("    </firmware>\n");
    // native-hardware: report the SMBIOS/DMI from <sysinfo> below, so the guest sees OEM strings
    // instead of the give-away "QEMU"/"Bochs"/"Standard PC" that anti-cheat + VM-detection read.
    if s.native_hardware {
        xml.push_str("    <smbios mode='sysinfo'/>\n");
    }
    xml.push_str("  </os>\n");

    // native-hardware: OEM-like SMBIOS tables. Values look like a real consumer desktop; the serials
    // derive from the station name so guests don't all share one (itself a tell) while render stays
    // deterministic.
    if s.native_hardware {
        let serial = fingerprint_serial(&s.name);
        xml.push_str("  <sysinfo type='smbios'>\n");
        xml.push_str("    <bios>\n");
        xml.push_str("      <entry name='vendor'>American Megatrends Inc.</entry>\n");
        xml.push_str("      <entry name='version'>2803</entry>\n");
        xml.push_str("      <entry name='date'>04/12/2023</entry>\n");
        xml.push_str("    </bios>\n");
        xml.push_str("    <system>\n");
        xml.push_str("      <entry name='manufacturer'>ASUS</entry>\n");
        xml.push_str("      <entry name='product'>System Product Name</entry>\n");
        xml.push_str("      <entry name='version'>System Version</entry>\n");
        let _ = writeln!(xml, "      <entry name='serial'>{serial}</entry>");
        xml.push_str("    </system>\n");
        xml.push_str("    <baseBoard>\n");
        xml.push_str("      <entry name='manufacturer'>ASUSTeK COMPUTER INC.</entry>\n");
        xml.push_str("      <entry name='product'>PRIME B550-PLUS</entry>\n");
        xml.push_str("      <entry name='version'>Rev X.0x</entry>\n");
        let _ = writeln!(xml, "      <entry name='serial'>{serial}</entry>");
        xml.push_str("    </baseBoard>\n");
        xml.push_str("  </sysinfo>\n");
    }

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
    // Shared Steam library over virtio-fs: the host's steam-library folder appears in the guest under
    // the tag `tendril-steamlib` (mounted + registered with Steam by the guest's first-boot setup).
    if let Some(dir) = &spec.steam_library_dir {
        xml.push_str("    <filesystem type='mount' accessmode='passthrough'>\n");
        xml.push_str("      <driver type='virtiofs'/>\n");
        let _ = writeln!(xml, "      <source dir='{}'/>", xesc(dir));
        xml.push_str("      <target dir='tendril-steamlib'/>\n");
        xml.push_str("    </filesystem>\n");
    }
    // Boot disk. Boots first unless install media is present.
    let disk_boot = if spec.media.install_iso.is_some() {
        2
    } else {
        1
    };
    xml.push_str("    <disk type='file' device='disk'>\n");
    xml.push_str("      <driver name='qemu' type='qcow2'/>\n");
    let _ = writeln!(xml, "      <source file='{}'/>", xesc(&spec.disk_path));
    xml.push_str("      <target dev='vda' bus='virtio'/>\n");
    let _ = writeln!(xml, "      <boot order='{disk_boot}'/>");
    xml.push_str("    </disk>\n");
    // Persistent data volume (vdb): a separate qcow2 that survives boot-disk / base-image swaps and
    // re-splits, so the user's games/saves are kept when the OS disk is replaced.
    if let Some(data) = &spec.data_disk_path {
        xml.push_str("    <disk type='file' device='disk'>\n");
        xml.push_str("      <driver name='qemu' type='qcow2'/>\n");
        let _ = writeln!(xml, "      <source file='{}'/>", xesc(data));
        xml.push_str("      <target dev='vdb' bus='virtio'/>\n");
        xml.push_str("    </disk>\n");
    }
    // OS install ISO (cdrom) — boots first while present.
    if let Some(iso) = &spec.media.install_iso {
        xml.push_str("    <disk type='file' device='cdrom'>\n");
        xml.push_str("      <driver name='qemu' type='raw'/>\n");
        let _ = writeln!(xml, "      <source file='{}'/>", xesc(iso));
        xml.push_str("      <target dev='sda' bus='sata'/>\n");
        xml.push_str("      <readonly/>\n");
        xml.push_str("      <boot order='1'/>\n");
        xml.push_str("    </disk>\n");
    }
    // virtio-win drivers (second cdrom) so Windows can see the virtio disk during setup.
    if let Some(iso) = &spec.media.virtio_iso {
        xml.push_str("    <disk type='file' device='cdrom'>\n");
        xml.push_str("      <driver name='qemu' type='raw'/>\n");
        let _ = writeln!(xml, "      <source file='{}'/>", xesc(iso));
        xml.push_str("      <target dev='sdb' bus='sata'/>\n");
        xml.push_str("      <readonly/>\n");
        xml.push_str("    </disk>\n");
    }
    // Unattended-setup seed (third cdrom): autounattend.xml (Windows) or a kickstart (Bazzite).
    if let Some(iso) = &spec.media.seed_iso {
        xml.push_str("    <disk type='file' device='cdrom'>\n");
        xml.push_str("      <driver name='qemu' type='raw'/>\n");
        let _ = writeln!(xml, "      <source file='{}'/>", xesc(iso));
        xml.push_str("      <target dev='sdc' bus='sata'/>\n");
        xml.push_str("      <readonly/>\n");
        xml.push_str("    </disk>\n");
    }
    // Network.
    xml.push_str("    <interface type='network'>\n");
    xml.push_str("      <source network='default'/>\n");
    xml.push_str("      <model type='virtio'/>\n");
    xml.push_str("    </interface>\n");
    // QEMU guest agent channel: once qemu-guest-agent is installed inside (Windows via virtio-win,
    // Bazzite via the qemu-guest-agent package), the host can read the guest's IP/hostname/OS and do
    // graceful shutdown. Harmless when the agent isn't present.
    xml.push_str("    <channel type='unix'>\n      <target type='virtio' name='org.qemu.guest_agent.0'/>\n    </channel>\n");
    // TPM 2.0 (Windows 11).
    xml.push_str("    <tpm model='tpm-crb'>\n");
    xml.push_str("      <backend type='emulator' version='2.0'/>\n");
    xml.push_str("    </tpm>\n");
    // A console for setup, before the passed-through GPU drives the real monitor. Listens on all
    // interfaces so the web console's proxy — and a native VNC viewer on the LAN — can reach it.
    xml.push_str("    <graphics type='vnc' port='-1' listen='0.0.0.0'/>\n");
    xml.push_str("    <video>\n      <model type='virtio'/>\n    </video>\n");
    // GPU passthrough: the whole IOMMU group as one unit (also an SR-IOV VF, which is a plain PCI fn).
    for addr in &spec.passthrough_addresses {
        if let Some(src) = pci_address_xml(addr) {
            xml.push_str("    <hostdev mode='subsystem' type='pci' managed='yes'>\n");
            let _ = writeln!(xml, "      <source>\n        {src}\n      </source>");
            xml.push_str("    </hostdev>\n");
        }
    }
    // vGPU: a mediated device, attached by UUID (created on the host before the domain starts).
    if let Some(uuid) = &spec.mdev_uuid {
        xml.push_str("    <hostdev mode='subsystem' type='mdev' model='vfio-pci'>\n");
        let _ = writeln!(
            xml,
            "      <source>\n        <address uuid='{uuid}'/>\n      </source>"
        );
        xml.push_str("    </hostdev>\n");
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

/// Escape a value for a single-quoted XML attribute / text node. Defense in depth: the web layer
/// validates station names, but this renderer is a public library API and disk/ISO paths are free-form.
fn xesc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('\'', "&apos;")
        .replace('"', "&quot;")
}

/// A deterministic pseudo-serial from a station name (FNV-1a → 12 hex digits), so native-hardware
/// guests report distinct OEM serials rather than one shared value that would itself flag a fleet.
fn fingerprint_serial(name: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in name.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{:012X}", h & 0xFFFF_FFFF_FFFF)
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
            mdev_uuid: None,
            media: InstallMedia::none(),
            usb_devices: vec![],
            steam_library_dir: None,
            data_disk_path: None,
            cpu_pinning: None,
            hugepages: false,
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
            mdev_uuid: None,
            media: InstallMedia::none(),
            usb_devices: vec![],
            steam_library_dir: None,
            data_disk_path: None,
            cpu_pinning: None,
            hugepages: false,
        };
        let xml = render(&spec);
        assert!(xml.contains("<hidden state='on'/>"));
        assert!(xml.contains("policy='disable' name='hypervisor'"));
        assert!(xml.contains("offset='utc'")); // SteamOS
                                               // SMBIOS/DMI is spoofed to OEM strings, and reported via <os><smbios mode='sysinfo'/>.
        assert!(xml.contains("<smbios mode='sysinfo'/>"));
        assert!(xml.contains("<sysinfo type='smbios'>"));
        assert!(xml.contains("American Megatrends"));
        assert!(!xml.contains("QEMU"));
        // Different station names yield different serials (so a fleet doesn't share one).
        assert_ne!(fingerprint_serial("alpha"), fingerprint_serial("beta"));
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
            mdev_uuid: None,
            media: InstallMedia {
                install_iso: Some("/isos/win11.iso".to_string()),
                virtio_iso: Some("/isos/virtio-win.iso".to_string()),
                seed_iso: Some("/isos/station1-seed.iso".to_string()),
            },
            usb_devices: vec![],
            steam_library_dir: None,
            data_disk_path: None,
            cpu_pinning: None,
            hugepages: false,
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
            mdev_uuid: None,
            media: InstallMedia::none(),
            usb_devices: vec![UsbPassthrough {
                vendor_id: 0x046d,
                product_id: 0xc52b,
            }],
            steam_library_dir: None,
            data_disk_path: None,
            cpu_pinning: None,
            hugepages: false,
        };
        let xml = render(&spec);
        assert!(xml.contains("type='usb'"));
        assert!(xml.contains("<vendor id='0x046d'/>"));
        assert!(xml.contains("<product id='0xc52b'/>"));
    }

    #[test]
    fn steam_library_renders_virtiofs_and_shared_memory() {
        let st = station(false, GuestOs::SteamOs);
        let mut spec = DomainSpec {
            station: &st,
            vcpus: 4,
            memory_mib: 8192,
            disk_path: "/d.qcow2".to_string(),
            passthrough_addresses: vec![],
            mdev_uuid: None,
            media: InstallMedia::none(),
            usb_devices: vec![],
            steam_library_dir: Some("/var/lib/tendril/store/steam-library".to_string()),
            data_disk_path: None,
            cpu_pinning: None,
            hugepages: false,
        };
        let on = render(&spec);
        assert!(
            on.contains("<access mode='shared'/>"),
            "virtio-fs needs shared memory backing"
        );
        assert!(on.contains("<driver type='virtiofs'/>"));
        assert!(on.contains("<source dir='/var/lib/tendril/store/steam-library'/>"));
        assert!(on.contains("<target dir='tendril-steamlib'/>"));
        // Off by default: neither the shared-memory backing nor the filesystem appear.
        spec.steam_library_dir = None;
        let off = render(&spec);
        assert!(!off.contains("virtiofs"));
        assert!(!off.contains("memoryBacking"));
    }

    #[test]
    fn mdev_uuid_renders_as_mdev_hostdev() {
        let st = station(false, GuestOs::Windows);
        let spec = DomainSpec {
            station: &st,
            vcpus: 4,
            memory_mib: 8192,
            disk_path: "/d.qcow2".to_string(),
            passthrough_addresses: vec![],
            mdev_uuid: Some("f2c0d6a4-1111-2222-3333-444455556666".to_string()),
            media: InstallMedia::none(),
            usb_devices: vec![],
            steam_library_dir: None,
            data_disk_path: None,
            cpu_pinning: None,
            hugepages: false,
        };
        let xml = render(&spec);
        assert!(xml.contains("type='mdev' model='vfio-pci'"));
        assert!(xml.contains("<address uuid='f2c0d6a4-1111-2222-3333-444455556666'/>"));
        assert_eq!(xml.matches("<hostdev").count(), 1); // just the mdev, no PCI group
    }

    #[test]
    fn cpu_pinning_and_hugepages_render() {
        let st = station(false, GuestOs::Windows);
        let spec = DomainSpec {
            station: &st,
            vcpus: 4,
            memory_mib: 8192,
            disk_path: "/d.qcow2".to_string(),
            passthrough_addresses: vec![],
            mdev_uuid: None,
            media: InstallMedia::none(),
            usb_devices: vec![],
            steam_library_dir: None,
            data_disk_path: None,
            cpu_pinning: CpuPinning::new(4, &[4, 5, 6, 7], &[0, 1]),
            hugepages: true,
        };
        let xml = render(&spec);
        assert!(xml.contains("<vcpu placement='static'>4</vcpu>"));
        assert!(xml.contains("<hugepages/>"));
        assert!(xml.contains("<cputune>"));
        assert!(xml.contains("<vcpupin vcpu='0' cpuset='4'/>"));
        assert!(xml.contains("<vcpupin vcpu='3' cpuset='7'/>"));
        assert!(xml.contains("<emulatorpin cpuset='0,1'/>"));
        // Off by default: no cputune / static placement / hugepages.
        let plain = station(false, GuestOs::Windows);
        let spec2 = DomainSpec {
            station: &plain,
            cpu_pinning: None,
            hugepages: false,
            ..spec
        };
        let xml2 = render(&spec2);
        assert!(!xml2.contains("cputune"));
        assert!(!xml2.contains("placement='static'"));
        assert!(!xml2.contains("hugepages"));
    }

    #[test]
    fn cpu_pinning_new_rejects_too_few_cores() {
        assert!(CpuPinning::new(4, &[4, 5], &[0]).is_none());
        assert!(CpuPinning::new(2, &[4, 5], &[]).is_none());
        assert_eq!(
            CpuPinning::new(2, &[6, 7], &[0, 1])
                .unwrap()
                .emulator_cpuset,
            "0,1"
        );
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

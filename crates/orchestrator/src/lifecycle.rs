//! Managing station VM lifecycle through the `virsh` CLI.
//!
//! We shell out to `virsh` rather than link libvirt so the workspace stays dependency-free. The
//! argument construction is pure (and unit-tested); only [`Libvirt::run`] touches the system.

use std::io;
use std::process::{Command, Output};

/// A libvirt connection, driven via `virsh`.
#[derive(Debug, Clone)]
pub struct Libvirt {
    /// Connection URI, e.g. `qemu:///system`.
    pub uri: String,
}

impl Default for Libvirt {
    fn default() -> Self {
        Self::system()
    }
}

/// A domain's run state (as reported by `virsh domstate`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DomainState {
    Running,
    Paused,
    ShutOff,
    /// The domain is not defined.
    Absent,
    Other,
}

impl DomainState {
    fn parse(s: &str) -> Self {
        match s.trim() {
            "running" => Self::Running,
            "paused" => Self::Paused,
            "shut off" => Self::ShutOff,
            "" => Self::Absent,
            _ => Self::Other,
        }
    }
}

/// Linux input keycode for Enter.
const KEY_ENTER: u32 = 28;
/// Enter taps (1/sec) after start. Generous, because firmware POST can be slow on a loaded host and
/// the "press any key to boot from CD" prompt has only a ~5-second window — miss it and the install
/// never starts. Extra taps land harmlessly in WinPE once Setup has taken over.
const KEY_ENTER_TAPS: u32 = 45;

impl Libvirt {
    /// The system libvirt instance (`qemu:///system`).
    pub fn system() -> Self {
        Self {
            uri: "qemu:///system".to_string(),
        }
    }

    /// The per-user session instance (`qemu:///session`).
    pub fn session() -> Self {
        Self {
            uri: "qemu:///session".to_string(),
        }
    }

    /// Build the full `virsh` argument list for a subcommand (connection URI + args). Pure.
    pub fn virsh_args(&self, sub: &[&str]) -> Vec<String> {
        let mut args = vec!["-c".to_string(), self.uri.clone()];
        args.extend(sub.iter().map(|s| (*s).to_string()));
        args
    }

    fn run(&self, sub: &[&str]) -> io::Result<Output> {
        Command::new("virsh").args(self.virsh_args(sub)).output()
    }

    fn ok(out: Output) -> io::Result<String> {
        if out.status.success() {
            Ok(String::from_utf8_lossy(&out.stdout).into_owned())
        } else {
            Err(io::Error::other(
                String::from_utf8_lossy(&out.stderr).trim().to_string(),
            ))
        }
    }

    /// Define a persistent domain from its XML. Validates, but does not start it.
    pub fn define(&self, name: &str, xml: &str) -> io::Result<()> {
        let path = std::env::temp_dir().join(format!("tendril-{name}.xml"));
        std::fs::write(&path, xml)?;
        let result = self.run(&["define", "--validate", &path.to_string_lossy()]);
        let _ = std::fs::remove_file(&path);
        Self::ok(result?)?;
        Ok(())
    }

    /// Start a defined domain (this is when a passthrough GPU is detached from the host).
    pub fn start(&self, name: &str) -> io::Result<()> {
        Self::ok(self.run(&["start", name])?).map(|_| ())
    }

    /// Send a key by Linux input keycode (e.g. 28 = Enter) to the domain's console.
    pub fn send_key(&self, name: &str, keycode: u32) -> io::Result<()> {
        Self::ok(self.run(&["send-key", name, &keycode.to_string()])?).map(|_| ())
    }

    /// Tap Enter repeatedly across the boot window.
    ///
    /// Windows install ISOs show "Press any key to boot from CD or DVD..." with a ~5-second timeout;
    /// if no key is pressed the firmware skips the CD and the unattended install never begins. With no
    /// human at the keyboard, we press it ourselves — harmless keystrokes once WinPE has taken over.
    pub fn clear_boot_prompt(&self, name: &str) {
        for _ in 0..KEY_ENTER_TAPS {
            let _ = self.send_key(name, KEY_ENTER);
            std::thread::sleep(std::time::Duration::from_secs(1));
        }
    }

    /// Request a graceful shutdown.
    pub fn shutdown(&self, name: &str) -> io::Result<()> {
        Self::ok(self.run(&["shutdown", name])?).map(|_| ())
    }

    /// Force a domain off.
    pub fn destroy(&self, name: &str) -> io::Result<()> {
        Self::ok(self.run(&["destroy", name])?).map(|_| ())
    }

    /// Remove a domain definition (and its nvram).
    pub fn undefine(&self, name: &str) -> io::Result<()> {
        Self::ok(self.run(&["undefine", name, "--nvram"])?).map(|_| ())
    }

    /// Attach a USB device (by vendor/product id) to a domain. Applies to the persistent config, and
    /// live too if the domain is running (hot-plug).
    pub fn attach_usb(&self, name: &str, vendor: u16, product: u16) -> io::Result<()> {
        self.usb_device_op("attach-device", name, vendor, product)
    }

    /// Detach a previously-attached USB device (by vendor/product id) from a domain.
    pub fn detach_usb(&self, name: &str, vendor: u16, product: u16) -> io::Result<()> {
        self.usb_device_op("detach-device", name, vendor, product)
    }

    fn usb_device_op(&self, op: &str, name: &str, vendor: u16, product: u16) -> io::Result<()> {
        let xml = format!(
            "<hostdev mode='subsystem' type='usb' managed='yes'>\
             <source><vendor id='0x{vendor:04x}'/><product id='0x{product:04x}'/></source>\
             </hostdev>"
        );
        let path =
            std::env::temp_dir().join(format!("tendril-usb-{name}-{vendor:04x}-{product:04x}.xml"));
        std::fs::write(&path, xml)?;
        let path_str = path.to_string_lossy().into_owned();
        // --config persists it; --live also hot-(un)plugs when the domain is running.
        let mut args = vec![op, name, path_str.as_str(), "--config"];
        if matches!(self.state(name), DomainState::Running) {
            args.push("--live");
        }
        let result = self.run(&args);
        let _ = std::fs::remove_file(&path);
        Self::ok(result?).map(|_| ())
    }

    /// The USB devices currently passed through to a domain, as `(vendor_id, product_id)` — parsed
    /// from its persistent XML.
    pub fn usb_devices(&self, name: &str) -> Vec<(u16, u16)> {
        let Ok(out) = self.run(&["dumpxml", "--inactive", name]) else {
            return Vec::new();
        };
        if !out.status.success() {
            return Vec::new();
        }
        parse_usb_hostdevs(&String::from_utf8_lossy(&out.stdout))
    }

    /// Names of all defined domains (running or not); empty if virsh is unreachable.
    pub fn list(&self) -> Vec<String> {
        match self.run(&["list", "--all", "--name"]) {
            Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout)
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .map(String::from)
                .collect(),
            _ => Vec::new(),
        }
    }

    /// Current state of a domain (`Absent` if it doesn't exist or virsh is unreachable).
    pub fn state(&self, name: &str) -> DomainState {
        match self.run(&["domstate", name]) {
            Ok(out) if out.status.success() => {
                DomainState::parse(&String::from_utf8_lossy(&out.stdout))
            }
            _ => DomainState::Absent,
        }
    }
}

/// Extract the `(vendor_id, product_id)` of every `type='usb'` `<hostdev>` in a domain's XML.
/// A tiny scan rather than a full XML parse — libvirt's output is stable and single-quoted.
fn parse_usb_hostdevs(xml: &str) -> Vec<(u16, u16)> {
    let mut out = Vec::new();
    for block in xml.split("<hostdev").skip(1) {
        let block = block.split("</hostdev>").next().unwrap_or(block);
        if !block.contains("type='usb'") {
            continue;
        }
        if let (Some(v), Some(p)) = (hex_attr(block, "vendor"), hex_attr(block, "product")) {
            out.push((v, p));
        }
    }
    out
}

/// Pull the hex id from a `<{elem} id='0x....'/>` element within `block`.
fn hex_attr(block: &str, elem: &str) -> Option<u16> {
    let needle = format!("<{elem} id='0x");
    let start = block.find(&needle)? + needle.len();
    let rest = &block[start..];
    let end = rest.find('\'')?;
    u16::from_str_radix(&rest[..end], 16).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn virsh_args_prepend_connection_uri() {
        let lv = Libvirt::system();
        assert_eq!(
            lv.virsh_args(&["start", "station1"]),
            vec!["-c", "qemu:///system", "start", "station1"]
        );
    }

    #[test]
    fn parses_usb_hostdevs_only() {
        let xml = "\
<domain>\
 <devices>\
  <hostdev mode='subsystem' type='pci' managed='yes'><source><address domain='0x0000' bus='0x07'/></source></hostdev>\
  <hostdev mode='subsystem' type='usb' managed='yes'><source><vendor id='0x046d'/><product id='0xc52b'/></source></hostdev>\
  <hostdev mode='subsystem' type='usb' managed='yes'><source><vendor id='0x1234'/><product id='0xabcd'/></source></hostdev>\
 </devices>\
</domain>";
        assert_eq!(
            parse_usb_hostdevs(xml),
            vec![(0x046d, 0xc52b), (0x1234, 0xabcd)]
        );
        assert!(parse_usb_hostdevs("<domain><devices/></domain>").is_empty());
    }

    #[test]
    fn parses_domain_states() {
        assert_eq!(DomainState::parse("running\n"), DomainState::Running);
        assert_eq!(DomainState::parse("shut off"), DomainState::ShutOff);
        assert_eq!(DomainState::parse("paused"), DomainState::Paused);
        assert_eq!(DomainState::parse(""), DomainState::Absent);
        assert_eq!(DomainState::parse("pmsuspended"), DomainState::Other);
    }
}

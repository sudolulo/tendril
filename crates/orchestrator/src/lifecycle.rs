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
    fn parses_domain_states() {
        assert_eq!(DomainState::parse("running\n"), DomainState::Running);
        assert_eq!(DomainState::parse("shut off"), DomainState::ShutOff);
        assert_eq!(DomainState::parse("paused"), DomainState::Paused);
        assert_eq!(DomainState::parse(""), DomainState::Absent);
        assert_eq!(DomainState::parse("pmsuspended"), DomainState::Other);
    }
}

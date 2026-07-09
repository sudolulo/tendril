//! Applying a [`ProvisioningPlan`]: turning bind targets into concrete host actions.
//!
//! Each [`Action`] is a single sysfs write. Binding a device to `vfio-pci` at runtime is a
//! three-step dance per device: unbind it from its current driver, set its `driver_override`, then
//! ask the PCI core to (re)probe it. Rendering is pure; execution is gated behind [`Mode::Execute`]
//! because it detaches the GPU from the host.

use std::fs;
use std::path::PathBuf;

use crate::strategy::ProvisioningPlan;

/// A single host mutation: write `value` into the sysfs `path`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Action {
    /// Human-readable description of the step.
    pub description: String,
    /// sysfs path written to.
    pub path: PathBuf,
    /// Value written to `path`.
    pub value: String,
    /// If true, a write failure is tolerated (e.g. unbinding a device that has no driver).
    pub optional: bool,
}

/// Whether [`execute`] performs the writes or only reports them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Report what would happen; touch nothing.
    DryRun,
    /// Actually perform the sysfs writes (detaches the GPU from the host driver).
    Execute,
}

/// Render the ordered host actions that enact `plan` at runtime.
pub fn render(plan: &ProvisioningPlan) -> Vec<Action> {
    let mut actions = Vec::with_capacity(plan.bind_addresses.len() * 3);
    for addr in &plan.bind_addresses {
        let dev = format!("/sys/bus/pci/devices/{addr}");
        actions.push(Action {
            description: format!("unbind {addr} from its current driver"),
            path: PathBuf::from(format!("{dev}/driver/unbind")),
            value: addr.clone(),
            optional: true,
        });
        actions.push(Action {
            description: format!("set driver_override={} for {addr}", plan.driver),
            path: PathBuf::from(format!("{dev}/driver_override")),
            value: plan.driver.clone(),
            optional: false,
        });
        actions.push(Action {
            description: format!("probe {addr} to bind {}", plan.driver),
            path: PathBuf::from("/sys/bus/pci/drivers_probe"),
            value: addr.clone(),
            optional: false,
        });
    }
    actions
}

/// Execute (or, in [`Mode::DryRun`], report) the actions in order.
pub fn execute(actions: &[Action], mode: Mode) -> std::io::Result<()> {
    let mut applied = 0usize;
    for action in actions {
        match mode {
            Mode::DryRun => println!(
                "  would write '{}' -> {}   # {}",
                action.value,
                action.path.display(),
                action.description
            ),
            Mode::Execute => match fs::write(&action.path, &action.value) {
                Ok(()) => {
                    applied += 1;
                    println!("  wrote '{}' -> {}", action.value, action.path.display());
                }
                Err(e) if action.optional => {
                    println!("  skipped {} ({e})", action.path.display());
                }
                // A required action failed mid-sequence: earlier binds/overrides are already applied,
                // so the host can be in a partial state. Surface that explicitly (there's no clean
                // rollback for a half-detached GPU) so the caller can re-run or reboot to recover.
                Err(e) => {
                    return Err(std::io::Error::new(
                        e.kind(),
                        format!(
                            "failed to write {} ({e}); {applied} earlier action(s) already applied — \
                             the host may be in a partial state (re-run apply or reboot)",
                            action.path.display()
                        ),
                    ));
                }
            },
        }
    }
    Ok(())
}

/// The kernel driver a PCI device is currently bound to, if any (reads the `driver` symlink).
pub fn current_driver(address: &str) -> Option<String> {
    let link = fs::read_link(format!("/sys/bus/pci/devices/{address}/driver")).ok()?;
    Some(link.file_name()?.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan() -> ProvisioningPlan {
        ProvisioningPlan {
            driver: "vfio-pci".to_string(),
            bind_addresses: vec!["0000:83:00.0".to_string(), "0000:83:00.1".to_string()],
            summary: "test".to_string(),
            note: None,
        }
    }

    #[test]
    fn renders_unbind_override_probe_per_device() {
        let actions = render(&plan());
        assert_eq!(actions.len(), 6); // 3 steps * 2 devices

        assert!(actions[0].path.ends_with("0000:83:00.0/driver/unbind"));
        assert!(actions[1].path.ends_with("0000:83:00.0/driver_override"));
        assert_eq!(actions[1].value, "vfio-pci");
        assert!(actions[2].path.ends_with("drivers_probe"));
        assert_eq!(actions[2].value, "0000:83:00.0");
    }

    #[test]
    fn unbind_is_optional_but_override_is_required() {
        let actions = render(&plan());
        assert!(actions[0].optional);
        assert!(!actions[1].optional);
        assert!(!actions[2].optional);
    }
}

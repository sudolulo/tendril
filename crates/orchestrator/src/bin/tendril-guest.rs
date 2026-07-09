//! `tendril-guest` — provision a station's disk and drive its OS install end-to-end.
//!
//! Composes the pieces: create the qcow2, generate an `autounattend.xml` seed ISO (Windows), render
//! the install domain (Windows ISO + virtio + seed, booting from the installer), and — with
//! `--define`/`--start` — register and launch it. Once Windows has installed itself, `--finalize`
//! re-renders the domain without any install media so it boots straight from disk.
//!
//! Safe by default: with no `--define`/`--start`/`--finalize`, it only creates the disk (if asked)
//! and prints the domain XML.
//!
//! ```text
//! # Windows station (autounattend.xml seed + virtio driver injection):
//! tendril-guest --create-disk --iso win11.iso --virtio-iso virtio-win.iso --unattend --start
//! # SteamOS-style station (Bazzite; kickstart seed on an OEMDRV disc):
//! tendril-guest --steamos --create-disk --iso bazzite-deck-nvidia.iso --unattend --start
//! # ...the OS installs itself, reboots into the desktop / gaming mode...
//! tendril-guest --finalize --start        # boot the installed station from disk (no media)
//! ```

use std::path::{Path, PathBuf};

use tendril_capability_engine::detect_with_groups;
use tendril_orchestrator::guest::{build_kickstart_seed, build_seed_iso};
use tendril_orchestrator::{
    provision, GuestOs, InstallMedia, KickstartSpec, Libvirt, StationRequest, UnattendSpec,
};
use tendril_provisioning::plan_for;

fn arg_value(flag: &str) -> Option<String> {
    let args: Vec<String> = std::env::args().collect();
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1).cloned())
}

fn has_flag(flag: &str) -> bool {
    std::env::args().any(|a| a == flag)
}

fn die(msg: impl std::fmt::Display) -> ! {
    eprintln!("{msg}");
    std::process::exit(1);
}

/// The seed ISO's output path: `--seed-iso` if given, else `<name>-seed.iso` next to the disk.
fn seed_path(name: &str, disk: &str) -> PathBuf {
    arg_value("--seed-iso")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            let dir = Path::new(disk).parent().unwrap_or_else(|| Path::new("."));
            dir.join(format!("{name}-seed.iso"))
        })
}

fn main() {
    let name = arg_value("--name").unwrap_or_else(|| "station1".to_string());
    let disk = arg_value("--disk").unwrap_or_else(|| format!("/var/lib/tendril/{name}.qcow2"));
    let size_gib: u32 = arg_value("--size-gib")
        .and_then(|s| s.parse().ok())
        .unwrap_or(128);
    let guest = if has_flag("--steamos") {
        GuestOs::SteamOs
    } else {
        GuestOs::Windows
    };
    let vcpus: u32 = arg_value("--vcpus")
        .and_then(|s| s.parse().ok())
        .unwrap_or(8);
    let memory_mib: u64 = arg_value("--memory-mib")
        .and_then(|s| s.parse().ok())
        .unwrap_or(16384);
    let finalize = has_flag("--finalize");
    let define = has_flag("--define") || has_flag("--start");
    let start = has_flag("--start");

    // Install media. Skipped entirely when finalizing (post-install boot from disk).
    let media = if finalize {
        InstallMedia::none()
    } else {
        let seed_iso = if has_flag("--unattend") {
            match guest {
                GuestOs::Windows => Some(build_unattend_seed(&name, &disk)),
                GuestOs::SteamOs => Some(build_kickstart_seed_file(&name, &disk)),
            }
        } else {
            None
        };
        InstallMedia {
            install_iso: arg_value("--iso"),
            virtio_iso: arg_value("--virtio-iso"),
            seed_iso,
        }
    };

    // GPU passthrough group (unless installing headless via VNC first).
    let passthrough_addresses = if has_flag("--no-gpu") {
        Vec::new()
    } else {
        resolve_passthrough_group()
    };

    // Hand the resolved request to the shared provisioning service.
    let req = StationRequest {
        name: name.clone(),
        guest,
        disk_path: disk.clone(),
        size_gib,
        create_disk: has_flag("--create-disk"),
        vcpus,
        memory_mib,
        native_hardware: has_flag("--native-hardware"),
        passthrough_addresses,
        mdev_uuid: None,
        media,
        usb_devices: Vec::new(),
        steam_library_dir: None,
        data_disk: None,
        cpu_pinning: None,
        hugepages: false,
        define,
        start,
    };
    let lv = Libvirt::system();
    let report = match provision(&req, &lv) {
        Ok(r) => r,
        Err(e) => die(format!("provision failed: {e}")),
    };
    if report.disk_created {
        eprintln!("created {size_gib} GiB disk at {disk}");
    }

    if !define {
        print!("{}", report.xml);
        eprintln!("(dry-run — pass --define to register, or --start to install/boot it)");
        return;
    }
    eprintln!(
        "defined domain '{name}'{}",
        if finalize {
            " (finalized: boots from disk, no install media)"
        } else {
            ""
        }
    );
    if report.started {
        eprintln!(
            "started '{name}' — connect a viewer to the VNC console to watch{}",
            if req.is_installing() {
                "; the OS will install itself unattended"
            } else {
                ""
            }
        );
        if req.needs_boot_prompt_clear() {
            eprintln!(
                "clearing the boot-from-CD prompt (~{}s)...",
                tendril_orchestrator::lifecycle::BOOT_PROMPT_TAPS
            );
            lv.clear_boot_prompt(&name);
        }
    }
}

/// Build the `autounattend.xml` seed ISO next to the disk, honoring account/locale overrides.
fn build_unattend_seed(name: &str, disk: &str) -> String {
    let mut spec = UnattendSpec {
        computer_name: name.to_uppercase(),
        ..UnattendSpec::default()
    };
    if let Some(v) = arg_value("--username") {
        spec.username = v;
    }
    if let Some(v) = arg_value("--password") {
        spec.password = v;
    }
    if let Some(v) = arg_value("--computer-name") {
        spec.computer_name = v;
    }
    if let Some(v) = arg_value("--locale") {
        spec.locale = v;
    }
    if let Some(v) = arg_value("--timezone") {
        spec.timezone = v;
    }
    if let Some(v) = arg_value("--edition") {
        spec.edition_name = v;
    }
    if has_flag("--no-autologon") {
        spec.autologon = false;
    }

    let seed = seed_path(name, disk);
    match build_seed_iso(&spec, &seed) {
        Ok(()) => {
            eprintln!("built unattended-setup seed ISO at {}", seed.display());
            seed.to_string_lossy().into_owned()
        }
        Err(e) => die(format!("build seed ISO failed: {e}")),
    }
}

/// Build the `ks.cfg` kickstart seed ISO (label `OEMDRV`) for a Bazzite station, honoring overrides.
fn build_kickstart_seed_file(name: &str, disk: &str) -> String {
    let mut spec = KickstartSpec {
        hostname: name.to_string(),
        ..KickstartSpec::default()
    };
    if let Some(v) = arg_value("--username") {
        spec.username = v;
    }
    if let Some(v) = arg_value("--password") {
        spec.password = v;
    }
    if let Some(v) = arg_value("--hostname").or_else(|| arg_value("--computer-name")) {
        spec.hostname = v;
    }
    if let Some(v) = arg_value("--locale") {
        spec.locale = v;
    }
    if let Some(v) = arg_value("--timezone") {
        spec.timezone = v;
    }
    if let Some(v) = arg_value("--image") {
        spec.image_ref = v;
    }
    if has_flag("--no-autologon") {
        spec.autologin = false;
    }
    if has_flag("--no-ssh") {
        spec.enable_ssh = false;
    }

    let seed = seed_path(name, disk);
    match build_kickstart_seed(&spec, &seed) {
        Ok(()) => {
            eprintln!("built kickstart seed ISO (OEMDRV) at {}", seed.display());
            seed.to_string_lossy().into_owned()
        }
        Err(e) => die(format!("build kickstart seed failed: {e}")),
    }
}

/// The first passthrough-capable GPU's whole IOMMU group, or exit if there is none.
fn resolve_passthrough_group() -> Vec<String> {
    let (matrix, groups) = detect_with_groups();
    let Some(cap) = matrix.passthrough_capable().next() else {
        die("No passthrough-capable GPU found. Pass --no-gpu to install headless via VNC first.");
    };
    plan_for(&cap.gpu, &groups).bind_addresses
}

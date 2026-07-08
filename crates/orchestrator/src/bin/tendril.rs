//! `tendril` — the interactive console for a Tendril host.
//!
//! A dependency-free, numbered-menu front-end over every Tendril function: inspect hardware, bind
//! GPUs for passthrough, create and manage gaming stations, and fetch install media. It calls the
//! same library services (`capability-engine`, `provisioning`, `orchestrator::provision`) a future
//! web UI will, so the two stay in lock-step. The Tendril OS launches this automatically on the
//! primary console.

use std::io::{self, Write};
use std::path::Path;
use std::process::Command;

use tendril_capability_engine::{detect, iommu, usb, GpuVendor, PassthroughViability};
use tendril_orchestrator::guest::{build_kickstart_seed, build_seed_iso};
use tendril_orchestrator::{
    provision, DomainState, GuestOs, InstallMedia, KickstartSpec, Libvirt, StationRequest,
    UnattendSpec,
};
use tendril_provisioning::{apply, Mode, PassthroughStrategy, ProvisioningStrategy};

const ISO_DIR: &str = "/var/lib/tendril/isos";
const DISK_DIR: &str = "/var/lib/tendril";

fn main() {
    loop {
        print_banner();
        println!("   1) Hardware & capabilities");
        println!("   2) GPU passthrough — bind a GPU to vfio-pci");
        println!("   3) Create a gaming station");
        println!("   4) Manage stations");
        println!("   5) Fetch install media");
        println!("   6) USB devices");
        println!("   7) Configure network");
        println!("   8) Set web admin password");
        println!("   9) Open Linux shell");
        println!("  10) Reboot");
        println!("  11) Shut down");
        println!("   0) Exit to login");
        match read_line("\nEnter an option: ").as_str() {
            "1" => menu_hardware(),
            "2" => menu_passthrough(),
            "3" => menu_create_station(),
            "4" => menu_manage(),
            "5" => menu_fetch_media(),
            "6" => menu_usb(),
            "7" => menu_network(),
            "8" => set_web_password(),
            "9" => drop_to_shell(),
            "10" => power("reboot"),
            "11" => power("poweroff"),
            "0" | "q" | "quit" | "exit" => return,
            "" => {}
            other => println!("Unknown option: {other}"),
        }
    }
}

/// TrueNAS-style console header: identity, address, and where the (planned) web UI will live.
fn print_banner() {
    println!(
        "\n\x1b[1m  T E N D R I L\x1b[0m  \x1b[2mv{}\x1b[0m",
        env!("CARGO_PKG_VERSION")
    );
    println!("  \x1b[2mGPU-passthrough gaming stations\x1b[0m\n");
    if let Some(h) = run_stdout("hostname", &[]) {
        if !h.trim().is_empty() {
            println!("  Hostname:  {}", h.trim());
        }
    }
    let ips = run_stdout("hostname", &["-I"]).unwrap_or_default();
    if let Some(ip) = ips.split_whitespace().next() {
        println!("  Address:   {ip}");
        println!("  \x1b[2mWeb UI:    http://{ip}\x1b[0m");
    }
    if !is_root() {
        println!("  \x1b[33m(not root — station / GPU / network / power actions need sudo)\x1b[0m");
    }
    println!("  ──────────────────────────────────────────────");
}

fn menu_network() {
    header("Network");
    if let Some(out) = run_stdout("ip", &["-brief", "-4", "addr"]) {
        print!("{out}");
    }
    println!("\n  1) Edit network (nmtui — interfaces, DNS, Wi-Fi)");
    println!("  2) Show routes & DNS");
    println!("  0) Back");
    match read_line("Select: ").as_str() {
        "1" => {
            if Command::new("nmtui").status().is_err() {
                println!("nmtui is not available; configure with `nmcli` from the shell.");
                pause();
            }
        }
        "2" => {
            if let Some(r) = run_stdout("ip", &["route"]) {
                println!("\nRoutes:\n{r}");
            }
            if let Ok(d) = std::fs::read_to_string("/etc/resolv.conf") {
                println!("DNS (/etc/resolv.conf):\n{d}");
            }
            pause();
        }
        _ => {}
    }
}

/// Reboot or power off the host (`systemctl reboot` / `poweroff`), with a typed confirmation.
fn power(action: &str) {
    let verb = if action == "reboot" {
        "Reboot"
    } else {
        "Shut down"
    };
    if !confirm_typed(&format!("{verb} this machine now?")) {
        return;
    }
    if let Err(e) = Command::new("systemctl").arg(action).status() {
        println!("\x1b[31m{verb} failed: {e}\x1b[0m");
        pause();
    }
}

// ── menus ───────────────────────────────────────────────────────────────────────────────────────

fn menu_hardware() {
    header("Hardware & capabilities");
    let matrix = detect();
    if matrix.gpus.is_empty() {
        println!("No display devices found.");
    }
    for (i, g) in matrix.gpus.iter().enumerate() {
        let model = g.gpu.model.as_deref().unwrap_or("GPU");
        println!(
            "  {}. {} {}  \x1b[2m[{}]\x1b[0m",
            i + 1,
            vendor(g.gpu.vendor),
            model,
            g.gpu.address
        );
        println!(
            "       capability: {:?}   passthrough: {}",
            g.capability,
            viability(g.viability)
        );
    }
    let ready = matrix.passthrough_capable().count();
    println!("\n{ready} GPU(s) ready for passthrough.");
    if ready == 0 && !matrix.gpus.is_empty() {
        println!("If you expected more, enable VT-d / AMD-Vi (IOMMU) in your BIOS/UEFI.");
    }
    pause();
}

fn menu_usb() {
    header("USB devices");
    let controllers = usb::controllers();
    println!("Host controllers ({}):", controllers.len());
    for c in &controllers {
        println!(
            "  [{}] {:04x}:{:04x}  passthrough: {}  (group devices: {})",
            c.address,
            c.vendor_id,
            c.device_id,
            viability(c.viability),
            c.iommu_group.len()
        );
    }
    let devices = usb::devices();
    println!("\nConnected devices ({}):", devices.len());
    for d in &devices {
        println!(
            "  {:04x}:{:04x}  {}",
            d.vendor_id,
            d.product_id,
            d.product.as_deref().unwrap_or("device")
        );
    }
    pause();
}

fn menu_passthrough() {
    header("GPU passthrough");
    let matrix = detect();
    let groups = iommu::read_groups();
    let capable: Vec<_> = matrix.passthrough_capable().collect();
    if capable.is_empty() {
        println!("No passthrough-capable GPU. Enable IOMMU in BIOS, then re-check Hardware.");
        return pause();
    }
    for (i, g) in capable.iter().enumerate() {
        println!(
            "  {}. {} {}  [{}]",
            i + 1,
            vendor(g.gpu.vendor),
            g.gpu.model.as_deref().unwrap_or("GPU"),
            g.gpu.address
        );
    }
    let Some(g) = pick("\nGPU to plan (0 to cancel): ", &capable) else {
        return;
    };
    let plan = PassthroughStrategy.plan(&g.gpu, iommu::group_of(&g.gpu.address, &groups));
    println!("\n{}", plan.summary);
    println!("Would bind these devices to \x1b[1m{}\x1b[0m:", plan.driver);
    for addr in &plan.bind_addresses {
        println!("  - {addr}");
    }
    if let Some(note) = &plan.note {
        println!("\x1b[33mNote: {note}\x1b[0m");
    }

    let actions = apply::render(&plan);
    println!("\n  1) Show the exact host changes (dry-run, changes nothing)");
    println!("  2) Bind now — detaches the GPU from the host");
    println!("  0) Cancel");
    match read_line("Select: ").as_str() {
        "1" => {
            println!();
            let _ = apply::execute(&actions, Mode::DryRun);
        }
        "2" => {
            if !is_root() {
                println!("\x1b[31mBinding needs root. Re-run tendril with sudo.\x1b[0m");
            } else if confirm_typed("This detaches the GPU from the host now.") {
                match apply::execute(&actions, Mode::Execute) {
                    Ok(()) => {
                        println!("\x1b[32mBound {} to {}.\x1b[0m", g.gpu.address, plan.driver)
                    }
                    Err(e) => println!("\x1b[31mbind failed: {e}\x1b[0m"),
                }
            }
        }
        _ => {}
    }
    pause();
}

fn menu_create_station() {
    header("Create a gaming station");
    let name = ask("Station name", "station1");
    let guest = match read_line("OS — 1) Windows  2) SteamOS (Bazzite)  [1]: ").as_str() {
        "2" => GuestOs::SteamOs,
        _ => GuestOs::Windows,
    };

    // GPU selection.
    let matrix = detect();
    let groups = iommu::read_groups();
    let capable: Vec<_> = matrix.passthrough_capable().collect();
    let mut passthrough = Vec::new();
    if capable.is_empty() {
        println!("(No passthrough-capable GPU found — the station will install headless via VNC.)");
    } else {
        println!("Assign a GPU:");
        for (i, g) in capable.iter().enumerate() {
            println!(
                "  {}. {} {} [{}]",
                i + 1,
                vendor(g.gpu.vendor),
                g.gpu.model.as_deref().unwrap_or("GPU"),
                g.gpu.address
            );
        }
        println!("  0. None (headless / attach later)");
        if let Some(g) = pick("GPU: ", &capable) {
            passthrough = PassthroughStrategy
                .plan(&g.gpu, iommu::group_of(&g.gpu.address, &groups))
                .bind_addresses;
        }
    }

    let disk = ask("Disk image path", &format!("{DISK_DIR}/{name}.qcow2"));
    let size_gib = ask("Disk size (GiB)", "128").parse().unwrap_or(128);
    let vcpus = ask("vCPUs", "8").parse().unwrap_or(8);
    let memory_mib = ask("Memory (MiB)", "16384").parse().unwrap_or(16384);

    // Install media.
    let default_iso = match guest {
        GuestOs::Windows => format!("{ISO_DIR}/win11.iso"),
        GuestOs::SteamOs => format!("{ISO_DIR}/bazzite-deck-nvidia.iso"),
    };
    let install_iso = some_if_nonempty(ask("Install ISO path", &default_iso));
    let virtio_iso = match guest {
        GuestOs::Windows => {
            some_if_nonempty(ask("virtio-win ISO", &format!("{ISO_DIR}/virtio-win.iso")))
        }
        GuestOs::SteamOs => None,
    };

    // Unattended seed.
    let seed_iso = if ask_yes("Install unattended (hands-off)?", true) {
        let username = ask("Username", "player");
        let password = ask("Password", "tendril");
        let seed_path = format!("{}/{name}-seed.iso", parent_of(&disk));
        let built = match guest {
            GuestOs::Windows => {
                let spec = UnattendSpec {
                    computer_name: ask("Computer name", &name.to_uppercase()),
                    username,
                    password,
                    ..UnattendSpec::default()
                };
                build_seed_iso(&spec, Path::new(&seed_path))
            }
            GuestOs::SteamOs => {
                let spec = KickstartSpec {
                    hostname: ask("Hostname", &name),
                    username,
                    password,
                    ..KickstartSpec::default()
                };
                build_kickstart_seed(&spec, Path::new(&seed_path))
            }
        };
        match built {
            Ok(()) => {
                println!("Built unattended seed at {seed_path}");
                Some(seed_path)
            }
            Err(e) => {
                println!("\x1b[31mCould not build the seed ISO: {e}\x1b[0m");
                None
            }
        }
    } else {
        None
    };

    let native_hardware = ask_yes(
        "Apply native-hardware overlay (anti-cheat; may violate game ToS)?",
        false,
    );
    let start = ask_yes("Start the station now (begins the install)?", true);

    let req = StationRequest {
        name: name.clone(),
        guest,
        disk_path: disk.clone(),
        size_gib,
        create_disk: !Path::new(&disk).exists(),
        vcpus,
        memory_mib,
        native_hardware,
        passthrough_addresses: passthrough,
        mdev_uuid: None,
        media: InstallMedia {
            install_iso,
            virtio_iso,
            seed_iso,
        },
        usb_devices: Vec::new(),
        steam_library_dir: None,
        data_disk: None,
        define: true,
        start,
    };

    println!("\nProvisioning '{name}'...");
    let lv = Libvirt::system();
    match provision(&req, &lv) {
        Ok(report) => {
            if report.disk_created {
                println!("  created {size_gib} GiB disk at {disk}");
            }
            println!("  defined domain '{name}'");
            if report.started {
                println!("  started — watch the console with: virsh domdisplay {name}");
                if req.needs_boot_prompt_clear() {
                    println!("  clearing the boot-from-CD prompt (~18s)...");
                    lv.clear_boot_prompt(&name);
                }
                println!(
                    "  the OS will install itself. When it reaches the desktop, run 'Manage \
                     stations' or `tendril-guest --finalize` so it boots from disk."
                );
            } else {
                println!("  not started. Start it later from 'Manage stations'.");
            }
        }
        Err(e) => println!("\x1b[31mprovision failed: {e}\x1b[0m"),
    }
    pause();
}

fn menu_manage() {
    header("Manage stations");
    let lv = Libvirt::system();
    let names = lv.list();
    if names.is_empty() {
        println!("No stations defined yet. Use 'Create a gaming station'.");
        return pause();
    }
    for (i, n) in names.iter().enumerate() {
        println!(
            "  {}. {}  \x1b[2m[{}]\x1b[0m",
            i + 1,
            n,
            state_label(lv.state(n))
        );
    }
    let choice = read_line("\nStation number (0 to go back): ");
    let Some(name) = choice
        .parse::<usize>()
        .ok()
        .filter(|n| *n >= 1 && *n <= names.len())
        .map(|n| names[n - 1].clone())
    else {
        return;
    };

    println!("\n{name} — {}", state_label(lv.state(&name)));
    println!("  1) Start");
    println!("  2) Shut down (graceful)");
    println!("  3) Force off");
    println!("  4) Delete (undefine + nvram)");
    println!("  0) Back");
    let result = match read_line("Select: ").as_str() {
        "1" => lv.start(&name),
        "2" => lv.shutdown(&name),
        "3" => lv.destroy(&name),
        "4" if confirm_typed(&format!("Delete station '{name}'?")) => lv.undefine(&name),
        _ => return,
    };
    match result {
        Ok(()) => println!("\x1b[32mdone.\x1b[0m"),
        Err(e) => println!("\x1b[31mfailed: {e}\x1b[0m"),
    }
    pause();
}

fn menu_fetch_media() {
    header("Fetch install media");
    println!("  1) Windows 11 + virtio-win");
    println!("  2) SteamOS (Bazzite)");
    println!("  0) Back");
    let (script, extra): (&str, Vec<String>) = match read_line("Select: ").as_str() {
        "1" => ("fetch-windows-media.sh", vec![]),
        "2" => (
            "fetch-steamos-media.sh",
            vec!["--variant".into(), ask("Bazzite variant", "deck-nvidia")],
        ),
        _ => return,
    };
    let Some(path) = locate_script(script) else {
        println!("Could not find {script}. Run it from the repo's scripts/ directory.");
        return pause();
    };
    let mut cmd = Command::new(&path);
    cmd.arg("--dest").arg(ISO_DIR).args(&extra);
    println!("Running {path} (downloads several GB)...\n");
    match cmd.status() {
        Ok(s) if s.success() => println!("\x1b[32mmedia ready in {ISO_DIR}\x1b[0m"),
        Ok(s) => println!("\x1b[31mfetch exited with status {s}\x1b[0m"),
        Err(e) => println!("\x1b[31mcould not run {path}: {e}\x1b[0m"),
    }
    pause();
}

/// Set the web control plane's admin password (delegates to `tendril-web --set-password`).
fn set_web_password() {
    header("Web admin password");
    match Command::new("tendril-web").arg("--set-password").status() {
        Ok(s) if s.success() => {}
        Ok(_) => println!("password not changed"),
        Err(e) => println!("tendril-web not available: {e}"),
    }
    pause();
}

fn drop_to_shell() {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string());
    println!("\nStarting a shell — type 'exit' to return to the Tendril console.");
    let _ = Command::new(shell).status();
}

/// Run a command and return its stdout on success (used for read-only host queries).
fn run_stdout(cmd: &str, args: &[&str]) -> Option<String> {
    Command::new(cmd)
        .args(args)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
}

// ── helpers ─────────────────────────────────────────────────────────────────────────────────────

fn read_line(prompt: &str) -> String {
    print!("{prompt}");
    let _ = io::stdout().flush();
    let mut s = String::new();
    // EOF (piped input exhausted / Ctrl-D) is treated as "quit" so the program always terminates.
    if io::stdin().read_line(&mut s).unwrap_or(0) == 0 {
        println!();
        std::process::exit(0);
    }
    s.trim().to_string()
}

fn ask(prompt: &str, default: &str) -> String {
    let v = read_line(&format!("{prompt} [{default}]: "));
    if v.is_empty() {
        default.to_string()
    } else {
        v
    }
}

fn ask_yes(prompt: &str, default_yes: bool) -> bool {
    let hint = if default_yes { "Y/n" } else { "y/N" };
    let v = read_line(&format!("{prompt} [{hint}]: ")).to_lowercase();
    if v.is_empty() {
        default_yes
    } else {
        v.starts_with('y')
    }
}

/// Require the user to type "yes" for a destructive action.
fn confirm_typed(what: &str) -> bool {
    read_line(&format!("{what} Type 'yes' to confirm: ")) == "yes"
}

fn pause() {
    let _ = read_line("\nPress Enter to continue...");
}

fn header(title: &str) {
    println!("\n\x1b[1m── {title} ──\x1b[0m");
}

/// Pick an item from `items` by 1-based number (0 or invalid → None).
fn pick<'a, T>(prompt: &str, items: &'a [T]) -> Option<&'a T> {
    read_line(prompt)
        .parse::<usize>()
        .ok()
        .filter(|n| *n >= 1 && *n <= items.len())
        .map(|n| &items[n - 1])
}

fn some_if_nonempty(s: String) -> Option<String> {
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

fn parent_of(path: &str) -> String {
    Path::new(path)
        .parent()
        .map(|p| p.to_string_lossy().into_owned())
        .filter(|p| !p.is_empty())
        .unwrap_or_else(|| ".".to_string())
}

fn vendor(v: GpuVendor) -> &'static str {
    match v {
        GpuVendor::Nvidia => "NVIDIA",
        GpuVendor::Amd => "AMD",
        GpuVendor::Intel => "Intel",
        GpuVendor::Unknown => "GPU",
    }
}

fn viability(v: PassthroughViability) -> &'static str {
    match v {
        PassthroughViability::Isolated => "isolated (clean)",
        PassthroughViability::SharedGroup => "shared IOMMU group (needs ACS override)",
        PassthroughViability::NoIommu => "no IOMMU (enable VT-d/AMD-Vi)",
    }
}

fn state_label(s: DomainState) -> &'static str {
    match s {
        DomainState::Running => "running",
        DomainState::Paused => "paused",
        DomainState::ShutOff => "shut off",
        DomainState::Absent => "absent",
        DomainState::Other => "other",
    }
}

/// True if this process's effective uid is 0 (read from /proc, no libc dependency).
fn is_root() -> bool {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("Uid:"))
                .and_then(|l| l.split_whitespace().nth(2).map(str::to_string))
        })
        .map(|euid| euid == "0")
        .unwrap_or(false)
}

/// Find a helper script in the installed location or the repo's scripts/ dir.
fn locate_script(name: &str) -> Option<String> {
    for base in ["/usr/libexec/tendril", "scripts", "./scripts"] {
        let p = format!("{base}/{name}");
        if Path::new(&p).exists() {
            return Some(p);
        }
    }
    None
}

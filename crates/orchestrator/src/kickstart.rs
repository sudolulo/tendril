//! Generate an Anaconda **kickstart** so a SteamOS-style gaming station installs itself, hands-off.
//!
//! Valve's SteamOS has no generic-PC installer as of 2026 — the only official media is the Steam Deck
//! *recovery image*, which is image-based (not scriptable) and **AMD-only**, so it can't drive an
//! NVIDIA passthrough station. Until Valve ships a generic ISO, Tendril's "SteamOS" station is
//! [Bazzite](https://bazzite.gg): an atomic, Steam-gaming-mode image with an Anaconda ISO — which
//! *is* scriptable, the Linux parallel to Windows' `autounattend.xml`.
//!
//! Anaconda auto-loads a kickstart named `ks.cfg` from a volume labelled `OEMDRV`; Tendril writes one
//! onto a seed ISO ([`crate::guest::build_kickstart_seed`]) and attaches it alongside the installer.

use std::fmt::Write as _;

/// The default OS payload — the Bazzite Steam-Deck NVIDIA image embedded on its installer ISO, at
/// Anaconda's install-source mount. Installs offline (no registry pull); matches the ISO that
/// `fetch-steamos-media.sh` grabs by default. Pass a registry ref (e.g.
/// `ghcr.io/ublue-os/bazzite-deck-nvidia:stable`) via `--image` for a different variant.
pub const DEFAULT_IMAGE_REF: &str = "/run/install/repo/bazzite-deck-nvidia-stable";

/// Inputs for the generated kickstart. Defaults suit a single local gaming station.
#[derive(Debug, Clone)]
pub struct KickstartSpec {
    /// Host name.
    pub hostname: String,
    /// Local user created during install (added to `wheel` for sudo).
    pub username: String,
    /// That user's password, in plaintext (a throwaway seed ISO on a local station).
    pub password: String,
    /// Time-zone id, e.g. `UTC`.
    pub timezone: String,
    /// glibc locale, e.g. `en_US.UTF-8`.
    pub locale: String,
    /// Console/X keyboard layout, e.g. `us`.
    pub keyboard: String,
    /// bootc/ostree container image to install (the ISO must be able to reach it).
    pub image_ref: String,
    /// Auto-login the user straight into Steam gaming mode (a station has no login keyboard).
    pub autologin: bool,
    /// Enable SSH on the installed station (headless management).
    pub enable_ssh: bool,
    /// Filename (on the OEMDRV seed disc) of an NVIDIA vGPU **guest** `.run` to install on first boot.
    /// `None` for whole-GPU-passthrough or non-NVIDIA stations. Set only for a station on an NVIDIA vGPU
    /// (mdev) slice.
    pub vgpu_guest_run: Option<String>,
    /// URL of the FastAPI-DLS client-config token to fetch on first boot (un-throttles the vGPU). Only
    /// meaningful alongside [`Self::vgpu_guest_run`].
    pub dls_token_url: Option<String>,
    /// Pre-enable the [Sunshine](https://github.com/LizardByte/Sunshine) game-stream host (Moonlight
    /// server). Governed by Tendril's station toggle — the parallel of installing Sunshine on a Windows
    /// station. Idle-free (Sunshine only encodes when a client connects, on the GPU's NVENC block), so
    /// it's safe to leave on. Best-effort on atomic Bazzite; unvalidated on real hardware.
    pub enable_sunshine: bool,
}

impl Default for KickstartSpec {
    fn default() -> Self {
        Self {
            hostname: "tendril".to_string(),
            username: "player".to_string(),
            password: "tendril".to_string(),
            timezone: "UTC".to_string(),
            locale: "en_US.UTF-8".to_string(),
            keyboard: "us".to_string(),
            image_ref: DEFAULT_IMAGE_REF.to_string(),
            autologin: true,
            enable_ssh: true,
            vgpu_guest_run: None,
            dls_token_url: None,
            enable_sunshine: false,
        }
    }
}

/// Bazzite's Steam gaming-mode (gamescope) session — what an autologin station should boot into.
const GAMESCOPE_SESSION: &str = "gamescope-session.desktop";

/// Render `spec` into an Anaconda kickstart.
pub fn render_kickstart(spec: &KickstartSpec) -> String {
    let mut ks = String::new();
    let _ = writeln!(
        ks,
        "# Tendril unattended install for a SteamOS-style (Bazzite) gaming station.\n\
         # Anaconda auto-loads this as ks.cfg from a volume labelled OEMDRV."
    );
    // Fully non-interactive: fail rather than stop at a prompt with nobody there to answer.
    let _ = writeln!(ks, "text --non-interactive");
    let _ = writeln!(ks, "lang {}", spec.locale);
    let _ = writeln!(ks, "keyboard {}", spec.keyboard);
    let _ = writeln!(ks, "timezone {} --utc", spec.timezone);
    let _ = writeln!(
        ks,
        "network --bootproto=dhcp --activate --hostname={}",
        spec.hostname
    );

    // Wipe the station disk and take Anaconda's atomic (btrfs) default layout.
    let _ = writeln!(ks, "zerombr");
    let _ = writeln!(ks, "clearpart --all --initlabel --disklabel=gpt");
    let _ = writeln!(ks, "autopart --type=btrfs --noswap");

    // OS payload. A path (e.g. the image embedded on the installer ISO at
    // /run/install/repo/<name>) installs offline; anything else is a registry ref pulled over the
    // installer's NAT network.
    let transport = if spec.image_ref.starts_with('/') {
        "oci"
    } else {
        "registry"
    };
    let _ = writeln!(
        ks,
        "ostreecontainer --url={} --transport={transport} --no-signature-verification",
        spec.image_ref
    );

    // Accounts: locked root, a sudo-capable local user.
    let _ = writeln!(ks, "rootpw --lock");
    let _ = writeln!(
        ks,
        "user --name={} --password={} --plaintext --groups=wheel",
        spec.username, spec.password
    );
    if spec.enable_ssh {
        let _ = writeln!(ks, "services --enabled=sshd");
        let _ = writeln!(ks, "firewall --enabled --service=ssh");
    }
    let _ = writeln!(ks, "reboot --eject");

    if spec.autologin {
        // Auto-login into Steam gaming mode via SDDM.
        let _ = write!(
            ks,
            "\n%post --erroronfail\n\
             mkdir -p /etc/sddm.conf.d\n\
             cat >/etc/sddm.conf.d/zz-tendril-autologin.conf <<'EOF'\n\
             [Autologin]\n\
             User={user}\n\
             Session={session}\n\
             Relogin=true\n\
             EOF\n\
             %end\n",
            user = spec.username,
            session = GAMESCOPE_SESSION,
        );
    }

    if let Some(run) = &spec.vgpu_guest_run {
        // Install the NVIDIA vGPU guest driver on first boot via a self-disabling oneshot.
        //
        // NOTE: Bazzite is an atomic/ostree image with a read-only /usr, so a runtime `.run` install is
        // **best-effort scaffolding** — the durable path is layering the driver into the image. This is
        // unvalidated on real hardware; it copies the driver off the OEMDRV seed, then a first-boot unit
        // runs it (and, if a DLS token URL is set, fetches the license token so the vGPU un-throttles).
        // Non-fatal (plain `%post`, `|| true`) so a failure never aborts the OS install.
        let token_exec = if let Some(token) = &spec.dls_token_url {
            format!(
                "ExecStartPost=/bin/sh -c 'mkdir -p /etc/nvidia/ClientConfigToken && \
                 curl --insecure -L \"{token}\" -o /etc/nvidia/ClientConfigToken/client_configuration_token.tok && \
                 sed -i \"s/^#*FeatureType=.*/FeatureType=1/\" /etc/nvidia/gridd.conf && \
                 systemctl restart nvidia-gridd'\n"
            )
        } else {
            String::new()
        };
        let _ = write!(
            ks,
            r#"
%post
mkdir -p /var/lib/tendril /run/tendril-seed
mount /dev/disk/by-label/OEMDRV /run/tendril-seed 2>/dev/null || true
cp /run/tendril-seed/{run} /var/lib/tendril/nvidia-vgpu-guest.run 2>/dev/null || true
umount /run/tendril-seed 2>/dev/null || true
chmod +x /var/lib/tendril/nvidia-vgpu-guest.run 2>/dev/null || true
cat >/etc/systemd/system/tendril-vgpu-guest.service <<'EOF'
[Unit]
Description=Tendril: install NVIDIA vGPU guest driver (first boot)
After=network-online.target
Wants=network-online.target
ConditionPathExists=/var/lib/tendril/nvidia-vgpu-guest.run
[Service]
Type=oneshot
RemainAfterExit=yes
ExecStart=/var/lib/tendril/nvidia-vgpu-guest.run --silent --dkms
{token_exec}ExecStartPost=/bin/systemctl disable tendril-vgpu-guest.service
[Install]
WantedBy=multi-user.target
EOF
systemctl enable tendril-vgpu-guest.service
%end
"#,
        );
    }

    if spec.enable_sunshine {
        // Pre-enable the Sunshine game-stream host. Zero idle cost — it only captures + NVENC-encodes
        // when a Moonlight client connects, on the GPU's dedicated encoder, so game framerate is
        // unaffected. Bazzite is streaming-focused and usually ships Sunshine as a user unit; if it's
        // absent, a first-boot service installs the Flatpak and wires an autostart for the station user.
        //
        // Best-effort/unvalidated on real hardware and atomic-image-specific: /usr is read-only, so the
        // helper and config land in /etc + the user's home (both writable), and capture needs uinput +
        // (for KMS grab) elevated caps that vary by Bazzite build.
        let _ = write!(
            ks,
            r##"
%post
# uinput access for Sunshine's virtual gamepad/keyboard, and the session user in the input group.
cat >/etc/udev/rules.d/85-tendril-sunshine.rules <<'EOF'
KERNEL=="uinput", SUBSYSTEM=="misc", MODE="0660", GROUP="input", OPTIONS+="static_node=uinput"
EOF
usermod -aG input {user} 2>/dev/null || true
# A user service needs a lingering systemd instance to run headless before the user logs in.
mkdir -p /var/lib/systemd/linger && touch /var/lib/systemd/linger/{user}
# If Bazzite already ships a Sunshine user unit, enable it for all users.
systemctl --global enable sunshine.service 2>/dev/null || true
# Fallback: first boot, if there's no sunshine, install the Flatpak and autostart it for the user.
mkdir -p /etc/tendril
cat >/etc/tendril/sunshine-setup.sh <<'EOF'
#!/bin/sh
command -v sunshine >/dev/null 2>&1 && exit 0
flatpak remote-add --if-not-exists flathub https://flathub.org/repo/flathub.flatpakrepo || true
flatpak install -y --noninteractive flathub dev.lizardbyte.app.Sunshine || exit 0
U=$(getent passwd {user} | cut -d: -f6)
install -d -o {user} -g {user} "$U/.config/systemd/user/default.target.wants"
cat >"$U/.config/systemd/user/sunshine.service" <<'UNIT'
[Unit]
Description=Sunshine game-stream host
[Service]
ExecStart=/usr/bin/flatpak run dev.lizardbyte.app.Sunshine
Restart=on-failure
[Install]
WantedBy=default.target
UNIT
ln -sf ../sunshine.service "$U/.config/systemd/user/default.target.wants/sunshine.service"
chown -R {user}:{user} "$U/.config/systemd"
EOF
chmod +x /etc/tendril/sunshine-setup.sh
cat >/etc/systemd/system/tendril-sunshine-setup.service <<'EOF'
[Unit]
Description=Tendril: ensure Sunshine is installed + autostarts (first boot)
After=network-online.target
Wants=network-online.target
[Service]
Type=oneshot
RemainAfterExit=yes
ExecStart=/etc/tendril/sunshine-setup.sh
ExecStartPost=/bin/systemctl disable tendril-sunshine-setup.service
[Install]
WantedBy=multi-user.target
EOF
systemctl enable tendril-sunshine-setup.service
%end
"##,
            user = spec.username,
        );
    }
    ks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_interactive_and_wipes_disk() {
        let ks = render_kickstart(&KickstartSpec::default());
        assert!(ks.contains("text --non-interactive"));
        assert!(ks.contains("clearpart --all"));
        assert!(ks.contains("autopart --type=btrfs"));
        assert!(ks.contains("reboot"));
    }

    #[test]
    fn installs_the_bazzite_image_payload() {
        let ks = render_kickstart(&KickstartSpec::default());
        assert!(ks.contains("ostreecontainer"));
        assert!(ks.contains("bazzite-deck-nvidia"));
    }

    #[test]
    fn transport_matches_ref_kind() {
        // A path installs offline from the ISO's embedded image...
        let offline = render_kickstart(&KickstartSpec {
            image_ref: "/run/install/repo/bazzite-deck-nvidia-stable".to_string(),
            ..Default::default()
        });
        assert!(offline.contains("--transport=oci"));
        // ...a registry ref pulls over the network.
        let online = render_kickstart(&KickstartSpec {
            image_ref: "ghcr.io/ublue-os/bazzite-deck:stable".to_string(),
            ..Default::default()
        });
        assert!(online.contains("--transport=registry"));
    }

    #[test]
    fn creates_sudo_user_and_ssh() {
        let ks = render_kickstart(&KickstartSpec {
            username: "gamer".to_string(),
            password: "pw123".to_string(),
            ..Default::default()
        });
        assert!(ks.contains("user --name=gamer --password=pw123 --plaintext --groups=wheel"));
        assert!(ks.contains("rootpw --lock"));
        assert!(ks.contains("services --enabled=sshd"));
    }

    #[test]
    fn autologin_boots_to_gaming_mode() {
        let on = render_kickstart(&KickstartSpec::default());
        assert!(on.contains("Autologin"));
        assert!(on.contains("gamescope-session"));
        let off = render_kickstart(&KickstartSpec {
            autologin: false,
            ..Default::default()
        });
        assert!(!off.contains("Autologin"));
    }

    #[test]
    fn ssh_can_be_disabled() {
        let ks = render_kickstart(&KickstartSpec {
            enable_ssh: false,
            ..Default::default()
        });
        assert!(!ks.contains("sshd"));
    }

    #[test]
    fn no_vgpu_post_by_default() {
        let ks = render_kickstart(&KickstartSpec::default());
        assert!(!ks.contains("tendril-vgpu-guest.service"));
    }

    #[test]
    fn installs_vgpu_guest_run_on_first_boot() {
        let ks = render_kickstart(&KickstartSpec {
            vgpu_guest_run: Some("nvidia-vgpu-guest.run".to_string()),
            ..Default::default()
        });
        // Copies the driver off the OEMDRV seed and enables a first-boot install service.
        assert!(ks.contains("mount /dev/disk/by-label/OEMDRV"));
        assert!(ks.contains("cp /run/tendril-seed/nvidia-vgpu-guest.run"));
        assert!(ks.contains("ExecStart=/var/lib/tendril/nvidia-vgpu-guest.run --silent --dkms"));
        assert!(ks.contains("systemctl enable tendril-vgpu-guest.service"));
        // Self-disabling oneshot.
        assert!(ks.contains("systemctl disable tendril-vgpu-guest.service"));
        // No token step unless a URL is given.
        assert!(!ks.contains("ClientConfigToken"));
    }

    #[test]
    fn fetches_dls_token_when_url_set() {
        let ks = render_kickstart(&KickstartSpec {
            vgpu_guest_run: Some("nvidia-vgpu-guest.run".to_string()),
            dls_token_url: Some("https://10.0.0.2:8443/-/client-token".to_string()),
            ..Default::default()
        });
        assert!(ks.contains("/etc/nvidia/ClientConfigToken/client_configuration_token.tok"));
        assert!(ks.contains("https://10.0.0.2:8443/-/client-token"));
        assert!(ks.contains("FeatureType=1"));
        assert!(ks.contains("systemctl restart nvidia-gridd"));
    }

    #[test]
    fn token_without_run_does_nothing() {
        // The token rides the same %post as the driver; no driver → no vGPU %post at all.
        let ks = render_kickstart(&KickstartSpec {
            dls_token_url: Some("https://h/-/client-token".to_string()),
            ..Default::default()
        });
        assert!(!ks.contains("ClientConfigToken"));
        assert!(!ks.contains("tendril-vgpu-guest.service"));
    }

    #[test]
    fn no_sunshine_by_default() {
        let ks = render_kickstart(&KickstartSpec::default());
        assert!(!ks.contains("sunshine"));
    }

    #[test]
    fn enables_sunshine_when_toggled() {
        let ks = render_kickstart(&KickstartSpec {
            username: "gamer".to_string(),
            enable_sunshine: true,
            ..Default::default()
        });
        // uinput rule + input group for the actual station user.
        assert!(ks.contains("KERNEL==\"uinput\""));
        assert!(ks.contains("usermod -aG input gamer"));
        // Enable an existing unit, with a Flatpak-install fallback wired as a first-boot service.
        assert!(ks.contains("systemctl --global enable sunshine.service"));
        assert!(ks.contains("dev.lizardbyte.app.Sunshine"));
        assert!(ks.contains("systemctl enable tendril-sunshine-setup.service"));
        // The fallback dir is created before it's written to.
        assert!(
            ks.find("mkdir -p /etc/tendril").unwrap()
                < ks.find("cat >/etc/tendril/sunshine-setup.sh").unwrap()
        );
    }
}

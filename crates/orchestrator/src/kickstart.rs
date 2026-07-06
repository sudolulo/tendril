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
}

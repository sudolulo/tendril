//! Generate a Windows `autounattend.xml` so a station installs itself, hands-off.
//!
//! A stock Windows 11 ISO can't be installed unattended in a VM without two things: the **virtio
//! storage driver** (otherwise Setup shows "we couldn't find any drives" — the virtio disk is
//! invisible), and a way past the **OOBE / Microsoft-account wall**. This module emits an answer file
//! that does both, plus auto-partitions the disk, creates a local user, and installs the virtio guest
//! tools (QEMU guest agent, balloon, network) on first logon.
//!
//! The file is delivered on a small "seed" ISO ([`crate::guest::build_seed_iso`]); Windows Setup
//! reads `autounattend.xml` from the root of any attached removable media during WinPE.

use std::fmt::Write as _;

use crate::station::GuestOs;

/// An application to fetch-and-silent-install on the station's first logon.
///
/// Unlike the NVIDIA vGPU guest driver (licensed → the admin stages it and it's baked onto the seed
/// disc), these are free/redistributable, so they're pulled straight from their official download URLs
/// at first boot — nothing to stage, and the seed ISO stays small. Requires guest network access on
/// first boot (Windows stations get NAT by default).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuestApp {
    /// Valve's Steam client.
    Steam,
    /// [Sunshine](https://github.com/LizardByte/Sunshine) — the self-hosted GameStream host that makes
    /// a seatless station playable over a Moonlight client. The highest-value add for a headless VM.
    Sunshine,
    /// Discord.
    Discord,
    /// [Moonlight](https://moonlight-stream.org) — the GameStream **client**, so a station can also
    /// *receive* a stream (e.g. play another station's games on this one). Installed via winget, since
    /// its GitHub release asset is versioned (no stable direct-download URL like Sunshine's).
    Moonlight,
}

impl GuestApp {
    /// Human label for the answer-file command description.
    fn label(self) -> &'static str {
        match self {
            GuestApp::Steam => "Steam",
            GuestApp::Sunshine => "Sunshine (game streaming host)",
            GuestApp::Discord => "Discord",
            GuestApp::Moonlight => "Moonlight (game-stream client)",
        }
    }
    /// The first-logon CommandLine that installs this app, already XML-escaped for the answer file.
    fn command(self) -> String {
        // URL apps: fetch the official installer to %TEMP% and run it silently. `&amp;` is cmd's
        // separator (XML-escaped); the URL is XML-escaped too (Discord's has a raw `&` in its query).
        let url_app = |url: &str, exe: &str, args: &str| {
            format!(
                "cmd /c curl.exe -fL \"{url}\" -o \"%TEMP%\\{exe}\" &amp; start /wait \"\" \"%TEMP%\\{exe}\" {args}",
                url = xml_escape(url)
            )
        };
        match self {
            GuestApp::Steam => url_app(
                "https://cdn.cloudflare.steamstatic.com/client/installer/SteamSetup.exe",
                "SteamSetup.exe",
                "/S",
            ),
            GuestApp::Sunshine => url_app(
                "https://github.com/LizardByte/Sunshine/releases/latest/download/Sunshine-Windows-AMD64-installer.exe",
                "SunshineSetup.exe",
                "/S",
            ),
            GuestApp::Discord => url_app(
                "https://discord.com/api/downloads/distributions/app/installers/latest?channel=stable&platform=win&arch=x64",
                "DiscordSetup.exe",
                "-s",
            ),
            // Moonlight's release asset is versioned (no stable direct URL), so use winget — present on
            // Windows 11. Accept the agreements so it runs non-interactively.
            GuestApp::Moonlight => "winget install --id MoonlightGameStreamingProject.Moonlight -e --silent --accept-package-agreements --accept-source-agreements".to_string(),
        }
    }
}

/// Inputs for the generated answer file. Defaults suit a single local gaming station.
#[derive(Debug, Clone)]
pub struct UnattendSpec {
    /// Windows computer name.
    pub computer_name: String,
    /// Local administrator account created during OOBE.
    pub username: String,
    /// That account's password. Stored in plaintext in the answer file (a throwaway seed ISO on a
    /// local station); keep it free of XML metacharacters (`& < > " '`).
    pub password: String,
    /// BCP-47 locale, e.g. `en-US`.
    pub locale: String,
    /// Windows time-zone id, e.g. `UTC` or `Pacific Standard Time`.
    pub timezone: String,
    /// Edition to install, matched against the ISO's image list (`/IMAGE/NAME`).
    pub edition_name: String,
    /// Log the user in automatically at boot (a gaming station has no keyboard at the login screen
    /// until the guest is up).
    pub autologon: bool,
    /// Filename (on the attached seed disc) of an NVIDIA vGPU **guest** driver installer to run on
    /// first logon. `None` for a whole-GPU-passthrough or non-NVIDIA station — those get their driver
    /// from Windows Update / the vendor, not a vGPU GRID guest package. Set only for a station bound to
    /// an NVIDIA vGPU (mdev) slice, which needs the matching GRID guest driver to use the vGPU at all.
    pub vgpu_driver_exe: Option<String>,
    /// URL of the FastAPI-DLS client-config token to fetch on first logon (removes the vGPU licensing
    /// throttle). `None` when guest licensing isn't running. Only meaningful alongside
    /// [`Self::vgpu_driver_exe`] — the token is inert without the guest driver.
    pub dls_token_url: Option<String>,
    /// Applications to fetch-and-silent-install on first logon (Steam, Sunshine, Discord). Empty for a
    /// bare station.
    pub apps: Vec<GuestApp>,
    /// The station has a persistent data volume (a second disk) — initialize + format it and assign a
    /// drive letter on first logon, so games/saves can live off the OS disk and survive reinstalls,
    /// base-image swaps, and re-splits.
    pub data_volume: bool,
}

impl Default for UnattendSpec {
    fn default() -> Self {
        Self {
            computer_name: "TENDRIL".to_string(),
            username: "player".to_string(),
            password: "tendril".to_string(),
            locale: "en-US".to_string(),
            timezone: "UTC".to_string(),
            edition_name: "Windows 11 Pro".to_string(),
            autologon: true,
            vgpu_driver_exe: None,
            dls_token_url: None,
            apps: Vec::new(),
            data_volume: false,
        }
    }
}

impl UnattendSpec {
    /// SteamOS/Linux guests don't use an answer file; this is Windows-only. Provided for symmetry so
    /// callers can branch on [`GuestOs`].
    pub fn for_guest(guest: GuestOs) -> Option<Self> {
        match guest {
            GuestOs::Windows => Some(Self::default()),
            GuestOs::SteamOs => None,
        }
    }
}

/// virtio-win driver folders needed while installing (storage so the disk is visible, plus network,
/// serial for the guest agent, and balloon), injected into the offline image so they survive reboot.
const DRIVER_DIRS: &[&str] = &["viostor", "vioscsi", "NetKVM", "vioserial", "Balloon"];
/// The virtio ISO's drive letter isn't deterministic in WinPE; list the likely ones. Paths that
/// don't resolve are skipped by Setup, so over-listing is harmless.
const DRIVER_DRIVES: &[char] = &['d', 'e', 'f'];

/// Escape `&`/`<`/`>` for text embedded in the XML answer file — notably URLs with query strings
/// (e.g. Discord's `?channel=stable&platform=win`), whose raw `&` would otherwise be invalid XML. The
/// answer-file parser turns these back into literal characters, and the URLs are double-quoted on the
/// command line so cmd doesn't treat a decoded `&` as a separator.
fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// One `<SynchronousCommand>` entry for the `oobeSystem` `FirstLogonCommands` block. Commands run in
/// `order` sequence after auto-logon; a failing one doesn't abort the rest.
fn logon_cmd(order: u32, desc: &str, line: &str) -> String {
    format!(
        "\n        <SynchronousCommand wcm:action=\"add\">\
         \n          <Order>{order}</Order>\
         \n          <Description>{desc}</Description>\
         \n          <CommandLine>{line}</CommandLine>\
         \n        </SynchronousCommand>"
    )
}

/// Render a Windows 11 `autounattend.xml` for `spec`.
pub fn render_autounattend(spec: &UnattendSpec) -> String {
    let mut driver_paths = String::new();
    for drive in DRIVER_DRIVES {
        for dir in DRIVER_DIRS {
            let _ = write!(
                driver_paths,
                "\n          <PathAndCredentials wcm:action=\"add\" wcm:keyValue=\"{drive}{dir}\">\
                 \n            <Path>{drive}:\\{dir}\\w11\\amd64</Path>\
                 \n          </PathAndCredentials>",
            );
        }
    }

    let autologon = if spec.autologon {
        format!(
            "\n      <AutoLogon>\
             \n        <Enabled>true</Enabled>\
             \n        <Username>{user}</Username>\
             \n        <LogonCount>2147483647</LogonCount>\
             \n        <Password><Value>{pass}</Value><PlainText>true</PlainText></Password>\
             \n      </AutoLogon>",
            user = xml_escape(&spec.username),
            pass = xml_escape(&spec.password),
        )
    } else {
        String::new()
    };

    // First-logon commands, in Order sequence. Order 1 is always the virtio guest tools; then (for an
    // NVIDIA vGPU station) the GRID guest driver, the DLS licensing token, and finally any apps.
    let mut first_logon = logon_cmd(
        1,
        "Install virtio guest tools (QEMU guest agent, balloon, drivers)",
        r"cmd /c for %d in (D E F G) do @if exist %d:\virtio-win-guest-tools.exe start /wait %d:\virtio-win-guest-tools.exe /install /passive /norestart",
    );
    let mut order = 2;
    // Bring up the virtio-fs service so a shared Steam library (if the station attaches one) mounts as
    // a drive. Harmless when there's no share — the service just starts. The user then adds it in Steam
    // (Settings → Storage → Add Drive). See docs/STEAM-GAMES.md.
    first_logon += &logon_cmd(
        order,
        "Start virtio-fs service (shared Steam library, if attached)",
        // `&` is XML-escaped — the answer file is parsed as XML (see the ampersand test).
        r#"cmd /c sc config VirtioFsSvc start= auto &amp; net start VirtioFsSvc"#,
    );
    order += 1;
    if spec.data_volume {
        // Initialize the persistent data volume (the second disk, which starts RAW) and give it a drive
        // letter, so games/saves can live off the OS disk. The RAW filter targets only the uninitialized
        // data disk — never the installed Windows boot disk. `& < >` would need XML-escaping; this has none.
        first_logon += &logon_cmd(
            order,
            "Initialize persistent data volume",
            r#"powershell -NoProfile -Command "Get-Disk | Where-Object PartitionStyle -Eq RAW | Initialize-Disk -PartitionStyle GPT -PassThru | New-Partition -AssignDriveLetter -UseMaximumSize | Format-Volume -FileSystem NTFS -NewFileSystemLabel TendrilData -Confirm:$false""#,
        );
        order += 1;
    }
    if let Some(exe) = &spec.vgpu_driver_exe {
        // NVIDIA's DCH installer supports a silent, no-reboot install; the vGPU binds after the
        // station's next boot. The seed disc's drive letter isn't deterministic, so probe the likely ones.
        first_logon += &logon_cmd(
            order,
            "Install NVIDIA vGPU guest driver",
            &format!(
                r"cmd /c for %d in (D E F G) do @if exist %d:\{exe} start /wait %d:\{exe} -s -noreboot"
            ),
        );
        order += 1;
    }
    if let Some(url) = &spec.dls_token_url {
        // `&` is cmd's command separator — XML-escaped here since the answer file is parsed as XML.
        // The driver reads this token at boot; no service restart needed (the vGPU driver install
        // above already requires a reboot to bind).
        let dir = r"C:\Program Files\NVIDIA Corporation\vGPU Licensing\ClientConfigToken";
        first_logon += &logon_cmd(
            order,
            "Fetch FastAPI-DLS vGPU licensing token",
            &format!(
                "cmd /c mkdir \"{dir}\" &amp; curl.exe --insecure -L \"{url}\" -o \"{dir}\\client_config_token.tok\"",
                url = xml_escape(url),
            ),
        );
        order += 1;
    }
    for app in &spec.apps {
        // Fetch the official installer to %TEMP% and run it silently. The `&amp;` is the cmd separator;
        // the URL is XML-escaped separately (Discord's has raw `&` in its query string).
        first_logon += &logon_cmd(order, &format!("Install {}", app.label()), &app.command());
        order += 1;
    }

    format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<unattend xmlns="urn:schemas-microsoft-com:unattend" xmlns:wcm="http://schemas.microsoft.com/WMIConfig/2002/State">

  <settings pass="windowsPE">
    <component name="Microsoft-Windows-International-Core-WinPE" processorArchitecture="amd64" publicKeyToken="31bf3856ad364e35" language="neutral" versionScope="nonSxS">
      <SetupUILanguage><UILanguage>{locale}</UILanguage></SetupUILanguage>
      <InputLocale>{locale}</InputLocale>
      <SystemLocale>{locale}</SystemLocale>
      <UILanguage>{locale}</UILanguage>
      <UserLocale>{locale}</UserLocale>
    </component>
    <component name="Microsoft-Windows-PnpCustomizationsWinPE" processorArchitecture="amd64" publicKeyToken="31bf3856ad364e35" language="neutral" versionScope="nonSxS">
      <DriverPaths>{driver_paths}
      </DriverPaths>
    </component>
    <component name="Microsoft-Windows-Setup" processorArchitecture="amd64" publicKeyToken="31bf3856ad364e35" language="neutral" versionScope="nonSxS">
      <DiskConfiguration>
        <Disk wcm:action="add">
          <DiskID>0</DiskID>
          <WillWipeDisk>true</WillWipeDisk>
          <CreatePartitions>
            <CreatePartition wcm:action="add"><Order>1</Order><Type>EFI</Type><Size>260</Size></CreatePartition>
            <CreatePartition wcm:action="add"><Order>2</Order><Type>MSR</Type><Size>128</Size></CreatePartition>
            <CreatePartition wcm:action="add"><Order>3</Order><Type>Primary</Type><Extend>true</Extend></CreatePartition>
          </CreatePartitions>
          <ModifyPartitions>
            <ModifyPartition wcm:action="add"><Order>1</Order><PartitionID>1</PartitionID><Format>FAT32</Format><Label>System</Label></ModifyPartition>
            <ModifyPartition wcm:action="add"><Order>2</Order><PartitionID>2</PartitionID></ModifyPartition>
            <ModifyPartition wcm:action="add"><Order>3</Order><PartitionID>3</PartitionID><Format>NTFS</Format><Label>Windows</Label><Letter>C</Letter></ModifyPartition>
          </ModifyPartitions>
        </Disk>
      </DiskConfiguration>
      <ImageInstall>
        <OSImage>
          <InstallFrom>
            <MetaData wcm:action="add"><Key>/IMAGE/NAME</Key><Value>{edition}</Value></MetaData>
          </InstallFrom>
          <InstallTo><DiskID>0</DiskID><PartitionID>3</PartitionID></InstallTo>
        </OSImage>
      </ImageInstall>
      <UserData>
        <AcceptEula>true</AcceptEula>
        <ProductKey><Key>VK7JG-NPHTM-C97JM-9MPGT-3V66T</Key></ProductKey>
      </UserData>
    </component>
  </settings>

  <settings pass="specialize">
    <component name="Microsoft-Windows-Shell-Setup" processorArchitecture="amd64" publicKeyToken="31bf3856ad364e35" language="neutral" versionScope="nonSxS">
      <ComputerName>{computer}</ComputerName>
      <TimeZone>{timezone}</TimeZone>
    </component>
    <component name="Microsoft-Windows-TerminalServices-LocalSessionManager" processorArchitecture="amd64" publicKeyToken="31bf3856ad364e35" language="neutral" versionScope="nonSxS">
      <fDenyTSConnections>false</fDenyTSConnections>
    </component>
    <component name="Networking-MPSSVC-Svc" processorArchitecture="amd64" publicKeyToken="31bf3856ad364e35" language="neutral" versionScope="nonSxS">
      <FirewallGroups>
        <FirewallGroup wcm:action="add" wcm:keyValue="RemoteDesktop">
          <Active>true</Active>
          <Group>Remote Desktop</Group>
          <Profile>all</Profile>
        </FirewallGroup>
      </FirewallGroups>
    </component>
  </settings>

  <settings pass="oobeSystem">
    <component name="Microsoft-Windows-International-Core" processorArchitecture="amd64" publicKeyToken="31bf3856ad364e35" language="neutral" versionScope="nonSxS">
      <InputLocale>{locale}</InputLocale>
      <SystemLocale>{locale}</SystemLocale>
      <UILanguage>{locale}</UILanguage>
      <UserLocale>{locale}</UserLocale>
    </component>
    <component name="Microsoft-Windows-Shell-Setup" processorArchitecture="amd64" publicKeyToken="31bf3856ad364e35" language="neutral" versionScope="nonSxS">
      <OOBE>
        <HideEULAPage>true</HideEULAPage>
        <HideLocalAccountScreen>true</HideLocalAccountScreen>
        <HideOnlineAccountScreens>true</HideOnlineAccountScreens>
        <HideWirelessSetupInOOBE>true</HideWirelessSetupInOOBE>
        <ProtectYourPC>3</ProtectYourPC>
        <SkipMachineOOBE>true</SkipMachineOOBE>
        <SkipUserOOBE>true</SkipUserOOBE>
      </OOBE>
      <UserAccounts>
        <LocalAccounts>
          <LocalAccount wcm:action="add">
            <Name>{user}</Name>
            <Group>Administrators</Group>
            <DisplayName>{user}</DisplayName>
            <Password><Value>{pass}</Value><PlainText>true</PlainText></Password>
          </LocalAccount>
        </LocalAccounts>
      </UserAccounts>{autologon}
      <FirstLogonCommands>{first_logon}
      </FirstLogonCommands>
    </component>
  </settings>

</unattend>
"#,
        locale = spec.locale,
        driver_paths = driver_paths,
        edition = xml_escape(&spec.edition_name),
        computer = xml_escape(&spec.computer_name),
        timezone = spec.timezone,
        user = xml_escape(&spec.username),
        pass = xml_escape(&spec.password),
        autologon = autologon,
        first_logon = first_logon,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_spec_defaults_present() {
        let spec = UnattendSpec::default();
        let xml = render_autounattend(&spec);
        // three setup passes
        assert!(xml.contains(r#"pass="windowsPE""#));
        assert!(xml.contains(r#"pass="specialize""#));
        assert!(xml.contains(r#"pass="oobeSystem""#));
    }

    #[test]
    fn data_volume_initializes_the_second_disk() {
        let xml = render_autounattend(&UnattendSpec {
            data_volume: true,
            ..UnattendSpec::default()
        });
        assert!(xml.contains("Initialize persistent data volume"));
        assert!(xml.contains("PartitionStyle -Eq RAW")); // targets only the uninitialized data disk
                                                         // Off by default.
        assert!(!render_autounattend(&UnattendSpec::default())
            .contains("Initialize persistent data volume"));
    }

    #[test]
    fn injects_virtio_storage_driver() {
        let xml = render_autounattend(&UnattendSpec::default());
        // the disk driver must be injected in WinPE or the target disk is invisible
        assert!(xml.contains("PnpCustomizationsWinPE"));
        assert!(xml.contains(r"e:\viostor\w11\amd64"));
        assert!(xml.contains(r"e:\vioscsi\w11\amd64"));
    }

    #[test]
    fn skips_oobe_and_creates_local_account() {
        let xml = render_autounattend(&UnattendSpec {
            username: "gamer".to_string(),
            password: "pw123".to_string(),
            ..Default::default()
        });
        assert!(xml.contains("<SkipMachineOOBE>true</SkipMachineOOBE>"));
        assert!(xml.contains("<HideOnlineAccountScreens>true</HideOnlineAccountScreens>"));
        assert!(xml.contains("<Name>gamer</Name>"));
        assert!(xml.contains("<Value>pw123</Value>"));
        assert!(xml.contains("<Group>Administrators</Group>"));
    }

    #[test]
    fn autologon_toggles() {
        let on = render_autounattend(&UnattendSpec {
            autologon: true,
            ..Default::default()
        });
        assert!(on.contains("<AutoLogon>"));
        let off = render_autounattend(&UnattendSpec {
            autologon: false,
            ..Default::default()
        });
        assert!(!off.contains("<AutoLogon>"));
    }

    #[test]
    fn wipes_disk_and_installs_guest_tools() {
        let xml = render_autounattend(&UnattendSpec::default());
        assert!(xml.contains("<WillWipeDisk>true</WillWipeDisk>"));
        assert!(xml.contains("virtio-win-guest-tools.exe"));
    }

    #[test]
    fn no_answer_file_for_steamos() {
        assert!(UnattendSpec::for_guest(GuestOs::SteamOs).is_none());
        assert!(UnattendSpec::for_guest(GuestOs::Windows).is_some());
    }

    /// Number of first-logon `<SynchronousCommand>` entries in the answer file.
    fn logon_count(xml: &str) -> usize {
        xml.matches("<SynchronousCommand").count()
    }

    #[test]
    fn plain_station_has_no_vgpu_token_or_apps() {
        let xml = render_autounattend(&UnattendSpec::default());
        assert!(!xml.contains("NVIDIA vGPU guest driver"));
        assert!(!xml.contains("ClientConfigToken"));
        assert!(!xml.contains("SteamSetup.exe"));
        // Only the virtio guest-tools command — nothing follows it.
        assert_eq!(logon_count(&xml), 2);
    }

    #[test]
    fn injects_vgpu_guest_driver_command() {
        let xml = render_autounattend(&UnattendSpec {
            vgpu_driver_exe: Some("nvidia-vgpu-guest.exe".to_string()),
            ..Default::default()
        });
        assert!(xml.contains("Install NVIDIA vGPU guest driver"));
        assert!(xml.contains(r"%d:\nvidia-vgpu-guest.exe -s -noreboot"));
        // virtio + virtio-fs + driver.
        assert_eq!(logon_count(&xml), 3);
    }

    #[test]
    fn injects_dls_token_after_driver_xml_escaped() {
        let xml = render_autounattend(&UnattendSpec {
            vgpu_driver_exe: Some("nvidia-vgpu-guest.exe".to_string()),
            dls_token_url: Some("https://10.0.0.2:8443/-/client-token".to_string()),
            ..Default::default()
        });
        assert!(xml.contains("client_config_token.tok"));
        assert!(xml.contains("https://10.0.0.2:8443/-/client-token"));
        // `&` separator must be XML-escaped in the answer file, never raw.
        assert!(xml.contains("&amp; curl.exe"));
        assert!(!xml.contains("\" & curl.exe"));
        // The driver command comes before the token command.
        assert!(
            xml.find("Install NVIDIA vGPU guest driver").unwrap()
                < xml.find("client_config_token.tok").unwrap()
        );
        assert_eq!(logon_count(&xml), 4);
    }

    #[test]
    fn installs_selected_apps_in_order() {
        let xml = render_autounattend(&UnattendSpec {
            apps: vec![GuestApp::Steam, GuestApp::Sunshine, GuestApp::Discord],
            ..Default::default()
        });
        assert!(xml.contains("Install Steam"));
        assert!(xml.contains("SteamSetup.exe\" /S"));
        assert!(xml.contains("Sunshine-Windows-AMD64-installer.exe"));
        assert!(xml.contains("SunshineSetup.exe\" /S"));
        assert!(xml.contains("DiscordSetup.exe\" -s"));
        // Steam before Sunshine before Discord; virtio + 3 apps = 4 commands.
        assert!(xml.find("Install Steam").unwrap() < xml.find("Install Discord").unwrap());
        assert_eq!(logon_count(&xml), 5);
    }

    #[test]
    fn app_url_query_ampersands_are_xml_escaped() {
        // Discord's installer URL carries a query string with raw `&`, which is invalid XML if not
        // escaped — the answer file would fail to parse.
        let xml = render_autounattend(&UnattendSpec {
            apps: vec![GuestApp::Discord],
            ..Default::default()
        });
        assert!(xml.contains("channel=stable&amp;platform=win&amp;arch=x64"));
        // No unescaped ampersand anywhere in the document (every `&` is part of an entity).
        for (i, _) in xml.match_indices('&') {
            let tail = &xml[i..];
            assert!(
                tail.starts_with("&amp;") || tail.starts_with("&lt;") || tail.starts_with("&gt;"),
                "unescaped '&' at byte {i}: {:?}",
                &tail[..tail.len().min(12)]
            );
        }
    }

    #[test]
    fn apps_follow_driver_and_token() {
        let xml = render_autounattend(&UnattendSpec {
            vgpu_driver_exe: Some("nvidia-vgpu-guest.exe".to_string()),
            dls_token_url: Some("https://h/-/client-token".to_string()),
            apps: vec![GuestApp::Steam],
            ..Default::default()
        });
        // virtio + virtio-fs + driver + token + steam.
        assert_eq!(logon_count(&xml), 5);
        assert!(xml.find("client_config_token.tok").unwrap() < xml.find("Install Steam").unwrap());
    }
}

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
            user = spec.username,
            pass = spec.password,
        )
    } else {
        String::new()
    };

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
      <FirstLogonCommands>
        <SynchronousCommand wcm:action="add">
          <Order>1</Order>
          <Description>Install virtio guest tools (QEMU guest agent, balloon, drivers)</Description>
          <CommandLine>cmd /c for %d in (D E F G) do @if exist %d:\virtio-win-guest-tools.exe start /wait %d:\virtio-win-guest-tools.exe /install /passive /norestart</CommandLine>
        </SynchronousCommand>
      </FirstLogonCommands>
    </component>
  </settings>

</unattend>
"#,
        locale = spec.locale,
        driver_paths = driver_paths,
        edition = spec.edition_name,
        computer = spec.computer_name,
        timezone = spec.timezone,
        user = spec.username,
        pass = spec.password,
        autologon = autologon,
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
}

//! A gaming station: one guest VM bound to a GPU (and, later, a physical seat).

/// Guest operating system for a station.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuestOs {
    Windows,
    SteamOs,
}

/// Declarative description of a gaming station, rendered into a libvirt domain.
#[derive(Debug, Clone)]
pub struct StationSpec {
    /// Station name.
    pub name: String,
    /// Guest OS to run.
    pub guest: GuestOs,
    /// Apply the opt-in "native-hardware" compatibility overlay (off by default).
    pub native_hardware: bool,
}

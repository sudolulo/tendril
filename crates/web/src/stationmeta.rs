//! Per-station settings — kiosk mode and the daily start/stop schedule — persisted as one small
//! JSON file per station under `/var/lib/tendril/meta/` (override with `TENDRIL_META_DIR`). Also
//! home of the schedule loop: every 30s it compares each saved `HH:MM` against the host's local
//! wall clock and fires lifecycle actions when the minute crosses one.

use serde::{Deserialize, Serialize};

use tendril_orchestrator::{DomainState, Libvirt};

/// A station's saved settings. Empty schedule strings mean "no schedule".
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct StationMeta {
    /// Reset the OS disk to the station's golden image on every start (the data volume survives).
    #[serde(default)]
    pub kiosk: bool,
    /// Daily auto-start time as `HH:MM` (host-local), or empty for none.
    #[serde(default)]
    pub sched_start: String,
    /// Daily graceful-shutdown time as `HH:MM` (host-local), or empty for none.
    #[serde(default)]
    pub sched_stop: String,
}

/// Where per-station settings live: env override (tests / relocated data), else the local default.
fn meta_dir() -> String {
    std::env::var("TENDRIL_META_DIR")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "/var/lib/tendril/meta".to_string())
}

/// A station's settings file path — `None` for an out-of-charset name (same rule as
/// `stations::valid_station_name`, so a crafted name can't become a path escape).
fn meta_file(name: &str) -> Option<String> {
    crate::stations::valid_station_name(name).then(|| format!("{}/{name}.json", meta_dir()))
}

/// A station's saved settings — the default when it has none (or the name is invalid).
pub(crate) fn load(name: &str) -> StationMeta {
    meta_file(name)
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// Persist a station's settings (JSON under [`meta_dir`]).
pub(crate) fn save(name: &str, meta: &StationMeta) -> Result<(), String> {
    let path = meta_file(name).ok_or("invalid station name")?;
    let _ = std::fs::create_dir_all(meta_dir());
    let json = serde_json::to_string(meta).map_err(|e| e.to_string())?;
    std::fs::write(path, json).map_err(|e| e.to_string())
}

/// Drop a station's settings file (called from the delete teardown).
pub(crate) fn remove(name: &str) {
    if let Some(p) = meta_file(name) {
        let _ = std::fs::remove_file(p);
    }
}

/// True for an empty string (no schedule) or a zero-padded 24-hour `HH:MM` — exactly what an
/// `<input type="time">` submits; anything else is rejected rather than silently never firing.
pub(crate) fn valid_hhmm(s: &str) -> bool {
    if s.is_empty() {
        return true;
    }
    if s.len() != 5 {
        return false;
    }
    let Some((h, m)) = s.split_once(':') else {
        return false;
    };
    let (Ok(h), Ok(m)) = (h.parse::<u32>(), m.parse::<u32>()) else {
        return false;
    };
    h < 24 && m < 60
}

// ── the schedule loop ─────────────────────────────────────────────────────────────────────────

/// Stations with a daily schedule set, as `(name, meta)`, from the meta dir.
fn scheduled() -> Vec<(String, StationMeta)> {
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(meta_dir()) {
        for e in rd.flatten() {
            let fname = e.file_name().to_string_lossy().into_owned();
            let Some(name) = fname.strip_suffix(".json") else {
                continue;
            };
            if !crate::stations::valid_station_name(name) {
                continue;
            }
            let meta = load(name);
            if !meta.sched_start.is_empty() || !meta.sched_stop.is_empty() {
                out.push((name.to_string(), meta));
            }
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// The lifecycle actions due for one station when the scheduler's clock moves from `last` (the last
/// minute it processed; empty on the first tick) to `now`, both `HH:MM`. Pure. Acting only when the
/// minute changes guards against double-firing — the loop ticks twice per minute.
pub(crate) fn due_actions(last: &str, now: &str, meta: &StationMeta) -> Vec<&'static str> {
    let mut actions = Vec::new();
    if last == now {
        return actions;
    }
    if !meta.sched_start.is_empty() && meta.sched_start == now {
        actions.push("start");
    }
    if !meta.sched_stop.is_empty() && meta.sched_stop == now {
        actions.push("stop");
    }
    actions
}

/// The current local wall-clock minute as `HH:MM` (shelled out to `date`, which respects the host's
/// timezone — matching how everything else here reads the host).
fn local_hhmm() -> Option<String> {
    crate::ui::run_stdout("date", &["+%H:%M"]).map(|s| s.trim().to_string())
}

/// One pass of the schedule loop: when the wall-clock minute has changed since `last`, fire any
/// station whose daily start/stop matches the new minute, then remember it. Actions go through
/// `stations::lifecycle` — not `virsh` directly — so a kiosk station's reset-on-start applies to
/// scheduled starts too, and a scheduled stop is the same graceful shutdown as the UI button.
pub(crate) fn scheduler_tick(last: &mut String) {
    if crate::ui::is_demo() {
        return;
    }
    let Some(now) = local_hhmm() else {
        return;
    };
    if *last == now {
        return;
    }
    let lv = Libvirt::system();
    for (name, meta) in scheduled() {
        for action in due_actions(last, &now, &meta) {
            // Only start a station that's off and only stop one that's running — a schedule never
            // fights a manual action (or another schedule) already in effect.
            let due = match action {
                "start" => matches!(lv.state(&name), DomainState::ShutOff),
                _ => matches!(lv.state(&name), DomainState::Running),
            };
            if !due {
                continue;
            }
            if let Err(e) = crate::stations::lifecycle(&name, action) {
                eprintln!("schedule: {action} {name} failed: {e}");
            }
        }
    }
    *last = now;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meta_round_trips_and_removes() {
        let dir = std::env::temp_dir().join(format!("tendril-meta-test-{}", std::process::id()));
        std::env::set_var("TENDRIL_META_DIR", &dir);
        let meta = StationMeta {
            kiosk: true,
            sched_start: "08:30".into(),
            sched_stop: "22:00".into(),
        };
        save("station1", &meta).unwrap();
        let back = load("station1");
        assert!(back.kiosk);
        assert_eq!(back.sched_start, "08:30");
        assert_eq!(back.sched_stop, "22:00");
        // Absent stations load as defaults; out-of-charset names refuse to save.
        assert!(!load("no-such-station").kiosk);
        assert!(save("../evil", &meta).is_err());
        // The schedule sweep sees the saved station (it has times set).
        let sched = scheduled();
        assert_eq!(sched.len(), 1);
        assert_eq!(sched[0].0, "station1");
        remove("station1");
        assert!(!load("station1").kiosk);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn schedule_fires_only_on_minute_crossings() {
        let meta = StationMeta {
            kiosk: false,
            sched_start: "08:00".into(),
            sched_stop: "20:30".into(),
        };
        assert_eq!(due_actions("07:59", "08:00", &meta), vec!["start"]);
        assert_eq!(due_actions("20:29", "20:30", &meta), vec!["stop"]);
        // Same minute again (the loop ticks twice a minute): no double fire.
        assert!(due_actions("08:00", "08:00", &meta).is_empty());
        assert!(due_actions("20:30", "20:30", &meta).is_empty());
        // A minute with nothing scheduled does nothing.
        assert!(due_actions("08:00", "08:01", &meta).is_empty());
        // First tick after startup (no last minute yet) still fires an exact match.
        assert_eq!(due_actions("", "08:00", &meta), vec!["start"]);
        // No schedule set → never fires.
        assert!(due_actions("07:59", "08:00", &StationMeta::default()).is_empty());
        // start == stop fires both, deterministically (a misconfiguration, not a crash).
        let both = StationMeta {
            sched_start: "09:00".into(),
            sched_stop: "09:00".into(),
            ..Default::default()
        };
        assert_eq!(due_actions("08:59", "09:00", &both), vec!["start", "stop"]);
    }

    #[test]
    fn hhmm_validation() {
        assert!(valid_hhmm("")); // empty = no schedule
        assert!(valid_hhmm("00:00"));
        assert!(valid_hhmm("23:59"));
        assert!(!valid_hhmm("24:00"));
        assert!(!valid_hhmm("12:60"));
        assert!(!valid_hhmm("9:30")); // must be zero-padded, as <input type="time"> submits
        assert!(!valid_hhmm("noon"));
    }
}

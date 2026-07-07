//! Shared UI: the page shell (top bar + nav), design tokens, and small components every page reuses.

use std::process::Command;

use maud::{html, Markup, PreEscaped, DOCTYPE};

use tendril_capability_engine::{GpuVendor, PassthroughViability};
use tendril_orchestrator::DomainState;

/// The nav items, in order: (href, key, label). `key` matches the `active` arg on each page.
const NAV: &[(&str, &str, &str)] = &[
    ("/", "dashboard", "Dashboard"),
    ("/stations", "stations", "Stations"),
    ("/hardware", "hardware", "Hardware"),
    ("/media", "media", "Media"),
    ("/network", "network", "Network"),
    ("/system", "system", "System"),
];

/// Full-page shell. `active` highlights the current nav item.
pub fn page(active: &str, title: &str, body: Markup) -> Markup {
    let host = run_stdout("hostname", &[])
        .unwrap_or_default()
        .trim()
        .to_string();
    let ip = run_stdout("hostname", &["-I"])
        .unwrap_or_default()
        .split_whitespace()
        .next()
        .unwrap_or("")
        .to_string();
    html! {
        (DOCTYPE)
        html lang="en" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                title { "Tendril · " (title) }
                script src="/assets/htmx.min.js" {}
                style { (PreEscaped(CSS)) }
            }
            body {
                header.topbar {
                    div.brand {
                        svg.glyph viewBox="0 0 24 24" fill="none" stroke="currentColor"
                            stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round" {
                            path d="M12 21c0-5 0-8-3-10S5 7 5 4" {}
                            path d="M12 21c0-4 .3-6.5 2.4-8.2C16.5 11 18 10 18 7" {}
                            circle cx="5" cy="4" r="1.4" {}
                            circle cx="18" cy="7" r="1.4" {}
                            circle cx="12" cy="21" r="1.4" {}
                        }
                        div { b { "TENDRIL" } }
                    }
                    nav.nav {
                        @for (href, key, label) in NAV {
                            a href=(href) class=(if *key == active { "active" } else { "" }) { (label) }
                        }
                    }
                    div.spacer {}
                    @if update_staged() {
                        a.updatebadge href="/system" title="A new OS image is downloaded and ready" { "⬆ Update ready" }
                    }
                    @if !host.is_empty() {
                        div.host { span.led {} (host) " · " span.mono { (ip) } }
                    }
                }
                main.wrap { (body) }
            }
        }
    }
}

/// A titled card/panel with an optional right-aligned count/subtitle.
pub fn panel(title: &str, meta: Option<&str>, body: Markup) -> Markup {
    html! {
        section.panel {
            header {
                h2 { (title) }
                @if let Some(m) = meta { span.count { (m) } }
            }
            (body)
        }
    }
}

/// A colored run-state pill.
pub fn state_pill(s: DomainState) -> Markup {
    html! { span class=(format!("pill {}", state_class(s))) { span.led {} (state_label(s)) } }
}

pub fn vendor(v: GpuVendor) -> &'static str {
    match v {
        GpuVendor::Nvidia => "NVIDIA",
        GpuVendor::Amd => "AMD",
        GpuVendor::Intel => "Intel",
        GpuVendor::Unknown => "GPU",
    }
}

pub fn viability(v: PassthroughViability) -> &'static str {
    match v {
        PassthroughViability::Isolated => "isolated (clean)",
        PassthroughViability::SharedGroup => "shared group (ACS override)",
        PassthroughViability::NoIommu => "no IOMMU",
    }
}

pub fn state_label(s: DomainState) -> &'static str {
    match s {
        DomainState::Running => "running",
        DomainState::Paused => "paused",
        DomainState::ShutOff => "shut off",
        DomainState::Absent => "absent",
        DomainState::Other => "other",
    }
}

pub fn state_class(s: DomainState) -> &'static str {
    match s {
        DomainState::Running => "running",
        DomainState::Paused => "installing",
        _ => "off",
    }
}

/// True if a bootc OS update is downloaded and staged (pending a reboot). Fast/local — reads
/// `bootc status`, no network. False when bootc is absent (e.g. a dev host).
pub fn update_staged() -> bool {
    let Some(j) = run_stdout("bootc", &["status", "--format", "json"]) else {
        return false;
    };
    // "staged": {...} → an update is staged; "staged": null → none.
    match j.find("\"staged\"") {
        Some(i) => j[i + "\"staged\"".len()..]
            .trim_start_matches([':', ' ', '\n', '\t', '\r'])
            .starts_with('{'),
        None => false,
    }
}

/// Run a command and return trimmed stdout on success (read-only host queries).
pub fn run_stdout(cmd: &str, args: &[&str]) -> Option<String> {
    Command::new(cmd)
        .args(args)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
}

pub const CSS: &str = r#"
:root {
  --bg:#0e1217; --surface:#161c24; --surface-2:#1b232d; --line:#273240;
  --fg:#e7edf4; --muted:#8b97a6; --faint:#5d6a7a;
  --accent:#2fd4c6; --accent-ink:#052b28; --accent-soft:rgba(47,212,198,.12);
  --ok:#46c46a; --info:#f0b429; --off:#6a7686; --crit:#f0616d;
  --ok-soft:rgba(70,196,106,.14); --info-soft:rgba(240,180,41,.14);
  --off-soft:rgba(106,118,134,.16); --crit-soft:rgba(240,97,109,.14);
  --radius:11px; --shadow:0 1px 0 rgba(255,255,255,.02), 0 8px 24px rgba(0,0,0,.28);
}
@media (prefers-color-scheme: light) {
  :root {
    --bg:#f4f7fa; --surface:#fff; --surface-2:#eef2f7; --line:#dde5ee;
    --fg:#10171f; --muted:#55606f; --faint:#8a95a3;
    --accent:#0f9c90; --accent-ink:#fff; --accent-soft:rgba(15,156,144,.10);
    --ok:#1a9e46; --info:#b9791a; --off:#7a8697; --crit:#cf3b46;
    --ok-soft:rgba(26,158,70,.10); --info-soft:rgba(185,121,26,.10);
    --off-soft:rgba(122,134,151,.12); --crit-soft:rgba(207,59,70,.10);
    --shadow:0 1px 2px rgba(16,23,31,.05), 0 8px 22px rgba(16,23,31,.07);
  }
}
:root[data-theme="dark"] {
  --bg:#0e1217; --surface:#161c24; --surface-2:#1b232d; --line:#273240;
  --fg:#e7edf4; --muted:#8b97a6; --faint:#5d6a7a;
  --accent:#2fd4c6; --accent-ink:#052b28; --accent-soft:rgba(47,212,198,.12);
  --ok:#46c46a; --info:#f0b429; --off:#6a7686; --crit:#f0616d;
  --ok-soft:rgba(70,196,106,.14); --info-soft:rgba(240,180,41,.14);
  --off-soft:rgba(106,118,134,.16); --crit-soft:rgba(240,97,109,.14);
  --shadow:0 1px 0 rgba(255,255,255,.02), 0 8px 24px rgba(0,0,0,.28);
}
:root[data-theme="light"] {
  --bg:#f4f7fa; --surface:#fff; --surface-2:#eef2f7; --line:#dde5ee;
  --fg:#10171f; --muted:#55606f; --faint:#8a95a3;
  --accent:#0f9c90; --accent-ink:#fff; --accent-soft:rgba(15,156,144,.10);
  --ok:#1a9e46; --info:#b9791a; --off:#7a8697; --crit:#cf3b46;
  --ok-soft:rgba(26,158,70,.10); --info-soft:rgba(185,121,26,.10);
  --off-soft:rgba(122,134,151,.12); --crit-soft:rgba(207,59,70,.10);
  --shadow:0 1px 2px rgba(16,23,31,.05), 0 8px 22px rgba(16,23,31,.07);
}
* { box-sizing:border-box; }
body { margin:0; background:var(--bg); color:var(--fg);
  font:15px/1.55 system-ui,-apple-system,"Segoe UI",Roboto,sans-serif; -webkit-font-smoothing:antialiased; }
a { color:var(--accent); text-decoration:none; }
.mono { font-family:ui-monospace,SFMono-Regular,Menlo,monospace; }
.num { font-variant-numeric:tabular-nums; }
.muted { color:var(--muted); }
.wrap { max-width:1080px; margin:22px auto 64px; padding:0 20px; }

.topbar { position:sticky; top:0; z-index:5; display:flex; align-items:center; gap:22px;
  padding:12px 22px; border-bottom:1px solid var(--line);
  background:color-mix(in srgb, var(--bg) 88%, transparent); backdrop-filter:blur(8px); }
.brand { display:flex; align-items:center; gap:10px; }
.glyph { width:24px; height:24px; color:var(--accent); flex:none; }
.brand b { letter-spacing:.24em; font-size:14px; }
.nav { display:flex; gap:4px; flex-wrap:wrap; }
.nav a { color:var(--muted); padding:6px 12px; border-radius:8px; font-size:14px; }
.nav a:hover { color:var(--fg); background:var(--surface-2); }
.nav a.active { color:var(--accent); background:var(--accent-soft); }
.spacer { flex:1; }
.host { display:flex; align-items:center; gap:8px; padding:6px 11px; border:1px solid var(--line);
  border-radius:999px; background:var(--surface); color:var(--muted); font-size:12.5px; }
.host .led { width:7px; height:7px; border-radius:50%; background:var(--ok); flex:none; }
.host .mono { color:var(--fg); }
.updatebadge { background:var(--info-soft); color:var(--info); border:1px solid var(--info);
  padding:5px 11px; border-radius:999px; font-size:12.5px; font-weight:600; white-space:nowrap; }
.updatebadge:hover { filter:brightness(1.12); }

.summary { display:grid; grid-template-columns:repeat(4,1fr); gap:14px; margin-bottom:24px; }
.stat { background:var(--surface); border:1px solid var(--line); border-radius:var(--radius);
  padding:15px 17px; box-shadow:var(--shadow); }
.stat .k { color:var(--muted); font-size:11px; text-transform:uppercase; letter-spacing:.09em; }
.stat .v { font-size:30px; font-weight:640; margin-top:4px; letter-spacing:-.01em; }
.stat .v small { font-size:14px; font-weight:500; color:var(--muted); }
.stat .v.accent { color:var(--accent); }

.panel { background:var(--surface); border:1px solid var(--line); border-radius:var(--radius);
  box-shadow:var(--shadow); margin-bottom:22px; overflow:hidden; }
.panel > header { display:flex; align-items:center; gap:10px; padding:14px 18px; border-bottom:1px solid var(--line); }
.panel > header h2 { margin:0; font-size:12px; text-transform:uppercase; letter-spacing:.1em; color:var(--muted); font-weight:700; }
.panel > header .count { color:var(--faint); font-size:12px; margin-left:auto; }
.pad { padding:16px 18px; }
.scroll { overflow-x:auto; }
table { width:100%; border-collapse:collapse; min-width:560px; }
th,td { text-align:left; padding:12px 18px; border-bottom:1px solid var(--line); vertical-align:middle; }
tr:last-child td { border-bottom:0; }
th { color:var(--faint); font-size:10.5px; font-weight:700; text-transform:uppercase; letter-spacing:.08em; }
tbody tr { transition:background .12s; }
tbody tr:hover { background:var(--surface-2); }
.name { font-weight:600; }
.sub { color:var(--muted); font-size:12.5px; }
.addr { color:var(--muted); font-size:12.5px; }
.right { text-align:right; } .actions { display:flex; gap:7px; justify-content:flex-end; }

.badge { font-size:10px; font-weight:700; letter-spacing:.06em; padding:2px 6px; border-radius:5px;
  border:1px solid var(--line); color:var(--muted); background:var(--surface-2); }
.pill { display:inline-flex; align-items:center; gap:7px; font-size:12px; font-weight:600;
  padding:4px 10px 4px 8px; border-radius:999px; }
.pill .led { width:7px; height:7px; border-radius:50%; flex:none; }
.pill.running { background:var(--ok-soft); color:var(--ok); } .pill.running .led { background:var(--ok); box-shadow:0 0 0 3px var(--ok-soft); }
.pill.installing { background:var(--info-soft); color:var(--info); } .pill.installing .led { background:var(--info); animation:pulse 1.4s ease-in-out infinite; }
.pill.off { background:var(--off-soft); color:var(--off); } .pill.off .led { background:var(--off); }
.via.clean::before { content:""; display:inline-block; width:6px; height:6px; border-radius:50%; background:var(--ok); margin-right:7px; vertical-align:middle; }

.btn { font:inherit; font-size:13.5px; cursor:pointer; border:1px solid var(--line);
  background:var(--surface); color:var(--fg); padding:7px 13px; border-radius:8px; transition:border-color .15s,background .15s; }
.btn:hover { border-color:var(--faint); background:var(--surface-2); }
.btn:focus-visible { outline:2px solid var(--accent); outline-offset:2px; }
.btn.primary { background:var(--accent); color:var(--accent-ink); border-color:transparent; font-weight:600; }
.btn.primary:hover { filter:brightness(1.06); }
.btn.sm { padding:5px 10px; font-size:12.5px; }
.btn.danger:hover { border-color:var(--crit); color:var(--crit); }
.btnrow { display:flex; gap:8px; flex-wrap:wrap; }

form.grid { display:grid; grid-template-columns:repeat(2,1fr); gap:16px 20px; }
.field { display:flex; flex-direction:column; gap:6px; }
.field.wide { grid-column:1 / -1; }
.field label { font-size:12px; color:var(--muted); font-weight:600; }
.field .hint { font-size:11.5px; color:var(--faint); }
input,select { font:inherit; font-size:14px; background:var(--bg); color:var(--fg);
  border:1px solid var(--line); border-radius:8px; padding:8px 10px; width:100%; }
input:focus,select:focus { outline:none; border-color:var(--accent); }
input[type=checkbox] { width:auto; accent-color:var(--accent); }
.check { flex-direction:row; align-items:center; gap:9px; }
.check label { font-size:13.5px; color:var(--fg); }

.banner { padding:.7rem .9rem; border-radius:8px; margin-bottom:14px; font-size:13.5px; }
.banner.error { background:var(--crit-soft); border:1px solid var(--crit); color:var(--crit); }
.banner.ok { background:var(--ok-soft); border:1px solid var(--ok); color:var(--ok); }

.console { width:100%; aspect-ratio:16/10; max-height:72vh; background:#000; border-radius:8px; overflow:hidden; }
.console #screen { width:100%; height:100%; }
.console canvas { display:block; }
.emptybox { padding:40px 18px; text-align:center; color:var(--muted); }

@keyframes pulse { 0%,100% { opacity:1; } 50% { opacity:.35; } }
@media (prefers-reduced-motion: reduce) { * { animation:none !important; transition:none !important; } }
@media (max-width:820px) { .summary { grid-template-columns:repeat(2,1fr); } form.grid { grid-template-columns:1fr; } .nav { order:3; width:100%; } }
"#;

//! Canned data for `TENDRIL_DEMO` mode. The public demo is fully self-contained: none of the data it
//! shows touches the machine it runs on, so it can't collide with a real Tendril instance on the same
//! host. Only stations/media/seats are canned — hardware/host facts are read live (they're read-only
//! and identical on any instance).

use maud::{html, Markup};
use tendril_orchestrator::DomainState;

use crate::ui;

/// Demo stations: (name, guest OS, state, has a GPU passed through).
fn stations() -> Vec<(&'static str, &'static str, DomainState, bool)> {
    vec![
        ("living-room", "Windows 11", DomainState::Running, true),
        ("office", "SteamOS (Bazzite)", DomainState::Running, true),
        ("den", "Windows 11", DomainState::ShutOff, true),
        (
            "guest-room",
            "SteamOS (Bazzite)",
            DomainState::ShutOff,
            false,
        ),
    ]
}

/// (total, running) station counts for the dashboard summary.
pub fn counts() -> (usize, usize) {
    let s = stations();
    (
        s.len(),
        s.iter()
            .filter(|(_, _, st, _)| matches!(st, DomainState::Running))
            .count(),
    )
}

/// The stations table (dashboard panel + Stations page). Actions are present but disabled by demo
/// mode's POST guard, so clicking them shows the "actions disabled" banner.
pub fn stations_fragment() -> Markup {
    html! {
        div #stations {
            div.scroll {
                table {
                    thead { tr { th { "Station" } th { "State" } th.right { "Actions" } } }
                    tbody { @for (name, os, state, gpu) in stations() {
                        tr {
                            td {
                                a href=(format!("/stations/{name}")) { (name) }
                                span.sub.mono style="margin-left:8px" { (os) }
                                @if !gpu {
                                    span title="No GPU passed through — no graphics acceleration"
                                        style="color:var(--crit); margin-left:6px; cursor:help" { "⚠" }
                                }
                            }
                            td { (ui::state_pill(state)) }
                            td.right { div.actions {
                                a.btn.sm href=(format!("/stations/{name}")) { "Open" }
                                @if matches!(state, DomainState::Running) {
                                    button.btn.sm.danger hx-post=(format!("/stations/{name}/stop")) hx-target="#stations" hx-swap="outerHTML" { "Shut down" }
                                } @else {
                                    button.btn.sm hx-post=(format!("/stations/{name}/start")) hx-target="#stations" hx-swap="outerHTML" { "Start" }
                                }
                                button.btn.sm.danger hx-post=(format!("/stations/{name}/delete")) hx-target="#stations" hx-swap="outerHTML" { "Delete" }
                            } }
                        }
                    } }
                }
            }
        }
    }
}

/// Canned station detail with a representative console preview (running stations show a mock
/// Windows/Steam screen; the live noVNC console works on a real install).
pub fn station_detail(name: &str) -> Markup {
    let found = stations().into_iter().find(|(n, ..)| *n == name);
    ui::page(
        "stations",
        name,
        html! {
            a.btn.sm href="/stations" { "← Stations" }
            @match found {
                Some((_, os, state, gpu)) => {
                    @let running = matches!(state, DomainState::Running);
                    (ui::panel(name, Some(if running { "console" } else { "powered off" }), html! {
                        div.pad {
                            (console_preview(os, running))
                            table style="margin-top:14px" { tbody {
                                tr { td.sub style="width:10rem" { "Guest OS" } td { (os) } }
                                tr { td.sub { "State" } td { (ui::state_pill(state)) } }
                                tr { td.sub { "GPU passthrough" } td { @if gpu { "yes — whole IOMMU group" } @else { "none (headless)" } } }
                            } }
                            p.sub style="margin-top:12px" { "Representative screen — this is the read-only demo. On a real Tendril install this is a live in-browser noVNC console." }
                        }
                    }))
                }
                None => (ui::panel("Not found", None, html! { div.pad { p.muted { "No such demo station." } } })),
            }
        },
    )
}

/// The console box for the demo detail: an original SVG mock of the guest's screen (no copyrighted
/// assets), or a powered-off screen when the station is stopped.
fn console_preview(os: &str, running: bool) -> Markup {
    let svg = if !running {
        SCREEN_OFF
    } else if os.contains("Windows") {
        SCREEN_WINDOWS
    } else {
        SCREEN_STEAM
    };
    html! {
        div.console { (maud::PreEscaped(svg)) }
    }
}

/// A mock Steam "gaming mode" screen (original artwork, generic game titles).
const SCREEN_STEAM: &str = r##"<svg viewBox="0 0 960 600" width="100%" height="100%" preserveAspectRatio="xMidYMid slice" xmlns="http://www.w3.org/2000/svg" font-family="system-ui,Segoe UI,sans-serif">
<defs>
<linearGradient id="sbg" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#1b2838"/><stop offset="1" stop-color="#0a1017"/></linearGradient>
<linearGradient id="t1" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="#3a6ea5"/><stop offset="1" stop-color="#14324f"/></linearGradient>
<linearGradient id="t2" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="#8a3ffc"/><stop offset="1" stop-color="#3b1a63"/></linearGradient>
<linearGradient id="t3" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="#e0673a"/><stop offset="1" stop-color="#6b2a14"/></linearGradient>
<linearGradient id="t4" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="#2fb37a"/><stop offset="1" stop-color="#144a33"/></linearGradient>
<linearGradient id="t5" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="#c93f6a"/><stop offset="1" stop-color="#5c1a30"/></linearGradient>
</defs>
<rect width="960" height="600" fill="url(#sbg)"/>
<circle cx="34" cy="40" r="12" fill="#66c0f4"/><path d="M28 40l5 5 9-11" stroke="#0a1017" stroke-width="3" fill="none"/>
<text x="62" y="47" fill="#c7d5e0" font-size="24" font-weight="700">Library</text>
<circle cx="878" cy="40" r="16" fill="#2a475e"/><text x="820" y="46" fill="#9fb0bf" font-size="16">87%</text><rect x="900" y="32" width="26" height="16" rx="3" fill="none" stroke="#9fb0bf"/><rect x="903" y="35" width="18" height="10" fill="#66c0f4"/>
<g>
<rect x="36" y="120" width="160" height="228" rx="10" fill="url(#t1)" stroke="#66c0f4" stroke-width="3"/><rect x="36" y="300" width="160" height="48" rx="0" fill="#0a1017" opacity="0.55"/><text x="50" y="330" fill="#e6eef5" font-size="16" font-weight="600">Nebula Drift</text>
<rect x="212" y="120" width="160" height="228" rx="10" fill="url(#t2)"/><text x="226" y="330" fill="#e6eef5" font-size="16" font-weight="600">Iron Vanguard</text>
<rect x="388" y="120" width="160" height="228" rx="10" fill="url(#t3)"/><text x="402" y="330" fill="#e6eef5" font-size="16" font-weight="600">Pixel Rally</text>
<rect x="564" y="120" width="160" height="228" rx="10" fill="url(#t4)"/><text x="578" y="330" fill="#e6eef5" font-size="16" font-weight="600">Deep Hollow</text>
<rect x="740" y="120" width="184" height="228" rx="10" fill="url(#t5)"/><text x="754" y="330" fill="#e6eef5" font-size="16" font-weight="600">Skyforge</text>
</g>
<text x="36" y="400" fill="#8ba0b2" font-size="15">RECENT</text><rect x="36" y="416" width="120" height="8" rx="4" fill="#2a475e"/>
<rect x="0" y="556" width="960" height="44" fill="#0a1017" opacity="0.7"/>
<circle cx="52" cy="578" r="9" fill="#66c0f4"/><text x="68" y="583" fill="#c7d5e0" font-size="15">Play</text>
<text x="150" y="583" fill="#7a8b99" font-size="15">Y Filter</text><text x="248" y="583" fill="#7a8b99" font-size="15">☰ Menu</text>
<text x="878" y="583" fill="#9fb0bf" font-size="15">2:14 PM</text>
</svg>"##;

/// A mock Windows 11 desktop (original artwork, no logos).
const SCREEN_WINDOWS: &str = r##"<svg viewBox="0 0 960 600" width="100%" height="100%" preserveAspectRatio="xMidYMid slice" xmlns="http://www.w3.org/2000/svg" font-family="system-ui,Segoe UI,sans-serif">
<defs>
<radialGradient id="wbg" cx="50%" cy="38%" r="85%"><stop offset="0" stop-color="#3f74d6"/><stop offset="0.55" stop-color="#1f3f86"/><stop offset="1" stop-color="#0b1c44"/></radialGradient>
</defs>
<rect width="960" height="600" fill="url(#wbg)"/>
<ellipse cx="480" cy="230" rx="300" ry="200" fill="#6f9bf0" opacity="0.25"/>
<ellipse cx="470" cy="250" rx="150" ry="120" fill="#a9c4ff" opacity="0.18"/>
<g>
<rect x="30" y="28" width="44" height="44" rx="8" fill="#2b6be0"/><path d="M42 50h20M52 40v20" stroke="#fff" stroke-width="3"/><text x="24" y="90" fill="#eaf1ff" font-size="13">This PC</text>
<rect x="30" y="112" width="44" height="44" rx="8" fill="#3a3f4b"/><path d="M40 122h24v24h-24z" fill="none" stroke="#cdd6e4" stroke-width="2"/><text x="26" y="174" fill="#eaf1ff" font-size="13">Recycle</text>
</g>
<g transform="translate(348,548)">
<rect x="0" y="0" width="264" height="42" rx="12" fill="#20242c" opacity="0.82"/>
<rect x="16" y="11" width="20" height="20" rx="3" fill="#2b6be0"/><rect x="16" y="11" width="9" height="9" fill="#4f8cff"/><rect x="27" y="11" width="9" height="9" fill="#4f8cff"/><rect x="16" y="22" width="9" height="9" fill="#4f8cff"/><rect x="27" y="22" width="9" height="9" fill="#4f8cff"/>
<circle cx="62" cy="21" r="10" fill="none" stroke="#cdd6e4" stroke-width="2"/><line x1="69" y1="28" x2="76" y2="35" stroke="#cdd6e4" stroke-width="2"/>
<rect x="96" y="11" width="20" height="20" rx="4" fill="#3a7bd5"/>
<rect x="128" y="11" width="20" height="20" rx="4" fill="#e6772e"/>
<rect x="160" y="11" width="20" height="20" rx="4" fill="#2aa775"/>
</g>
<text x="898" y="576" fill="#eaf1ff" font-size="14" text-anchor="end">3:42 PM</text>
<text x="898" y="592" fill="#c9d6ef" font-size="12" text-anchor="end">7/7/2026</text>
</svg>"##;

/// A powered-off screen.
const SCREEN_OFF: &str = r##"<svg viewBox="0 0 960 600" width="100%" height="100%" preserveAspectRatio="xMidYMid slice" xmlns="http://www.w3.org/2000/svg" font-family="system-ui,sans-serif">
<rect width="960" height="600" fill="#05070a"/>
<circle cx="480" cy="272" r="34" fill="none" stroke="#3a4652" stroke-width="4"/><line x1="480" y1="248" x2="480" y2="276" stroke="#3a4652" stroke-width="4"/>
<text x="480" y="342" fill="#5d6a7a" font-size="18" text-anchor="middle">powered off</text>
</svg>"##;

/// Demo install media: (filename, size, verification-state) — `verified` / `local` (no upstream).
fn media_rows() -> Vec<(&'static str, &'static str, &'static str)> {
    vec![
        ("win11.iso", "5.8 GB", "local"),
        ("virtio-win.iso", "708 MB", "local"),
        ("bazzite-deck-nvidia.iso", "3.1 GB", "verified"),
    ]
}

/// The Media page's install-media table (canned, with the same provenance tooltips as the real one).
pub fn media_table() -> Markup {
    html! {
        div.scroll {
            table {
                thead { tr { th { "File" } th { "Verification" } th.right { "Size" } } }
                tbody { @for (f, sz, ver) in media_rows() {
                    tr {
                        td {
                            span.mono { (f) }
                            @if let Some(p) = crate::pages::provenance(f) {
                                span.info title=(p) style="margin-left:6px; cursor:help; color:var(--muted); border-bottom:1px dotted var(--muted)" { "\u{24D8} source" }
                            }
                        }
                        td {
                            @if ver == "verified" {
                                span.pill.running { span.led {} "verified" }
                            } @else {
                                span.sub { "sha256 recorded \u{b7} no upstream" }
                            }
                        }
                        td.right.num { (sz) }
                    }
                } }
            }
        }
    }
}

/// Demo saved images (golden templates): (name, human size).
pub fn images() -> Vec<(String, String)> {
    vec![
        ("windows11-base".to_string(), "18.4 GB".to_string()),
        ("bazzite-gaming".to_string(), "12.1 GB".to_string()),
    ]
}

/// Demo seats (named USB device groups).
pub fn seats() -> Vec<(&'static str, &'static str)> {
    vec![
        ("Living room", "Xbox controller, wireless keyboard"),
        ("Office", "Logitech mouse, mechanical keyboard"),
    ]
}

/// The Seats panel (Hardware page), canned.
pub fn seats_panel() -> Markup {
    html! {
        div #seats {
            div.pad {
                table {
                    thead { tr { th { "Seat" } th { "Devices" } th.right { "" } } }
                    tbody { @for (name, devs) in seats() {
                        tr { td.name { (name) } td.sub { (devs) } td.right { button.btn.sm.danger { "Delete" } } }
                    } }
                }
            }
        }
    }
}

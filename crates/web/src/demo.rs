//! Canned data for `TENDRIL_DEMO` mode. The public demo is fully self-contained: none of the data it
//! shows touches the machine it runs on, so it can't collide with a real Tendril instance on the same
//! host. Only stations/media/seats are canned — hardware/host facts are read live (they're read-only
//! and identical on any instance).

use maud::{html, Markup};
use tendril_orchestrator::DomainState;

use crate::ui;

/// Demo stations: (name, guest OS, state, has a GPU passed through).
pub fn stations() -> Vec<(&'static str, &'static str, DomainState, bool)> {
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

/// A simple canned station detail (no live console — that needs a real VM).
pub fn station_detail(name: &str) -> Markup {
    let found = stations().into_iter().find(|(n, ..)| *n == name);
    ui::page(
        "stations",
        name,
        html! {
            a.btn.sm href="/stations" { "← Stations" }
            @match found {
                Some((_, os, state, gpu)) => {
                    (ui::panel(name, None, html! {
                        div.pad {
                            table { tbody {
                                tr { td.sub style="width:10rem" { "Guest OS" } td { (os) } }
                                tr { td.sub { "State" } td { (ui::state_pill(state)) } }
                                tr { td.sub { "GPU passthrough" } td { @if gpu { "yes — whole IOMMU group" } @else { "none (headless)" } } }
                            } }
                            p.sub style="margin-top:14px" { "This is the read-only demo — the live in-browser console and lifecycle controls work on a real Tendril install." }
                        }
                    }))
                }
                None => (ui::panel("Not found", None, html! { div.pad { p.muted { "No such demo station." } } })),
            }
        },
    )
}

/// Demo install media: (filename, size, verification-state) — `verified` / `local` (no upstream).
pub fn media_rows() -> Vec<(&'static str, &'static str, &'static str)> {
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

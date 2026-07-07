//! Seats: named groups of USB devices (a player's keyboard/mouse/controller) that get passed through
//! to a station as a unit. Persisted as one line per seat in `/etc/tendril/seats.conf`
//! (`name\tvvvv:pppp,vvvv:pppp`).

use std::path::Path as FsPath;

use axum::extract::Query;
use maud::{html, Markup};
use serde::Deserialize;

use tendril_capability_engine::usb;
use tendril_orchestrator::UsbPassthrough;

fn seats_file() -> String {
    std::env::var("TENDRIL_SEATS_FILE").unwrap_or_else(|_| "/etc/tendril/seats.conf".to_string())
}

pub struct Seat {
    pub name: String,
    pub devices: Vec<(u16, u16)>,
}

pub fn load() -> Vec<Seat> {
    std::fs::read_to_string(seats_file())
        .map(|txt| txt.lines().filter_map(parse_line).collect())
        .unwrap_or_default()
}

fn parse_line(l: &str) -> Option<Seat> {
    let (name, rest) = l.split_once('\t')?;
    let name = name.trim();
    if name.is_empty() {
        return None;
    }
    let devices = rest.split(',').filter_map(parse_id).collect();
    Some(Seat {
        name: name.to_string(),
        devices,
    })
}

fn parse_id(d: &str) -> Option<(u16, u16)> {
    let (v, p) = d.trim().split_once(':')?;
    Some((
        u16::from_str_radix(v, 16).ok()?,
        u16::from_str_radix(p, 16).ok()?,
    ))
}

fn save(seats: &[Seat]) -> std::io::Result<()> {
    let file = seats_file();
    if let Some(d) = FsPath::new(&file).parent() {
        std::fs::create_dir_all(d)?;
    }
    let mut out = String::new();
    for s in seats {
        let ids: Vec<String> = s
            .devices
            .iter()
            .map(|(v, p)| format!("{v:04x}:{p:04x}"))
            .collect();
        out.push_str(&s.name);
        out.push('\t');
        out.push_str(&ids.join(","));
        out.push('\n');
    }
    std::fs::write(file, out)
}

/// The USB devices belonging to a named seat (for the station create flow).
pub fn devices_of(name: &str) -> Vec<UsbPassthrough> {
    load()
        .into_iter()
        .find(|s| s.name == name)
        .map(|s| {
            s.devices
                .into_iter()
                .map(|(v, p)| UsbPassthrough {
                    vendor_id: v,
                    product_id: p,
                })
                .collect()
        })
        .unwrap_or_default()
}

// ── UI ──────────────────────────────────────────────────────────────────────────────────────

/// The Seats management panel (list + create form); swapped in place by create/delete.
pub fn panel() -> Markup {
    let seats = load();
    let usb = usb::devices();
    let friendly = |v: u16, p: u16| {
        usb.iter()
            .find(|d| d.vendor_id == v && d.product_id == p)
            .and_then(|d| d.product.clone())
            .unwrap_or_else(|| format!("{v:04x}:{p:04x}"))
    };
    html! {
        div #seats {
            div.pad {
                @if seats.is_empty() {
                    p.muted { "No seats yet. A seat groups a player's USB devices so a station picks them up as one." }
                } @else {
                    table {
                        thead { tr { th { "Seat" } th { "Devices" } th.right { "" } } }
                        tbody { @for s in &seats {
                            tr {
                                td.name { (s.name) }
                                td.sub { (s.devices.iter().map(|(v,p)| friendly(*v,*p)).collect::<Vec<_>>().join(", ")) }
                                td.right {
                                    button.btn.sm.danger
                                        hx-post=(format!("/seats/delete?name={}", urlencode(&s.name)))
                                        hx-target="#seats" hx-swap="outerHTML"
                                        hx-confirm=(format!("Delete seat '{}'?", s.name)) { "Delete" }
                                }
                            }
                        } }
                    }
                }
                @if usb.is_empty() {
                    p.sub style="margin-top:12px" { "No USB devices detected to build a seat from." }
                } @else {
                    form hx-post="/seats" hx-target="#seats" hx-swap="outerHTML" style="margin-top:16px; border-top:1px solid var(--line); padding-top:14px" {
                        div.field { label { "New seat name" } input name="name" placeholder="e.g. Living room" required; }
                        div style="margin:8px 0; display:flex; flex-direction:column; gap:6px" {
                            @for d in &usb {
                                @let id = format!("{:04x}:{:04x}", d.vendor_id, d.product_id);
                                @let uid = format!("seat-{id}");
                                div.check {
                                    input type="checkbox" name="usb" value=(id) id=(uid);
                                    label for=(uid) { (d.product.as_deref().unwrap_or("USB device")) " " span.sub.mono { "(" (id) ")" } }
                                }
                            }
                        }
                        button.btn.primary type="submit" { "Create seat" }
                    }
                }
            }
        }
    }
}

pub async fn create(axum::Form(form): axum::Form<Vec<(String, String)>>) -> Markup {
    let name = form
        .iter()
        .find(|(k, _)| k == "name")
        .map(|(_, v)| v.trim().to_string())
        .unwrap_or_default();
    if !name.is_empty() {
        let devices: Vec<(u16, u16)> = form
            .iter()
            .filter(|(k, _)| k == "usb")
            .filter_map(|(_, v)| parse_id(v))
            .collect();
        let mut seats = load();
        seats.retain(|s| s.name != name); // replace if the name already exists
        seats.push(Seat { name, devices });
        let _ = save(&seats);
    }
    panel()
}

#[derive(Deserialize)]
pub struct NameQuery {
    name: String,
}

pub async fn delete(Query(q): Query<NameQuery>) -> Markup {
    let mut seats = load();
    seats.retain(|s| s.name != q.name);
    let _ = save(&seats);
    panel()
}

/// Minimal percent-encoding for a seat name placed in a query string.
fn urlencode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' | b'.' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

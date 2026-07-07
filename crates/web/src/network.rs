//! Network configuration via NetworkManager (`nmcli`).
//!
//! Read-only status (interfaces, routes, DNS) plus editable IPv4 settings per connection: switch a
//! connection between DHCP and a static address/gateway/DNS and apply it live. Applying can briefly
//! drop the link you're managing over — the page warns about that up front.

use axum::extract::Form;
use maud::{html, Markup};
use serde::Deserialize;

use crate::ui;

/// A NetworkManager connection profile (one row of `nmcli connection show`).
struct Conn {
    name: String,
    device: String,
    ctype: String,
}

/// Active, real connections we're willing to edit (skip loopback and the unmanaged/virtual bits).
fn connections() -> Vec<Conn> {
    let out = ui::run_stdout(
        "nmcli",
        &[
            "-t",
            "-f",
            "NAME,DEVICE,TYPE,STATE",
            "connection",
            "show",
            "--active",
        ],
    )
    .unwrap_or_default();
    out.lines()
        .filter_map(|l| {
            let f = split_terse(l);
            let (name, device, ctype, state) = (
                f.first()?,
                f.get(1)?,
                f.get(2)?,
                f.get(3).map(|s| s.as_str()).unwrap_or(""),
            );
            if device == "lo" || ctype == "loopback" || state != "activated" {
                return None;
            }
            Some(Conn {
                name: name.clone(),
                device: device.clone(),
                ctype: ctype.clone(),
            })
        })
        .collect()
}

/// The current IPv4 config of a connection: (method, address CIDR, gateway, dns).
fn ipv4(name: &str) -> (String, String, String, String) {
    let out = ui::run_stdout(
        "nmcli",
        &[
            "-t",
            "-f",
            "ipv4.method,ipv4.addresses,ipv4.gateway,ipv4.dns",
            "connection",
            "show",
            name,
        ],
    )
    .unwrap_or_default();
    let mut method = String::from("auto");
    let (mut addr, mut gw, mut dns) = (String::new(), String::new(), String::new());
    for line in out.lines() {
        // Terse -f output is "key:value"; the key itself has no ':' so split once is safe.
        if let Some((k, v)) = line.split_once(':') {
            let v = v.trim().replace("\\:", ":");
            match k {
                "ipv4.method" => method = v,
                "ipv4.addresses" => addr = v,
                "ipv4.gateway" if v != "--" => gw = v,
                "ipv4.dns" if v != "--" => dns = v.replace(',', " "),
                _ => {}
            }
        }
    }
    (method, addr, gw, dns)
}

/// Split one terse (`-t`) nmcli line on unescaped ':' and unescape the fields.
fn split_terse(line: &str) -> Vec<String> {
    let mut fields = Vec::new();
    let mut cur = String::new();
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' {
            if let Some(&n) = chars.peek() {
                cur.push(n);
                chars.next();
            }
        } else if c == ':' {
            fields.push(std::mem::take(&mut cur));
        } else {
            cur.push(c);
        }
    }
    fields.push(cur);
    fields
}

// ── UI ──────────────────────────────────────────────────────────────────────────────────────

pub async fn page() -> Markup {
    ui::page(
        "network",
        "Network",
        html! {
            div.banner.warn.pad style="margin-bottom:14px" {
                strong { "Heads up: " }
                "changing an interface Tendril is reachable over can drop this page. If that happens, "
                "fix it from the console (" span.mono { "tendril" } " → Configure network) on the host display."
            }
            (ui::panel("IPv4 configuration", None, config_panel()))
            (ui::panel("Interfaces", None, html! {
                pre.pad.mono style="margin:0; overflow-x:auto" { (ui::run_stdout("ip", &["-brief", "addr"]).unwrap_or_default()) }
            }))
            (ui::panel("Routes", None, html! {
                pre.pad.mono style="margin:0; overflow-x:auto" { (ui::run_stdout("ip", &["route"]).unwrap_or_default()) }
            }))
            (ui::panel("DNS (resolv.conf)", None, html! {
                pre.pad.mono style="margin:0; overflow-x:auto" { (dns()) }
            }))
        },
    )
}

/// The editable per-connection IPv4 forms; swapped in place after an apply.
fn config_panel() -> Markup {
    config_panel_note(None)
}

fn config_panel_note(note: Option<Markup>) -> Markup {
    let conns = connections();
    html! {
        div #netconfig {
            @if let Some(n) = note { div.pad style="padding-bottom:0" { (n) } }
            @if conns.is_empty() {
                p.muted.pad { "No active managed connections found (NetworkManager may not be managing this host's interfaces)." }
            } @else {
                div.pad {
                    @for c in &conns {
                        @let (method, addr, gw, dns) = ipv4(&c.name);
                        @let manual = method == "manual";
                        form.netform hx-post="/network/apply" hx-target="#netconfig" hx-swap="outerHTML"
                            hx-confirm=(format!("Apply IPv4 settings to '{}' ({})? This reactivates the connection and may briefly drop the link.", c.name, c.device)) {
                            input type="hidden" name="name" value=(c.name);
                            div.netrow {
                                div.name { (c.name) " " span.sub.mono { "(" (c.device) " · " (c.ctype) ")" } }
                            }
                            div.field {
                                label { "Method" }
                                select name="method" {
                                    option value="auto" selected[!manual] { "Automatic (DHCP)" }
                                    option value="manual" selected[manual] { "Manual (static)" }
                                }
                            }
                            div.netgrid {
                                div.field { label { "Address (CIDR)" } input name="address" placeholder="192.168.1.50/24" value=(addr); }
                                div.field { label { "Gateway" } input name="gateway" placeholder="192.168.1.1" value=(gw); }
                                div.field { label { "DNS (space-separated)" } input name="dns" placeholder="1.1.1.1 8.8.8.8" value=(dns); }
                            }
                            p.sub style="margin:2px 0 10px" { "Address, gateway, and DNS are used only when the method is Manual." }
                            button.btn.primary type="submit" { "Apply to " (c.device) }
                            hr style="border:0; border-top:1px solid var(--line); margin:16px 0 4px";
                        }
                    }
                }
            }
        }
    }
}

fn dns() -> String {
    std::fs::read_to_string("/etc/resolv.conf")
        .unwrap_or_default()
        .lines()
        .filter(|l| l.starts_with("nameserver") || l.starts_with("search"))
        .collect::<Vec<_>>()
        .join("\n")
}

#[derive(Deserialize)]
pub struct ApplyForm {
    name: String,
    method: String,
    #[serde(default)]
    address: String,
    #[serde(default)]
    gateway: String,
    #[serde(default)]
    dns: String,
}

pub async fn apply(Form(f): Form<ApplyForm>) -> Markup {
    let note = match apply_inner(&f) {
        Ok(()) => html! { div.banner.ok { "Applied network settings to " (f.name) "." } },
        Err(e) => html! { div.banner.error { "Failed: " (e) } },
    };
    config_panel_note(Some(note))
}

fn apply_inner(f: &ApplyForm) -> Result<(), String> {
    let name = f.name.as_str();
    if f.method == "manual" {
        let addr = f.address.trim();
        if addr.is_empty() {
            return Err(
                "A static address (CIDR, e.g. 192.168.1.50/24) is required for Manual.".into(),
            );
        }
        // Normalize DNS: accept spaces or commas, hand nmcli a comma-separated list.
        let dns: Vec<&str> = f.dns.split([' ', ',']).filter(|s| !s.is_empty()).collect();
        ui::run_result(
            "nmcli",
            &[
                "connection",
                "modify",
                name,
                "ipv4.method",
                "manual",
                "ipv4.addresses",
                addr,
                "ipv4.gateway",
                f.gateway.trim(),
                "ipv4.dns",
                &dns.join(","),
            ],
        )?;
    } else {
        // Back to DHCP: clear the static fields so they don't linger.
        ui::run_result(
            "nmcli",
            &[
                "connection",
                "modify",
                name,
                "ipv4.method",
                "auto",
                "ipv4.addresses",
                "",
                "ipv4.gateway",
                "",
                "ipv4.dns",
                "",
            ],
        )?;
    }
    // Reactivate so the change takes effect now.
    ui::run_result("nmcli", &["connection", "up", name])?;
    Ok(())
}

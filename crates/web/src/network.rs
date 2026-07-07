//! Network configuration via NetworkManager (`nmcli`).
//!
//! Editable IPv4 settings per physical connection: switch between DHCP and a static
//! address/gateway/DNS and apply it live. Every change runs on a 60-second trial that auto-reverts
//! unless kept (see `apply`), so reconfiguring the link you're connected over is safe. Read-only
//! interface/route/DNS status is tucked behind an Advanced toggle.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use axum::extract::{Form, Query};
use maud::{html, Markup, PreEscaped};
use serde::Deserialize;

use crate::ui;

/// How long a newly-applied config is on trial before it auto-reverts (TrueNAS-style).
const TEST_SECS: u64 = 60;

/// A config change on trial: what to restore if the user doesn't confirm in time. Keyed by
/// connection name. `token` disambiguates overlapping applies to the same connection so a stale
/// timer never reverts a newer change.
struct Pending {
    backup: Ipv4Cfg,
    token: u64,
}

static PENDING: LazyLock<Mutex<HashMap<String, Pending>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
static NEXT_TOKEN: AtomicU64 = AtomicU64::new(1);

/// A connection's IPv4 settings (method plus the static fields), used for backup/restore.
#[derive(Clone)]
struct Ipv4Cfg {
    method: String,
    address: String,
    gateway: String,
    dns: String,
}

/// A NetworkManager connection profile (one row of `nmcli connection show`).
struct Conn {
    name: String,
    device: String,
    ctype: String,
}

/// Active physical connections a user would actually reconfigure — real ethernet/Wi-Fi only. We skip
/// loopback and the software plumbing (podman/docker/libvirt bridges, veth, tun/tap) so the page
/// isn't cluttered with interfaces nobody assigns a static IP to.
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
            // Only real NICs; skip virtual plumbing that happens to be "activated".
            let physical = ctype == "802-3-ethernet" || ctype == "802-11-wireless";
            let virtual_dev = [
                "podman", "docker", "virbr", "veth", "br-", "tun", "tap", "cni",
            ]
            .iter()
            .any(|p| device.starts_with(p));
            if !physical || virtual_dev || state != "activated" {
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

/// The current IPv4 config of a connection.
fn ipv4(name: &str) -> Ipv4Cfg {
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
    let mut cfg = Ipv4Cfg {
        method: String::from("auto"),
        address: String::new(),
        gateway: String::new(),
        dns: String::new(),
    };
    for line in out.lines() {
        // Terse -f output is "key:value"; the key itself has no ':' so split once is safe.
        if let Some((k, v)) = line.split_once(':') {
            let v = v.trim().replace("\\:", ":");
            match k {
                "ipv4.method" => cfg.method = v,
                "ipv4.addresses" => cfg.address = v,
                "ipv4.gateway" if v != "--" => cfg.gateway = v,
                "ipv4.dns" if v != "--" => cfg.dns = v.replace(',', " "),
                _ => {}
            }
        }
    }
    cfg
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
            p.sub style="margin:0 0 14px" {
                "Changes apply on a 60-second trial and revert automatically unless you keep them — "
                "so it's safe to adjust even the connection you're on."
            }
            (ui::panel("IPv4 configuration", None, config_panel()))
            details.advanced style="margin-top:4px" {
                summary { "Advanced: interfaces, routes & DNS (read-only)" }
                (ui::panel("Interfaces", None, html! {
                    pre.pad.mono style="margin:0; overflow-x:auto" { (ui::run_stdout("ip", &["-brief", "addr"]).unwrap_or_default()) }
                }))
                (ui::panel("Routes", None, html! {
                    pre.pad.mono style="margin:0; overflow-x:auto" { (ui::run_stdout("ip", &["route"]).unwrap_or_default()) }
                }))
                (ui::panel("DNS (resolv.conf)", None, html! {
                    pre.pad.mono style="margin:0; overflow-x:auto" { (dns()) }
                }))
            }
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
                        @let cfg = ipv4(&c.name);
                        @let manual = cfg.method == "manual";
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
                                div.field { label { "Address (CIDR)" } input name="address" placeholder="192.168.1.50/24" value=(cfg.address); }
                            }
                            details.advanced style="margin:10px 0" {
                                summary { "Advanced: gateway & DNS" }
                                div.netgrid {
                                    div.field { label { "Gateway" } input name="gateway" placeholder="192.168.1.1" value=(cfg.gateway); }
                                    div.field { label { "DNS (space-separated)" } input name="dns" placeholder="1.1.1.1 8.8.8.8" value=(cfg.dns); }
                                }
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

/// Apply a change *on trial*: back up the current config, apply the new one, then arm a server-side
/// timer that auto-reverts after `TEST_SECS` unless the user clicks "Keep". The revert lives on the
/// server (a spawned task), not the browser — so it still fires if the change cuts off the very link
/// you'd use to confirm.
pub async fn apply(Form(f): Form<ApplyForm>) -> Markup {
    if f.method == "manual" && f.address.trim().is_empty() {
        return config_panel_note(Some(html! {
            div.banner.error { "A static address (CIDR, e.g. 192.168.1.50/24) is required for Manual." }
        }));
    }
    let name = f.name.clone();
    let token = NEXT_TOKEN.fetch_add(1, Ordering::SeqCst);

    // Reserve the trial *before* touching the profile. Capturing the backup and inserting the pending
    // entry under a single lock — ahead of `modify_ipv4`/`up` — means a concurrent apply (double
    // submit, two tabs) reuses the true prior config instead of snapshotting our half-applied change.
    let backup = {
        let mut pend = PENDING.lock().unwrap();
        let backup = pend
            .get(&name)
            .map(|p| p.backup.clone())
            .unwrap_or_else(|| ipv4(&name));
        pend.insert(
            name.clone(),
            Pending {
                backup: backup.clone(),
                token,
            },
        );
        backup
    };

    if let Err(e) = modify_ipv4(&f) {
        abort_trial(&name, token, &backup);
        return config_panel_note(Some(html! { div.banner.error { "Failed: " (e) } }));
    }
    if let Err(e) = ui::run_result("nmcli", &["connection", "up", &name]) {
        // Activation failed — undo our reservation and put the old config back.
        abort_trial(&name, token, &backup);
        return config_panel_note(Some(
            html! { div.banner.error { "Failed to activate: " (e) } },
        ));
    }

    // The trial is armed; the timer below reverts it after TEST_SECS unless confirmed/superseded.
    let revert_name = name.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(TEST_SECS)).await;
        // Revert only if this exact trial is still pending (not kept, not replaced by a newer apply).
        let backup = {
            let mut pend = PENDING.lock().unwrap();
            match pend.get(&revert_name) {
                Some(p) if p.token == token => pend.remove(&revert_name).map(|p| p.backup),
                _ => None,
            }
        };
        if let Some(backup) = backup {
            let _ = tokio::task::spawn_blocking(move || restore(&revert_name, &backup)).await;
        }
    });

    testing_fragment(&name)
}

/// Undo a reserved-but-failed trial: drop our pending entry (only if it's still ours — a newer apply
/// may have superseded it) and restore the backup. No-op if we were superseded.
fn abort_trial(name: &str, token: u64, backup: &Ipv4Cfg) {
    let ours = {
        let mut pend = PENDING.lock().unwrap();
        match pend.get(name) {
            Some(p) if p.token == token => {
                pend.remove(name);
                true
            }
            _ => false,
        }
    };
    if ours {
        let _ = restore(name, backup);
    }
}

/// Write the requested IPv4 settings onto the connection profile (no activation).
fn modify_ipv4(f: &ApplyForm) -> Result<(), String> {
    let name = f.name.as_str();
    if f.method == "manual" {
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
                f.address.trim(),
                "ipv4.gateway",
                f.gateway.trim(),
                "ipv4.dns",
                &dns.join(","),
            ],
        )
        .map(|_| ())
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
        )
        .map(|_| ())
    }
}

/// Restore a connection to a backed-up config and reactivate it.
fn restore(name: &str, b: &Ipv4Cfg) -> Result<(), String> {
    ui::run_result(
        "nmcli",
        &[
            "connection",
            "modify",
            name,
            "ipv4.method",
            &b.method,
            "ipv4.addresses",
            &b.address,
            "ipv4.gateway",
            &b.gateway,
            "ipv4.dns",
            &b.dns.replace(' ', ","),
        ],
    )?;
    ui::run_result("nmcli", &["connection", "up", name])?;
    Ok(())
}

/// The "on trial" fragment: a countdown and Keep / Revert-now buttons. The countdown is cosmetic;
/// the authoritative revert is the server-side timer armed in `apply`.
fn testing_fragment(name: &str) -> Markup {
    let enc = urlencode(name);
    html! {
        div #netconfig {
            div.banner.warn {
                strong { "Testing new settings on " (name) ". " }
                "They revert automatically in " span #netcd { (TEST_SECS) } " s unless you keep them. "
                "If this page stops responding, just wait — you'll be back on the old settings."
            }
            div.pad {
                div.btnrow {
                    button.btn.primary
                        hx-post=(format!("/network/confirm?name={enc}"))
                        hx-target="#netconfig" hx-swap="outerHTML" { "Keep these settings" }
                    button.btn
                        hx-post=(format!("/network/revert?name={enc}"))
                        hx-target="#netconfig" hx-swap="outerHTML" { "Revert now" }
                }
            }
            (PreEscaped(format!(
                "<script>(function(){{var n={TEST_SECS},e=document.getElementById('netcd');\
                 var t=setInterval(function(){{n--;if(e)e.textContent=n;\
                 if(n<=0){{clearInterval(t);location.reload();}}}},1000);}})();</script>"
            )))
        }
    }
}

#[derive(Deserialize)]
pub struct NameQuery {
    name: String,
}

/// Keep the trial config permanently: cancel the pending revert.
pub async fn confirm(Query(q): Query<NameQuery>) -> Markup {
    let kept = PENDING.lock().unwrap().remove(&q.name).is_some();
    let note = if kept {
        html! { div.banner.ok { "Kept the new settings for " (q.name) "." } }
    } else {
        html! { div.banner.warn { "No pending change for " (q.name) " — it may have already reverted." } }
    };
    config_panel_note(Some(note))
}

/// Revert the trial config now instead of waiting out the timer.
pub async fn revert(Query(q): Query<NameQuery>) -> Markup {
    let backup = PENDING.lock().unwrap().remove(&q.name).map(|p| p.backup);
    let note = match backup {
        Some(b) => match restore(&q.name, &b) {
            Ok(()) => html! { div.banner.ok { "Reverted " (q.name) " to the previous settings." } },
            Err(e) => html! { div.banner.error { "Revert failed: " (e) } },
        },
        None => html! { div.banner.warn { "Nothing to revert for " (q.name) "." } },
    };
    config_panel_note(Some(note))
}

/// Minimal percent-encoding for a connection name placed in a query string.
fn urlencode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' | b'.' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

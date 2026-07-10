//! API tokens: long-lived bearer credentials for scripts and monitoring (`Authorization: Bearer
//! tnd_…`), resolved by the auth middleware to the same Admin/Viewer roles as the interactive login.
//!
//! Tokens are `tnd_` + 48 hex chars (24 random bytes) and are shown **once** at creation; only their
//! hex SHA-256 is stored (`/etc/tendril/api-tokens.json`, override `TENDRIL_API_TOKENS`). No
//! constant-time compare needed: the presented token is hashed and its digest looked up — an attacker
//! learns nothing usable from timing a digest comparison of a 192-bit random value.

use argon2::password_hash::rand_core::{OsRng, RngCore};
use maud::{html, Markup};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::auth::Role;
use crate::ui;

fn store_path() -> String {
    std::env::var("TENDRIL_API_TOKENS")
        .unwrap_or_else(|_| "/etc/tendril/api-tokens.json".to_string())
}

/// One stored token: its label, digest, role, and creation stamp — never the token itself.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct TokenRec {
    name: String,
    sha256: String,
    /// "admin" | "viewer" (see [`parse_role`]).
    role: String,
    created: String,
}

fn parse_role(s: &str) -> Option<Role> {
    match s {
        "admin" => Some(Role::Admin),
        "viewer" => Some(Role::Viewer),
        _ => None,
    }
}

fn load() -> Vec<TokenRec> {
    std::fs::read_to_string(store_path())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// Persist the token list (0600 — the digests aren't reversible, but there's no reason to share them).
fn save(recs: &[TokenRec]) -> Result<(), String> {
    let p = store_path();
    if let Some(dir) = std::path::Path::new(&p).parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let json = serde_json::to_string_pretty(recs).map_err(|e| e.to_string())?;
    ui::write_secret(&p, json.as_bytes()).map_err(|e| e.to_string())
}

fn sha256_hex(s: &str) -> String {
    use std::fmt::Write as _;
    let digest = Sha256::digest(s.as_bytes());
    let mut out = String::with_capacity(64);
    for b in digest {
        let _ = write!(out, "{b:02x}");
    }
    out
}

/// A fresh token: `tnd_` + 48 hex chars (24 OsRng bytes — same entropy source as sessions).
fn new_token() -> String {
    use std::fmt::Write as _;
    let mut b = [0u8; 24];
    OsRng.fill_bytes(&mut b);
    let mut s = String::with_capacity(4 + 48);
    s.push_str("tnd_");
    for x in b {
        let _ = write!(s, "{x:02x}");
    }
    s
}

/// Resolve a presented bearer token to `(name, role)`, or `None` for anything unknown/malformed.
/// Called by the auth middleware on every bearer request — one SHA-256 + a small list scan.
pub fn role_for(presented: &str) -> Option<(String, Role)> {
    if !presented.starts_with("tnd_") {
        return None;
    }
    let h = sha256_hex(presented);
    load()
        .into_iter()
        .find(|r| r.sha256 == h)
        .and_then(|r| parse_role(&r.role).map(|role| (r.name, role)))
}

/// Mint a token: store its digest, return the raw token (the only time it exists in the clear).
/// Names are unique — re-using one would make the audit trail ambiguous.
fn create(name: &str, role: Role) -> Result<String, String> {
    let mut recs = load();
    if recs.iter().any(|r| r.name == name) {
        return Err(format!("a token named “{name}” already exists"));
    }
    let token = new_token();
    let (y, m, d, h, mi, s) = ui::utc_now_civil();
    recs.push(TokenRec {
        name: name.to_string(),
        sha256: sha256_hex(&token),
        role: match role {
            Role::Admin => "admin".to_string(),
            Role::Viewer => "viewer".to_string(),
        },
        created: format!("{y:04}-{m:02}-{d:02} {h:02}:{mi:02}:{s:02} UTC"),
    });
    save(&recs)?;
    Ok(token)
}

/// Revoke a token by name. `Ok(false)` when no such token existed.
fn revoke(name: &str) -> Result<bool, String> {
    let mut recs = load();
    let before = recs.len();
    recs.retain(|r| r.name != name);
    if recs.len() == before {
        return Ok(false);
    }
    save(&recs)?;
    Ok(true)
}

// ── handlers + panel ─────────────────────────────────────────────────────────────────────────

/// Token names land in the audit log (tab-delimited) and the JSON store — same charset as station
/// names, so they can't inject columns or look like paths.
fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

#[derive(Deserialize)]
pub struct CreateForm {
    #[serde(default)]
    name: String,
    #[serde(default)]
    role: String,
}

/// Create a token (POST /system/tokens) and show it once in the response banner.
pub async fn create_action(axum::Form(f): axum::Form<CreateForm>) -> Markup {
    if ui::is_demo() {
        return body_with(Some(
            html! { div.banner.warn style="margin:0 0 10px" { "Disabled in the live demo." } },
        ));
    }
    let name = f.name.trim();
    if !valid_name(name) {
        return body_with(Some(
            html! { div.banner.error style="margin:0 0 10px" { "Token name may only contain letters, numbers, - _ . (max 64)." } },
        ));
    }
    let Some(role) = parse_role(&f.role) else {
        return body_with(Some(
            html! { div.banner.error style="margin:0 0 10px" { "Pick a role: admin or viewer." } },
        ));
    };
    match create(name, role) {
        Ok(token) => {
            crate::auth::audit("admin", &format!("token-create {name}"), 200);
            body_with(Some(html! {
                div.banner.ok style="margin:0 0 10px" {
                    "Token " b { (name) } " created — copy it now, it isn't stored:"
                    pre.mono style="margin:8px 0 0; padding:8px 10px; background:var(--bg2,#0002); border-radius:6px; overflow-x:auto; font-size:12px; word-break:break-all" { (token) }
                }
            }))
        }
        Err(e) => body_with(Some(
            html! { div.banner.error style="margin:0 0 10px" { "Couldn't create the token: " (e) } },
        )),
    }
}

#[derive(Deserialize)]
pub struct RevokeForm {
    #[serde(default)]
    name: String,
}

/// Revoke a token by name (POST /system/tokens/revoke).
pub async fn revoke_action(axum::Form(f): axum::Form<RevokeForm>) -> Markup {
    if ui::is_demo() {
        return body_with(Some(
            html! { div.banner.warn style="margin:0 0 10px" { "Disabled in the live demo." } },
        ));
    }
    let name = f.name.trim().to_string();
    let banner = match revoke(&name) {
        Ok(true) => {
            crate::auth::audit("admin", &format!("token-revoke {name}"), 200);
            html! { div.banner.ok style="margin:0 0 10px" { "Token " b { (name) } " revoked." } }
        }
        Ok(false) => {
            html! { div.banner.warn style="margin:0 0 10px" { "No token by that name." } }
        }
        Err(e) => {
            html! { div.banner.error style="margin:0 0 10px" { "Couldn't revoke: " (e) } }
        }
    };
    body_with(Some(banner))
}

/// The "API tokens" panel for the System page.
pub fn panel() -> Markup {
    ui::panel(
        "API tokens",
        Some("bearer credentials for scripts & monitoring"),
        body_with(None),
    )
}

fn body_with(banner: Option<Markup>) -> Markup {
    let recs = load();
    html! {
        div.pad #tokens-panel {
            @if let Some(b) = banner { (b) }
            p.sub style="margin:0 0 10px" {
                "Long-lived credentials for the HTTP API — send "
                span.mono { "Authorization: Bearer tnd_…" } " on any request. An " b { "admin" }
                " token can do everything; a " b { "viewer" } " token is read-only (handy for scraping "
                a href="/metrics" { span.mono { "/metrics" } } ")."
            }
            @if recs.is_empty() {
                p.sub style="margin:0 0 10px" { "No tokens yet." }
            } @else {
                div.scroll { table {
                    thead { tr { th { "Name" } th { "Role" } th { "Created" } th.right { "" } } }
                    tbody { @for r in &recs {
                        tr {
                            td.mono { (r.name) }
                            td { span.badge { (r.role) } }
                            td.sub { (r.created) }
                            td.right {
                                form hx-post="/system/tokens/revoke" hx-target="#tokens-panel" hx-swap="outerHTML"
                                    hx-confirm=(format!("Revoke token '{}'? Anything using it loses access immediately.", r.name))
                                    style="display:inline" {
                                    input type="hidden" name="name" value=(r.name);
                                    button.btn.sm.danger type="submit" { "Revoke" }
                                }
                            }
                        }
                    } }
                } }
            }
            form hx-post="/system/tokens" hx-target="#tokens-panel" hx-swap="outerHTML"
                style="display:flex; gap:8px; align-items:flex-end; flex-wrap:wrap; margin-top:12px" {
                div.field { label { "Name" } input name="name" placeholder="grafana" style="width:14em"; }
                div.field { label { "Role" }
                    select name="role" style="width:10em" {
                        option value="viewer" { "viewer (read-only)" }
                        option value="admin" { "admin" }
                    }
                }
                button.btn.primary type="submit" { "Create token" }
            }
            p.sub style="margin:10px 0 0" { "The token is shown once at creation — only its SHA-256 is stored." }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_matches_known_vector() {
        assert_eq!(
            sha256_hex("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn token_shape() {
        let t = new_token();
        assert!(t.starts_with("tnd_"));
        assert_eq!(t.len(), 4 + 48);
        assert!(t[4..].chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(t, new_token());
    }

    #[test]
    fn name_charset() {
        assert!(valid_name("grafana-01"));
        assert!(!valid_name(""));
        assert!(!valid_name("has space"));
        assert!(!valid_name("tab\tname"));
        assert!(!valid_name(&"x".repeat(65)));
    }

    /// One test drives the whole store lifecycle — the store path is process-global env state, so
    /// splitting these into parallel tests would race on it.
    #[test]
    fn create_resolve_revoke_round_trip() {
        let p =
            std::env::temp_dir().join(format!("tendril-tokens-test-{}.json", std::process::id()));
        std::env::set_var("TENDRIL_API_TOKENS", &p);
        let _ = std::fs::remove_file(&p);

        let admin = create("ops", Role::Admin).unwrap();
        let viewer = create("grafana", Role::Viewer).unwrap();
        // Duplicate names are refused.
        assert!(create("ops", Role::Viewer).is_err());
        // Presented tokens resolve to their name + role; garbage doesn't.
        assert_eq!(role_for(&admin), Some(("ops".to_string(), Role::Admin)));
        assert_eq!(
            role_for(&viewer),
            Some(("grafana".to_string(), Role::Viewer))
        );
        assert_eq!(role_for("tnd_nope"), None);
        assert_eq!(role_for("Bearer whatever"), None);
        // Revoking kills resolution; revoking again reports "not found".
        assert_eq!(revoke("ops"), Ok(true));
        assert_eq!(role_for(&admin), None);
        assert_eq!(revoke("ops"), Ok(false));
        assert_eq!(
            role_for(&viewer),
            Some(("grafana".to_string(), Role::Viewer))
        );

        let _ = std::fs::remove_file(&p);
    }
}

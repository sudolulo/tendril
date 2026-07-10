//! Named admin/viewer accounts — an **additive** layer over the two legacy shared passwords.
//!
//! The single admin password (`webauth`) and the viewer password stay the primary credentials (and
//! the whole setup / seeded-default flow); named users just give each person their own sign-in so
//! the audit log records *who* made a change instead of a shared "admin".
//!
//! The store is `/etc/tendril/users.json` (override `TENDRIL_USERS`): a list of
//! `{name, hash, role, created}` records, where `hash` is an Argon2 PHC string (the same
//! hash/verify core as the legacy passwords — see `auth::hash_str` / `auth::verify_str`).
//! Written 0600 — password hashes aren't for sharing.

use maud::{html, Markup};
use serde::{Deserialize, Serialize};

use crate::auth::Role;
use crate::ui;

fn store_path() -> String {
    std::env::var("TENDRIL_USERS").unwrap_or_else(|_| "/etc/tendril/users.json".to_string())
}

/// One named account: name, Argon2 PHC hash, role ("admin" | "viewer"), creation stamp.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct UserRec {
    name: String,
    hash: String,
    role: String,
    created: String,
}

fn load() -> Vec<UserRec> {
    std::fs::read_to_string(store_path())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// Persist the user list, 0600 from the first byte (creating the parent dir).
fn save(recs: &[UserRec]) -> Result<(), String> {
    let p = store_path();
    if let Some(dir) = std::path::Path::new(&p).parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let json = serde_json::to_string_pretty(recs).map_err(|e| e.to_string())?;
    ui::write_secret(&p, json.as_bytes()).map_err(|e| e.to_string())
}

/// Verify a named user's password: `Some(role)` when the name exists and the password matches its
/// stored hash. Called by the login handler when the username field is filled in.
pub(crate) fn verify(name: &str, pw: &str) -> Option<Role> {
    load()
        .into_iter()
        .find(|u| u.name == name)
        .filter(|u| crate::auth::verify_str(&u.hash, pw))
        .and_then(|u| Role::parse(&u.role))
}

/// The built-in login labels — a named user with one of these would make the audit trail ambiguous
/// (and shadow the legacy flows in people's heads), so they're refused.
const RESERVED: [&str; 2] = ["admin", "viewer"];

/// Usernames land in the audit log (tab-delimited) and the JSON store — same charset rule as
/// station names (reusing that validator), capped like token names.
fn valid_name(name: &str) -> bool {
    name.len() <= 64 && crate::stations::valid_station_name(name)
}

/// Validate an add request without touching the store: charset, reserved names, password length.
fn validate_new(name: &str, pw: &str, role: &str) -> Result<Role, String> {
    if !valid_name(name) {
        return Err("Username may only contain letters, numbers, - _ . (max 64).".into());
    }
    if RESERVED.contains(&name) {
        return Err(format!(
            "“{name}” is reserved for the built-in logins — pick another name."
        ));
    }
    if pw.chars().count() < 6 {
        return Err("Use a password of at least 6 characters.".into());
    }
    Role::parse(role).ok_or_else(|| "Pick a role: admin or viewer.".into())
}

/// Add a named user (validated + hashed). Duplicate names are refused.
fn add(name: &str, pw: &str, role: Role) -> Result<(), String> {
    let mut recs = load();
    if recs.iter().any(|u| u.name == name) {
        return Err(format!("a user named “{name}” already exists"));
    }
    let hash = crate::auth::hash_str(pw)?;
    let (y, m, d, h, mi, s) = ui::utc_now_civil();
    recs.push(UserRec {
        name: name.to_string(),
        hash,
        role: role.as_str().to_string(),
        created: format!("{y:04}-{m:02}-{d:02} {h:02}:{mi:02}:{s:02} UTC"),
    });
    save(&recs)
}

/// Remove a named user by name. `Ok(false)` when no such user existed.
fn remove(name: &str) -> Result<bool, String> {
    let mut recs = load();
    let before = recs.len();
    recs.retain(|u| u.name != name);
    if recs.len() == before {
        return Ok(false);
    }
    save(&recs)?;
    Ok(true)
}

// ── handlers + panel section ─────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct AddForm {
    #[serde(default)]
    name: String,
    #[serde(default)]
    password: String,
    #[serde(default)]
    role: String,
}

/// Add a named user (POST /system/users) and re-render the section.
pub async fn add_action(axum::Form(f): axum::Form<AddForm>) -> Markup {
    if ui::is_demo() {
        return section_with(Some(
            html! { div.banner.warn style="margin:0 0 10px" { "Disabled in the live demo." } },
        ));
    }
    let name = f.name.trim();
    let role = match validate_new(name, &f.password, &f.role) {
        Ok(r) => r,
        Err(e) => {
            return section_with(Some(
                html! { div.banner.error style="margin:0 0 10px" { (e) } },
            ))
        }
    };
    match add(name, &f.password, role) {
        Ok(()) => {
            crate::auth::audit(
                "admin",
                &format!("user-add {name} ({})", role.as_str()),
                200,
            );
            section_with(Some(html! {
                div.banner.ok style="margin:0 0 10px" {
                    "User " b { (name) } " added — they sign in with their username and password."
                }
            }))
        }
        Err(e) => section_with(Some(
            html! { div.banner.error style="margin:0 0 10px" { "Couldn't add the user: " (e) } },
        )),
    }
}

#[derive(Deserialize)]
pub struct RemoveForm {
    #[serde(default)]
    name: String,
}

/// Remove a named user (POST /system/users/remove) and sweep their live sessions so the removal
/// takes effect immediately.
pub async fn remove_action(axum::Form(f): axum::Form<RemoveForm>) -> Markup {
    if ui::is_demo() {
        return section_with(Some(
            html! { div.banner.warn style="margin:0 0 10px" { "Disabled in the live demo." } },
        ));
    }
    let name = f.name.trim().to_string();
    let banner = match remove(&name) {
        Ok(true) => {
            crate::auth::revoke_sessions_for(&name);
            crate::auth::audit("admin", &format!("user-remove {name}"), 200);
            html! { div.banner.ok style="margin:0 0 10px" { "User " b { (name) } " removed and signed out." } }
        }
        Ok(false) => {
            html! { div.banner.warn style="margin:0 0 10px" { "No user by that name." } }
        }
        Err(e) => {
            html! { div.banner.error style="margin:0 0 10px" { "Couldn't remove: " (e) } }
        }
    };
    section_with(Some(banner))
}

/// The "Named users" section of the System page's Access & audit panel.
pub(crate) fn section() -> Markup {
    section_with(None)
}

fn section_with(banner: Option<Markup>) -> Markup {
    let recs = load();
    html! {
        div #users-section style="margin-top:16px; padding-top:14px; border-top:1px solid var(--line)" {
            @if let Some(b) = banner { (b) }
            div.sub style="font-weight:600; margin-bottom:4px" { "Named users" }
            p.sub style="margin:0 0 8px" {
                "Personal sign-ins with their own password and role, so the audit log records "
                b { "who" } " made each change. The main admin and viewer passwords above keep "
                "working unchanged. Removing a user signs out their active sessions immediately."
            }
            @if recs.is_empty() {
                p.sub style="margin:0 0 10px" { "No named users yet." }
            } @else {
                div.scroll { table {
                    thead { tr { th { "Name" } th { "Role" } th { "Created" } th.right { "" } } }
                    tbody { @for u in &recs {
                        tr {
                            td.mono { (u.name) }
                            td { span.badge { (u.role) } }
                            td.sub { (u.created) }
                            td.right {
                                form hx-post="/system/users/remove" hx-target="#users-section" hx-swap="outerHTML"
                                    hx-confirm=(format!("Remove user '{}'? Their active sessions are signed out immediately.", u.name))
                                    style="display:inline" {
                                    input type="hidden" name="name" value=(u.name);
                                    button.btn.sm.danger type="submit" { "Remove" }
                                }
                            }
                        }
                    } }
                } }
            }
            form hx-post="/system/users" hx-target="#users-section" hx-swap="outerHTML"
                style="display:flex; gap:8px; align-items:flex-end; flex-wrap:wrap; margin-top:12px" {
                div.field { label { "Username" } input name="name" placeholder="alice" style="width:12em"; }
                div.field { label { "Password" } input type="password" name="password" style="width:12em"; }
                div.field { label { "Role" }
                    select name="role" style="width:10em" {
                        option value="viewer" { "viewer (read-only)" }
                        option value="admin" { "admin" }
                    }
                }
                button.btn.primary type="submit" { "Add user" }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_validation() {
        // Reserved names collide with the built-in logins.
        assert!(validate_new("admin", "longenough", "admin").is_err());
        assert!(validate_new("viewer", "longenough", "viewer").is_err());
        // Charset: same rule as station names; length capped.
        assert!(validate_new("has space", "longenough", "admin").is_err());
        assert!(validate_new("tab\tname", "longenough", "admin").is_err());
        assert!(validate_new("-leading-dash", "longenough", "admin").is_err());
        assert!(validate_new("", "longenough", "admin").is_err());
        assert!(validate_new(&"x".repeat(65), "longenough", "admin").is_err());
        // Password minimum + role parsing.
        assert!(validate_new("alice", "short", "admin").is_err());
        assert!(validate_new("alice", "longenough", "root").is_err());
        assert_eq!(
            validate_new("alice", "longenough", "admin"),
            Ok(Role::Admin)
        );
        assert_eq!(
            validate_new("bob-2.ops_1", "longenough", "viewer"),
            Ok(Role::Viewer)
        );
    }

    /// One test drives the whole store lifecycle — the store path is process-global env state, so
    /// splitting these into parallel tests would race on it (same pattern as the API-token tests).
    #[test]
    fn add_verify_remove_round_trip() {
        let p =
            std::env::temp_dir().join(format!("tendril-users-test-{}.json", std::process::id()));
        std::env::set_var("TENDRIL_USERS", &p);
        let _ = std::fs::remove_file(&p);

        add("alice", "correct-horse", Role::Admin).unwrap();
        add("bob", "battery-staple", Role::Viewer).unwrap();
        // Duplicate names are refused.
        assert!(add("alice", "whatever-else", Role::Viewer).is_err());
        // Right password → the stored role; wrong password / unknown user → None.
        assert_eq!(verify("alice", "correct-horse"), Some(Role::Admin));
        assert_eq!(verify("bob", "battery-staple"), Some(Role::Viewer));
        assert_eq!(verify("alice", "battery-staple"), None);
        assert_eq!(verify("mallory", "correct-horse"), None);
        // Removal kills verification; removing again reports "not found".
        assert_eq!(remove("alice"), Ok(true));
        assert_eq!(verify("alice", "correct-horse"), None);
        assert_eq!(remove("alice"), Ok(false));
        assert_eq!(verify("bob", "battery-staple"), Some(Role::Viewer));

        let _ = std::fs::remove_file(&p);
    }
}

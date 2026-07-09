//! Authentication: a single admin password (Argon2-hashed) with server-side sessions, plus an
//! optional "trust reverse-proxy" mode for SSO front-ends (Authelia/NPM).
//!
//! - No password set yet → every request redirects to `/setup` to create the admin password.
//! - Otherwise → requests without a valid session cookie redirect to `/login`.
//! - If `TENDRIL_TRUST_PROXY_HEADER` names a header the proxy sets (e.g. `Remote-User`), a request
//!   carrying it non-empty is treated as authenticated and Tendril's own login is bypassed.
//!
//! Sessions live in memory (a fresh login after a service restart is fine for an appliance).

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use argon2::password_hash::rand_core::{OsRng, RngCore};
use argon2::password_hash::SaltString;
use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier};
use axum::extract::Request;
use axum::http::header::{COOKIE, SET_COOKIE};
use axum::middleware::Next;
use axum::response::{IntoResponse, Redirect, Response};
use axum::Form;
use maud::{html, Markup, PreEscaped, DOCTYPE};
use serde::Deserialize;

use crate::ui;

const SESSION_TTL: Duration = Duration::from_secs(24 * 3600);
static SESSIONS: LazyLock<Mutex<HashMap<String, (Instant, Role)>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// A signed-in principal's access level. Admin can do everything; Viewer is read-only (mutating
/// requests are refused, like the public demo) — a way to hand out visibility without the admin secret.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Admin,
    Viewer,
}

impl Role {
    fn as_str(self) -> &'static str {
        match self {
            Role::Admin => "admin",
            Role::Viewer => "viewer",
        }
    }
}

// ── password store ──────────────────────────────────────────────────────────────────────────

fn auth_file() -> String {
    std::env::var("TENDRIL_AUTH_FILE").unwrap_or_else(|_| "/etc/tendril/webauth".to_string())
}

pub fn is_configured() -> bool {
    read_hash().is_some()
}

fn read_hash() -> Option<String> {
    std::fs::read_to_string(auth_file())
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Marker file (beside the auth file) meaning the current admin password is a **baked default** — set
/// by an unattended install — that the user must replace before using the console. Any real password
/// change clears it.
fn default_marker() -> String {
    format!("{}.default", auth_file())
}

/// True when the admin password is a default that must be changed before the console is usable.
pub fn password_is_default() -> bool {
    std::path::Path::new(&default_marker()).exists()
}

/// Flag the current password as a must-change default (called by the unattended-install seed path).
pub fn mark_password_default() {
    let _ = std::fs::write(
        default_marker(),
        "seeded by unattended install — change on first login\n",
    );
}

/// Hash and store a new admin password (file mode 0600).
pub fn set_password(pw: &str) -> std::io::Result<()> {
    let salt = SaltString::generate(&mut OsRng);
    let hash = Argon2::default()
        .hash_password(pw.as_bytes(), &salt)
        .map_err(|e| std::io::Error::other(e.to_string()))?
        .to_string();
    let file = auth_file();
    if let Some(dir) = std::path::Path::new(&file).parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&file, hash)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600));
    }
    // A real password is now set — it's no longer the baked default.
    let _ = std::fs::remove_file(default_marker());
    Ok(())
}

fn verify_password(pw: &str) -> bool {
    let Some(stored) = read_hash() else {
        return false;
    };
    let Ok(parsed) = PasswordHash::new(&stored) else {
        return false;
    };
    Argon2::default()
        .verify_password(pw.as_bytes(), &parsed)
        .is_ok()
}

// ── viewer (read-only) credential ──────────────────────────────────────────────────────────────

fn viewer_file() -> String {
    format!("{}.viewer", auth_file())
}

/// Whether a read-only viewer password is set.
pub fn viewer_configured() -> bool {
    std::fs::read_to_string(viewer_file())
        .ok()
        .map(|s| !s.trim().is_empty())
        .unwrap_or(false)
}

/// Hash and store the read-only viewer password (file mode 0600).
pub fn set_viewer_password(pw: &str) -> std::io::Result<()> {
    let salt = SaltString::generate(&mut OsRng);
    let hash = Argon2::default()
        .hash_password(pw.as_bytes(), &salt)
        .map_err(|e| std::io::Error::other(e.to_string()))?
        .to_string();
    let file = viewer_file();
    if let Some(dir) = std::path::Path::new(&file).parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&file, hash)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

/// Remove the viewer password (disables read-only sign-in).
pub fn clear_viewer_password() {
    let _ = std::fs::remove_file(viewer_file());
}

fn verify_viewer(pw: &str) -> bool {
    let Ok(stored) = std::fs::read_to_string(viewer_file()) else {
        return false;
    };
    let stored = stored.trim();
    if stored.is_empty() {
        return false;
    }
    let Ok(parsed) = PasswordHash::new(stored) else {
        return false;
    };
    Argon2::default()
        .verify_password(pw.as_bytes(), &parsed)
        .is_ok()
}

// ── audit log ────────────────────────────────────────────────────────────────────────────────

fn audit_path() -> String {
    std::env::var("TENDRIL_AUDIT_FILE").unwrap_or_else(|_| "/var/lib/tendril/audit.log".to_string())
}

/// Append an audit record — `timestamp \t actor \t action \t status`. Best-effort (never fails a
/// request).
pub fn audit(actor: &str, action: &str, status: u16) {
    use std::io::Write as _;
    let line = format!("{}\t{}\t{}\t{}\n", now_utc_string(), actor, action, status);
    let path = audit_path();
    if let Some(dir) = std::path::Path::new(&path).parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let _ = f.write_all(line.as_bytes());
    }
}

/// The most recent `n` audit lines, newest first.
pub fn audit_tail(n: usize) -> Vec<String> {
    let Ok(s) = std::fs::read_to_string(audit_path()) else {
        return Vec::new();
    };
    let mut lines: Vec<String> = s
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(String::from)
        .collect();
    lines.reverse();
    lines.truncate(n);
    lines
}

/// Format the current time as `YYYY-MM-DD HH:MM:SS UTC` without a date crate (civil-from-days).
fn now_utc_string() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let (days, rem) = (secs / 86400, secs % 86400);
    let (h, mi, s) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    let z = days as i64 + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02} {h:02}:{mi:02}:{s:02} UTC")
}

// ── sessions ────────────────────────────────────────────────────────────────────────────────

fn new_token() -> String {
    use std::fmt::Write as _;
    let mut b = [0u8; 32];
    OsRng.fill_bytes(&mut b);
    let mut s = String::with_capacity(64);
    for x in b {
        let _ = write!(s, "{x:02x}");
    }
    s
}

fn create_session(role: Role) -> String {
    let token = new_token();
    let mut s = SESSIONS.lock().unwrap();
    // Sweep expired sessions so the map can't grow unbounded across many logins.
    let now = Instant::now();
    s.retain(|_, (exp, _)| *exp > now);
    s.insert(token.clone(), (now + SESSION_TTL, role));
    token
}

/// The role for a session token if it's valid + unexpired, else `None` (expired tokens are evicted).
fn session_role(token: &str) -> Option<Role> {
    let mut s = SESSIONS.lock().unwrap();
    match s.get(token) {
        Some(&(exp, role)) if exp > Instant::now() => Some(role),
        Some(_) => {
            s.remove(token);
            None
        }
        None => None,
    }
}

fn session_cookie(token: &str, max_age: i64) -> String {
    // Mark Secure when we're serving TLS, so the session token never rides a plaintext hop.
    let secure = if crate::tls::enabled() {
        "; Secure"
    } else {
        ""
    };
    format!("tendril_session={token}; HttpOnly; SameSite=Lax; Path=/; Max-Age={max_age}{secure}")
}

fn cookie_token(req: &Request) -> Option<String> {
    let cookies = req.headers().get(COOKIE)?.to_str().ok()?;
    cookies
        .split(';')
        .map(str::trim)
        .find_map(|c| c.strip_prefix("tendril_session=").map(str::to_string))
}

/// The role a set of request headers authenticates as, or `None`. A trusted reverse-proxy header
/// authenticates as Admin (the proxy owns access control); otherwise the session cookie carries the
/// role.
pub fn role_from_headers(headers: &axum::http::HeaderMap) -> Option<Role> {
    if let Ok(name) = std::env::var("TENDRIL_TRUST_PROXY_HEADER") {
        if !name.is_empty() {
            if let Some(v) = headers.get(&name) {
                if v.to_str().map(|s| !s.is_empty()).unwrap_or(false) {
                    return Some(Role::Admin);
                }
            }
        }
    }
    let cookies = headers.get(COOKIE)?.to_str().ok()?;
    let token = cookies
        .split(';')
        .map(str::trim)
        .find_map(|c| c.strip_prefix("tendril_session="))?;
    session_role(token)
}

fn role_of(req: &Request) -> Option<Role> {
    role_from_headers(req.headers())
}

/// Whether these request headers authenticate as an admin — used to hide secret UI (fleet token, join
/// code) from read-only viewers on otherwise-readable pages.
pub fn is_admin(headers: &axum::http::HeaderMap) -> bool {
    role_from_headers(headers) == Some(Role::Admin)
}

/// The actor label for the audit log (proxy-set user if present, else the role name).
fn actor_of(req: &Request) -> String {
    if let Ok(name) = std::env::var("TENDRIL_TRUST_PROXY_HEADER") {
        if !name.is_empty() {
            if let Some(u) = req.headers().get(&name).and_then(|v| v.to_str().ok()) {
                if !u.is_empty() {
                    // Strip control chars so a proxy-set user can't inject columns/lines into the
                    // tab-delimited audit log.
                    return u.chars().filter(|c| !c.is_control()).collect();
                }
            }
        }
    }
    role_of(req)
        .map(|r| r.as_str().to_string())
        .unwrap_or_else(|| "anon".to_string())
}

// ── middleware ──────────────────────────────────────────────────────────────────────────────

/// Gate every request: allow the auth endpoints and assets; force `/setup` until a password exists;
/// otherwise require a session (or a trusted proxy header).
pub async fn require_auth(req: Request, next: Next) -> Response {
    let path = req.uri().path();
    // Public read-only demo: skip login, and turn every mutating request (POST) into a no-op that
    // returns a friendly banner — so the instance is safe to expose behind a proxy.
    if ui::is_demo() {
        if req.method() == axum::http::Method::POST {
            let banner = r#"<div class="banner warn" style="margin:0">🎭 This is a live demo — actions are disabled. <a href="https://git.onetick.ninja/flan/tendril">Run Tendril</a> to use it for real.</div>"#;
            return (
                [(axum::http::header::CONTENT_TYPE, "text/html; charset=utf-8")],
                banner,
            )
                .into_response();
        }
        return next.run(req).await;
    }
    let open =
        path.starts_with("/assets/") || path == "/login" || path == "/logout" || path == "/setup";
    // A peer node calling our federation API authenticates with the shared federation token.
    let federation_api = path.starts_with("/api/")
        && req
            .headers()
            .get("X-Tendril-Federation")
            .and_then(|v| v.to_str().ok())
            .is_some_and(crate::federation::token_ok);
    if federation_api {
        return next.run(req).await;
    }
    // A baked default password (unattended install) must be changed before anything else — force
    // `/setup` even for an otherwise-valid session, so the default is never usable in practice.
    if password_is_default() {
        return if open {
            next.run(req).await
        } else {
            Redirect::to("/setup").into_response()
        };
    }
    let role = role_of(&req);
    if !open && role.is_none() {
        // Not authenticated: first run has no password → set one; otherwise sign in.
        return if !is_configured() {
            Redirect::to("/setup").into_response()
        } else {
            Redirect::to("/login").into_response()
        };
    }
    let is_post = req.method() == axum::http::Method::POST;
    // A few GETs return fleet trust material (the join code embeds the shared token + the fleet CA
    // *private key*), so they're admin-only even though they're reads.
    let sensitive_get = matches!(path, "/fleet/join-code");
    // Viewer is read-only: refuse mutations (and secret-returning GETs) with a friendly banner.
    if !open && role == Some(Role::Viewer) && (is_post || sensitive_get) {
        let banner = r#"<div class="banner warn" style="margin:0">👁 Read-only access — sign in as an admin to make changes.</div>"#;
        return (
            [(axum::http::header::CONTENT_TYPE, "text/html; charset=utf-8")],
            banner,
        )
            .into_response();
    }
    // Audit admin mutations (after they run, with the outcome).
    if !open && is_post {
        let actor = actor_of(&req);
        let action = format!("POST {path}");
        let resp = next.run(req).await;
        audit(&actor, &action, resp.status().as_u16());
        return resp;
    }
    next.run(req).await
}

// ── handlers ────────────────────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct LoginForm {
    password: String,
}

#[derive(Deserialize)]
pub struct SetupForm {
    password: String,
    confirm: String,
}

pub async fn login_page() -> Markup {
    render(
        "Sign in",
        None,
        html! {
            form method="post" action="/login" {
                div.field { label { "Admin password" } input type="password" name="password" autofocus required; }
                button.btn.primary type="submit" style="width:100%; margin-top:6px" { "Sign in" }
            }
        },
    )
}

pub async fn login(Form(f): Form<LoginForm>) -> Response {
    let role = if verify_password(&f.password) {
        Some(Role::Admin)
    } else if verify_viewer(&f.password) {
        Some(Role::Viewer)
    } else {
        None
    };
    if let Some(role) = role {
        let token = create_session(role);
        audit(role.as_str(), "login", 200);
        (
            [(
                SET_COOKIE,
                session_cookie(&token, SESSION_TTL.as_secs() as i64),
            )],
            Redirect::to("/"),
        )
            .into_response()
    } else {
        // Throttle brute force: a fixed delay on every failed attempt caps guesses to a couple per
        // second (Argon2's cost is the only other brake).
        audit("anon", "login-fail", 401);
        tokio::time::sleep(std::time::Duration::from_millis(750)).await;
        render(
            "Sign in",
            Some("Incorrect password."),
            html! {
                form method="post" action="/login" {
                    div.field { label { "Admin password" } input type="password" name="password" autofocus required; }
                    button.btn.primary type="submit" style="width:100%; margin-top:6px" { "Sign in" }
                }
            },
        )
        .into_response()
    }
}

pub async fn logout(req: Request) -> Response {
    if let Some(t) = cookie_token(&req) {
        SESSIONS.lock().unwrap().remove(&t);
    }
    (
        [(SET_COOKIE, session_cookie("", 0))],
        Redirect::to("/login"),
    )
        .into_response()
}

pub async fn setup_page() -> Response {
    // A configured, non-default password → nothing to set up. A default one still needs replacing.
    if is_configured() && !password_is_default() {
        return Redirect::to("/login").into_response();
    }
    setup_form(None).into_response()
}

pub async fn setup(Form(f): Form<SetupForm>) -> Response {
    if is_configured() && !password_is_default() {
        return Redirect::to("/login").into_response();
    }
    if f.password.chars().count() < 6 {
        return setup_form(Some("Use at least 6 characters.")).into_response();
    }
    if f.password != f.confirm {
        return setup_form(Some("Passwords don't match.")).into_response();
    }
    match set_password(&f.password) {
        Ok(()) => {
            let token = create_session(Role::Admin);
            (
                [(
                    SET_COOKIE,
                    session_cookie(&token, SESSION_TTL.as_secs() as i64),
                )],
                Redirect::to("/"),
            )
                .into_response()
        }
        Err(e) => setup_form(Some(&format!("Could not save the password: {e}"))).into_response(),
    }
}

fn setup_form(error: Option<&str>) -> Markup {
    let default = password_is_default();
    render(
        if default {
            "Change the default password"
        } else {
            "Welcome to Tendril"
        },
        error,
        html! {
            p.sub style="margin:-6px 0 14px" {
                @if default {
                    "This node shipped with a default admin password (unattended install). Choose a new one before continuing."
                } @else {
                    "Set an admin password to secure the control plane."
                }
            }
            form method="post" action="/setup" {
                div.field { label { "New password" } input type="password" name="password" autofocus required; }
                div.field { label { "Confirm" } input type="password" name="confirm" required; }
                button.btn.primary type="submit" style="width:100%; margin-top:6px" {
                    (if default { "Change & sign in" } else { "Create & sign in" })
                }
            }
        },
    )
}

#[derive(Deserialize)]
pub struct ChangePwForm {
    current: String,
    new: String,
    confirm: String,
}

/// Change the admin password from the console (verifies the current one first). Returns a result
/// banner, HTMX-swapped into the System page panel.
pub async fn change_password(Form(f): Form<ChangePwForm>) -> Markup {
    if ui::is_demo() {
        return html! { div.banner.warn style="margin:0" { "Disabled in the live demo." } };
    }
    if !verify_password(&f.current) {
        return html! { div.banner.error style="margin:0" { "Current password is incorrect." } };
    }
    if f.new.chars().count() < 6 {
        return html! { div.banner.error style="margin:0" { "New password must be at least 6 characters." } };
    }
    if f.new != f.confirm {
        return html! { div.banner.error style="margin:0" { "New password and confirmation don't match." } };
    }
    match set_password(&f.new) {
        Ok(()) => html! { div.banner.ok style="margin:0" { "Admin password updated." } },
        Err(e) => {
            html! { div.banner.error style="margin:0" { "Couldn't save the new password: " (e) } }
        }
    }
}

/// The "Admin password" panel body for the System page.
pub fn password_panel() -> Markup {
    html! {
        div.pad {
            p.sub style="margin:0 0 10px" { "Change the password you use to sign in to this web console." }
            form.grid hx-post="/system/password" hx-target="#pw-result" hx-swap="innerHTML" {
                div.field.wide { label { "Current password" } input type="password" name="current" required; }
                div.field { label { "New password" } input type="password" name="new" required; }
                div.field { label { "Confirm new password" } input type="password" name="confirm" required; }
                div.field.wide { div.btnrow { button.btn.primary type="submit" { "Change password" } } }
            }
            div #pw-result style="margin-top:10px" {}
        }
    }
}

#[derive(Deserialize)]
pub struct ViewerForm {
    #[serde(default)]
    password: String,
    /// "clear" to disable read-only login; anything else = set the password.
    #[serde(default)]
    action: String,
}

/// Set or clear the read-only viewer password (admin action; the middleware already blocks viewers).
pub async fn set_viewer(Form(f): Form<ViewerForm>) -> Markup {
    if ui::is_demo() {
        return html! { div.banner.warn style="margin:0" { "Disabled in the live demo." } };
    }
    if f.action == "clear" {
        clear_viewer_password();
        audit("admin", "viewer-disable", 200);
        return access_body(Some(
            html! { div.banner.ok style="margin:0" { "Read-only access disabled." } },
        ));
    }
    if f.password.chars().count() < 6 {
        return access_body(Some(
            html! { div.banner.error style="margin:0" { "Use at least 6 characters." } },
        ));
    }
    match set_viewer_password(&f.password) {
        Ok(()) => {
            audit("admin", "viewer-set", 200);
            access_body(Some(
                html! { div.banner.ok style="margin:0" { "Read-only viewer password set." } },
            ))
        }
        Err(e) => access_body(Some(
            html! { div.banner.error style="margin:0" { "Couldn't save: " (e) } },
        )),
    }
}

/// Download the full audit log as text.
pub async fn audit_download() -> Response {
    let body = std::fs::read_to_string(audit_path()).unwrap_or_default();
    (
        [
            (
                axum::http::header::CONTENT_TYPE,
                "text/plain; charset=utf-8",
            ),
            (
                axum::http::header::CONTENT_DISPOSITION,
                "attachment; filename=tendril-audit.log",
            ),
        ],
        body,
    )
        .into_response()
}

/// The "Access & audit" panel for the System page: read-only viewer credential + a log of changes.
pub fn access_panel() -> Markup {
    crate::ui::panel(
        "Access & audit",
        Some("read-only viewer login + a log of changes"),
        access_body(None),
    )
}

fn access_body(banner: Option<Markup>) -> Markup {
    let has_viewer = viewer_configured();
    let lines = audit_tail(200);
    html! {
        div.pad #access-panel {
            @if let Some(b) = banner { (b) }
            div.sub style="font-weight:600; margin:0 0 4px" { "Read-only viewer" }
            p.sub style="margin:0 0 8px" {
                "An optional second password granting " b { "read-only" } " access — see everything, change "
                "nothing. Hand it out instead of the admin password."
            }
            form hx-post="/system/viewer" hx-target="#access-panel" hx-swap="outerHTML"
                style="display:flex; gap:8px; align-items:center; flex-wrap:wrap" {
                input type="password" name="password"
                    placeholder=(if has_viewer { "set a new viewer password" } else { "viewer password" })
                    style="width:16em";
                button.btn.primary type="submit" { (if has_viewer { "Update" } else { "Enable read-only login" }) }
                @if has_viewer {
                    button.btn.sm.danger type="submit" name="action" value="clear"
                        hx-confirm="Disable read-only viewer login?" { "Disable" }
                    span.pill.running { span.led {} "on" }
                }
            }
            div style="margin-top:16px; padding-top:14px; border-top:1px solid var(--line)" {
                div.sub style="font-weight:600; margin-bottom:6px" { "Audit log" }
                @if lines.is_empty() {
                    p.sub style="margin:0" { "No recorded actions yet." }
                } @else {
                    p.sub style="margin:0 0 6px" { "Recent changes, newest first. " a href="/system/audit/download" { "Download full log" } }
                    pre.mono style="margin:0; max-height:280px; overflow:auto; font-size:12px; white-space:pre-wrap" {
                        @for l in &lines { (l) "\n" }
                    }
                }
            }
        }
    }
}

/// A minimal, nav-less page for the auth screens, styled with the shared tokens.
fn render(title: &str, error: Option<&str>, body: Markup) -> Markup {
    html! {
        (DOCTYPE)
        html lang="en" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                title { "Tendril · " (title) }
                style { (PreEscaped(ui::CSS)) }
            }
            body {
                div style="min-height:100vh; display:flex; align-items:center; justify-content:center; padding:20px" {
                    div style="width:100%; max-width:360px" {
                        div style="display:flex; align-items:center; gap:10px; justify-content:center; margin-bottom:18px" {
                            svg.glyph viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round" style="width:26px;height:26px" {
                                path d="M12 21c0-5 0-8-3-10S5 7 5 4" {}
                                path d="M12 21c0-4 .3-6.5 2.4-8.2C16.5 11 18 10 18 7" {}
                                circle cx="5" cy="4" r="1.4" {}
                                circle cx="18" cy="7" r="1.4" {}
                                circle cx="12" cy="21" r="1.4" {}
                            }
                            b style="letter-spacing:.24em" { "TENDRIL" }
                        }
                        section.panel { div.pad {
                            h2 style="margin:0 0 12px; font-size:1.05rem" { (title) }
                            @if let Some(e) = error { div.banner.error { (e) } }
                            (body)
                        } }
                    }
                }
            }
        }
    }
}

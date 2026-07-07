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
static SESSIONS: LazyLock<Mutex<HashMap<String, Instant>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

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

fn create_session() -> String {
    let token = new_token();
    SESSIONS
        .lock()
        .unwrap()
        .insert(token.clone(), Instant::now() + SESSION_TTL);
    token
}

fn valid_session(token: &str) -> bool {
    let mut s = SESSIONS.lock().unwrap();
    match s.get(token) {
        Some(&exp) if exp > Instant::now() => true,
        Some(_) => {
            s.remove(token);
            false
        }
        None => false,
    }
}

fn session_cookie(token: &str, max_age: i64) -> String {
    format!("tendril_session={token}; HttpOnly; SameSite=Lax; Path=/; Max-Age={max_age}")
}

fn cookie_token(req: &Request) -> Option<String> {
    let cookies = req.headers().get(COOKIE)?.to_str().ok()?;
    cookies
        .split(';')
        .map(str::trim)
        .find_map(|c| c.strip_prefix("tendril_session=").map(str::to_string))
}

fn authenticated(req: &Request) -> bool {
    // Reverse-proxy trust: a configured header, present and non-empty, means the proxy authenticated.
    if let Ok(name) = std::env::var("TENDRIL_TRUST_PROXY_HEADER") {
        if !name.is_empty() {
            if let Some(v) = req.headers().get(&name) {
                if v.to_str().map(|s| !s.is_empty()).unwrap_or(false) {
                    return true;
                }
            }
        }
    }
    cookie_token(req)
        .map(|t| valid_session(&t))
        .unwrap_or(false)
}

// ── middleware ──────────────────────────────────────────────────────────────────────────────

/// Gate every request: allow the auth endpoints and assets; force `/setup` until a password exists;
/// otherwise require a session (or a trusted proxy header).
pub async fn require_auth(req: Request, next: Next) -> Response {
    let path = req.uri().path();
    let open =
        path.starts_with("/assets/") || path == "/login" || path == "/logout" || path == "/setup";
    if open || authenticated(&req) {
        return next.run(req).await;
    }
    // Not authenticated: first run has no password → set one; otherwise sign in.
    if !is_configured() {
        Redirect::to("/setup").into_response()
    } else {
        Redirect::to("/login").into_response()
    }
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
    if verify_password(&f.password) {
        let token = create_session();
        (
            [(
                SET_COOKIE,
                session_cookie(&token, SESSION_TTL.as_secs() as i64),
            )],
            Redirect::to("/"),
        )
            .into_response()
    } else {
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
    if is_configured() {
        return Redirect::to("/login").into_response();
    }
    setup_form(None).into_response()
}

pub async fn setup(Form(f): Form<SetupForm>) -> Response {
    if is_configured() {
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
            let token = create_session();
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
    render(
        "Welcome to Tendril",
        error,
        html! {
            p.sub style="margin:-6px 0 14px" { "Set an admin password to secure the control plane." }
            form method="post" action="/setup" {
                div.field { label { "New password" } input type="password" name="password" autofocus required; }
                div.field { label { "Confirm" } input type="password" name="confirm" required; }
                button.btn.primary type="submit" style="width:100%; margin-top:6px" { "Create & sign in" }
            }
        },
    )
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

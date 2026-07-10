//! Push notifications for long-running / background outcomes (station installs, OS updates, fleet
//! reachability, ISO fetches) — an ntfy-compatible POST: the title rides a `Title:` header and the
//! message is the body, so a plain webhook still gets both.
//!
//! Config lives in `/etc/tendril/notify.conf` (override with `TENDRIL_NOTIFY_CONF`), `key=value`
//! lines like `federation.conf`: `url=` (required — empty/missing file means notifications are off)
//! and optional `auth=` (sent verbatim as an `Authorization` header value, e.g. `Bearer tk_…`).
//! The conf is 0600 — the auth value is a secret.

use maud::{html, Markup};
use serde::Deserialize;

use crate::ui;

fn conf_path() -> String {
    std::env::var("TENDRIL_NOTIFY_CONF").unwrap_or_else(|_| "/etc/tendril/notify.conf".to_string())
}

/// Parse notify.conf text into `(url, auth)` — `key=value` lines, `#` comments, last key wins.
fn parse_conf(txt: &str) -> (Option<String>, Option<String>) {
    let mut url = None;
    let mut auth = None;
    for line in txt.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            let v = v.trim().to_string();
            match k.trim() {
                "url" => url = Some(v),
                "auth" => auth = Some(v),
                _ => {}
            }
        }
    }
    (
        url.filter(|s| !s.is_empty()),
        auth.filter(|s| !s.is_empty()),
    )
}

/// The saved `(url, auth)` pair, unvalidated (the panel shows whatever is stored).
fn conf() -> (Option<String>, Option<String>) {
    std::fs::read_to_string(conf_path())
        .map(|t| parse_conf(&t))
        .unwrap_or((None, None))
}

/// The endpoint notifications go to, if one is configured AND still a valid http(s) URL — the URL is
/// validated at save time too, but a hand-edited conf must not reach `curl` unchecked (it's an argv
/// token; a `-`-leading or `file:` value would be an option/local read).
fn configured_url() -> Option<String> {
    conf().0.filter(|u| ui::is_http_url(u))
}

/// Whether notifications are on (a valid URL is configured). The demo never notifies.
pub fn enabled() -> bool {
    !ui::is_demo() && configured_url().is_some()
}

/// Send one notification synchronously (one `curl`, 10s cap). The shared core behind the detached
/// [`notify`] and the panel's "Send test" (which wants the error surfaced, not swallowed).
fn send(url: &str, auth: Option<&str>, title: &str, body: &str) -> Result<(), String> {
    // The title becomes a header value — strip control chars so a station/peer name from a config
    // file can't inject headers into the request.
    let title: String = title.chars().filter(|c| !c.is_control()).collect();
    let mut args: Vec<String> = vec![
        "-fsS".into(),
        "--max-time".into(),
        "10".into(),
        "-H".into(),
        format!("Title: {title}"),
    ];
    if let Some(a) = auth {
        args.push("-H".into());
        args.push(format!("Authorization: {a}"));
    }
    args.extend(["-d".into(), body.to_string(), "--".into(), url.to_string()]);
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    ui::run_result("curl", &refs).map(|_| ())
}

/// Fire-and-forget notification: no-op when unconfigured or in the demo; otherwise a detached thread
/// POSTs to the configured endpoint. Never blocks a request path; failures go to stderr only.
pub fn notify(title: &str, body: &str) {
    if ui::is_demo() {
        return;
    }
    let Some(url) = configured_url() else { return };
    let auth = conf().1;
    let (title, body) = (title.to_string(), body.to_string());
    std::thread::spawn(move || {
        if let Err(e) = send(&url, auth.as_deref(), &title, &body) {
            eprintln!("notify: POST to {url} failed: {e}");
        }
    });
}

// ── handlers + panel ─────────────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct NotifyForm {
    #[serde(default)]
    url: String,
    #[serde(default)]
    auth: String,
}

/// Save the notification settings (POST /system/notify). An empty URL turns notifications off. The
/// whole conf is rewritten 0600 via `ui::write_secret` — the auth value is a secret.
pub async fn save(axum::Form(f): axum::Form<NotifyForm>) -> Markup {
    if ui::is_demo() {
        return body_with(Some(
            html! { div.banner.warn style="margin:0 0 10px" { "Disabled in the live demo." } },
        ));
    }
    let url = f.url.trim().to_string();
    let auth = f.auth.trim().to_string();
    if !url.is_empty() && !ui::is_http_url(&url) {
        return body_with(Some(
            html! { div.banner.error style="margin:0 0 10px" { "The URL must be http(s):// with no spaces or shell characters." } },
        ));
    }
    // The auth value becomes a conf line and a header value — a newline would inject either.
    if !auth.is_empty() && !ui::safe_field(&auth) {
        return body_with(Some(
            html! { div.banner.error style="margin:0 0 10px" { "The authorization value can't contain control characters." } },
        ));
    }
    let mut content = format!("url={url}\n");
    if !auth.is_empty() {
        content.push_str(&format!("auth={auth}\n"));
    }
    let p = conf_path();
    if let Some(dir) = std::path::Path::new(&p).parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let banner = match ui::write_secret(&p, content.as_bytes()) {
        Ok(()) if url.is_empty() => {
            html! { div.banner.ok style="margin:0 0 10px" { "Notifications turned off." } }
        }
        Ok(()) => {
            html! { div.banner.ok style="margin:0 0 10px" { "Notifications on — background events now post to " span.mono { (url) } "." } }
        }
        Err(e) => {
            html! { div.banner.error style="margin:0 0 10px" { "Couldn't save: " (e) } }
        }
    };
    body_with(Some(banner))
}

/// Send a test notification (POST /system/notify/test) — synchronously, so a failure is shown here
/// instead of being swallowed like the fire-and-forget path.
pub async fn test() -> Markup {
    if ui::is_demo() {
        return body_with(Some(
            html! { div.banner.warn style="margin:0 0 10px" { "Disabled in the live demo." } },
        ));
    }
    let Some(url) = configured_url() else {
        return body_with(Some(
            html! { div.banner.error style="margin:0 0 10px" { "Set a notification URL first." } },
        ));
    };
    let auth = conf().1;
    let res = tokio::task::spawn_blocking(move || {
        send(
            &url,
            auth.as_deref(),
            "Tendril test notification",
            "Notifications are working.",
        )
    })
    .await
    .unwrap_or_else(|_| Err("test task panicked".into()));
    let banner = match res {
        Ok(()) => {
            html! { div.banner.ok style="margin:0 0 10px" { "Test sent — check your notification channel." } }
        }
        Err(e) => html! { div.banner.error style="margin:0 0 10px" { "Test failed: " (e) } },
    };
    body_with(Some(banner))
}

/// The "Notifications" panel for the System page.
pub fn panel() -> Markup {
    ui::panel(
        "Notifications",
        Some("push background events to ntfy or a webhook"),
        body_with(None),
    )
}

fn body_with(banner: Option<Markup>) -> Markup {
    let (url, auth) = conf();
    let on = enabled();
    html! {
        div.pad #notify-panel {
            @if let Some(b) = banner { (b) }
            p.sub style="margin:0 0 10px" {
                "Get a push when something finishes in the background — a station install, a staged OS "
                "update, a fleet node going dark. Point it at an "
                a href="https://ntfy.sh" target="_blank" rel="noreferrer" { "ntfy" }
                " topic or any webhook (the title rides a " span.mono { "Title:" } " header)."
            }
            div style="display:flex; align-items:center; gap:10px; margin-bottom:10px" {
                @if on {
                    span.pill.running { span.led {} "on" }
                    span.sub.mono { (url.as_deref().unwrap_or("")) }
                } @else {
                    span.pill.off { span.led {} "off" }
                }
            }
            form.grid hx-post="/system/notify" hx-target="#notify-panel" hx-swap="outerHTML" {
                div.field.wide {
                    label { "URL" }
                    input name="url" placeholder="https://ntfy.sh/my-topic" value=(url.as_deref().unwrap_or(""));
                    span.hint { "Leave empty to turn notifications off." }
                }
                div.field.wide {
                    label { "Authorization header (optional)" }
                    input name="auth" placeholder="Bearer tk_…" value=(auth.as_deref().unwrap_or(""));
                    span.hint { "Sent verbatim as the " span.mono { "Authorization" } " header. What you save here is what's used — clearing it removes it." }
                }
                div.field.wide { div.btnrow {
                    button.btn.primary type="submit" { "Save" }
                    @if on {
                        // hx-target/hx-swap inherit from the form; type=button so this never
                        // double-fires as a plain form submit.
                        button.btn type="button" hx-post="/system/notify/test" { "Send test" }
                    }
                } }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conf_parsing() {
        assert_eq!(parse_conf(""), (None, None));
        assert_eq!(
            parse_conf("url=https://ntfy.sh/t\n"),
            (Some("https://ntfy.sh/t".into()), None)
        );
        assert_eq!(
            parse_conf("# comment\nurl = https://x/y \nauth= Bearer abc\n"),
            (Some("https://x/y".into()), Some("Bearer abc".into()))
        );
        // Empty values mean unset; unknown keys are ignored; the last key wins.
        assert_eq!(parse_conf("url=\nauth=\nbogus=1"), (None, None));
        assert_eq!(
            parse_conf("url=https://a\nurl=https://b").0,
            Some("https://b".into())
        );
    }
}

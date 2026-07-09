//! HTTPS for the web UI: a self-signed cert auto-generated on first boot (or a user-provided cert),
//! terminated by the app's own rustls server. The same cert infrastructure backs federation
//! node-to-node TLS later.
//!
//! Opt-in via `TENDRIL_TLS=on` (default HTTP, so existing deployments and reverse-proxy setups are
//! unaffected). Cert/key default to `/etc/tendril/tls/{cert,key}.pem`; override with `TENDRIL_TLS_CERT`
//! / `TENDRIL_TLS_KEY` to use a real certificate.

use std::path::Path;
use std::sync::OnceLock;

use axum::extract::Multipart;
use axum_server::tls_rustls::RustlsConfig;
use maud::{html, Markup};

use crate::ui;

const CERT_DIR: &str = "/etc/tendril/tls";

/// The live rustls config, stashed by `main` after it starts the HTTPS server, so the UI can hot-reload
/// the cert without a restart. `None` when we're serving plain HTTP.
static LIVE_CONFIG: OnceLock<RustlsConfig> = OnceLock::new();

/// Record the live TLS config so `reload()` can swap the cert in place.
pub fn set_live_config(cfg: RustlsConfig) {
    let _ = LIVE_CONFIG.set(cfg);
}

/// Hot-reload the running server's cert+key from disk (no restart). No-op if we aren't serving TLS.
pub async fn reload() -> Result<(), String> {
    let (cert_p, key_p) = paths();
    match LIVE_CONFIG.get() {
        Some(cfg) => cfg
            .reload_from_pem_file(&cert_p, &key_p)
            .await
            .map_err(|e| format!("reload failed: {e}")),
        None => Ok(()),
    }
}

/// Whether the web UI serves HTTPS. **Mandatory by default** — HTTPS unless `TENDRIL_TLS` is
/// explicitly set off (the escape hatch for deployments behind a TLS-terminating reverse proxy that
/// speaks plain HTTP to the app).
pub fn enabled() -> bool {
    !matches!(
        std::env::var("TENDRIL_TLS")
            .unwrap_or_default()
            .to_lowercase()
            .as_str(),
        "off" | "0" | "false" | "no"
    )
}

fn paths() -> (String, String) {
    (
        std::env::var("TENDRIL_TLS_CERT").unwrap_or_else(|_| format!("{CERT_DIR}/cert.pem")),
        std::env::var("TENDRIL_TLS_KEY").unwrap_or_else(|_| format!("{CERT_DIR}/key.pem")),
    )
}

/// Ensure a cert+key exist — a user-provided one, or an auto-generated self-signed one (via the
/// `openssl` CLI) — and return their paths.
pub fn ensure() -> Result<(String, String), String> {
    let (cert_p, key_p) = paths();
    if Path::new(&cert_p).exists() && Path::new(&key_p).exists() {
        return Ok((cert_p, key_p));
    }
    if let Some(d) = Path::new(&cert_p).parent() {
        let _ = std::fs::create_dir_all(d);
    }
    // 10-year P-256 self-signed cert covering this box's hostnames/IPs.
    crate::ui::precreate_key(&key_p);
    crate::ui::run_result(
        "openssl",
        &[
            "req",
            "-x509",
            "-newkey",
            "ec",
            "-pkeyopt",
            "ec_paramgen_curve:prime256v1",
            "-nodes",
            "-days",
            "3650",
            "-subj",
            "/CN=Tendril",
            "-addext",
            &ui::san_ext(&ui::cert_sans()),
            "-keyout",
            &key_p,
            "-out",
            &cert_p,
        ],
    )
    .map_err(|e| format!("openssl cert generation failed: {e}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&key_p, std::fs::Permissions::from_mode(0o600));
    }
    Ok((cert_p, key_p))
}

// ── certificate management UI ─────────────────────────────────────────────────────────────────

/// A field from the current cert via `openssl x509 -noout <flag>`, with the `label=` prefix stripped.
fn cert_field(cert_p: &str, flag: &str, strip: &str) -> Option<String> {
    let out = ui::run_result("openssl", &["x509", "-in", cert_p, "-noout", flag]).ok()?;
    let out = out.trim();
    Some(out.strip_prefix(strip).unwrap_or(out).trim().to_string()).filter(|s| !s.is_empty())
}

/// Whether the current cert looks self-signed (subject == issuer) — drives the UI hint.
fn is_self_signed(cert_p: &str) -> bool {
    match (
        cert_field(cert_p, "-subject", ""),
        cert_field(cert_p, "-issuer", ""),
    ) {
        (Some(s), Some(i)) => {
            s.strip_prefix("subject=").unwrap_or(&s).trim()
                == i.strip_prefix("issuer=").unwrap_or(&i).trim()
        }
        _ => false,
    }
}

/// Summary of the cert currently in use, for display.
struct CertInfo {
    subject: String,
    sans: String,
    expires: String,
    fingerprint: String,
    self_signed: bool,
}

fn cert_info() -> Option<CertInfo> {
    let (cert_p, _) = paths();
    if !Path::new(&cert_p).exists() {
        return None;
    }
    let sans = ui::run_result(
        "openssl",
        &["x509", "-in", &cert_p, "-noout", "-ext", "subjectAltName"],
    )
    .ok()
    .and_then(|s| {
        s.lines()
            .map(str::trim)
            .find(|l| l.contains("DNS:") || l.contains("IP:"))
            .map(str::to_string)
    })
    .unwrap_or_else(|| "—".to_string());
    Some(CertInfo {
        subject: cert_field(&cert_p, "-subject", "subject=").unwrap_or_else(|| "—".to_string()),
        sans,
        expires: cert_field(&cert_p, "-enddate", "notAfter=").unwrap_or_else(|| "—".to_string()),
        fingerprint: ui::run_result(
            "openssl",
            &["x509", "-in", &cert_p, "-noout", "-fingerprint", "-sha256"],
        )
        .ok()
        .map(|s| {
            let s = s.trim();
            s.strip_prefix("sha256 Fingerprint=")
                .unwrap_or(s)
                .trim()
                .to_string()
        })
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "—".to_string()),
        self_signed: is_self_signed(&cert_p),
    })
}

/// Validate that a PEM cert and key parse and **belong together** (same public key), so we never write
/// a mismatched pair that would make the server fail to start / reload. Returns the parsed-out details
/// on success.
fn validate_pair(cert_pem: &[u8], key_pem: &[u8]) -> Result<(), String> {
    let tmp = std::env::temp_dir();
    let pid = std::process::id();
    let cert_t = tmp.join(format!("tendril-cert-{pid}.pem"));
    let key_t = tmp.join(format!("tendril-key-{pid}.pem"));
    let cleanup = || {
        let _ = std::fs::remove_file(&cert_t);
        let _ = std::fs::remove_file(&key_t);
    };
    // Write the (private-key-bearing) temp files 0600 and O_EXCL — so the key isn't world-readable in
    // the shared temp dir and a pre-planted symlink at the predictable name can't be followed.
    let write = |p: &Path, b: &[u8]| -> Result<(), String> {
        use std::io::Write as _;
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        opts.open(p)
            .and_then(|mut f| f.write_all(b))
            .map_err(|e| e.to_string())
    };
    let run = |args: &[&str]| ui::run_result("openssl", args);
    let result = (|| {
        write(&cert_t, cert_pem)?;
        write(&key_t, key_pem)?;
        let cert_s = cert_t.to_string_lossy().to_string();
        let key_s = key_t.to_string_lossy().to_string();
        // Both must parse.
        let cert_pub = run(&["x509", "-in", &cert_s, "-noout", "-pubkey"])
            .map_err(|e| format!("certificate isn't valid PEM: {e}"))?;
        let key_pub = run(&["pkey", "-in", &key_s, "-pubout"])
            .map_err(|e| format!("private key isn't valid PEM: {e}"))?;
        if cert_pub.trim() != key_pub.trim() {
            return Err(
                "the certificate and private key don't match (different public keys).".into(),
            );
        }
        Ok(())
    })();
    cleanup();
    result
}

/// Atomically install a validated cert+key to the configured paths (key mode 0600) and hot-reload.
fn install_pair(cert_pem: &[u8], key_pem: &[u8]) -> Result<(), String> {
    let (cert_p, key_p) = paths();
    if let Some(d) = Path::new(&cert_p).parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let cert_tmp = format!("{cert_p}.new");
    let key_tmp = format!("{key_p}.new");
    std::fs::write(&cert_tmp, cert_pem).map_err(|e| e.to_string())?;
    // Write the key at 0600 from the first byte (never a 0644 window): pre-create it locked down,
    // clearing any stale `.new` from a prior aborted install first.
    let _ = std::fs::remove_file(&key_tmp);
    crate::ui::precreate_key(&key_tmp);
    std::fs::write(&key_tmp, key_pem).map_err(|e| e.to_string())?;
    std::fs::rename(&cert_tmp, &cert_p).map_err(|e| e.to_string())?;
    std::fs::rename(&key_tmp, &key_p).map_err(|e| e.to_string())?;
    Ok(())
}

/// The System-page TLS panel: current cert details, upload a real cert, or regenerate a self-signed one.
pub fn panel() -> Markup {
    ui::panel(
        "HTTPS certificate",
        Some(if enabled() { "in use" } else { "off" }),
        cert_body(None),
    )
}

fn cert_body(banner: Option<Markup>) -> Markup {
    let info = cert_info();
    html! {
        div.pad #tls-panel {
            @if let Some(b) = banner { (b) }
            @if !enabled() {
                p.sub { "This instance serves plain HTTP (TENDRIL_TLS=off) — usually because a reverse proxy terminates TLS in front of it. Certificate management is disabled here." }
            } @else if let Some(i) = info {
                table { tbody {
                    tr { td.sub style="white-space:nowrap" { "Subject" } td.mono { (i.subject) } }
                    tr { td.sub style="white-space:nowrap" { "Names (SAN)" } td.mono style="word-break:break-all" { (i.sans) } }
                    tr { td.sub style="white-space:nowrap" { "Expires" } td.mono { (i.expires) } }
                    tr { td.sub style="white-space:nowrap" { "SHA-256" } td.mono style="word-break:break-all" { (i.fingerprint) } }
                    tr { td.sub style="white-space:nowrap" { "Type" } td { @if i.self_signed { span.badge title="Browsers warn on self-signed certs" { "self-signed" } } @else { "provided" } } }
                } }
                @if i.self_signed {
                    p.sub style="margin-top:10px" { "Browsers warn on the self-signed cert. Upload a real certificate (from your CA or an internal PKI) to remove the warning." }
                }
                div style="margin-top:16px; padding-top:14px; border-top:1px solid var(--line)" {
                    div.sub style="font-weight:600; margin-bottom:8px" { "Install a certificate" }
                    form hx-post="/system/tls/upload" hx-encoding="multipart/form-data"
                        hx-target="#tls-panel" hx-swap="outerHTML" {
                        div.field {
                            label { "Certificate (PEM — full chain: server cert first, then intermediates)" }
                            textarea name="cert" rows="5" required placeholder="-----BEGIN CERTIFICATE-----" style="font-family:var(--mono); font-size:12px" {}
                        }
                        div.field {
                            label { "Private key (PEM — unencrypted)" }
                            textarea name="key" rows="5" required placeholder="-----BEGIN PRIVATE KEY-----" style="font-family:var(--mono); font-size:12px" {}
                        }
                        div.sub style="margin:-4px 0 10px" { "Or upload files: "
                            input type="file" name="certfile" accept=".pem,.crt,.cer" style="font-size:12px";
                            " "
                            input type="file" name="keyfile" accept=".pem,.key" style="font-size:12px";
                        }
                        button.btn.primary type="submit" { "Validate & install" }
                    }
                    div.btnrow style="margin-top:14px; padding-top:12px; border-top:1px solid var(--line)" {
                        button.btn hx-post="/system/tls/regenerate" hx-target="#tls-panel" hx-swap="outerHTML"
                            hx-confirm="Replace the current certificate with a freshly generated self-signed one? Browsers will warn until you install a real cert." { "Regenerate self-signed" }
                    }
                }
            } @else {
                p.sub { "No certificate found yet." }
            }
        }
    }
}

/// Read a multipart field's text, preferring an uploaded file over a pasted textarea. Fields are
/// consumed in order, so collect them all first.
pub async fn upload(mut mp: Multipart) -> Markup {
    if ui::is_demo() {
        return cert_body(Some(html! { div.banner.warn { "Disabled in the demo." } }));
    }
    let mut cert = Vec::new();
    let mut key = Vec::new();
    while let Ok(Some(field)) = mp.next_field().await {
        let name = field.name().unwrap_or("").to_string();
        let data = field.bytes().await.unwrap_or_default();
        match name.as_str() {
            "cert" if !data.trim_ascii().is_empty() && cert.is_empty() => cert = data.to_vec(),
            "certfile" if !data.trim_ascii().is_empty() => cert = data.to_vec(),
            "key" if !data.trim_ascii().is_empty() && key.is_empty() => key = data.to_vec(),
            "keyfile" if !data.trim_ascii().is_empty() => key = data.to_vec(),
            _ => {}
        }
    }
    if cert.trim_ascii().is_empty() || key.trim_ascii().is_empty() {
        return cert_body(Some(
            html! { div.banner.error { "Provide both a certificate and a private key." } },
        ));
    }
    if let Err(e) = validate_pair(&cert, &key) {
        return cert_body(Some(html! { div.banner.error { (e) } }));
    }
    if let Err(e) = install_pair(&cert, &key) {
        return cert_body(Some(
            html! { div.banner.error { "Couldn't save the certificate: " (e) } },
        ));
    }
    let msg = match reload().await {
        Ok(()) => {
            html! { div.banner.ok { "Certificate installed and now live — no restart needed." } }
        }
        Err(e) => {
            html! { div.banner.warn { "Certificate saved, but hot-reload failed (" (e) "); it will take effect on the next restart." } }
        }
    };
    cert_body(Some(msg))
}

/// Regenerate a self-signed cert (delete the current one, re-run `ensure`) and hot-reload.
pub async fn regenerate() -> Markup {
    if ui::is_demo() {
        return cert_body(Some(html! { div.banner.warn { "Disabled in the demo." } }));
    }
    let (cert_p, key_p) = paths();
    let _ = std::fs::remove_file(&cert_p);
    let _ = std::fs::remove_file(&key_p);
    if let Err(e) = ensure() {
        return cert_body(Some(
            html! { div.banner.error { "Couldn't regenerate the certificate: " (e) } },
        ));
    }
    let msg = match reload().await {
        Ok(()) => {
            html! { div.banner.ok { "Generated a fresh self-signed certificate — now live." } }
        }
        Err(e) => {
            html! { div.banner.warn { "Certificate generated, but hot-reload failed (" (e) "); it will take effect on the next restart." } }
        }
    };
    cert_body(Some(msg))
}

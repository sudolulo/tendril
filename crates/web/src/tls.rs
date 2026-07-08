//! HTTPS for the web UI: a self-signed cert auto-generated on first boot (or a user-provided cert),
//! terminated by the app's own rustls server. The same cert infrastructure backs federation
//! node-to-node TLS later.
//!
//! Opt-in via `TENDRIL_TLS=on` (default HTTP, so existing deployments and reverse-proxy setups are
//! unaffected). Cert/key default to `/etc/tendril/tls/{cert,key}.pem`; override with `TENDRIL_TLS_CERT`
//! / `TENDRIL_TLS_KEY` to use a real certificate.

use std::net::IpAddr;
use std::path::Path;

const CERT_DIR: &str = "/etc/tendril/tls";

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

/// Hostnames/IPs to put in the self-signed cert's SANs so it validates however the box is reached.
fn sans() -> Vec<String> {
    let mut v = vec!["localhost".to_string(), "127.0.0.1".to_string()];
    if let Some(h) = crate::ui::run_stdout("hostname", &[]) {
        let h = h.trim();
        if !h.is_empty() {
            v.push(h.to_string());
        }
    }
    if let Some(ips) = crate::ui::run_stdout("hostname", &["-I"]) {
        for ip in ips.split_whitespace() {
            v.push(ip.to_string());
        }
    }
    v.sort();
    v.dedup();
    v
}

/// The `subjectAltName` extension value from our SANs, e.g. `DNS:localhost,IP:127.0.0.1,DNS:box`.
fn san_ext(sans: &[String]) -> String {
    let parts: Vec<String> = sans
        .iter()
        .map(|s| {
            if s.parse::<IpAddr>().is_ok() {
                format!("IP:{s}")
            } else {
                format!("DNS:{s}")
            }
        })
        .collect();
    format!("subjectAltName={}", parts.join(","))
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
            &san_ext(&sans()),
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

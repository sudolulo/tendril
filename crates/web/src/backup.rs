//! Settings backup & restore: download `/etc/tendril` as a tar.gz, and restore one over it.
//!
//! The archive contains **every secret** (password hashes, the federation token + CA key, API-token
//! hashes, notify auth) — the download route is on `auth.rs`'s `sensitive_get` list so a read-only
//! viewer can't fetch it. Restore extracts to a staging dir first and verifies nothing escapes it
//! (symlink/`..` traversal) before copying over `/etc/tendril`.

use axum::extract::Multipart;
use axum::http::header;
use axum::response::{IntoResponse, Response};
use maud::{html, Markup};

use crate::ui;

/// Everything under here is "the settings" — the same dir every module's conf/secret defaults to.
const ETC_DIR: &str = "/etc/tendril";

/// Files under `/etc/tendril` that must stay 0600 (secrets). `std::fs::copy` preserves the source
/// permissions — which are whatever the uploader's tar/umask produced — so these are re-tightened
/// explicitly after a restore.
const SECRET_FILES: &[&str] = &[
    "webauth",
    "webauth.viewer",
    "federation-token",
    "notify.conf",
    "smb-creds",
    "api-tokens.json",
];

/// Stream `/etc/tendril` as a tar.gz (GET /system/backup). The tarball is binary — this reads
/// `Command::output()`'s raw stdout bytes directly (`ui::run_result` returns a lossy `String`,
/// which would corrupt it).
pub async fn download() -> Response {
    // The demo must never hand out a real co-located instance's secrets — the middleware only gates
    // POSTs, and this GET is the most sensitive one there is.
    if ui::is_demo() {
        return (
            [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
            "(demo — the settings backup is disabled; it contains every secret)\n",
        )
            .into_response();
    }
    let out = tokio::task::spawn_blocking(|| {
        std::process::Command::new("tar")
            .args(["-czf", "-", "-C", ETC_DIR, "."])
            .output()
    })
    .await;
    match out {
        Ok(Ok(o)) if o.status.success() => (
            [
                (header::CONTENT_TYPE, "application/gzip"),
                (
                    header::CONTENT_DISPOSITION,
                    "attachment; filename=tendril-settings.tar.gz",
                ),
            ],
            o.stdout,
        )
            .into_response(),
        Ok(Ok(o)) => (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!(
                "backup failed: {}",
                String::from_utf8_lossy(&o.stderr).trim()
            ),
        )
            .into_response(),
        _ => (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            "backup failed: couldn't run tar",
        )
            .into_response(),
    }
}

/// Verify every path under `staging` stays under `staging`: reject `..` components and symlinks that
/// resolve outside it (including dangling ones — they can't be proven safe). Plain files and dirs
/// from `read_dir` can't traverse on their own; symlinks in the archive are the escape vector.
fn verify_staging(staging: &std::path::Path) -> Result<(), String> {
    let root = staging
        .canonicalize()
        .map_err(|e| format!("staging dir vanished: {e}"))?;
    fn walk(dir: &std::path::Path, root: &std::path::Path) -> Result<(), String> {
        let rd = std::fs::read_dir(dir).map_err(|e| format!("read {}: {e}", dir.display()))?;
        for e in rd {
            let e = e.map_err(|e| e.to_string())?;
            let path = e.path();
            if path
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir))
            {
                return Err(format!("archive path escapes: {}", path.display()));
            }
            let ft = e.file_type().map_err(|e| e.to_string())?;
            if ft.is_symlink() {
                let target = path.canonicalize().map_err(|_| {
                    format!("archive symlink can't be resolved: {}", path.display())
                })?;
                if !target.starts_with(root) {
                    return Err(format!(
                        "archive symlink points outside the archive: {}",
                        path.display()
                    ));
                }
            } else if ft.is_dir() {
                walk(&path, root)?;
            }
        }
        Ok(())
    }
    walk(&root, &root)
}

/// Copy the staging tree over `dst` (dirs created, files overwritten; verified-internal symlinks are
/// copied as their target's content). Nothing in `dst` is deleted — a restore is additive/overwriting,
/// so a half-uploaded archive can't wipe settings it didn't contain.
fn copy_tree(src: &std::path::Path, dst: &std::path::Path) -> Result<(), String> {
    std::fs::create_dir_all(dst).map_err(|e| format!("create {}: {e}", dst.display()))?;
    let rd = std::fs::read_dir(src).map_err(|e| format!("read {}: {e}", src.display()))?;
    for e in rd {
        let e = e.map_err(|e| e.to_string())?;
        let from = e.path();
        let to = dst.join(e.file_name());
        // `metadata()` follows symlinks — after `verify_staging`, any link resolves inside staging,
        // so copying what it points at is safe (and simpler than recreating links under /etc).
        let meta = std::fs::metadata(&from).map_err(|e| format!("{}: {e}", from.display()))?;
        if meta.is_dir() {
            copy_tree(&from, &to)?;
        } else {
            std::fs::copy(&from, &to).map_err(|e| format!("copy {}: {e}", from.display()))?;
        }
    }
    Ok(())
}

/// Extract an uploaded settings archive safely and copy it over `/etc/tendril`, then re-tighten the
/// known secret files to 0600. Blocking (tar + fs walk) — called via `spawn_blocking`.
fn restore_archive(tmp: &std::path::Path) -> Result<(), String> {
    let staging = std::env::temp_dir().join(format!(
        "tendril-restore-{}-{}",
        std::process::id(),
        ui::now_utc_compact()
    ));
    std::fs::create_dir_all(&staging).map_err(|e| format!("create staging dir: {e}"))?;
    let cleanup = || {
        let _ = std::fs::remove_dir_all(&staging);
    };
    let res = (|| {
        ui::run_result(
            "tar",
            &[
                "-xzf",
                &tmp.to_string_lossy(),
                "-C",
                &staging.to_string_lossy(),
            ],
        )
        .map_err(|e| format!("that doesn't look like a settings backup: {e}"))?;
        verify_staging(&staging)?;
        copy_tree(&staging, std::path::Path::new(ETC_DIR))?;
        for f in SECRET_FILES {
            let p = format!("{ETC_DIR}/{f}");
            if std::path::Path::new(&p).exists() {
                ui::chmod_600(&p);
            }
        }
        Ok(())
    })();
    cleanup();
    res
}

/// Restore a settings backup (POST /system/restore, multipart field `archive`). The upload is
/// streamed to a temp file, extracted to staging, verified, then copied over `/etc/tendril`.
pub async fn restore(mut mp: Multipart) -> Markup {
    if ui::is_demo() {
        return html! { div.banner.warn style="margin:0" { "Disabled in the live demo." } };
    }
    let tmp = std::env::temp_dir().join(format!("tendril-restore-{}.tar.gz", std::process::id()));
    let mut wrote = 0u64;
    while let Ok(Some(mut field)) = mp.next_field().await {
        if field.name().unwrap_or("") != "archive" {
            continue;
        }
        use std::io::Write as _;
        let Ok(mut f) = std::fs::File::create(&tmp) else {
            return html! { div.banner.error style="margin:0" { "Couldn't open a temp file for the upload." } };
        };
        loop {
            match field.chunk().await {
                Ok(Some(bytes)) => {
                    if f.write_all(&bytes).is_err() {
                        let _ = std::fs::remove_file(&tmp);
                        return html! { div.banner.error style="margin:0" { "Write failed while saving the upload." } };
                    }
                    wrote += bytes.len() as u64;
                }
                Ok(None) => break,
                Err(_) => {
                    let _ = std::fs::remove_file(&tmp);
                    return html! { div.banner.error style="margin:0" { "Upload was interrupted." } };
                }
            }
        }
    }
    if wrote == 0 {
        let _ = std::fs::remove_file(&tmp);
        return html! { div.banner.error style="margin:0" { "Choose a settings backup (.tar.gz) to restore." } };
    }
    let tmp2 = tmp.clone();
    let res = tokio::task::spawn_blocking(move || restore_archive(&tmp2))
        .await
        .unwrap_or_else(|_| Err("restore task panicked".into()));
    let _ = std::fs::remove_file(&tmp);
    match res {
        Ok(()) => html! { div.banner.ok style="margin:0" {
            "Settings restored over " span.mono { (ETC_DIR) } ". Restart " span.mono { "tendril-web" }
            " to apply everything (passwords, tokens, and federation trust are re-read live, but "
            "TLS and the mTLS listener load at startup)."
        } },
        Err(e) => {
            html! { div.banner.error style="margin:0" { "Restore failed — nothing was changed unless the copy step started: " (e) } }
        }
    }
}

/// The "Backup & restore" panel for the System page.
pub fn panel() -> Markup {
    ui::panel(
        "Backup & restore",
        Some("every setting in /etc/tendril"),
        html! {
            div.pad {
                p.sub style="margin:0 0 10px" {
                    "Download everything under " span.mono { (ETC_DIR) } " — passwords, tokens, fleet "
                    "trust, TLS material — as one archive. " b { "Treat it as a secret." }
                }
                div.btnrow {
                    a.btn href="/system/backup" download="tendril-settings.tar.gz" { "⬇ Download settings backup" }
                }
                div style="margin-top:16px; padding-top:14px; border-top:1px solid var(--line)" {
                    div.sub style="font-weight:600; margin-bottom:6px" { "Restore" }
                    p.sub style="margin:0 0 8px" {
                        "Upload a settings backup to copy it over " span.mono { (ETC_DIR) } ". Existing "
                        "files are overwritten; nothing is deleted."
                    }
                    form hx-post="/system/restore" hx-encoding="multipart/form-data"
                        hx-target="#restore-result" hx-swap="innerHTML"
                        hx-confirm="Restore this backup over /etc/tendril? Passwords, tokens, and fleet trust are replaced by the archive's versions."
                        style="display:flex; gap:8px; align-items:center; flex-wrap:wrap" {
                        input type="file" name="archive" accept=".tar.gz,.tgz,application/gzip" required
                            style="width:auto";
                        button.btn.danger type="submit" { "Restore" }
                    }
                    div #restore-result style="margin-top:10px" {}
                }
            }
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn staging_verification_catches_symlink_escapes() {
        let base = std::env::temp_dir().join(format!("tendril-backup-test-{}", std::process::id()));
        let staging = base.join("staging");
        std::fs::create_dir_all(staging.join("sub")).unwrap();
        std::fs::write(staging.join("webauth"), "hash").unwrap();
        std::fs::write(staging.join("sub/notify.conf"), "url=").unwrap();
        // Plain files + dirs pass.
        assert!(verify_staging(&staging).is_ok());
        // An internal symlink is fine.
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(staging.join("webauth"), staging.join("link-in")).unwrap();
            assert!(verify_staging(&staging).is_ok());
            // A symlink escaping the staging dir is rejected.
            std::os::unix::fs::symlink("/etc/hostname", staging.join("link-out")).unwrap();
            assert!(verify_staging(&staging).is_err());
            std::fs::remove_file(staging.join("link-out")).unwrap();
            // A dangling symlink can't be proven safe — rejected too.
            std::os::unix::fs::symlink(staging.join("gone"), staging.join("dangle")).unwrap();
            assert!(verify_staging(&staging).is_err());
        }
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn copy_tree_overwrites_without_deleting() {
        let base = std::env::temp_dir().join(format!("tendril-copy-test-{}", std::process::id()));
        let (src, dst) = (base.join("src"), base.join("dst"));
        std::fs::create_dir_all(src.join("sub")).unwrap();
        std::fs::create_dir_all(&dst).unwrap();
        std::fs::write(src.join("a"), "new").unwrap();
        std::fs::write(src.join("sub/b"), "b").unwrap();
        std::fs::write(dst.join("a"), "old").unwrap();
        std::fs::write(dst.join("keep"), "kept").unwrap();
        copy_tree(&src, &dst).unwrap();
        assert_eq!(std::fs::read_to_string(dst.join("a")).unwrap(), "new");
        assert_eq!(std::fs::read_to_string(dst.join("sub/b")).unwrap(), "b");
        // Files the archive didn't contain survive.
        assert_eq!(std::fs::read_to_string(dst.join("keep")).unwrap(), "kept");
        let _ = std::fs::remove_dir_all(&base);
    }
}

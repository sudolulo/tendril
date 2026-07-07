//! Remote media/image storage. Point Tendril's `isos/` and `images/` at a mounted **NFS or SMB**
//! share so every node (and station-image clone) sees the same media and golden images — the shared
//! store behind clustering. Station **disks** stay local (fast, per-node); only media + images move.
//!
//! Config lives at `/etc/tendril/storage.conf` (`key=value` lines). When a store is configured and
//! actually mounted, `iso_dir()`/`image_dir()` resolve to `<mount>/isos` and `<mount>/images`;
//! otherwise they fall back to the local defaults. The mount is persisted in `/etc/fstab` (with
//! `nofail,_netdev`) so it survives reboots.

use std::path::Path as FsPath;

use axum::extract::Form;
use maud::{html, Markup};
use serde::Deserialize;

use crate::ui;

const LOCAL_ISOS: &str = "/var/lib/tendril/isos";
const LOCAL_IMAGES: &str = "/var/lib/tendril/images";
const SMB_CREDS: &str = "/etc/tendril/smb-creds";
const FSTAB_TAG: &str = "# tendril-store (managed)";

fn conf_path() -> String {
    std::env::var("TENDRIL_STORAGE_CONF")
        .unwrap_or_else(|_| "/etc/tendril/storage.conf".to_string())
}

#[derive(Clone, Default)]
pub struct Store {
    pub kind: String,   // "nfs" | "smb"
    pub remote: String, // nfs: server:/export   smb: //server/share
    pub mount: String,  // mount point, e.g. /var/lib/tendril/store
    pub options: String,
    pub username: String, // smb only
}

pub fn load() -> Option<Store> {
    let txt = std::fs::read_to_string(conf_path()).ok()?;
    let mut s = Store::default();
    for line in txt.lines() {
        if let Some((k, v)) = line.split_once('=') {
            let v = v.trim().to_string();
            match k.trim() {
                "type" => s.kind = v,
                "remote" => s.remote = v,
                "mount" => s.mount = v,
                "options" => s.options = v,
                "username" => s.username = v,
                _ => {}
            }
        }
    }
    (!s.remote.is_empty() && !s.mount.is_empty()).then_some(s)
}

fn save(s: &Store) -> std::io::Result<()> {
    let p = conf_path();
    if let Some(d) = FsPath::new(&p).parent() {
        std::fs::create_dir_all(d)?;
    }
    std::fs::write(
        &p,
        format!(
            "type={}\nremote={}\nmount={}\noptions={}\nusername={}\n",
            s.kind, s.remote, s.mount, s.options, s.username
        ),
    )
}

/// True if `mount` is a current mount point.
fn is_mounted(mount: &str) -> bool {
    !mount.is_empty()
        && ui::run_stdout("findmnt", &["-nT", mount])
            .map(|o| !o.trim().is_empty())
            .unwrap_or(false)
}

/// The store's mount point *iff* a store is configured and actually mounted right now.
fn active_mount() -> Option<String> {
    let s = load()?;
    is_mounted(&s.mount).then_some(s.mount)
}

/// Where install ISOs live: env override, else the mounted store's `isos/`, else the local default.
pub fn iso_dir() -> String {
    std::env::var("TENDRIL_ISO_DIR")
        .ok()
        .or_else(|| active_mount().map(|m| format!("{m}/isos")))
        .unwrap_or_else(|| LOCAL_ISOS.to_string())
}

/// Where golden station images live: env override, else the mounted store's `images/`, else local.
pub fn image_dir() -> String {
    std::env::var("TENDRIL_IMAGE_DIR")
        .ok()
        .or_else(|| active_mount().map(|m| format!("{m}/images")))
        .unwrap_or_else(|| LOCAL_IMAGES.to_string())
}

// ── mount / unmount ─────────────────────────────────────────────────────────────────────────────

fn mount_store(s: &Store, password: &str) -> Result<(), String> {
    std::fs::create_dir_all(&s.mount).map_err(|e| e.to_string())?;
    let mut opts = s.options.clone();
    if s.kind == "smb" {
        // Credentials file, root-only.
        if let Some(d) = FsPath::new(SMB_CREDS).parent() {
            let _ = std::fs::create_dir_all(d);
        }
        std::fs::write(
            SMB_CREDS,
            format!("username={}\npassword={}\n", s.username, password),
        )
        .map_err(|e| e.to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(SMB_CREDS, std::fs::Permissions::from_mode(0o600));
        }
        let extra = format!("credentials={SMB_CREDS},uid=0,gid=0,file_mode=0660,dir_mode=0770");
        opts = if opts.is_empty() {
            extra
        } else {
            format!("{opts},{extra}")
        };
    }
    let fstype = if s.kind == "smb" { "cifs" } else { "nfs" };
    let mut args = vec!["-t", fstype, &s.remote, &s.mount];
    if !opts.is_empty() {
        args.push("-o");
        args.push(&opts);
    }
    ui::run_result("mount", &args).map_err(|e| e.to_string())?;
    // Create the media/image subdirs on the share.
    let _ = std::fs::create_dir_all(format!("{}/isos", s.mount));
    let _ = std::fs::create_dir_all(format!("{}/images", s.mount));
    persist_fstab(s, &opts);
    Ok(())
}

/// Add/replace the managed fstab line so the mount survives reboot (nofail so a dead share can't
/// block boot).
fn persist_fstab(s: &Store, opts: &str) {
    let fstype = if s.kind == "smb" { "cifs" } else { "nfs" };
    let opts = if opts.is_empty() {
        "defaults".to_string()
    } else {
        opts.to_string()
    };
    let line = format!(
        "{} {} {} {opts},nofail,_netdev 0 0  {FSTAB_TAG}",
        s.remote, s.mount, fstype
    );
    let current = std::fs::read_to_string("/etc/fstab").unwrap_or_default();
    let mut kept: Vec<String> = current
        .lines()
        .filter(|l| !l.contains(FSTAB_TAG))
        .map(String::from)
        .collect();
    kept.push(line);
    let _ = std::fs::write("/etc/fstab", kept.join("\n") + "\n");
}

fn unpersist_fstab() {
    if let Ok(current) = std::fs::read_to_string("/etc/fstab") {
        let kept: Vec<&str> = current.lines().filter(|l| !l.contains(FSTAB_TAG)).collect();
        let _ = std::fs::write("/etc/fstab", kept.join("\n") + "\n");
    }
}

// ── handlers + UI ───────────────────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct StoreForm {
    kind: String,
    remote: String,
    #[serde(default)]
    mount: String,
    #[serde(default)]
    options: String,
    #[serde(default)]
    username: String,
    #[serde(default)]
    password: String,
}

pub async fn configure(Form(f): Form<StoreForm>) -> Markup {
    let remote = f.remote.trim().to_string();
    if remote.is_empty() {
        return panel_with(Some(
            html! { div.banner.error { "A server/share address is required." } },
        ));
    }
    let s = Store {
        kind: if f.kind == "smb" { "smb" } else { "nfs" }.to_string(),
        remote,
        mount: {
            let m = f.mount.trim();
            if m.is_empty() {
                "/var/lib/tendril/store".to_string()
            } else {
                m.to_string()
            }
        },
        options: f.options.trim().to_string(),
        username: f.username.trim().to_string(),
    };
    match mount_store(&s, &f.password) {
        Ok(()) => {
            let _ = save(&s);
            panel_with(Some(
                html! { div.banner.ok { "Mounted " (s.remote) " at " (s.mount) ". ISOs and images now live on the share." } },
            ))
        }
        Err(e) => panel_with(Some(html! { div.banner.error { "Mount failed: " (e) } })),
    }
}

pub async fn unmount() -> Markup {
    if let Some(s) = load() {
        let _ = ui::run_result("umount", &[&s.mount]);
        unpersist_fstab();
        let _ = std::fs::remove_file(conf_path());
        let _ = std::fs::remove_file(SMB_CREDS);
    }
    panel_with(Some(
        html! { div.banner.ok { "Disconnected the remote store. ISOs and images use local storage again." } },
    ))
}

pub fn panel() -> Markup {
    panel_with(None)
}

fn panel_with(note: Option<Markup>) -> Markup {
    let store = load();
    let mounted = store
        .as_ref()
        .map(|s| is_mounted(&s.mount))
        .unwrap_or(false);
    html! {
        div #storage {
            div.pad {
                @if let Some(n) = note { (n) }
                @if ui::is_demo() {
                    p.muted { "In the demo, media and images are canned. On a real install this connects an NFS/SMB share so every node shares the same media and golden images." }
                } @else if let (Some(s), true) = (store.as_ref(), mounted) {
                    p { "Connected: " strong { (s.remote) } " (" (s.kind.to_uppercase()) ") mounted at " span.mono { (s.mount) } "." }
                    p.sub { "ISOs: " span.mono { (iso_dir()) } " · Images: " span.mono { (image_dir()) } }
                    button.btn.danger hx-post="/storage/unmount" hx-target="#storage" hx-swap="outerHTML"
                        hx-confirm="Disconnect the remote store? ISOs/images fall back to local storage; anything only on the share won't be visible until reconnected." { "Disconnect" }
                } @else {
                    @if store.is_some() && !mounted {
                        div.banner.warn { "A store is configured but not mounted (share unreachable?). Re-enter to remount." }
                    }
                    p.sub { "Media and images currently use local storage. Connect an NFS or SMB share to put them on shared network storage." }
                    form hx-post="/storage/configure" hx-target="#storage" hx-swap="outerHTML" {
                        div.grid {
                            div.field { label { "Type" } select name="kind" {
                                option value="nfs" { "NFS" } option value="smb" { "SMB / CIFS" } } }
                            div.field { label { "Server / share" } input name="remote" placeholder="nfs: 10.0.0.5:/tank/tendril  ·  smb: //10.0.0.5/tendril" required; }
                            div.field { label { "Mount point" } input name="mount" placeholder="/var/lib/tendril/store"; }
                            div.field { label { "Options (optional)" } input name="options" placeholder="e.g. vers=4.1  ·  vers=3.0"; }
                            div.field { label { "SMB username (SMB only)" } input name="username"; }
                            div.field { label { "SMB password (SMB only)" } input type="password" name="password"; }
                        }
                        button.btn.primary type="submit" style="margin-top:10px" { "Connect & mount" }
                    }
                    p.sub style="margin-top:8px" { "The mount is saved to /etc/fstab (nofail) so it reconnects on boot. Station disks stay local." }
                }
            }
        }
    }
}

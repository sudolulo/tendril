//! Remote media/image storage. Point Tendril's `isos/` and `images/` at a mounted **NFS or SMB**
//! share so every node (and station-image clone) sees the same media and golden images — the shared
//! store behind federation. Station **disks** stay local (fast, per-node); only media + images move.
//!
//! Config lives at `/etc/tendril/storage.conf` (`key=value` lines). When a store is configured and
//! actually mounted, `iso_dir()`/`image_dir()` resolve to `<mount>/isos` and `<mount>/images`;
//! otherwise they fall back to the local defaults. The mount is persisted in `/etc/fstab` (with
//! `nofail,_netdev`) so it survives reboots.

use std::path::Path as FsPath;

use axum::extract::Form;
use maud::{html, Markup, PreEscaped};
use serde::Deserialize;

use crate::ui;

const LOCAL_ISOS: &str = "/var/lib/tendril/isos";
const LOCAL_IMAGES: &str = "/var/lib/tendril/images";
const SMB_CREDS: &str = "/etc/tendril/smb-creds";
const FSTAB_TAG: &str = "# tendril-store (managed)";
/// Default mount point when the user leaves it blank.
const DEFAULT_MOUNT: &str = "/var/lib/tendril/store";

/// Sensible default mount options per type when the user leaves the field blank. SMB benefits from a
/// modern protocol version (the 1.0 default is insecure/often disabled); NFS negotiates on its own.
fn default_options(kind: &str) -> &'static str {
    if kind == "smb" {
        "vers=3.0"
    } else {
        ""
    }
}

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
    pub tls: bool,        // nfs only: encrypt the transport with RPC-over-TLS (xprtsec=tls)
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
                "tls" => s.tls = v == "true",
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
            "type={}\nremote={}\nmount={}\noptions={}\nusername={}\ntls={}\n",
            s.kind, s.remote, s.mount, s.options, s.username, s.tls
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

/// The mounted shared store's root — the fleet coordination point (node presence + shared token).
/// `None` when this node runs on local storage only (federation then needs manual peers).
pub fn store_root() -> Option<String> {
    std::env::var("TENDRIL_STORE_ROOT")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(active_mount)
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

/// The fleet station registry: env override, else the mounted store's `registry/`, else local. Shared
/// so a node's stations are known even when the node itself is down (for re-home).
pub fn registry_dir() -> String {
    std::env::var("TENDRIL_REGISTRY_DIR")
        .ok()
        .or_else(|| active_mount().map(|m| format!("{m}/registry")))
        .unwrap_or_else(|| "/var/lib/tendril/registry".to_string())
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
    } else if s.tls {
        // NFS over TLS (RPC-with-TLS) — encrypts the transport without Kerberos. Requires a recent
        // kernel + `tlshd` (ktls-utils) running on both this node and the NFS server.
        opts = if opts.is_empty() {
            "xprtsec=tls".to_string()
        } else {
            format!("{opts},xprtsec=tls")
        };
    }
    let fstype = if s.kind == "smb" { "cifs" } else { "nfs" };
    let mut args = vec!["-t", fstype, &s.remote, &s.mount];
    if !opts.is_empty() {
        args.push("-o");
        args.push(&opts);
    }
    ui::run_result("mount", &args).map_err(|e| e.to_string())?;
    // Create the media/image/registry subdirs on the share.
    let _ = std::fs::create_dir_all(format!("{}/isos", s.mount));
    let _ = std::fs::create_dir_all(format!("{}/images", s.mount));
    let _ = std::fs::create_dir_all(format!("{}/registry", s.mount));
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
    #[serde(default)]
    tls: String,
}

pub async fn configure(Form(f): Form<StoreForm>) -> Markup {
    let remote = f.remote.trim().to_string();
    if remote.is_empty() {
        return panel_with(Some(
            html! { div.banner.error { "A server/share address is required." } },
        ));
    }
    let kind = if f.kind == "smb" { "smb" } else { "nfs" }.to_string();
    let mount = {
        let m = f.mount.trim();
        if m.is_empty() {
            DEFAULT_MOUNT.to_string()
        } else {
            m.to_string()
        }
    };
    let options = {
        let o = f.options.trim();
        if o.is_empty() {
            default_options(&kind).to_string()
        } else {
            o.to_string()
        }
    };
    let s = Store {
        tls: kind == "nfs" && !f.tls.trim().is_empty(),
        kind,
        remote,
        mount,
        options,
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
                            div.field { label { "Type" } select #store-kind name="kind" onchange="tendrilStore()" {
                                option value="nfs" { "NFS" } option value="smb" { "SMB / CIFS" } } }
                            div.field { label { "Server / share" } input #store-remote name="remote" required; }
                            div.field.store-smb { label { "SMB username" } input name="username"; }
                            div.field.store-smb { label { "SMB password" } input type="password" name="password"; }
                        }
                        div.field.check.store-nfs style="margin-top:8px" {
                            input type="checkbox" name="tls" id="store-tls";
                            label for="store-tls" { "Encrypt with TLS (NFS-over-TLS)" }
                            span.hint { "Optional. Encrypts the NFS transport without Kerberos (adds xprtsec=tls) — needs a recent kernel and tlshd running on this node and the NFS server. Off = plain NFS (use only on trusted networks)." }
                        }
                        details.advanced.wide style="margin-top:4px" {
                            summary { "Advanced: mount point & options" }
                            div.grid {
                                div.field { label { "Mount point" } input name="mount" placeholder=(DEFAULT_MOUNT); }
                                div.field { label { "Mount options" } input #store-options name="options"; }
                            }
                            p.sub style="margin:6px 0 2px" { "Leave blank for sensible defaults — mounts at " span.mono { (DEFAULT_MOUNT) } ", and SMB negotiates SMB3." }
                        }
                        button.btn.primary type="submit" style="margin-top:10px" { "Connect & mount" }
                    }
                    p.sub style="margin-top:8px" { "The mount is saved to /etc/fstab (nofail) so it reconnects on boot. Station disks stay local." }
                    (PreEscaped(
                        "<script>window.tendrilStore=function(){\
                         var k=document.getElementById('store-kind');if(!k)return;var smb=k.value==='smb';\
                         document.querySelectorAll('.store-smb').forEach(function(e){e.style.display=smb?'':'none';});\
                         document.querySelectorAll('.store-nfs').forEach(function(e){e.style.display=smb?'none':'';});\
                         var r=document.getElementById('store-remote');\
                         if(r)r.placeholder=smb?'//10.0.0.5/tendril':'10.0.0.5:/tank/tendril';\
                         var o=document.getElementById('store-options');\
                         if(o)o.placeholder=smb?'vers=3.0':'defaults (kernel negotiates)';\
                         };tendrilStore();</script>"
                    ))
                }
            }
        }
    }
}

//! Saved station images — capture an installed station's disk as a reusable golden image (stored in
//! media), then clone new stations from it. Cloning uses a qcow2 overlay (see
//! `orchestrator::guest::create_overlay`): instant and deduplicated. Saving flattens + compresses the
//! disk into a standalone, portable image — the basis for shipping a built station to other machines
//! (clustering).

use std::path::Path as FsPath;

use axum::extract::{Form, Path, Query};
use maud::{html, Markup};
use serde::Deserialize;

use tendril_orchestrator::{DomainState, Libvirt};

use crate::ui;

/// Where golden images live — resolves to a mounted remote store's `images/` when configured, else
/// local (see `storage::image_dir`).
pub fn images_dir() -> String {
    crate::storage::image_dir()
}

/// Saved images as (name, human-readable size). Names are the `.qcow2` basename.
pub fn list() -> Vec<(String, String)> {
    if ui::is_demo() {
        return crate::demo::images();
    }
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(images_dir()) {
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().into_owned();
            if let Some(base) = n.strip_suffix(".qcow2") {
                let sz = e.metadata().map(|m| human(m.len())).unwrap_or_default();
                out.push((base.to_string(), sz));
            }
        }
    }
    out.sort();
    out
}

/// Full path of a saved image, guarding against traversal; `None` if it doesn't exist.
pub fn path_of(name: &str) -> Option<String> {
    let clean = sanitize(name);
    if clean.is_empty() {
        return None;
    }
    let p = format!("{}/{clean}.qcow2", images_dir());
    FsPath::new(&p).exists().then_some(p)
}

/// Keep image names to a safe charset (they become file names and query values).
fn sanitize(name: &str) -> String {
    name.trim()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        .collect()
}

fn human(n: u64) -> String {
    let (n, u) = if n >= 1 << 30 {
        (n as f64 / (1u64 << 30) as f64, "GB")
    } else {
        (n as f64 / (1u64 << 20) as f64, "MB")
    };
    format!("{n:.1} {u}")
}

/// A station's primary disk path, via virsh.
fn station_disk(name: &str) -> Option<String> {
    let out = ui::run_stdout(
        "virsh",
        &["-c", "qemu:///system", "domblklist", "--details", name],
    )?;
    out.lines().find_map(|l| {
        let c: Vec<&str> = l.split_whitespace().collect();
        (c.len() >= 4 && c[1] == "disk").then(|| c[3].to_string())
    })
}

// ── handlers ──────────────────────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct SaveForm {
    image_name: String,
}

/// Capture a (shut-off) station's disk as a compressed standalone golden image.
pub async fn save(Path(station): Path<String>, Form(f): Form<SaveForm>) -> Markup {
    let name = sanitize(&f.image_name);
    if name.is_empty() {
        return note(false, "Image name required (letters, numbers, - _ .).");
    }
    let lv = Libvirt::system();
    if matches!(lv.state(&station), DomainState::Running) {
        return note(
            false,
            "Shut the station down first so its disk is captured consistently.",
        );
    }
    let Some(src) = station_disk(&station) else {
        return note(false, "Couldn't find the station's disk.");
    };
    let dir = images_dir();
    let dest = format!("{dir}/{name}.qcow2");
    if FsPath::new(&dest).exists() {
        return note(false, "An image with that name already exists.");
    }
    let _ = std::fs::create_dir_all(&dir);
    // Flatten + compress into a portable standalone image (no backing chain).
    match ui::run_result("qemu-img", &["convert", "-c", "-O", "qcow2", &src, &dest]) {
        Ok(_) => note(
            true,
            &format!("Saved image \u{201c}{name}\u{201d}. Pick it as a base in the create-station wizard."),
        ),
        Err(e) => note(false, &format!("Save failed: {e}")),
    }
}

#[derive(Deserialize)]
pub struct NameQuery {
    name: String,
}

pub async fn delete(Query(q): Query<NameQuery>) -> Markup {
    if let Some(p) = path_of(&q.name) {
        let _ = std::fs::remove_file(p);
    }
    panel()
}

fn note(ok: bool, msg: &str) -> Markup {
    html! { div class=(if ok { "banner ok" } else { "banner error" }) style="margin:0" { (msg) } }
}

// ── UI ──────────────────────────────────────────────────────────────────────────────────────

/// The saved-images panel for the Media page.
pub fn panel() -> Markup {
    let imgs = list();
    html! {
        div #images {
            div.pad {
                @if imgs.is_empty() {
                    p.muted { "No saved images yet. Open a station that's shut off and use " strong { "Save as image" } " to capture its installed disk as a reusable template." }
                } @else {
                    div.scroll { table {
                        thead { tr { th { "Image" } th.right { "Size" } th.right { "" } } }
                        tbody { @for (n, sz) in &imgs {
                            tr {
                                td.mono { (n) }
                                td.right.num { (sz) }
                                td.right {
                                    button.btn.sm.danger
                                        hx-post=(format!("/images/delete?name={}", urlencode(n)))
                                        hx-target="#images" hx-swap="outerHTML"
                                        hx-confirm=(format!("Delete image '{n}'? Stations cloned from it (overlays) depend on it and will break.")) { "Delete" }
                                }
                            }
                        } }
                    } }
                }
                p.sub style="margin-top:10px" { "Golden images are qcow2 templates. New stations clone them as copy-on-write overlays — instant and deduplicated (the base is shared, not copied) — which is the groundwork for shipping a built station to other machines." }
            }
        }
    }
}

/// Minimal percent-encoding for an image name in a query string.
fn urlencode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' | b'.' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

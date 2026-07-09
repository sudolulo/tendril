//! Shared XML escaping for the orchestrator's hand-rendered XML documents (the libvirt domain and
//! the Windows answer file).

/// Escape a value for a single-quoted XML attribute / text node (`& < > ' "`).
///
/// Defense in depth for the domain renderer: the web layer validates station names, but the
/// renderer is a public library API and disk/ISO paths are free-form. Also load-bearing in the
/// answer file for URLs with query strings (e.g. Discord's `?channel=stable&platform=win`), whose
/// raw `&` would otherwise be invalid XML — the answer-file parser turns these back into literal
/// characters, and the URLs are double-quoted on the command line so cmd doesn't treat a decoded
/// `&` as a separator.
pub(crate) fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('\'', "&apos;")
        .replace('"', "&quot;")
}

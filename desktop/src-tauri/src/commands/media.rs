//! Local filename sanitization for native save-file dialogs.
//!
//! The legacy remote media pipeline (upload, download, fetch, token-authed
//! HTTP) was removed in the M3 cutover — the packaged app has no media
//! transport yet, so media upload/download is gone. What remains here is the
//! local-only filename sanitizer used by the native save-file path (e.g.
//! QR/PNG download).

/// Sanitize a filename for use as a display label or save-dialog suggestion.
///
/// Strips any directory components (keeps only the final path segment), removes
/// control characters, and bounds length to 255. Returns a fallback when the
/// result would be empty.
pub(crate) fn sanitize_filename(name: &str) -> String {
    // Keep only the final path segment — defend against `../` and absolute paths
    // regardless of separator style.
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name).trim();
    let cleaned: String = base.chars().filter(|c| !c.is_control()).take(255).collect();
    if cleaned.is_empty() {
        "file".to_string()
    } else {
        cleaned
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sanitize_filename() {
        assert_eq!(sanitize_filename("report.pdf"), "report.pdf");
        // Strips directory components and traversal.
        assert_eq!(sanitize_filename("../../etc/passwd"), "passwd");
        assert_eq!(sanitize_filename("/abs/path/notes.txt"), "notes.txt");
        assert_eq!(sanitize_filename(r"C:\Users\me\doc.docx"), "doc.docx");
        // Empty / separator-only falls back.
        assert_eq!(sanitize_filename(""), "file");
        assert_eq!(sanitize_filename("/"), "file");
        // Control chars removed.
        assert_eq!(sanitize_filename("a\nb\tc.txt"), "abc.txt");
    }
}

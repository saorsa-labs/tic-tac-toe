//! Native clipboard write for the renderer.
//!
//! The legacy remote media download/fetch pipeline (download_image,
//! download_file, fetch_media_bytes, fetch_snapshot_bytes,
//! copy_image_to_clipboard) was removed in the M3 cutover — the packaged app
//! has no remote media transport. What remains is the native text-clipboard
//! write, which the renderer uses for delayed clipboard writes that WebKit
//! permission revocation would otherwise break.

use crate::commands::clipboard::with_clipboard;

/// Write text (optionally HTML) to the system clipboard through the native
/// shell.
///
/// WebKit can revoke browser clipboard permission after a user action awaits a
/// long-running operation. Keeping the delayed write in the native layer makes
/// that flow reliable on macOS.
#[tauri::command]
pub async fn copy_text_to_clipboard(
    text: String,
    html: Option<String>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    let (tx, rx) = std::sync::mpsc::sync_channel::<Result<(), String>>(1);
    let clipboard_app = app.clone();
    app.run_on_main_thread(move || {
        let result = with_clipboard(&clipboard_app, |clipboard| {
            if let Some(html) = html {
                clipboard.set_html(html, Some(text))
            } else {
                clipboard.set_text(text)
            }
        });
        let _ = tx.send(result);
    })
    .map_err(|e| format!("main thread dispatch failed: {e}"))?;

    rx.recv()
        .map_err(|_| "clipboard result channel closed unexpectedly".to_string())?
}

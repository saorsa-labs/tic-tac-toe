use tauri::{Emitter, Manager};
use url::Url;

fn activate_main_window(app: &tauri::AppHandle) {
    let Some(window) = app.get_webview_window("main") else {
        return;
    };

    if let Err(error) = window.unminimize() {
        eprintln!("buzz-desktop: failed to unminimize main window for deep link: {error}");
    }
    if let Err(error) = window.show() {
        eprintln!("buzz-desktop: failed to show main window for deep link: {error}");
    }
    if let Err(error) = window.set_focus() {
        eprintln!("buzz-desktop: failed to focus main window for deep link: {error}");
    }
}

fn parse_message_deep_link(url: &Url) -> Option<serde_json::Value> {
    let mut channel: Option<String> = None;
    let mut message_id: Option<String> = None;
    let mut thread: Option<String> = None;
    for (key, value) in url.query_pairs() {
        let value = value.into_owned();
        if value.is_empty() {
            continue;
        }
        match key.as_ref() {
            "channel" => channel = Some(value),
            "id" => message_id = Some(value),
            "thread" => thread = Some(value),
            _ => {}
        }
    }
    Some(serde_json::json!({
        "channelId": channel?,
        "messageId": message_id?,
        "threadRootId": thread,
    }))
}

/// Handle application message deep links.
///
/// Only `buzz://message?…` links are honoured. Native `x0x://invite/...` links
/// are not accepted: the secure-group invite bootstrap is gated pending x0x
/// frontier review, so no invite is queued, emitted, or consumed here.
pub(crate) fn handle_deep_link_url(app: &tauri::AppHandle, url_str: &str) {
    let url = match Url::parse(url_str) {
        Ok(url) => url,
        Err(error) => {
            eprintln!("buzz-desktop: invalid deep link URL {url_str:?}: {error}");
            return;
        }
    };

    if url.scheme() != "buzz" {
        eprintln!("buzz-desktop: ignoring unsupported deep link scheme: {url_str}");
        return;
    }

    match url.host_str() {
        Some("message") => {
            let Some(payload) = parse_message_deep_link(&url) else {
                eprintln!("buzz-desktop: message deep link missing channel or id: {url_str}");
                return;
            };
            activate_main_window(app);
            let _ = app.emit("deep-link-message", payload);
        }
        Some(action) => {
            eprintln!("buzz-desktop: unknown deep link action: {action}");
        }
        None => {
            eprintln!("buzz-desktop: deep link missing action: {url_str}");
        }
    }
}

#[cfg(test)]
mod tests {
    use url::Url;

    use super::parse_message_deep_link;
    #[test]
    fn parse_message_deep_link_extracts_required_params() {
        let url = Url::parse("buzz://message?channel=abc&id=xyz").unwrap();
        let payload = parse_message_deep_link(&url).expect("required params present");
        assert_eq!(payload["channelId"], "abc");
        assert_eq!(payload["messageId"], "xyz");
        assert!(payload["threadRootId"].is_null());
    }

    #[test]
    fn parse_message_deep_link_includes_thread_root() {
        let url = Url::parse("buzz://message?channel=abc&id=xyz&thread=root1").unwrap();
        let payload = parse_message_deep_link(&url).expect("required params present");
        assert_eq!(payload["threadRootId"], "root1");
    }

    #[test]
    fn parse_message_deep_link_rejects_missing_or_empty_required_params() {
        for raw in [
            "buzz://message?channel=abc",
            "buzz://message?channel=&id=foo",
            "buzz://message?channel=abc&id=",
        ] {
            let url = Url::parse(raw).unwrap();
            assert!(parse_message_deep_link(&url).is_none(), "accepted {raw}");
        }
    }

    #[test]
    fn parse_message_deep_link_treats_empty_thread_as_absent() {
        let url = Url::parse("buzz://message?channel=abc&id=xyz&thread=").unwrap();
        let payload = parse_message_deep_link(&url).expect("required params present");
        assert!(payload["threadRootId"].is_null());
    }
}

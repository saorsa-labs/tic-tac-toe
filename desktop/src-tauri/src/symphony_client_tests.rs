// Deterministic tests for the symphony typed client: loopback fail-closed
// validation, SSE frame parsing, and error Display token secrecy.
#![allow(dead_code)]
use super::*;

fn http() -> reqwest::Client {
    reqwest::Client::new()
}

// ── Loopback fail-closed ────────────────────────────────────────────────────

#[test]
fn rejects_non_loopback_host() {
    let err = SymphonyClient::new("http://example.com:1234", "tok".into(), http())
        .expect_err("non-loopback must fail");
    assert!(matches!(err, SymphonyClientError::NotLoopback { .. }));
    let msg = format!("{err}");
    assert!(msg.contains("example.com"));
    assert!(!msg.contains("tok"), "token leaked into NotLoopback error");
}

#[test]
fn rejects_public_ip() {
    let err = SymphonyClient::new("http://8.8.8.8:1234", "tok".into(), http())
        .expect_err("public IP must fail");
    assert!(matches!(err, SymphonyClientError::NotLoopback { .. }));
}

#[test]
fn rejects_https_scheme() {
    // Only http:// is permitted on loopback (the daemon is plain HTTP).
    let err = SymphonyClient::new("https://127.0.0.1:1234", "tok".into(), http())
        .expect_err("https must fail closed");
    assert!(matches!(err, SymphonyClientError::NotLoopback { .. }));
}

#[test]
fn rejects_unparseable_url() {
    assert!(SymphonyClient::new("not a url", "tok".into(), http()).is_err());
}

#[test]
fn accepts_loopback_endpoints() {
    assert!(SymphonyClient::new("http://127.0.0.1:1", "t".into(), http()).is_ok());
    assert!(SymphonyClient::new("http://localhost:1", "t".into(), http()).is_ok());
    assert!(SymphonyClient::new("http://[::1]:1", "t".into(), http()).is_ok());
}

#[test]
fn trailing_slash_normalized() {
    let c = SymphonyClient::new("http://127.0.0.1:1/", "t".into(), http()).unwrap();
    // event_stream_url must not double-slash.
    assert_eq!(
        c.event_stream_url(),
        "http://127.0.0.1:1/symphony/events?token=t"
    );
}

#[test]
fn debug_redacts_token() {
    let c = SymphonyClient::new("http://127.0.0.1:1", "supersecret".into(), http()).unwrap();
    let dbg = format!("{c:?}");
    assert!(dbg.contains("127.0.0.1"));
    assert!(
        !dbg.contains("supersecret"),
        "token leaked into client Debug"
    );
    assert!(dbg.contains("<redacted>"));
}

// ── SSE frame parsing ───────────────────────────────────────────────────────

#[test]
fn parses_event_and_data() {
    let mut buf = "event:task_claimed\ndata:{\"id\":\"x\"}\n\n".to_string();
    let frame = parse_sse_frame(&mut buf).expect("one frame");
    assert_eq!(frame.event, "task_claimed");
    assert_eq!(frame.data, "{\"id\":\"x\"}");
    assert!(buf.is_empty(), "buffer drained: {buf:?}");
}

#[test]
fn parses_crlf_terminated_frame() {
    let mut buf = "event:handoff\ndata:ok\r\n\r\n".to_string();
    let frame = parse_sse_frame(&mut buf).expect("one crlf frame");
    assert_eq!(frame.event, "handoff");
    assert_eq!(frame.data, "ok");
}

#[test]
fn heartbeat_parses() {
    let mut buf = "event:heartbeat\ndata:ok\n\n".to_string();
    let frame = parse_sse_frame(&mut buf).unwrap();
    assert_eq!(frame.event, "heartbeat");
    assert_eq!(frame.data, "ok");
}

#[test]
fn data_only_frame_defaults_to_message_event() {
    let mut buf = "data:hello\n\n".to_string();
    let frame = parse_sse_frame(&mut buf).unwrap();
    assert_eq!(frame.event, "message");
    assert_eq!(frame.data, "hello");
}

#[test]
fn multi_data_lines_joined() {
    let mut buf = "event:proof\ndata:line1\ndata:line2\n\n".to_string();
    let frame = parse_sse_frame(&mut buf).unwrap();
    assert_eq!(frame.data, "line1\nline2");
}

#[test]
fn comments_ignored() {
    let mut buf = ":keep-alive\n\n".to_string();
    assert!(
        parse_sse_frame(&mut buf).is_none(),
        "comment-only is not a frame"
    );
    assert!(buf.is_empty());
}

#[test]
fn incomplete_buffer_returns_none() {
    let mut buf = "event:task\ndata:par".to_string(); // no terminator yet
    assert!(parse_sse_frame(&mut buf).is_none());
    assert_eq!(buf, "event:task\ndata:par", "partial frame preserved");
}

#[test]
fn multiple_frames_in_one_buffer() {
    let mut buf = "event:a\ndata:1\n\nevent:b\ndata:2\n\n".to_string();
    let f1 = parse_sse_frame(&mut buf).unwrap();
    assert_eq!(f1.event, "a");
    let f2 = parse_sse_frame(&mut buf).unwrap();
    assert_eq!(f2.event, "b");
    assert!(buf.is_empty());
}

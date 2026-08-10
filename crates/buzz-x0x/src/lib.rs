use std::fmt::{Display, Formatter};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use reqwest::{Client, Method, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

const MAX_AGENT_ID_BYTES: usize = 64;
const MAX_GROUP_ID_BYTES: usize = 64;
const MAX_MENTIONS: usize = 50;
const MAX_PUBLIC_MESSAGE_BYTES: usize = 64 * 1024;
const MAX_API_TOKEN_BYTES: usize = 4 * 1024;
const MAX_API_RESPONSE_BYTES: usize = 1024 * 1024;
const HTTP_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug)]
pub struct AppError(String);

impl AppError {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl Display for AppError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for AppError {}

#[derive(Clone, Debug)]
pub struct RuntimeConfig {
    data_dir: PathBuf,
    agent_id: String,
    owner_agent_id: String,
    group_id: String,
}

impl RuntimeConfig {
    pub fn from_env() -> Result<Self, AppError> {
        let data_dir = required_env("X0X_DATA_DIR")?;
        let data_dir = PathBuf::from(data_dir);
        if !data_dir.is_absolute() {
            return Err(AppError::new("X0X_DATA_DIR must be an absolute path"));
        }

        let agent_id = required_env("X0X_AGENT_ID")?;
        validate_hex_id("X0X_AGENT_ID", &agent_id, MAX_AGENT_ID_BYTES)?;
        let owner_agent_id = required_env("X0X_OWNER_AGENT_ID")?;
        validate_hex_id("X0X_OWNER_AGENT_ID", &owner_agent_id, MAX_AGENT_ID_BYTES)?;
        let group_id = required_env("X0X_GROUP_ID")?;
        validate_hex_id("X0X_GROUP_ID", &group_id, MAX_GROUP_ID_BYTES)?;

        Ok(Self {
            data_dir,
            agent_id,
            owner_agent_id,
            group_id,
        })
    }

    #[cfg(test)]
    fn for_test(
        data_dir: PathBuf,
        agent_id: impl Into<String>,
        owner_agent_id: impl Into<String>,
        group_id: impl Into<String>,
    ) -> Self {
        Self {
            data_dir,
            agent_id: agent_id.into(),
            owner_agent_id: owner_agent_id.into(),
            group_id: group_id.into(),
        }
    }

    pub fn group_id(&self) -> &str {
        &self.group_id
    }
}

#[derive(Clone)]
pub struct X0xTools {
    config: RuntimeConfig,
    client: Client,
}

impl X0xTools {
    pub fn new(config: RuntimeConfig) -> Result<Self, AppError> {
        let client = Client::builder()
            // This client is deliberately incapable of inheriting an HTTP
            // proxy from the desktop environment. Every accepted endpoint is
            // a numeric loopback SocketAddr resolved from x0xd's control file.
            .no_proxy()
            .connect_timeout(Duration::from_secs(5))
            .timeout(HTTP_TIMEOUT)
            .build()
            .map_err(|error| AppError::new(format!("failed to create HTTP client: {error}")))?;
        Ok(Self { config, client })
    }

    pub fn tools_list(&self) -> Value {
        json!({
            "tools": [
                {
                    "name": "space_members",
                    "description": "List active and banned members in this managed agent's native x0x space.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {},
                        "additionalProperties": false
                    }
                },
                {
                    "name": "space_send",
                    "description": "Send a native x0x space message, optionally mentioning members or replying in a thread.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "text": {
                                "type": "string",
                                "description": "Message text."
                            },
                            "mentions": {
                                "type": "array",
                                "items": {
                                    "type": "string",
                                    "pattern": "^[0-9a-f]{64}$"
                                },
                                "maxItems": MAX_MENTIONS
                            },
                            "thread_root": {
                                "type": "string",
                                "pattern": "^[0-9a-f]{64}$"
                            },
                            "thread_parent": {
                                "type": "string",
                                "pattern": "^[0-9a-f]{64}$"
                            }
                        },
                        "required": ["text"],
                        "additionalProperties": false
                    }
                }
            ]
        })
    }

    pub async fn call_tool(&self, name: &str, arguments: Value) -> Result<Value, AppError> {
        match name {
            "space_members" => {
                validate_no_arguments(&arguments)?;
                self.space_members().await
            }
            "space_send" => {
                let request = parse_send_arguments(arguments)?;
                self.space_send(request).await
            }
            _ => Err(AppError::new(format!("unknown tool: {name}"))),
        }
    }

    async fn space_members(&self) -> Result<Value, AppError> {
        let path = format!("/groups/{}/members", self.config.group_id);
        self.request_json(Method::GET, &path, None).await
    }

    async fn space_send(&self, request: SpaceSend) -> Result<Value, AppError> {
        let body = build_group_send_body(request)?;
        let path = format!("/groups/{}/send", self.config.group_id);
        self.request_json(Method::POST, &path, Some(body)).await
    }

    async fn request_json(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<Value, AppError> {
        // Read both files for every tool call. x0xd may rotate its listener or
        // durable token during a managed-agent restart.
        let endpoint = resolve_endpoint(&self.config.data_dir)?;
        let url = format!("{}{}", endpoint.base_url, path);
        let mut request = self
            .client
            .request(method, url)
            .bearer_auth(endpoint.api_token);
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request
            .send()
            .await
            .map_err(|error| AppError::new(format!("x0xd request failed: {error}")))?;
        decode_response(response).await
    }

    pub fn server_instructions(&self) -> String {
        format!(
            "Native x0x space tools for managed agent {} owned by {} in space {}. No relay or Nostr transport is available.",
            self.config.agent_id, self.config.owner_agent_id, self.config.group_id
        )
    }
}

struct Endpoint {
    base_url: String,
    api_token: String,
}

fn resolve_endpoint(data_dir: &Path) -> Result<Endpoint, AppError> {
    let address_text = read_control_file(&data_dir.join("api.port"), "api.port")?;
    let address: SocketAddr = address_text
        .parse()
        .map_err(|_| AppError::new("api.port must contain a numeric socket address"))?;
    if !address.ip().is_loopback() || address.port() == 0 {
        return Err(AppError::new(
            "api.port must identify a non-zero loopback listener",
        ));
    }
    let api_token = read_control_file(&data_dir.join("api-token"), "api-token")?;
    if api_token.len() > MAX_API_TOKEN_BYTES
        || api_token
            .chars()
            .any(|character| character.is_control() || character.is_whitespace())
    {
        return Err(AppError::new("api-token has an invalid format"));
    }
    let host = match address {
        SocketAddr::V4(value) => value.ip().to_string(),
        SocketAddr::V6(value) => format!("[{}]", value.ip()),
    };
    Ok(Endpoint {
        base_url: format!("http://{host}:{}", address.port()),
        api_token,
    })
}

fn read_control_file(path: &Path, label: &str) -> Result<String, AppError> {
    let value = std::fs::read_to_string(path)
        .map_err(|error| AppError::new(format!("failed to read {label}: {error}")))?;
    let value = value.trim();
    if value.is_empty() {
        return Err(AppError::new(format!("{label} is empty")));
    }
    Ok(value.to_owned())
}

async fn decode_response(response: reqwest::Response) -> Result<Value, AppError> {
    let status = response.status();
    let bytes = response
        .bytes()
        .await
        .map_err(|error| AppError::new(format!("failed to read x0xd response: {error}")))?;
    if bytes.len() > MAX_API_RESPONSE_BYTES {
        return Err(AppError::new("x0xd response exceeds the size limit"));
    }
    let value: Value = serde_json::from_slice(&bytes)
        .map_err(|error| AppError::new(format!("x0xd returned invalid JSON: {error}")))?;
    if status.is_success() {
        return Ok(value);
    }
    Err(http_error(status, &value))
}

fn http_error(status: StatusCode, value: &Value) -> AppError {
    let detail = value
        .get("error")
        .and_then(Value::as_str)
        .unwrap_or("request rejected");
    AppError::new(format!("x0xd returned HTTP {}: {detail}", status.as_u16()))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SpaceSend {
    text: String,
    #[serde(default)]
    mentions: Vec<String>,
    #[serde(default)]
    thread_root: Option<String>,
    #[serde(default)]
    thread_parent: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ChannelMessageEnvelope<'a> {
    text: &'a str,
    created_at: u64,
    client_id: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    mentions: Vec<String>,
}

#[derive(Serialize)]
struct GroupSendBody {
    body: String,
    kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    thread_root: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    thread_parent: Option<String>,
}

fn parse_send_arguments(arguments: Value) -> Result<SpaceSend, AppError> {
    let request: SpaceSend = serde_json::from_value(arguments)
        .map_err(|error| AppError::new(format!("invalid space_send arguments: {error}")))?;
    if request.text.trim().is_empty() {
        return Err(AppError::new("text must not be empty"));
    }
    if request.mentions.len() > MAX_MENTIONS {
        return Err(AppError::new(format!(
            "mentions exceeds the maximum of {MAX_MENTIONS}"
        )));
    }
    for mention in &request.mentions {
        validate_hex_id("mention", mention, MAX_AGENT_ID_BYTES)?;
    }
    validate_optional_message_id("thread_root", request.thread_root.as_deref())?;
    validate_optional_message_id("thread_parent", request.thread_parent.as_deref())?;
    if request.thread_parent.is_some() && request.thread_root.is_none() {
        return Err(AppError::new(
            "thread_parent requires thread_root to also be set",
        ));
    }
    Ok(request)
}

fn build_group_send_body(request: SpaceSend) -> Result<Value, AppError> {
    let created_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| AppError::new("system clock is before the Unix epoch"))?
        .as_millis();
    let created_at = u64::try_from(created_at)
        .map_err(|_| AppError::new("current timestamp exceeds the supported range"))?;
    let envelope = ChannelMessageEnvelope {
        text: &request.text,
        created_at,
        client_id: uuid::Uuid::new_v4().to_string(),
        mentions: request.mentions,
    };
    let body = serde_json::to_string(&envelope)
        .map_err(|error| AppError::new(format!("failed to encode message: {error}")))?;
    if body.len() > MAX_PUBLIC_MESSAGE_BYTES {
        return Err(AppError::new(format!(
            "encoded message exceeds the {MAX_PUBLIC_MESSAGE_BYTES}-byte x0xd limit"
        )));
    }
    serde_json::to_value(GroupSendBody {
        body,
        kind: "chat",
        thread_root: request.thread_root,
        thread_parent: request.thread_parent,
    })
    .map_err(|error| AppError::new(format!("failed to encode group request: {error}")))
}

fn validate_no_arguments(arguments: &Value) -> Result<(), AppError> {
    let Some(object) = arguments.as_object() else {
        return Err(AppError::new("arguments must be an object"));
    };
    if object.is_empty() {
        Ok(())
    } else {
        Err(AppError::new("space_members does not accept arguments"))
    }
}

fn validate_optional_message_id(label: &str, value: Option<&str>) -> Result<(), AppError> {
    if let Some(value) = value {
        validate_hex_id(label, value, 64)?;
    }
    Ok(())
}

fn validate_hex_id(label: &str, value: &str, expected_bytes: usize) -> Result<(), AppError> {
    if value.len() == expected_bytes
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Ok(());
    }
    Err(AppError::new(format!(
        "{label} must be exactly {expected_bytes} lowercase hexadecimal characters"
    )))
}

fn required_env(name: &str) -> Result<String, AppError> {
    std::env::var(name)
        .ok()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| AppError::new(format!("missing required environment variable {name}")))
}

pub fn initialize_result(tools: &X0xTools) -> Value {
    json!({
        "protocolVersion": "2025-06-18",
        "capabilities": { "tools": {} },
        "serverInfo": {
            "name": "buzz-x0x-mcp",
            "version": env!("CARGO_PKG_VERSION")
        },
        "instructions": tools.server_instructions()
    })
}

pub async fn handle_request(tools: &X0xTools, request: &Value) -> Option<Value> {
    let id = request.get("id").cloned()?;
    if id.is_null() {
        return None;
    }
    let method = request.get("method").and_then(Value::as_str);
    let result = match method {
        Some("initialize") => Ok(initialize_result(tools)),
        Some("ping") => Ok(json!({})),
        Some("tools/list") => Ok(tools.tools_list()),
        Some("tools/call") => handle_tool_call(tools, request).await,
        Some(other) => {
            return Some(json_rpc_error(
                id,
                -32601,
                format!("method not found: {other}"),
            ));
        }
        None => {
            return Some(json_rpc_error(
                id,
                -32600,
                "request method must be a string".to_owned(),
            ));
        }
    };

    match result {
        Ok(result) => Some(json!({ "jsonrpc": "2.0", "id": id, "result": result })),
        Err(error) => Some(tool_error_result(id, &error.to_string())),
    }
}

async fn handle_tool_call(tools: &X0xTools, request: &Value) -> Result<Value, AppError> {
    let params = request
        .get("params")
        .and_then(Value::as_object)
        .ok_or_else(|| AppError::new("tools/call params must be an object"))?;
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| AppError::new("tools/call name must be a string"))?;
    let arguments = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| Value::Object(Map::new()));
    let value = tools.call_tool(name, arguments).await?;
    let text = serde_json::to_string(&value)
        .map_err(|error| AppError::new(format!("failed to encode tool result: {error}")))?;
    Ok(json!({
        "content": [{ "type": "text", "text": text }],
        "isError": false
    }))
}

pub fn parse_error_response() -> Value {
    json_rpc_error(Value::Null, -32700, "invalid JSON".to_owned())
}

fn tool_error_result(id: Value, message: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": {
            "content": [{ "type": "text", "text": message }],
            "isError": true
        }
    })
}

fn json_rpc_error(id: Value, code: i64, message: String) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": message }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

    const AGENT_ID: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const OWNER_ID: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    const GROUP_ID: &str = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";

    #[test]
    fn send_body_matches_desktop_native_envelope_and_threads() {
        let root = "d".repeat(64);
        let request = parse_send_arguments(json!({
            "text": "work complete",
            "mentions": [OWNER_ID],
            "thread_root": root,
            "thread_parent": root
        }))
        .expect("valid arguments");
        let body = build_group_send_body(request).expect("valid send body");
        assert_eq!(body["kind"], "chat");
        assert_eq!(body["thread_root"], "d".repeat(64));
        assert_eq!(body["thread_parent"], "d".repeat(64));
        let envelope: Value =
            serde_json::from_str(body["body"].as_str().expect("body string")).expect("envelope");
        assert_eq!(envelope["text"], "work complete");
        assert_eq!(envelope["mentions"][0], OWNER_ID);
        assert!(envelope["createdAt"].as_u64().is_some());
        assert!(uuid::Uuid::parse_str(envelope["clientId"].as_str().expect("client id")).is_ok());
    }

    #[test]
    fn send_validation_rejects_bad_ids_parent_without_root_and_oversize() {
        let bad_mention = parse_send_arguments(json!({
            "text": "hello",
            "mentions": ["ABC"]
        }));
        assert!(bad_mention.is_err());

        let missing_root = parse_send_arguments(json!({
            "text": "hello",
            "thread_parent": "d".repeat(64)
        }));
        assert!(missing_root.is_err());

        let request = parse_send_arguments(json!({ "text": "x".repeat(MAX_PUBLIC_MESSAGE_BYTES) }))
            .expect("text parses before envelope size check");
        assert!(build_group_send_body(request).is_err());
    }

    #[test]
    fn endpoint_resolution_rejects_non_loopback_and_invalid_token() {
        let directory = tempfile::tempdir().expect("temp dir");
        std::fs::write(directory.path().join("api.port"), "192.0.2.1:1234\n").expect("write port");
        std::fs::write(directory.path().join("api-token"), "secret\n").expect("write token");
        assert!(resolve_endpoint(directory.path()).is_err());

        std::fs::write(directory.path().join("api.port"), "127.0.0.1:1234\n").expect("write port");
        std::fs::write(directory.path().join("api-token"), "bad token\n").expect("write token");
        assert!(resolve_endpoint(directory.path()).is_err());
    }

    #[tokio::test]
    async fn tool_calls_reread_endpoint_and_token_each_time() {
        let directory = tempfile::tempdir().expect("temp dir");
        let first = spawn_fake_server(
            "GET",
            &format!("/groups/{GROUP_ID}/members"),
            "token-one",
            None,
            json!({"ok": true, "member_count": 2}),
        );
        write_endpoint(directory.path(), first.address, "token-one");
        let tools = X0xTools::new(RuntimeConfig::for_test(
            directory.path().to_path_buf(),
            AGENT_ID,
            OWNER_ID,
            GROUP_ID,
        ))
        .expect("tools");
        let members = tools
            .call_tool("space_members", json!({}))
            .await
            .expect("members call");
        assert_eq!(members["member_count"], 2);
        first.join.join().expect("first server");

        let second = spawn_fake_server(
            "POST",
            &format!("/groups/{GROUP_ID}/send"),
            "token-two",
            Some("reply from agent"),
            json!({"ok": true, "msg_id": "e".repeat(64)}),
        );
        write_endpoint(directory.path(), second.address, "token-two");
        let sent = tools
            .call_tool(
                "space_send",
                json!({"text": "reply from agent", "mentions": [OWNER_ID]}),
            )
            .await
            .expect("send call");
        assert_eq!(sent["msg_id"], "e".repeat(64));
        second.join.join().expect("second server");
    }

    #[tokio::test]
    async fn mcp_initialize_lists_tools_and_returns_tool_errors_as_results() {
        let directory = tempfile::tempdir().expect("temp dir");
        let tools = X0xTools::new(RuntimeConfig::for_test(
            directory.path().to_path_buf(),
            AGENT_ID,
            OWNER_ID,
            GROUP_ID,
        ))
        .expect("tools");

        let initialize = handle_request(
            &tools,
            &json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}}),
        )
        .await
        .expect("initialize response");
        assert_eq!(initialize["result"]["protocolVersion"], "2025-06-18");
        assert_eq!(initialize["result"]["serverInfo"]["name"], "buzz-x0x-mcp");

        let list = handle_request(
            &tools,
            &json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}}),
        )
        .await
        .expect("tools/list response");
        let names: Vec<&str> = list["result"]["tools"]
            .as_array()
            .expect("tools array")
            .iter()
            .filter_map(|tool| tool["name"].as_str())
            .collect();
        assert_eq!(names, ["space_members", "space_send"]);

        let invalid = handle_request(
            &tools,
            &json!({
                "jsonrpc": "2.0",
                "id": 3,
                "method": "tools/call",
                "params": {"name": "space_send", "arguments": {"text": ""}}
            }),
        )
        .await
        .expect("tool error response");
        assert_eq!(invalid["result"]["isError"], true);
        assert!(invalid["result"]["content"][0]["text"]
            .as_str()
            .expect("error text")
            .contains("must not be empty"));
    }

    struct FakeServer {
        address: SocketAddr,
        join: thread::JoinHandle<()>,
    }

    fn spawn_fake_server(
        method: &'static str,
        path: &str,
        token: &'static str,
        expected_text: Option<&'static str>,
        response: Value,
    ) -> FakeServer {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake x0xd");
        let address = listener.local_addr().expect("fake address");
        let path = path.to_owned();
        let response = response.to_string();
        let join = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept request");
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .expect("read timeout");
            let request = read_http_request(&mut stream);
            assert!(request.starts_with(&format!("{method} {path} HTTP/1.1\r\n")));
            assert!(request
                .to_ascii_lowercase()
                .contains(&format!("authorization: bearer {token}").to_ascii_lowercase()));
            if let Some(text) = expected_text {
                let (_, body) = request.split_once("\r\n\r\n").expect("request body");
                let group_send: Value = serde_json::from_str(body).expect("group send JSON");
                let envelope: Value = serde_json::from_str(
                    group_send["body"].as_str().expect("native envelope string"),
                )
                .expect("native envelope JSON");
                assert_eq!(envelope["text"], text);
                assert_eq!(envelope["mentions"][0], OWNER_ID);
            }
            let wire = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                response.len(), response
            );
            stream.write_all(wire.as_bytes()).expect("write response");
        });
        FakeServer { address, join }
    }

    fn read_http_request(stream: &mut std::net::TcpStream) -> String {
        let mut bytes = Vec::new();
        let mut buffer = [0_u8; 2048];
        loop {
            let count = stream.read(&mut buffer).expect("read request");
            if count == 0 {
                break;
            }
            bytes.extend_from_slice(&buffer[..count]);
            if let Some(header_end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
                let header_end = header_end + 4;
                let headers = String::from_utf8_lossy(&bytes[..header_end]);
                let content_length = headers
                    .lines()
                    .find_map(|line| {
                        line.to_ascii_lowercase()
                            .strip_prefix("content-length: ")
                            .map(str::to_owned)
                    })
                    .and_then(|value| value.trim().parse::<usize>().ok())
                    .unwrap_or(0);
                if bytes.len() >= header_end + content_length {
                    break;
                }
            }
        }
        String::from_utf8(bytes).expect("UTF-8 request")
    }

    fn write_endpoint(directory: &Path, address: SocketAddr, token: &str) {
        std::fs::write(directory.join("api.port"), format!("{address}\n")).expect("write port");
        std::fs::write(directory.join("api-token"), format!("{token}\n")).expect("write token");
    }
}

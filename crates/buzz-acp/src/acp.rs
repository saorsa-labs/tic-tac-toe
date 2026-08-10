use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use futures_util::StreamExt;
use serde_json::{json, Value};
use thiserror::Error;
use tokio::io::AsyncWriteExt;
use tokio::process::{Child, ChildStdin, ChildStdout};
use tokio_util::codec::{FramedRead, LinesCodec, LinesCodecError};

use crate::config::Config;

const MAX_LINE_BYTES: usize = 10 * 1024 * 1024;
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Error)]
pub enum AcpError {
    #[error("failed to spawn configured agent: {0}")]
    Spawn(#[source] std::io::Error),
    #[error("configured agent did not expose piped {0}")]
    MissingPipe(&'static str),
    #[error("ACP I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("ACP frame exceeded the maximum line size")]
    OversizedFrame,
    #[error("ACP JSON decode failed: {0}")]
    Json(#[from] serde_json::Error),
    #[error("ACP agent exited before responding")]
    AgentExited,
    #[error("ACP request timed out after {0:?}")]
    Timeout(Duration),
    #[error("ACP protocol error: {0}")]
    Protocol(String),
    #[error("ACP agent error {code}: {message}")]
    Agent { code: i64, message: String },
}

pub struct AcpClient {
    child: Child,
    stdin: ChildStdin,
    stdout: FramedRead<ChildStdout, LinesCodec>,
    next_id: u64,
    idle_timeout: Duration,
    max_turn_duration: Option<Duration>,
    session_id: String,
}

impl AcpClient {
    pub async fn start(config: &Config) -> Result<Self, AcpError> {
        let mut command = tokio::process::Command::new(&config.agent_command);
        command
            .args(&config.agent_args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true);
        for (name, _) in std::env::vars_os() {
            if is_sensitive_agent_env(&name) {
                command.env_remove(name);
            }
        }
        let mut child = command.spawn().map_err(AcpError::Spawn)?;
        let stdin = child.stdin.take().ok_or(AcpError::MissingPipe("stdin"))?;
        let stdout = child.stdout.take().ok_or(AcpError::MissingPipe("stdout"))?;
        let mut client = Self {
            child,
            stdin,
            stdout: FramedRead::new(stdout, LinesCodec::new_with_max_length(MAX_LINE_BYTES)),
            next_id: 1,
            idle_timeout: config.idle_timeout,
            max_turn_duration: config.max_turn_duration,
            session_id: String::new(),
        };

        client
            .request(
                "initialize",
                json!({
                    "protocolVersion": 2,
                    "clientCapabilities": {},
                    "clientInfo": {
                        "name": "buzz-acp",
                        "version": env!("CARGO_PKG_VERSION")
                    }
                }),
            )
            .await?;

        let cwd = std::env::current_dir().map_err(AcpError::Io)?;
        let session = client
            .request(
                "session/new",
                json!({
                    "cwd": absolute_path_string(&cwd)?,
                    "mcpServers": [native_mcp_server(config)],
                    "systemPrompt": config.system_prompt,
                }),
            )
            .await?;
        client.session_id = session
            .get("sessionId")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| AcpError::Protocol("session/new omitted sessionId".to_string()))?
            .to_string();
        Ok(client)
    }

    pub async fn prompt(&mut self, prompt: &str) -> Result<String, AcpError> {
        enforce_max_turn_duration(self.max_turn_duration, self.prompt_inner(prompt)).await
    }

    async fn prompt_inner(&mut self, prompt: &str) -> Result<String, AcpError> {
        let session_id = self.session_id.clone();
        let id = self
            .send_request(
                "session/prompt",
                json!({
                    "sessionId": session_id,
                    "prompt": [{ "type": "text", "text": prompt }]
                }),
            )
            .await?;
        let mut output = String::new();
        let _ = self.read_response(id, &mut output).await?;
        Ok(output.trim().to_string())
    }

    pub async fn shutdown(&mut self) {
        let _ = self.child.start_kill();
        let _ = tokio::time::timeout(SHUTDOWN_TIMEOUT, self.child.wait()).await;
    }

    async fn request(&mut self, method: &str, params: Value) -> Result<Value, AcpError> {
        let id = self.send_request(method, params).await?;
        self.read_response(id, &mut String::new()).await
    }

    async fn send_request(&mut self, method: &str, params: Value) -> Result<u64, AcpError> {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        let mut line = serde_json::to_vec(&json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        }))?;
        line.push(b'\n');
        tokio::time::timeout(self.idle_timeout, self.stdin.write_all(&line))
            .await
            .map_err(|_| AcpError::Timeout(self.idle_timeout))??;
        self.stdin.flush().await?;
        Ok(id)
    }

    async fn read_response(
        &mut self,
        expected_id: u64,
        output: &mut String,
    ) -> Result<Value, AcpError> {
        loop {
            let next = tokio::time::timeout(self.idle_timeout, self.stdout.next())
                .await
                .map_err(|_| AcpError::Timeout(self.idle_timeout))?;
            let line = match next {
                Some(Ok(line)) => line,
                Some(Err(LinesCodecError::MaxLineLengthExceeded)) => {
                    return Err(AcpError::OversizedFrame)
                }
                Some(Err(LinesCodecError::Io(error))) => return Err(AcpError::Io(error)),
                None => return Err(AcpError::AgentExited),
            };
            let frame: Value = serde_json::from_str(&line)?;
            collect_agent_text(&frame, output);
            if frame.get("id").and_then(Value::as_u64) != Some(expected_id) {
                continue;
            }
            if let Some(error) = frame.get("error") {
                return Err(AcpError::Agent {
                    code: error.get("code").and_then(Value::as_i64).unwrap_or(-32000),
                    message: error
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown ACP agent error")
                        .to_string(),
                });
            }
            return frame
                .get("result")
                .cloned()
                .ok_or_else(|| AcpError::Protocol("response omitted result".to_string()));
        }
    }
}

async fn enforce_max_turn_duration<T, F>(
    max_turn_duration: Option<Duration>,
    future: F,
) -> Result<T, AcpError>
where
    F: std::future::Future<Output = Result<T, AcpError>>,
{
    if let Some(duration) = max_turn_duration {
        return tokio::time::timeout(duration, future)
            .await
            .map_err(|_| AcpError::Timeout(duration))?;
    }
    future.await
}

fn collect_agent_text(frame: &Value, output: &mut String) {
    let update = match frame.get("params").and_then(|params| params.get("update")) {
        Some(update) => update,
        None => return,
    };
    if update.get("sessionUpdate").and_then(Value::as_str) != Some("agent_message_chunk") {
        return;
    }
    if let Some(text) = update
        .get("content")
        .and_then(|content| content.get("text"))
        .and_then(Value::as_str)
    {
        output.push_str(text);
    }
}

fn native_mcp_server(config: &Config) -> Value {
    let command = std::env::current_exe()
        .ok()
        .map(|path| path.with_file_name(format!("buzz-x0x-mcp{}", std::env::consts::EXE_SUFFIX)))
        .filter(|path| path.is_file())
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_else(|| "buzz-x0x-mcp".to_string());
    json!({
        "name": "buzz-x0x",
        "command": command,
        "args": [],
        "env": [
            { "name": "X0X_DATA_DIR", "value": config.data_dir.to_string_lossy() },
            { "name": "X0X_AGENT_ID", "value": config.agent_id },
            { "name": "X0X_OWNER_AGENT_ID", "value": config.owner_agent_id },
            { "name": "X0X_GROUP_ID", "value": config.group_id },
        ]
    })
}

fn is_sensitive_agent_env(name: &std::ffi::OsStr) -> bool {
    name.as_encoded_bytes().starts_with(b"X0X_")
        || matches!(
            name.to_str(),
            Some(
                "NOSTR_PRIVATE_KEY"
                    | "NOSTR_SECRET_KEY"
                    | "BUZZ_PRIVATE_KEY"
                    | "BUZZ_RELAY_URL"
                    | "BUZZ_RELAY_HTTP"
                    | "BUZZ_AUTH_TAG"
                    | "BUZZ_API_TOKEN"
                    | "BUZZ_ACP_PRIVATE_KEY"
                    | "BUZZ_ACP_API_TOKEN"
                    | "BUZZ_SHARE_IDENTITY"
                    | "BUZZ_MANAGED_AGENT"
                    | "BUZZ_MANAGED_AGENT_START_NONCE"
            )
        )
}

fn absolute_path_string(path: &Path) -> Result<String, AcpError> {
    if !path.is_absolute() {
        return Err(AcpError::Protocol(
            "current working directory is not absolute".to_string(),
        ));
    }
    Ok(path.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collects_only_agent_message_chunks() {
        let mut output = String::new();
        collect_agent_text(
            &json!({
                "method": "session/update",
                "params": { "update": {
                    "sessionUpdate": "agent_message_chunk",
                    "content": { "type": "text", "text": "hello" }
                }}
            }),
            &mut output,
        );
        collect_agent_text(
            &json!({
                "method": "session/update",
                "params": { "update": {
                    "sessionUpdate": "agent_thought_chunk",
                    "content": { "type": "text", "text": "secret" }
                }}
            }),
            &mut output,
        );
        assert_eq!(output, "hello");
    }

    #[test]
    fn agent_environment_scrubs_native_and_legacy_identity_credentials() {
        for name in [
            "X0X_DATA_DIR",
            "X0X_AGENT_ID",
            "X0X_OWNER_AGENT_ID",
            "X0X_GROUP_ID",
            "X0X_API_TOKEN",
            "X0X_API_URL",
            "NOSTR_PRIVATE_KEY",
            "NOSTR_SECRET_KEY",
            "BUZZ_PRIVATE_KEY",
            "BUZZ_RELAY_URL",
            "BUZZ_RELAY_HTTP",
            "BUZZ_AUTH_TAG",
            "BUZZ_API_TOKEN",
            "BUZZ_ACP_PRIVATE_KEY",
            "BUZZ_ACP_API_TOKEN",
            "BUZZ_SHARE_IDENTITY",
            "BUZZ_MANAGED_AGENT",
            "BUZZ_MANAGED_AGENT_START_NONCE",
        ] {
            assert!(is_sensitive_agent_env(std::ffi::OsStr::new(name)));
        }
        for name in ["BUZZ_ACP_AGENT_COMMAND", "ANTHROPIC_API_KEY"] {
            assert!(!is_sensitive_agent_env(std::ffi::OsStr::new(name)));
        }
    }

    #[tokio::test(start_paused = true)]
    async fn absolute_turn_duration_caps_continuously_active_agents() {
        let duration = Duration::from_secs(3);
        let error = enforce_max_turn_duration(Some(duration), async {
            std::future::pending::<()>().await;
            Ok(())
        })
        .await
        .expect_err("absolute turn cap must fire even without an idle period");
        assert!(matches!(error, AcpError::Timeout(actual) if actual == duration));
    }
}

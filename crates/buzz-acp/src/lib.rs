#![forbid(unsafe_code)]

mod acp;
mod config;
mod lifecycle;
mod x0x;

use std::collections::VecDeque;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use acp::{AcpClient, AcpError};
use config::{Config, ConfigError};
use lifecycle::{Lifecycle, LifecyclePublisher};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::mpsc;
use x0x::{ChannelEnvelope, HistoryRow, X0xClient, X0xError};

const RECONCILE_INTERVAL: Duration = Duration::from_secs(2);
const MAX_CONTEXT_MESSAGES: usize = 100;
const PROMPT_CONTEXT_MESSAGES: usize = 20;
const EXPLICIT_REPLY_PREFIX: &str = "X0X_REPLY:";

#[derive(Debug, Error)]
pub enum HarnessError {
    #[error(transparent)]
    Config(#[from] ConfigError),
    #[error(transparent)]
    X0x(#[from] X0xError),
    #[error(transparent)]
    Acp(#[from] AcpError),
    #[error("state I/O failed: {0}")]
    StateIo(#[from] std::io::Error),
    #[error("state JSON failed: {0}")]
    StateJson(#[from] serde_json::Error),
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct DurableState {
    last_seen_id: i64,
}

#[derive(Debug, Clone)]
struct ContextMessage {
    author: String,
    text: String,
    msg_id: String,
}

#[derive(Default)]
struct ConversationContext {
    messages: VecDeque<ContextMessage>,
}

struct HarnessContext<'a> {
    config: &'a Config,
    lifecycle: &'a LifecyclePublisher,
}

impl ConversationContext {
    fn push(&mut self, row: &HistoryRow, envelope: &ChannelEnvelope) {
        self.messages.push_back(ContextMessage {
            author: row
                .author_agent
                .clone()
                .unwrap_or_else(|| "unknown".to_string()),
            text: envelope.text.clone(),
            msg_id: row.msg_id.clone(),
        });
        while self.messages.len() > MAX_CONTEXT_MESSAGES {
            self.messages.pop_front();
        }
    }

    fn render_recent(&self) -> String {
        self.messages
            .iter()
            .rev()
            .take(PROMPT_CONTEXT_MESSAGES)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .map(|message| {
                format!(
                    "- {} [{}]: {}",
                    message.author, message.msg_id, message.text
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

pub fn run() -> Result<(), HarnessError> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(async_main())
}

async fn async_main() -> Result<(), HarnessError> {
    let _ = tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .try_init();

    let config = Config::from_env()?;
    let lifecycle = LifecyclePublisher::new(&config);
    let result = run_harness(&config, &lifecycle).await;
    if result.is_err() {
        let _ = lifecycle.publish(
            Lifecycle::Failed,
            Some("native ACP harness failed; inspect the agent log"),
        );
    }
    result
}

async fn run_harness(config: &Config, lifecycle: &LifecyclePublisher) -> Result<(), HarnessError> {
    let x0x = X0xClient::new(config.data_dir.clone())?;
    x0x.verify_identity(&config.agent_id).await?;
    let stable_group_id = x0x.resolve_group(&config.group_id).await?;
    let state_path = durable_state_path(config);
    let loaded_state = load_state(&state_path)?;
    let recent = x0x.recent_history(&stable_group_id).await?;
    let mut context = ConversationContext::default();
    for row in &recent {
        if let Some(envelope) = row
            .envelope()
            .filter(|_| row.is_safe_context(&stable_group_id, &config.agent_id))
        {
            context.push(row, &envelope);
        }
    }

    let mut state = match loaded_state {
        Some(state) => state,
        None => {
            let state = DurableState {
                last_seen_id: recent.iter().map(|row| row.id).max().unwrap_or_default(),
            };
            persist_state(&state_path, &state)?;
            tracing::info!(
                watermark = state.last_seen_id,
                "first launch: watermarked existing group history without replying"
            );
            state
        }
    };

    lifecycle.publish(Lifecycle::Listening, None)?;
    let mut acp = AcpClient::start(config).await?;
    lifecycle.publish(Lifecycle::Ready, None)?;
    let (wake_tx, mut wake_rx) = mpsc::channel(config.parallelism);
    let wake_client = x0x.clone();
    let wake_group = stable_group_id.clone();
    let wake_task = tokio::spawn(async move {
        wake_client.run_wake_stream(&wake_group, wake_tx).await;
    });
    let mut interval = tokio::time::interval(RECONCILE_INTERVAL);

    loop {
        tokio::select! {
            _ = interval.tick() => {}
            wake = wake_rx.recv() => {
                if wake.is_none() {
                    return Err(HarnessError::X0x(X0xError::Transport(
                        "WebSocket wake task stopped".to_string()
                    )));
                }
            }
            signal = tokio::signal::ctrl_c() => {
                signal?;
                break;
            }
        }
        reconcile(
            &HarnessContext { config, lifecycle },
            &x0x,
            &stable_group_id,
            &state_path,
            &mut state,
            &mut context,
            &mut acp,
        )
        .await?;
    }

    wake_task.abort();
    acp.shutdown().await;
    Ok(())
}

async fn reconcile(
    harness: &HarnessContext<'_>,
    x0x: &X0xClient,
    stable_group_id: &str,
    state_path: &Path,
    state: &mut DurableState,
    context: &mut ConversationContext,
    acp: &mut AcpClient,
) -> Result<(), HarnessError> {
    let config = harness.config;
    let rows = x0x
        .history_after(stable_group_id, state.last_seen_id)
        .await?;
    for row in rows {
        let envelope = row.envelope();
        if let Some(envelope) = envelope
            .as_ref()
            .filter(|_| row.is_safe_context(stable_group_id, &config.agent_id))
        {
            context.push(&row, envelope);
        }

        if let Some(envelope) =
            envelope.filter(|envelope| should_trigger(config, stable_group_id, &row, envelope))
        {
            after_durable_claim(state_path, state, row.id, || async {
                harness.lifecycle.publish(Lifecycle::Waking, None)?;
                handle_directed_message(config, x0x, &row, &envelope, context, acp).await?;
                harness.lifecycle.publish(Lifecycle::Ready, None)?;
                Ok(())
            })
            .await?;
        } else {
            claim_row(state_path, state, row.id)?;
        }
    }
    Ok(())
}

fn claim_row(state_path: &Path, state: &mut DurableState, row_id: i64) -> Result<(), HarnessError> {
    state.last_seen_id = state.last_seen_id.max(row_id);
    persist_state(state_path, state)
}

async fn after_durable_claim<F, Fut>(
    state_path: &Path,
    state: &mut DurableState,
    row_id: i64,
    operation: F,
) -> Result<(), HarnessError>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Result<(), HarnessError>>,
{
    // Claim before entering ACP. ACP tools can perform externally-visible
    // sends that cannot be rolled back, so retrying this prompt after a reply
    // or watermark failure would duplicate a successful side effect.
    claim_row(state_path, state, row_id)?;
    operation().await
}

fn should_trigger(
    config: &Config,
    stable_group_id: &str,
    row: &HistoryRow,
    envelope: &ChannelEnvelope,
) -> bool {
    let Some(author) = row.author_agent.as_deref() else {
        return false;
    };
    row.is_verified_inbound(stable_group_id)
        && !author.eq_ignore_ascii_case(&config.agent_id)
        && config.author_allowed(author)
        && !envelope.agent_generated
        && envelope
            .mentions
            .iter()
            .any(|mention| mention.eq_ignore_ascii_case(&config.agent_id))
}

async fn handle_directed_message(
    config: &Config,
    x0x: &X0xClient,
    row: &HistoryRow,
    envelope: &ChannelEnvelope,
    context: &ConversationContext,
    acp: &mut AcpClient,
) -> Result<(), HarnessError> {
    let author = row.author_agent.as_deref().unwrap_or_default();
    let is_owner = author.eq_ignore_ascii_case(&config.owner_agent_id);
    let prompt = build_prompt(config, row, envelope, context, is_owner);
    let final_text = acp.prompt(&prompt).await?;
    let Some(reply) = reply_for_author(&final_text, is_owner) else {
        tracing::info!(
            author,
            msg_id = row.msg_id,
            "agent chose not to post a group reply"
        );
        return Ok(());
    };

    let response_envelope = ChannelEnvelope {
        text: reply,
        created_at: now_millis(),
        client_id: uuid::Uuid::new_v4().to_string(),
        mentions: vec![author.to_string()],
        agent_generated: true,
    };
    let body = serde_json::to_string(&response_envelope)?;
    let thread_root = row.thread_root.as_deref().unwrap_or(&row.msg_id);
    x0x.send_group_reply(&config.group_id, &body, thread_root, &row.msg_id)
        .await?;
    Ok(())
}

fn build_prompt(
    config: &Config,
    row: &HistoryRow,
    envelope: &ChannelEnvelope,
    context: &ConversationContext,
    is_owner: bool,
) -> String {
    let peer_instruction = if is_owner {
        "Your final text will be posted as a threaded reply."
    } else {
        "To post a reply, begin your final text with exactly `X0X_REPLY:`. Without that prefix, no message will be sent."
    };
    format!(
        "[Native x0x directed space message]\n\
         Space: {}\n\
         Sender AgentId: {}\n\
         Message id: {}\n\
         Thread root: {}\n\
         Thread parent: {}\n\
         Message: {}\n\n\
         Recent durable conversation (oldest to newest):\n{}\n\n\
         Native x0x MCP tools: `space_members` lists this space's roster and \
         `space_send` sends a space message. Return final text for the reply to \
         this sender. Use `space_send` only when the incoming request explicitly \
         asks you to delegate to or notify another member, and never duplicate \
         the sender reply through that tool; the harness preserves its verified \
         thread root and parent.\n\n\
         {peer_instruction}",
        config.group_id,
        row.author_agent.as_deref().unwrap_or("unknown"),
        row.msg_id,
        row.thread_root.as_deref().unwrap_or(&row.msg_id),
        row.thread_parent.as_deref().unwrap_or("none"),
        envelope.text,
        context.render_recent(),
    )
}

fn reply_for_author(final_text: &str, is_owner: bool) -> Option<String> {
    let trimmed = final_text.trim();
    if let Some(explicit) = trimmed.strip_prefix(EXPLICIT_REPLY_PREFIX) {
        let explicit = explicit.trim();
        return (!explicit.is_empty()).then(|| explicit.to_string());
    }
    (is_owner && !trimmed.is_empty()).then(|| trimmed.to_string())
}

fn durable_state_path(config: &Config) -> PathBuf {
    config
        .data_dir
        .join(format!("buzz-acp-{}.json", config.group_id))
}

fn load_state(path: &Path) -> Result<Option<DurableState>, HarnessError> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(HarnessError::StateIo(error)),
    }
}

fn persist_state(path: &Path, state: &DurableState) -> Result<(), HarnessError> {
    let temporary = path.with_extension("json.tmp");
    let bytes = serde_json::to_vec(state)?;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&temporary)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    std::fs::rename(&temporary, path)?;
    #[cfg(unix)]
    if let Some(parent) = path.parent() {
        std::fs::File::open(parent)?.sync_all()?;
    }
    Ok(())
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use base64::Engine as _;

    use super::*;
    use crate::config::RespondTo;

    #[test]
    fn directed_trigger_requires_verified_exact_mention_and_owner() {
        let config = test_config(RespondTo::OwnerOnly);
        let mut row = test_row(&config.owner_agent_id, &config.agent_id);
        let envelope = row.envelope().expect("valid envelope");
        assert!(should_trigger(&config, "stable", &row, &envelope));

        row.provenance = "LocalSend".to_string();
        assert!(!should_trigger(&config, "stable", &row, &envelope));
    }

    #[test]
    fn directed_trigger_rejects_allowed_author_without_mention() {
        let config = test_config(RespondTo::OwnerOnly);
        let row = test_row(&config.owner_agent_id, &"c".repeat(64));
        let envelope = row.envelope().expect("valid envelope");
        assert!(!should_trigger(&config, "stable", &row, &envelope));
    }

    #[test]
    fn generated_peer_reply_cannot_trigger_an_allowlisted_agent() {
        let allowed = "c".repeat(64);
        let mut config = test_config(RespondTo::Allowlist);
        config.respond_to_allowlist.insert(allowed.clone());
        let mut row = test_row(&allowed, &config.agent_id);
        let mut envelope = row.envelope().expect("valid envelope");
        envelope.agent_generated = true;
        row.payload = base64::engine::general_purpose::STANDARD
            .encode(serde_json::to_vec(&envelope).expect("encode envelope"));
        let envelope = row.envelope().expect("valid generated envelope");

        assert!(!should_trigger(&config, "stable", &row, &envelope));
    }

    #[tokio::test]
    async fn directed_row_is_durably_claimed_before_any_acp_side_effect() {
        let directory = tempfile::tempdir().expect("tempdir");
        let state_path = directory.path().join("state.json");
        let mut state = DurableState::default();

        let error = after_durable_claim(&state_path, &mut state, 42, || async {
            let claimed = load_state(&state_path)
                .expect("load claimed state")
                .expect("claimed state exists");
            assert_eq!(claimed.last_seen_id, 42);
            Err(HarnessError::X0x(X0xError::Transport(
                "simulated reply failure".to_string(),
            )))
        })
        .await
        .expect_err("reply failure must propagate");

        assert!(error.to_string().contains("simulated reply failure"));
        assert_eq!(
            load_state(&state_path)
                .expect("reload state")
                .expect("state remains durable")
                .last_seen_id,
            42
        );
    }

    #[test]
    fn peer_reply_requires_explicit_prefix() {
        assert_eq!(reply_for_author("hello", false), None);
        assert_eq!(
            reply_for_author("X0X_REPLY: hello", false).as_deref(),
            Some("hello")
        );
        assert_eq!(reply_for_author("hello", true).as_deref(), Some("hello"));
    }

    #[test]
    fn only_verified_remote_or_signed_local_rows_enter_context() {
        let config = test_config(RespondTo::OwnerOnly);
        let mut row = test_row(&config.owner_agent_id, &config.agent_id);
        assert!(row.is_safe_context("stable", &config.agent_id));

        row.provenance = "Unverified".to_string();
        assert!(!row.is_safe_context("stable", &config.agent_id));

        row.direction = "Outbound".to_string();
        row.provenance = "LocalSend".to_string();
        row.author_agent = Some(config.agent_id.clone());
        assert!(row.is_safe_context("stable", &config.agent_id));

        row.author_agent = Some(config.owner_agent_id.clone());
        assert!(!row.is_safe_context("stable", &config.agent_id));
    }

    #[test]
    fn directed_prompt_names_native_space_tools() {
        let config = test_config(RespondTo::OwnerOnly);
        let row = test_row(&config.owner_agent_id, &config.agent_id);
        let envelope = row.envelope().expect("valid envelope");
        let prompt = build_prompt(
            &config,
            &row,
            &envelope,
            &ConversationContext::default(),
            true,
        );
        assert!(prompt.contains("`space_members`"));
        assert!(prompt.contains("`space_send`"));
        assert!(prompt.contains("explicitly asks you to delegate to or notify another member"));
        assert!(!prompt.contains("community_send"));
    }

    fn test_config(respond_to: RespondTo) -> Config {
        Config {
            data_dir: PathBuf::from("/tmp/x0x"),
            agent_id: "a".repeat(64),
            owner_agent_id: "b".repeat(64),
            group_id: "group".to_string(),
            agent_command: "buzz-agent".to_string(),
            agent_args: Vec::new(),
            system_prompt: None,
            respond_to,
            respond_to_allowlist: HashSet::new(),
            idle_timeout: Duration::from_secs(1),
            max_turn_duration: None,
            parallelism: 1,
            start_nonce: None,
        }
    }

    fn test_row(author: &str, mention: &str) -> HistoryRow {
        let envelope = serde_json::json!({
            "text": "please respond",
            "createdAt": 1,
            "clientId": "client",
            "mentions": [mention],
        });
        HistoryRow {
            id: 1,
            msg_id: "1".repeat(64),
            scope: "group:stable".to_string(),
            author_agent: Some(author.to_string()),
            direction: "Inbound".to_string(),
            _content_type: "text/plain".to_string(),
            payload: base64::engine::general_purpose::STANDARD.encode(envelope.to_string()),
            provenance: "VerifiedEnvelope".to_string(),
            _sent_at_ms: 1,
            _seen_at_ms: 1,
            signed: true,
            thread_root: None,
            thread_parent: None,
        }
    }
}

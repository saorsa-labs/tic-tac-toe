#![forbid(unsafe_code)]

mod acp;
mod config;
mod lifecycle;
mod x0x;

use std::collections::VecDeque;
use std::future::Future;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use acp::{AcpClient, AcpError, AcpTurnOutcome};
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
const MAX_SILENT_END_TURN_RETRIES: usize = 1;

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
    #[error(
        "directed ACP turn ended without visible output or a completed tool after {attempts} attempt(s); stop reason: {stop_reason}"
    )]
    SilentDirectedTurn {
        attempts: usize,
        stop_reason: String,
    },
}

type PromptFuture<'a> = Pin<Box<dyn Future<Output = Result<AcpTurnOutcome, AcpError>> + 'a>>;

trait PromptAgent {
    fn prompt<'a>(&'a mut self, prompt: &'a str) -> PromptFuture<'a>;
}

impl PromptAgent for AcpClient {
    fn prompt<'a>(&'a mut self, prompt: &'a str) -> PromptFuture<'a> {
        Box::pin(AcpClient::prompt(self, prompt))
    }
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
    let previous_watermark = state.last_seen_id;
    claim_row(state_path, state, row_id)?;
    let result = operation().await;
    if matches!(&result, Err(HarnessError::SilentDirectedTurn { .. })) {
        // SilentDirectedTurn is constructed only after observing neither
        // visible assistant text nor a clean completed tool. It is therefore
        // the sole error for which releasing the claim cannot duplicate a
        // known side effect. Persist the rollback before surfacing the error
        // so a supervisor restart can reconcile this exact mention again.
        state.last_seen_id = previous_watermark;
        persist_state(state_path, state)?;
    }
    result
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
    acp: &mut impl PromptAgent,
) -> Result<(), HarnessError> {
    let author = row.author_agent.as_deref().unwrap_or_default();
    let is_owner = author.eq_ignore_ascii_case(&config.owner_agent_id);
    let prompt = build_prompt(config, row, envelope, context, is_owner);
    let outcome = prompt_directed_message(acp, &prompt).await?;
    let sent = post_turn_reply(&outcome, is_owner, |reply| async move {
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
    })
    .await?;
    if !sent {
        if outcome.has_completed_tool() {
            tracing::info!(
                author,
                msg_id = row.msg_id,
                completed_tools = outcome.completed_tool_call_ids.len(),
                "agent completed a tool without posting a group reply"
            );
        } else {
            tracing::info!(
                author,
                msg_id = row.msg_id,
                "agent chose not to post a group reply"
            );
        }
    }
    Ok(())
}

async fn prompt_directed_message(
    acp: &mut impl PromptAgent,
    prompt: &str,
) -> Result<AcpTurnOutcome, HarnessError> {
    let mut attempts = 0_usize;
    loop {
        attempts = attempts.saturating_add(1);
        let outcome = acp.prompt(prompt).await?;
        if outcome.has_visible_text() || outcome.has_completed_tool() {
            return Ok(outcome);
        }
        let may_retry =
            outcome.stop_reason == "end_turn" && attempts <= MAX_SILENT_END_TURN_RETRIES;
        if !may_retry {
            return Err(HarnessError::SilentDirectedTurn {
                attempts,
                stop_reason: outcome.stop_reason,
            });
        }
        tracing::warn!(
            attempt = attempts,
            max_retries = MAX_SILENT_END_TURN_RETRIES,
            "directed ACP turn ended silently; retrying once"
        );
    }
}

async fn post_turn_reply<F, Fut>(
    outcome: &AcpTurnOutcome,
    is_owner: bool,
    send: F,
) -> Result<bool, HarnessError>
where
    F: FnOnce(String) -> Fut,
    Fut: Future<Output = Result<(), HarnessError>>,
{
    let Some(reply) = reply_for_author(&outcome.assistant_text, is_owner) else {
        return Ok(false);
    };
    send(reply).await?;
    Ok(true)
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
    use std::sync::atomic::{AtomicUsize, Ordering};

    use base64::Engine as _;

    use super::*;
    use crate::config::RespondTo;

    struct ScriptedPrompter {
        outcomes: VecDeque<AcpTurnOutcome>,
        calls: usize,
    }

    impl ScriptedPrompter {
        fn new(outcomes: impl IntoIterator<Item = AcpTurnOutcome>) -> Self {
            Self {
                outcomes: outcomes.into_iter().collect(),
                calls: 0,
            }
        }
    }

    impl PromptAgent for ScriptedPrompter {
        fn prompt<'a>(&'a mut self, _prompt: &'a str) -> PromptFuture<'a> {
            self.calls = self.calls.saturating_add(1);
            let outcome = self.outcomes.pop_front().ok_or_else(|| {
                AcpError::Protocol("scripted ACP outcome was not provided".to_string())
            });
            Box::pin(std::future::ready(outcome))
        }
    }

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
    async fn ambiguous_send_failure_keeps_the_durable_claim() {
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

    #[tokio::test]
    async fn completed_tool_success_keeps_the_durable_claim() {
        let directory = tempfile::tempdir().expect("tempdir");
        let state_path = directory.path().join("state.json");
        let mut state = DurableState::default();
        let mut acp =
            ScriptedPrompter::new([completed_tool_wire_turn("space-send-1", Some(false))]);

        after_durable_claim(&state_path, &mut state, 42, || async {
            let outcome = prompt_directed_message(&mut acp, "directed prompt").await?;
            let posted = post_turn_reply(&outcome, true, |_reply| {
                std::future::ready(Err(HarnessError::X0x(X0xError::Transport(
                    "harness reply must not run after a tool-only turn".to_string(),
                ))))
            })
            .await?;
            assert!(!posted);
            Ok(())
        })
        .await
        .expect("completed tool turn succeeds");

        assert_eq!(state.last_seen_id, 42);
        assert_eq!(
            load_state(&state_path)
                .expect("reload state")
                .expect("claim remains durable")
                .last_seen_id,
            42
        );
    }

    #[tokio::test]
    async fn blank_or_failed_tool_end_turn_gets_one_bounded_retry() {
        for first_turn in [
            silent_turn(),
            completed_tool_wire_turn("failed-tool", Some(true)),
        ] {
            let mut acp = ScriptedPrompter::new([first_turn, visible_turn("recovered")]);
            let outcome = prompt_directed_message(&mut acp, "directed prompt")
                .await
                .expect("one retry should recover");

            assert_eq!(outcome.assistant_text, "recovered");
            assert_eq!(acp.calls, 2);
        }
    }

    #[tokio::test]
    async fn completed_tool_does_not_retry_or_duplicate_a_harness_reply() {
        for is_error in [Some(false), None] {
            let mut acp = ScriptedPrompter::new([
                completed_tool_wire_turn("space-send-1", is_error),
                visible_turn("must remain unused"),
            ]);
            let outcome = prompt_directed_message(&mut acp, "directed prompt")
                .await
                .expect("completed tool is a successful turn");
            let sends = AtomicUsize::new(0);
            let posted = post_turn_reply(&outcome, true, |_reply| {
                sends.fetch_add(1, Ordering::SeqCst);
                std::future::ready(Ok(()))
            })
            .await
            .expect("no harness reply needed");

            assert_eq!(acp.calls, 1);
            assert!(!posted);
            assert_eq!(sends.load(Ordering::SeqCst), 0);
        }
    }

    #[tokio::test]
    async fn silent_then_answer_posts_exactly_once() {
        let mut acp = ScriptedPrompter::new([silent_turn(), visible_turn("one answer")]);
        let outcome = prompt_directed_message(&mut acp, "directed prompt")
            .await
            .expect("retry should recover");
        let sends = AtomicUsize::new(0);
        let posted = post_turn_reply(&outcome, true, |reply| {
            assert_eq!(reply, "one answer");
            sends.fetch_add(1, Ordering::SeqCst);
            std::future::ready(Ok(()))
        })
        .await
        .expect("reply should post");

        assert!(posted);
        assert_eq!(acp.calls, 2);
        assert_eq!(sends.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn silent_exhaustion_releases_claim_and_restart_retry_posts_once() {
        let directory = tempfile::tempdir().expect("tempdir");
        let state_path = directory.path().join("state.json");
        let mut state = DurableState { last_seen_id: 7 };
        persist_state(&state_path, &state).expect("persist prior watermark");
        let mut acp = ScriptedPrompter::new([
            silent_turn(),
            completed_tool_wire_turn("failed-tool", Some(true)),
        ]);

        let error = after_durable_claim(&state_path, &mut state, 42, || async {
            prompt_directed_message(&mut acp, "directed prompt")
                .await
                .map(|_| ())
        })
        .await
        .expect_err("exhaustion must not become success");

        assert!(matches!(
            error,
            HarnessError::SilentDirectedTurn {
                attempts: 2,
                ref stop_reason
            } if stop_reason == "end_turn"
        ));
        assert_eq!(acp.calls, 2);
        assert_eq!(
            load_state(&state_path)
                .expect("reload state")
                .expect("prior state is restored")
                .last_seen_id,
            7
        );
        assert_eq!(state.last_seen_id, 7);

        // Simulate the supervisor restart/reconcile decision and successful
        // processing of the same history row.
        assert!(42 > state.last_seen_id);
        let mut restarted_acp = ScriptedPrompter::new([visible_turn("recovered once")]);
        let sends = AtomicUsize::new(0);
        after_durable_claim(&state_path, &mut state, 42, || async {
            let outcome = prompt_directed_message(&mut restarted_acp, "directed prompt").await?;
            let posted = post_turn_reply(&outcome, true, |reply| {
                assert_eq!(reply, "recovered once");
                sends.fetch_add(1, Ordering::SeqCst);
                std::future::ready(Ok(()))
            })
            .await?;
            assert!(posted);
            Ok(())
        })
        .await
        .expect("restart retry succeeds");

        assert_eq!(restarted_acp.calls, 1);
        assert_eq!(sends.load(Ordering::SeqCst), 1);
        assert_eq!(state.last_seen_id, 42);
    }

    #[tokio::test]
    async fn silent_claim_rollback_persistence_failure_is_surfaced() {
        let directory = tempfile::tempdir().expect("tempdir");
        let state_path = directory.path().join("state.json");
        let mut state = DurableState { last_seen_id: 7 };
        persist_state(&state_path, &state).expect("persist prior watermark");

        let error = after_durable_claim(&state_path, &mut state, 42, || async {
            std::fs::remove_file(&state_path).expect("remove claimed state");
            std::fs::create_dir(&state_path).expect("block atomic rollback rename");
            Err(HarnessError::SilentDirectedTurn {
                attempts: 2,
                stop_reason: "end_turn".to_string(),
            })
        })
        .await
        .expect_err("rollback persistence failure must be returned");

        assert!(matches!(error, HarnessError::StateIo(_)));
        assert_eq!(state.last_seen_id, 7);
    }

    #[tokio::test]
    async fn silent_non_end_turn_fails_without_retry() {
        let mut acp = ScriptedPrompter::new([
            AcpTurnOutcome {
                stop_reason: "max_tokens".to_string(),
                assistant_text: String::new(),
                completed_tool_call_ids: Vec::new(),
            },
            visible_turn("must remain unused"),
        ]);

        let error = prompt_directed_message(&mut acp, "directed prompt")
            .await
            .expect_err("only end_turn is retryable");

        assert!(matches!(
            error,
            HarnessError::SilentDirectedTurn {
                attempts: 1,
                ref stop_reason
            } if stop_reason == "max_tokens"
        ));
        assert_eq!(acp.calls, 1);
    }

    #[test]
    fn passive_row_remains_claim_only_without_an_acp_turn() {
        let directory = tempfile::tempdir().expect("tempdir");
        let state_path = directory.path().join("state.json");
        let config = test_config(RespondTo::OwnerOnly);
        let row = test_row(&config.owner_agent_id, &"c".repeat(64));
        let envelope = row.envelope().expect("valid envelope");
        let acp = ScriptedPrompter::new([visible_turn("must remain unused")]);
        let mut state = DurableState::default();

        assert!(!should_trigger(&config, "stable", &row, &envelope));
        claim_row(&state_path, &mut state, row.id).expect("claim passive row");

        assert_eq!(acp.calls, 0);
        assert_eq!(state.last_seen_id, row.id);
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

    fn silent_turn() -> AcpTurnOutcome {
        AcpTurnOutcome {
            stop_reason: "end_turn".to_string(),
            assistant_text: String::new(),
            completed_tool_call_ids: Vec::new(),
        }
    }

    fn visible_turn(text: &str) -> AcpTurnOutcome {
        AcpTurnOutcome {
            stop_reason: "end_turn".to_string(),
            assistant_text: text.to_string(),
            completed_tool_call_ids: Vec::new(),
        }
    }

    fn completed_tool_wire_turn(tool_call_id: &str, is_error: Option<bool>) -> AcpTurnOutcome {
        let mut update = serde_json::json!({
            "method": "session/update",
            "params": { "update": {
                "sessionUpdate": "tool_call_update",
                "toolCallId": tool_call_id,
                "status": "completed"
            }}
        });
        if let Some(is_error) = is_error {
            update["params"]["update"]["rawOutput"] = serde_json::json!({ "isError": is_error });
        }
        crate::acp::test_turn_outcome([update], "end_turn")
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

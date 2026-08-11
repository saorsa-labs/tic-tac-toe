#![forbid(unsafe_code)]

mod acp;
mod config;
mod lifecycle;
mod storage;
mod x0x;

use std::collections::VecDeque;
use std::future::Future;
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
const MAX_AGENT_WAKE_GENERATION: u8 = 1;
const MAX_SILENT_END_TURN_RETRIES: usize = 1;
const SILENT_RECONCILE_BACKOFFS: [Duration; 3] = [
    Duration::from_secs(5),
    Duration::from_secs(30),
    Duration::from_secs(120),
];

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
        "tool-free directed ACP turn ended silently after {attempts} attempt(s); stop reason: {stop_reason}"
    )]
    SilentDirectedTurn {
        attempts: usize,
        stop_reason: String,
    },
    #[error(
        "directed ACP turn ended with ambiguous tool activity and cannot be retried safely; stop reason: {stop_reason}"
    )]
    AmbiguousDirectedToolTurn { stop_reason: String },
    #[error("directed ACP turn ended without output before end_turn; stop reason: {stop_reason}")]
    IncompleteDirectedTurn { stop_reason: String },
    #[error("pending causal handoff is invalid: {0}")]
    PendingCausalInvalid(String),
    #[error("pending causal message {0} is absent from child durable history")]
    PendingCausalMissing(String),
    #[error("pending causal message {0} failed exact signed trigger validation")]
    PendingCausalRejected(String),
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

type ReconcileFuture<'a> = Pin<Box<dyn Future<Output = Result<(), HarnessError>> + 'a>>;

trait ReconcileAttempt {
    fn run<'a>(&'a mut self) -> ReconcileFuture<'a>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReconcileRecoveryOutcome {
    Reconciled,
    ShutdownRequested,
}

struct LiveReconcileAttempt<'a> {
    harness: &'a HarnessContext<'a>,
    x0x: &'a X0xClient,
    stable_group_id: &'a str,
    state_path: &'a Path,
    state: &'a mut DurableState,
    context: &'a mut ConversationContext,
    acp: &'a mut AcpClient,
}

impl ReconcileAttempt for LiveReconcileAttempt<'_> {
    fn run<'a>(&'a mut self) -> ReconcileFuture<'a> {
        Box::pin(reconcile(
            self.harness,
            self.x0x,
            self.stable_group_id,
            self.state_path,
            self.state,
            self.context,
            self.acp,
        ))
    }
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct DurableState {
    last_seen_id: i64,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct PendingCausalMessage {
    version: u8,
    start_nonce: String,
    group_id: String,
    msg_id: String,
    state: PendingCausalState,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum PendingCausalState {
    Pending,
    /// The exact row and watermark are durably reserved, but ACP side effects
    /// have not started. Safe to recover after a process crash.
    Claimed,
    /// ACP may have produced an externally visible side effect. Never retry
    /// this state without a completed tombstone.
    Executing,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct CompletedCausalMessage {
    version: u8,
    group_id: String,
    msg_id: String,
}

#[derive(Debug, Clone)]
struct PendingCausalEntry {
    path: PathBuf,
    message: PendingCausalMessage,
}

#[derive(Debug, Clone)]
struct ContextMessage {
    author: String,
    text: String,
    msg_id: String,
    mentions: Vec<String>,
    agent_generated: bool,
    thread_root: Option<String>,
    thread_parent: Option<String>,
}

#[derive(Default)]
struct ConversationContext {
    messages: VecDeque<ContextMessage>,
}

struct HarnessContext<'a> {
    config: &'a Config,
    lifecycle: &'a LifecyclePublisher,
    causal_nonce: &'a str,
}

impl ConversationContext {
    fn push(&mut self, row: &HistoryRow, envelope: &ChannelEnvelope) {
        if self
            .messages
            .iter()
            .any(|message| message.msg_id == row.msg_id)
        {
            return;
        }
        self.messages.push_back(ContextMessage {
            author: row
                .author_agent
                .clone()
                .unwrap_or_else(|| "unknown".to_string()),
            text: envelope.text.clone(),
            msg_id: row.msg_id.clone(),
            mentions: envelope.mentions.clone(),
            agent_generated: envelope.agent_generated,
            thread_root: row.thread_root.clone(),
            thread_parent: row.thread_parent.clone(),
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
    let causal_nonce = config
        .start_nonce
        .clone()
        .unwrap_or_else(|| uuid::Uuid::new_v4().simple().to_string());
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
        let harness = HarnessContext {
            config,
            lifecycle,
            causal_nonce: &causal_nonce,
        };
        let mut attempt = LiveReconcileAttempt {
            harness: &harness,
            x0x: &x0x,
            stable_group_id: &stable_group_id,
            state_path: &state_path,
            state: &mut state,
            context: &mut context,
            acp: &mut acp,
        };
        if reconcile_with_silent_recovery(&mut attempt).await?
            == ReconcileRecoveryOutcome::ShutdownRequested
        {
            break;
        }
    }

    wake_task.abort();
    acp.shutdown().await;
    Ok(())
}

async fn reconcile_with_silent_recovery(
    attempt: &mut impl ReconcileAttempt,
) -> Result<ReconcileRecoveryOutcome, HarnessError> {
    let mut recovery_index = 0_usize;
    loop {
        match attempt.run().await {
            Ok(()) => return Ok(ReconcileRecoveryOutcome::Reconciled),
            Err(error @ HarnessError::SilentDirectedTurn { .. })
                if recovery_index < SILENT_RECONCILE_BACKOFFS.len() =>
            {
                let delay = SILENT_RECONCILE_BACKOFFS[recovery_index];
                recovery_index = recovery_index.saturating_add(1);
                tracing::warn!(
                    recovery_attempt = recovery_index,
                    max_recovery_attempts = SILENT_RECONCILE_BACKOFFS.len(),
                    ?delay,
                    reason = %error,
                    "tool-free directed turn remained silent; retrying reconciliation after backoff"
                );
                tokio::select! {
                    _ = tokio::time::sleep(delay) => {}
                    signal = tokio::signal::ctrl_c() => {
                        signal?;
                        return Ok(ReconcileRecoveryOutcome::ShutdownRequested);
                    }
                }
            }
            Err(error) => return Err(error),
        }
    }
}

fn pending_causal_dir(config: &Config) -> PathBuf {
    crate::storage::scoped_path(&config.data_dir, &config.group_id, "pending")
}

fn completed_causal_dir(config: &Config) -> PathBuf {
    crate::storage::scoped_path(&config.data_dir, &config.group_id, "completed")
}

fn completed_causal_path(config: &Config, msg_id: &str) -> PathBuf {
    completed_causal_dir(config).join(format!("{msg_id}.json"))
}

fn completed_causal_exists(config: &Config, msg_id: &str) -> Result<bool, HarnessError> {
    let path = completed_causal_path(config, msg_id);
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.into()),
    };
    let completed: CompletedCausalMessage = serde_json::from_slice(&bytes)?;
    if completed.version != 1 || completed.group_id != config.group_id || completed.msg_id != msg_id
    {
        return Err(HarnessError::PendingCausalInvalid(
            path.display().to_string(),
        ));
    }
    Ok(true)
}

fn persist_completed_causal(config: &Config, msg_id: &str) -> Result<(), HarnessError> {
    let dir = completed_causal_dir(config);
    crate::storage::ensure_dir(&dir)?;
    let path = completed_causal_path(config, msg_id);
    if completed_causal_exists(config, msg_id)? {
        return Ok(());
    }
    let bytes = serde_json::to_vec(&CompletedCausalMessage {
        version: 1,
        group_id: config.group_id.clone(),
        msg_id: msg_id.to_string(),
    })?;
    crate::storage::write_atomic(&path, &bytes)?;
    Ok(())
}

fn load_pending_causal(
    config: &Config,
    start_nonce: &str,
) -> Result<Vec<PendingCausalEntry>, HarnessError> {
    let dir = pending_causal_dir(config);
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };
    let mut pending = Vec::new();
    for entry in entries {
        let path = entry?.path();
        if path.extension().and_then(|value| value.to_str()) != Some("json") {
            continue;
        }
        let message: PendingCausalMessage = serde_json::from_slice(&std::fs::read(&path)?)?;
        let file_msg_id = path.file_stem().and_then(|value| value.to_str());
        let canonical_msg_id = message.msg_id.to_ascii_lowercase();
        let supported_version = matches!(message.version, 1 | 2);
        let identity_valid = supported_version
            && message.group_id == config.group_id
            && canonical_msg_id.len() == 64
            && canonical_msg_id
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
            && file_msg_id == Some(canonical_msg_id.as_str());
        if identity_valid && completed_causal_exists(config, &canonical_msg_id)? {
            crate::storage::remove_durable(&path)?;
            continue;
        }
        let state_valid = matches!(
            (message.version, message.state),
            (1, PendingCausalState::Pending | PendingCausalState::Claimed)
                | (
                    2,
                    PendingCausalState::Pending
                        | PendingCausalState::Claimed
                        | PendingCausalState::Executing,
                )
        );
        let valid = identity_valid && message.start_nonce == start_nonce && state_valid;
        if !valid {
            // A different nonce belongs to an earlier harness generation and
            // must never be adopted by this process. Other malformed fields
            // on our nonce are fatal rather than silently ignored.
            if message.start_nonce != start_nonce {
                continue;
            }
            return Err(HarnessError::PendingCausalInvalid(
                path.display().to_string(),
            ));
        }
        pending.push(PendingCausalEntry {
            path,
            message: PendingCausalMessage {
                version: if message.version == 1 && message.state == PendingCausalState::Pending {
                    2
                } else {
                    message.version
                },
                msg_id: canonical_msg_id,
                ..message
            },
        });
    }
    pending.sort_by(|left, right| left.message.msg_id.cmp(&right.message.msg_id));
    Ok(pending)
}

/// Put every directed row, including ordinary live/history delivery, through
/// the same exact durable handoff used by a cold-wake redelivery. Creation is
/// no-replace so a concurrent desktop wake cannot reset a claimed/executing
/// row back to pending.
fn stage_causal_message(
    config: &Config,
    start_nonce: &str,
    msg_id: &str,
) -> Result<Option<PendingCausalEntry>, HarnessError> {
    let canonical_msg_id = msg_id.to_ascii_lowercase();
    if canonical_msg_id.len() != 64
        || !canonical_msg_id
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
        || start_nonce.len() != 32
        || !start_nonce.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(HarnessError::PendingCausalInvalid(msg_id.to_string()));
    }
    if completed_causal_exists(config, &canonical_msg_id)? {
        return Ok(None);
    }

    let dir = pending_causal_dir(config);
    crate::storage::ensure_dir(&dir)?;
    let path = dir.join(format!("{canonical_msg_id}.json"));
    let message = PendingCausalMessage {
        version: 2,
        start_nonce: start_nonce.to_ascii_lowercase(),
        group_id: config.group_id.clone(),
        msg_id: canonical_msg_id.clone(),
        state: PendingCausalState::Pending,
    };
    let bytes = serde_json::to_vec(&message)?;
    match crate::storage::write_atomic_new(&path, &bytes) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error.into()),
    }

    let staged = load_pending_causal(config, start_nonce)?
        .into_iter()
        .find(|entry| entry.message.msg_id == canonical_msg_id);
    match staged {
        Some(entry)
            if entry.message.state == PendingCausalState::Pending
                || (entry.message.version == 2
                    && entry.message.state == PendingCausalState::Claimed) =>
        {
            Ok(Some(entry))
        }
        Some(_) => Ok(None),
        None if completed_causal_exists(config, &canonical_msg_id)? => Ok(None),
        None => Err(HarnessError::PendingCausalInvalid(
            path.display().to_string(),
        )),
    }
}

fn persist_pending_causal(entry: &PendingCausalEntry) -> Result<(), HarnessError> {
    let bytes = serde_json::to_vec(&entry.message)?;
    crate::storage::write_atomic(&entry.path, &bytes)?;
    Ok(())
}

fn remove_pending_causal(path: &Path) -> Result<(), HarnessError> {
    crate::storage::remove_durable(path)?;
    Ok(())
}

async fn after_durable_pending_claim<F, Fut>(
    config: &Config,
    state_path: &Path,
    state: &mut DurableState,
    row_id: i64,
    entry: &mut PendingCausalEntry,
    operation: F,
) -> Result<(), HarnessError>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<(), HarnessError>>,
{
    let previous_watermark = state.last_seen_id;
    entry.message.state = PendingCausalState::Claimed;
    persist_pending_causal(entry)?;
    claim_row(state_path, state, row_id)?;
    entry.message.state = PendingCausalState::Executing;
    persist_pending_causal(entry)?;

    let result = operation().await;
    match &result {
        Ok(()) => {
            persist_completed_causal(config, &entry.message.msg_id)?;
            remove_pending_causal(&entry.path)?;
        }
        Err(HarnessError::SilentDirectedTurn { .. }) => {
            // No visible text and no completed/ambiguous tool means no known
            // side effect. Release both claims so the same exact row can be
            // retried by this process or rebound on a later clean launch.
            // Persist the retryable handoff BEFORE lowering the watermark. A
            // crash between the two leaves a pending exact-row override above
            // a high watermark, which the next harness safely processes.
            entry.message.state = PendingCausalState::Pending;
            persist_pending_causal(entry)?;
            state.last_seen_id = previous_watermark;
            persist_state(state_path, state)?;
        }
        Err(_) => {
            // Ambiguous ACP/tool/send failures retain both durable claims.
        }
    }
    result
}

async fn after_durable_ordinary_claim<F, Fut>(
    config: &Config,
    start_nonce: &str,
    state_path: &Path,
    state: &mut DurableState,
    row_id: i64,
    msg_id: &str,
    operation: F,
) -> Result<(), HarnessError>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<(), HarnessError>>,
{
    let Some(mut entry) = stage_causal_message(config, start_nonce, msg_id)? else {
        // A completed or ambiguous exact claim is already authoritative. Move
        // the ordinary history cursor past it without entering ACP again.
        return claim_row(state_path, state, row_id);
    };
    after_durable_pending_claim(config, state_path, state, row_id, &mut entry, operation).await
}

async fn reconcile_pending_causal(
    harness: &HarnessContext<'_>,
    x0x: &X0xClient,
    stable_group_id: &str,
    state_path: &Path,
    state: &mut DurableState,
    context: &mut ConversationContext,
    acp: &mut AcpClient,
) -> Result<(), HarnessError> {
    let mut pending_rows = Vec::new();
    for entry in load_pending_causal(harness.config, harness.causal_nonce)? {
        if entry.message.state == PendingCausalState::Executing
            || (entry.message.version == 1 && entry.message.state == PendingCausalState::Claimed)
        {
            continue;
        }
        let msg_id = entry.message.msg_id.clone();
        let row = x0x
            .history_get(&msg_id)
            .await?
            .ok_or_else(|| HarnessError::PendingCausalMissing(msg_id.clone()))?;
        let envelope = row
            .envelope()
            .ok_or_else(|| HarnessError::PendingCausalRejected(msg_id.clone()))?;
        pending_rows.push((entry, row, envelope));
    }
    pending_rows.sort_by_key(|(_, row, _)| row.id);

    for (mut entry, row, envelope) in pending_rows {
        let msg_id = entry.message.msg_id.clone();

        // Bounded child delegation depends on the exact owner root. Load it
        // explicitly when it is older than the recent-context window; never
        // reconstruct or inject content from the desktop handoff.
        if let Some(root_id) = envelope.delegation_root.as_deref() {
            if !context
                .messages
                .iter()
                .any(|message| message.msg_id == root_id)
            {
                if let Some(root) = x0x.history_get(root_id).await? {
                    if let Some(root_envelope) = root
                        .envelope()
                        .filter(|_| root.is_safe_context(stable_group_id, &harness.config.agent_id))
                    {
                        context.push(&root, &root_envelope);
                    }
                }
            }
        }
        if row.is_safe_context(stable_group_id, &harness.config.agent_id) {
            context.push(&row, &envelope);
        }
        if !should_trigger(harness.config, stable_group_id, &row, &envelope, context) {
            return Err(HarnessError::PendingCausalRejected(msg_id));
        }

        after_durable_pending_claim(
            harness.config,
            state_path,
            state,
            row.id,
            &mut entry,
            || async {
                harness.lifecycle.publish(Lifecycle::Waking, None)?;
                handle_directed_message(harness.config, x0x, &row, &envelope, context, acp).await?;
                harness.lifecycle.publish(Lifecycle::Ready, None)?;
                Ok(())
            },
        )
        .await?;
    }
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
    reconcile_pending_causal(
        harness,
        x0x,
        stable_group_id,
        state_path,
        state,
        context,
        acp,
    )
    .await?;
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

        if let Some(envelope) = envelope
            .filter(|envelope| should_trigger(config, stable_group_id, &row, envelope, context))
        {
            let msg_id = row.msg_id.clone();
            after_durable_ordinary_claim(
                config,
                harness.causal_nonce,
                state_path,
                state,
                row.id,
                &msg_id,
                || async {
                    harness.lifecycle.publish(Lifecycle::Waking, None)?;
                    handle_directed_message(config, x0x, &row, &envelope, context, acp).await?;
                    harness.lifecycle.publish(Lifecycle::Ready, None)?;
                    Ok(())
                },
            )
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

#[cfg(test)]
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
    context: &ConversationContext,
) -> bool {
    let Some(author) = row.author_agent.as_deref() else {
        return false;
    };
    let basic_gate = row.is_verified_inbound(stable_group_id)
        && !author.eq_ignore_ascii_case(&config.agent_id)
        && config.author_allowed(author)
        && envelope
            .mentions
            .iter()
            .any(|mention| mention.eq_ignore_ascii_case(&config.agent_id));
    if !basic_gate {
        return false;
    }
    if !envelope.agent_generated {
        return true;
    }
    intentional_agent_delegation(config, row, envelope, context)
}

fn intentional_agent_delegation(
    config: &Config,
    row: &HistoryRow,
    envelope: &ChannelEnvelope,
    context: &ConversationContext,
) -> bool {
    let (Some(generation), Some(delegation_root)) = (
        envelope.agent_generation,
        envelope.delegation_root.as_deref(),
    ) else {
        return false;
    };
    if generation == 0 || generation > MAX_AGENT_WAKE_GENERATION {
        return false;
    }
    if row.thread_root.as_deref() != Some(delegation_root)
        || row.thread_parent.as_deref() != Some(delegation_root)
    {
        return false;
    }
    let Some(delegate) = row.author_agent.as_deref() else {
        return false;
    };
    context.messages.iter().any(|message| {
        message.msg_id == delegation_root
            && message.author.eq_ignore_ascii_case(&config.owner_agent_id)
            && !message.agent_generated
            && message.thread_root.is_none()
            && message.thread_parent.is_none()
            && message
                .mentions
                .iter()
                .any(|mention| mention.eq_ignore_ascii_case(delegate))
    })
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
            agent_generation: None,
            delegation_root: None,
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
        if outcome.has_ambiguous_tool_activity() {
            return Err(HarnessError::AmbiguousDirectedToolTurn {
                stop_reason: outcome.stop_reason,
            });
        }
        if outcome.has_visible_text() || outcome.has_completed_tool() {
            return Ok(outcome);
        }
        if outcome.saw_any_tool_call {
            return Err(HarnessError::AmbiguousDirectedToolTurn {
                stop_reason: outcome.stop_reason,
            });
        }
        if outcome.stop_reason != "end_turn" {
            return Err(HarnessError::IncompleteDirectedTurn {
                stop_reason: outcome.stop_reason,
            });
        }
        let may_retry = attempts <= MAX_SILENT_END_TURN_RETRIES;
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
         thread root and parent. For a delegated send, keep this Thread root and \
         set `thread_parent` to this Message id so the native tool can derive its \
         bounded delegation generation.\n\n\
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
    crate::storage::scoped_path(&config.data_dir, &config.group_id, "json")
}

fn load_state(path: &Path) -> Result<Option<DurableState>, HarnessError> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(HarnessError::StateIo(error)),
    }
}

fn persist_state(path: &Path, state: &DurableState) -> Result<(), HarnessError> {
    let bytes = serde_json::to_vec(state)?;
    crate::storage::write_atomic(path, &bytes)?;
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

    struct ScriptedMentionReconcile {
        state_path: PathBuf,
        state: DurableState,
        acp: ScriptedPrompter,
        reply_sends: usize,
        reconcile_calls: usize,
        directed: bool,
    }

    impl ReconcileAttempt for ScriptedMentionReconcile {
        fn run<'a>(&'a mut self) -> ReconcileFuture<'a> {
            Box::pin(async move {
                let Self {
                    state_path,
                    state,
                    acp,
                    reply_sends,
                    reconcile_calls,
                    directed,
                } = self;
                *reconcile_calls = reconcile_calls.saturating_add(1);
                const ROW_ID: i64 = 42;
                if ROW_ID <= state.last_seen_id {
                    return Ok(());
                }
                if !*directed {
                    return claim_row(state_path, state, ROW_ID);
                }
                after_durable_claim(state_path, state, ROW_ID, || async {
                    let outcome = prompt_directed_message(acp, "directed prompt").await?;
                    let _posted = post_turn_reply(&outcome, true, |_reply| {
                        *reply_sends = reply_sends.saturating_add(1);
                        std::future::ready(Ok(()))
                    })
                    .await?;
                    Ok(())
                })
                .await
            })
        }
    }

    #[test]
    fn directed_trigger_requires_verified_exact_mention_and_owner() {
        let config = test_config(RespondTo::OwnerOnly);
        let mut row = test_row(&config.owner_agent_id, &config.agent_id);
        let envelope = row.envelope().expect("valid envelope");
        assert!(should_trigger(
            &config,
            "stable",
            &row,
            &envelope,
            &ConversationContext::default()
        ));

        row.provenance = "LocalSend".to_string();
        assert!(!should_trigger(
            &config,
            "stable",
            &row,
            &envelope,
            &ConversationContext::default()
        ));
    }

    #[test]
    fn directed_trigger_rejects_allowed_author_without_mention() {
        let config = test_config(RespondTo::OwnerOnly);
        let row = test_row(&config.owner_agent_id, &"c".repeat(64));
        let envelope = row.envelope().expect("valid envelope");
        assert!(!should_trigger(
            &config,
            "stable",
            &row,
            &envelope,
            &ConversationContext::default()
        ));
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

        assert!(!should_trigger(
            &config,
            "stable",
            &row,
            &envelope,
            &ConversationContext::default()
        ));
    }

    #[test]
    fn owner_root_authorizes_one_allowlisted_agent_delegation() {
        let guide = "c".repeat(64);
        let mut config = test_config(RespondTo::Allowlist);
        config.respond_to_allowlist.insert(guide.clone());

        let root = test_row(&config.owner_agent_id, &guide);
        let root_envelope = root.envelope().expect("valid owner root");
        let mut context = ConversationContext::default();
        context.push(&root, &root_envelope);

        let mut delegated = test_row(&guide, &config.agent_id);
        delegated.msg_id = "2".repeat(64);
        delegated.thread_root = Some(root.msg_id.clone());
        delegated.thread_parent = Some(root.msg_id.clone());
        let mut delegated_envelope = delegated.envelope().expect("valid delegated envelope");
        delegated_envelope.agent_generated = true;
        delegated_envelope.agent_generation = Some(1);
        delegated_envelope.delegation_root = Some(root.msg_id.clone());
        set_row_envelope(&mut delegated, &delegated_envelope);
        let delegated_envelope = delegated.envelope().expect("encoded delegation");

        assert!(should_trigger(
            &config,
            "stable",
            &delegated,
            &delegated_envelope,
            &context
        ));
    }

    #[test]
    fn second_generation_cannot_wake_another_agent() {
        let guide = "c".repeat(64);
        let x = "d".repeat(64);
        let mut config = test_config(RespondTo::Allowlist);
        config.respond_to_allowlist.insert(x.clone());

        let root = test_row(&config.owner_agent_id, &guide);
        let root_envelope = root.envelope().expect("valid owner root");
        let mut context = ConversationContext::default();
        context.push(&root, &root_envelope);

        let mut delegated = test_row(&x, &config.agent_id);
        delegated.msg_id = "3".repeat(64);
        delegated.thread_root = Some(root.msg_id.clone());
        delegated.thread_parent = Some("2".repeat(64));
        let mut delegated_envelope = delegated.envelope().expect("valid delegated envelope");
        delegated_envelope.agent_generated = true;
        delegated_envelope.agent_generation = Some(2);
        delegated_envelope.delegation_root = Some(root.msg_id.clone());
        set_row_envelope(&mut delegated, &delegated_envelope);
        let delegated_envelope = delegated.envelope().expect("encoded delegation");

        assert!(!should_trigger(
            &config,
            "stable",
            &delegated,
            &delegated_envelope,
            &context
        ));
    }

    #[test]
    fn forged_delegation_without_owner_correlation_is_rejected() {
        let guide = "c".repeat(64);
        let unrelated = "d".repeat(64);
        let mut config = test_config(RespondTo::Allowlist);
        config.respond_to_allowlist.insert(guide.clone());

        let root = test_row(&config.owner_agent_id, &unrelated);
        let root_envelope = root.envelope().expect("valid owner root");
        let mut context = ConversationContext::default();
        context.push(&root, &root_envelope);

        let mut forged = test_row(&guide, &config.agent_id);
        forged.msg_id = "4".repeat(64);
        forged.thread_root = Some(root.msg_id.clone());
        forged.thread_parent = Some(root.msg_id.clone());
        let mut forged_envelope = forged.envelope().expect("valid forged envelope");
        forged_envelope.agent_generated = true;
        forged_envelope.agent_generation = Some(1);
        forged_envelope.delegation_root = Some(root.msg_id.clone());
        set_row_envelope(&mut forged, &forged_envelope);
        let forged_envelope = forged.envelope().expect("encoded forged envelope");

        assert!(!should_trigger(
            &config,
            "stable",
            &forged,
            &forged_envelope,
            &context
        ));
    }

    #[test]
    fn threaded_owner_reply_cannot_be_rebased_as_a_delegation_root() {
        let guide = "c".repeat(64);
        let actual_root = "9".repeat(64);
        let mut config = test_config(RespondTo::Allowlist);
        config.respond_to_allowlist.insert(guide.clone());

        let mut owner_reply = test_row(&config.owner_agent_id, &guide);
        owner_reply.thread_root = Some(actual_root.clone());
        owner_reply.thread_parent = Some(actual_root);
        let owner_reply_envelope = owner_reply.envelope().expect("valid owner reply");
        let mut context = ConversationContext::default();
        context.push(&owner_reply, &owner_reply_envelope);

        let mut rebased = test_row(&guide, &config.agent_id);
        rebased.msg_id = "5".repeat(64);
        rebased.thread_root = Some(owner_reply.msg_id.clone());
        rebased.thread_parent = Some(owner_reply.msg_id.clone());
        let mut rebased_envelope = rebased.envelope().expect("valid rebased envelope");
        rebased_envelope.agent_generated = true;
        rebased_envelope.agent_generation = Some(1);
        rebased_envelope.delegation_root = Some(owner_reply.msg_id.clone());
        set_row_envelope(&mut rebased, &rebased_envelope);
        let rebased_envelope = rebased.envelope().expect("encoded rebased envelope");

        assert!(!should_trigger(
            &config,
            "stable",
            &rebased,
            &rebased_envelope,
            &context
        ));
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
    async fn ambiguous_acp_failure_keeps_the_durable_claim() {
        let directory = tempfile::tempdir().expect("tempdir");
        let state_path = directory.path().join("state.json");
        let mut state = DurableState::default();

        let error = after_durable_claim(&state_path, &mut state, 42, || async {
            Err(HarnessError::Acp(AcpError::Timeout(Duration::from_secs(1))))
        })
        .await
        .expect_err("ACP failure must propagate");

        assert!(matches!(error, HarnessError::Acp(AcpError::Timeout(_))));
        assert_eq!(state.last_seen_id, 42);
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
    async fn pending_causal_claim_bypasses_high_watermark_and_releases_only_silence() {
        let directory = tempfile::tempdir().expect("tempdir");
        let mut config = test_config(RespondTo::OwnerOnly);
        config.data_dir = directory.path().to_path_buf();
        config.start_nonce = Some("1".repeat(32));
        let state_path = directory.path().join("state.json");
        let mut state = DurableState { last_seen_id: 99 };
        persist_state(&state_path, &state).expect("persist first-launch max watermark");
        let pending_dir = pending_causal_dir(&config);
        crate::storage::ensure_dir(&pending_dir).expect("create pending directory");
        let pending_path = pending_dir.join(format!("{}.json", "a".repeat(64)));
        let mut entry = PendingCausalEntry {
            path: pending_path.clone(),
            message: PendingCausalMessage {
                version: 2,
                start_nonce: "1".repeat(32),
                group_id: "group".to_string(),
                msg_id: "a".repeat(64),
                state: PendingCausalState::Pending,
            },
        };
        persist_pending_causal(&entry).expect("persist pending handoff");

        let error =
            after_durable_pending_claim(&config, &state_path, &mut state, 42, &mut entry, || {
                std::future::ready(Err(HarnessError::SilentDirectedTurn {
                    attempts: 2,
                    stop_reason: "end_turn".to_string(),
                }))
            })
            .await
            .expect_err("silent turn releases the pending handoff");

        assert!(matches!(error, HarnessError::SilentDirectedTurn { .. }));
        assert_eq!(state.last_seen_id, 99);
        let released: PendingCausalMessage = serde_json::from_slice(
            &std::fs::read(&pending_path).expect("released pending file exists"),
        )
        .expect("released pending file parses");
        assert_eq!(released.state, PendingCausalState::Pending);

        // This also models a crash after the retryable pending transition but
        // before lowering a high watermark: the pending override is processed
        // independently of that watermark and completes exactly once.
        state.last_seen_id = 99;
        persist_state(&state_path, &state).expect("persist crash-window watermark");
        after_durable_pending_claim(&config, &state_path, &mut state, 42, &mut entry, || {
            std::future::ready(Ok(()))
        })
        .await
        .expect("retry succeeds despite first-launch watermark");
        assert_eq!(state.last_seen_id, 99);
        assert!(!pending_path.exists());
        assert!(completed_causal_exists(&config, &"a".repeat(64)).expect("read completion"));

        // A stale pending file recreated after stop is removed by the durable
        // exact group+msg completion ledger and cannot trigger another turn.
        entry.message.state = PendingCausalState::Pending;
        persist_pending_causal(&entry).expect("recreate stale pending file");
        assert!(load_pending_causal(&config, &"1".repeat(32))
            .expect("completion suppresses replay")
            .is_empty());
        assert!(!pending_path.exists());

        let invalid_completion = CompletedCausalMessage {
            version: 1,
            group_id: "different-route".to_string(),
            msg_id: "a".repeat(64),
        };
        crate::storage::write_atomic(
            &completed_causal_path(&config, &"a".repeat(64)),
            &serde_json::to_vec(&invalid_completion).expect("encode invalid completion"),
        )
        .expect("mutate completion group");
        persist_pending_causal(&entry).expect("recreate pending beside invalid completion");
        assert!(matches!(
            load_pending_causal(&config, &"1".repeat(32)),
            Err(HarnessError::PendingCausalInvalid(_))
        ));
    }

    #[tokio::test]
    async fn ordinary_completion_suppresses_a_later_staged_causal_replay() {
        let directory = tempfile::tempdir().expect("tempdir");
        let mut config = test_config(RespondTo::OwnerOnly);
        config.data_dir = directory.path().to_path_buf();
        let nonce = "1".repeat(32);
        let msg_id = "c".repeat(64);
        let state_path = directory.path().join("state.json");
        let mut state = DurableState { last_seen_id: 7 };
        persist_state(&state_path, &state).expect("persist prior watermark");
        let turns = AtomicUsize::new(0);

        after_durable_ordinary_claim(
            &config,
            &nonce,
            &state_path,
            &mut state,
            42,
            &msg_id,
            || {
                turns.fetch_add(1, Ordering::SeqCst);
                std::future::ready(Ok(()))
            },
        )
        .await
        .expect("ordinary directed row completes");
        assert_eq!(turns.load(Ordering::SeqCst), 1);
        assert!(completed_causal_exists(&config, &msg_id).expect("read ordinary completion"));

        // Model a stale wake artifact that appears after the ordinary live
        // path completed. The exact shared tombstone removes it before ACP.
        let pending_dir = pending_causal_dir(&config);
        crate::storage::ensure_dir(&pending_dir).expect("create pending directory");
        let stale = PendingCausalEntry {
            path: pending_dir.join(format!("{msg_id}.json")),
            message: PendingCausalMessage {
                version: 2,
                start_nonce: nonce.clone(),
                group_id: config.group_id.clone(),
                msg_id: msg_id.clone(),
                state: PendingCausalState::Pending,
            },
        };
        persist_pending_causal(&stale).expect("stage stale causal wake");
        assert!(load_pending_causal(&config, &nonce)
            .expect("shared completion suppresses stale wake")
            .is_empty());

        after_durable_ordinary_claim(
            &config,
            &nonce,
            &state_path,
            &mut state,
            42,
            &msg_id,
            || {
                turns.fetch_add(1, Ordering::SeqCst);
                std::future::ready(Ok(()))
            },
        )
        .await
        .expect("replayed ordinary row is consumed");
        assert_eq!(turns.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn ambiguous_pending_causal_failure_keeps_both_claims() {
        let directory = tempfile::tempdir().expect("tempdir");
        let mut config = test_config(RespondTo::OwnerOnly);
        config.data_dir = directory.path().to_path_buf();
        config.start_nonce = Some("1".repeat(32));
        let state_path = directory.path().join("state.json");
        let mut state = DurableState { last_seen_id: 7 };
        persist_state(&state_path, &state).expect("persist prior watermark");
        let pending_dir = pending_causal_dir(&config);
        crate::storage::ensure_dir(&pending_dir).expect("create pending directory");
        let pending_path = pending_dir.join(format!("{}.json", "b".repeat(64)));
        let mut entry = PendingCausalEntry {
            path: pending_path.clone(),
            message: PendingCausalMessage {
                version: 2,
                start_nonce: "1".repeat(32),
                group_id: "group".to_string(),
                msg_id: "b".repeat(64),
                state: PendingCausalState::Pending,
            },
        };
        persist_pending_causal(&entry).expect("persist pending handoff");

        let error =
            after_durable_pending_claim(&config, &state_path, &mut state, 42, &mut entry, || {
                std::future::ready(Err(HarnessError::AmbiguousDirectedToolTurn {
                    stop_reason: "end_turn".to_string(),
                }))
            })
            .await
            .expect_err("ambiguous side effect remains visible");

        assert!(matches!(
            error,
            HarnessError::AmbiguousDirectedToolTurn { .. }
        ));
        assert_eq!(state.last_seen_id, 42);
        let claimed: PendingCausalMessage = serde_json::from_slice(
            &std::fs::read(pending_path).expect("claimed pending file remains"),
        )
        .expect("claimed pending file parses");
        assert_eq!(claimed.state, PendingCausalState::Executing);
        assert!(!completed_causal_exists(&config, &"b".repeat(64)).expect("read completion"));
    }

    #[tokio::test]
    async fn tool_free_blank_end_turn_gets_one_inline_retry() {
        let mut acp = ScriptedPrompter::new([silent_turn(), visible_turn("recovered")]);
        let outcome = prompt_directed_message(&mut acp, "directed prompt")
            .await
            .expect("one retry should recover");

        assert_eq!(outcome.assistant_text, "recovered");
        assert_eq!(acp.calls, 2);
    }

    #[tokio::test]
    async fn failed_tool_is_observable_and_never_retried_inline() {
        let mut acp = ScriptedPrompter::new([
            completed_tool_wire_turn("failed-tool", Some(true)),
            visible_turn("must remain unused"),
        ]);

        let error = prompt_directed_message(&mut acp, "directed prompt")
            .await
            .expect_err("ambiguous tool must fail visibly");

        assert!(matches!(
            error,
            HarnessError::AmbiguousDirectedToolTurn { ref stop_reason }
                if stop_reason == "end_turn"
        ));
        assert_eq!(acp.calls, 1);
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

    #[tokio::test(start_paused = true)]
    async fn real_recovery_path_reenters_tool_free_silent_row_and_posts_once() {
        let directory = tempfile::tempdir().expect("tempdir");
        let state_path = directory.path().join("state.json");
        let state = DurableState { last_seen_id: 7 };
        persist_state(&state_path, &state).expect("persist prior watermark");
        let mut attempt = ScriptedMentionReconcile {
            state_path,
            state,
            acp: ScriptedPrompter::new([
                silent_turn(),
                silent_turn(),
                visible_turn("recovered once"),
            ]),
            reply_sends: 0,
            reconcile_calls: 0,
            directed: true,
        };

        reconcile_with_silent_recovery(&mut attempt)
            .await
            .expect("in-process recovery succeeds");

        assert_eq!(attempt.reconcile_calls, 2);
        assert_eq!(attempt.acp.calls, 3);
        assert_eq!(attempt.reply_sends, 1);
        assert_eq!(attempt.state.last_seen_id, 42);
    }

    #[tokio::test(start_paused = true)]
    async fn real_recovery_path_caps_silence_and_leaves_row_released() {
        let directory = tempfile::tempdir().expect("tempdir");
        let state_path = directory.path().join("state.json");
        let state = DurableState { last_seen_id: 7 };
        persist_state(&state_path, &state).expect("persist prior watermark");
        let mut attempt = ScriptedMentionReconcile {
            state_path,
            state,
            acp: ScriptedPrompter::new(std::iter::repeat_with(silent_turn).take(8)),
            reply_sends: 0,
            reconcile_calls: 0,
            directed: true,
        };

        let error = reconcile_with_silent_recovery(&mut attempt)
            .await
            .expect_err("recovery cap must remain observable");

        assert!(matches!(error, HarnessError::SilentDirectedTurn { .. }));
        assert_eq!(attempt.reconcile_calls, 4);
        assert_eq!(attempt.acp.calls, 8);
        assert_eq!(attempt.reply_sends, 0);
        assert_eq!(attempt.state.last_seen_id, 7);
        assert_eq!(
            load_state(&attempt.state_path)
                .expect("reload state")
                .expect("released state exists")
                .last_seen_id,
            7
        );
    }

    #[tokio::test(start_paused = true)]
    async fn real_recovery_path_does_not_retry_ambiguous_tool_and_keeps_claim() {
        let directory = tempfile::tempdir().expect("tempdir");
        let state_path = directory.path().join("state.json");
        let state = DurableState { last_seen_id: 7 };
        persist_state(&state_path, &state).expect("persist prior watermark");
        let mut attempt = ScriptedMentionReconcile {
            state_path,
            state,
            acp: ScriptedPrompter::new([
                completed_tool_wire_turn("timed-out-space-send", Some(true)),
                visible_turn("must remain unused"),
            ]),
            reply_sends: 0,
            reconcile_calls: 0,
            directed: true,
        };

        let error = reconcile_with_silent_recovery(&mut attempt)
            .await
            .expect_err("ambiguous tool remains a visible failure");

        assert!(matches!(
            error,
            HarnessError::AmbiguousDirectedToolTurn { .. }
        ));
        assert_eq!(attempt.reconcile_calls, 1);
        assert_eq!(attempt.acp.calls, 1);
        assert_eq!(attempt.reply_sends, 0);
        assert_eq!(attempt.state.last_seen_id, 42);
    }

    #[tokio::test(start_paused = true)]
    async fn real_recovery_path_does_not_retry_non_end_turn_and_keeps_claim() {
        let directory = tempfile::tempdir().expect("tempdir");
        let state_path = directory.path().join("state.json");
        let state = DurableState { last_seen_id: 7 };
        persist_state(&state_path, &state).expect("persist prior watermark");
        let mut attempt = ScriptedMentionReconcile {
            state_path,
            state,
            acp: ScriptedPrompter::new([
                incomplete_turn("max_tokens"),
                visible_turn("must remain unused"),
            ]),
            reply_sends: 0,
            reconcile_calls: 0,
            directed: true,
        };

        let error = reconcile_with_silent_recovery(&mut attempt)
            .await
            .expect_err("non-end turn remains a visible failure");

        assert!(matches!(error, HarnessError::IncompleteDirectedTurn { .. }));
        assert_eq!(attempt.reconcile_calls, 1);
        assert_eq!(attempt.acp.calls, 1);
        assert_eq!(attempt.reply_sends, 0);
        assert_eq!(attempt.state.last_seen_id, 42);
    }

    #[tokio::test(start_paused = true)]
    async fn real_recovery_path_accepts_clean_tool_once() {
        let directory = tempfile::tempdir().expect("tempdir");
        let state_path = directory.path().join("state.json");
        let state = DurableState { last_seen_id: 7 };
        persist_state(&state_path, &state).expect("persist prior watermark");
        let mut attempt = ScriptedMentionReconcile {
            state_path,
            state,
            acp: ScriptedPrompter::new([completed_tool_wire_turn("clean-space-send", Some(false))]),
            reply_sends: 0,
            reconcile_calls: 0,
            directed: true,
        };

        reconcile_with_silent_recovery(&mut attempt)
            .await
            .expect("clean tool succeeds");

        assert_eq!(attempt.reconcile_calls, 1);
        assert_eq!(attempt.acp.calls, 1);
        assert_eq!(attempt.reply_sends, 0);
        assert_eq!(attempt.state.last_seen_id, 42);
    }

    #[tokio::test(start_paused = true)]
    async fn real_recovery_path_leaves_passive_rows_prompt_free() {
        let directory = tempfile::tempdir().expect("tempdir");
        let state_path = directory.path().join("state.json");
        let state = DurableState { last_seen_id: 7 };
        persist_state(&state_path, &state).expect("persist prior watermark");
        let mut attempt = ScriptedMentionReconcile {
            state_path,
            state,
            acp: ScriptedPrompter::new([visible_turn("must remain unused")]),
            reply_sends: 0,
            reconcile_calls: 0,
            directed: false,
        };

        reconcile_with_silent_recovery(&mut attempt)
            .await
            .expect("passive claim succeeds");

        assert_eq!(attempt.reconcile_calls, 1);
        assert_eq!(attempt.acp.calls, 0);
        assert_eq!(attempt.reply_sends, 0);
        assert_eq!(attempt.state.last_seen_id, 42);
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
            incomplete_turn("max_tokens"),
            visible_turn("must remain unused"),
        ]);

        let error = prompt_directed_message(&mut acp, "directed prompt")
            .await
            .expect_err("only end_turn is retryable");

        assert!(matches!(
            error,
            HarnessError::IncompleteDirectedTurn { ref stop_reason }
                if stop_reason == "max_tokens"
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

        assert!(!should_trigger(
            &config,
            "stable",
            &row,
            &envelope,
            &ConversationContext::default()
        ));
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
    fn conversation_context_deduplicates_reconciled_rows_by_message_id() {
        let config = test_config(RespondTo::OwnerOnly);
        let row = test_row(&config.owner_agent_id, &config.agent_id);
        let envelope = row.envelope().expect("valid envelope");
        let mut context = ConversationContext::default();

        context.push(&row, &envelope);
        context.push(&row, &envelope);

        assert_eq!(context.messages.len(), 1);
        assert_eq!(context.render_recent().matches(&row.msg_id).count(), 1);
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
        assert!(prompt.contains("bounded delegation generation"));
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
            saw_any_tool_call: false,
            ambiguous_tool_activity: false,
            completed_tool_call_ids: Vec::new(),
        }
    }

    fn visible_turn(text: &str) -> AcpTurnOutcome {
        AcpTurnOutcome {
            stop_reason: "end_turn".to_string(),
            assistant_text: text.to_string(),
            saw_any_tool_call: false,
            ambiguous_tool_activity: false,
            completed_tool_call_ids: Vec::new(),
        }
    }

    fn incomplete_turn(stop_reason: &str) -> AcpTurnOutcome {
        AcpTurnOutcome {
            stop_reason: stop_reason.to_string(),
            assistant_text: String::new(),
            saw_any_tool_call: false,
            ambiguous_tool_activity: false,
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

    fn set_row_envelope(row: &mut HistoryRow, envelope: &ChannelEnvelope) {
        row.payload = base64::engine::general_purpose::STANDARD
            .encode(serde_json::to_vec(envelope).expect("encode envelope"));
    }
}

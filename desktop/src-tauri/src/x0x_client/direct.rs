use serde::Serialize;

use super::{DirectSendReceipt, X0xClient, X0xClientError, DIRECT_SEND_REQUEST_TIMEOUT};

/// `POST /direct/send` body. The daemon validates `agent_id` as a 64-hex
/// AgentId and `payload` as base64. `logical_id` gives a retry a stable
/// sender-local identity; optional thread fields are canonical msg_ids.
#[derive(Serialize)]
pub(super) struct SendDirectBody<'a> {
    pub(super) agent_id: &'a str,
    pub(super) payload: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) logical_id: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) thread_root: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) thread_parent: Option<&'a str>,
}

impl X0xClient {
    /// `POST /direct/send` — native one-to-one direct message send.
    ///
    /// `logical_id` is the envelope `clientId`, so retrying the same optimistic
    /// message derives the same sender/recipient-scoped daemon request id.
    pub async fn send_direct_message(
        &self,
        agent_id: &str,
        payload_b64: &str,
        logical_id: Option<&str>,
        thread_root: Option<&str>,
        thread_parent: Option<&str>,
    ) -> Result<DirectSendReceipt, X0xClientError> {
        let body = SendDirectBody {
            agent_id,
            payload: payload_b64,
            logical_id,
            thread_root,
            thread_parent,
        };
        self.post_json_with_timeout("/direct/send", &body, DIRECT_SEND_REQUEST_TIMEOUT)
            .await
            .map_err(map_direct_send_error)
    }
}

/// Pull the daemon `error` field out of a Status excerpt such as
/// `/direct/send: {"ok":false,"error":"idempotency_conflict",...}`.
pub(super) fn daemon_error_code(excerpt: &str) -> Option<&str> {
    let key = "\"error\":\"";
    let start = excerpt.find(key)? + key.len();
    let rest = excerpt.get(start..)?;
    let end = rest.find('"')?;
    let code = &rest[..end];
    if code.is_empty() {
        None
    } else {
        Some(code)
    }
}

/// Product copy for ADR 0030 send refusals. The two 409s prescribe opposite
/// repairs: upgrade/opt-out vs never-retry-these-bytes. A 504 is "maybe
/// arrived" — the client must reuse `logical_id`, not mint a new one.
pub(crate) fn map_direct_send_error(err: X0xClientError) -> X0xClientError {
    let X0xClientError::Status(status, excerpt) = &err else {
        return err;
    };
    let Some(code) = daemon_error_code(excerpt) else {
        if *status == 504 {
            return X0xClientError::Status(
                *status,
                "Delivery wasn't confirmed. The message may still have arrived — retrying keeps the same id so it will not duplicate.".to_string(),
            );
        }
        return err;
    };
    let message = match code {
        "recipient_ack_semantics_unavailable" => {
            "Peer needs upgrading — it can't confirm durable delivery yet."
        }
        "idempotency_conflict" => {
            "That message id was already used for different content. Retrying won't help — send it as a new message."
        }
        "recipient_key_unavailable" => {
            "Peer not found — no published key or contact card for that agent yet."
        }
        "logical_id_requires_durable_ack" => {
            "Message id requires durable delivery; drop the id or keep durable send."
        }
        "require_gossip_ack_removed" => {
            "This client sent a field the daemon removed (require_gossip_ack). Update the app."
        }
        "timeout" => {
            "Delivery wasn't confirmed. The message may still have arrived — retrying keeps the same id so it will not duplicate."
        }
        _ => return err,
    };
    X0xClientError::Status(*status, message.to_string())
}

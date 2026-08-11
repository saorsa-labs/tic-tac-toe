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
    }
}

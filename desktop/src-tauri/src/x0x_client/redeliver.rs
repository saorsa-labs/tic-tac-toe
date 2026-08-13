use serde::{Deserialize, Serialize};

use super::{X0xClient, X0xClientError, DIRECT_SEND_REQUEST_TIMEOUT};

/// Narrow retained-artifact redelivery request. The daemon resolves the
/// canonical signed group message by path `msg_id`; callers never supply body
/// bytes, so this surface cannot synthesize or substitute message content.
#[derive(Serialize)]
struct RedeliverGroupMessageBody<'a> {
    agent_id: &'a str,
}

/// Durable target receipt from
/// `POST /groups/:id/messages/:msg_id/redeliver`.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct GroupMessageRedeliveryReceipt {
    pub ok: bool,
    pub group_id: String,
    pub msg_id: String,
    pub agent_id: String,
    pub outcome: String,
}

impl X0xClient {
    /// Re-deliver one retained signed group artifact to one exact active
    /// member and wait for the target's durable history receipt.
    ///
    /// The request intentionally contains no payload. A successful response
    /// therefore proves only that x0xd found the path-addressed retained
    /// artifact and established that exact artifact in the target history.
    pub async fn redeliver_group_message(
        &self,
        group_id: &str,
        msg_id: &str,
        agent_id: &str,
    ) -> Result<GroupMessageRedeliveryReceipt, X0xClientError> {
        let body = RedeliverGroupMessageBody { agent_id };
        self.post_json_with_timeout(
            &format!("/groups/{group_id}/messages/{msg_id}/redeliver"),
            &body,
            DIRECT_SEND_REQUEST_TIMEOUT,
        )
        .await
    }
}

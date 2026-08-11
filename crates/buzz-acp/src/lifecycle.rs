use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::config::Config;

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Lifecycle {
    Listening,
    Waking,
    Ready,
    Failed,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct LifecycleReceipt<'a> {
    start_nonce: &'a str,
    lifecycle: Lifecycle,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<&'a str>,
}

pub struct LifecyclePublisher {
    path: Option<PathBuf>,
    start_nonce: Option<String>,
}

impl LifecyclePublisher {
    pub fn new(config: &Config) -> Self {
        Self {
            path: config.start_nonce.as_ref().map(|_| {
                crate::storage::scoped_path(&config.data_dir, &config.group_id, "lifecycle.json")
            }),
            start_nonce: config.start_nonce.clone(),
        }
    }

    pub fn publish(&self, lifecycle: Lifecycle, error: Option<&str>) -> std::io::Result<()> {
        let (Some(path), Some(start_nonce)) = (&self.path, &self.start_nonce) else {
            return Ok(());
        };
        let receipt = LifecycleReceipt {
            start_nonce,
            lifecycle,
            error,
        };
        crate::storage::write_atomic(
            path,
            &serde_json::to_vec(&receipt).map_err(std::io::Error::other)?,
        )
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::time::Duration;

    use super::*;
    use crate::config::{Config, RespondTo};

    #[test]
    fn receipt_carries_the_exact_generation_nonce_and_lifecycle() {
        let directory = tempfile::tempdir().expect("tempdir");
        let config = Config {
            data_dir: directory.path().to_path_buf(),
            agent_id: "a".repeat(64),
            owner_agent_id: "b".repeat(64),
            group_id: "group".to_string(),
            agent_command: "agent".to_string(),
            agent_args: Vec::new(),
            system_prompt: None,
            respond_to: RespondTo::OwnerOnly,
            respond_to_allowlist: HashSet::new(),
            idle_timeout: Duration::from_secs(1),
            max_turn_duration: None,
            parallelism: 1,
            start_nonce: Some("0123456789abcdef0123456789abcdef".to_string()),
        };
        LifecyclePublisher::new(&config)
            .publish(Lifecycle::Ready, None)
            .expect("publish receipt");
        let path = crate::storage::scoped_path(directory.path(), "group", "lifecycle.json");
        assert!(!path
            .file_name()
            .expect("lifecycle filename")
            .to_string_lossy()
            .contains("group"));
        let value: serde_json::Value =
            serde_json::from_slice(&std::fs::read(path).expect("read receipt"))
                .expect("decode receipt");
        assert_eq!(value["startNonce"], "0123456789abcdef0123456789abcdef");
        assert_eq!(value["lifecycle"], "ready");
    }
}

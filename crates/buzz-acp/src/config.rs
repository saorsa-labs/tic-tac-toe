use std::collections::HashSet;
use std::path::PathBuf;
use std::time::Duration;

use thiserror::Error;

const DEFAULT_IDLE_TIMEOUT_SECS: u64 = 900;

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("required environment variable {0} is missing")]
    Missing(&'static str),
    #[error("{name} must be exactly 64 hexadecimal characters")]
    InvalidAgentId { name: &'static str },
    #[error("BUZZ_ACP_RESPOND_TO must be owner-only, allowlist, anyone, or nobody")]
    InvalidRespondTo,
    #[error("BUZZ_ACP_RESPOND_TO_ALLOWLIST contains an invalid AgentId")]
    InvalidAllowlist,
    #[error("BUZZ_ACP_RESPOND_TO=allowlist requires a non-empty allowlist")]
    EmptyAllowlist,
    #[error("BUZZ_ACP_IDLE_TIMEOUT must be a positive integer")]
    InvalidIdleTimeout,
    #[error("X0X_GROUP_ID contains unsupported path characters")]
    InvalidGroupId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RespondTo {
    OwnerOnly,
    Allowlist,
    Anyone,
    Nobody,
}

#[derive(Debug, Clone)]
pub struct Config {
    pub data_dir: PathBuf,
    pub agent_id: String,
    pub owner_agent_id: String,
    pub group_id: String,
    pub agent_command: String,
    pub agent_args: Vec<String>,
    pub system_prompt: Option<String>,
    pub respond_to: RespondTo,
    pub respond_to_allowlist: HashSet<String>,
    pub idle_timeout: Duration,
}

impl Config {
    pub fn from_env() -> Result<Self, ConfigError> {
        let data_dir = required("X0X_DATA_DIR")?;
        let agent_id = normalized_agent_id("X0X_AGENT_ID", &required("X0X_AGENT_ID")?)?;
        let owner_agent_id =
            normalized_agent_id("X0X_OWNER_AGENT_ID", &required("X0X_OWNER_AGENT_ID")?)?;
        let group_id = required("X0X_GROUP_ID")?;
        if group_id.len() > 256
            || !group_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        {
            return Err(ConfigError::InvalidGroupId);
        }

        let respond_to = match std::env::var("BUZZ_ACP_RESPOND_TO")
            .unwrap_or_else(|_| "owner-only".to_string())
            .as_str()
        {
            "owner-only" => RespondTo::OwnerOnly,
            "allowlist" => RespondTo::Allowlist,
            "anyone" => RespondTo::Anyone,
            "nobody" => RespondTo::Nobody,
            _ => return Err(ConfigError::InvalidRespondTo),
        };
        let respond_to_allowlist = parse_allowlist(
            std::env::var("BUZZ_ACP_RESPOND_TO_ALLOWLIST")
                .unwrap_or_default()
                .as_str(),
        )?;
        if respond_to == RespondTo::Allowlist && respond_to_allowlist.is_empty() {
            return Err(ConfigError::EmptyAllowlist);
        }

        let idle_timeout_secs = std::env::var("BUZZ_ACP_IDLE_TIMEOUT")
            .ok()
            .map(|value| value.parse::<u64>())
            .transpose()
            .map_err(|_| ConfigError::InvalidIdleTimeout)?
            .unwrap_or(DEFAULT_IDLE_TIMEOUT_SECS);
        if idle_timeout_secs == 0 {
            return Err(ConfigError::InvalidIdleTimeout);
        }

        Ok(Self {
            data_dir: PathBuf::from(data_dir),
            agent_id,
            owner_agent_id,
            group_id,
            agent_command: std::env::var("BUZZ_ACP_AGENT_COMMAND")
                .unwrap_or_else(|_| "buzz-agent".to_string()),
            agent_args: split_args(&std::env::var("BUZZ_ACP_AGENT_ARGS").unwrap_or_default()),
            system_prompt: std::env::var("BUZZ_ACP_SYSTEM_PROMPT")
                .ok()
                .filter(|value| !value.trim().is_empty()),
            respond_to,
            respond_to_allowlist,
            idle_timeout: Duration::from_secs(idle_timeout_secs),
        })
    }

    pub fn author_allowed(&self, author: &str) -> bool {
        let author = author.to_ascii_lowercase();
        match self.respond_to {
            RespondTo::OwnerOnly => author == self.owner_agent_id,
            RespondTo::Allowlist => {
                author == self.owner_agent_id || self.respond_to_allowlist.contains(&author)
            }
            RespondTo::Anyone => true,
            RespondTo::Nobody => false,
        }
    }
}

fn required(name: &'static str) -> Result<String, ConfigError> {
    std::env::var(name)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .ok_or(ConfigError::Missing(name))
}

fn normalized_agent_id(name: &'static str, value: &str) -> Result<String, ConfigError> {
    let value = value.trim();
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(ConfigError::InvalidAgentId { name });
    }
    Ok(value.to_ascii_lowercase())
}

fn parse_allowlist(raw: &str) -> Result<HashSet<String>, ConfigError> {
    raw.split(',')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .map(|entry| {
            normalized_agent_id("BUZZ_ACP_RESPOND_TO_ALLOWLIST", entry)
                .map_err(|_| ConfigError::InvalidAllowlist)
        })
        .collect()
}

fn split_args(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(str::trim)
        .filter(|arg| !arg.is_empty())
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn author_gate_owner_only_is_exact() {
        let config = test_config(RespondTo::OwnerOnly, HashSet::new());
        assert!(config.author_allowed(&"b".repeat(64)));
        assert!(!config.author_allowed(&"c".repeat(64)));
    }

    #[test]
    fn allowlist_adds_to_owner() {
        let allowed = "c".repeat(64);
        let config = test_config(RespondTo::Allowlist, HashSet::from([allowed.clone()]));
        assert!(config.author_allowed(&"b".repeat(64)));
        assert!(config.author_allowed(&allowed));
        assert!(!config.author_allowed(&"d".repeat(64)));
    }

    #[test]
    fn split_args_drops_empty_entries() {
        assert_eq!(split_args("acp, ,--flag"), vec!["acp", "--flag"]);
    }

    fn test_config(respond_to: RespondTo, respond_to_allowlist: HashSet<String>) -> Config {
        Config {
            data_dir: PathBuf::from("/tmp/x0x"),
            agent_id: "a".repeat(64),
            owner_agent_id: "b".repeat(64),
            group_id: "group".to_string(),
            agent_command: "buzz-agent".to_string(),
            agent_args: Vec::new(),
            system_prompt: None,
            respond_to,
            respond_to_allowlist,
            idle_timeout: Duration::from_secs(1),
        }
    }
}

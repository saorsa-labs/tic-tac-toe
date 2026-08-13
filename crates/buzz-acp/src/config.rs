use std::collections::HashSet;
use std::path::PathBuf;
use std::time::Duration;

use thiserror::Error;

const DEFAULT_IDLE_TIMEOUT_SECS: u64 = 900;
const MAX_SYSTEM_PROMPT_BYTES: usize = 512 * 1024;

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
    #[error("BUZZ_ACP_MAX_TURN_DURATION must be a positive integer")]
    InvalidMaxTurnDuration,
    #[error("native x0x ACP supports exactly one worker; BUZZ_ACP_AGENTS must be 1")]
    InvalidParallelism,
    #[error("BUZZ_MANAGED_AGENT_START_NONCE must be exactly 32 hexadecimal characters")]
    InvalidStartNonce,
    #[error("BUZZ_ACP_SETUP_PAYLOAD is unsupported by the native x0x harness")]
    UnsupportedSetup,
    #[error("combined BUZZ_ACP_SYSTEM_PROMPT and BUZZ_ACP_TEAM_INSTRUCTIONS exceed 512 KiB")]
    SystemPromptTooLarge,
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
    pub max_turn_duration: Option<Duration>,
    pub parallelism: usize,
    pub start_nonce: Option<String>,
}

impl Config {
    pub fn from_env() -> Result<Self, ConfigError> {
        reject_unsupported_setup(std::env::var_os("BUZZ_ACP_SETUP_PAYLOAD").as_deref())?;
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

        let max_turn_duration = std::env::var("BUZZ_ACP_MAX_TURN_DURATION")
            .ok()
            .map(|value| value.parse::<u64>())
            .transpose()
            .map_err(|_| ConfigError::InvalidMaxTurnDuration)?
            .map(|seconds| {
                if seconds == 0 {
                    Err(ConfigError::InvalidMaxTurnDuration)
                } else {
                    Ok(Duration::from_secs(seconds))
                }
            })
            .transpose()?;
        let parallelism = std::env::var("BUZZ_ACP_AGENTS")
            .ok()
            .map(|value| value.parse::<usize>())
            .transpose()
            .map_err(|_| ConfigError::InvalidParallelism)?
            .unwrap_or(1);
        validate_parallelism(parallelism)?;
        let start_nonce = std::env::var("BUZZ_MANAGED_AGENT_START_NONCE")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .map(|value| normalized_start_nonce(&value))
            .transpose()?;
        let system_prompt = combined_system_prompt(
            std::env::var("BUZZ_ACP_SYSTEM_PROMPT").ok(),
            std::env::var("BUZZ_ACP_TEAM_INSTRUCTIONS").ok(),
        )?;

        Ok(Self {
            data_dir: PathBuf::from(data_dir),
            agent_id,
            owner_agent_id,
            group_id,
            agent_command: std::env::var("BUZZ_ACP_AGENT_COMMAND")
                .unwrap_or_else(|_| "buzz-agent".to_string()),
            agent_args: split_args(&std::env::var("BUZZ_ACP_AGENT_ARGS").unwrap_or_default()),
            system_prompt,
            respond_to,
            respond_to_allowlist,
            idle_timeout: Duration::from_secs(idle_timeout_secs),
            max_turn_duration,
            parallelism,
            start_nonce,
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

fn reject_unsupported_setup(value: Option<&std::ffi::OsStr>) -> Result<(), ConfigError> {
    if value.is_some() {
        Err(ConfigError::UnsupportedSetup)
    } else {
        Ok(())
    }
}

fn validate_parallelism(parallelism: usize) -> Result<(), ConfigError> {
    if parallelism == 1 {
        Ok(())
    } else {
        Err(ConfigError::InvalidParallelism)
    }
}

fn normalized_start_nonce(value: &str) -> Result<String, ConfigError> {
    let value = value.trim();
    if value.len() != 32 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(ConfigError::InvalidStartNonce);
    }
    Ok(value.to_ascii_lowercase())
}

fn combined_system_prompt(
    system_prompt: Option<String>,
    team_instructions: Option<String>,
) -> Result<Option<String>, ConfigError> {
    let system_prompt = system_prompt.filter(|value| !value.trim().is_empty());
    let team_instructions = team_instructions.filter(|value| !value.trim().is_empty());
    let combined = match (system_prompt, team_instructions) {
        (Some(prompt), Some(team)) => {
            Some(format!("{prompt}\n\n[Team instructions]\n{}", team.trim()))
        }
        (Some(prompt), None) => Some(prompt),
        (None, Some(team)) => Some(format!("[Team instructions]\n{}", team.trim())),
        (None, None) => None,
    };
    if combined
        .as_ref()
        .is_some_and(|value| value.len() > MAX_SYSTEM_PROMPT_BYTES)
    {
        return Err(ConfigError::SystemPromptTooLarge);
    }
    Ok(combined)
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

    #[test]
    fn team_instructions_are_part_of_the_acp_system_prompt() {
        let combined = combined_system_prompt(
            Some("persona".to_string()),
            Some("work with Pip".to_string()),
        )
        .expect("valid prompt")
        .expect("combined prompt");
        assert_eq!(combined, "persona\n\n[Team instructions]\nwork with Pip");
    }

    #[test]
    fn lifecycle_nonce_is_fixed_width_hex() {
        assert_eq!(
            normalized_start_nonce("ABCDEF0123456789ABCDEF0123456789").expect("valid nonce"),
            "abcdef0123456789abcdef0123456789"
        );
        assert!(normalized_start_nonce("predictable").is_err());
    }

    #[test]
    fn unsupported_setup_and_parallel_workers_fail_closed() {
        assert!(reject_unsupported_setup(None).is_ok());
        assert!(reject_unsupported_setup(Some(std::ffi::OsStr::new("{}"))).is_err());
        assert!(validate_parallelism(1).is_ok());
        assert!(validate_parallelism(24).is_err());
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
            max_turn_duration: None,
            parallelism: 1,
            start_nonce: None,
        }
    }
}

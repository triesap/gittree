use gittree_observability::{ObservabilityConfigError, ObservabilityError, ObservabilityHandle};
pub use gittree_core::{CommandParseError, ParsedCommand, parse_cli_command};
pub mod ingest;
pub use ingest::{
    DispatchFilterConfig, IngestRejectReason, RelayEventEnvelope, is_dispatch_command_event,
};

const ENV_BIND: &str = "GITTREE_DISPATCH_BIND";
const ENV_ADMIN_PUBKEY: &str = "GITTREE_DISPATCH_ADMIN_PUBKEY";
const ENV_RELAY_URLS: &str = "GITTREE_DISPATCH_RELAY_URLS";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DispatchConfig {
    pub bind: String,
    pub admin_pubkey: String,
    pub relay_urls: Vec<String>,
}

impl DispatchConfig {
    pub fn from_env() -> Result<Self, DispatchError> {
        let mut get_var = |key| std::env::var(key).ok();
        Self::from_env_with(&mut get_var)
    }

    pub fn from_env_with(
        get_var: &mut dyn FnMut(&'static str) -> Option<String>,
    ) -> Result<Self, DispatchError> {
        let bind = get_var(ENV_BIND).unwrap_or_else(|| "127.0.0.1:8091".to_string());
        let admin_pubkey = get_var(ENV_ADMIN_PUBKEY)
            .ok_or_else(|| DispatchError::Config(format!("missing env {ENV_ADMIN_PUBKEY}")))?;
        let relay_urls = parse_csv(&get_var(ENV_RELAY_URLS).unwrap_or_default());
        if relay_urls.is_empty() {
            return Err(DispatchError::Config(format!(
                "missing relay urls in {ENV_RELAY_URLS}"
            )));
        }
        Ok(Self {
            bind,
            admin_pubkey,
            relay_urls,
        })
    }
}

fn parse_csv(input: &str) -> Vec<String> {
    input
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .collect()
}

#[derive(Debug)]
pub enum DispatchError {
    Config(String),
    ObservabilityConfig(ObservabilityConfigError),
    Observability(ObservabilityError),
}

impl std::fmt::Display for DispatchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DispatchError::Config(message) => write!(f, "dispatch config error: {message}"),
            DispatchError::ObservabilityConfig(err) => {
                write!(f, "dispatch observability config error: {err}")
            }
            DispatchError::Observability(err) => write!(f, "dispatch observability error: {err}"),
        }
    }
}

impl std::error::Error for DispatchError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            DispatchError::Config(_) => None,
            DispatchError::ObservabilityConfig(err) => Some(err),
            DispatchError::Observability(err) => Some(err),
        }
    }
}

pub fn init_observability() -> Result<ObservabilityHandle, DispatchError> {
    let config = gittree_observability::ObservabilityConfig::from_env("gittree-dispatch")
        .map_err(DispatchError::ObservabilityConfig)?;
    let handle = gittree_observability::init(&config).map_err(DispatchError::Observability)?;
    Ok(handle)
}

pub async fn serve(config: DispatchConfig) -> Result<(), DispatchError> {
    let _guard = init_observability()?;
    tracing::info!(
        bind = %config.bind,
        relay_count = config.relay_urls.len(),
        "dispatch service scaffold initialized"
    );
    Ok(())
}

pub fn parse_command_content(content: &str) -> Result<ParsedCommand, CommandParseError> {
    parse_cli_command(content)
}

pub fn dispatch_filter_config(config: &DispatchConfig) -> DispatchFilterConfig {
    DispatchFilterConfig {
        admin_pubkey: config.admin_pubkey.clone(),
        relay_allowlist: config.relay_urls.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::{DispatchConfig, DispatchError, dispatch_filter_config, parse_csv};
    use std::collections::HashMap;

    fn from_pairs(values: &[(&'static str, &'static str)]) -> Result<DispatchConfig, DispatchError> {
        let map: HashMap<&'static str, &'static str> = values.iter().copied().collect();
        let mut get_var = |key: &'static str| map.get(key).map(|value| value.to_string());
        DispatchConfig::from_env_with(&mut get_var)
    }

    #[test]
    fn parse_csv_handles_empty_segments() {
        let values = parse_csv("a, ,b,, c ");
        assert_eq!(values, vec!["a", "b", "c"]);
    }

    #[test]
    fn parse_command_content_delegates_to_core_parser() {
        let command = super::parse_command_content("gittree account create").expect("command");
        assert_eq!(command.action, "create");
    }

    #[test]
    fn dispatch_filter_config_uses_dispatch_settings() {
        let config = from_pairs(&[
            ("GITTREE_DISPATCH_ADMIN_PUBKEY", "npub1admin"),
            ("GITTREE_DISPATCH_RELAY_URLS", "wss://gittr.ee,wss://relay.example"),
        ])
        .expect("config");
        let filter = dispatch_filter_config(&config);
        assert_eq!(filter.admin_pubkey, "npub1admin");
        assert_eq!(
            filter.relay_allowlist,
            vec!["wss://gittr.ee".to_string(), "wss://relay.example".to_string()]
        );
    }

    #[test]
    fn from_env_requires_admin_pubkey() {
        let err = from_pairs(&[("GITTREE_DISPATCH_RELAY_URLS", "wss://gittr.ee")])
            .expect_err("missing admin key");
        assert!(matches!(err, DispatchError::Config(message) if message.contains("GITTREE_DISPATCH_ADMIN_PUBKEY")));
    }

    #[test]
    fn from_env_requires_relay_urls() {
        let err = from_pairs(&[("GITTREE_DISPATCH_ADMIN_PUBKEY", "npub1admin")])
            .expect_err("missing relay urls");
        assert!(matches!(err, DispatchError::Config(message) if message.contains("GITTREE_DISPATCH_RELAY_URLS")));
    }

    #[test]
    fn from_env_loads_expected_values() {
        let config = from_pairs(&[
            ("GITTREE_DISPATCH_BIND", "127.0.0.1:19091"),
            ("GITTREE_DISPATCH_ADMIN_PUBKEY", "npub1admin"),
            (
                "GITTREE_DISPATCH_RELAY_URLS",
                "wss://gittr.ee,wss://relay.example",
            ),
        ])
        .expect("config");
        assert_eq!(config.bind, "127.0.0.1:19091");
        assert_eq!(config.admin_pubkey, "npub1admin");
        assert_eq!(
            config.relay_urls,
            vec!["wss://gittr.ee".to_string(), "wss://relay.example".to_string()]
        );
    }
}

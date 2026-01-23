use gittree_config::{ConfigError, ServicesConfig};
use gittree_core::{RepoState, UpdateDecision};
use serde::Deserialize;
use std::path::Path;
use std::time::Duration;

const ENV_STATE_URL: &str = "GITTREE_STATE_URL";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookConfig {
    pub state_url: String,
}

impl HookConfig {
    pub fn from_env() -> Result<Self, HookConfigError> {
        let _services = ServicesConfig::from_env_validated().map_err(HookConfigError::Config)?;
        let state_url =
            std::env::var(ENV_STATE_URL).map_err(|_| HookConfigError::MissingEnv(ENV_STATE_URL))?;
        Ok(Self { state_url })
    }
}

#[derive(Debug)]
pub enum HookConfigError {
    Config(ConfigError),
    MissingEnv(&'static str),
}

impl std::fmt::Display for HookConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HookConfigError::Config(err) => write!(f, "hook config error: {err}"),
            HookConfigError::MissingEnv(key) => write!(f, "missing env {key}"),
        }
    }
}

impl std::error::Error for HookConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            HookConfigError::Config(err) => Some(err),
            HookConfigError::MissingEnv(_) => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefUpdate {
    pub old: String,
    pub new: String,
    pub reference: String,
}

#[derive(Debug)]
pub enum HookError {
    InvalidLine(String),
}

impl std::fmt::Display for HookError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HookError::InvalidLine(line) => write!(f, "invalid ref line: {line}"),
        }
    }
}

impl std::error::Error for HookError {}

#[derive(Debug)]
pub enum HookServiceError {
    Config(HookConfigError),
    Parse(HookError),
    Core(String),
    State(String),
    Reject(String),
}

impl std::fmt::Display for HookServiceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HookServiceError::Config(err) => write!(f, "hook config error: {err}"),
            HookServiceError::Parse(err) => write!(f, "hook parse error: {err}"),
            HookServiceError::Core(err) => write!(f, "hook core error: {err}"),
            HookServiceError::State(err) => write!(f, "hook state error: {err}"),
            HookServiceError::Reject(reason) => write!(f, "{reason}"),
        }
    }
}

impl std::error::Error for HookServiceError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            HookServiceError::Config(err) => Some(err),
            HookServiceError::Parse(err) => Some(err),
            HookServiceError::Core(_) => None,
            HookServiceError::State(_) => None,
            HookServiceError::Reject(_) => None,
        }
    }
}

pub fn parse_updates(input: &str) -> Result<Vec<RefUpdate>, HookError> {
    let mut updates = Vec::new();
    for line in input.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let mut parts = line.split_whitespace();
        let old = parts
            .next()
            .ok_or_else(|| HookError::InvalidLine(line.to_string()))?;
        let new = parts
            .next()
            .ok_or_else(|| HookError::InvalidLine(line.to_string()))?;
        let reference = parts
            .next()
            .ok_or_else(|| HookError::InvalidLine(line.to_string()))?;
        updates.push(RefUpdate {
            old: old.to_string(),
            new: new.to_string(),
            reference: reference.to_string(),
        });
    }
    Ok(updates)
}

pub trait StateFetcher {
    fn latest_state(
        &self,
        pubkey: &str,
        identifier: &str,
    ) -> Result<Option<RepoState>, HookServiceError>;
}

#[derive(Debug)]
pub struct HttpStateFetcher {
    base_url: String,
    client: reqwest::blocking::Client,
}

impl HttpStateFetcher {
    pub fn new(base_url: impl Into<String>, timeout: Duration) -> Result<Self, HookServiceError> {
        let client = reqwest::blocking::Client::builder()
            .timeout(timeout)
            .build()
            .map_err(|err| HookServiceError::State(err.to_string()))?;
        Ok(Self {
            base_url: base_url.into(),
            client,
        })
    }

    fn state_endpoint(&self, pubkey: &str, identifier: &str) -> String {
        format!(
            "{}/state/{pubkey}/{identifier}",
            self.base_url.trim_end_matches('/')
        )
    }
}

#[derive(Debug, Deserialize)]
struct StateResponse {
    identifier: String,
    state: std::collections::HashMap<String, String>,
}

impl StateFetcher for HttpStateFetcher {
    fn latest_state(
        &self,
        pubkey: &str,
        identifier: &str,
    ) -> Result<Option<RepoState>, HookServiceError> {
        let url = self.state_endpoint(pubkey, identifier);
        let response = self
            .client
            .get(url)
            .send()
            .map_err(|err| HookServiceError::State(err.to_string()))?;

        if response.status().as_u16() == 404 {
            return Ok(None);
        }

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().unwrap_or_default();
            return Err(HookServiceError::State(format!(
                "state service error {status}: {body}"
            )));
        }

        let state = response
            .json::<StateResponse>()
            .map_err(|err| HookServiceError::State(err.to_string()))?;

        Ok(Some(RepoState {
            identifier: state.identifier,
            state: state.state,
        }))
    }
}

pub fn evaluate_pre_receive<F>(
    fetcher: &F,
    repo_path: impl AsRef<Path>,
    updates: &[RefUpdate],
) -> Result<UpdateDecision, HookServiceError>
where
    F: StateFetcher,
{
    let repo = gittree_core::parse_repo_path(repo_path)
        .map_err(|err| HookServiceError::Core(err.to_string()))?;
    let state = fetcher.latest_state(&repo.pubkey, &repo.identifier)?;
    let core_updates: Vec<gittree_core::RefUpdate<'_>> = updates
        .iter()
        .map(|update| gittree_core::RefUpdate::new(&update.old, &update.new, &update.reference))
        .collect();
    Ok(gittree_core::evaluate_updates(
        &core_updates,
        state.as_ref(),
    ))
}

#[cfg(test)]
mod tests {
    use super::HookError;
    use super::StateFetcher;
    use super::evaluate_pre_receive;
    use super::parse_updates;
    use gittree_core::RepoState;
    use std::collections::HashMap;

    #[test]
    fn parse_updates_accepts_lines() {
        let input = "old new refs/heads/main\nold2 new2 refs/tags/v1";
        let updates = parse_updates(input).expect("updates");
        assert_eq!(updates.len(), 2);
        assert_eq!(updates[0].reference, "refs/heads/main");
        assert_eq!(updates[1].new, "new2");
    }

    #[test]
    fn parse_updates_rejects_missing_fields() {
        let err = parse_updates("only-two parts").unwrap_err();
        assert!(matches!(err, HookError::InvalidLine(_)));
    }

    struct MockFetcher {
        state: Option<RepoState>,
    }

    impl StateFetcher for MockFetcher {
        fn latest_state(
            &self,
            _pubkey: &str,
            _identifier: &str,
        ) -> Result<Option<RepoState>, super::HookServiceError> {
            Ok(self.state.clone())
        }
    }

    #[test]
    fn evaluate_pre_receive_rejects_mismatch() {
        let mut state_map = HashMap::new();
        state_map.insert(
            "refs/heads/main".to_string(),
            "0123456789abcdef0123456789abcdef01234567".to_string(),
        );
        state_map.insert("HEAD".to_string(), "ref: refs/heads/main".to_string());
        let fetcher = MockFetcher {
            state: Some(RepoState {
                identifier: "repo".to_string(),
                state: state_map,
            }),
        };
        let updates = vec![super::RefUpdate {
            old: "0".repeat(40),
            new: "1".repeat(40),
            reference: "refs/heads/main".to_string(),
        }];
        let repo_path = std::path::Path::new("/tmp")
            .join("npub1gjttreegkzys8jlhdnfm3qe39h2gka79cpndd0jsms5fk7tuhcnsdw56jq")
            .join("repo.git");
        let decision = evaluate_pre_receive(&fetcher, repo_path, &updates).expect("decision");
        assert!(matches!(
            decision,
            gittree_core::UpdateDecision::Reject { .. }
        ));
    }
}

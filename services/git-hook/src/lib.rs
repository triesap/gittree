use gittree_config::{ConfigError, ServicesConfig};
use gittree_core::{RepoState, UpdateDecision};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::time::Duration;

const ENV_STATE_URL: &str = "GITTREE_STATE_URL";
const ENV_SYNC_URL: &str = "GITTREE_SYNC_URL";
const ENV_HOOK_MODE: &str = "GITTREE_HOOK_MODE";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookConfig {
    pub state_url: String,
    pub sync_url: Option<String>,
    pub mode: HookMode,
}

impl HookConfig {
    pub fn from_env() -> Result<Self, HookConfigError> {
        let _services = ServicesConfig::from_env_validated().map_err(HookConfigError::Config)?;
        let state_url =
            std::env::var(ENV_STATE_URL).map_err(|_| HookConfigError::MissingEnv(ENV_STATE_URL))?;
        let mode = HookMode::from_env()?;
        let sync_url = std::env::var(ENV_SYNC_URL).ok();
        if matches!(mode, HookMode::PostReceive) && sync_url.is_none() {
            return Err(HookConfigError::MissingEnv(ENV_SYNC_URL));
        }
        Ok(Self {
            state_url,
            sync_url,
            mode,
        })
    }
}

#[derive(Debug)]
pub enum HookConfigError {
    Config(ConfigError),
    MissingEnv(&'static str),
    InvalidMode(String),
}

impl std::fmt::Display for HookConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HookConfigError::Config(err) => write!(f, "hook config error: {err}"),
            HookConfigError::MissingEnv(key) => write!(f, "missing env {key}"),
            HookConfigError::InvalidMode(value) => write!(f, "invalid hook mode: {value}"),
        }
    }
}

impl std::error::Error for HookConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            HookConfigError::Config(err) => Some(err),
            HookConfigError::MissingEnv(_) => None,
            HookConfigError::InvalidMode(_) => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookMode {
    PreReceive,
    PostReceive,
}

impl HookMode {
    pub fn from_env() -> Result<Self, HookConfigError> {
        let mode = std::env::var(ENV_HOOK_MODE).unwrap_or_else(|_| "pre-receive".to_string());
        match mode.as_str() {
            "pre-receive" => Ok(HookMode::PreReceive),
            "post-receive" => Ok(HookMode::PostReceive),
            value => Err(HookConfigError::InvalidMode(value.to_string())),
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
    InvalidPayload(String),
}

impl std::fmt::Display for HookError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HookError::InvalidLine(line) => write!(f, "invalid ref line: {line}"),
            HookError::InvalidPayload(message) => write!(f, "invalid payload: {message}"),
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForgejoPushEvent {
    pub owner: String,
    pub repo: String,
    pub full_name: String,
    pub reference: String,
    pub before: String,
    pub after: String,
}

#[derive(Debug, Deserialize)]
struct ForgejoPushPayload {
    #[serde(rename = "ref")]
    reference: String,
    before: String,
    after: String,
    repository: ForgejoRepoPayload,
}

#[derive(Debug, Deserialize)]
struct ForgejoRepoPayload {
    name: String,
    #[serde(default)]
    full_name: Option<String>,
    owner: ForgejoUserPayload,
}

#[derive(Debug, Deserialize)]
struct ForgejoUserPayload {
    username: String,
}

pub fn parse_forgejo_push(payload: &str) -> Result<ForgejoPushEvent, HookError> {
    let parsed: ForgejoPushPayload =
        serde_json::from_str(payload).map_err(|err| HookError::InvalidPayload(err.to_string()))?;

    ensure_non_empty("ref", &parsed.reference)?;
    ensure_non_empty("before", &parsed.before)?;
    ensure_non_empty("after", &parsed.after)?;
    ensure_non_empty("repository.name", &parsed.repository.name)?;
    ensure_non_empty("repository.owner.username", &parsed.repository.owner.username)?;

    let full_name = parsed
        .repository
        .full_name
        .unwrap_or_else(|| format!("{}/{}", parsed.repository.owner.username, parsed.repository.name));

    if !full_name.contains('/') {
        return Err(HookError::InvalidPayload(format!(
            "invalid repository full_name: {full_name}"
        )));
    }

    Ok(ForgejoPushEvent {
        owner: parsed.repository.owner.username,
        repo: parsed.repository.name,
        full_name,
        reference: parsed.reference,
        before: parsed.before,
        after: parsed.after,
    })
}

fn ensure_non_empty(field: &str, value: &str) -> Result<(), HookError> {
    if value.trim().is_empty() {
        return Err(HookError::InvalidPayload(format!(
            "missing required field: {field}"
        )));
    }
    Ok(())
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

pub trait PostReceiveNotifier {
    fn notify(&self, payload: PostReceivePayload) -> Result<(), HookServiceError>;
}

#[derive(Debug, Clone, Serialize)]
pub struct PostReceivePayload {
    pub pubkey: String,
    pub identifier: String,
    pub updates: Vec<RefUpdatePayload>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RefUpdatePayload {
    pub old: String,
    pub new: String,
    pub reference: String,
}

impl From<&RefUpdate> for RefUpdatePayload {
    fn from(update: &RefUpdate) -> Self {
        Self {
            old: update.old.clone(),
            new: update.new.clone(),
            reference: update.reference.clone(),
        }
    }
}

#[derive(Debug)]
pub struct HttpPostReceiveNotifier {
    endpoint: String,
    client: reqwest::blocking::Client,
}

impl HttpPostReceiveNotifier {
    pub fn new(endpoint: impl Into<String>, timeout: Duration) -> Result<Self, HookServiceError> {
        let client = reqwest::blocking::Client::builder()
            .timeout(timeout)
            .build()
            .map_err(|err| HookServiceError::State(err.to_string()))?;
        Ok(Self {
            endpoint: endpoint.into(),
            client,
        })
    }
}

impl PostReceiveNotifier for HttpPostReceiveNotifier {
    fn notify(&self, payload: PostReceivePayload) -> Result<(), HookServiceError> {
        let response = self
            .client
            .post(&self.endpoint)
            .json(&payload)
            .send()
            .map_err(|err| HookServiceError::State(err.to_string()))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().unwrap_or_default();
            return Err(HookServiceError::State(format!(
                "post-receive error {status}: {body}"
            )));
        }

        Ok(())
    }
}

pub fn handle_post_receive<N>(
    notifier: &N,
    repo_path: impl AsRef<Path>,
    updates: &[RefUpdate],
) -> Result<(), HookServiceError>
where
    N: PostReceiveNotifier,
{
    let repo = gittree_core::parse_repo_path(repo_path)
        .map_err(|err| HookServiceError::Core(err.to_string()))?;
    let payload = PostReceivePayload {
        pubkey: repo.pubkey,
        identifier: repo.identifier,
        updates: updates.iter().map(RefUpdatePayload::from).collect(),
    };
    notifier.notify(payload)
}

#[cfg(test)]
mod tests {
    use super::HookError;
    use super::PostReceiveNotifier;
    use super::PostReceivePayload;
    use super::StateFetcher;
    use super::evaluate_pre_receive;
    use super::handle_post_receive;
    use super::parse_forgejo_push;
    use super::parse_updates;
    use gittree_core::RepoState;
    use std::collections::HashMap;
    use std::sync::Mutex;

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

    #[test]
    fn parse_forgejo_push_accepts_payload() {
        let payload = r#"
        {
            "ref": "refs/heads/main",
            "before": "0000000000000000000000000000000000000000",
            "after": "1111111111111111111111111111111111111111",
            "repository": {
                "name": "repo",
                "full_name": "owner/repo",
                "owner": { "username": "owner" }
            }
        }
        "#;
        let event = parse_forgejo_push(payload).expect("event");
        assert_eq!(event.owner, "owner");
        assert_eq!(event.repo, "repo");
        assert_eq!(event.full_name, "owner/repo");
        assert_eq!(event.reference, "refs/heads/main");
    }

    #[test]
    fn parse_forgejo_push_rejects_missing_fields() {
        let payload = r#"
        {
            "ref": "",
            "before": "0000",
            "after": "1111",
            "repository": {
                "name": "repo",
                "owner": { "username": "owner" }
            }
        }
        "#;
        let err = parse_forgejo_push(payload).unwrap_err();
        assert!(matches!(err, HookError::InvalidPayload(_)));
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

    struct MockNotifier {
        payloads: Mutex<Vec<PostReceivePayload>>,
    }

    impl MockNotifier {
        fn new() -> Self {
            Self {
                payloads: Mutex::new(Vec::new()),
            }
        }
    }

    impl PostReceiveNotifier for MockNotifier {
        fn notify(&self, payload: PostReceivePayload) -> Result<(), super::HookServiceError> {
            let mut payloads = self.payloads.lock().expect("payload lock");
            payloads.push(payload);
            Ok(())
        }
    }

    #[test]
    fn handle_post_receive_sends_payload() {
        let notifier = MockNotifier::new();
        let updates = vec![super::RefUpdate {
            old: "0".repeat(40),
            new: "1".repeat(40),
            reference: "refs/heads/main".to_string(),
        }];
        let repo_path = std::path::Path::new("/tmp")
            .join("npub1gjttreegkzys8jlhdnfm3qe39h2gka79cpndd0jsms5fk7tuhcnsdw56jq")
            .join("repo.git");
        handle_post_receive(&notifier, repo_path, &updates).expect("post receive");
        let payloads = notifier.payloads.lock().expect("payload lock");
        assert_eq!(payloads.len(), 1);
        assert_eq!(payloads[0].updates[0].reference, "refs/heads/main");
    }
}

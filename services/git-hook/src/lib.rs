use gittree_config::{ConfigError, ServicesConfig};
use gittree_core::{RepoMapping, RepoState, UpdateDecision};
use hmac::Mac;
use serde::{Deserialize, Serialize};
use std::io::{IsTerminal, Read};
use std::path::{Path, PathBuf};
use std::time::Duration;

const ENV_STATE_URL: &str = "GITTREE_STATE_URL";
const ENV_SYNC_URL: &str = "GITTREE_SYNC_URL";
const ENV_HOOK_MODE: &str = "GITTREE_HOOK_MODE";
const ENV_HOOK_REPO_PATH: &str = "GITTREE_HOOK_REPO_PATH";
const ENV_HOOK_STDIN_FILE: &str = "GITTREE_HOOK_STDIN_FILE";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookConfig {
    pub state_url: String,
    pub sync_url: Option<String>,
    pub mode: HookMode,
}

impl HookConfig {
    pub fn from_env() -> Result<Self, HookConfigError> {
        Self::from_env_with_overrides(None, None, None)
    }

    pub fn from_env_with_overrides(
        mode: Option<HookMode>,
        state_url: Option<String>,
        sync_url: Option<String>,
    ) -> Result<Self, HookConfigError> {
        let _services = ServicesConfig::from_env_validated().map_err(HookConfigError::Config)?;
        let state_url = state_url
            .or_else(|| std::env::var(ENV_STATE_URL).ok())
            .ok_or(HookConfigError::MissingEnv(ENV_STATE_URL))?;
        let mode = mode.unwrap_or(HookMode::from_env()?);
        let sync_url = sync_url.or_else(|| std::env::var(ENV_SYNC_URL).ok());
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
    InvalidSignature(String),
}

impl std::fmt::Display for HookError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HookError::InvalidLine(line) => write!(f, "invalid ref line: {line}"),
            HookError::InvalidPayload(message) => write!(f, "invalid payload: {message}"),
            HookError::InvalidSignature(message) => write!(f, "invalid signature: {message}"),
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

pub fn run_hook_from_env(mode: HookMode) -> Result<(), HookServiceError> {
    let config =
        HookConfig::from_env_with_overrides(Some(mode), None, None).map_err(HookServiceError::Config)?;
    let stdin_file = env_path(ENV_HOOK_STDIN_FILE);
    run_hook(config, stdin_file.as_deref())
}

pub fn run_hook(config: HookConfig, stdin_file: Option<&Path>) -> Result<(), HookServiceError> {
    tracing::info!(mode = ?config.mode, "git hook configured");
    validate_input_source(std::io::stdin().is_terminal(), stdin_file)?;
    let input = read_input(stdin_file)?;
    let updates = match parse_updates(&input) {
        Ok(updates) => updates,
        Err(err) => {
            if matches!(config.mode, HookMode::PostReceive) {
                eprintln!("post-receive parse failed: {err}");
                return Ok(());
            }
            return Err(HookServiceError::Parse(err));
        }
    };
    let repo_path = std::env::var_os(ENV_HOOK_REPO_PATH)
        .or_else(|| std::env::var_os("GIT_DIR"))
        .map(PathBuf::from)
        .unwrap_or(std::env::current_dir().map_err(|err| {
            HookServiceError::Core(format!("failed to read repo path: {err}"))
        })?);
    match config.mode {
        HookMode::PreReceive => {
            let fetcher = HttpStateFetcher::new(config.state_url, Duration::from_secs(5))?;
            let decision = evaluate_pre_receive(&fetcher, repo_path, &updates)?;
            if let UpdateDecision::Reject { reason } = decision {
                return Err(HookServiceError::Reject(reason));
            }
        }
        HookMode::PostReceive => {
            let sync_url = config.sync_url.ok_or_else(|| {
                HookServiceError::Config(HookConfigError::MissingEnv(ENV_SYNC_URL))
            })?;
            let notifier = HttpPostReceiveNotifier::new(sync_url, Duration::from_secs(5))?;
            if let Err(err) = handle_post_receive(&notifier, repo_path, &updates) {
                eprintln!("post-receive notify failed: {err}");
            }
        }
    }
    Ok(())
}

fn env_path(key: &str) -> Option<PathBuf> {
    let value = std::env::var(key).ok()?;
    if value.trim().is_empty() {
        return None;
    }
    Some(PathBuf::from(value))
}

fn read_input(stdin_file: Option<&Path>) -> Result<String, HookServiceError> {
    if let Some(path) = stdin_file {
        std::fs::read_to_string(path).map_err(|err| {
            HookServiceError::Core(format!("failed to read stdin file {}: {err}", path.display()))
        })
    } else {
        let mut input = String::new();
        std::io::stdin()
            .read_to_string(&mut input)
            .map_err(|err| HookServiceError::Core(format!("failed to read stdin: {err}")))?;
        Ok(input)
    }
}

fn validate_input_source(
    stdin_is_tty: bool,
    stdin_file: Option<&Path>,
) -> Result<(), HookServiceError> {
    if stdin_is_tty && stdin_file.is_none() {
        return Err(HookServiceError::Core(
            "refusing to run interactively; provide --stdin-file or pipe hook input".to_string(),
        ));
    }
    Ok(())
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

pub fn verify_forgejo_signature(
    secret: &str,
    payload: &[u8],
    signature_header: &str,
) -> Result<(), HookError> {
    if secret.is_empty() {
        return Err(HookError::InvalidSignature(
            "missing webhook secret".to_string(),
        ));
    }
    let signature_header = signature_header.trim();
    if signature_header.is_empty() {
        return Err(HookError::InvalidSignature(
            "missing signature header".to_string(),
        ));
    }
    let signature = signature_header
        .strip_prefix("sha256=")
        .unwrap_or(signature_header);
    let provided = hex::decode(signature).map_err(|_| {
        HookError::InvalidSignature("invalid signature encoding".to_string())
    })?;

    let mut mac =
        hmac::Hmac::<sha2::Sha256>::new_from_slice(secret.as_bytes()).map_err(|_| {
            HookError::InvalidSignature("invalid signature secret".to_string())
        })?;
    mac.update(payload);
    mac.verify_slice(&provided).map_err(|_| {
        HookError::InvalidSignature("signature mismatch".to_string())
    })?;
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
    let core_updates: Vec<gittree_core::RefUpdate<'_>> = updates
        .iter()
        .map(|update| gittree_core::RefUpdate::new(&update.old, &update.new, &update.reference))
        .collect();
    let needs_state = core_updates
        .iter()
        .any(|update| !update.ref_name.starts_with("refs/nostr/"));
    let state = if needs_state {
        fetcher.latest_state(&repo.pubkey, &repo.identifier)?
    } else {
        None
    };
    Ok(gittree_core::evaluate_updates(
        &core_updates,
        state.as_ref(),
    ))
}

pub trait PostReceiveNotifier {
    fn notify(&self, payload: PostReceivePayload) -> Result<(), HookServiceError>;
}

pub trait MappingResolver {
    fn resolve_mapping(
        &self,
        owner: &str,
        repo: &str,
    ) -> Result<Option<RepoMapping>, HookServiceError>;
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

pub fn handle_forgejo_push<R, N>(
    resolver: &R,
    notifier: &N,
    payload: &str,
) -> Result<(), HookServiceError>
where
    R: MappingResolver,
    N: PostReceiveNotifier,
{
    let event = parse_forgejo_push(payload).map_err(HookServiceError::Parse)?;
    let mapping = resolver
        .resolve_mapping(&event.owner, &event.repo)?
        .ok_or_else(|| HookServiceError::Reject("missing repo mapping".to_string()))?;
    let payload = PostReceivePayload {
        pubkey: mapping.pubkey,
        identifier: mapping.identifier,
        updates: vec![RefUpdatePayload {
            old: event.before,
            new: event.after,
            reference: event.reference,
        }],
    };
    notifier.notify(payload)
}

#[cfg(test)]
mod tests {
    use super::read_input;
    use super::validate_input_source;
    use super::HookError;
    use super::MappingResolver;
    use super::PostReceiveNotifier;
    use super::PostReceivePayload;
    use super::StateFetcher;
    use super::evaluate_pre_receive;
    use super::handle_forgejo_push;
    use super::handle_post_receive;
    use super::parse_forgejo_push;
    use super::parse_updates;
    use super::verify_forgejo_signature;
    use hmac::Mac;
    use gittree_core::RepoMapping;
    use gittree_core::RepoState;
    use std::collections::HashMap;
    use std::io::Write;
    use std::sync::Mutex;

    #[test]
    fn read_input_reads_file() {
        let mut path = std::env::temp_dir();
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        path.push(format!("gittree-hook-input-{nanos}.txt"));
        let mut file = std::fs::File::create(&path).expect("create file");
        writeln!(file, "old new refs/heads/main").expect("write file");
        let contents = read_input(Some(&path)).expect("read input");
        assert!(contents.contains("refs/heads/main"));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn validate_input_source_rejects_tty_without_file() {
        assert!(validate_input_source(true, None).is_err());
    }

    #[test]
    fn validate_input_source_accepts_file_or_pipe() {
        assert!(validate_input_source(false, None).is_ok());
        assert!(validate_input_source(true, Some(std::path::Path::new("input.txt"))).is_ok());
    }

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

    #[test]
    fn verify_forgejo_signature_accepts_valid() {
        let secret = "secret";
        let payload = b"{\"ok\":true}";
        let mut mac =
            hmac::Hmac::<sha2::Sha256>::new_from_slice(secret.as_bytes()).expect("mac");
        mac.update(payload);
        let signature = hex::encode(mac.finalize().into_bytes());
        let header = format!("sha256={signature}");
        verify_forgejo_signature(secret, payload, &header).expect("valid signature");
    }

    #[test]
    fn verify_forgejo_signature_rejects_invalid() {
        let err = verify_forgejo_signature("secret", b"payload", "sha256=deadbeef").unwrap_err();
        assert!(matches!(err, HookError::InvalidSignature(_)));
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

    struct FailingFetcher;

    impl StateFetcher for FailingFetcher {
        fn latest_state(
            &self,
            _pubkey: &str,
            _identifier: &str,
        ) -> Result<Option<RepoState>, super::HookServiceError> {
            Err(super::HookServiceError::State(
                "state service unavailable".to_string(),
            ))
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

    #[test]
    fn evaluate_pre_receive_accepts_nostr_without_state() {
        let updates = vec![super::RefUpdate {
            old: "0".repeat(40),
            new: "1".repeat(40),
            reference: "refs/nostr/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                .to_string(),
        }];
        let repo_path = std::path::Path::new("/tmp")
            .join("npub1gjttreegkzys8jlhdnfm3qe39h2gka79cpndd0jsms5fk7tuhcnsdw56jq")
            .join("repo.git");
        let decision =
            evaluate_pre_receive(&FailingFetcher, repo_path, &updates).expect("decision");
        assert!(matches!(decision, gittree_core::UpdateDecision::Accept));
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

    struct MockResolver {
        mapping: Option<RepoMapping>,
    }

    impl MappingResolver for MockResolver {
        fn resolve_mapping(
            &self,
            _owner: &str,
            _repo: &str,
        ) -> Result<Option<RepoMapping>, super::HookServiceError> {
            Ok(self.mapping.clone())
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

    #[test]
    fn handle_forgejo_push_resolves_mapping() {
        let resolver = MockResolver {
            mapping: Some(
                RepoMapping::new("owner", "repo", "11".repeat(32), "repo").expect("mapping"),
            ),
        };
        let notifier = MockNotifier::new();
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
        handle_forgejo_push(&resolver, &notifier, payload).expect("handle");
        let payloads = notifier.payloads.lock().expect("payload lock");
        assert_eq!(payloads.len(), 1);
        assert_eq!(payloads[0].identifier, "repo");
        assert_eq!(payloads[0].updates[0].reference, "refs/heads/main");
    }

    #[test]
    fn handle_forgejo_push_rejects_missing_mapping() {
        let resolver = MockResolver { mapping: None };
        let notifier = MockNotifier::new();
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
        let err = handle_forgejo_push(&resolver, &notifier, payload).unwrap_err();
        assert!(matches!(err, super::HookServiceError::Reject(_)));
    }
}

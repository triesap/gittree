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
    let config = HookConfig::from_env_with_overrides(Some(mode), None, None)
        .map_err(HookServiceError::Config)?;
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
    let repo_path =
        std::env::var_os(ENV_HOOK_REPO_PATH)
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
            HookServiceError::Core(format!(
                "failed to read stdin file {}: {err}",
                path.display()
            ))
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
    ensure_non_empty(
        "repository.owner.username",
        &parsed.repository.owner.username,
    )?;

    let full_name = parsed.repository.full_name.unwrap_or_else(|| {
        format!(
            "{}/{}",
            parsed.repository.owner.username, parsed.repository.name
        )
    });

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
    let provided = hex::decode(signature)
        .map_err(|_| HookError::InvalidSignature("invalid signature encoding".to_string()))?;

    let mut mac = hmac::Hmac::<sha2::Sha256>::new_from_slice(secret.as_bytes())
        .map_err(|_| HookError::InvalidSignature("invalid signature secret".to_string()))?;
    mac.update(payload);
    mac.verify_slice(&provided)
        .map_err(|_| HookError::InvalidSignature("signature mismatch".to_string()))?;
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
    use super::HookConfigError;
    use super::HookError;
    use super::HookMode;
    use super::HookServiceError;
    use super::MappingResolver;
    use super::PostReceiveNotifier;
    use super::PostReceivePayload;
    use super::StateFetcher;
    use super::env_path;
    use super::evaluate_pre_receive;
    use super::handle_forgejo_push;
    use super::handle_post_receive;
    use super::parse_forgejo_push;
    use super::parse_updates;
    use super::read_input;
    use super::validate_input_source;
    use super::verify_forgejo_signature;
    use gittree_core::RepoMapping;
    use gittree_core::RepoState;
    use hmac::Mac;
    use std::collections::HashMap;
    use std::error::Error;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::{Mutex, OnceLock};
    use std::time::{SystemTime, UNIX_EPOCH};

    const SAMPLE_NPUB: &str = "npub1gjttreegkzys8jlhdnfm3qe39h2gka79cpndd0jsms5fk7tuhcnsdw56jq";

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    fn with_env_var(key: &str, value: Option<&str>, run: impl FnOnce()) {
        let _guard = env_lock().lock().expect("env lock");
        let previous = std::env::var(key).ok();

        match value {
            Some(value) => {
                // SAFETY: tests serialize environment mutation with a process-wide mutex.
                unsafe { std::env::set_var(key, value) };
            }
            None => {
                // SAFETY: tests serialize environment mutation with a process-wide mutex.
                unsafe { std::env::remove_var(key) };
            }
        }

        run();

        match previous {
            Some(value) => {
                // SAFETY: tests serialize environment mutation with a process-wide mutex.
                unsafe { std::env::set_var(key, value) };
            }
            None => {
                // SAFETY: tests serialize environment mutation with a process-wide mutex.
                unsafe { std::env::remove_var(key) };
            }
        }
    }

    fn with_env_vars(vars: &[(&str, Option<&str>)], run: impl FnOnce()) {
        let _guard = env_lock().lock().expect("env lock");
        let previous: Vec<(&str, Option<std::ffi::OsString>)> = vars
            .iter()
            .map(|(key, _)| (*key, std::env::var_os(key)))
            .collect();

        for (key, value) in vars {
            match value {
                Some(value) => {
                    // SAFETY: tests serialize environment mutation with a process-wide mutex.
                    unsafe { std::env::set_var(key, value) };
                }
                None => {
                    // SAFETY: tests serialize environment mutation with a process-wide mutex.
                    unsafe { std::env::remove_var(key) };
                }
            }
        }

        run();

        for (key, previous) in previous {
            match previous {
                Some(value) => {
                    // SAFETY: tests serialize environment mutation with a process-wide mutex.
                    unsafe { std::env::set_var(key, value) };
                }
                None => {
                    // SAFETY: tests serialize environment mutation with a process-wide mutex.
                    unsafe { std::env::remove_var(key) };
                }
            }
        }
    }

    fn write_updates_file(contents: &str) -> std::path::PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("gittree-hook-updates-{nanos}.txt"));
        std::fs::write(&path, contents).expect("write updates file");
        path
    }

    fn start_mock_http_server(
        status: &str,
        content_type: &str,
        body: &str,
    ) -> (String, std::thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind test server");
        let addr = listener.local_addr().expect("server addr");
        let status = status.to_string();
        let content_type = content_type.to_string();
        let body = body.to_string();
        let handle = std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut request = [0u8; 1024];
                let _ = stream.read(&mut request);
                let response = format!(
                    "HTTP/1.1 {status}\r\ncontent-type: {content_type}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes());
                let _ = stream.flush();
            }
        });
        (format!("http://{addr}"), handle)
    }

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
    fn hook_mode_from_env_defaults_to_pre_receive() {
        with_env_var(super::ENV_HOOK_MODE, None, || {
            let mode = HookMode::from_env().expect("mode");
            assert_eq!(mode, HookMode::PreReceive);
        });
    }

    #[test]
    fn hook_mode_from_env_accepts_post_receive() {
        with_env_var(super::ENV_HOOK_MODE, Some("post-receive"), || {
            let mode = HookMode::from_env().expect("mode");
            assert_eq!(mode, HookMode::PostReceive);
        });
    }

    #[test]
    fn hook_mode_from_env_rejects_unknown_mode() {
        with_env_var(super::ENV_HOOK_MODE, Some("bad-mode"), || {
            let err = HookMode::from_env().expect_err("invalid mode");
            assert!(matches!(err, HookConfigError::InvalidMode(_)));
        });
    }

    #[test]
    fn env_path_ignores_empty_values() {
        with_env_var("GITTREE_TEST_PATH", Some("   "), || {
            assert!(env_path("GITTREE_TEST_PATH").is_none());
        });
        with_env_var("GITTREE_TEST_PATH", Some("/tmp/input.txt"), || {
            assert_eq!(
                env_path("GITTREE_TEST_PATH"),
                Some(std::path::PathBuf::from("/tmp/input.txt"))
            );
        });
    }

    #[test]
    fn read_input_reports_missing_file_errors() {
        let missing = std::path::Path::new("/tmp/does-not-exist-gittree-hook.txt");
        let err = read_input(Some(missing)).expect_err("read should fail");
        assert!(matches!(err, HookServiceError::Core(_)));
    }

    #[test]
    fn hook_config_and_service_errors_display_and_source() {
        let missing_env = HookConfigError::MissingEnv("KEY");
        assert_eq!(missing_env.to_string(), "missing env KEY");
        assert!(missing_env.source().is_none());

        let invalid_mode = HookConfigError::InvalidMode("bad".to_string());
        assert_eq!(invalid_mode.to_string(), "invalid hook mode: bad");
        assert!(invalid_mode.source().is_none());

        let config_wrapped =
            HookConfigError::Config(gittree_config::ConfigError::MissingEnv("STATE"));
        assert!(config_wrapped.to_string().contains("hook config error:"));
        assert!(config_wrapped.source().is_some());

        let config_service = HookServiceError::Config(HookConfigError::MissingEnv("STATE"));
        assert_eq!(
            config_service.to_string(),
            "hook config error: missing env STATE"
        );
        assert!(config_service.source().is_some());

        let parse_service = HookServiceError::Parse(HookError::InvalidPayload("bad".to_string()));
        assert_eq!(
            parse_service.to_string(),
            "hook parse error: invalid payload: bad"
        );
        assert!(parse_service.source().is_some());

        let core_service = HookServiceError::Core("boom".to_string());
        assert_eq!(core_service.to_string(), "hook core error: boom");
        assert!(core_service.source().is_none());

        let state_service = HookServiceError::State("down".to_string());
        assert_eq!(state_service.to_string(), "hook state error: down");
        assert!(state_service.source().is_none());

        let reject_service = HookServiceError::Reject("nope".to_string());
        assert_eq!(reject_service.to_string(), "nope");
        assert!(reject_service.source().is_none());
    }

    #[test]
    fn with_env_var_restores_existing_values() {
        // SAFETY: dedicated test key avoids collisions with non-test code.
        unsafe { std::env::set_var("GITTREE_TEST_RESTORE", "before") };
        with_env_var("GITTREE_TEST_RESTORE", Some("after"), || {
            assert_eq!(
                std::env::var("GITTREE_TEST_RESTORE").ok().as_deref(),
                Some("after")
            );
        });
        assert_eq!(
            std::env::var("GITTREE_TEST_RESTORE").ok().as_deref(),
            Some("before")
        );
        // SAFETY: dedicated test key cleanup.
        unsafe { std::env::remove_var("GITTREE_TEST_RESTORE") };
    }

    #[test]
    fn hook_config_from_env_uses_defaults_and_overrides() {
        with_env_vars(
            &[
                (super::ENV_STATE_URL, Some("http://127.0.0.1:8082")),
                (super::ENV_HOOK_MODE, Some("pre-receive")),
                (super::ENV_SYNC_URL, None),
            ],
            || {
                let config = super::HookConfig::from_env().expect("config");
                assert_eq!(config.state_url, "http://127.0.0.1:8082");
                assert_eq!(config.mode, HookMode::PreReceive);
                assert!(config.sync_url.is_none());
            },
        );
    }

    #[test]
    fn hook_config_post_receive_requires_sync_url() {
        with_env_vars(
            &[
                (super::ENV_STATE_URL, Some("http://127.0.0.1:8082")),
                (super::ENV_HOOK_MODE, Some("post-receive")),
                (super::ENV_SYNC_URL, None),
            ],
            || {
                let err = super::HookConfig::from_env().expect_err("missing sync url");
                assert!(matches!(
                    err,
                    HookConfigError::MissingEnv(super::ENV_SYNC_URL)
                ));
            },
        );
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
    fn parse_updates_ignores_blank_lines() {
        let input = "\n\nold new refs/heads/main\n  \n";
        let updates = parse_updates(input).expect("updates");
        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0].reference, "refs/heads/main");
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
    fn parse_forgejo_push_derives_full_name_when_absent() {
        let payload = r#"
        {
            "ref": "refs/heads/main",
            "before": "0000000000000000000000000000000000000000",
            "after": "1111111111111111111111111111111111111111",
            "repository": {
                "name": "repo",
                "owner": { "username": "owner" }
            }
        }
        "#;
        let event = parse_forgejo_push(payload).expect("event");
        assert_eq!(event.full_name, "owner/repo");
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
    fn parse_forgejo_push_rejects_invalid_full_name() {
        let payload = r#"
        {
            "ref": "refs/heads/main",
            "before": "0000000000000000000000000000000000000000",
            "after": "1111111111111111111111111111111111111111",
            "repository": {
                "name": "repo",
                "full_name": "ownerrepo",
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
        let mut mac = hmac::Hmac::<sha2::Sha256>::new_from_slice(secret.as_bytes()).expect("mac");
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

    #[test]
    fn verify_forgejo_signature_rejects_missing_secret() {
        let err = verify_forgejo_signature("", b"payload", "sha256=deadbeef").unwrap_err();
        assert!(matches!(err, HookError::InvalidSignature(_)));
    }

    #[test]
    fn verify_forgejo_signature_rejects_missing_signature_header() {
        let err = verify_forgejo_signature("secret", b"payload", "   ").unwrap_err();
        assert!(matches!(err, HookError::InvalidSignature(_)));
    }

    #[test]
    fn verify_forgejo_signature_rejects_invalid_encoding() {
        let err = verify_forgejo_signature("secret", b"payload", "sha256=zz").unwrap_err();
        assert!(matches!(err, HookError::InvalidSignature(_)));
    }

    #[test]
    fn hook_error_display_messages_are_stable() {
        assert_eq!(
            HookError::InvalidLine("line".to_string()).to_string(),
            "invalid ref line: line"
        );
        assert_eq!(
            HookError::InvalidPayload("payload".to_string()).to_string(),
            "invalid payload: payload"
        );
        assert_eq!(
            HookError::InvalidSignature("signature".to_string()).to_string(),
            "invalid signature: signature"
        );
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
            reference:
                "refs/nostr/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                    .to_string(),
        }];
        let repo_path = std::path::Path::new("/tmp")
            .join("npub1gjttreegkzys8jlhdnfm3qe39h2gka79cpndd0jsms5fk7tuhcnsdw56jq")
            .join("repo.git");
        let decision =
            evaluate_pre_receive(&FailingFetcher, repo_path, &updates).expect("decision");
        assert!(matches!(decision, gittree_core::UpdateDecision::Accept));
    }

    #[test]
    fn evaluate_pre_receive_propagates_fetch_errors_for_non_nostr_updates() {
        let updates = vec![super::RefUpdate {
            old: "0".repeat(40),
            new: "1".repeat(40),
            reference: "refs/heads/main".to_string(),
        }];
        let repo_path = std::path::Path::new("/tmp")
            .join("npub1gjttreegkzys8jlhdnfm3qe39h2gka79cpndd0jsms5fk7tuhcnsdw56jq")
            .join("repo.git");
        let err = evaluate_pre_receive(&FailingFetcher, repo_path, &updates).unwrap_err();
        assert!(matches!(err, super::HookServiceError::State(_)));
    }

    #[test]
    fn run_hook_from_env_pre_receive_accepts_nostr_updates() {
        let updates = format!(
            "{} {} refs/nostr/{}\n",
            "0".repeat(40),
            "1".repeat(40),
            "a".repeat(64)
        );
        let updates_path = write_updates_file(&updates);
        let repo_path = std::path::Path::new("/tmp")
            .join(SAMPLE_NPUB)
            .join("repo.git");
        with_env_vars(
            &[
                (super::ENV_STATE_URL, Some("http://127.0.0.1:8082")),
                (
                    super::ENV_HOOK_REPO_PATH,
                    Some(repo_path.to_str().expect("repo path")),
                ),
                (
                    super::ENV_HOOK_STDIN_FILE,
                    Some(updates_path.to_str().expect("updates path")),
                ),
            ],
            || {
                super::run_hook_from_env(HookMode::PreReceive).expect("run hook");
            },
        );
        let _ = std::fs::remove_file(updates_path);
    }

    #[test]
    fn run_hook_post_receive_requires_sync_url() {
        let updates = format!("{} {} refs/heads/main\n", "0".repeat(40), "1".repeat(40));
        let updates_path = write_updates_file(&updates);
        let repo_path = std::path::Path::new("/tmp")
            .join(SAMPLE_NPUB)
            .join("repo.git");
        let config = super::HookConfig {
            state_url: "http://127.0.0.1:8082".to_string(),
            sync_url: None,
            mode: HookMode::PostReceive,
        };
        with_env_vars(
            &[(
                super::ENV_HOOK_REPO_PATH,
                Some(repo_path.to_str().expect("repo path")),
            )],
            || {
                let err = super::run_hook(config, Some(&updates_path)).expect_err("missing sync");
                assert!(matches!(
                    err,
                    HookServiceError::Config(HookConfigError::MissingEnv(super::ENV_SYNC_URL))
                ));
            },
        );
        let _ = std::fs::remove_file(updates_path);
    }

    #[test]
    fn run_hook_post_receive_ignores_notifier_errors() {
        let updates = format!("{} {} refs/heads/main\n", "0".repeat(40), "1".repeat(40));
        let updates_path = write_updates_file(&updates);
        let repo_path = std::path::Path::new("/tmp")
            .join(SAMPLE_NPUB)
            .join("repo.git");
        let (sync_url, handle) =
            start_mock_http_server("500 Internal Server Error", "text/plain", "nope");
        let config = super::HookConfig {
            state_url: "http://127.0.0.1:8082".to_string(),
            sync_url: Some(sync_url),
            mode: HookMode::PostReceive,
        };
        with_env_vars(
            &[(
                super::ENV_HOOK_REPO_PATH,
                Some(repo_path.to_str().expect("repo path")),
            )],
            || {
                super::run_hook(config, Some(&updates_path)).expect("run hook");
            },
        );
        handle.join().expect("server join");
        let _ = std::fs::remove_file(updates_path);
    }

    #[test]
    fn run_hook_uses_current_dir_when_repo_env_missing() {
        let updates = format!(
            "{} {} refs/nostr/{}\n",
            "0".repeat(40),
            "1".repeat(40),
            "a".repeat(64)
        );
        let updates_path = write_updates_file(&updates);
        let config = super::HookConfig {
            state_url: "http://127.0.0.1:8082".to_string(),
            sync_url: None,
            mode: HookMode::PreReceive,
        };
        with_env_vars(
            &[(super::ENV_HOOK_REPO_PATH, None), ("GIT_DIR", None)],
            || {
                let err = super::run_hook(config, Some(&updates_path)).expect_err("invalid cwd");
                assert!(matches!(err, HookServiceError::Core(_)));
            },
        );
        let _ = std::fs::remove_file(updates_path);
    }

    #[test]
    fn http_state_fetcher_returns_none_for_not_found() {
        let (base_url, handle) = start_mock_http_server("404 Not Found", "text/plain", "missing");
        let fetcher = super::HttpStateFetcher::new(base_url, std::time::Duration::from_secs(1))
            .expect("fetcher");
        let state = fetcher
            .latest_state("11".repeat(32).as_str(), "repo")
            .expect("state fetch");
        assert!(state.is_none());
        handle.join().expect("server join");
    }

    #[test]
    fn http_state_fetcher_returns_error_on_non_success() {
        let (base_url, handle) =
            start_mock_http_server("500 Internal Server Error", "text/plain", "boom");
        let fetcher = super::HttpStateFetcher::new(base_url, std::time::Duration::from_secs(1))
            .expect("fetcher");
        let err = fetcher
            .latest_state("11".repeat(32).as_str(), "repo")
            .expect_err("state should fail");
        assert!(matches!(err, super::HookServiceError::State(_)));
        handle.join().expect("server join");
    }

    #[test]
    fn http_state_fetcher_parses_success_response() {
        let body = format!(
            "{{\"identifier\":\"repo\",\"state\":{{\"HEAD\":\"ref: refs/heads/main\",\"refs/heads/main\":\"{}\"}}}}",
            "11".repeat(20)
        );
        let (base_url, handle) = start_mock_http_server("200 OK", "application/json", &body);
        let fetcher = super::HttpStateFetcher::new(base_url, std::time::Duration::from_secs(1))
            .expect("fetcher");
        let state = fetcher
            .latest_state("11".repeat(32).as_str(), "repo")
            .expect("state fetch")
            .expect("state payload");
        assert_eq!(state.identifier, "repo");
        assert_eq!(state.state.get("refs/heads/main"), Some(&"11".repeat(20)));
        handle.join().expect("server join");
    }

    #[test]
    fn http_post_receive_notifier_reports_error_status() {
        let (endpoint, handle) =
            start_mock_http_server("500 Internal Server Error", "text/plain", "nope");
        let notifier =
            super::HttpPostReceiveNotifier::new(endpoint, std::time::Duration::from_secs(1))
                .expect("notifier");
        let payload = PostReceivePayload {
            pubkey: "11".repeat(32),
            identifier: "repo".to_string(),
            updates: vec![super::RefUpdatePayload {
                old: "0".repeat(40),
                new: "1".repeat(40),
                reference: "refs/heads/main".to_string(),
            }],
        };
        let err = notifier.notify(payload).expect_err("notify should fail");
        assert!(matches!(err, super::HookServiceError::State(_)));
        handle.join().expect("server join");
    }

    #[test]
    fn http_post_receive_notifier_accepts_success_status() {
        let (endpoint, handle) = start_mock_http_server("200 OK", "application/json", "{}");
        let notifier =
            super::HttpPostReceiveNotifier::new(endpoint, std::time::Duration::from_secs(1))
                .expect("notifier");
        let payload = PostReceivePayload {
            pubkey: "11".repeat(32),
            identifier: "repo".to_string(),
            updates: vec![super::RefUpdatePayload {
                old: "0".repeat(40),
                new: "1".repeat(40),
                reference: "refs/heads/main".to_string(),
            }],
        };
        notifier.notify(payload).expect("notify");
        handle.join().expect("server join");
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

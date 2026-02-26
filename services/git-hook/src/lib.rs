use gittree_config::{ConfigError, ServicesConfig};
use gittree_core::{RepoMapping, RepoState, UpdateDecision};
use hmac::Mac;
use serde::{Deserialize, Serialize};
use std::io::IsTerminal;
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
        let _services = match ServicesConfig::from_env_validated() {
            Ok(services) => services,
            Err(err) => return Err(HookConfigError::Config(err)),
        };
        let state_url = if let Some(state_url) = state_url {
            state_url
        } else if let Ok(env_state_url) = std::env::var(ENV_STATE_URL) {
            env_state_url
        } else {
            return Err(HookConfigError::MissingEnv(ENV_STATE_URL));
        };
        let mode = mode.unwrap_or(HookMode::from_env()?);
        let sync_url = if let Some(sync_url) = sync_url {
            Some(sync_url)
        } else {
            std::env::var(ENV_SYNC_URL).ok()
        };
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
        let mode = match std::env::var(ENV_HOOK_MODE) {
            Ok(mode) => mode,
            Err(_) => "pre-receive".to_string(),
        };
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
    let config = match HookConfig::from_env_with_overrides(Some(mode), None, None) {
        Ok(config) => config,
        Err(err) => return Err(HookServiceError::Config(err)),
    };
    let stdin_file = env_path(ENV_HOOK_STDIN_FILE);
    run_hook(config, stdin_file.as_deref())
}

pub fn run_hook(config: HookConfig, stdin_file: Option<&Path>) -> Result<(), HookServiceError> {
    run_hook_with_terminal(config, stdin_file, std::io::stdin().is_terminal())
}

fn run_hook_with_terminal(
    config: HookConfig,
    stdin_file: Option<&Path>,
    stdin_is_terminal: bool,
) -> Result<(), HookServiceError> {
    validate_input_source(stdin_is_terminal, stdin_file)?;
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
    let repo_path = if let Some(path) = std::env::var_os(ENV_HOOK_REPO_PATH) {
        PathBuf::from(path)
    } else if let Some(path) = std::env::var_os("GIT_DIR") {
        PathBuf::from(path)
    } else {
        match std::env::current_dir() {
            Ok(path) => path,
            Err(err) => {
                return Err(HookServiceError::Core(format!(
                    "failed to read repo path: {err}"
                )));
            }
        }
    };
    match config.mode {
        HookMode::PreReceive => {
            let fetcher = HttpStateFetcher::new(config.state_url, Duration::from_secs(5));
            let decision = evaluate_pre_receive(&fetcher, &repo_path, &updates)?;
            if let UpdateDecision::Reject { reason } = decision {
                return Err(HookServiceError::Reject(reason));
            }
        }
        HookMode::PostReceive => {
            let sync_url = if let Some(sync_url) = config.sync_url {
                sync_url
            } else {
                return Err(HookServiceError::Config(HookConfigError::MissingEnv(
                    ENV_SYNC_URL,
                )));
            };
            let notifier = HttpPostReceiveNotifier::new(sync_url, Duration::from_secs(5));
            if handle_post_receive(&notifier, &repo_path, &updates).is_err() {}
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
        match std::fs::read_to_string(path) {
            Ok(value) => Ok(value),
            Err(err) => Err(HookServiceError::Core(format!(
                "failed to read stdin file {}: {err}",
                path.display()
            ))),
        }
    } else {
        let mut stdin = std::io::stdin();
        read_from_reader(&mut stdin)
    }
}

fn read_from_reader(reader: &mut dyn std::io::Read) -> Result<String, HookServiceError> {
    let mut input = String::new();
    match reader.read_to_string(&mut input) {
        Ok(_) => Ok(input),
        Err(err) => Err(HookServiceError::Core(format!(
            "failed to read stdin: {err}"
        ))),
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
            .expect("trimmed non-empty hook line must contain at least one token");
        let new = if let Some(new) = parts.next() {
            new
        } else {
            return Err(HookError::InvalidLine(line.to_string()));
        };
        let reference = if let Some(reference) = parts.next() {
            reference
        } else {
            return Err(HookError::InvalidLine(line.to_string()));
        };
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
    let parsed: ForgejoPushPayload = match serde_json::from_str(payload) {
        Ok(parsed) => parsed,
        Err(err) => return Err(HookError::InvalidPayload(err.to_string())),
    };

    ensure_non_empty("ref", &parsed.reference)?;
    ensure_non_empty("before", &parsed.before)?;
    ensure_non_empty("after", &parsed.after)?;
    ensure_non_empty("repository.name", &parsed.repository.name)?;
    ensure_non_empty(
        "repository.owner.username",
        &parsed.repository.owner.username,
    )?;

    let full_name = if let Some(full_name) = parsed.repository.full_name {
        full_name
    } else {
        format!(
            "{}/{}",
            parsed.repository.owner.username, parsed.repository.name
        )
    };

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
    let provided = match hex::decode(signature) {
        Ok(decoded) => decoded,
        Err(_) => {
            return Err(HookError::InvalidSignature(
                "invalid signature encoding".to_string(),
            ));
        }
    };

    // HMAC accepts arbitrary key lengths for SHA-256; this constructor is effectively infallible here.
    let mut mac = hmac::Hmac::<sha2::Sha256>::new_from_slice(secret.as_bytes())
        .expect("hmac sha256 key init should be infallible");
    mac.update(payload);
    if mac.verify_slice(&provided).is_err() {
        return Err(HookError::InvalidSignature(
            "signature mismatch".to_string(),
        ));
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
    timeout: Duration,
}

impl HttpStateFetcher {
    pub fn new(base_url: impl Into<String>, timeout: Duration) -> Self {
        Self {
            base_url: base_url.into(),
            client: reqwest::blocking::Client::new(),
            timeout,
        }
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
        let response = match self.client.get(url).timeout(self.timeout).send() {
            Ok(response) => response,
            Err(err) => return Err(HookServiceError::State(err.to_string())),
        };

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

        let state = match response.json::<StateResponse>() {
            Ok(state) => state,
            Err(err) => return Err(HookServiceError::State(err.to_string())),
        };

        Ok(Some(RepoState {
            identifier: state.identifier,
            state: state.state,
        }))
    }
}

pub fn evaluate_pre_receive(
    fetcher: &dyn StateFetcher,
    repo_path: &Path,
    updates: &[RefUpdate],
) -> Result<UpdateDecision, HookServiceError> {
    let repo = match gittree_core::parse_repo_path(repo_path) {
        Ok(repo) => repo,
        Err(err) => return Err(HookServiceError::Core(err.to_string())),
    };
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
    timeout: Duration,
}

impl HttpPostReceiveNotifier {
    pub fn new(endpoint: impl Into<String>, timeout: Duration) -> Self {
        Self {
            endpoint: endpoint.into(),
            client: reqwest::blocking::Client::new(),
            timeout,
        }
    }
}

impl PostReceiveNotifier for HttpPostReceiveNotifier {
    fn notify(&self, payload: PostReceivePayload) -> Result<(), HookServiceError> {
        let response = match self
            .client
            .post(&self.endpoint)
            .timeout(self.timeout)
            .json(&payload)
            .send()
        {
            Ok(response) => response,
            Err(err) => return Err(HookServiceError::State(err.to_string())),
        };

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

pub fn handle_post_receive(
    notifier: &dyn PostReceiveNotifier,
    repo_path: &Path,
    updates: &[RefUpdate],
) -> Result<(), HookServiceError> {
    let repo = match gittree_core::parse_repo_path(repo_path) {
        Ok(repo) => repo,
        Err(err) => return Err(HookServiceError::Core(err.to_string())),
    };
    let payload = PostReceivePayload {
        pubkey: repo.pubkey,
        identifier: repo.identifier,
        updates: updates.iter().map(RefUpdatePayload::from).collect(),
    };
    notifier.notify(payload)
}

pub fn handle_forgejo_push(
    resolver: &dyn MappingResolver,
    notifier: &dyn PostReceiveNotifier,
    payload: &str,
) -> Result<(), HookServiceError> {
    let event = match parse_forgejo_push(payload) {
        Ok(event) => event,
        Err(err) => return Err(HookServiceError::Parse(err)),
    };
    let mapping = if let Some(mapping) = resolver.resolve_mapping(&event.owner, &event.repo)? {
        mapping
    } else {
        return Err(HookServiceError::Reject("missing repo mapping".to_string()));
    };
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
    use super::env_path;
    use super::evaluate_pre_receive;
    use super::handle_forgejo_push;
    use super::handle_post_receive;
    use super::parse_forgejo_push;
    use super::parse_updates;
    use super::read_from_reader;
    use super::read_input;
    use super::validate_input_source;
    use super::verify_forgejo_signature;
    use super::ConfigError;
    use super::HookConfigError;
    use super::HookError;
    use super::HookMode;
    use super::HookServiceError;
    use super::MappingResolver;
    use super::PostReceiveNotifier;
    use super::PostReceivePayload;
    use super::StateFetcher;
    use gittree_core::RepoMapping;
    use gittree_core::RepoState;
    use hmac::Mac;
    use std::collections::HashMap;
    use std::error::Error;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Mutex, OnceLock};
    use std::time::{SystemTime, UNIX_EPOCH};

    const SAMPLE_NPUB: &str = "npub1gjttreegkzys8jlhdnfm3qe39h2gka79cpndd0jsms5fk7tuhcnsdw56jq";

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    fn hook_config_error_kind(err: &HookConfigError) -> (&'static str, Option<&'static str>) {
        match err {
            HookConfigError::MissingEnv(key) => ("missing_env", Some(*key)),
            HookConfigError::InvalidMode(_) => ("invalid_mode", None),
            HookConfigError::Config(_) => ("config", None),
        }
    }

    fn hook_service_error_kind(err: &HookServiceError) -> &'static str {
        match err {
            HookServiceError::Config(_) => "config",
            HookServiceError::Parse(_) => "parse",
            HookServiceError::Core(_) => "core",
            HookServiceError::State(_) => "state",
            HookServiceError::Reject(_) => "reject",
        }
    }

    fn hook_error_kind(err: &HookError) -> &'static str {
        match err {
            HookError::InvalidLine(_) => "invalid_line",
            HookError::InvalidPayload(_) => "invalid_payload",
            HookError::InvalidSignature(_) => "invalid_signature",
        }
    }

    fn update_decision_kind(decision: &gittree_core::UpdateDecision) -> &'static str {
        match decision {
            gittree_core::UpdateDecision::Accept => "accept",
            gittree_core::UpdateDecision::Reject { .. } => "reject",
        }
    }

    fn with_env_var(key: &str, value: Option<&str>, run: &mut dyn FnMut()) {
        let _guard = env_lock()
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
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

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run()));

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

        if let Err(payload) = result {
            std::panic::resume_unwind(payload);
        }
    }

    fn with_env_vars(vars: &[(&str, Option<&str>)], run: &mut dyn FnMut()) {
        let _guard = env_lock()
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
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

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run()));

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

        if let Err(payload) = result {
            std::panic::resume_unwind(payload);
        }
    }

    fn write_updates_file(contents: &str) -> std::path::PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        let suffix = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("gittree-hook-updates-{nanos}-{suffix}.txt"));
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
            let (mut stream, _) = listener.accept().expect("accept");
            let mut request = [0u8; 1024];
            let _ = stream.read(&mut request);
            let response = format!(
                "HTTP/1.1 {status}\r\ncontent-type: {content_type}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(response.as_bytes());
            let _ = stream.flush();
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
        with_env_var(super::ENV_HOOK_MODE, None, &mut || {
            let mode = HookMode::from_env().expect("mode");
            assert_eq!(mode, HookMode::PreReceive);
        });
    }

    #[test]
    fn hook_mode_from_env_accepts_post_receive() {
        with_env_var(super::ENV_HOOK_MODE, Some("post-receive"), &mut || {
            let mode = HookMode::from_env().expect("mode");
            assert_eq!(mode, HookMode::PostReceive);
        });
    }

    #[test]
    fn hook_mode_from_env_rejects_unknown_mode() {
        with_env_var(super::ENV_HOOK_MODE, Some("bad-mode"), &mut || {
            let err = HookMode::from_env().expect_err("invalid mode");
            assert_eq!(hook_config_error_kind(&err), ("invalid_mode", None));
        });
    }

    #[test]
    fn hook_config_error_kind_handles_config_variant() {
        let err = HookConfigError::Config(ConfigError::MissingEnv("MISSING"));
        assert_eq!(hook_config_error_kind(&err), ("config", None));
    }

    #[test]
    fn env_path_ignores_empty_values() {
        with_env_var("GITTREE_TEST_PATH", Some("   "), &mut || {
            assert!(env_path("GITTREE_TEST_PATH").is_none());
        });
        with_env_var("GITTREE_TEST_PATH", Some("/tmp/input.txt"), &mut || {
            assert_eq!(
                env_path("GITTREE_TEST_PATH"),
                Some(std::path::PathBuf::from("/tmp/input.txt"))
            );
        });
    }

    #[test]
    fn env_path_returns_none_for_missing_value() {
        with_env_var("GITTREE_TEST_PATH", None, &mut || {
            assert!(env_path("GITTREE_TEST_PATH").is_none());
        });
    }

    #[test]
    fn read_input_reports_missing_file_errors() {
        let missing = std::path::Path::new("/tmp/does-not-exist-gittree-hook.txt");
        let err = read_input(Some(missing)).expect_err("read should fail");
        assert_eq!(hook_service_error_kind(&err), "core");
    }

    struct FailingReader;

    impl Read for FailingReader {
        fn read(&mut self, _buf: &mut [u8]) -> std::io::Result<usize> {
            Err(std::io::Error::other("boom"))
        }
    }

    #[test]
    fn read_from_reader_reports_stdin_io_errors() {
        let mut reader = FailingReader;
        let err = read_from_reader(&mut reader).expect_err("read should fail");
        assert_eq!(hook_service_error_kind(&err), "core");
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
        with_env_var("GITTREE_TEST_RESTORE", Some("after"), &mut || {
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
    fn with_env_var_recovers_from_poisoned_lock() {
        let _ = std::panic::catch_unwind(|| {
            let _guard = env_lock().lock().expect("lock");
            panic!("poison env lock");
        });

        with_env_var("GITTREE_TEST_POISONED_LOCK", Some("after"), &mut || {
            assert_eq!(
                std::env::var("GITTREE_TEST_POISONED_LOCK").ok().as_deref(),
                Some("after")
            );
        });

        // SAFETY: dedicated test key cleanup.
        unsafe { std::env::remove_var("GITTREE_TEST_POISONED_LOCK") };
    }

    #[test]
    fn with_env_vars_restores_existing_values() {
        // SAFETY: dedicated test key avoids collisions with non-test code.
        unsafe { std::env::set_var("GITTREE_TEST_RESTORE_VARS", "before") };
        with_env_vars(&[("GITTREE_TEST_RESTORE_VARS", Some("after"))], &mut || {
            assert_eq!(
                std::env::var("GITTREE_TEST_RESTORE_VARS").ok().as_deref(),
                Some("after")
            );
        });
        assert_eq!(
            std::env::var("GITTREE_TEST_RESTORE_VARS").ok().as_deref(),
            Some("before")
        );
        // SAFETY: dedicated test key cleanup.
        unsafe { std::env::remove_var("GITTREE_TEST_RESTORE_VARS") };
    }

    #[test]
    #[should_panic(expected = "with_env_var panic path")]
    fn with_env_var_resumes_panics() {
        with_env_var("GITTREE_TEST_RESTORE_PANIC", Some("during"), &mut || {
            panic!("with_env_var panic path");
        });
    }

    #[test]
    #[should_panic(expected = "with_env_vars panic path")]
    fn with_env_vars_resumes_panics() {
        with_env_vars(
            &[("GITTREE_TEST_RESTORE_VARS_PANIC", Some("during"))],
            &mut || {
                panic!("with_env_vars panic path");
            },
        );
    }

    #[test]
    fn hook_config_from_env_uses_defaults_and_overrides() {
        with_env_vars(
            &[
                (super::ENV_STATE_URL, Some("http://127.0.0.1:8082")),
                (super::ENV_HOOK_MODE, Some("pre-receive")),
                (super::ENV_SYNC_URL, None),
            ],
            &mut || {
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
            &mut || {
                let err = super::HookConfig::from_env().expect_err("missing sync url");
                assert_eq!(
                    hook_config_error_kind(&err),
                    ("missing_env", Some(super::ENV_SYNC_URL))
                );
            },
        );
    }

    #[test]
    fn hook_config_reports_invalid_env_mode() {
        with_env_vars(&[(super::ENV_HOOK_MODE, Some("invalid-mode"))], &mut || {
            let err = super::HookConfig::from_env_with_overrides(
                None,
                Some("http://127.0.0.1:8082".to_string()),
                None,
            )
            .expect_err("invalid mode should fail");
            assert_eq!(hook_config_error_kind(&err).0, "invalid_mode");
        });
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
        assert_eq!(hook_error_kind(&err), "invalid_line");
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
        assert_eq!(hook_error_kind(&err), "invalid_payload");
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
        assert_eq!(hook_error_kind(&err), "invalid_payload");
    }

    #[test]
    fn parse_forgejo_push_rejects_invalid_json() {
        let err = parse_forgejo_push("{not-json}").expect_err("invalid payload");
        assert_eq!(hook_error_kind(&err), "invalid_payload");
    }

    #[test]
    fn parse_forgejo_push_rejects_empty_required_fields() {
        let mut payload = serde_json::json!({
            "ref": "refs/heads/main",
            "before": "0".repeat(40),
            "after": "1".repeat(40),
            "repository": {
                "name": "repo",
                "owner": { "username": "owner" }
            }
        });

        payload["before"] = serde_json::Value::String(String::new());
        let before_err = parse_forgejo_push(&payload.to_string()).expect_err("empty before");
        assert_eq!(hook_error_kind(&before_err), "invalid_payload");

        payload["before"] = serde_json::Value::String("0".repeat(40));
        payload["after"] = serde_json::Value::String(String::new());
        let after_err = parse_forgejo_push(&payload.to_string()).expect_err("empty after");
        assert_eq!(hook_error_kind(&after_err), "invalid_payload");

        payload["after"] = serde_json::Value::String("1".repeat(40));
        payload["repository"]["name"] = serde_json::Value::String(String::new());
        let name_err = parse_forgejo_push(&payload.to_string()).expect_err("empty repo name");
        assert_eq!(hook_error_kind(&name_err), "invalid_payload");
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
        assert_eq!(hook_error_kind(&err), "invalid_signature");
    }

    #[test]
    fn verify_forgejo_signature_rejects_missing_secret() {
        let err = verify_forgejo_signature("", b"payload", "sha256=deadbeef").unwrap_err();
        assert_eq!(hook_error_kind(&err), "invalid_signature");
    }

    #[test]
    fn verify_forgejo_signature_rejects_missing_signature_header() {
        let err = verify_forgejo_signature("secret", b"payload", "   ").unwrap_err();
        assert_eq!(hook_error_kind(&err), "invalid_signature");
    }

    #[test]
    fn verify_forgejo_signature_rejects_invalid_encoding() {
        let err = verify_forgejo_signature("secret", b"payload", "sha256=zz").unwrap_err();
        assert_eq!(hook_error_kind(&err), "invalid_signature");
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
        let decision = evaluate_pre_receive(&fetcher, &repo_path, &updates).expect("decision");
        assert_eq!(update_decision_kind(&decision), "reject");
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
            evaluate_pre_receive(&FailingFetcher, &repo_path, &updates).expect("decision");
        assert_eq!(update_decision_kind(&decision), "accept");
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
        let err = evaluate_pre_receive(&FailingFetcher, &repo_path, &updates).unwrap_err();
        assert_eq!(hook_service_error_kind(&err), "state");
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
            &mut || {
                super::run_hook_from_env(HookMode::PreReceive).expect("run hook");
            },
        );
        let _ = std::fs::remove_file(updates_path);
    }

    #[test]
    fn run_hook_pre_receive_returns_parse_errors() {
        let updates_path = write_updates_file("invalid\n");
        let repo_path = std::path::Path::new("/tmp")
            .join(SAMPLE_NPUB)
            .join("repo.git");
        let config = super::HookConfig {
            state_url: "http://127.0.0.1:8082".to_string(),
            sync_url: None,
            mode: HookMode::PreReceive,
        };
        with_env_vars(
            &[(
                super::ENV_HOOK_REPO_PATH,
                Some(repo_path.to_str().expect("repo path")),
            )],
            &mut || {
                let err = super::run_hook(config.clone(), Some(&updates_path))
                    .expect_err("parse error");
                assert_eq!(hook_service_error_kind(&err), "parse");
            },
        );
        let _ = std::fs::remove_file(updates_path);
    }

    #[test]
    fn run_hook_post_receive_ignores_parse_errors() {
        let updates_path = write_updates_file("invalid\n");
        let repo_path = std::path::Path::new("/tmp")
            .join(SAMPLE_NPUB)
            .join("repo.git");
        let config = super::HookConfig {
            state_url: "http://127.0.0.1:8082".to_string(),
            sync_url: Some("http://127.0.0.1:8088".to_string()),
            mode: HookMode::PostReceive,
        };
        with_env_vars(
            &[(
                super::ENV_HOOK_REPO_PATH,
                Some(repo_path.to_str().expect("repo path")),
            )],
            &mut || {
                super::run_hook(config.clone(), Some(&updates_path))
                    .expect("post receive parse errors");
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
            &mut || {
                let err = super::run_hook(config.clone(), Some(&updates_path))
                    .expect_err("missing sync");
                assert_eq!(hook_service_error_kind(&err), "config");
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
            &mut || {
                super::run_hook(config.clone(), Some(&updates_path)).expect("run hook");
            },
        );
        handle.join().expect("server join");
        let _ = std::fs::remove_file(updates_path);
    }

    #[test]
    fn run_hook_uses_git_dir_when_repo_path_env_missing() {
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
            &[
                (super::ENV_HOOK_REPO_PATH, None),
                ("GIT_DIR", Some(repo_path.to_str().expect("repo path"))),
            ],
            &mut || {
                super::run_hook(config.clone(), Some(&updates_path)).expect("run hook");
            },
        );
        handle.join().expect("server join");
        let _ = std::fs::remove_file(updates_path);
    }

    #[test]
    fn run_hook_pre_receive_surfaces_reject_reason() {
        let updates = format!("{} {} refs/heads/main\n", "1".repeat(40), "2".repeat(40));
        let updates_path = write_updates_file(&updates);
        let repo_path = std::path::Path::new("/tmp")
            .join(SAMPLE_NPUB)
            .join("repo.git");
        let body = format!(
            "{{\"identifier\":\"repo\",\"state\":{{\"HEAD\":\"ref: refs/heads/main\",\"refs/heads/main\":\"{}\"}}}}",
            "f".repeat(40)
        );
        let (state_url, handle) = start_mock_http_server("200 OK", "application/json", &body);
        let config = super::HookConfig {
            state_url,
            sync_url: None,
            mode: HookMode::PreReceive,
        };
        with_env_vars(
            &[(
                super::ENV_HOOK_REPO_PATH,
                Some(repo_path.to_str().expect("repo path")),
            )],
            &mut || {
                let err = super::run_hook(config.clone(), Some(&updates_path))
                    .expect_err("reject");
                assert_eq!(hook_service_error_kind(&err), "reject");
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
            &mut || {
                let err = super::run_hook(config.clone(), Some(&updates_path))
                    .expect_err("invalid cwd");
                assert_eq!(hook_service_error_kind(&err), "core");
            },
        );
        let _ = std::fs::remove_file(updates_path);
    }

    #[test]
    fn run_hook_reports_current_dir_lookup_errors() {
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
        let mut result: Option<Result<(), super::HookServiceError>> = None;
        with_env_vars(&[(super::ENV_HOOK_REPO_PATH, None), ("GIT_DIR", None)], &mut || {
                let original_dir = std::env::current_dir().expect("current dir");
                let temp = std::env::temp_dir().join(format!(
                    "gittree-hook-cwd-{}",
                    SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .expect("time")
                        .as_nanos()
                ));
                std::fs::create_dir_all(&temp).expect("create temp dir");
                std::env::set_current_dir(&temp).expect("set current dir");
                std::fs::remove_dir_all(&temp).expect("remove temp dir");
                let outcome = super::run_hook(config.clone(), Some(&updates_path));
                std::env::set_current_dir(&original_dir).expect("restore current dir");
                result = Some(outcome);
            });
        let err = result
            .expect("expected hook result")
            .expect_err("expected current dir failure");
        assert_eq!(hook_service_error_kind(&err), "core");
        let _ = std::fs::remove_file(updates_path);
    }

    #[test]
    fn run_hook_reads_stdin_when_no_file_is_provided() {
        let repo_path = std::path::Path::new("/tmp")
            .join(SAMPLE_NPUB)
            .join("repo.git");
        let config = super::HookConfig {
            state_url: "http://127.0.0.1:8082".to_string(),
            sync_url: None,
            mode: HookMode::PreReceive,
        };
        with_env_vars(
            &[(
                super::ENV_HOOK_REPO_PATH,
                Some(repo_path.to_str().expect("repo path")),
            )],
            &mut || {
                super::run_hook(config.clone(), None).expect("stdin read should not fail");
            },
        );
    }

    #[test]
    fn run_hook_with_terminal_rejects_tty_without_stdin_file() {
        let config = super::HookConfig {
            state_url: "http://127.0.0.1:8082".to_string(),
            sync_url: None,
            mode: HookMode::PreReceive,
        };
        let err = super::run_hook_with_terminal(config, None, true).expect_err("tty should fail");
        assert_eq!(hook_service_error_kind(&err), "core");
    }

    #[test]
    fn run_hook_with_terminal_reports_missing_stdin_file_error() {
        let repo_path = std::path::Path::new("/tmp")
            .join(SAMPLE_NPUB)
            .join("repo.git");
        let config = super::HookConfig {
            state_url: "http://127.0.0.1:8082".to_string(),
            sync_url: None,
            mode: HookMode::PreReceive,
        };
        let missing = std::path::Path::new("/tmp/does-not-exist-gittree-hook-run-hook.txt");
        with_env_vars(
            &[(
                super::ENV_HOOK_REPO_PATH,
                Some(repo_path.to_str().expect("repo path")),
            )],
            &mut || {
                let err = super::run_hook_with_terminal(config.clone(), Some(missing), false)
                    .expect_err("missing stdin file should fail");
                assert_eq!(hook_service_error_kind(&err), "core");
            },
        );
    }

    #[test]
    fn http_state_fetcher_returns_none_for_not_found() {
        let (base_url, handle) = start_mock_http_server("404 Not Found", "text/plain", "missing");
        let fetcher = super::HttpStateFetcher::new(base_url, std::time::Duration::from_secs(1));
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
        let fetcher = super::HttpStateFetcher::new(base_url, std::time::Duration::from_secs(1));
        let err = fetcher
            .latest_state("11".repeat(32).as_str(), "repo")
            .expect_err("state should fail");
        assert_eq!(hook_service_error_kind(&err), "state");
        handle.join().expect("server join");
    }

    #[test]
    fn http_state_fetcher_parses_success_response() {
        let body = format!(
            "{{\"identifier\":\"repo\",\"state\":{{\"HEAD\":\"ref: refs/heads/main\",\"refs/heads/main\":\"{}\"}}}}",
            "11".repeat(20)
        );
        let (base_url, handle) = start_mock_http_server("200 OK", "application/json", &body);
        let fetcher = super::HttpStateFetcher::new(base_url, std::time::Duration::from_secs(1));
        let state = fetcher
            .latest_state("11".repeat(32).as_str(), "repo")
            .expect("state fetch")
            .expect("state payload");
        assert_eq!(state.identifier, "repo");
        assert_eq!(state.state.get("refs/heads/main"), Some(&"11".repeat(20)));
        handle.join().expect("server join");
    }

    #[test]
    fn http_state_fetcher_reports_send_errors() {
        let fetcher = super::HttpStateFetcher::new(
            "http://127.0.0.1:1",
            std::time::Duration::from_millis(100),
        );
        let err = fetcher
            .latest_state("11".repeat(32).as_str(), "repo")
            .expect_err("state should fail");
        assert_eq!(hook_service_error_kind(&err), "state");
    }

    #[test]
    fn http_state_fetcher_reports_json_parse_errors() {
        let (base_url, handle) = start_mock_http_server("200 OK", "application/json", "{");
        let fetcher = super::HttpStateFetcher::new(base_url, std::time::Duration::from_secs(1));
        let err = fetcher
            .latest_state("11".repeat(32).as_str(), "repo")
            .expect_err("state should fail");
        assert_eq!(hook_service_error_kind(&err), "state");
        handle.join().expect("server join");
    }

    #[test]
    fn http_post_receive_notifier_reports_error_status() {
        let (endpoint, handle) =
            start_mock_http_server("500 Internal Server Error", "text/plain", "nope");
        let notifier =
            super::HttpPostReceiveNotifier::new(endpoint, std::time::Duration::from_secs(1));
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
        assert_eq!(hook_service_error_kind(&err), "state");
        handle.join().expect("server join");
    }

    #[test]
    fn http_post_receive_notifier_accepts_success_status() {
        let (endpoint, handle) = start_mock_http_server("200 OK", "application/json", "{}");
        let notifier =
            super::HttpPostReceiveNotifier::new(endpoint, std::time::Duration::from_secs(1));
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

    #[test]
    fn http_post_receive_notifier_reports_send_errors() {
        let notifier = super::HttpPostReceiveNotifier::new(
            "http://127.0.0.1:1",
            std::time::Duration::from_millis(100),
        );
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
        assert_eq!(hook_service_error_kind(&err), "state");
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
        handle_post_receive(&notifier, &repo_path, &updates).expect("post receive");
        let payloads = notifier.payloads.lock().expect("payload lock");
        assert_eq!(payloads.len(), 1);
        assert_eq!(payloads[0].updates[0].reference, "refs/heads/main");
    }

    #[test]
    fn handle_post_receive_rejects_invalid_repo_path() {
        let notifier = MockNotifier::new();
        let updates = vec![super::RefUpdate {
            old: "0".repeat(40),
            new: "1".repeat(40),
            reference: "refs/heads/main".to_string(),
        }];
        let err = handle_post_receive(
            &notifier,
            std::path::Path::new("/tmp/not-an-npub/repo.git"),
            &updates,
        )
            .expect_err("invalid path");
        assert_eq!(hook_service_error_kind(&err), "core");
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
        assert_eq!(hook_service_error_kind(&err), "reject");
    }

    struct ErrorResolver;

    impl MappingResolver for ErrorResolver {
        fn resolve_mapping(
            &self,
            _owner: &str,
            _repo: &str,
        ) -> Result<Option<RepoMapping>, super::HookServiceError> {
            Err(super::HookServiceError::Core(
                "mapping resolver failed".to_string(),
            ))
        }
    }

    #[test]
    fn handle_forgejo_push_propagates_mapping_resolver_errors() {
        let resolver = ErrorResolver;
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
        assert_eq!(hook_service_error_kind(&err), "core");
    }

    #[test]
    fn handle_forgejo_push_rejects_invalid_json_payload() {
        let resolver = MockResolver { mapping: None };
        let notifier = MockNotifier::new();
        let err = handle_forgejo_push(&resolver, &notifier, "{not-json}")
            .expect_err("invalid payload should fail");
        assert_eq!(hook_service_error_kind(&err), "parse");
    }
}

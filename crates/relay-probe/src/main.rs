use gittree_config::{RelayProbeConfig, RelayTargetsConfig};
use gittree_relay_adapter::{RelayAdapterConfig, WebsocketRelayAdapter};
use gittree_relay_probe::{
    HttpRelayProbeClient, RelayProbeClient, RelayProbeError, RelayProbeResult, probe_relay,
    probe_relay_with_adapter_result,
};
use gittree_storage::{
    PostgresRepositories, RelayCompatibilityRecord, RelayCompatibilityRepository,
    RelayProbeMetadata, StorageConfig, StorageError,
};
use std::process::exit;
use std::time::Duration;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug)]
struct ProbeCli {
    relay: Option<String>,
    all: bool,
    json: bool,
    store: bool,
    active: Option<bool>,
    timeout_secs: Option<u64>,
    secret_key: Option<String>,
}

impl ProbeCli {
    fn parse<I, T>(args: I) -> Result<Self, RelayProbeError>
    where
        I: IntoIterator<Item = T>,
        T: Into<std::ffi::OsString>,
    {
        let mut relay: Option<String> = None;
        let mut all = false;
        let mut json = false;
        let mut store = false;
        let mut active: Option<bool> = None;
        let mut timeout_secs: Option<u64> = None;
        let mut secret_key: Option<String> = None;

        let mut iter = args
            .into_iter()
            .map(|arg| arg.into().to_string_lossy().to_string());
        iter.next();
        while let Some(value) = iter.next() {
            match value.as_str() {
                "--all" => all = true,
                "--json" => json = true,
                "--store" => store = true,
                "--active" => active = Some(true),
                "--no-active" => active = Some(false),
                "--timeout-secs" => {
                    let Some(next) = iter.next() else {
                        return Err(RelayProbeError::InvalidRelayUrl(
                            "missing timeout value".to_string(),
                        ));
                    };
                    let parsed = next.parse::<u64>().map_err(|_| {
                        RelayProbeError::InvalidRelayUrl("invalid timeout value".to_string())
                    })?;
                    timeout_secs = Some(parsed);
                }
                "--secret-key" => {
                    let Some(next) = iter.next() else {
                        return Err(RelayProbeError::InvalidRelayUrl(
                            "missing secret key".to_string(),
                        ));
                    };
                    secret_key = Some(next);
                }
                "--relay" => {
                    let Some(next) = iter.next() else {
                        return Err(RelayProbeError::InvalidRelayUrl("--relay".to_string()));
                    };
                    relay = Some(next);
                }
                _ if value.starts_with("--relay=") => {
                    relay = Some(value.trim_start_matches("--relay=").to_string());
                }
                "--help" | "-h" => {
                    print_help();
                    exit(0);
                }
                other => {
                    return Err(RelayProbeError::InvalidRelayUrl(format!(
                        "unknown flag {other}"
                    )));
                }
            }
        }

        if all && relay.is_some() {
            return Err(RelayProbeError::InvalidRelayUrl(
                "cannot combine --all with --relay".to_string(),
            ));
        }
        if !all && relay.is_none() {
            return Err(RelayProbeError::InvalidRelayUrl(
                "missing --relay or --all".to_string(),
            ));
        }
        Ok(Self {
            relay,
            all,
            json,
            store,
            active,
            timeout_secs,
            secret_key,
        })
    }
}

fn print_help() {
    println!(
        "gittree-relay-probe --relay <wss://relay> [--active] [--timeout-secs N] [--json] [--store]\n       gittree-relay-probe --all [--active] [--timeout-secs N] [--json] [--store]\n\nFlags:\n  --relay <url>       Relay URL to probe\n  --all               Probe relay targets from GITTREE_RELAY_URLS\n  --active            Run active write/read probe\n  --no-active         Disable active probe even if env enables it\n  --timeout-secs <n>  Active probe timeout in seconds\n  --secret-key <hex>  Hex secret key for probe signer (optional)\n  --json              Output JSON report\n  --store             Persist compatibility result to storage\n  -h, --help          Show help\n"
    );
}

fn main() {
    dotenvy::dotenv().ok();
    if let Err(err) = run() {
        eprintln!("relay probe failed: {err}");
        exit(1);
    }
}

fn run() -> Result<(), ProbeCommandError> {
    run_with_args(std::env::args_os())
}

fn run_with_args<I, T>(args: I) -> Result<(), ProbeCommandError>
where
    I: IntoIterator<Item = T>,
    T: Into<std::ffi::OsString>,
{
    let cli = ProbeCli::parse(args).map_err(ProbeCommandError::Cli)?;
    run_with_cli(cli)
}

fn run_with_cli(cli: ProbeCli) -> Result<(), ProbeCommandError> {
    let client = HttpRelayProbeClient::new().map_err(ProbeCommandError::Cli)?;
    let runtime = tokio::runtime::Runtime::new()
        .map_err(|err| ProbeCommandError::Runtime(err.to_string()))?;
    let mut probe_config = RelayProbeConfig::from_env().map_err(ProbeCommandError::Config)?;
    if let Some(active) = cli.active {
        probe_config.active = active;
    }
    if let Some(timeout_secs) = cli.timeout_secs {
        probe_config.timeout_secs = timeout_secs;
    }
    if let Some(secret_key) = cli.secret_key.clone() {
        probe_config.secret_key = Some(secret_key);
    }
    probe_config.validate().map_err(ProbeCommandError::Config)?;
    let results = execute_probe_with_client(&cli, &probe_config, &runtime, &client)?;
    let output = render_probe_output(&cli, &results);
    if !output.is_empty() {
        print!("{output}");
    }

    Ok(())
}

fn execute_probe_with_client<C: RelayProbeClient>(
    cli: &ProbeCli,
    probe_config: &RelayProbeConfig,
    runtime: &tokio::runtime::Runtime,
    client: &C,
) -> Result<Vec<RelayProbeResult>, ProbeCommandError> {
    let targets = resolve_targets(cli)?;
    let mut results = Vec::with_capacity(targets.len());

    for relay_url in targets {
        let mut result = probe_relay(&relay_url, client).map_err(ProbeCommandError::Cli)?;
        if probe_config.active {
            let mut adapter_config = RelayAdapterConfig::new(&relay_url)
                .with_timeout(Duration::from_secs(probe_config.timeout_secs));
            if let Some(secret_key) = probe_config.secret_key.clone() {
                adapter_config = adapter_config.with_secret_key(secret_key);
            }
            let adapter = WebsocketRelayAdapter::new(adapter_config);
            result = runtime
                .block_on(probe_relay_with_adapter_result(result, &adapter))
                .map_err(ProbeCommandError::Cli)?;
        }
        if cli.store {
            runtime.block_on(store_probe_result(&result))?;
        }
        results.push(result);
    }

    Ok(results)
}

fn render_probe_output(cli: &ProbeCli, results: &[RelayProbeResult]) -> String {
    if cli.json {
        if cli.all {
            let json = serde_json::to_string_pretty(results).unwrap_or_else(|_| "[]".to_string());
            return format!("{json}\n");
        }
        if let Some(result) = results.first() {
            let json = serde_json::to_string_pretty(result).unwrap_or_else(|_| "{}".to_string());
            return format!("{json}\n");
        }
        return String::new();
    }

    let mut output = String::new();
    for result in results {
        output.push_str(&format!("relay: {}\n", result.relay_url));
        output.push_str(&format!("compatible: {}\n", result.report.is_compatible()));
        if !result.report.missing_required.is_empty() {
            output.push_str(&format!(
                "missing required: {:?}\n",
                result.report.missing_required
            ));
        }
        if !result.report.missing_optional.is_empty() {
            output.push_str(&format!(
                "missing optional: {:?}\n",
                result.report.missing_optional
            ));
        }
    }
    output
}

async fn store_probe_result(result: &RelayProbeResult) -> Result<(), ProbeCommandError> {
    let storage = storage_from_env().map_err(ProbeCommandError::StorageConfig)?;
    let options = storage
        .write_connect_options()
        .map_err(ProbeCommandError::Storage)?;
    let pool = storage
        .pool_options()
        .map_err(ProbeCommandError::Storage)?
        .connect_with(options)
        .await
        .map_err(StorageError::from)
        .map_err(ProbeCommandError::Storage)?;
    let repo = PostgresRepositories::new(pool);
    store_probe_result_with_repo(&repo, result, now_unix_timestamp())
        .await
        .map_err(ProbeCommandError::Storage)?;
    Ok(())
}

async fn store_probe_result_with_repo<R: RelayCompatibilityRepository>(
    repo: &R,
    result: &RelayProbeResult,
    checked_at: i64,
) -> Result<(), StorageError> {
    let metadata = RelayProbeMetadata {
        nip11_url: result.nip11_url.clone(),
        nip11_available: result.nip11_available,
        active_probe_ok: result.active_probe.as_ref().map(|probe| probe.ok),
        active_probe_error: result
            .active_probe
            .as_ref()
            .and_then(|probe| probe.error.clone()),
    };
    let record = RelayCompatibilityRecord::new(&result.report, checked_at, &metadata)?;
    repo.upsert_relay_compatibility(record).await?;
    Ok(())
}

fn resolve_targets(cli: &ProbeCli) -> Result<Vec<String>, ProbeCommandError> {
    if cli.all {
        let config = RelayTargetsConfig::from_env_validated().map_err(ProbeCommandError::Config)?;
        if config.relay_urls.is_empty() {
            return Err(ProbeCommandError::MissingTargets(
                "GITTREE_RELAY_URLS is empty".to_string(),
            ));
        }
        return Ok(config.relay_urls);
    }
    let relay = cli
        .relay
        .clone()
        .ok_or_else(|| ProbeCommandError::MissingTargets("missing --relay value".to_string()))?;
    Ok(vec![relay])
}

fn now_unix_timestamp() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

const ENV_STORAGE_READ_URL: &str = "GITTREE_STORAGE_READ_URL";
const ENV_STORAGE_WRITE_URL: &str = "GITTREE_STORAGE_WRITE_URL";
const ENV_STORAGE_MAX_CONNECTIONS: &str = "GITTREE_STORAGE_MAX_CONNECTIONS";
const ENV_STORAGE_MIN_CONNECTIONS: &str = "GITTREE_STORAGE_MIN_CONNECTIONS";
const ENV_STORAGE_IDLE_TIMEOUT_SECS: &str = "GITTREE_STORAGE_IDLE_TIMEOUT_SECS";
const ENV_STORAGE_MAX_LIFETIME_SECS: &str = "GITTREE_STORAGE_MAX_LIFETIME_SECS";
const ENV_STORAGE_APP_NAME: &str = "GITTREE_STORAGE_APP_NAME";

#[derive(Debug)]
enum ProbeCommandError {
    Cli(RelayProbeError),
    Config(gittree_config::ConfigError),
    MissingTargets(String),
    Runtime(String),
    StorageConfig(StorageConfigError),
    Storage(StorageError),
}

impl std::fmt::Display for ProbeCommandError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProbeCommandError::Cli(err) => write!(f, "{err}"),
            ProbeCommandError::Config(err) => write!(f, "relay probe config error: {err}"),
            ProbeCommandError::MissingTargets(message) => write!(f, "{message}"),
            ProbeCommandError::Runtime(message) => {
                write!(f, "relay probe runtime error: {message}")
            }
            ProbeCommandError::StorageConfig(err) => {
                write!(f, "relay probe storage config error: {err}")
            }
            ProbeCommandError::Storage(err) => write!(f, "relay probe storage error: {err}"),
        }
    }
}

impl std::error::Error for ProbeCommandError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ProbeCommandError::Cli(err) => Some(err),
            ProbeCommandError::Config(err) => Some(err),
            ProbeCommandError::MissingTargets(_) => None,
            ProbeCommandError::Runtime(_) => None,
            ProbeCommandError::StorageConfig(err) => Some(err),
            ProbeCommandError::Storage(err) => Some(err),
        }
    }
}

#[derive(Debug)]
enum StorageConfigError {
    MissingEnv(&'static str),
    InvalidEnv { key: &'static str, value: String },
    InvalidConfig(String),
}

impl std::fmt::Display for StorageConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StorageConfigError::MissingEnv(key) => write!(f, "missing env {key}"),
            StorageConfigError::InvalidEnv { key, value } => {
                write!(f, "invalid env {key}: {value}")
            }
            StorageConfigError::InvalidConfig(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for StorageConfigError {}

fn storage_from_env() -> Result<StorageConfig, StorageConfigError> {
    let read_connection = std::env::var(ENV_STORAGE_READ_URL)
        .map_err(|_| StorageConfigError::MissingEnv(ENV_STORAGE_READ_URL))?;
    let write_connection = std::env::var(ENV_STORAGE_WRITE_URL).ok();
    let max_connections = env_u32(ENV_STORAGE_MAX_CONNECTIONS)?.unwrap_or(10);
    let min_connections = env_u32(ENV_STORAGE_MIN_CONNECTIONS)?.unwrap_or(2);
    let idle_timeout_secs = env_u64(ENV_STORAGE_IDLE_TIMEOUT_SECS)?;
    let max_lifetime_secs = env_u64(ENV_STORAGE_MAX_LIFETIME_SECS)?;
    let application_name = std::env::var(ENV_STORAGE_APP_NAME).ok();

    let config = StorageConfig {
        read_connection,
        write_connection,
        max_connections,
        min_connections,
        idle_timeout_secs,
        max_lifetime_secs,
        application_name,
    };

    config
        .validate()
        .map_err(|err| StorageConfigError::InvalidConfig(err.to_string()))?;

    Ok(config)
}

fn env_u32(key: &'static str) -> Result<Option<u32>, StorageConfigError> {
    match std::env::var(key) {
        Ok(value) => {
            if value.trim().is_empty() {
                return Ok(None);
            }
            value
                .parse::<u32>()
                .map(Some)
                .map_err(|_| StorageConfigError::InvalidEnv { key, value })
        }
        Err(_) => Ok(None),
    }
}

fn env_u64(key: &'static str) -> Result<Option<u64>, StorageConfigError> {
    match std::env::var(key) {
        Ok(value) => {
            if value.trim().is_empty() {
                return Ok(None);
            }
            value
                .parse::<u64>()
                .map(Some)
                .map_err(|_| StorageConfigError::InvalidEnv { key, value })
        }
        Err(_) => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ProbeCli, ProbeCommandError, RelayProbeError, RelayProbeResult, StorageConfigError,
        execute_probe_with_client, now_unix_timestamp, print_help, render_probe_output,
        resolve_targets, run_with_args, storage_from_env, store_probe_result,
        store_probe_result_with_repo,
    };
    use gittree_config::RelayProbeConfig;
    use gittree_core::{RelayCapability, RelayCompatibilityReport};
    use gittree_relay_probe::RelayProbeClient;
    use gittree_storage::{InMemoryRepositories, RelayCompatibilityRepository, StorageError};
    use std::error::Error;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn with_env_var<F: FnOnce()>(key: &str, value: &str, f: F) {
        let previous = std::env::var_os(key);
        // SAFETY: tests run single-threaded in this crate; we restore the previous value after.
        unsafe {
            std::env::set_var(key, value);
        }
        f();
        match previous {
            Some(old) => unsafe {
                std::env::set_var(key, old);
            },
            None => unsafe {
                std::env::remove_var(key);
            },
        }
    }

    #[test]
    fn parse_accepts_store_flag() {
        let cli =
            ProbeCli::parse(["probe", "--relay", "wss://relay.example", "--store"]).expect("cli");
        assert!(cli.store);
        assert!(!cli.all);
        assert_eq!(cli.relay.as_deref(), Some("wss://relay.example"));
    }

    #[test]
    fn parse_accepts_all_flag() {
        let cli = ProbeCli::parse(["probe", "--all"]).expect("cli");
        assert!(cli.all);
        assert!(cli.relay.is_none());
    }

    #[test]
    fn parse_accepts_active_flag() {
        let cli =
            ProbeCli::parse(["probe", "--relay", "wss://relay.example", "--active"]).expect("cli");
        assert_eq!(cli.active, Some(true));
    }

    #[test]
    fn parse_accepts_timeout_flag() {
        let cli = ProbeCli::parse([
            "probe",
            "--relay",
            "wss://relay.example",
            "--timeout-secs",
            "12",
        ])
        .expect("cli");
        assert_eq!(cli.timeout_secs, Some(12));
    }

    #[test]
    fn parse_accepts_inline_relay_and_optional_flags() {
        let cli = ProbeCli::parse([
            "probe",
            "--relay=wss://relay.example",
            "--json",
            "--no-active",
            "--secret-key",
            "00",
        ])
        .expect("cli");
        assert_eq!(cli.relay.as_deref(), Some("wss://relay.example"));
        assert!(cli.json);
        assert_eq!(cli.active, Some(false));
        assert_eq!(cli.secret_key.as_deref(), Some("00"));
    }

    #[test]
    fn parse_rejects_all_with_relay() {
        let err =
            ProbeCli::parse(["probe", "--all", "--relay", "wss://relay.example"]).unwrap_err();
        assert!(matches!(err, RelayProbeError::InvalidRelayUrl(_)));
    }

    #[test]
    fn parse_rejects_missing_timeout_value() {
        let err = ProbeCli::parse(["probe", "--relay", "wss://relay.example", "--timeout-secs"])
            .expect_err("missing timeout");
        assert!(
            matches!(err, RelayProbeError::InvalidRelayUrl(message) if message == "missing timeout value")
        );
    }

    #[test]
    fn parse_rejects_invalid_timeout_value() {
        let err = ProbeCli::parse([
            "probe",
            "--relay",
            "wss://relay.example",
            "--timeout-secs",
            "invalid",
        ])
        .expect_err("invalid timeout");
        assert!(
            matches!(err, RelayProbeError::InvalidRelayUrl(message) if message == "invalid timeout value")
        );
    }

    #[test]
    fn parse_rejects_missing_secret_key() {
        let err = ProbeCli::parse(["probe", "--relay", "wss://relay.example", "--secret-key"])
            .expect_err("missing secret key");
        assert!(
            matches!(err, RelayProbeError::InvalidRelayUrl(message) if message == "missing secret key")
        );
    }

    #[test]
    fn parse_rejects_missing_relay_value() {
        let err = ProbeCli::parse(["probe", "--relay"]).expect_err("missing relay value");
        assert!(matches!(err, RelayProbeError::InvalidRelayUrl(message) if message == "--relay"));
    }

    #[test]
    fn parse_rejects_unknown_flag() {
        let err = ProbeCli::parse(["probe", "--relay", "wss://relay.example", "--wat"])
            .expect_err("unknown flag");
        assert!(
            matches!(err, RelayProbeError::InvalidRelayUrl(message) if message == "unknown flag --wat")
        );
    }

    #[test]
    fn parse_requires_relay_or_all() {
        let err = ProbeCli::parse(["probe"]).expect_err("missing relay or all");
        assert!(
            matches!(err, RelayProbeError::InvalidRelayUrl(message) if message == "missing --relay or --all")
        );
    }

    #[test]
    fn print_help_smoke() {
        print_help();
    }

    #[tokio::test]
    async fn store_probe_result_writes_record() {
        let repo = InMemoryRepositories::new();
        let report = RelayCompatibilityReport {
            relay_url: "wss://relay.example".to_string(),
            supported: vec![RelayCapability::Nip01, RelayCapability::Nip34],
            missing_required: Vec::new(),
            missing_optional: Vec::new(),
        };
        let result = RelayProbeResult {
            relay_url: report.relay_url.clone(),
            nip11_url: Some("https://relay.example/".to_string()),
            nip11_available: true,
            report,
            observed_capabilities: vec![RelayCapability::Nip01, RelayCapability::Nip34],
            nip11: None,
            active_probe: Some(gittree_relay_probe::ActiveProbeResult {
                ok: true,
                error: None,
            }),
            warnings: Vec::new(),
        };
        store_probe_result_with_repo(&repo, &result, 1)
            .await
            .expect("store");
        let record = repo
            .relay_compatibility("wss://relay.example")
            .await
            .expect("fetch");
        let record = record.expect("record");
        assert_eq!(record.nip11_url.as_deref(), Some("https://relay.example/"));
        assert!(record.nip11_available);
        assert_eq!(record.active_probe_ok, Some(true));
        assert_eq!(record.active_probe_error, None);
    }

    #[test]
    fn resolve_targets_requires_env_list() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_env_var("GITTREE_RELAY_URLS", "", || {
            let cli = ProbeCli::parse(["probe", "--all"]).expect("cli");
            let err = resolve_targets(&cli).unwrap_err();
            assert!(matches!(err, ProbeCommandError::MissingTargets(_)));
        });
    }

    #[test]
    fn resolve_targets_returns_relay_for_single_mode() {
        let cli = ProbeCli::parse(["probe", "--relay", "wss://relay.example"]).expect("cli");
        let targets = resolve_targets(&cli).expect("targets");
        assert_eq!(targets, vec!["wss://relay.example".to_string()]);
    }

    #[test]
    fn resolve_targets_reads_env_for_all_mode() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_env_var(
            "GITTREE_RELAY_URLS",
            "wss://relay.one,wss://relay.two",
            || {
                let cli = ProbeCli::parse(["probe", "--all"]).expect("cli");
                let targets = resolve_targets(&cli).expect("targets");
                assert_eq!(
                    targets,
                    vec!["wss://relay.one".to_string(), "wss://relay.two".to_string()]
                );
            },
        );
    }

    #[test]
    fn resolve_targets_reports_invalid_relay_url_in_env() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_env_var("GITTREE_RELAY_URLS", "not-a-relay", || {
            let cli = ProbeCli::parse(["probe", "--all"]).expect("cli");
            let err = resolve_targets(&cli).expect_err("invalid env relay url");
            assert!(matches!(err, ProbeCommandError::Config(_)));
        });
    }

    #[test]
    fn storage_config_requires_url() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        unsafe {
            std::env::remove_var(super::ENV_STORAGE_READ_URL);
        }
        let err = storage_from_env().unwrap_err();
        assert!(matches!(err, StorageConfigError::MissingEnv(_)));
    }

    #[test]
    fn storage_config_ignores_empty_timeouts() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_env_var(
            super::ENV_STORAGE_READ_URL,
            "postgres://user:pass@localhost:5432/gittree",
            || {
                with_env_var(super::ENV_STORAGE_IDLE_TIMEOUT_SECS, "", || {
                    with_env_var(super::ENV_STORAGE_MAX_LIFETIME_SECS, "", || {
                        let config = storage_from_env().expect("config");
                        assert_eq!(config.idle_timeout_secs, None);
                        assert_eq!(config.max_lifetime_secs, None);
                    });
                });
            },
        );
    }

    #[test]
    fn storage_config_reads_optional_write_and_app_name() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_env_var(
            super::ENV_STORAGE_READ_URL,
            "postgres://user:pass@localhost:5432/gittree",
            || {
                with_env_var(
                    super::ENV_STORAGE_WRITE_URL,
                    "postgres://user:pass@localhost:5432/gittree-write",
                    || {
                        with_env_var(super::ENV_STORAGE_APP_NAME, "relay-probe-tests", || {
                            let config = storage_from_env().expect("config");
                            assert_eq!(
                                config.write_connection.as_deref(),
                                Some("postgres://user:pass@localhost:5432/gittree-write")
                            );
                            assert_eq!(
                                config.application_name.as_deref(),
                                Some("relay-probe-tests")
                            );
                        });
                    },
                );
            },
        );
    }

    #[test]
    fn storage_config_rejects_invalid_numeric_env() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_env_var(
            super::ENV_STORAGE_READ_URL,
            "postgres://user:pass@localhost:5432/gittree",
            || {
                with_env_var(super::ENV_STORAGE_MAX_CONNECTIONS, "invalid", || {
                    let err = storage_from_env().expect_err("invalid max connections");
                    assert!(matches!(
                        err,
                        StorageConfigError::InvalidEnv {
                            key: super::ENV_STORAGE_MAX_CONNECTIONS,
                            ..
                        }
                    ));
                });
            },
        );
    }

    #[test]
    fn storage_config_rejects_invalid_u64_env() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_env_var(
            super::ENV_STORAGE_READ_URL,
            "postgres://user:pass@localhost:5432/gittree",
            || {
                with_env_var(super::ENV_STORAGE_IDLE_TIMEOUT_SECS, "invalid", || {
                    let err = storage_from_env().expect_err("invalid idle timeout");
                    assert!(matches!(
                        err,
                        StorageConfigError::InvalidEnv {
                            key: super::ENV_STORAGE_IDLE_TIMEOUT_SECS,
                            ..
                        }
                    ));
                });
            },
        );
    }

    #[test]
    fn storage_config_rejects_invalid_pool_bounds() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_env_var(
            super::ENV_STORAGE_READ_URL,
            "postgres://user:pass@localhost:5432/gittree",
            || {
                with_env_var(super::ENV_STORAGE_MAX_CONNECTIONS, "1", || {
                    with_env_var(super::ENV_STORAGE_MIN_CONNECTIONS, "2", || {
                        let err = storage_from_env().expect_err("invalid pool bounds");
                        assert!(matches!(err, StorageConfigError::InvalidConfig(_)));
                    });
                });
            },
        );
    }

    #[test]
    fn with_env_var_restores_previous_value() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        let key = "GITTREE_RELAY_PROBE_TEST_ENV";
        // SAFETY: protected by ENV_LOCK and restored at the end of the test.
        unsafe {
            std::env::set_var(key, "original");
        }
        with_env_var(key, "temporary", || {
            assert_eq!(std::env::var(key).ok().as_deref(), Some("temporary"));
        });
        assert_eq!(std::env::var(key).ok().as_deref(), Some("original"));
        // SAFETY: clean up test-only key.
        unsafe {
            std::env::remove_var(key);
        }
    }

    #[test]
    fn now_unix_timestamp_is_non_negative() {
        assert!(now_unix_timestamp() >= 0);
    }

    fn sample_probe_result() -> RelayProbeResult {
        let report = RelayCompatibilityReport {
            relay_url: "wss://relay.example".to_string(),
            supported: vec![RelayCapability::Nip01, RelayCapability::Nip34],
            missing_required: Vec::new(),
            missing_optional: Vec::new(),
        };
        RelayProbeResult {
            relay_url: report.relay_url.clone(),
            nip11_url: Some("https://relay.example/".to_string()),
            nip11_available: true,
            report,
            observed_capabilities: vec![RelayCapability::Nip01, RelayCapability::Nip34],
            nip11: None,
            active_probe: Some(gittree_relay_probe::ActiveProbeResult {
                ok: true,
                error: None,
            }),
            warnings: Vec::new(),
        }
    }

    struct StubProbeClient {
        response: Result<Option<String>, RelayProbeError>,
    }

    impl RelayProbeClient for StubProbeClient {
        fn fetch_nip11(&self, _url: &str) -> Result<Option<String>, RelayProbeError> {
            match &self.response {
                Ok(body) => Ok(body.clone()),
                Err(err) => Err(err.clone()),
            }
        }
    }

    #[test]
    fn execute_probe_with_client_collects_single_result() {
        let cli = ProbeCli {
            relay: Some("wss://relay.example".to_string()),
            all: false,
            json: false,
            store: false,
            active: Some(false),
            timeout_secs: None,
            secret_key: None,
        };
        let probe_config = RelayProbeConfig {
            active: false,
            timeout_secs: 5,
            secret_key: None,
        };
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        let client = StubProbeClient {
            response: Ok(Some(
                r#"{"name":"relay","supported_nips":[1,11,34]}"#.to_string(),
            )),
        };

        let results =
            execute_probe_with_client(&cli, &probe_config, &runtime, &client).expect("results");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].relay_url, "wss://relay.example");
        assert!(results[0].nip11_available);
    }

    #[test]
    fn execute_probe_with_client_maps_probe_errors() {
        let cli = ProbeCli {
            relay: Some("wss://relay.example".to_string()),
            all: false,
            json: false,
            store: false,
            active: Some(false),
            timeout_secs: None,
            secret_key: None,
        };
        let probe_config = RelayProbeConfig {
            active: false,
            timeout_secs: 5,
            secret_key: None,
        };
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        let client = StubProbeClient {
            response: Err(RelayProbeError::Http("boom".to_string())),
        };

        let err = execute_probe_with_client(&cli, &probe_config, &runtime, &client)
            .expect_err("probe failure");
        assert!(matches!(
            err,
            ProbeCommandError::Cli(RelayProbeError::Http(_))
        ));
    }

    #[test]
    fn render_probe_output_covers_json_and_text_modes() {
        let mut detailed = sample_probe_result();
        detailed.report.missing_required = vec![RelayCapability::Nip34];
        detailed.report.missing_optional = vec![RelayCapability::Nip11];

        let text_cli = ProbeCli {
            relay: Some("wss://relay.example".to_string()),
            all: false,
            json: false,
            store: false,
            active: None,
            timeout_secs: None,
            secret_key: None,
        };
        let text_output = render_probe_output(&text_cli, &[detailed.clone()]);
        assert!(text_output.contains("relay: wss://relay.example"));
        assert!(text_output.contains("missing required"));
        assert!(text_output.contains("missing optional"));

        let json_all_cli = ProbeCli {
            relay: None,
            all: true,
            json: true,
            store: false,
            active: None,
            timeout_secs: None,
            secret_key: None,
        };
        let json_all_output = render_probe_output(&json_all_cli, &[detailed.clone()]);
        assert!(json_all_output.trim_start().starts_with('['));

        let json_one_cli = ProbeCli {
            relay: Some("wss://relay.example".to_string()),
            all: false,
            json: true,
            store: false,
            active: None,
            timeout_secs: None,
            secret_key: None,
        };
        let json_one_output = render_probe_output(&json_one_cli, &[detailed]);
        assert!(json_one_output.trim_start().starts_with('{'));
        assert!(render_probe_output(&json_one_cli, &[]).is_empty());
    }

    #[tokio::test]
    async fn store_probe_result_reports_storage_config_error() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        let previous = std::env::var_os(super::ENV_STORAGE_READ_URL);
        // SAFETY: protected by ENV_LOCK and restored below.
        unsafe {
            std::env::remove_var(super::ENV_STORAGE_READ_URL);
        }
        let err = store_probe_result(&sample_probe_result())
            .await
            .expect_err("missing storage config");
        match previous {
            Some(value) => unsafe {
                std::env::set_var(super::ENV_STORAGE_READ_URL, value);
            },
            None => unsafe {
                std::env::remove_var(super::ENV_STORAGE_READ_URL);
            },
        }
        assert!(matches!(err, ProbeCommandError::StorageConfig(_)));
    }

    #[test]
    fn probe_command_error_display_and_source_cover_variants() {
        let cli = ProbeCommandError::Cli(RelayProbeError::InvalidRelayUrl("bad relay".to_string()));
        assert_eq!(cli.to_string(), "invalid relay url: bad relay");
        assert!(cli.source().is_some());

        let config_error = RelayProbeConfig::from_toml_str("timeout_secs = 0")
            .expect_err("invalid relay probe config");
        let config = ProbeCommandError::Config(config_error);
        assert!(config.to_string().contains("relay probe config error"));
        assert!(config.source().is_some());

        let missing = ProbeCommandError::MissingTargets("missing".to_string());
        assert_eq!(missing.to_string(), "missing");
        assert!(missing.source().is_none());

        let runtime = ProbeCommandError::Runtime("runtime down".to_string());
        assert_eq!(
            runtime.to_string(),
            "relay probe runtime error: runtime down"
        );
        assert!(runtime.source().is_none());

        let storage_cfg = ProbeCommandError::StorageConfig(StorageConfigError::MissingEnv("KEY"));
        assert!(
            storage_cfg
                .to_string()
                .contains("relay probe storage config error")
        );
        assert!(storage_cfg.source().is_some());

        let storage = ProbeCommandError::Storage(StorageError::Internal {
            message: "store failed".to_string(),
        });
        assert!(storage.to_string().contains("relay probe storage error"));
        assert!(storage.source().is_some());
    }

    #[test]
    fn storage_config_error_display_covers_all_variants() {
        let missing = StorageConfigError::MissingEnv("KEY");
        assert_eq!(missing.to_string(), "missing env KEY");

        let invalid = StorageConfigError::InvalidEnv {
            key: "KEY",
            value: "bad".to_string(),
        };
        assert_eq!(invalid.to_string(), "invalid env KEY: bad");

        let invalid_config = StorageConfigError::InvalidConfig("bounds".to_string());
        assert_eq!(invalid_config.to_string(), "bounds");
    }

    #[test]
    fn run_with_args_reports_cli_errors() {
        let err = run_with_args(["probe"]).expect_err("missing args");
        assert!(matches!(err, ProbeCommandError::Cli(_)));
    }

    #[test]
    fn run_with_args_reports_probe_config_errors() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_env_var("GITTREE_RELAY_PROBE_TIMEOUT_SECS", "0", || {
            let err = run_with_args(["probe", "--relay", "wss://relay.example"])
                .expect_err("invalid relay probe config");
            assert!(matches!(err, ProbeCommandError::Config(_)));
        });
    }

    #[test]
    fn run_with_args_reports_missing_targets_for_all_mode() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_env_var("GITTREE_RELAY_URLS", "", || {
            let err = run_with_args(["probe", "--all"]).expect_err("missing targets");
            assert!(matches!(err, ProbeCommandError::MissingTargets(_)));
        });
    }
}

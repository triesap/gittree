use gittree_relay_probe::{HttpRelayProbeClient, RelayProbeError, RelayProbeResult, probe_relay};
use gittree_storage::{
    PostgresRepositories, RelayCompatibilityRecord, RelayCompatibilityRepository, StorageConfig,
    StorageError,
};
use std::process::exit;
use std::time::{SystemTime, UNIX_EPOCH};

struct ProbeCli {
    relay: String,
    json: bool,
    store: bool,
}

impl ProbeCli {
    fn parse<I, T>(args: I) -> Result<Self, RelayProbeError>
    where
        I: IntoIterator<Item = T>,
        T: Into<std::ffi::OsString>,
    {
        let mut relay: Option<String> = None;
        let mut json = false;
        let mut store = false;

        let mut iter = args.into_iter().map(|arg| arg.into().to_string_lossy().to_string());
        iter.next();
        while let Some(value) = iter.next() {
            match value.as_str() {
                "--json" => json = true,
                "--store" => store = true,
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

        let relay = relay.ok_or_else(|| RelayProbeError::InvalidRelayUrl("missing --relay".into()))?;
        Ok(Self { relay, json, store })
    }
}

fn print_help() {
    println!(
        "gittree-relay-probe --relay <wss://relay> [--json] [--store]\n\nFlags:\n  --relay <url>  Relay URL to probe\n  --json         Output JSON report\n  --store        Persist compatibility result to storage\n  -h, --help     Show help\n"
    );
}

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();
    if let Err(err) = run().await {
        eprintln!("relay probe failed: {err}");
        exit(1);
    }
}

async fn run() -> Result<(), ProbeCommandError> {
    let cli = ProbeCli::parse(std::env::args_os()).map_err(ProbeCommandError::Cli)?;
    let client = HttpRelayProbeClient::new().map_err(ProbeCommandError::Cli)?;
    let result = probe_relay(&cli.relay, &client).map_err(ProbeCommandError::Cli)?;

    if cli.store {
        store_probe_result(&result).await?;
    }

    if cli.json {
        let json = serde_json::to_string_pretty(&result).unwrap_or_else(|_| "{}".to_string());
        println!("{json}");
    } else {
        println!("relay: {}", result.relay_url);
        println!("compatible: {}", result.report.is_compatible());
        if !result.report.missing_required.is_empty() {
            println!("missing required: {:?}", result.report.missing_required);
        }
        if !result.report.missing_optional.is_empty() {
            println!("missing optional: {:?}", result.report.missing_optional);
        }
    }

    Ok(())
}

async fn store_probe_result(result: &RelayProbeResult) -> Result<(), ProbeCommandError> {
    let record =
        RelayCompatibilityRecord::new(&result.report, now_unix_timestamp())
            .map_err(ProbeCommandError::Storage)?;
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
    repo.upsert_relay_compatibility(record)
        .await
        .map_err(ProbeCommandError::Storage)?;
    Ok(())
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
    StorageConfig(StorageConfigError),
    Storage(StorageError),
}

impl std::fmt::Display for ProbeCommandError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProbeCommandError::Cli(err) => write!(f, "{err}"),
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
    use super::{ProbeCli, StorageConfigError, storage_from_env};
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
        let cli = ProbeCli::parse([
            "probe",
            "--relay",
            "wss://relay.example",
            "--store",
        ])
        .expect("cli");
        assert!(cli.store);
        assert_eq!(cli.relay, "wss://relay.example");
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
}

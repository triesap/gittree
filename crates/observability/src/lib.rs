use opentelemetry_otlp::WithExportConfig;
use std::path::PathBuf;
use tracing_appender::non_blocking::{NonBlocking, WorkerGuard};
use tracing_subscriber::prelude::*;

type BoxSubscriber = Box<dyn tracing::Subscriber + Send + Sync>;
type SetSubscriberFn = dyn FnMut(BoxSubscriber) -> Result<(), ObservabilityError>;

const ENV_OTLP_ENDPOINT: &str = "GITTREE_OTLP_ENDPOINT";
const ENV_LOG_JSON: &str = "GITTREE_LOG_JSON";
const ENV_LOG_DIR: &str = "GITTREE_LOG_DIR";
const ENV_LOG_STDOUT: &str = "GITTREE_LOG_STDOUT";
const ENV_METRICS_ENABLED: &str = "GITTREE_METRICS_ENABLED";

pub const LOG_FIELD_RELAY_URL: &str = "relay_url";
pub const LOG_FIELD_RELAY_PROBE_STATUS: &str = "relay_probe_status";
pub const LOG_FIELD_RELAY_PROBE_DETAIL: &str = "relay_probe_detail";
pub const METRIC_RELAY_COMPATIBLE: &str = "gittree_relay_compatibility_ok";
pub const METRIC_RELAY_INCOMPATIBLE: &str = "gittree_relay_compatibility_bad";

pub struct RelayCompatibilityMetrics {
    compatible: opentelemetry::metrics::Counter<u64>,
    incompatible: opentelemetry::metrics::Counter<u64>,
}

impl RelayCompatibilityMetrics {
    pub fn new() -> Self {
        let meter = opentelemetry::global::meter("gittree");
        let compatible = meter
            .u64_counter(METRIC_RELAY_COMPATIBLE)
            .with_description("relay compatibility checks that passed")
            .init();
        let incompatible = meter
            .u64_counter(METRIC_RELAY_INCOMPATIBLE)
            .with_description("relay compatibility checks that failed")
            .init();
        Self {
            compatible,
            incompatible,
        }
    }

    pub fn record(&self, is_compatible: bool) {
        if is_compatible {
            self.compatible.add(1, &[]);
        } else {
            self.incompatible.add(1, &[]);
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObservabilityConfig {
    pub service_name: String,
    pub otlp_endpoint: Option<String>,
    pub log_json: bool,
    pub log_dir: Option<PathBuf>,
    pub log_stdout: bool,
    pub metrics_enabled: bool,
}

impl Default for ObservabilityConfig {
    fn default() -> Self {
        Self {
            service_name: "gittree".to_string(),
            otlp_endpoint: None,
            log_json: false,
            log_dir: Some(PathBuf::from("logs")),
            log_stdout: true,
            metrics_enabled: true,
        }
    }
}

impl ObservabilityConfig {
    pub fn from_env(service_name: impl Into<String>) -> Result<Self, ObservabilityConfigError> {
        let mut config = ObservabilityConfig::default();
        config.service_name = service_name.into();
        config.otlp_endpoint = env_string(ENV_OTLP_ENDPOINT);
        config.log_json = env_bool(ENV_LOG_JSON)?.unwrap_or(config.log_json);
        config.log_stdout = env_bool(ENV_LOG_STDOUT)?.unwrap_or(config.log_stdout);
        config.metrics_enabled = env_bool(ENV_METRICS_ENABLED)?.unwrap_or(config.metrics_enabled);
        if let Some(log_dir) = env_log_dir(ENV_LOG_DIR) {
            config.log_dir = log_dir;
        }
        Ok(config)
    }
}

#[derive(Debug)]
pub enum ObservabilityConfigError {
    InvalidEnv { key: &'static str, value: String },
}

impl std::fmt::Display for ObservabilityConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ObservabilityConfigError::InvalidEnv { key, value } => {
                write!(f, "invalid env {key}: {value}")
            }
        }
    }
}

impl std::error::Error for ObservabilityConfigError {}

fn env_bool(key: &'static str) -> Result<Option<bool>, ObservabilityConfigError> {
    match std::env::var(key) {
        Ok(value) => {
            if value.trim().is_empty() {
                return Ok(None);
            }
            parse_bool(&value)
                .map(Some)
                .ok_or(ObservabilityConfigError::InvalidEnv { key, value })
        }
        Err(_) => Ok(None),
    }
}

fn env_string(key: &'static str) -> Option<String> {
    match std::env::var(key) {
        Ok(value) if value.trim().is_empty() => None,
        Ok(value) => Some(value),
        Err(_) => None,
    }
}

fn env_log_dir(key: &'static str) -> Option<Option<PathBuf>> {
    match std::env::var(key) {
        Ok(value) => {
            if value.trim().is_empty() {
                Some(None)
            } else {
                Some(Some(PathBuf::from(value)))
            }
        }
        Err(_) => None,
    }
}

fn parse_bool(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" => Some(true),
        "false" | "0" | "no" => Some(false),
        _ => None,
    }
}

pub struct ObservabilityHandle {
    prometheus_registry: Option<prometheus::Registry>,
    log_guard: Option<WorkerGuard>,
}

impl std::fmt::Debug for ObservabilityHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ObservabilityHandle")
            .field("prometheus_registry", &self.prometheus_registry)
            .finish()
    }
}

impl ObservabilityHandle {
    pub fn prometheus_registry(&self) -> Option<&prometheus::Registry> {
        self.prometheus_registry.as_ref()
    }

    pub fn log_guard(&self) -> Option<&WorkerGuard> {
        self.log_guard.as_ref()
    }
}

#[derive(Debug)]
pub enum ObservabilityError {
    TraceInit(String),
    MetricsInit(String),
    SubscriberInit(String),
    LogInit(String),
}

impl std::fmt::Display for ObservabilityError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ObservabilityError::TraceInit(message) => {
                write!(f, "observability trace init failed: {message}")
            }
            ObservabilityError::MetricsInit(message) => {
                write!(f, "observability metrics init failed: {message}")
            }
            ObservabilityError::SubscriberInit(message) => {
                write!(f, "observability subscriber init failed: {message}")
            }
            ObservabilityError::LogInit(message) => {
                write!(f, "observability log init failed: {message}")
            }
        }
    }
}

impl std::error::Error for ObservabilityError {}

fn set_global_subscriber(subscriber: BoxSubscriber) -> Result<(), ObservabilityError> {
    tracing::subscriber::set_global_default(subscriber)
        .map_err(|err| ObservabilityError::SubscriberInit(err.to_string()))
}

fn build_file_writer(
    config: &ObservabilityConfig,
) -> Result<(Option<NonBlocking>, Option<WorkerGuard>), ObservabilityError> {
    let Some(base_dir) = &config.log_dir else {
        return Ok((None, None));
    };
    let service_dir = base_dir.join(&config.service_name);
    std::fs::create_dir_all(&service_dir)
        .map_err(|err| ObservabilityError::LogInit(err.to_string()))?;
    let file_name = format!("{}.log", config.service_name);
    let appender = tracing_appender::rolling::daily(service_dir, file_name);
    let (non_blocking, guard) = tracing_appender::non_blocking(appender);
    Ok((Some(non_blocking), Some(guard)))
}

fn init_with_subscriber(
    config: &ObservabilityConfig,
    set_subscriber: &mut SetSubscriberFn,
) -> Result<ObservabilityHandle, ObservabilityError> {
    let resource = opentelemetry_sdk::Resource::new(vec![opentelemetry::KeyValue::new(
        "service.name",
        config.service_name.clone(),
    )]);
    let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    let (file_writer, log_guard) = build_file_writer(config)?;

    if let Some(endpoint) = &config.otlp_endpoint {
        let tracer = opentelemetry_otlp::new_pipeline()
            .tracing()
            .with_trace_config(
                opentelemetry_sdk::trace::Config::default().with_resource(resource.clone()),
            )
            .with_exporter(
                opentelemetry_otlp::new_exporter()
                    .tonic()
                    .with_endpoint(endpoint),
            )
            .install_batch(opentelemetry_sdk::runtime::Tokio)
            .map_err(|err| ObservabilityError::TraceInit(err.to_string()))?;

        if config.log_json {
            let stdout_layer = config.log_stdout.then(|| {
                tracing_subscriber::fmt::layer()
                    .json()
                    .with_filter(env_filter.clone())
            });
            let file_layer = file_writer.as_ref().map(|writer| {
                tracing_subscriber::fmt::layer()
                    .json()
                    .with_writer(writer.clone())
                    .with_ansi(false)
                    .with_filter(env_filter.clone())
            });
            let otel_layer = tracing_opentelemetry::layer().with_tracer(tracer);
            let subscriber = tracing_subscriber::registry()
                .with(stdout_layer)
                .with(file_layer)
                .with(otel_layer);
            set_subscriber(Box::new(subscriber))?;
        } else {
            let stdout_layer = config
                .log_stdout
                .then(|| tracing_subscriber::fmt::layer().with_filter(env_filter.clone()));
            let file_layer = file_writer.as_ref().map(|writer| {
                tracing_subscriber::fmt::layer()
                    .with_writer(writer.clone())
                    .with_ansi(false)
                    .with_filter(env_filter.clone())
            });
            let otel_layer = tracing_opentelemetry::layer().with_tracer(tracer);
            let subscriber = tracing_subscriber::registry()
                .with(stdout_layer)
                .with(file_layer)
                .with(otel_layer);
            set_subscriber(Box::new(subscriber))?;
        }
    } else {
        if config.log_json {
            let stdout_layer = config.log_stdout.then(|| {
                tracing_subscriber::fmt::layer()
                    .json()
                    .with_filter(env_filter.clone())
            });
            let file_layer = file_writer.as_ref().map(|writer| {
                tracing_subscriber::fmt::layer()
                    .json()
                    .with_writer(writer.clone())
                    .with_ansi(false)
                    .with_filter(env_filter.clone())
            });
            let subscriber = tracing_subscriber::registry()
                .with(stdout_layer)
                .with(file_layer);
            set_subscriber(Box::new(subscriber))?;
        } else {
            let stdout_layer = config
                .log_stdout
                .then(|| tracing_subscriber::fmt::layer().with_filter(env_filter.clone()));
            let file_layer = file_writer.as_ref().map(|writer| {
                tracing_subscriber::fmt::layer()
                    .with_writer(writer.clone())
                    .with_ansi(false)
                    .with_filter(env_filter.clone())
            });
            let subscriber = tracing_subscriber::registry()
                .with(stdout_layer)
                .with(file_layer);
            set_subscriber(Box::new(subscriber))?;
        }
    }

    let mut handle = ObservabilityHandle {
        prometheus_registry: None,
        log_guard,
    };

    if config.metrics_enabled {
        let registry = prometheus::Registry::new();
        let exporter = opentelemetry_prometheus::exporter()
            .with_registry(registry.clone())
            .build()
            .map_err(|err| ObservabilityError::MetricsInit(err.to_string()))?;

        let provider = opentelemetry_sdk::metrics::SdkMeterProvider::builder()
            .with_resource(resource)
            .with_reader(exporter)
            .build();

        opentelemetry::global::set_meter_provider(provider);
        handle.prometheus_registry = Some(registry);
    }

    Ok(handle)
}

pub fn init(config: &ObservabilityConfig) -> Result<ObservabilityHandle, ObservabilityError> {
    let mut setter = set_global_subscriber;
    init_with_subscriber(config, &mut setter)
}

#[cfg(test)]
mod tests {
    use super::{
        BoxSubscriber, ObservabilityConfig, ObservabilityError, ObservabilityHandle,
        RelayCompatibilityMetrics,
    };
    use std::path::PathBuf;
    use std::sync::Mutex;
    use std::time::{SystemTime, UNIX_EPOCH};

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn unique_temp_dir(prefix: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        std::env::temp_dir().join(format!("{prefix}-{nanos}"))
    }

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
    fn default_config_has_expected_defaults() {
        let config = ObservabilityConfig::default();
        assert_eq!(config.service_name, "gittree");
        assert!(config.otlp_endpoint.is_none());
        assert!(!config.log_json);
        assert_eq!(config.log_dir, Some(PathBuf::from("logs")));
        assert!(config.log_stdout);
        assert!(config.metrics_enabled);
    }

    #[test]
    fn env_config_reads_flags() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_env_var("GITTREE_OTLP_ENDPOINT", "http://localhost:4317", || {
            with_env_var("GITTREE_LOG_JSON", "true", || {
                with_env_var("GITTREE_LOG_DIR", "logs-test", || {
                    with_env_var("GITTREE_LOG_STDOUT", "false", || {
                        with_env_var("GITTREE_METRICS_ENABLED", "false", || {
                            let config = ObservabilityConfig::from_env("svc").expect("config");
                            assert_eq!(config.service_name, "svc");
                            assert_eq!(
                                config.otlp_endpoint.as_deref(),
                                Some("http://localhost:4317")
                            );
                            assert!(config.log_json);
                            assert_eq!(config.log_dir, Some(PathBuf::from("logs-test")));
                            assert!(!config.log_stdout);
                            assert!(!config.metrics_enabled);
                        });
                    });
                });
            });
        });
    }

    #[test]
    fn env_config_allows_empty_log_dir() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_env_var("GITTREE_LOG_DIR", "", || {
            let config = ObservabilityConfig::from_env("svc").expect("config");
            assert_eq!(config.log_dir, None);
        });
    }

    #[test]
    fn env_config_rejects_invalid_bool() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_env_var("GITTREE_LOG_JSON", "maybe", || {
            let err = ObservabilityConfig::from_env("svc").expect_err("invalid");
            assert!(err.to_string().contains("GITTREE_LOG_JSON"));
        });
    }

    #[test]
    fn env_config_uses_defaults_for_empty_bool_values() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_env_var("GITTREE_LOG_JSON", "", || {
            with_env_var("GITTREE_LOG_STDOUT", "", || {
                with_env_var("GITTREE_METRICS_ENABLED", "", || {
                    let config = ObservabilityConfig::from_env("svc").expect("config");
                    assert!(!config.log_json);
                    assert!(config.log_stdout);
                    assert!(config.metrics_enabled);
                });
            });
        });
    }

    #[test]
    fn init_returns_handle() {
        let config = ObservabilityConfig::default();
        let mut set_subscriber = |_| Ok(());
        let handle = super::init_with_subscriber(&config, &mut set_subscriber).expect("init");
        assert!(handle.prometheus_registry().is_some());
    }

    #[test]
    fn relay_probe_log_fields_are_non_empty() {
        assert!(!super::LOG_FIELD_RELAY_URL.is_empty());
        assert!(!super::LOG_FIELD_RELAY_PROBE_STATUS.is_empty());
        assert!(!super::LOG_FIELD_RELAY_PROBE_DETAIL.is_empty());
    }

    #[test]
    fn relay_compatibility_metrics_have_names() {
        assert!(!super::METRIC_RELAY_COMPATIBLE.is_empty());
        assert!(!super::METRIC_RELAY_INCOMPATIBLE.is_empty());
    }

    #[test]
    fn relay_compatibility_metrics_new_and_record_paths() {
        let metrics = RelayCompatibilityMetrics::new();
        metrics.record(true);
        metrics.record(false);
    }

    #[test]
    fn observability_error_display_messages_are_stable() {
        let trace = ObservabilityError::TraceInit("trace".to_string());
        let metrics = ObservabilityError::MetricsInit("metrics".to_string());
        let subscriber = ObservabilityError::SubscriberInit("subscriber".to_string());
        let log = ObservabilityError::LogInit("log".to_string());
        assert!(trace.to_string().contains("trace init failed"));
        assert!(metrics.to_string().contains("metrics init failed"));
        assert!(subscriber.to_string().contains("subscriber init failed"));
        assert!(log.to_string().contains("log init failed"));
    }

    #[test]
    fn handle_debug_and_log_guard_accessors_cover_paths() {
        let handle = ObservabilityHandle {
            prometheus_registry: None,
            log_guard: None,
        };
        assert!(handle.log_guard().is_none());
        assert!(format!("{handle:?}").contains("ObservabilityHandle"));
    }

    #[test]
    fn env_config_handles_empty_and_absent_otlp_endpoint() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_env_var("GITTREE_OTLP_ENDPOINT", "", || {
            let config = ObservabilityConfig::from_env("svc").expect("config");
            assert!(config.otlp_endpoint.is_none());
        });
        // SAFETY: tests run single-threaded in this crate and we reset this key in this test.
        unsafe {
            std::env::remove_var("GITTREE_OTLP_ENDPOINT");
        }
        let config = ObservabilityConfig::from_env("svc").expect("config");
        assert!(config.otlp_endpoint.is_none());
    }

    #[test]
    fn init_with_subscriber_covers_json_non_otlp_and_metrics_disabled() {
        let mut config = ObservabilityConfig::default();
        config.log_json = true;
        config.log_stdout = false;
        config.log_dir = None;
        config.metrics_enabled = false;
        let mut set_subscriber = |_| Ok(());
        let handle = super::init_with_subscriber(&config, &mut set_subscriber).expect("init");
        assert!(handle.prometheus_registry().is_none());
    }

    #[test]
    fn init_with_subscriber_covers_json_non_otlp_with_writers() {
        let mut config = ObservabilityConfig::default();
        let temp_dir = unique_temp_dir("gittree-observability-json");
        config.log_json = true;
        config.log_stdout = true;
        config.log_dir = Some(temp_dir.clone());
        config.metrics_enabled = false;
        let mut set_subscriber = |_| Ok(());
        super::init_with_subscriber(&config, &mut set_subscriber).expect("init");
        std::fs::remove_dir_all(temp_dir).ok();
    }

    #[test]
    fn init_with_subscriber_propagates_subscriber_failure() {
        let mut config = ObservabilityConfig::default();
        config.log_dir = None;
        let mut set_subscriber = |_| Err(ObservabilityError::SubscriberInit("boom".to_string()));
        let err = super::init_with_subscriber(&config, &mut set_subscriber)
            .expect_err("subscriber error");
        assert!(err.to_string().contains("subscriber init failed"));
    }

    #[test]
    fn init_with_subscriber_maps_log_init_error_for_invalid_path() {
        let mut config = ObservabilityConfig::default();
        config.log_dir = Some(PathBuf::from("/dev/null"));
        let mut set_subscriber = |_| Ok(());
        let err = super::init_with_subscriber(&config, &mut set_subscriber).expect_err("log init");
        assert!(err.to_string().contains("log init failed"));
    }

    #[test]
    fn init_with_subscriber_covers_otlp_json_and_text_paths_inside_runtime() {
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        let _guard = runtime.enter();

        let mut json_config = ObservabilityConfig::default();
        json_config.otlp_endpoint = Some("http://localhost:4317".to_string());
        json_config.log_json = true;
        let temp_dir = unique_temp_dir("gittree-observability-otlp");
        json_config.log_dir = Some(temp_dir.clone());
        json_config.metrics_enabled = false;
        let mut set_subscriber = |_| Ok(());
        super::init_with_subscriber(&json_config, &mut set_subscriber).expect("otlp json");

        let mut text_config = json_config.clone();
        text_config.log_json = false;
        super::init_with_subscriber(&text_config, &mut set_subscriber).expect("otlp text");
        std::fs::remove_dir_all(temp_dir).ok();
    }

    #[test]
    fn set_global_subscriber_reports_subscriber_conflict() {
        let subscriber: BoxSubscriber = Box::new(tracing_subscriber::registry());
        super::set_global_subscriber(subscriber).expect("initial subscriber");
        let second: BoxSubscriber = Box::new(tracing_subscriber::registry());
        let err = super::set_global_subscriber(second).expect_err("subscriber conflict");
        assert!(err.to_string().contains("subscriber init failed"));
    }

    #[test]
    fn with_env_var_restores_existing_values() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        // SAFETY: test serializes env access with ENV_LOCK and restores the previous value.
        unsafe {
            std::env::set_var("GITTREE_TEST_OBS_KEY", "before");
        }
        with_env_var("GITTREE_TEST_OBS_KEY", "after", || {
            assert_eq!(
                std::env::var("GITTREE_TEST_OBS_KEY").ok().as_deref(),
                Some("after")
            );
        });
        assert_eq!(
            std::env::var("GITTREE_TEST_OBS_KEY").ok().as_deref(),
            Some("before")
        );
        // SAFETY: test serializes env access with ENV_LOCK and cleans up this test key.
        unsafe {
            std::env::remove_var("GITTREE_TEST_OBS_KEY");
        }
    }
}

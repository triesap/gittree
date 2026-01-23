use opentelemetry_otlp::WithExportConfig;
use tracing_subscriber::prelude::*;

const ENV_OTLP_ENDPOINT: &str = "GITTREE_OTLP_ENDPOINT";
const ENV_LOG_JSON: &str = "GITTREE_LOG_JSON";
const ENV_METRICS_ENABLED: &str = "GITTREE_METRICS_ENABLED";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObservabilityConfig {
    pub service_name: String,
    pub otlp_endpoint: Option<String>,
    pub log_json: bool,
    pub metrics_enabled: bool,
}

impl Default for ObservabilityConfig {
    fn default() -> Self {
        Self {
            service_name: "gittree".to_string(),
            otlp_endpoint: None,
            log_json: false,
            metrics_enabled: true,
        }
    }
}

impl ObservabilityConfig {
    pub fn from_env(service_name: impl Into<String>) -> Result<Self, ObservabilityConfigError> {
        let otlp_endpoint = std::env::var(ENV_OTLP_ENDPOINT).ok();
        let log_json = env_bool(ENV_LOG_JSON)?.unwrap_or(false);
        let metrics_enabled = env_bool(ENV_METRICS_ENABLED)?.unwrap_or(true);
        Ok(Self {
            service_name: service_name.into(),
            otlp_endpoint,
            log_json,
            metrics_enabled,
        })
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
        Ok(value) => parse_bool(&value)
            .map(Some)
            .ok_or(ObservabilityConfigError::InvalidEnv { key, value }),
        Err(_) => Ok(None),
    }
}

fn parse_bool(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" => Some(true),
        "false" | "0" | "no" => Some(false),
        _ => None,
    }
}

#[derive(Debug)]
pub struct ObservabilityHandle {
    prometheus_registry: Option<prometheus::Registry>,
}

impl ObservabilityHandle {
    pub fn prometheus_registry(&self) -> Option<&prometheus::Registry> {
        self.prometheus_registry.as_ref()
    }
}

#[derive(Debug)]
pub enum ObservabilityError {
    TraceInit(String),
    MetricsInit(String),
    SubscriberInit(String),
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
        }
    }
}

impl std::error::Error for ObservabilityError {}

pub fn init(config: &ObservabilityConfig) -> Result<ObservabilityHandle, ObservabilityError> {
    let resource = opentelemetry_sdk::Resource::new(vec![opentelemetry::KeyValue::new(
        "service.name",
        config.service_name.clone(),
    )]);

    if let Some(endpoint) = &config.otlp_endpoint {
        let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
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
            let fmt_layer = tracing_subscriber::fmt::layer()
                .json()
                .with_filter(env_filter.clone());
            let otel_layer = tracing_opentelemetry::layer().with_tracer(tracer);
            let subscriber = tracing_subscriber::registry()
                .with(fmt_layer)
                .with(otel_layer);
            tracing::subscriber::set_global_default(subscriber)
                .map_err(|err| ObservabilityError::SubscriberInit(err.to_string()))?;
        } else {
            let fmt_layer = tracing_subscriber::fmt::layer().with_filter(env_filter);
            let otel_layer = tracing_opentelemetry::layer().with_tracer(tracer);
            let subscriber = tracing_subscriber::registry()
                .with(fmt_layer)
                .with(otel_layer);
            tracing::subscriber::set_global_default(subscriber)
                .map_err(|err| ObservabilityError::SubscriberInit(err.to_string()))?;
        }
    } else {
        let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
        if config.log_json {
            let fmt_layer = tracing_subscriber::fmt::layer()
                .json()
                .with_filter(env_filter.clone());
            let subscriber = tracing_subscriber::registry().with(fmt_layer);
            tracing::subscriber::set_global_default(subscriber)
                .map_err(|err| ObservabilityError::SubscriberInit(err.to_string()))?;
        } else {
            let fmt_layer = tracing_subscriber::fmt::layer().with_filter(env_filter);
            let subscriber = tracing_subscriber::registry().with(fmt_layer);
            tracing::subscriber::set_global_default(subscriber)
                .map_err(|err| ObservabilityError::SubscriberInit(err.to_string()))?;
        }
    }

    let mut handle = ObservabilityHandle {
        prometheus_registry: None,
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

#[cfg(test)]
mod tests {
    use super::ObservabilityConfig;
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
    fn default_config_has_expected_defaults() {
        let config = ObservabilityConfig::default();
        assert_eq!(config.service_name, "gittree");
        assert!(config.otlp_endpoint.is_none());
        assert!(!config.log_json);
        assert!(config.metrics_enabled);
    }

    #[test]
    fn env_config_reads_flags() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_env_var("GITTREE_OTLP_ENDPOINT", "http://localhost:4317", || {
            with_env_var("GITTREE_LOG_JSON", "true", || {
                with_env_var("GITTREE_METRICS_ENABLED", "false", || {
                    let config = ObservabilityConfig::from_env("svc").expect("config");
                    assert_eq!(config.service_name, "svc");
                    assert_eq!(
                        config.otlp_endpoint.as_deref(),
                        Some("http://localhost:4317")
                    );
                    assert!(config.log_json);
                    assert!(!config.metrics_enabled);
                });
            });
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
    fn init_returns_handle() {
        let config = ObservabilityConfig::default();
        let handle = super::init(&config).expect("init");
        assert!(handle.prometheus_registry().is_some());
    }
}

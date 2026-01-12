use opentelemetry_otlp::WithExportConfig;
use tracing_subscriber::prelude::*;

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

    #[test]
    fn default_config_has_expected_defaults() {
        let config = ObservabilityConfig::default();
        assert_eq!(config.service_name, "gittree");
        assert!(config.otlp_endpoint.is_none());
        assert!(!config.log_json);
        assert!(config.metrics_enabled);
    }

    #[test]
    fn init_returns_handle() {
        let config = ObservabilityConfig::default();
        let handle = super::init(&config).expect("init");
        assert!(handle.prometheus_registry().is_some());
    }
}

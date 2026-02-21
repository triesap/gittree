use gittree_observability::{ObservabilityConfig, RelayCompatibilityMetrics, init};

fn capture_env(key: &str) -> Option<std::ffi::OsString> {
    std::env::var_os(key)
}

fn restore_env(key: &str, value: Option<std::ffi::OsString>) {
    match value {
        Some(value) => {
            // SAFETY: integration test process is single-purpose and restores captured values.
            unsafe { std::env::set_var(key, value) };
        }
        None => {
            // SAFETY: integration test process is single-purpose and restores captured values.
            unsafe { std::env::remove_var(key) };
        }
    }
}

#[test]
fn runtime_paths_cover_non_test_instantiations() {
    let keys = [
        "GITTREE_OTLP_ENDPOINT",
        "GITTREE_LOG_JSON",
        "GITTREE_LOG_DIR",
        "GITTREE_LOG_STDOUT",
        "GITTREE_METRICS_ENABLED",
    ];
    let previous: Vec<(&str, Option<std::ffi::OsString>)> =
        keys.iter().map(|key| (*key, capture_env(key))).collect();

    // SAFETY: integration test process is single-purpose and restores captured values.
    unsafe {
        std::env::set_var("GITTREE_OTLP_ENDPOINT", "http://localhost:4317");
        std::env::set_var("GITTREE_LOG_JSON", "true");
        std::env::set_var("GITTREE_LOG_DIR", "");
        std::env::set_var("GITTREE_LOG_STDOUT", "false");
        std::env::set_var("GITTREE_METRICS_ENABLED", "false");
    }

    let config = ObservabilityConfig::from_env("runtime-observability").expect("config");
    assert_eq!(
        config.otlp_endpoint.as_deref(),
        Some("http://localhost:4317")
    );
    assert!(config.log_json);
    assert!(!config.log_stdout);
    assert!(!config.metrics_enabled);
    assert!(config.log_dir.is_none());

    let metrics = RelayCompatibilityMetrics::new();
    metrics.record(true);
    metrics.record(false);

    let mut init_config = config;
    init_config.otlp_endpoint = None;
    let handle = init(&init_config).expect("init");
    assert!(handle.prometheus_registry().is_none());
    assert!(handle.log_guard().is_none());

    for (key, value) in previous {
        restore_env(key, value);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObservabilityConfig {
    pub service_name: String,
}

impl Default for ObservabilityConfig {
    fn default() -> Self {
        Self {
            service_name: "gittree".to_string(),
        }
    }
}

#[derive(Debug)]
pub enum ObservabilityError {
    InitFailed(String),
}

impl std::fmt::Display for ObservabilityError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ObservabilityError::InitFailed(message) => write!(f, "observability init failed: {message}"),
        }
    }
}

impl std::error::Error for ObservabilityError {}

pub fn init(_config: &ObservabilityConfig) -> Result<(), ObservabilityError> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::init;
    use super::ObservabilityConfig;

    #[test]
    fn default_config_has_service_name() {
        let config = ObservabilityConfig::default();
        assert_eq!(config.service_name, "gittree");
    }

    #[test]
    fn init_returns_ok() {
        let config = ObservabilityConfig::default();
        assert!(init(&config).is_ok());
    }
}

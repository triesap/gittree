use gittree_config::{ConfigError, ServicesConfig};
use gittree_observability::ObservabilityError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdmissionConfig {
    pub bind: String,
}

impl AdmissionConfig {
    pub fn from_env() -> Result<Self, ConfigError> {
        let services = ServicesConfig::from_env_validated()?;
        Ok(Self {
            bind: services.admission.bind,
        })
    }
}

#[derive(Debug)]
pub enum AdmissionError {
    Config(ConfigError),
    Observability(ObservabilityError),
}

impl std::fmt::Display for AdmissionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AdmissionError::Config(err) => write!(f, "admission config error: {err}"),
            AdmissionError::Observability(err) => {
                write!(f, "admission observability error: {err}")
            }
        }
    }
}

impl std::error::Error for AdmissionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            AdmissionError::Config(err) => Some(err),
            AdmissionError::Observability(err) => Some(err),
        }
    }
}

pub fn init_observability() -> Result<(), AdmissionError> {
    let config = gittree_observability::ObservabilityConfig {
        service_name: "gittree-admission".to_string(),
        ..gittree_observability::ObservabilityConfig::default()
    };
    gittree_observability::init(&config).map_err(AdmissionError::Observability)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::AdmissionConfig;
    use gittree_config::ServicesConfig;

    #[test]
    fn config_loads_from_env() {
        let config = AdmissionConfig::from_env().expect("config");
        let services = ServicesConfig::from_env_validated().expect("services");
        assert_eq!(config.bind, services.admission.bind);
    }
}

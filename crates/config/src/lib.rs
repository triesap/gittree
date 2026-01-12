#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GittreeConfig {
    pub relay_bind: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigError {
    InvalidRelayBind(String),
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConfigError::InvalidRelayBind(value) => {
                write!(f, "invalid relay bind address: {value}")
            }
        }
    }
}

impl std::error::Error for ConfigError {}

impl Default for GittreeConfig {
    fn default() -> Self {
        Self {
            relay_bind: "0.0.0.0:8080".to_string(),
        }
    }
}

impl GittreeConfig {
    pub fn from_env() -> Self {
        let relay_bind =
            std::env::var("GITTREE_RELAY_BIND").unwrap_or_else(|_| "0.0.0.0:8080".to_string());

        Self { relay_bind }
    }

    pub fn validate(&self) -> Result<(), ConfigError> {
        self.relay_bind
            .parse::<std::net::SocketAddr>()
            .map_err(|_| ConfigError::InvalidRelayBind(self.relay_bind.clone()))?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::GittreeConfig;
    use super::ConfigError;

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
    fn default_config_has_relay_bind() {
        let config = GittreeConfig::default();
        assert_eq!(config.relay_bind, "0.0.0.0:8080");
    }

    #[test]
    fn env_config_overrides_relay_bind() {
        with_env_var("GITTREE_RELAY_BIND", "127.0.0.1:9000", || {
            let config = GittreeConfig::from_env();
            assert_eq!(config.relay_bind, "127.0.0.1:9000");
        });
    }

    #[test]
    fn env_config_falls_back_to_default() {
        // SAFETY: this test controls the env var for its duration only.
        unsafe {
            std::env::remove_var("GITTREE_RELAY_BIND");
        }
        let config = GittreeConfig::from_env();
        assert_eq!(config.relay_bind, "0.0.0.0:8080");
    }

    #[test]
    fn validate_accepts_valid_relay_bind() {
        let config = GittreeConfig {
            relay_bind: "127.0.0.1:9000".to_string(),
        };
        assert_eq!(config.validate(), Ok(()));
    }

    #[test]
    fn validate_rejects_invalid_relay_bind() {
        let config = GittreeConfig {
            relay_bind: "not-an-addr".to_string(),
        };
        assert_eq!(
            config.validate(),
            Err(ConfigError::InvalidRelayBind("not-an-addr".to_string()))
        );
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GittreeConfig {
    pub relay_bind: String,
}

const DEFAULT_RELAY_BIND: &str = "0.0.0.0:8080";
const ENV_RELAY_BIND: &str = "GITTREE_RELAY_BIND";

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
            relay_bind: DEFAULT_RELAY_BIND.to_string(),
        }
    }
}

impl GittreeConfig {
    pub fn from_env() -> Self {
        let relay_bind =
            std::env::var(ENV_RELAY_BIND).unwrap_or_else(|_| DEFAULT_RELAY_BIND.to_string());

        Self { relay_bind }
    }

    pub fn validate(&self) -> Result<(), ConfigError> {
        self.relay_bind
            .parse::<std::net::SocketAddr>()
            .map_err(|_| ConfigError::InvalidRelayBind(self.relay_bind.clone()))?;

        Ok(())
    }

    pub fn relay_bind_addr(&self) -> Result<std::net::SocketAddr, ConfigError> {
        self.relay_bind
            .parse::<std::net::SocketAddr>()
            .map_err(|_| ConfigError::InvalidRelayBind(self.relay_bind.clone()))
    }

    pub fn relay_bind_ip(&self) -> Result<std::net::IpAddr, ConfigError> {
        self.relay_bind_addr().map(|addr| addr.ip())
    }

    pub fn relay_bind_port(&self) -> Result<u16, ConfigError> {
        self.relay_bind_addr().map(|addr| addr.port())
    }

    pub fn from_env_validated() -> Result<Self, ConfigError> {
        let config = Self::from_env();
        config.validate()?;
        Ok(config)
    }
}

#[cfg(test)]
mod tests {
    use super::ConfigError;
    use super::GittreeConfig;
    use crate::DEFAULT_RELAY_BIND;
    use crate::ENV_RELAY_BIND;

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
        with_env_var(ENV_RELAY_BIND, "127.0.0.1:9000", || {
            let config = GittreeConfig::from_env();
            assert_eq!(config.relay_bind, "127.0.0.1:9000");
        });
    }

    #[test]
    fn env_config_falls_back_to_default() {
        // SAFETY: this test controls the env var for its duration only.
        unsafe {
            std::env::remove_var(ENV_RELAY_BIND);
        }
        let config = GittreeConfig::from_env();
        assert_eq!(config.relay_bind, DEFAULT_RELAY_BIND);
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

    #[test]
    fn relay_bind_addr_parses_socket_addr() {
        let config = GittreeConfig {
            relay_bind: "127.0.0.1:9000".to_string(),
        };
        let addr = config.relay_bind_addr().expect("valid socket addr");
        assert_eq!(addr, "127.0.0.1:9000".parse().expect("parse addr"));
    }

    #[test]
    fn relay_bind_addr_reports_invalid_bind() {
        let config = GittreeConfig {
            relay_bind: "bad".to_string(),
        };
        assert_eq!(
            config.relay_bind_addr(),
            Err(ConfigError::InvalidRelayBind("bad".to_string()))
        );
    }

    #[test]
    fn relay_bind_ip_returns_ip() {
        let config = GittreeConfig {
            relay_bind: "127.0.0.1:9100".to_string(),
        };
        let ip = config.relay_bind_ip().expect("valid ip");
        assert_eq!(
            ip,
            "127.0.0.1".parse::<std::net::IpAddr>().expect("parse ip")
        );
    }

    #[test]
    fn relay_bind_port_returns_port() {
        let config = GittreeConfig {
            relay_bind: "127.0.0.1:9100".to_string(),
        };
        let port = config.relay_bind_port().expect("valid port");
        assert_eq!(port, 9100);
    }

    #[test]
    fn from_env_validated_returns_error_for_invalid_bind() {
        with_env_var(ENV_RELAY_BIND, "bad:addr", || {
            let result = GittreeConfig::from_env_validated();
            assert_eq!(
                result,
                Err(ConfigError::InvalidRelayBind("bad:addr".to_string()))
            );
        });
    }

    #[test]
    fn from_env_validated_accepts_valid_bind() {
        with_env_var(ENV_RELAY_BIND, "0.0.0.0:7000", || {
            let config = GittreeConfig::from_env_validated();
            assert!(matches!(
                config,
                Ok(GittreeConfig {
                    relay_bind,
                }) if relay_bind == "0.0.0.0:7000"
            ));
        });
    }
}

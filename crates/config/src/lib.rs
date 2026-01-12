#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GittreeConfig {
    pub relay_bind: String,
}

impl Default for GittreeConfig {
    fn default() -> Self {
        Self {
            relay_bind: "0.0.0.0:8080".to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::GittreeConfig;

    #[test]
    fn default_config_has_relay_bind() {
        let config = GittreeConfig::default();
        assert_eq!(config.relay_bind, "0.0.0.0:8080");
    }
}

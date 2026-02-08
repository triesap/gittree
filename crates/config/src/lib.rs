#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GittreeConfig {
    pub relay_bind: String,
}

const DEFAULT_RELAY_BIND: &str = "0.0.0.0:8080";
const DEFAULT_ADMISSION_BIND: &str = "127.0.0.1:8081";
const DEFAULT_STATE_BIND: &str = "127.0.0.1:8082";
const DEFAULT_COORDINATOR_BIND: &str = "127.0.0.1:8083";
const DEFAULT_SYNC_BIND: &str = "127.0.0.1:8084";
const DEFAULT_GIT_HTTP_BIND: &str = "127.0.0.1:8085";
const DEFAULT_UI_BIND: &str = "127.0.0.1:8086";
const DEFAULT_WEBHOOK_BIND: &str = "127.0.0.1:8087";
const DEFAULT_CONTROL_BIND: &str = "127.0.0.1:8088";
const DEFAULT_AUTH_BIND: &str = "127.0.0.1:8089";
const DEFAULT_UI_AUTH_URL: &str = "http://localhost:8089";
const DEFAULT_UI_APP_URL: &str = "http://localhost:8090";
const DEFAULT_UI_CONTROL_URL: &str = "http://localhost:8088";
const ENV_RELAY_BIND: &str = "GITTREE_RELAY_BIND";
const ENV_RELAY_URLS: &str = "GITTREE_RELAY_URLS";
const ENV_RELAY_COMPAT_MODE: &str = "GITTREE_RELAY_COMPAT_MODE";
const ENV_RELAY_PROBE_ACTIVE: &str = "GITTREE_RELAY_PROBE_ACTIVE";
const ENV_RELAY_PROBE_TIMEOUT_SECS: &str = "GITTREE_RELAY_PROBE_TIMEOUT_SECS";
const ENV_RELAY_PROBE_SECRET_KEY: &str = "GITTREE_RELAY_PROBE_SECRET_KEY";
const ENV_RELAY_POLICY_MAX_CONTENT_LEN: &str = "GITTREE_RELAY_POLICY_MAX_CONTENT_LEN";
const ENV_RELAY_POLICY_MAX_TAGS: &str = "GITTREE_RELAY_POLICY_MAX_TAGS";
const ENV_RELAY_POLICY_MAX_TAG_VALUES: &str = "GITTREE_RELAY_POLICY_MAX_TAG_VALUES";
const ENV_RELAY_POLICY_MAX_TAG_VALUE_LEN: &str = "GITTREE_RELAY_POLICY_MAX_TAG_VALUE_LEN";
const ENV_RELAY_POLICY_MAX_FUTURE_SECS: &str = "GITTREE_RELAY_POLICY_MAX_FUTURE_SECS";
const ENV_RELAY_POLICY_MAX_SUBSCRIPTIONS: &str = "GITTREE_RELAY_POLICY_MAX_SUBSCRIPTIONS";
const ENV_RELAY_POLICY_MAX_LIMIT: &str = "GITTREE_RELAY_POLICY_MAX_LIMIT";
const ENV_RELAY_POLICY_MAX_MESSAGE_BYTES: &str = "GITTREE_RELAY_POLICY_MAX_MESSAGE_BYTES";
const ENV_RELAY_POLICY_MAX_EVENTS_PER_MIN: &str = "GITTREE_RELAY_POLICY_MAX_EVENTS_PER_MIN";
const ENV_RELAY_POLICY_MAX_REQUESTS_PER_MIN: &str = "GITTREE_RELAY_POLICY_MAX_REQUESTS_PER_MIN";
const ENV_RELAY_POLICY_RETENTION_MAX_AGE_SECS: &str =
    "GITTREE_RELAY_POLICY_RETENTION_MAX_AGE_SECS";
const ENV_RELAY_POLICY_AUTH_REQUIRED: &str = "GITTREE_RELAY_POLICY_AUTH_REQUIRED";
const ENV_ADMISSION_BIND: &str = "GITTREE_ADMISSION_BIND";
const ENV_STATE_BIND: &str = "GITTREE_STATE_BIND";
const ENV_COORDINATOR_BIND: &str = "GITTREE_COORDINATOR_BIND";
const ENV_SYNC_BIND: &str = "GITTREE_SYNC_BIND";
const ENV_GIT_HTTP_BIND: &str = "GITTREE_GIT_HTTP_BIND";
const ENV_UI_BIND: &str = "GITTREE_UI_BIND";
const ENV_WEBHOOK_BIND: &str = "GITTREE_WEBHOOK_BIND";
const ENV_CONTROL_BIND: &str = "GITTREE_CONTROL_BIND";
const ENV_AUTH_BIND: &str = "GITTREE_AUTH_BIND";
const ENV_FORGEJO_BASE_URL: &str = "GITTREE_FORGEJO_BASE_URL";
const ENV_FORGEJO_API_TOKEN: &str = "GITTREE_FORGEJO_API_TOKEN";
const ENV_FORGEJO_OWNER: &str = "GITTREE_FORGEJO_OWNER";
const ENV_FORGEJO_WEBHOOK_URL: &str = "GITTREE_FORGEJO_WEBHOOK_URL";
const ENV_FORGEJO_WEBHOOK_SECRET: &str = "GITTREE_FORGEJO_WEBHOOK_SECRET";
const ENV_FORGEJO_REPO_PRIVATE: &str = "GITTREE_FORGEJO_REPO_PRIVATE";
const ENV_UI_REPO_ROOT: &str = "GITTREE_UI_REPO_ROOT";
const ENV_UI_PUBLIC_GIT_URL: &str = "GITTREE_UI_PUBLIC_GIT_URL";
const ENV_UI_AUTH_URL: &str = "GITTREE_UI_AUTH_URL";
const ENV_UI_APP_URL: &str = "GITTREE_UI_APP_URL";
const ENV_UI_CONTROL_URL: &str = "GITTREE_UI_CONTROL_URL";
const ENV_CONTROL_TOKEN: &str = "GITTREE_CONTROL_TOKEN";
const ENV_CONTROL_ADMIN_KEYS: &str = "GITTREE_CONTROL_ADMIN_KEYS";
const ENV_AUTH_EMAIL_DOMAIN: &str = "GITTREE_AUTH_EMAIL_DOMAIN";
const ENV_AUTH_MAX_SKEW_SECONDS: &str = "GITTREE_AUTH_MAX_SKEW_SECONDS";

const DEFAULT_RELAY_COMPAT_MODE: RelayCompatibilityMode = RelayCompatibilityMode::Strict;
const DEFAULT_RELAY_PROBE_TIMEOUT_SECS: u64 = 5;
const DEFAULT_RELAY_POLICY_MAX_CONTENT_LEN: u64 = 8_192;
const DEFAULT_RELAY_POLICY_MAX_TAGS: u64 = 128;
const DEFAULT_RELAY_POLICY_MAX_TAG_VALUES: u64 = 16;
const DEFAULT_RELAY_POLICY_MAX_TAG_VALUE_LEN: u64 = 512;
const DEFAULT_RELAY_POLICY_MAX_FUTURE_SECS: u64 = 60;
const DEFAULT_AUTH_EMAIL_DOMAIN: &str = "local.test";
const DEFAULT_AUTH_MAX_SKEW_SECS: u64 = 60;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceConfig {
    pub bind: String,
}

impl ServiceConfig {
    pub fn new(bind: impl Into<String>) -> Self {
        Self { bind: bind.into() }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServicesConfig {
    pub relay: ServiceConfig,
    pub admission: ServiceConfig,
    pub state: ServiceConfig,
    pub coordinator: ServiceConfig,
    pub sync: ServiceConfig,
    pub git_http: ServiceConfig,
    pub ui: ServiceConfig,
    pub webhook: ServiceConfig,
    pub control: ServiceConfig,
    pub auth: ServiceConfig,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelayTargetsConfig {
    pub relay_urls: Vec<String>,
}

impl RelayTargetsConfig {
    pub fn from_env() -> Self {
        let relay_urls = parse_relay_urls(std::env::var(ENV_RELAY_URLS).unwrap_or_default());
        Self { relay_urls }
    }

    pub fn from_env_validated() -> Result<Self, ConfigError> {
        let config = Self::from_env();
        config.validate()?;
        Ok(config)
    }

    pub fn from_toml_str(input: &str) -> Result<Self, ConfigError> {
        let parsed: TomlRelayTargets = toml::from_str(input)
            .map_err(|source| ConfigError::TomlParse { path: None, source })?;
        let relay_urls = parsed.relay_urls.unwrap_or_default();
        let config = Self { relay_urls };
        config.validate()?;
        Ok(config)
    }

    pub fn from_toml_file(path: impl AsRef<std::path::Path>) -> Result<Self, ConfigError> {
        let path = path.as_ref();
        let contents = std::fs::read_to_string(path).map_err(|source| ConfigError::ReadConfig {
            path: path.to_path_buf(),
            source,
        })?;
        Self::from_toml_str(&contents).map_err(|err| err.with_path(path))
    }

    pub fn validate(&self) -> Result<(), ConfigError> {
        for url in &self.relay_urls {
            validate_relay_url(url)?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForgejoConfig {
    pub base_url: String,
    pub api_token: String,
    pub owner: String,
    pub webhook_url: String,
    pub webhook_secret: String,
    pub repo_private: bool,
}

impl ForgejoConfig {
    pub fn from_env() -> Result<Self, ConfigError> {
        let base_url = env_required_string(ENV_FORGEJO_BASE_URL)?;
        let api_token = env_required_string(ENV_FORGEJO_API_TOKEN)?;
        let owner = env_required_string(ENV_FORGEJO_OWNER)?;
        let webhook_url = env_required_string(ENV_FORGEJO_WEBHOOK_URL)?;
        let webhook_secret = env_required_string(ENV_FORGEJO_WEBHOOK_SECRET)?;
        let repo_private = env_bool_default(ENV_FORGEJO_REPO_PRIVATE, true)?;
        let config = Self {
            base_url,
            api_token,
            owner,
            webhook_url,
            webhook_secret,
            repo_private,
        };
        config.validate()?;
        Ok(config)
    }

    pub fn from_toml_str(input: &str) -> Result<Self, ConfigError> {
        let parsed: TomlForgejoRoot = toml::from_str(input)
            .map_err(|source| ConfigError::TomlParse { path: None, source })?;
        let config = parsed.into_config()?;
        config.validate()?;
        Ok(config)
    }

    pub fn from_toml_file(path: impl AsRef<std::path::Path>) -> Result<Self, ConfigError> {
        let path = path.as_ref();
        let contents = std::fs::read_to_string(path).map_err(|source| ConfigError::ReadConfig {
            path: path.to_path_buf(),
            source,
        })?;
        Self::from_toml_str(&contents).map_err(|err| err.with_path(path))
    }

    pub fn validate(&self) -> Result<(), ConfigError> {
        validate_http_url("forgejo.base_url", &self.base_url)?;
        validate_http_url("forgejo.webhook_url", &self.webhook_url)?;
        if self.api_token.trim().is_empty() {
            return Err(ConfigError::InvalidConfig {
                field: "forgejo.api_token",
                value: self.api_token.clone(),
            });
        }
        if self.owner.trim().is_empty() {
            return Err(ConfigError::InvalidConfig {
                field: "forgejo.owner",
                value: self.owner.clone(),
            });
        }
        if self.webhook_secret.trim().is_empty() {
            return Err(ConfigError::InvalidConfig {
                field: "forgejo.webhook_secret",
                value: self.webhook_secret.clone(),
            });
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UiConfig {
    pub repo_root: std::path::PathBuf,
    pub public_git_url: String,
    pub auth_url: String,
    pub app_url: String,
    pub control_url: String,
}

impl UiConfig {
    pub fn from_env() -> Result<Self, ConfigError> {
        let repo_root = env_required_path(ENV_UI_REPO_ROOT)?;
        let public_git_url = env_required_string(ENV_UI_PUBLIC_GIT_URL)?;
        let auth_url = env_optional_string(ENV_UI_AUTH_URL)
            .unwrap_or_else(|| DEFAULT_UI_AUTH_URL.to_string());
        let app_url = env_optional_string(ENV_UI_APP_URL)
            .unwrap_or_else(|| DEFAULT_UI_APP_URL.to_string());
        let control_url = env_optional_string(ENV_UI_CONTROL_URL)
            .unwrap_or_else(|| DEFAULT_UI_CONTROL_URL.to_string());
        let config = Self {
            repo_root,
            public_git_url,
            auth_url,
            app_url,
            control_url,
        };
        config.validate()?;
        Ok(config)
    }

    pub fn from_toml_str(input: &str) -> Result<Self, ConfigError> {
        let parsed: TomlUiRoot = toml::from_str(input)
            .map_err(|source| ConfigError::TomlParse { path: None, source })?;
        let config = parsed.into_config()?;
        config.validate()?;
        Ok(config)
    }

    pub fn from_toml_file(path: impl AsRef<std::path::Path>) -> Result<Self, ConfigError> {
        let path = path.as_ref();
        let contents = std::fs::read_to_string(path).map_err(|source| ConfigError::ReadConfig {
            path: path.to_path_buf(),
            source,
        })?;
        Self::from_toml_str(&contents).map_err(|err| err.with_path(path))
    }

    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.repo_root.as_os_str().is_empty() {
            return Err(ConfigError::InvalidConfig {
                field: "ui.repo_root",
                value: "".to_string(),
            });
        }
        validate_http_url("ui.public_git_url", &self.public_git_url)?;
        validate_http_url("ui.auth_url", &self.auth_url)?;
        validate_http_url("ui.app_url", &self.app_url)?;
        validate_http_url("ui.control_url", &self.control_url)?;
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControlAuthConfig {
    pub token: String,
    pub admin_keys: Vec<String>,
}

impl ControlAuthConfig {
    pub fn from_env() -> Result<Self, ConfigError> {
        let token = env_required_string(ENV_CONTROL_TOKEN)?;
        let admin_keys = env_optional_string(ENV_CONTROL_ADMIN_KEYS)
            .map(parse_csv_values)
            .unwrap_or_default();
        let config = Self { token, admin_keys };
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.token.trim().is_empty() {
            return Err(ConfigError::InvalidConfig {
                field: "control.token",
                value: self.token.clone(),
            });
        }
        if self.admin_keys.iter().any(|key| key.trim().is_empty()) {
            return Err(ConfigError::InvalidConfig {
                field: "control.admin_keys",
                value: "empty key".to_string(),
            });
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthConfig {
    pub email_domain: String,
    pub max_skew_seconds: u64,
}

impl AuthConfig {
    pub fn from_env() -> Result<Self, ConfigError> {
        let email_domain = env_optional_string(ENV_AUTH_EMAIL_DOMAIN)
            .unwrap_or_else(|| DEFAULT_AUTH_EMAIL_DOMAIN.to_string());
        let max_skew_seconds = match env_optional_string(ENV_AUTH_MAX_SKEW_SECONDS) {
            Some(value) => value.parse::<u64>().map_err(|_| ConfigError::InvalidConfig {
                field: "auth.max_skew_seconds",
                value,
            })?,
            None => DEFAULT_AUTH_MAX_SKEW_SECS,
        };
        let config = Self {
            email_domain,
            max_skew_seconds,
        };
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.email_domain.trim().is_empty() {
            return Err(ConfigError::InvalidConfig {
                field: "auth.email_domain",
                value: self.email_domain.clone(),
            });
        }
        if self.max_skew_seconds == 0 {
            return Err(ConfigError::InvalidConfig {
                field: "auth.max_skew_seconds",
                value: self.max_skew_seconds.to_string(),
            });
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelayCompatibilityMode {
    Strict,
    Warn,
    Allow,
}

impl RelayCompatibilityMode {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "strict" => Some(Self::Strict),
            "warn" | "warning" => Some(Self::Warn),
            "allow" => Some(Self::Allow),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            RelayCompatibilityMode::Strict => "strict",
            RelayCompatibilityMode::Warn => "warn",
            RelayCompatibilityMode::Allow => "allow",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelayCompatibilityConfig {
    pub mode: RelayCompatibilityMode,
}

impl Default for RelayCompatibilityConfig {
    fn default() -> Self {
        Self {
            mode: DEFAULT_RELAY_COMPAT_MODE,
        }
    }
}

impl RelayCompatibilityConfig {
    pub fn from_env() -> Result<Self, ConfigError> {
        let mode = env_or_default(ENV_RELAY_COMPAT_MODE, DEFAULT_RELAY_COMPAT_MODE.as_str());
        let mode = RelayCompatibilityMode::parse(&mode)
            .ok_or_else(|| ConfigError::InvalidRelayCompatibilityMode(mode))?;
        Ok(Self { mode })
    }

    pub fn from_toml_str(input: &str) -> Result<Self, ConfigError> {
        let parsed: TomlRelayCompatibilityRoot = toml::from_str(input)
            .map_err(|source| ConfigError::TomlParse { path: None, source })?;
        parsed.into_config()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelayProbeConfig {
    pub active: bool,
    pub timeout_secs: u64,
    pub secret_key: Option<String>,
}

impl Default for RelayProbeConfig {
    fn default() -> Self {
        Self {
            active: false,
            timeout_secs: DEFAULT_RELAY_PROBE_TIMEOUT_SECS,
            secret_key: None,
        }
    }
}

impl RelayProbeConfig {
    pub fn from_env() -> Result<Self, ConfigError> {
        let active = env_bool(ENV_RELAY_PROBE_ACTIVE)?.unwrap_or(false);
        let timeout_secs =
            env_u64(ENV_RELAY_PROBE_TIMEOUT_SECS)?.unwrap_or(DEFAULT_RELAY_PROBE_TIMEOUT_SECS);
        let secret_key = env_optional_string(ENV_RELAY_PROBE_SECRET_KEY);
        let config = Self {
            active,
            timeout_secs,
            secret_key,
        };
        config.validate()?;
        Ok(config)
    }

    pub fn from_toml_str(input: &str) -> Result<Self, ConfigError> {
        let parsed: TomlRelayProbeRoot = toml::from_str(input)
            .map_err(|source| ConfigError::TomlParse { path: None, source })?;
        let config = parsed.into_config();
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.timeout_secs == 0 {
            return Err(ConfigError::InvalidRelayProbeConfig {
                field: "relay_probe.timeout_secs",
                value: "0".to_string(),
            });
        }
        if let Some(secret) = &self.secret_key {
            if !is_hex_len(secret, 64) {
                return Err(ConfigError::InvalidRelayProbeConfig {
                    field: "relay_probe.secret_key",
                    value: secret.clone(),
                });
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelayPolicyConfig {
    pub max_content_len: u64,
    pub max_tags: u64,
    pub max_tag_values: u64,
    pub max_tag_value_len: u64,
    pub max_future_seconds: u64,
    pub max_subscriptions: Option<u64>,
    pub max_limit: Option<u64>,
    pub max_message_bytes: Option<u64>,
    pub max_events_per_min: Option<u64>,
    pub max_requests_per_min: Option<u64>,
    pub retention_max_age_seconds: Option<u64>,
    pub auth_required: bool,
}

impl Default for RelayPolicyConfig {
    fn default() -> Self {
        Self {
            max_content_len: DEFAULT_RELAY_POLICY_MAX_CONTENT_LEN,
            max_tags: DEFAULT_RELAY_POLICY_MAX_TAGS,
            max_tag_values: DEFAULT_RELAY_POLICY_MAX_TAG_VALUES,
            max_tag_value_len: DEFAULT_RELAY_POLICY_MAX_TAG_VALUE_LEN,
            max_future_seconds: DEFAULT_RELAY_POLICY_MAX_FUTURE_SECS,
            max_subscriptions: None,
            max_limit: None,
            max_message_bytes: None,
            max_events_per_min: None,
            max_requests_per_min: None,
            retention_max_age_seconds: None,
            auth_required: false,
        }
    }
}

impl RelayPolicyConfig {
    pub fn from_env() -> Result<Self, ConfigError> {
        let max_content_len = env_u64_policy(
            ENV_RELAY_POLICY_MAX_CONTENT_LEN,
            "relay_policy.max_content_len",
        )?
        .unwrap_or(DEFAULT_RELAY_POLICY_MAX_CONTENT_LEN);
        let max_tags =
            env_u64_policy(ENV_RELAY_POLICY_MAX_TAGS, "relay_policy.max_tags")?
                .unwrap_or(DEFAULT_RELAY_POLICY_MAX_TAGS);
        let max_tag_values = env_u64_policy(
            ENV_RELAY_POLICY_MAX_TAG_VALUES,
            "relay_policy.max_tag_values",
        )?
        .unwrap_or(DEFAULT_RELAY_POLICY_MAX_TAG_VALUES);
        let max_tag_value_len = env_u64_policy(
            ENV_RELAY_POLICY_MAX_TAG_VALUE_LEN,
            "relay_policy.max_tag_value_len",
        )?
        .unwrap_or(DEFAULT_RELAY_POLICY_MAX_TAG_VALUE_LEN);
        let max_future_seconds = env_u64_policy(
            ENV_RELAY_POLICY_MAX_FUTURE_SECS,
            "relay_policy.max_future_seconds",
        )?
        .unwrap_or(DEFAULT_RELAY_POLICY_MAX_FUTURE_SECS);
        let max_subscriptions = env_u64_policy(
            ENV_RELAY_POLICY_MAX_SUBSCRIPTIONS,
            "relay_policy.max_subscriptions",
        )?;
        let max_limit = env_u64_policy(ENV_RELAY_POLICY_MAX_LIMIT, "relay_policy.max_limit")?;
        let max_message_bytes = env_u64_policy(
            ENV_RELAY_POLICY_MAX_MESSAGE_BYTES,
            "relay_policy.max_message_bytes",
        )?;
        let max_events_per_min = env_u64_policy(
            ENV_RELAY_POLICY_MAX_EVENTS_PER_MIN,
            "relay_policy.max_events_per_min",
        )?;
        let max_requests_per_min = env_u64_policy(
            ENV_RELAY_POLICY_MAX_REQUESTS_PER_MIN,
            "relay_policy.max_requests_per_min",
        )?;
        let retention_max_age_seconds = env_u64_policy(
            ENV_RELAY_POLICY_RETENTION_MAX_AGE_SECS,
            "relay_policy.retention_max_age_seconds",
        )?;
        let auth_required = env_bool_policy(
            ENV_RELAY_POLICY_AUTH_REQUIRED,
            "relay_policy.auth_required",
        )?
        .unwrap_or(false);

        let config = Self {
            max_content_len,
            max_tags,
            max_tag_values,
            max_tag_value_len,
            max_future_seconds,
            max_subscriptions,
            max_limit,
            max_message_bytes,
            max_events_per_min,
            max_requests_per_min,
            retention_max_age_seconds,
            auth_required,
        };
        config.validate()?;
        Ok(config)
    }

    pub fn from_toml_str(input: &str) -> Result<Self, ConfigError> {
        let parsed: TomlRelayPolicyRoot = toml::from_str(input)
            .map_err(|source| ConfigError::TomlParse { path: None, source })?;
        let config = parsed.into_config();
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<(), ConfigError> {
        validate_policy_limit("relay_policy.max_content_len", self.max_content_len)?;
        validate_policy_limit("relay_policy.max_tags", self.max_tags)?;
        validate_policy_limit("relay_policy.max_tag_values", self.max_tag_values)?;
        validate_policy_limit("relay_policy.max_tag_value_len", self.max_tag_value_len)?;
        validate_policy_limit("relay_policy.max_future_seconds", self.max_future_seconds)?;
        if let Some(limit) = self.max_subscriptions {
            validate_policy_limit("relay_policy.max_subscriptions", limit)?;
        }
        if let Some(limit) = self.max_limit {
            validate_policy_limit("relay_policy.max_limit", limit)?;
        }
        if let Some(limit) = self.max_message_bytes {
            validate_policy_limit("relay_policy.max_message_bytes", limit)?;
        }
        if let Some(limit) = self.max_events_per_min {
            validate_policy_limit("relay_policy.max_events_per_min", limit)?;
        }
        if let Some(limit) = self.max_requests_per_min {
            validate_policy_limit("relay_policy.max_requests_per_min", limit)?;
        }
        if let Some(limit) = self.retention_max_age_seconds {
            validate_policy_limit("relay_policy.retention_max_age_seconds", limit)?;
        }
        Ok(())
    }
}

impl Default for ServicesConfig {
    fn default() -> Self {
        Self {
            relay: ServiceConfig::new(DEFAULT_RELAY_BIND),
            admission: ServiceConfig::new(DEFAULT_ADMISSION_BIND),
            state: ServiceConfig::new(DEFAULT_STATE_BIND),
            coordinator: ServiceConfig::new(DEFAULT_COORDINATOR_BIND),
            sync: ServiceConfig::new(DEFAULT_SYNC_BIND),
            git_http: ServiceConfig::new(DEFAULT_GIT_HTTP_BIND),
            ui: ServiceConfig::new(DEFAULT_UI_BIND),
            webhook: ServiceConfig::new(DEFAULT_WEBHOOK_BIND),
            control: ServiceConfig::new(DEFAULT_CONTROL_BIND),
            auth: ServiceConfig::new(DEFAULT_AUTH_BIND),
        }
    }
}

impl ServicesConfig {
    pub fn from_env() -> Self {
        Self {
            relay: ServiceConfig::new(env_or_default(ENV_RELAY_BIND, DEFAULT_RELAY_BIND)),
            admission: ServiceConfig::new(env_or_default(
                ENV_ADMISSION_BIND,
                DEFAULT_ADMISSION_BIND,
            )),
            state: ServiceConfig::new(env_or_default(ENV_STATE_BIND, DEFAULT_STATE_BIND)),
            coordinator: ServiceConfig::new(env_or_default(
                ENV_COORDINATOR_BIND,
                DEFAULT_COORDINATOR_BIND,
            )),
            sync: ServiceConfig::new(env_or_default(ENV_SYNC_BIND, DEFAULT_SYNC_BIND)),
            git_http: ServiceConfig::new(env_or_default(ENV_GIT_HTTP_BIND, DEFAULT_GIT_HTTP_BIND)),
            ui: ServiceConfig::new(env_or_default(ENV_UI_BIND, DEFAULT_UI_BIND)),
            webhook: ServiceConfig::new(env_or_default(ENV_WEBHOOK_BIND, DEFAULT_WEBHOOK_BIND)),
            control: ServiceConfig::new(env_or_default(ENV_CONTROL_BIND, DEFAULT_CONTROL_BIND)),
            auth: ServiceConfig::new(env_or_default(ENV_AUTH_BIND, DEFAULT_AUTH_BIND)),
        }
    }

    pub fn from_toml_str(input: &str) -> Result<Self, ConfigError> {
        let parsed: TomlServicesRoot = toml::from_str(input)
            .map_err(|source| ConfigError::TomlParse { path: None, source })?;
        Ok(parsed.into_services())
    }

    pub fn from_toml_file(path: impl AsRef<std::path::Path>) -> Result<Self, ConfigError> {
        let path = path.as_ref();
        let contents = std::fs::read_to_string(path).map_err(|source| ConfigError::ReadConfig {
            path: path.to_path_buf(),
            source,
        })?;
        Self::from_toml_str(&contents).map_err(|err| err.with_path(path))
    }

    pub fn validate(&self) -> Result<(), ConfigError> {
        validate_service_bind("relay", &self.relay.bind)?;
        validate_service_bind("admission", &self.admission.bind)?;
        validate_service_bind("state", &self.state.bind)?;
        validate_service_bind("coordinator", &self.coordinator.bind)?;
        validate_service_bind("sync", &self.sync.bind)?;
        validate_service_bind("git_http", &self.git_http.bind)?;
        validate_service_bind("ui", &self.ui.bind)?;
        validate_service_bind("webhook", &self.webhook.bind)?;
        validate_service_bind("control", &self.control.bind)?;
        validate_service_bind("auth", &self.auth.bind)?;
        Ok(())
    }

    pub fn from_env_validated() -> Result<Self, ConfigError> {
        let config = Self::from_env();
        config.validate()?;
        Ok(config)
    }

    pub fn from_toml_file_validated(
        path: impl AsRef<std::path::Path>,
    ) -> Result<Self, ConfigError> {
        let config = Self::from_toml_file(path)?;
        config.validate()?;
        Ok(config)
    }
}

fn env_or_default(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

fn env_required_string(key: &'static str) -> Result<String, ConfigError> {
    let value = std::env::var(key).map_err(|_| ConfigError::MissingEnv(key))?;
    if value.trim().is_empty() {
        return Err(ConfigError::MissingEnv(key));
    }
    Ok(value)
}

fn env_required_path(key: &'static str) -> Result<std::path::PathBuf, ConfigError> {
    env_required_string(key).map(std::path::PathBuf::from)
}

fn env_bool_default(key: &'static str, default: bool) -> Result<bool, ConfigError> {
    match std::env::var(key) {
        Ok(value) => {
            if value.trim().is_empty() {
                return Ok(default);
            }
            parse_bool(&value).ok_or_else(|| ConfigError::InvalidConfig { field: key, value })
        }
        Err(_) => Ok(default),
    }
}

fn env_optional_string(key: &str) -> Option<String> {
    match std::env::var(key) {
        Ok(value) if value.trim().is_empty() => None,
        Ok(value) => Some(value),
        Err(_) => None,
    }
}

fn env_bool(key: &'static str) -> Result<Option<bool>, ConfigError> {
    match std::env::var(key) {
        Ok(value) => {
            if value.trim().is_empty() {
                return Ok(None);
            }
            parse_bool(&value)
                .map(Some)
                .ok_or_else(|| ConfigError::InvalidRelayProbeConfig { field: key, value })
        }
        Err(_) => Ok(None),
    }
}

fn env_u64(key: &'static str) -> Result<Option<u64>, ConfigError> {
    match std::env::var(key) {
        Ok(value) => {
            if value.trim().is_empty() {
                return Ok(None);
            }
            value.parse::<u64>().map(Some).map_err(|_| {
                ConfigError::InvalidRelayProbeConfig {
                    field: key,
                    value,
                }
            })
        }
        Err(_) => Ok(None),
    }
}

fn env_bool_policy(
    key: &'static str,
    field: &'static str,
) -> Result<Option<bool>, ConfigError> {
    match std::env::var(key) {
        Ok(value) => {
            if value.trim().is_empty() {
                return Ok(None);
            }
            parse_bool(&value).map(Some).ok_or_else(|| {
                ConfigError::InvalidRelayPolicyConfig {
                    field,
                    value,
                }
            })
        }
        Err(_) => Ok(None),
    }
}

fn env_u64_policy(key: &'static str, field: &'static str) -> Result<Option<u64>, ConfigError> {
    match std::env::var(key) {
        Ok(value) => {
            if value.trim().is_empty() {
                return Ok(None);
            }
            value.parse::<u64>().map(Some).map_err(|_| {
                ConfigError::InvalidRelayPolicyConfig {
                    field,
                    value,
                }
            })
        }
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

fn is_hex_len(value: &str, len: usize) -> bool {
    value.len() == len && value.chars().all(|c| c.is_ascii_hexdigit())
}

fn validate_service_bind(service: &'static str, value: &str) -> Result<(), ConfigError> {
    value
        .parse::<std::net::SocketAddr>()
        .map_err(|_| ConfigError::InvalidServiceBind {
            service,
            value: value.to_string(),
        })?;
    Ok(())
}

fn parse_relay_urls(raw: String) -> Vec<String> {
    raw.split(',')
        .map(|entry| entry.trim().to_string())
        .filter(|entry| !entry.is_empty())
        .collect()
}

fn parse_csv_values(raw: String) -> Vec<String> {
    raw.split(',')
        .map(|entry| entry.trim().to_string())
        .filter(|entry| !entry.is_empty())
        .collect()
}

fn validate_relay_url(value: &str) -> Result<(), ConfigError> {
    let parsed = url::Url::parse(value)
        .map_err(|_| ConfigError::InvalidRelayUrl(value.to_string()))?;
    match parsed.scheme() {
        "ws" | "wss" | "http" | "https" => Ok(()),
        _ => Err(ConfigError::InvalidRelayUrl(value.to_string())),
    }
}

fn validate_http_url(field: &'static str, value: &str) -> Result<(), ConfigError> {
    let parsed = url::Url::parse(value)
        .map_err(|_| ConfigError::InvalidConfig { field, value: value.to_string() })?;
    match parsed.scheme() {
        "http" | "https" => Ok(()),
        _ => Err(ConfigError::InvalidConfig {
            field,
            value: value.to_string(),
        }),
    }
}

fn require_toml_field<T>(value: Option<T>, field: &'static str) -> Result<T, ConfigError> {
    value.ok_or_else(|| ConfigError::InvalidConfig {
        field,
        value: "missing".to_string(),
    })
}

fn validate_policy_limit(field: &'static str, value: u64) -> Result<(), ConfigError> {
    if value == 0 {
        return Err(ConfigError::InvalidRelayPolicyConfig {
            field,
            value: value.to_string(),
        });
    }
    Ok(())
}

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct TomlConfig {
    relay_bind: Option<String>,
}

impl TomlConfig {
    fn into_config(self) -> GittreeConfig {
        GittreeConfig {
            relay_bind: self
                .relay_bind
                .unwrap_or_else(|| DEFAULT_RELAY_BIND.to_string()),
        }
    }
}

impl TomlServicesRoot {
    fn into_services(self) -> ServicesConfig {
        let services = self.services.unwrap_or_default();
        ServicesConfig {
            relay: ServiceConfig::new(bind_or_default(services.relay, DEFAULT_RELAY_BIND)),
            admission: ServiceConfig::new(bind_or_default(
                services.admission,
                DEFAULT_ADMISSION_BIND,
            )),
            state: ServiceConfig::new(bind_or_default(services.state, DEFAULT_STATE_BIND)),
            coordinator: ServiceConfig::new(bind_or_default(
                services.coordinator,
                DEFAULT_COORDINATOR_BIND,
            )),
            sync: ServiceConfig::new(bind_or_default(services.sync, DEFAULT_SYNC_BIND)),
            git_http: ServiceConfig::new(bind_or_default(services.git_http, DEFAULT_GIT_HTTP_BIND)),
            ui: ServiceConfig::new(bind_or_default(services.ui, DEFAULT_UI_BIND)),
            webhook: ServiceConfig::new(bind_or_default(services.webhook, DEFAULT_WEBHOOK_BIND)),
            control: ServiceConfig::new(bind_or_default(services.control, DEFAULT_CONTROL_BIND)),
            auth: ServiceConfig::new(bind_or_default(services.auth, DEFAULT_AUTH_BIND)),
        }
    }
}

fn bind_or_default(config: Option<TomlServiceConfig>, default: &str) -> String {
    config
        .and_then(|entry| entry.bind)
        .unwrap_or_else(|| default.to_string())
}

#[derive(Debug, Default, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct TomlServicesRoot {
    services: Option<TomlServicesConfig>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct TomlRelayTargets {
    relay_urls: Option<Vec<String>>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct TomlForgejoRoot {
    forgejo: Option<TomlForgejoConfig>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct TomlForgejoConfig {
    base_url: Option<String>,
    api_token: Option<String>,
    owner: Option<String>,
    webhook_url: Option<String>,
    webhook_secret: Option<String>,
    repo_private: Option<bool>,
}

impl TomlForgejoRoot {
    fn into_config(self) -> Result<ForgejoConfig, ConfigError> {
        let config = self.forgejo.ok_or_else(|| ConfigError::InvalidConfig {
            field: "forgejo",
            value: "missing".to_string(),
        })?;
        let base_url = require_toml_field(config.base_url, "forgejo.base_url")?;
        let api_token = require_toml_field(config.api_token, "forgejo.api_token")?;
        let owner = require_toml_field(config.owner, "forgejo.owner")?;
        let webhook_url = require_toml_field(config.webhook_url, "forgejo.webhook_url")?;
        let webhook_secret = require_toml_field(config.webhook_secret, "forgejo.webhook_secret")?;
        let repo_private = config.repo_private.unwrap_or(true);
        Ok(ForgejoConfig {
            base_url,
            api_token,
            owner,
            webhook_url,
            webhook_secret,
            repo_private,
        })
    }
}

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct TomlUiRoot {
    ui: Option<TomlUiConfig>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct TomlUiConfig {
    repo_root: Option<String>,
    public_git_url: Option<String>,
    auth_url: Option<String>,
    app_url: Option<String>,
    control_url: Option<String>,
}

impl TomlUiRoot {
    fn into_config(self) -> Result<UiConfig, ConfigError> {
        let config = self.ui.ok_or_else(|| ConfigError::InvalidConfig {
            field: "ui",
            value: "missing".to_string(),
        })?;
        let repo_root = require_toml_field(config.repo_root, "ui.repo_root")?;
        let public_git_url = require_toml_field(config.public_git_url, "ui.public_git_url")?;
        let auth_url = config
            .auth_url
            .unwrap_or_else(|| DEFAULT_UI_AUTH_URL.to_string());
        let app_url = config
            .app_url
            .unwrap_or_else(|| DEFAULT_UI_APP_URL.to_string());
        let control_url = config
            .control_url
            .unwrap_or_else(|| DEFAULT_UI_CONTROL_URL.to_string());
        Ok(UiConfig {
            repo_root: std::path::PathBuf::from(repo_root),
            public_git_url,
            auth_url,
            app_url,
            control_url,
        })
    }
}

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct TomlRelayCompatibilityRoot {
    relay_compatibility: Option<TomlRelayCompatibility>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct TomlRelayCompatibility {
    mode: Option<String>,
}

impl TomlRelayCompatibilityRoot {
    fn into_config(self) -> Result<RelayCompatibilityConfig, ConfigError> {
        let mode = self
            .relay_compatibility
            .and_then(|value| value.mode)
            .unwrap_or_else(|| DEFAULT_RELAY_COMPAT_MODE.as_str().to_string());
        let mode = RelayCompatibilityMode::parse(&mode)
            .ok_or_else(|| ConfigError::InvalidRelayCompatibilityMode(mode))?;
        Ok(RelayCompatibilityConfig { mode })
    }
}

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct TomlRelayProbeRoot {
    relay_probe: Option<TomlRelayProbeConfig>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct TomlRelayProbeConfig {
    active: Option<bool>,
    timeout_secs: Option<u64>,
    secret_key: Option<String>,
}

impl TomlRelayProbeRoot {
    fn into_config(self) -> RelayProbeConfig {
        let config = self.relay_probe.unwrap_or(TomlRelayProbeConfig {
            active: None,
            timeout_secs: None,
            secret_key: None,
        });
        RelayProbeConfig {
            active: config.active.unwrap_or(false),
            timeout_secs: config
                .timeout_secs
                .unwrap_or(DEFAULT_RELAY_PROBE_TIMEOUT_SECS),
            secret_key: config.secret_key,
        }
    }
}

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct TomlRelayPolicyRoot {
    relay_policy: Option<TomlRelayPolicyConfig>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct TomlRelayPolicyConfig {
    max_content_len: Option<u64>,
    max_tags: Option<u64>,
    max_tag_values: Option<u64>,
    max_tag_value_len: Option<u64>,
    max_future_seconds: Option<u64>,
    max_subscriptions: Option<u64>,
    max_limit: Option<u64>,
    max_message_bytes: Option<u64>,
    max_events_per_min: Option<u64>,
    max_requests_per_min: Option<u64>,
    retention_max_age_seconds: Option<u64>,
    auth_required: Option<bool>,
}

impl TomlRelayPolicyRoot {
    fn into_config(self) -> RelayPolicyConfig {
        let config = self.relay_policy.unwrap_or(TomlRelayPolicyConfig {
            max_content_len: None,
            max_tags: None,
            max_tag_values: None,
            max_tag_value_len: None,
            max_future_seconds: None,
            max_subscriptions: None,
            max_limit: None,
            max_message_bytes: None,
            max_events_per_min: None,
            max_requests_per_min: None,
            retention_max_age_seconds: None,
            auth_required: None,
        });
        RelayPolicyConfig {
            max_content_len: config
                .max_content_len
                .unwrap_or(DEFAULT_RELAY_POLICY_MAX_CONTENT_LEN),
            max_tags: config.max_tags.unwrap_or(DEFAULT_RELAY_POLICY_MAX_TAGS),
            max_tag_values: config
                .max_tag_values
                .unwrap_or(DEFAULT_RELAY_POLICY_MAX_TAG_VALUES),
            max_tag_value_len: config
                .max_tag_value_len
                .unwrap_or(DEFAULT_RELAY_POLICY_MAX_TAG_VALUE_LEN),
            max_future_seconds: config
                .max_future_seconds
                .unwrap_or(DEFAULT_RELAY_POLICY_MAX_FUTURE_SECS),
            max_subscriptions: config.max_subscriptions,
            max_limit: config.max_limit,
            max_message_bytes: config.max_message_bytes,
            max_events_per_min: config.max_events_per_min,
            max_requests_per_min: config.max_requests_per_min,
            retention_max_age_seconds: config.retention_max_age_seconds,
            auth_required: config.auth_required.unwrap_or(false),
        }
    }
}

#[derive(Debug, Default, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct TomlServicesConfig {
    relay: Option<TomlServiceConfig>,
    admission: Option<TomlServiceConfig>,
    state: Option<TomlServiceConfig>,
    coordinator: Option<TomlServiceConfig>,
    sync: Option<TomlServiceConfig>,
    git_http: Option<TomlServiceConfig>,
    ui: Option<TomlServiceConfig>,
    webhook: Option<TomlServiceConfig>,
    control: Option<TomlServiceConfig>,
    auth: Option<TomlServiceConfig>,
}

#[derive(Debug, Default, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct TomlServiceConfig {
    bind: Option<String>,
}

#[derive(Debug)]
pub enum ConfigError {
    InvalidRelayBind(String),
    InvalidRelayUrl(String),
    InvalidRelayCompatibilityMode(String),
    InvalidRelayProbeConfig { field: &'static str, value: String },
    InvalidRelayPolicyConfig { field: &'static str, value: String },
    InvalidServiceBind {
        service: &'static str,
        value: String,
    },
    MissingEnv(&'static str),
    InvalidConfig { field: &'static str, value: String },
    ReadConfig {
        path: std::path::PathBuf,
        source: std::io::Error,
    },
    TomlParse {
        path: Option<std::path::PathBuf>,
        source: toml::de::Error,
    },
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConfigError::InvalidRelayBind(value) => {
                write!(f, "invalid relay bind address: {value}")
            }
            ConfigError::InvalidRelayUrl(value) => {
                write!(f, "invalid relay url: {value}")
            }
            ConfigError::InvalidRelayCompatibilityMode(value) => {
                write!(f, "invalid relay compatibility mode: {value}")
            }
            ConfigError::InvalidRelayProbeConfig { field, value } => {
                write!(f, "invalid relay probe config {field}: {value}")
            }
            ConfigError::InvalidRelayPolicyConfig { field, value } => {
                write!(f, "invalid relay policy config {field}: {value}")
            }
            ConfigError::InvalidServiceBind { service, value } => {
                write!(f, "invalid {service} bind address: {value}")
            }
            ConfigError::MissingEnv(key) => write!(f, "missing env {key}"),
            ConfigError::InvalidConfig { field, value } => {
                write!(f, "invalid config {field}: {value}")
            }
            ConfigError::ReadConfig { path, source } => {
                write!(f, "failed to read config file {}: {source}", path.display())
            }
            ConfigError::TomlParse {
                path: Some(path),
                source,
            } => write!(
                f,
                "failed to parse config file {}: {source}",
                path.display()
            ),
            ConfigError::TomlParse { path: None, source } => {
                write!(f, "failed to parse config: {source}")
            }
        }
    }
}

impl std::error::Error for ConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ConfigError::InvalidRelayBind(_) => None,
            ConfigError::InvalidRelayUrl(_) => None,
            ConfigError::InvalidRelayCompatibilityMode(_) => None,
            ConfigError::InvalidRelayProbeConfig { .. } => None,
            ConfigError::InvalidRelayPolicyConfig { .. } => None,
            ConfigError::InvalidServiceBind { .. } => None,
            ConfigError::MissingEnv(_) => None,
            ConfigError::InvalidConfig { .. } => None,
            ConfigError::ReadConfig { source, .. } => Some(source),
            ConfigError::TomlParse { source, .. } => Some(source),
        }
    }
}

impl ConfigError {
    fn with_path(self, path: &std::path::Path) -> Self {
        match self {
            ConfigError::TomlParse { path: None, source } => ConfigError::TomlParse {
                path: Some(path.to_path_buf()),
                source,
            },
            other => other,
        }
    }
}

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

    pub fn from_env_with_keys(relay_bind_key: &str) -> Self {
        let relay_bind =
            std::env::var(relay_bind_key).unwrap_or_else(|_| DEFAULT_RELAY_BIND.to_string());

        Self { relay_bind }
    }

    pub fn from_toml_str(input: &str) -> Result<Self, ConfigError> {
        let parsed: TomlConfig = toml::from_str(input)
            .map_err(|source| ConfigError::TomlParse { path: None, source })?;

        Ok(parsed.into_config())
    }

    pub fn from_toml_file(path: impl AsRef<std::path::Path>) -> Result<Self, ConfigError> {
        let path = path.as_ref();
        let contents = std::fs::read_to_string(path).map_err(|source| ConfigError::ReadConfig {
            path: path.to_path_buf(),
            source,
        })?;

        Self::from_toml_str(&contents).map_err(|err| err.with_path(path))
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

    pub fn from_env_validated_with_keys(relay_bind_key: &str) -> Result<Self, ConfigError> {
        let config = Self::from_env_with_keys(relay_bind_key);
        config.validate()?;
        Ok(config)
    }

    pub fn from_toml_file_validated(
        path: impl AsRef<std::path::Path>,
    ) -> Result<Self, ConfigError> {
        let config = Self::from_toml_file(path)?;
        config.validate()?;
        Ok(config)
    }
}

#[cfg(test)]
mod tests {
    use super::AuthConfig;
    use super::ConfigError;
    use super::ControlAuthConfig;
    use super::ForgejoConfig;
    use super::GittreeConfig;
    use super::RelayCompatibilityConfig;
    use super::RelayCompatibilityMode;
    use super::RelayPolicyConfig;
    use super::RelayProbeConfig;
    use super::RelayTargetsConfig;
    use super::ServicesConfig;
    use super::UiConfig;
    use crate::DEFAULT_AUTH_BIND;
    use crate::DEFAULT_AUTH_EMAIL_DOMAIN;
    use crate::DEFAULT_AUTH_MAX_SKEW_SECS;
    use crate::{DEFAULT_UI_APP_URL, DEFAULT_UI_AUTH_URL, DEFAULT_UI_CONTROL_URL};
    use crate::DEFAULT_ADMISSION_BIND;
    use crate::DEFAULT_CONTROL_BIND;
    use crate::DEFAULT_COORDINATOR_BIND;
    use crate::DEFAULT_GIT_HTTP_BIND;
    use crate::DEFAULT_RELAY_BIND;
    use crate::DEFAULT_RELAY_POLICY_MAX_CONTENT_LEN;
    use crate::DEFAULT_RELAY_POLICY_MAX_FUTURE_SECS;
    use crate::DEFAULT_RELAY_POLICY_MAX_TAGS;
    use crate::DEFAULT_RELAY_POLICY_MAX_TAG_VALUE_LEN;
    use crate::DEFAULT_RELAY_POLICY_MAX_TAG_VALUES;
    use crate::DEFAULT_STATE_BIND;
    use crate::DEFAULT_SYNC_BIND;
    use crate::DEFAULT_UI_BIND;
    use crate::DEFAULT_WEBHOOK_BIND;
    use crate::ENV_AUTH_BIND;
    use crate::ENV_AUTH_EMAIL_DOMAIN;
    use crate::ENV_AUTH_MAX_SKEW_SECONDS;
    use crate::ENV_ADMISSION_BIND;
    use crate::ENV_CONTROL_ADMIN_KEYS;
    use crate::ENV_CONTROL_BIND;
    use crate::ENV_CONTROL_TOKEN;
    use crate::ENV_COORDINATOR_BIND;
    use crate::ENV_FORGEJO_API_TOKEN;
    use crate::ENV_FORGEJO_BASE_URL;
    use crate::ENV_FORGEJO_OWNER;
    use crate::ENV_FORGEJO_REPO_PRIVATE;
    use crate::ENV_FORGEJO_WEBHOOK_SECRET;
    use crate::ENV_FORGEJO_WEBHOOK_URL;
    use crate::ENV_GIT_HTTP_BIND;
    use crate::ENV_WEBHOOK_BIND;
    use crate::ENV_RELAY_BIND;
    use crate::ENV_RELAY_COMPAT_MODE;
    use crate::ENV_RELAY_POLICY_AUTH_REQUIRED;
    use crate::ENV_RELAY_POLICY_MAX_CONTENT_LEN;
    use crate::ENV_RELAY_POLICY_MAX_EVENTS_PER_MIN;
    use crate::ENV_RELAY_POLICY_MAX_FUTURE_SECS;
    use crate::ENV_RELAY_POLICY_MAX_LIMIT;
    use crate::ENV_RELAY_POLICY_MAX_MESSAGE_BYTES;
    use crate::ENV_RELAY_POLICY_MAX_REQUESTS_PER_MIN;
    use crate::ENV_RELAY_POLICY_MAX_SUBSCRIPTIONS;
    use crate::ENV_RELAY_POLICY_MAX_TAGS;
    use crate::ENV_RELAY_POLICY_MAX_TAG_VALUE_LEN;
    use crate::ENV_RELAY_POLICY_MAX_TAG_VALUES;
    use crate::ENV_RELAY_POLICY_RETENTION_MAX_AGE_SECS;
    use crate::ENV_RELAY_PROBE_ACTIVE;
    use crate::ENV_RELAY_PROBE_SECRET_KEY;
    use crate::ENV_RELAY_PROBE_TIMEOUT_SECS;
    use crate::ENV_RELAY_URLS;
    use crate::ENV_STATE_BIND;
    use crate::ENV_SYNC_BIND;
    use crate::ENV_UI_BIND;
    use crate::ENV_UI_PUBLIC_GIT_URL;
    use crate::ENV_UI_REPO_ROOT;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());
    const ENV_RELAY_BIND_TEST1: &str = "GITTREE_RELAY_BIND_TEST1";
    const ENV_RELAY_BIND_TEST2: &str = "GITTREE_RELAY_BIND_TEST2";

    fn write_temp_config(contents: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        let mut path = std::env::temp_dir();
        path.push(format!(
            "gittree-config-{nanos}-{}.toml",
            std::process::id()
        ));
        std::fs::write(&path, contents).expect("write config file");
        path
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
    fn default_config_has_relay_bind() {
        let config = GittreeConfig::default();
        assert_eq!(config.relay_bind, DEFAULT_RELAY_BIND);
    }

    #[test]
    fn env_config_overrides_relay_bind() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_env_var(ENV_RELAY_BIND, "127.0.0.1:9000", || {
            let config = GittreeConfig::from_env();
            assert_eq!(config.relay_bind, "127.0.0.1:9000");
        });
    }

    #[test]
    fn env_config_falls_back_to_default() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        // SAFETY: this test controls the env var for its duration only.
        unsafe {
            std::env::remove_var(ENV_RELAY_BIND);
        }
        let config = GittreeConfig::from_env();
        assert_eq!(config.relay_bind, DEFAULT_RELAY_BIND);
    }

    #[test]
    fn relay_targets_env_parses_list() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_env_var(ENV_RELAY_URLS, "wss://relay.one, wss://relay.two ,", || {
            let config = RelayTargetsConfig::from_env_validated().expect("relay targets");
            assert_eq!(
                config.relay_urls,
                vec!["wss://relay.one".to_string(), "wss://relay.two".to_string()]
            );
        });
    }

    #[test]
    fn relay_targets_env_rejects_invalid_url() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_env_var(ENV_RELAY_URLS, "ftp://relay.example", || {
            let err = RelayTargetsConfig::from_env_validated().unwrap_err();
            assert!(matches!(
                err,
                ConfigError::InvalidRelayUrl(value) if value == "ftp://relay.example"
            ));
        });
    }

    #[test]
    fn relay_targets_toml_parses_urls() {
        let config =
            RelayTargetsConfig::from_toml_str("relay_urls = [\"wss://relay.example\"]")
                .expect("relay targets");
        assert_eq!(config.relay_urls, vec!["wss://relay.example".to_string()]);
    }

    #[test]
    fn relay_compat_mode_env_parses() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_env_var(ENV_RELAY_COMPAT_MODE, "warn", || {
            let config = RelayCompatibilityConfig::from_env().expect("compat config");
            assert_eq!(config.mode, RelayCompatibilityMode::Warn);
        });
    }

    #[test]
    fn relay_compat_mode_env_rejects_invalid() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_env_var(ENV_RELAY_COMPAT_MODE, "nope", || {
            let err = RelayCompatibilityConfig::from_env().unwrap_err();
            assert!(matches!(
                err,
                ConfigError::InvalidRelayCompatibilityMode(value) if value == "nope"
            ));
        });
    }

    #[test]
    fn relay_probe_env_parses() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_env_var(ENV_RELAY_PROBE_ACTIVE, "true", || {
            with_env_var(ENV_RELAY_PROBE_TIMEOUT_SECS, "7", || {
                with_env_var(ENV_RELAY_PROBE_SECRET_KEY, &"11".repeat(32), || {
                    let config = RelayProbeConfig::from_env().expect("probe config");
                    assert!(config.active);
                    assert_eq!(config.timeout_secs, 7);
                    assert_eq!(config.secret_key, Some("11".repeat(32)));
                });
            });
        });
    }

    #[test]
    fn relay_probe_env_rejects_bad_secret() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_env_var(ENV_RELAY_PROBE_SECRET_KEY, "bad", || {
            let err = RelayProbeConfig::from_env().unwrap_err();
            assert!(matches!(
                err,
                ConfigError::InvalidRelayProbeConfig { field, .. } if field == "relay_probe.secret_key"
            ));
        });
    }

    #[test]
    fn relay_probe_toml_parses() {
        let config = RelayProbeConfig::from_toml_str(
            r#"[relay_probe]
active = true
timeout_secs = 9
secret_key = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
"#,
        )
        .expect("probe config");
        assert!(config.active);
        assert_eq!(config.timeout_secs, 9);
        assert_eq!(
            config.secret_key,
            Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string())
        );
    }

    #[test]
    fn relay_probe_toml_rejects_bad_secret() {
        let err = RelayProbeConfig::from_toml_str(
            r#"[relay_probe]
active = true
timeout_secs = 9
secret_key = "22"
"#,
        )
        .unwrap_err();
        assert!(matches!(
            err,
            ConfigError::InvalidRelayProbeConfig { field, .. } if field == "relay_probe.secret_key"
        ));
    }

    #[test]
    fn relay_policy_env_parses() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_env_var(ENV_RELAY_POLICY_MAX_CONTENT_LEN, "9000", || {
            with_env_var(ENV_RELAY_POLICY_MAX_TAGS, "33", || {
                with_env_var(ENV_RELAY_POLICY_MAX_TAG_VALUES, "12", || {
                    with_env_var(ENV_RELAY_POLICY_MAX_TAG_VALUE_LEN, "120", || {
                        with_env_var(ENV_RELAY_POLICY_MAX_FUTURE_SECS, "30", || {
                            with_env_var(ENV_RELAY_POLICY_MAX_SUBSCRIPTIONS, "9", || {
                                with_env_var(ENV_RELAY_POLICY_MAX_LIMIT, "200", || {
                                    with_env_var(ENV_RELAY_POLICY_MAX_MESSAGE_BYTES, "9999", || {
                                        with_env_var(ENV_RELAY_POLICY_MAX_EVENTS_PER_MIN, "60", || {
                                            with_env_var(ENV_RELAY_POLICY_MAX_REQUESTS_PER_MIN, "30", || {
                                                with_env_var(ENV_RELAY_POLICY_RETENTION_MAX_AGE_SECS, "3600", || {
                                                    with_env_var(ENV_RELAY_POLICY_AUTH_REQUIRED, "true", || {
                                                        let config =
                                                            RelayPolicyConfig::from_env().expect("policy");
                                                        assert_eq!(config.max_content_len, 9000);
                                                        assert_eq!(config.max_tags, 33);
                                                        assert_eq!(config.max_tag_values, 12);
                                                        assert_eq!(config.max_tag_value_len, 120);
                                                        assert_eq!(config.max_future_seconds, 30);
                                                        assert_eq!(config.max_subscriptions, Some(9));
                                                        assert_eq!(config.max_limit, Some(200));
                                                        assert_eq!(config.max_message_bytes, Some(9999));
                                                        assert_eq!(config.max_events_per_min, Some(60));
                                                        assert_eq!(config.max_requests_per_min, Some(30));
                                                        assert_eq!(config.retention_max_age_seconds, Some(3600));
                                                        assert!(config.auth_required);
                                                    });
                                                });
                                            });
                                        });
                                    });
                                });
                            });
                        });
                    });
                });
            });
        });
    }

    #[test]
    fn relay_policy_env_defaults_apply() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        unsafe {
            std::env::remove_var(ENV_RELAY_POLICY_MAX_CONTENT_LEN);
            std::env::remove_var(ENV_RELAY_POLICY_MAX_TAGS);
            std::env::remove_var(ENV_RELAY_POLICY_MAX_TAG_VALUES);
            std::env::remove_var(ENV_RELAY_POLICY_MAX_TAG_VALUE_LEN);
            std::env::remove_var(ENV_RELAY_POLICY_MAX_FUTURE_SECS);
            std::env::remove_var(ENV_RELAY_POLICY_MAX_SUBSCRIPTIONS);
            std::env::remove_var(ENV_RELAY_POLICY_MAX_LIMIT);
            std::env::remove_var(ENV_RELAY_POLICY_MAX_MESSAGE_BYTES);
            std::env::remove_var(ENV_RELAY_POLICY_MAX_EVENTS_PER_MIN);
            std::env::remove_var(ENV_RELAY_POLICY_MAX_REQUESTS_PER_MIN);
            std::env::remove_var(ENV_RELAY_POLICY_RETENTION_MAX_AGE_SECS);
            std::env::remove_var(ENV_RELAY_POLICY_AUTH_REQUIRED);
        }
        let config = RelayPolicyConfig::from_env().expect("policy");
        assert_eq!(config.max_content_len, DEFAULT_RELAY_POLICY_MAX_CONTENT_LEN);
        assert_eq!(config.max_tags, DEFAULT_RELAY_POLICY_MAX_TAGS);
        assert_eq!(config.max_tag_values, DEFAULT_RELAY_POLICY_MAX_TAG_VALUES);
        assert_eq!(config.max_tag_value_len, DEFAULT_RELAY_POLICY_MAX_TAG_VALUE_LEN);
        assert_eq!(config.max_future_seconds, DEFAULT_RELAY_POLICY_MAX_FUTURE_SECS);
        assert_eq!(config.max_subscriptions, None);
        assert_eq!(config.max_limit, None);
        assert_eq!(config.max_message_bytes, None);
        assert_eq!(config.max_events_per_min, None);
        assert_eq!(config.max_requests_per_min, None);
        assert_eq!(config.retention_max_age_seconds, None);
        assert!(!config.auth_required);
    }

    #[test]
    fn relay_policy_env_rejects_zero() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_env_var(ENV_RELAY_POLICY_MAX_TAGS, "0", || {
            let err = RelayPolicyConfig::from_env().unwrap_err();
            assert!(matches!(
                err,
                ConfigError::InvalidRelayPolicyConfig { field, .. } if field == "relay_policy.max_tags"
            ));
        });
    }

    #[test]
    fn relay_policy_toml_parses() {
        let config = RelayPolicyConfig::from_toml_str(
            r#"[relay_policy]
max_content_len = 4096
max_tags = 48
max_tag_values = 10
max_tag_value_len = 80
max_future_seconds = 10
max_subscriptions = 5
max_limit = 250
max_message_bytes = 10000
max_events_per_min = 60
max_requests_per_min = 30
retention_max_age_seconds = 3600
auth_required = true
"#,
        )
        .expect("policy config");
        assert_eq!(config.max_content_len, 4096);
        assert_eq!(config.max_tags, 48);
        assert_eq!(config.max_tag_values, 10);
        assert_eq!(config.max_tag_value_len, 80);
        assert_eq!(config.max_future_seconds, 10);
        assert_eq!(config.max_subscriptions, Some(5));
        assert_eq!(config.max_limit, Some(250));
        assert_eq!(config.max_message_bytes, Some(10000));
        assert_eq!(config.max_events_per_min, Some(60));
        assert_eq!(config.max_requests_per_min, Some(30));
        assert_eq!(config.retention_max_age_seconds, Some(3600));
        assert!(config.auth_required);
    }

    #[test]
    fn relay_policy_toml_rejects_zero() {
        let err = RelayPolicyConfig::from_toml_str(
            r#"[relay_policy]
max_content_len = 0
"#,
        )
        .unwrap_err();
        assert!(matches!(
            err,
            ConfigError::InvalidRelayPolicyConfig { field, .. } if field == "relay_policy.max_content_len"
        ));
    }

    #[test]
    fn validate_accepts_valid_relay_bind() {
        let config = GittreeConfig {
            relay_bind: "127.0.0.1:9000".to_string(),
        };
        assert!(config.validate().is_ok());
    }

    #[test]
    fn validate_rejects_invalid_relay_bind() {
        let config = GittreeConfig {
            relay_bind: "not-an-addr".to_string(),
        };
        assert!(matches!(
            config.validate(),
            Err(ConfigError::InvalidRelayBind(value)) if value == "not-an-addr"
        ));
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
        assert!(matches!(
            config.relay_bind_addr(),
            Err(ConfigError::InvalidRelayBind(value)) if value == "bad"
        ));
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
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_env_var(ENV_RELAY_BIND, "bad:addr", || {
            let result = GittreeConfig::from_env_validated();
            assert!(matches!(
                result,
                Err(ConfigError::InvalidRelayBind(value)) if value == "bad:addr"
            ));
        });
    }

    #[test]
    fn from_env_validated_accepts_valid_bind() {
        let _guard = ENV_LOCK.lock().expect("env lock");
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

    #[test]
    fn from_env_with_keys_reads_custom_key() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_env_var(ENV_RELAY_BIND_TEST1, "127.0.0.1:8081", || {
            let config = GittreeConfig::from_env_with_keys(ENV_RELAY_BIND_TEST1);
            assert_eq!(config.relay_bind, "127.0.0.1:8081");
        });
    }

    #[test]
    fn from_env_validated_with_keys_accepts_valid_bind() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_env_var(ENV_RELAY_BIND_TEST2, "127.0.0.1:8082", || {
            let config = GittreeConfig::from_env_validated_with_keys(ENV_RELAY_BIND_TEST2);
            assert!(matches!(
                config,
                Ok(GittreeConfig { relay_bind }) if relay_bind == "127.0.0.1:8082"
            ));
        });
    }

    #[test]
    fn toml_str_parses_valid_config() {
        let config =
            GittreeConfig::from_toml_str("relay_bind = \"127.0.0.1:9999\"").expect("parse config");
        assert_eq!(config.relay_bind, "127.0.0.1:9999");
    }

    #[test]
    fn toml_str_rejects_invalid_config() {
        let result = GittreeConfig::from_toml_str("relay_bind = [");
        assert!(matches!(result, Err(ConfigError::TomlParse { .. })));
    }

    #[test]
    fn toml_file_reads_valid_config() {
        let path = write_temp_config("relay_bind = \"127.0.0.1:9998\"");
        let config = GittreeConfig::from_toml_file(&path).expect("read config");
        assert_eq!(config.relay_bind, "127.0.0.1:9998");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn toml_file_reports_missing_file() {
        let mut path = std::env::temp_dir();
        path.push(format!(
            "gittree-config-missing-{}.toml",
            std::process::id()
        ));
        let result = GittreeConfig::from_toml_file(&path);
        assert!(matches!(result, Err(ConfigError::ReadConfig { .. })));
    }

    #[test]
    fn toml_file_validated_rejects_invalid_bind() {
        let path = write_temp_config("relay_bind = \"invalid\"");
        let result = GittreeConfig::from_toml_file_validated(&path);
        assert!(matches!(
            result,
            Err(ConfigError::InvalidRelayBind(value)) if value == "invalid"
        ));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn services_config_has_expected_defaults() {
        let services = ServicesConfig::default();
        assert_eq!(services.relay.bind, DEFAULT_RELAY_BIND);
        assert_eq!(services.admission.bind, DEFAULT_ADMISSION_BIND);
        assert_eq!(services.state.bind, DEFAULT_STATE_BIND);
        assert_eq!(services.coordinator.bind, DEFAULT_COORDINATOR_BIND);
        assert_eq!(services.sync.bind, DEFAULT_SYNC_BIND);
        assert_eq!(services.git_http.bind, DEFAULT_GIT_HTTP_BIND);
        assert_eq!(services.ui.bind, DEFAULT_UI_BIND);
        assert_eq!(services.webhook.bind, DEFAULT_WEBHOOK_BIND);
        assert_eq!(services.control.bind, DEFAULT_CONTROL_BIND);
        assert_eq!(services.auth.bind, DEFAULT_AUTH_BIND);
    }

    #[test]
    fn services_config_from_env_uses_defaults_when_unset() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        unsafe {
            std::env::remove_var(ENV_RELAY_BIND);
            std::env::remove_var(ENV_ADMISSION_BIND);
            std::env::remove_var(ENV_STATE_BIND);
            std::env::remove_var(ENV_COORDINATOR_BIND);
            std::env::remove_var(ENV_SYNC_BIND);
            std::env::remove_var(ENV_GIT_HTTP_BIND);
            std::env::remove_var(ENV_UI_BIND);
            std::env::remove_var(ENV_WEBHOOK_BIND);
            std::env::remove_var(ENV_CONTROL_BIND);
            std::env::remove_var(ENV_AUTH_BIND);
        }

        let services = ServicesConfig::from_env();
        assert_eq!(services.relay.bind, DEFAULT_RELAY_BIND);
        assert_eq!(services.admission.bind, DEFAULT_ADMISSION_BIND);
        assert_eq!(services.state.bind, DEFAULT_STATE_BIND);
        assert_eq!(services.coordinator.bind, DEFAULT_COORDINATOR_BIND);
        assert_eq!(services.sync.bind, DEFAULT_SYNC_BIND);
        assert_eq!(services.git_http.bind, DEFAULT_GIT_HTTP_BIND);
        assert_eq!(services.ui.bind, DEFAULT_UI_BIND);
        assert_eq!(services.webhook.bind, DEFAULT_WEBHOOK_BIND);
        assert_eq!(services.control.bind, DEFAULT_CONTROL_BIND);
        assert_eq!(services.auth.bind, DEFAULT_AUTH_BIND);
    }

    #[test]
    fn services_config_from_env_overrides_bind() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_env_var(ENV_ADMISSION_BIND, "127.0.0.1:9091", || {
            let services = ServicesConfig::from_env();
            assert_eq!(services.admission.bind, "127.0.0.1:9091");
        });
    }

    #[test]
    fn services_toml_parses_overrides() {
        let toml = r#"
[services.relay]
bind = "127.0.0.1:9010"

[services.admission]
bind = "127.0.0.1:9011"

[services.control]
bind = "127.0.0.1:9019"

[services.auth]
bind = "127.0.0.1:9020"
"#;
        let services = ServicesConfig::from_toml_str(toml).expect("parse services");
        assert_eq!(services.relay.bind, "127.0.0.1:9010");
        assert_eq!(services.admission.bind, "127.0.0.1:9011");
        assert_eq!(services.control.bind, "127.0.0.1:9019");
        assert_eq!(services.auth.bind, "127.0.0.1:9020");
        assert_eq!(services.state.bind, DEFAULT_STATE_BIND);
        assert_eq!(services.git_http.bind, DEFAULT_GIT_HTTP_BIND);
        assert_eq!(services.ui.bind, DEFAULT_UI_BIND);
        assert_eq!(services.webhook.bind, DEFAULT_WEBHOOK_BIND);
    }

    #[test]
    fn services_toml_rejects_invalid_config() {
        let result = ServicesConfig::from_toml_str("services = [");
        assert!(matches!(result, Err(ConfigError::TomlParse { .. })));
    }

    #[test]
    fn services_toml_file_reads_valid_config() {
        let toml = r#"
[services.state]
bind = "127.0.0.1:9101"
"#;
        let path = write_temp_config(toml);
        let services = ServicesConfig::from_toml_file(&path).expect("read services");
        assert_eq!(services.state.bind, "127.0.0.1:9101");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn services_config_validate_rejects_invalid_bind() {
        let mut services = ServicesConfig::default();
        services.state.bind = "bad".to_string();
        let err = services.validate().unwrap_err();
        assert!(matches!(
            err,
            ConfigError::InvalidServiceBind {
                service: "state",
                ..
            }
        ));
    }

    #[test]
    fn services_config_from_env_validated_reports_invalid_bind() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_env_var(ENV_STATE_BIND, "bad", || {
            let err = ServicesConfig::from_env_validated().unwrap_err();
            assert!(matches!(
                err,
                ConfigError::InvalidServiceBind {
                    service: "state",
                    ..
                }
            ));
        });
    }

    #[test]
    fn services_toml_file_validated_rejects_invalid_bind() {
        let toml = r#"
[services.coordinator]
bind = "bad"
"#;
        let path = write_temp_config(toml);
        let result = ServicesConfig::from_toml_file_validated(&path);
        assert!(matches!(
            result,
            Err(ConfigError::InvalidServiceBind {
                service: "coordinator",
                ..
            })
        ));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn forgejo_config_from_env_parses() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_env_var(ENV_FORGEJO_BASE_URL, "http://localhost:3000", || {
            with_env_var(ENV_FORGEJO_API_TOKEN, "token", || {
                with_env_var(ENV_FORGEJO_OWNER, "gittree", || {
                    with_env_var(ENV_FORGEJO_WEBHOOK_URL, "http://localhost:8090/", || {
                        with_env_var(ENV_FORGEJO_WEBHOOK_SECRET, "secret", || {
                            with_env_var(ENV_FORGEJO_REPO_PRIVATE, "false", || {
                                let config = ForgejoConfig::from_env().expect("forgejo");
                                assert_eq!(config.base_url, "http://localhost:3000");
                                assert_eq!(config.api_token, "token");
                                assert_eq!(config.owner, "gittree");
                                assert_eq!(config.webhook_url, "http://localhost:8090/");
                                assert_eq!(config.webhook_secret, "secret");
                                assert!(!config.repo_private);
                            });
                        });
                    });
                });
            });
        });
    }

    #[test]
    fn forgejo_config_from_toml_parses() {
        let toml = r#"
[forgejo]
base_url = "http://localhost:3000"
api_token = "token"
owner = "gittree"
webhook_url = "http://localhost:8090/"
webhook_secret = "secret"
repo_private = true
"#;
        let config = ForgejoConfig::from_toml_str(toml).expect("forgejo");
        assert_eq!(config.base_url, "http://localhost:3000");
        assert_eq!(config.api_token, "token");
        assert_eq!(config.owner, "gittree");
        assert_eq!(config.webhook_url, "http://localhost:8090/");
        assert_eq!(config.webhook_secret, "secret");
        assert!(config.repo_private);
    }

    #[test]
    fn ui_config_from_env_parses() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_env_var(ENV_UI_REPO_ROOT, "/tmp/gittree-ui", || {
            with_env_var(ENV_UI_PUBLIC_GIT_URL, "http://localhost:8085", || {
                let config = UiConfig::from_env().expect("ui");
                assert_eq!(config.repo_root, std::path::PathBuf::from("/tmp/gittree-ui"));
                assert_eq!(config.public_git_url, "http://localhost:8085");
                assert_eq!(config.auth_url, DEFAULT_UI_AUTH_URL);
                assert_eq!(config.app_url, DEFAULT_UI_APP_URL);
                assert_eq!(config.control_url, DEFAULT_UI_CONTROL_URL);
            });
        });
    }

    #[test]
    fn ui_config_from_toml_parses() {
        let toml = r#"
[ui]
repo_root = "/tmp/gittree-ui"
public_git_url = "http://localhost:8085"
"#;
        let config = UiConfig::from_toml_str(toml).expect("ui");
        assert_eq!(config.repo_root, std::path::PathBuf::from("/tmp/gittree-ui"));
        assert_eq!(config.public_git_url, "http://localhost:8085");
        assert_eq!(config.auth_url, DEFAULT_UI_AUTH_URL);
        assert_eq!(config.app_url, DEFAULT_UI_APP_URL);
        assert_eq!(config.control_url, DEFAULT_UI_CONTROL_URL);
    }

    #[test]
    fn control_auth_from_env_parses() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_env_var(ENV_CONTROL_TOKEN, "token", || {
            with_env_var(ENV_CONTROL_ADMIN_KEYS, "npub1, npub2", || {
                let config = ControlAuthConfig::from_env().expect("control");
                assert_eq!(config.token, "token");
                assert_eq!(config.admin_keys, vec!["npub1".to_string(), "npub2".to_string()]);
            });
        });
    }

    #[test]
    fn auth_config_defaults_apply() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        unsafe {
            std::env::remove_var(ENV_AUTH_EMAIL_DOMAIN);
            std::env::remove_var(ENV_AUTH_MAX_SKEW_SECONDS);
        }
        let config = AuthConfig::from_env().expect("auth");
        assert_eq!(config.email_domain, DEFAULT_AUTH_EMAIL_DOMAIN);
        assert_eq!(config.max_skew_seconds, DEFAULT_AUTH_MAX_SKEW_SECS);
    }

    #[test]
    fn auth_config_env_overrides_apply() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_env_var(ENV_AUTH_EMAIL_DOMAIN, "example.test", || {
            with_env_var(ENV_AUTH_MAX_SKEW_SECONDS, "120", || {
                let config = AuthConfig::from_env().expect("auth");
                assert_eq!(config.email_domain, "example.test");
                assert_eq!(config.max_skew_seconds, 120);
            });
        });
    }
}

use gittree_config::{ConfigError, ServicesConfig};
use std::time::Duration;

const ENV_UPSTREAM_URL: &str = "GITTREE_GIT_HTTP_UPSTREAM_URL";
const ENV_TIMEOUT_SECS: &str = "GITTREE_GIT_HTTP_TIMEOUT_SECS";
const DEFAULT_TIMEOUT_SECS: u64 = 10;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitHttpConfig {
    pub bind: String,
    pub upstream_url: String,
    pub timeout: Duration,
}

impl GitHttpConfig {
    pub fn from_env() -> Result<Self, GitHttpConfigError> {
        let services = ServicesConfig::from_env_validated().map_err(GitHttpConfigError::Config)?;
        let upstream_url = std::env::var(ENV_UPSTREAM_URL)
            .map_err(|_| GitHttpConfigError::MissingEnv(ENV_UPSTREAM_URL))?;
        if url::Url::parse(&upstream_url).is_err() {
            return Err(GitHttpConfigError::InvalidEnv {
                key: ENV_UPSTREAM_URL,
                value: upstream_url,
            });
        }
        let timeout_secs = env_u64(ENV_TIMEOUT_SECS)?.unwrap_or(DEFAULT_TIMEOUT_SECS);
        Ok(Self {
            bind: services.git_http.bind,
            upstream_url,
            timeout: Duration::from_secs(timeout_secs),
        })
    }
}

#[derive(Debug)]
pub enum GitHttpConfigError {
    Config(ConfigError),
    MissingEnv(&'static str),
    InvalidEnv { key: &'static str, value: String },
}

impl std::fmt::Display for GitHttpConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GitHttpConfigError::Config(err) => write!(f, "git-http config error: {err}"),
            GitHttpConfigError::MissingEnv(key) => write!(f, "missing env {key}"),
            GitHttpConfigError::InvalidEnv { key, value } => {
                write!(f, "invalid env {key}: {value}")
            }
        }
    }
}

impl std::error::Error for GitHttpConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            GitHttpConfigError::Config(err) => Some(err),
            GitHttpConfigError::MissingEnv(_) => None,
            GitHttpConfigError::InvalidEnv { .. } => None,
        }
    }
}

fn env_u64(key: &'static str) -> Result<Option<u64>, GitHttpConfigError> {
    match std::env::var(key) {
        Ok(value) => value
            .parse::<u64>()
            .map(Some)
            .map_err(|_| GitHttpConfigError::InvalidEnv { key, value }),
        Err(_) => Ok(None),
    }
}

#[derive(Debug)]
pub enum GitHttpError {
    Config(GitHttpConfigError),
}

impl std::fmt::Display for GitHttpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GitHttpError::Config(err) => write!(f, "git-http error: {err}"),
        }
    }
}

impl std::error::Error for GitHttpError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            GitHttpError::Config(err) => Some(err),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitHttpRequest<'a> {
    pub method: &'a str,
    pub path: &'a str,
    pub query: Option<&'a str>,
}

impl<'a> GitHttpRequest<'a> {
    pub fn new(method: &'a str, path: &'a str, query: Option<&'a str>) -> Self {
        Self {
            method,
            path,
            query,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GitHttpRoute {
    NotFound,
}

#[derive(Debug, Default)]
pub struct GitHttpRouter;

impl GitHttpRouter {
    pub fn new() -> Self {
        Self
    }

    pub fn route(&self, _request: &GitHttpRequest<'_>) -> GitHttpRoute {
        GitHttpRoute::NotFound
    }
}

#[cfg(test)]
mod tests {
    use super::ENV_TIMEOUT_SECS;
    use super::ENV_UPSTREAM_URL;
    use super::GitHttpConfig;
    use std::sync::Mutex;
    use std::time::Duration;

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
    fn config_loads_from_env() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_env_var(ENV_UPSTREAM_URL, "https://git.example", || {
            with_env_var("GITTREE_GIT_HTTP_BIND", "127.0.0.1:9090", || {
                with_env_var(ENV_TIMEOUT_SECS, "15", || {
                    let config = GitHttpConfig::from_env().expect("config");
                    assert_eq!(config.bind, "127.0.0.1:9090");
                    assert_eq!(config.upstream_url, "https://git.example");
                    assert_eq!(config.timeout, Duration::from_secs(15));
                });
            });
        });
    }
}

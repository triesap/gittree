use gittree_config::{ConfigError, ServicesConfig};
use std::path::Path;
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
    InfoRefs {
        repo: NormalizedRepo,
        service: GitHttpService,
    },
    UploadPack {
        repo: NormalizedRepo,
    },
    ReceivePack {
        repo: NormalizedRepo,
    },
    NotFound,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GitHttpService {
    UploadPack,
    ReceivePack,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NormalizedRepo {
    pub npub: String,
    pub identifier: String,
    pub canonical_path: String,
}

#[derive(Debug, Default)]
pub struct GitHttpRouter;

impl GitHttpRouter {
    pub fn new() -> Self {
        Self
    }

    pub fn route(&self, request: &GitHttpRequest<'_>) -> GitHttpRoute {
        route_request(request)
    }
}

pub fn route_request(request: &GitHttpRequest<'_>) -> GitHttpRoute {
    let (npub, repo_segment, rest) = match split_repo_segments(request.path) {
        Some(parts) => parts,
        None => return GitHttpRoute::NotFound,
    };
    if !repo_segment.ends_with(".git") {
        return GitHttpRoute::NotFound;
    }
    let repo = match normalize_repo_path(&npub, &repo_segment) {
        Ok(repo) => repo,
        Err(_) => return GitHttpRoute::NotFound,
    };
    if rest.len() == 2 && rest[0] == "info" && rest[1] == "refs" && is_get(request.method) {
        let service = match parse_service(request.query) {
            Ok(service) => service,
            Err(_) => return GitHttpRoute::NotFound,
        };
        return GitHttpRoute::InfoRefs { repo, service };
    }
    if rest.len() == 1 && rest[0] == "git-upload-pack" && is_post(request.method) {
        return GitHttpRoute::UploadPack { repo };
    }
    if rest.len() == 1 && rest[0] == "git-receive-pack" && is_post(request.method) {
        return GitHttpRoute::ReceivePack { repo };
    }
    GitHttpRoute::NotFound
}

fn split_repo_segments(path: &str) -> Option<(String, String, Vec<String>)> {
    let trimmed = path.trim_start_matches('/');
    let mut parts = trimmed.split('/').filter(|segment| !segment.is_empty());
    let npub = parts.next()?.to_string();
    let repo = parts.next()?.to_string();
    let rest = parts.map(|segment| segment.to_string()).collect::<Vec<_>>();
    if rest.is_empty() {
        return None;
    }
    Some((npub, repo, rest))
}

fn normalize_repo_path(
    npub: &str,
    repo_segment: &str,
) -> Result<NormalizedRepo, GitHttpRouteError> {
    let path = Path::new("/").join(npub).join(repo_segment);
    let parsed = gittree_core::parse_repo_path(&path)
        .map_err(|err| GitHttpRouteError::InvalidRepo(err.to_string()))?;
    Ok(NormalizedRepo {
        canonical_path: format!("/{}/{}.git", parsed.npub, parsed.identifier),
        identifier: parsed.identifier,
        npub: parsed.npub,
    })
}

fn parse_service(query: Option<&str>) -> Result<GitHttpService, GitHttpRouteError> {
    let query = query.ok_or(GitHttpRouteError::MissingService)?;
    for pair in query.split('&') {
        let mut parts = pair.splitn(2, '=');
        if parts.next() != Some("service") {
            continue;
        }
        let value = parts.next().unwrap_or("");
        return match value {
            "git-upload-pack" => Ok(GitHttpService::UploadPack),
            "git-receive-pack" => Ok(GitHttpService::ReceivePack),
            _ => Err(GitHttpRouteError::InvalidService(value.to_string())),
        };
    }
    Err(GitHttpRouteError::MissingService)
}

fn is_get(method: &str) -> bool {
    method.eq_ignore_ascii_case("GET")
}

fn is_post(method: &str) -> bool {
    method.eq_ignore_ascii_case("POST")
}

#[derive(Debug)]
pub enum GitHttpRouteError {
    InvalidRepo(String),
    MissingService,
    InvalidService(String),
}

#[cfg(test)]
mod tests {
    use super::ENV_TIMEOUT_SECS;
    use super::ENV_UPSTREAM_URL;
    use super::GitHttpConfig;
    use super::GitHttpRequest;
    use super::GitHttpRoute;
    use super::GitHttpService;
    use super::route_request;
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

    #[test]
    fn route_request_handles_info_refs() {
        let request = GitHttpRequest::new(
            "GET",
            "/npub1gjttreegkzys8jlhdnfm3qe39h2gka79cpndd0jsms5fk7tuhcnsdw56jq/repo.git/info/refs",
            Some("service=git-upload-pack"),
        );
        let route = route_request(&request);
        assert!(matches!(
            route,
            GitHttpRoute::InfoRefs {
                service: GitHttpService::UploadPack,
                ..
            }
        ));
    }

    #[test]
    fn route_request_handles_receive_pack() {
        let request = GitHttpRequest::new(
            "POST",
            "/npub1gjttreegkzys8jlhdnfm3qe39h2gka79cpndd0jsms5fk7tuhcnsdw56jq/repo.git/git-receive-pack",
            None,
        );
        let route = route_request(&request);
        assert!(matches!(route, GitHttpRoute::ReceivePack { .. }));
    }

    #[test]
    fn route_request_rejects_missing_git_suffix() {
        let request = GitHttpRequest::new(
            "GET",
            "/npub1gjttreegkzys8jlhdnfm3qe39h2gka79cpndd0jsms5fk7tuhcnsdw56jq/repo/info/refs",
            Some("service=git-upload-pack"),
        );
        let route = route_request(&request);
        assert!(matches!(route, GitHttpRoute::NotFound));
    }
}

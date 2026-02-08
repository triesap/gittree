#![forbid(unsafe_code)]

use gittree_app_core::{RepoDetail, RepoListResponse};
use leptos::prelude::*;

#[cfg(feature = "ssr")]
use axum::extract::FromRef;
#[cfg(feature = "ssr")]
use gittree_core::parse_repo_path;
#[cfg(feature = "ssr")]
use gittree_storage::{RepoMappingRecord, RepoMappingRepository};
#[cfg(feature = "ssr")]
use leptos::config::LeptosOptions;
#[cfg(feature = "ssr")]
use std::path::PathBuf;
#[cfg(feature = "ssr")]
use std::sync::Arc;

#[derive(Debug)]
pub enum AppUiError {
    BadRequest(String),
    NotFound(String),
    Storage(String),
    Internal(String),
}

impl std::fmt::Display for AppUiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AppUiError::BadRequest(message) => write!(f, "bad request: {message}"),
            AppUiError::NotFound(message) => write!(f, "not found: {message}"),
            AppUiError::Storage(message) => write!(f, "storage error: {message}"),
            AppUiError::Internal(message) => write!(f, "internal error: {message}"),
        }
    }
}

impl std::error::Error for AppUiError {}

#[cfg(feature = "ssr")]
#[derive(Clone)]
pub struct AppUiState {
    pub repositories: Arc<dyn RepoMappingRepository>,
    pub repo_root: PathBuf,
    pub public_git_url: String,
    pub auth_url: String,
    pub app_url: String,
    pub control_url: String,
    pub base_path: String,
    pub leptos_options: LeptosOptions,
}

#[cfg(feature = "ssr")]
impl AppUiState {
    pub fn new(
        repositories: Arc<dyn RepoMappingRepository>,
        repo_root: PathBuf,
        public_git_url: String,
        auth_url: String,
        app_url: String,
        control_url: String,
        base_path: String,
        leptos_options: LeptosOptions,
    ) -> Self {
        Self {
            repositories,
            repo_root,
            public_git_url,
            auth_url,
            app_url,
            control_url,
            base_path,
            leptos_options,
        }
    }
}

#[cfg(feature = "ssr")]
impl FromRef<AppUiState> for LeptosOptions {
    fn from_ref(state: &AppUiState) -> Self {
        state.leptos_options.clone()
    }
}

#[cfg(feature = "ssr")]
pub async fn list_repo_items(state: &AppUiState) -> Result<Vec<RepoListItem>, AppUiError> {
    let mappings = state
        .repositories
        .list_mappings()
        .await
        .map_err(|err| AppUiError::Storage(err.to_string()))?;
    let mut items = Vec::with_capacity(mappings.len());
    for mapping in mappings {
        items.push(repo_list_item(&state.public_git_url, mapping)?);
    }
    Ok(items)
}

#[cfg(feature = "ssr")]
pub async fn repo_detail_item(
    state: &AppUiState,
    npub: &str,
    identifier: &str,
) -> Result<RepoDetail, AppUiError> {
    let identifier = normalize_identifier(identifier);
    let repo_path = state
        .repo_root
        .join(npub)
        .join(format!("{identifier}.git"));
    let parsed = parse_repo_path(&repo_path)
        .map_err(|err| AppUiError::BadRequest(err.to_string()))?;
    let pubkey_bytes = hex::decode(&parsed.pubkey)
        .map_err(|_| AppUiError::BadRequest("invalid pubkey".to_string()))?;
    let mapping = state
        .repositories
        .mapping_by_repo(&pubkey_bytes, &parsed.identifier)
        .await
        .map_err(|err| AppUiError::Storage(err.to_string()))?
        .ok_or_else(|| AppUiError::NotFound("missing repo mapping".to_string()))?;
    let item = repo_list_item(&state.public_git_url, mapping)?;
    Ok(RepoDetail::from(item))
}

#[cfg(feature = "ssr")]
fn repo_list_item(
    public_git_url: &str,
    mapping: RepoMappingRecord,
) -> Result<RepoListItem, AppUiError> {
    let npub = npub_from_bytes(&mapping.pubkey)
        .map_err(|err| AppUiError::Internal(err.to_string()))?;
    let forgejo = mapping.forgejo_full_name();
    let identifier = mapping.identifier;
    let clone_url = clone_url(public_git_url, &npub, &identifier);
    Ok(RepoListItem::new(npub, identifier, forgejo, clone_url))
}

#[server(prefix = "/api", name = ListRepositoriesFn)]
pub async fn list_repositories() -> Result<RepoListResponse, ServerFnError> {
    let state = use_context::<AppUiState>()
        .ok_or_else(|| ServerFnError::new("missing app state"))?;
    let items = list_repo_items(&state).await?;
    Ok(RepoListResponse { items })
}

#[server(prefix = "/api", name = RepoDetailFn)]
pub async fn repo_detail(npub: String, identifier: String) -> Result<RepoDetail, ServerFnError> {
    let state = use_context::<AppUiState>()
        .ok_or_else(|| ServerFnError::new("missing app state"))?;
    let detail = repo_detail_item(&state, &npub, &identifier).await?;
    Ok(detail)
}

#[cfg(all(test, feature = "ssr"))]
mod tests {
    use super::{list_repo_items, repo_detail_item, AppUiState};
    use gittree_app_core::npub_from_bytes;
    use gittree_core::RepoMapping;
    use gittree_storage::{InMemoryRepositories, RepoMappingRecord, RepoMappingRepository};
    use leptos::config::LeptosOptions;
    use std::sync::Arc;

    fn test_state(repositories: Arc<dyn RepoMappingRepository>) -> AppUiState {
        AppUiState::new(
            repositories,
            "/tmp/gittree".into(),
            "http://localhost:8085".to_string(),
            "http://localhost:8089".to_string(),
            "http://localhost:8090".to_string(),
            "http://localhost:8088".to_string(),
            "/".to_string(),
            LeptosOptions::builder()
                .output_name("gittree-app-ui")
                .site_root("crates/app-ui/dist")
                .site_pkg_dir("pkg")
                .site_addr("127.0.0.1:0".parse::<std::net::SocketAddr>().expect("addr"))
                .build(),
        )
    }

    #[test]
    fn app_ui_state_stores_auth_url() {
        let repositories: Arc<dyn RepoMappingRepository> = Arc::new(InMemoryRepositories::new());
        let state = test_state(repositories);
        assert_eq!(state.auth_url, "http://localhost:8089");
    }

    #[test]
    fn app_ui_state_stores_app_url() {
        let repositories: Arc<dyn RepoMappingRepository> = Arc::new(InMemoryRepositories::new());
        let state = test_state(repositories);
        assert_eq!(state.app_url, "http://localhost:8090");
    }

    #[test]
    fn app_ui_state_stores_control_url() {
        let repositories: Arc<dyn RepoMappingRepository> = Arc::new(InMemoryRepositories::new());
        let state = test_state(repositories);
        assert_eq!(state.control_url, "http://localhost:8088");
    }

    #[tokio::test]
    async fn list_repo_items_returns_entries() {
        let repositories = Arc::new(InMemoryRepositories::new());
        let mapping = RepoMapping::new(
            "owner",
            "repo",
            "11".repeat(32),
            "repo",
        )
        .expect("mapping");
        let record = RepoMappingRecord::new(&mapping).expect("record");
        repositories
            .upsert_mapping(record)
            .await
            .expect("insert mapping");

        let repositories: Arc<dyn RepoMappingRepository> = repositories;
        let state = test_state(repositories);
        let items = list_repo_items(&state).await.expect("items");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].forgejo, "owner/repo");
        assert!(items[0].clone_url.contains("http://localhost:8085"));
    }

    #[tokio::test]
    async fn repo_detail_item_returns_repo() {
        let repositories = Arc::new(InMemoryRepositories::new());
        let mapping = RepoMapping::new(
            "owner",
            "repo",
            "11".repeat(32),
            "repo",
        )
        .expect("mapping");
        let record = RepoMappingRecord::new(&mapping).expect("record");
        let npub = npub_from_bytes(&record.pubkey).expect("npub");
        repositories
            .upsert_mapping(record)
            .await
            .expect("insert mapping");

        let repositories: Arc<dyn RepoMappingRepository> = repositories;
        let state = test_state(repositories);
        let detail = repo_detail_item(&state, &npub, "repo")
            .await
            .expect("detail");
        assert_eq!(detail.identifier, "repo");
        assert_eq!(detail.forgejo, "owner/repo");
    }
}

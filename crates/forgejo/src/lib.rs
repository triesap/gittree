use async_trait::async_trait;
use gittree_config::ForgejoConfig;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForgejoMethod {
    Get,
    Post,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForgejoRequest {
    pub method: ForgejoMethod,
    pub url: String,
    pub body: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForgejoResponse {
    pub status: u16,
    pub body: String,
}

#[async_trait]
pub trait ForgejoTransport: Send + Sync {
    async fn send(&self, request: ForgejoRequest) -> Result<ForgejoResponse, ForgejoError>;
}

#[async_trait]
impl<T: ForgejoTransport + ?Sized> ForgejoTransport for std::sync::Arc<T> {
    async fn send(&self, request: ForgejoRequest) -> Result<ForgejoResponse, ForgejoError> {
        (**self).send(request).await
    }
}

#[derive(Clone)]
pub struct ReqwestTransport {
    client: reqwest::Client,
    token: String,
}

async fn map_request_error<E, Fut, T>(operation: Fut) -> Result<T, ForgejoError>
where
    E: std::fmt::Display,
    Fut: std::future::IntoFuture<Output = Result<T, E>>,
{
    operation
        .into_future()
        .await
        .map_err(|err| ForgejoError::Request(err.to_string()))
}

impl ReqwestTransport {
    pub fn new(token: impl Into<String>) -> Result<Self, ForgejoError> {
        let client = reqwest::Client::builder()
            .build()
            .map_err(|err| ForgejoError::Request(err.to_string()))?;
        Ok(Self {
            client,
            token: token.into(),
        })
    }
}

#[async_trait]
impl ForgejoTransport for ReqwestTransport {
    async fn send(&self, request: ForgejoRequest) -> Result<ForgejoResponse, ForgejoError> {
        let method = match request.method {
            ForgejoMethod::Get => reqwest::Method::GET,
            ForgejoMethod::Post => reqwest::Method::POST,
        };
        let mut builder = self.client.request(method, request.url);
        builder = builder.header("Authorization", format!("token {}", self.token));
        builder = builder.header("Accept", "application/json");
        if let Some(body) = request.body {
            builder = builder.header("Content-Type", "application/json");
            builder = builder.body(body);
        }
        let response = map_request_error(builder.send()).await?;
        let status = response.status().as_u16();
        let body = map_request_error(response.text()).await?;
        Ok(ForgejoResponse { status, body })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForgejoRepo {
    pub owner: String,
    pub name: String,
    pub full_name: String,
    pub html_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForgejoUser {
    pub username: String,
    pub email: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForgejoOrg {
    pub name: String,
    pub full_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForgejoPullRequest {
    pub number: u64,
    pub url: String,
    pub html_url: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ForgejoCreateUser {
    pub username: String,
    pub email: String,
    pub password: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub full_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub must_change_password: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub send_notify: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ForgejoCreateOrg {
    pub username: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub full_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub visibility: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ForgejoCreateRepo {
    pub name: String,
    pub description: Option<String>,
    pub private: Option<bool>,
    pub auto_init: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ForgejoCreatePullRequest {
    pub head: String,
    pub base: String,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
}

#[derive(Debug)]
pub enum ForgejoError {
    Request(String),
    Response { status: u16, body: String },
    Parse(String),
    NotFound(String),
}

impl std::fmt::Display for ForgejoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ForgejoError::Request(message) => write!(f, "forgejo request error: {message}"),
            ForgejoError::Response { status, body } => {
                write!(f, "forgejo response {status}: {body}")
            }
            ForgejoError::Parse(message) => write!(f, "forgejo parse error: {message}"),
            ForgejoError::NotFound(message) => write!(f, "forgejo not found: {message}"),
        }
    }
}

impl std::error::Error for ForgejoError {}

#[derive(Clone)]
pub struct ForgejoClient<T> {
    config: ForgejoConfig,
    transport: T,
}

impl ForgejoClient<ReqwestTransport> {
    pub fn new(config: ForgejoConfig) -> Result<Self, ForgejoError> {
        let transport = ReqwestTransport::new(config.api_token.clone())?;
        Ok(Self { config, transport })
    }
}

impl<T: ForgejoTransport> ForgejoClient<T> {
    pub fn with_transport(config: ForgejoConfig, transport: T) -> Self {
        Self { config, transport }
    }

    pub async fn ensure_repo(
        &self,
        name: &str,
        description: Option<&str>,
    ) -> Result<ForgejoRepo, ForgejoError> {
        if let Some(repo) = self.get_repo(name).await? {
            return Ok(repo);
        }
        self.create_repo(name, description).await
    }

    pub async fn ensure_repo_for_owner(
        &self,
        owner: &str,
        name: &str,
        description: Option<&str>,
    ) -> Result<ForgejoRepo, ForgejoError> {
        if let Some(repo) = self.get_repo_for_owner(owner, name).await? {
            return Ok(repo);
        }
        self.create_repo_for_owner(
            owner,
            ForgejoCreateRepo {
                name: name.to_string(),
                description: description.map(|value| value.to_string()),
                private: None,
                auto_init: None,
            },
        )
        .await
    }

    pub async fn ensure_user(&self, user: ForgejoCreateUser) -> Result<ForgejoUser, ForgejoError> {
        if let Some(existing) = self.get_user(&user.username).await? {
            return Ok(existing);
        }
        self.create_user(user).await
    }

    pub async fn ensure_webhook(&self, repo: &str) -> Result<(), ForgejoError> {
        let hooks = self.list_hooks(repo).await?;
        if hooks.iter().any(|hook| {
            hook.config
                .url
                .as_deref()
                .map(|url| url == self.config.webhook_url)
                .unwrap_or(false)
        }) {
            return Ok(());
        }
        self.create_hook(repo).await
    }

    pub async fn ensure_webhook_for_owner(
        &self,
        owner: &str,
        repo: &str,
    ) -> Result<(), ForgejoError> {
        let hooks = self.list_hooks_for_owner(owner, repo).await?;
        if hooks.iter().any(|hook| {
            hook.config
                .url
                .as_deref()
                .map(|url| url == self.config.webhook_url)
                .unwrap_or(false)
        }) {
            return Ok(());
        }
        self.create_hook_for_owner(owner, repo).await
    }

    pub async fn create_user(&self, user: ForgejoCreateUser) -> Result<ForgejoUser, ForgejoError> {
        let url = join_url(&self.config.base_url, "/api/v1/admin/users");
        let response = self
            .transport
            .send(ForgejoRequest {
                method: ForgejoMethod::Post,
                url,
                body: Some(serialize_json(&user)?),
            })
            .await?;
        match response.status {
            200 | 201 => {
                let user = parse_json::<ForgejoUserResponse>(&response.body)?;
                user.into_user()
            }
            status => Err(ForgejoError::Response {
                status,
                body: response.body,
            }),
        }
    }

    pub async fn create_org(
        &self,
        owner: &str,
        org: ForgejoCreateOrg,
    ) -> Result<ForgejoOrg, ForgejoError> {
        let url = join_url(
            &self.config.base_url,
            &format!("/api/v1/admin/users/{owner}/orgs"),
        );
        let response = self
            .transport
            .send(ForgejoRequest {
                method: ForgejoMethod::Post,
                url,
                body: Some(serialize_json(&org)?),
            })
            .await?;
        match response.status {
            200 | 201 => {
                let org = parse_json::<ForgejoOrgResponse>(&response.body)?;
                Ok(org.into_org())
            }
            status => Err(ForgejoError::Response {
                status,
                body: response.body,
            }),
        }
    }

    pub async fn create_repo_for_owner(
        &self,
        owner: &str,
        repo: ForgejoCreateRepo,
    ) -> Result<ForgejoRepo, ForgejoError> {
        let url = join_url(
            &self.config.base_url,
            &format!("/api/v1/admin/users/{owner}/repos"),
        );
        let payload = CreateAdminRepoPayload {
            name: repo.name,
            description: repo.description,
            private: repo.private.unwrap_or(self.config.repo_private),
            auto_init: repo.auto_init.unwrap_or(false),
        };
        let response = self
            .transport
            .send(ForgejoRequest {
                method: ForgejoMethod::Post,
                url,
                body: Some(serialize_json(&payload)?),
            })
            .await?;
        match response.status {
            201 => {
                let repo = parse_json::<ForgejoRepoResponse>(&response.body)?;
                Ok(repo.into_repo())
            }
            409 => self
                .get_repo_for_owner(owner, &payload.name)
                .await?
                .ok_or_else(|| ForgejoError::NotFound(payload.name.clone())),
            status => Err(ForgejoError::Response {
                status,
                body: response.body,
            }),
        }
    }

    pub async fn create_pull_request(
        &self,
        owner: &str,
        repo: &str,
        request: ForgejoCreatePullRequest,
    ) -> Result<ForgejoPullRequest, ForgejoError> {
        let url = join_url(
            &self.config.base_url,
            &format!("/api/v1/repos/{owner}/{repo}/pulls"),
        );
        let response = self
            .transport
            .send(ForgejoRequest {
                method: ForgejoMethod::Post,
                url,
                body: Some(serialize_json(&request)?),
            })
            .await?;
        match response.status {
            201 => {
                let pr = parse_json::<ForgejoPullRequestResponse>(&response.body)?;
                pr.into_pull_request()
            }
            status => Err(ForgejoError::Response {
                status,
                body: response.body,
            }),
        }
    }

    async fn get_repo(&self, name: &str) -> Result<Option<ForgejoRepo>, ForgejoError> {
        self.get_repo_for_owner(&self.config.owner, name).await
    }

    async fn get_user(&self, username: &str) -> Result<Option<ForgejoUser>, ForgejoError> {
        let url = join_url(
            &self.config.base_url,
            &format!("/api/v1/users/{username}"),
        );
        let response = self
            .transport
            .send(ForgejoRequest {
                method: ForgejoMethod::Get,
                url,
                body: None,
            })
            .await?;
        match response.status {
            200 => {
                let user = parse_json::<ForgejoUserResponse>(&response.body)?;
                user.into_user().map(Some)
            }
            404 => Ok(None),
            status => Err(ForgejoError::Response {
                status,
                body: response.body,
            }),
        }
    }

    async fn get_repo_for_owner(
        &self,
        owner: &str,
        name: &str,
    ) -> Result<Option<ForgejoRepo>, ForgejoError> {
        let url = join_url(
            &self.config.base_url,
            &format!("/api/v1/repos/{owner}/{name}"),
        );
        let response = self
            .transport
            .send(ForgejoRequest {
                method: ForgejoMethod::Get,
                url,
                body: None,
            })
            .await?;
        match response.status {
            200 => {
                let repo = parse_json::<ForgejoRepoResponse>(&response.body)?;
                Ok(Some(repo.into_repo()))
            }
            404 => Ok(None),
            status => Err(ForgejoError::Response {
                status,
                body: response.body,
            }),
        }
    }

    async fn create_repo(
        &self,
        name: &str,
        description: Option<&str>,
    ) -> Result<ForgejoRepo, ForgejoError> {
        let payload = CreateRepoPayload {
            name: name.to_string(),
            description: description.map(|value| value.to_string()),
            private: self.config.repo_private,
        };
        let org_url = join_url(
            &self.config.base_url,
            &format!("/api/v1/orgs/{}/repos", self.config.owner),
        );
        let response = self
            .transport
            .send(ForgejoRequest {
                method: ForgejoMethod::Post,
                url: org_url,
                body: Some(serialize_json(&payload)?),
            })
            .await?;
        match response.status {
            201 => {
                let repo = parse_json::<ForgejoRepoResponse>(&response.body)?;
                return Ok(repo.into_repo());
            }
            403 | 404 => {
                tracing::debug!(status = response.status, "org repo creation unavailable");
            }
            409 => {
                return self
                    .get_repo(name)
                    .await?
                    .ok_or_else(|| ForgejoError::NotFound(name.to_string()));
            }
            status => {
                return Err(ForgejoError::Response {
                    status,
                    body: response.body,
                });
            }
        }

        let user_url = join_url(&self.config.base_url, "/api/v1/user/repos");
        let response = self
            .transport
            .send(ForgejoRequest {
                method: ForgejoMethod::Post,
                url: user_url,
                body: Some(serialize_json(&payload)?),
            })
            .await?;
        match response.status {
            201 => {
                let repo = parse_json::<ForgejoRepoResponse>(&response.body)?;
                Ok(repo.into_repo())
            }
            409 => self
                .get_repo(name)
                .await?
                .ok_or_else(|| ForgejoError::NotFound(name.to_string())),
            status => Err(ForgejoError::Response {
                status,
                body: response.body,
            }),
        }
    }

    async fn list_hooks(&self, repo: &str) -> Result<Vec<ForgejoHookResponse>, ForgejoError> {
        self.list_hooks_for_owner(&self.config.owner, repo).await
    }

    async fn list_hooks_for_owner(
        &self,
        owner: &str,
        repo: &str,
    ) -> Result<Vec<ForgejoHookResponse>, ForgejoError> {
        let url = join_url(
            &self.config.base_url,
            &format!("/api/v1/repos/{owner}/{repo}/hooks"),
        );
        let response = self
            .transport
            .send(ForgejoRequest {
                method: ForgejoMethod::Get,
                url,
                body: None,
            })
            .await?;
        match response.status {
            200 => Ok(parse_json::<Vec<ForgejoHookResponse>>(&response.body)?),
            status => Err(ForgejoError::Response {
                status,
                body: response.body,
            }),
        }
    }

    async fn create_hook(&self, repo: &str) -> Result<(), ForgejoError> {
        self.create_hook_for_owner(&self.config.owner, repo).await
    }

    async fn create_hook_for_owner(
        &self,
        owner: &str,
        repo: &str,
    ) -> Result<(), ForgejoError> {
        let url = join_url(
            &self.config.base_url,
            &format!("/api/v1/repos/{owner}/{repo}/hooks"),
        );
        let payload = CreateHookPayload {
            hook_type: "gitea".to_string(),
            config: HookConfig {
                url: self.config.webhook_url.clone(),
                content_type: "json".to_string(),
                secret: self.config.webhook_secret.clone(),
            },
            events: vec!["push".to_string()],
            active: true,
        };
        let response = self
            .transport
            .send(ForgejoRequest {
                method: ForgejoMethod::Post,
                url,
                body: Some(serialize_json(&payload)?),
            })
            .await?;
        match response.status {
            200 | 201 => Ok(()),
            status => Err(ForgejoError::Response {
                status,
                body: response.body,
            }),
        }
    }
}

#[derive(Debug, Deserialize)]
struct ForgejoRepoResponse {
    full_name: String,
    name: String,
    owner: ForgejoRepoOwner,
    html_url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ForgejoRepoOwner {
    username: String,
}

impl ForgejoRepoResponse {
    fn into_repo(self) -> ForgejoRepo {
        ForgejoRepo {
            owner: self.owner.username,
            name: self.name,
            full_name: self.full_name,
            html_url: self.html_url,
        }
    }
}

#[derive(Debug, Deserialize)]
struct ForgejoUserResponse {
    login: Option<String>,
    username: Option<String>,
    email: Option<String>,
}

impl ForgejoUserResponse {
    fn into_user(self) -> Result<ForgejoUser, ForgejoError> {
        let username = match (self.login, self.username) {
            (Some(login), Some(username)) if login != username => {
                return Err(ForgejoError::Parse(
                    "forgejo user response has mismatched login and username".to_string(),
                ));
            }
            (Some(login), _) => login,
            (None, Some(username)) => username,
            (None, None) => {
                return Err(ForgejoError::Parse(
                    "forgejo user response missing username".to_string(),
                ));
            }
        };
        Ok(ForgejoUser {
            username,
            email: self.email,
        })
    }
}

#[derive(Debug, Deserialize)]
struct ForgejoOrgResponse {
    #[serde(alias = "username")]
    name: String,
    full_name: Option<String>,
}

impl ForgejoOrgResponse {
    fn into_org(self) -> ForgejoOrg {
        ForgejoOrg {
            name: self.name,
            full_name: self.full_name,
        }
    }
}

#[derive(Debug, Deserialize)]
struct ForgejoPullRequestResponse {
    number: i64,
    url: String,
    html_url: Option<String>,
}

impl ForgejoPullRequestResponse {
    fn into_pull_request(self) -> Result<ForgejoPullRequest, ForgejoError> {
        let number = u64::try_from(self.number).map_err(|_| ForgejoError::Parse(format!(
            "invalid pull request number: {}",
            self.number
        )))?;
        Ok(ForgejoPullRequest {
            number,
            url: self.url,
            html_url: self.html_url,
        })
    }
}

#[derive(Debug, Deserialize)]
struct ForgejoHookResponse {
    config: ForgejoHookConfig,
}

#[derive(Debug, Deserialize)]
struct ForgejoHookConfig {
    url: Option<String>,
}

#[derive(Debug, Serialize)]
struct CreateRepoPayload {
    name: String,
    description: Option<String>,
    private: bool,
}

#[derive(Debug, Serialize)]
struct CreateAdminRepoPayload {
    name: String,
    description: Option<String>,
    private: bool,
    #[serde(skip_serializing_if = "is_false")]
    auto_init: bool,
}

fn is_false(value: &bool) -> bool {
    !*value
}

#[derive(Debug, Serialize)]
struct CreateHookPayload {
    #[serde(rename = "type")]
    hook_type: String,
    config: HookConfig,
    events: Vec<String>,
    active: bool,
}

#[derive(Debug, Serialize)]
struct HookConfig {
    url: String,
    #[serde(rename = "content_type")]
    content_type: String,
    secret: String,
}

fn join_url(base: &str, path: &str) -> String {
    format!("{}/{}", base.trim_end_matches('/'), path.trim_start_matches('/'))
}

fn parse_json<T: for<'de> Deserialize<'de>>(input: &str) -> Result<T, ForgejoError> {
    serde_json::from_str(input).map_err(|err| ForgejoError::Parse(err.to_string()))
}

fn serialize_json<T: Serialize>(value: &T) -> Result<String, ForgejoError> {
    serde_json::to_string(value).map_err(|err| ForgejoError::Parse(err.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    #[derive(Clone, Default)]
    struct MockTransport {
        requests: Arc<Mutex<Vec<ForgejoRequest>>>,
        responses: Arc<Mutex<VecDeque<ForgejoResponse>>>,
    }

    impl MockTransport {
        fn new(responses: Vec<ForgejoResponse>) -> Self {
            Self {
                requests: Arc::new(Mutex::new(Vec::new())),
                responses: Arc::new(Mutex::new(VecDeque::from(responses))),
            }
        }

        fn requests(&self) -> Vec<ForgejoRequest> {
            self.requests.lock().expect("requests").clone()
        }
    }

    #[async_trait]
    impl ForgejoTransport for MockTransport {
        async fn send(&self, request: ForgejoRequest) -> Result<ForgejoResponse, ForgejoError> {
            self.requests.lock().expect("requests").push(request);
            self.responses
                .lock()
                .expect("responses")
                .pop_front()
                .ok_or_else(|| ForgejoError::Request("missing mock response".to_string()))
        }
    }

    fn test_config() -> ForgejoConfig {
        ForgejoConfig {
            base_url: "http://localhost:3000".to_string(),
            api_token: "token".to_string(),
            owner: "gittree".to_string(),
            webhook_url: "http://localhost:8090/".to_string(),
            webhook_secret: "secret".to_string(),
            repo_private: true,
        }
    }

    fn repo_json(owner: &str, name: &str) -> String {
        format!(
            r#"{{"full_name":"{owner}/{name}","name":"{name}","owner":{{"username":"{owner}"}},"html_url":"http://localhost/{owner}/{name}"}}"#
        )
    }

    fn user_json(username: &str) -> String {
        format!(
            r#"{{"login":"{username}","username":"{username}","email":"{username}@example.com"}}"#
        )
    }

    #[tokio::test]
    async fn map_request_error_returns_ok_value() {
        let value = super::map_request_error::<&'static str, _, _>(async {
            Ok::<u64, &'static str>(42)
        })
        .await
        .expect("ok");
        assert_eq!(value, 42);
    }

    #[tokio::test]
    async fn map_request_error_maps_error_value() {
        let err = super::map_request_error::<&'static str, _, _>(async {
            Err::<u64, &'static str>("boom")
        })
        .await
        .expect_err("error");
        assert!(matches!(err, ForgejoError::Request(message) if message.contains("boom")));
    }

    #[test]
    fn forgejo_error_display_covers_request_response_and_not_found() {
        let request = ForgejoError::Request("boom".to_string());
        assert!(format!("{request}").contains("boom"));

        let response = ForgejoError::Response {
            status: 418,
            body: "teapot".to_string(),
        };
        let message = format!("{response}");
        assert!(message.contains("418"), "{message}");
        assert!(message.contains("teapot"), "{message}");

        let not_found = ForgejoError::NotFound("missing".to_string());
        assert!(format!("{not_found}").contains("missing"));
    }

    #[test]
    fn forgejo_user_response_rejects_mismatched_login_and_username() {
        let response = ForgejoUserResponse {
            login: Some("alice".to_string()),
            username: Some("bob".to_string()),
            email: None,
        };
        let err = response.into_user().expect_err("parse");
        assert!(matches!(err, ForgejoError::Parse(_)));
    }

    #[test]
    fn forgejo_user_response_rejects_missing_username() {
        let response = ForgejoUserResponse {
            login: None,
            username: None,
            email: None,
        };
        let err = response.into_user().expect_err("parse");
        assert!(matches!(err, ForgejoError::Parse(_)));
    }

    #[test]
    fn forgejo_user_response_accepts_username_without_login() {
        let response = ForgejoUserResponse {
            login: None,
            username: Some("alice".to_string()),
            email: Some("alice@example.com".to_string()),
        };
        let user = response.into_user().expect("parse");
        assert_eq!(user.username, "alice");
        assert_eq!(user.email.as_deref(), Some("alice@example.com"));
    }

    #[tokio::test]
    async fn client_supports_arc_transport() {
        let responses = vec![ForgejoResponse {
            status: 200,
            body: repo_json("gittree", "alpha"),
        }];
        let transport = Arc::new(MockTransport::new(responses));
        let client = ForgejoClient::with_transport(test_config(), transport.clone());

        let repo = client.ensure_repo("alpha", None).await.expect("repo");
        assert_eq!(repo.full_name, "gittree/alpha");

        let requests = transport.requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].method, ForgejoMethod::Get);
    }

    #[tokio::test]
    async fn ensure_repo_returns_existing() {
        let responses = vec![ForgejoResponse {
            status: 200,
            body: repo_json("gittree", "alpha"),
        }];
        let transport = MockTransport::new(responses);
        let client = ForgejoClient::with_transport(test_config(), transport.clone());

        let repo = client
            .ensure_repo("alpha", Some("desc"))
            .await
            .expect("repo");
        assert_eq!(repo.full_name, "gittree/alpha");

        let requests = transport.requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].method, ForgejoMethod::Get);
        assert!(requests[0]
            .url
            .ends_with("/api/v1/repos/gittree/alpha"));
    }

    #[tokio::test]
    async fn ensure_repo_creates_when_missing() {
        let responses = vec![
            ForgejoResponse {
                status: 404,
                body: "not found".to_string(),
            },
            ForgejoResponse {
                status: 201,
                body: repo_json("gittree", "beta"),
            },
        ];
        let transport = MockTransport::new(responses);
        let client = ForgejoClient::with_transport(test_config(), transport.clone());

        let repo = client.ensure_repo("beta", None).await.expect("repo");
        assert_eq!(repo.full_name, "gittree/beta");

        let requests = transport.requests();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].method, ForgejoMethod::Get);
        assert_eq!(requests[1].method, ForgejoMethod::Post);
        assert!(requests[1].url.ends_with("/api/v1/orgs/gittree/repos"));
        let body = requests[1].body.clone().expect("body");
        assert!(body.contains("\"name\":\"beta\""));
    }

    #[tokio::test]
    async fn create_repo_non_success_returns_response_error() {
        let responses = vec![
            ForgejoResponse {
                status: 404,
                body: "not found".to_string(),
            },
            ForgejoResponse {
                status: 500,
                body: "nope".to_string(),
            },
        ];
        let transport = MockTransport::new(responses);
        let client = ForgejoClient::with_transport(test_config(), transport);

        let err = client
            .ensure_repo("beta", None)
            .await
            .expect_err("non success");
        assert!(matches!(
            err,
            ForgejoError::Response { status: 500, body } if body == "nope"
        ));
    }

    #[test]
    fn client_new_constructs_reqwest_transport() {
        let client = ForgejoClient::new(test_config()).expect("client");
        assert_eq!(client.config.owner, "gittree");
    }

    #[tokio::test]
    async fn ensure_webhook_creates_when_missing() {
        let responses = vec![
            ForgejoResponse {
                status: 200,
                body: r#"[]"#.to_string(),
            },
            ForgejoResponse {
                status: 201,
                body: "created".to_string(),
            },
        ];
        let transport = MockTransport::new(responses);
        let client = ForgejoClient::with_transport(test_config(), transport.clone());

        client.ensure_webhook("repo").await.expect("hook");

        let requests = transport.requests();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].method, ForgejoMethod::Get);
        assert_eq!(requests[1].method, ForgejoMethod::Post);
        assert!(requests[1]
            .url
            .ends_with("/api/v1/repos/gittree/repo/hooks"));
        let body = requests[1].body.clone().expect("body");
        assert!(body.contains("\"push\""));
        assert!(body.contains("\"secret\":\"secret\""));
    }

    #[tokio::test]
    async fn ensure_repo_for_owner_targets_owner() {
        let responses = vec![
            ForgejoResponse {
                status: 404,
                body: "not found".to_string(),
            },
            ForgejoResponse {
                status: 201,
                body: repo_json("alice", "delta"),
            },
        ];
        let transport = MockTransport::new(responses);
        let client = ForgejoClient::with_transport(test_config(), transport.clone());

        let repo = client
            .ensure_repo_for_owner("alice", "delta", None)
            .await
            .expect("repo");
        assert_eq!(repo.full_name, "alice/delta");

        let requests = transport.requests();
        assert_eq!(requests.len(), 2);
        assert!(requests[0].url.ends_with("/api/v1/repos/alice/delta"));
        assert!(requests[1]
            .url
            .ends_with("/api/v1/admin/users/alice/repos"));
    }

    #[tokio::test]
    async fn ensure_repo_for_owner_returns_existing() {
        let responses = vec![ForgejoResponse {
            status: 200,
            body: repo_json("alice", "delta"),
        }];
        let transport = MockTransport::new(responses);
        let client = ForgejoClient::with_transport(test_config(), transport.clone());

        let repo = client
            .ensure_repo_for_owner("alice", "delta", None)
            .await
            .expect("repo");
        assert_eq!(repo.full_name, "alice/delta");

        let requests = transport.requests();
        assert_eq!(requests.len(), 1);
        assert!(requests[0].url.ends_with("/api/v1/repos/alice/delta"));
    }

    #[tokio::test]
    async fn ensure_webhook_for_owner_targets_owner() {
        let responses = vec![
            ForgejoResponse {
                status: 200,
                body: r#"[]"#.to_string(),
            },
            ForgejoResponse {
                status: 201,
                body: "created".to_string(),
            },
        ];
        let transport = MockTransport::new(responses);
        let client = ForgejoClient::with_transport(test_config(), transport.clone());

        client
            .ensure_webhook_for_owner("alice", "repo")
            .await
            .expect("hook");

        let requests = transport.requests();
        assert_eq!(requests.len(), 2);
        assert!(requests[0]
            .url
            .ends_with("/api/v1/repos/alice/repo/hooks"));
        assert!(requests[1]
            .url
            .ends_with("/api/v1/repos/alice/repo/hooks"));
    }

    #[tokio::test]
    async fn ensure_webhook_for_owner_skips_create_when_already_present() {
        let responses = vec![ForgejoResponse {
            status: 200,
            body: r#"[{"config":{"url":"http://localhost:8090/"}}]"#.to_string(),
        }];
        let transport = MockTransport::new(responses);
        let client = ForgejoClient::with_transport(test_config(), transport.clone());

        client
            .ensure_webhook_for_owner("alice", "repo")
            .await
            .expect("hook");

        let requests = transport.requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].method, ForgejoMethod::Get);
        assert!(requests[0]
            .url
            .ends_with("/api/v1/repos/alice/repo/hooks"));
    }

    #[tokio::test]
    async fn create_user_posts_admin_endpoint() {
        let responses = vec![ForgejoResponse {
            status: 201,
            body: user_json("alice"),
        }];
        let transport = MockTransport::new(responses);
        let client = ForgejoClient::with_transport(test_config(), transport.clone());

        let user = client
            .create_user(ForgejoCreateUser {
                username: "alice".to_string(),
                email: "alice@example.com".to_string(),
                password: "secret".to_string(),
                full_name: None,
                must_change_password: Some(true),
                send_notify: Some(false),
            })
            .await
            .expect("user");

        assert_eq!(user.username, "alice");
        let requests = transport.requests();
        assert_eq!(requests.len(), 1);
        assert!(requests[0].url.ends_with("/api/v1/admin/users"));
        let body = requests[0].body.clone().expect("body");
        assert!(body.contains("\"username\":\"alice\""));
        assert!(body.contains("\"email\":\"alice@example.com\""));
    }

    #[tokio::test]
    async fn ensure_user_returns_existing() {
        let responses = vec![ForgejoResponse {
            status: 200,
            body: user_json("alice"),
        }];
        let transport = MockTransport::new(responses);
        let client = ForgejoClient::with_transport(test_config(), transport.clone());

        let user = client
            .ensure_user(ForgejoCreateUser {
                username: "alice".to_string(),
                email: "alice@example.com".to_string(),
                password: "secret".to_string(),
                full_name: None,
                must_change_password: Some(true),
                send_notify: Some(false),
            })
            .await
            .expect("user");

        assert_eq!(user.username, "alice");
        let requests = transport.requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].method, ForgejoMethod::Get);
        assert!(requests[0].url.ends_with("/api/v1/users/alice"));
    }

    #[tokio::test]
    async fn ensure_user_creates_when_missing() {
        let responses = vec![
            ForgejoResponse {
                status: 404,
                body: "".to_string(),
            },
            ForgejoResponse {
                status: 201,
                body: user_json("alice"),
            },
        ];
        let transport = MockTransport::new(responses);
        let client = ForgejoClient::with_transport(test_config(), transport.clone());

        let user = client
            .ensure_user(ForgejoCreateUser {
                username: "alice".to_string(),
                email: "alice@example.com".to_string(),
                password: "secret".to_string(),
                full_name: None,
                must_change_password: Some(true),
                send_notify: Some(false),
            })
            .await
            .expect("user");

        assert_eq!(user.username, "alice");
        let requests = transport.requests();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].method, ForgejoMethod::Get);
        assert!(requests[0].url.ends_with("/api/v1/users/alice"));
        assert_eq!(requests[1].method, ForgejoMethod::Post);
        assert!(requests[1].url.ends_with("/api/v1/admin/users"));
    }

    #[tokio::test]
    async fn create_org_posts_admin_endpoint() {
        let responses = vec![ForgejoResponse {
            status: 201,
            body: r#"{"name":"acme","full_name":"Acme Org"}"#.to_string(),
        }];
        let transport = MockTransport::new(responses);
        let client = ForgejoClient::with_transport(test_config(), transport.clone());

        let org = client
            .create_org(
                "admin",
                ForgejoCreateOrg {
                    username: "acme".to_string(),
                    full_name: Some("Acme Org".to_string()),
                    description: None,
                    visibility: None,
                },
            )
            .await
            .expect("org");

        assert_eq!(org.name, "acme");
        let requests = transport.requests();
        assert_eq!(requests.len(), 1);
        assert!(requests[0]
            .url
            .ends_with("/api/v1/admin/users/admin/orgs"));
    }

    #[tokio::test]
    async fn create_repo_posts_admin_endpoint() {
        let responses = vec![ForgejoResponse {
            status: 201,
            body: repo_json("alice", "demo"),
        }];
        let transport = MockTransport::new(responses);
        let client = ForgejoClient::with_transport(test_config(), transport.clone());

        let repo = client
            .create_repo_for_owner(
                "alice",
                ForgejoCreateRepo {
                    name: "demo".to_string(),
                    description: Some("demo".to_string()),
                    private: Some(false),
                    auto_init: Some(true),
                },
            )
            .await
            .expect("repo");

        assert_eq!(repo.full_name, "alice/demo");
        let requests = transport.requests();
        assert_eq!(requests.len(), 1);
        assert!(requests[0]
            .url
            .ends_with("/api/v1/admin/users/alice/repos"));
        let body = requests[0].body.clone().expect("body");
        assert!(body.contains("\"name\":\"demo\""));
        assert!(body.contains("\"auto_init\":true"));
    }

    #[tokio::test]
    async fn create_pull_request_posts_repo_endpoint() {
        let responses = vec![ForgejoResponse {
            status: 201,
            body: r#"{"number":5,"url":"http://localhost/api/v1/repos/gittree/demo/pulls/5","html_url":"http://localhost/gittree/demo/pulls/5"}"#.to_string(),
        }];
        let transport = MockTransport::new(responses);
        let client = ForgejoClient::with_transport(test_config(), transport.clone());

        let pr = client
            .create_pull_request(
                "gittree",
                "demo",
                ForgejoCreatePullRequest {
                    head: "feature".to_string(),
                    base: "main".to_string(),
                    title: "Add thing".to_string(),
                    body: Some("desc".to_string()),
                },
            )
            .await
            .expect("pr");

        assert_eq!(pr.number, 5);
        let requests = transport.requests();
        assert_eq!(requests.len(), 1);
        assert!(requests[0]
            .url
            .ends_with("/api/v1/repos/gittree/demo/pulls"));
        let body = requests[0].body.clone().expect("body");
        assert!(body.contains("\"head\":\"feature\""));
        assert!(body.contains("\"base\":\"main\""));
        assert!(body.contains("\"title\":\"Add thing\""));
    }

    #[tokio::test]
    async fn ensure_repo_falls_back_to_user_endpoint_on_org_forbidden() {
        let responses = vec![
            ForgejoResponse {
                status: 404,
                body: "missing".to_string(),
            },
            ForgejoResponse {
                status: 403,
                body: "forbidden".to_string(),
            },
            ForgejoResponse {
                status: 201,
                body: repo_json("gittree", "fallback"),
            },
        ];
        let transport = MockTransport::new(responses);
        let client = ForgejoClient::with_transport(test_config(), transport.clone());

        let repo = client.ensure_repo("fallback", None).await.expect("repo");
        assert_eq!(repo.full_name, "gittree/fallback");

        let requests = transport.requests();
        assert_eq!(requests.len(), 3);
        assert!(requests[1].url.ends_with("/api/v1/orgs/gittree/repos"));
        assert!(requests[2].url.ends_with("/api/v1/user/repos"));
    }

    #[tokio::test]
    async fn ensure_repo_falls_back_to_user_endpoint_on_org_not_found() {
        let responses = vec![
            ForgejoResponse {
                status: 404,
                body: "missing".to_string(),
            },
            ForgejoResponse {
                status: 404,
                body: "org endpoint unavailable".to_string(),
            },
            ForgejoResponse {
                status: 201,
                body: repo_json("gittree", "fallback-404"),
            },
        ];
        let transport = MockTransport::new(responses);
        let client = ForgejoClient::with_transport(test_config(), transport.clone());

        let repo = client
            .ensure_repo("fallback-404", None)
            .await
            .expect("repo");
        assert_eq!(repo.full_name, "gittree/fallback-404");

        let requests = transport.requests();
        assert_eq!(requests.len(), 3);
        assert!(requests[1].url.ends_with("/api/v1/orgs/gittree/repos"));
        assert!(requests[2].url.ends_with("/api/v1/user/repos"));
    }

    #[tokio::test]
    async fn ensure_repo_with_arc_transport_falls_back_on_org_forbidden() {
        let responses = vec![
            ForgejoResponse {
                status: 404,
                body: "missing".to_string(),
            },
            ForgejoResponse {
                status: 403,
                body: "forbidden".to_string(),
            },
            ForgejoResponse {
                status: 201,
                body: repo_json("gittree", "arc-fallback"),
            },
        ];
        let transport = Arc::new(MockTransport::new(responses));
        let client = ForgejoClient::with_transport(test_config(), transport.clone());

        let repo = client
            .ensure_repo("arc-fallback", None)
            .await
            .expect("repo");
        assert_eq!(repo.full_name, "gittree/arc-fallback");

        let requests = transport.requests();
        assert_eq!(requests.len(), 3);
        assert!(requests[1].url.ends_with("/api/v1/orgs/gittree/repos"));
        assert!(requests[2].url.ends_with("/api/v1/user/repos"));
    }

    #[tokio::test]
    async fn create_repo_for_owner_conflict_reads_existing_repo() {
        let responses = vec![
            ForgejoResponse {
                status: 409,
                body: "exists".to_string(),
            },
            ForgejoResponse {
                status: 200,
                body: repo_json("alice", "demo"),
            },
        ];
        let transport = MockTransport::new(responses);
        let client = ForgejoClient::with_transport(test_config(), transport.clone());

        let repo = client
            .create_repo_for_owner(
                "alice",
                ForgejoCreateRepo {
                    name: "demo".to_string(),
                    description: None,
                    private: None,
                    auto_init: None,
                },
            )
            .await
            .expect("repo");
        assert_eq!(repo.full_name, "alice/demo");

        let requests = transport.requests();
        assert_eq!(requests.len(), 2);
        assert!(requests[0]
            .url
            .ends_with("/api/v1/admin/users/alice/repos"));
        assert!(requests[1].url.ends_with("/api/v1/repos/alice/demo"));
    }

    #[tokio::test]
    async fn create_repo_for_owner_conflict_missing_repo_returns_not_found() {
        let responses = vec![
            ForgejoResponse {
                status: 409,
                body: "exists".to_string(),
            },
            ForgejoResponse {
                status: 404,
                body: "missing".to_string(),
            },
        ];
        let transport = MockTransport::new(responses);
        let client = ForgejoClient::with_transport(test_config(), transport);

        let err = client
            .create_repo_for_owner(
                "alice",
                ForgejoCreateRepo {
                    name: "demo".to_string(),
                    description: None,
                    private: None,
                    auto_init: None,
                },
            )
            .await
            .expect_err("missing repo should fail");
        assert!(matches!(err, ForgejoError::NotFound(name) if name == "demo"));
    }

    #[tokio::test]
    async fn ensure_webhook_skips_create_when_already_present() {
        let responses = vec![ForgejoResponse {
            status: 200,
            body: r#"[{"config":{"url":"http://localhost:8090/"}}]"#.to_string(),
        }];
        let transport = MockTransport::new(responses);
        let client = ForgejoClient::with_transport(test_config(), transport.clone());

        client.ensure_webhook("repo").await.expect("hook");
        let requests = transport.requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].method, ForgejoMethod::Get);
    }

    #[tokio::test]
    async fn create_pull_request_rejects_negative_number() {
        let responses = vec![ForgejoResponse {
            status: 201,
            body: r#"{"number":-1,"url":"http://localhost/api/v1/repos/gittree/demo/pulls/-1"}"#
                .to_string(),
        }];
        let transport = MockTransport::new(responses);
        let client = ForgejoClient::with_transport(test_config(), transport);

        let err = client
            .create_pull_request(
                "gittree",
                "demo",
                ForgejoCreatePullRequest {
                    head: "feature".to_string(),
                    base: "main".to_string(),
                    title: "Add thing".to_string(),
                    body: None,
                },
            )
            .await
            .expect_err("negative number should fail");
        assert!(matches!(err, ForgejoError::Parse(message) if message.contains("invalid pull request number")));
    }

    #[tokio::test]
    async fn create_user_non_success_returns_response_error() {
        let responses = vec![ForgejoResponse {
            status: 409,
            body: "already exists".to_string(),
        }];
        let transport = MockTransport::new(responses);
        let client = ForgejoClient::with_transport(test_config(), transport);

        let err = client
            .create_user(ForgejoCreateUser {
                username: "alice".to_string(),
                email: "alice@example.com".to_string(),
                password: "secret".to_string(),
                full_name: None,
                must_change_password: None,
                send_notify: None,
            })
            .await
            .expect_err("status error");
        assert!(matches!(err, ForgejoError::Response { status: 409, .. }));
    }

    #[tokio::test]
    async fn create_org_non_success_returns_response_error() {
        let responses = vec![ForgejoResponse {
            status: 500,
            body: "oops".to_string(),
        }];
        let transport = MockTransport::new(responses);
        let client = ForgejoClient::with_transport(test_config(), transport);

        let err = client
            .create_org(
                "admin",
                ForgejoCreateOrg {
                    username: "acme".to_string(),
                    full_name: None,
                    description: None,
                    visibility: None,
                },
            )
            .await
            .expect_err("status error");
        assert!(matches!(err, ForgejoError::Response { status: 500, .. }));
    }

    #[tokio::test]
    async fn create_pull_request_non_success_returns_response_error() {
        let responses = vec![ForgejoResponse {
            status: 422,
            body: "invalid branch".to_string(),
        }];
        let transport = MockTransport::new(responses);
        let client = ForgejoClient::with_transport(test_config(), transport);

        let err = client
            .create_pull_request(
                "gittree",
                "demo",
                ForgejoCreatePullRequest {
                    head: "feature".to_string(),
                    base: "main".to_string(),
                    title: "Add thing".to_string(),
                    body: None,
                },
            )
            .await
            .expect_err("status error");
        assert!(matches!(err, ForgejoError::Response { status: 422, .. }));
    }

    #[tokio::test]
    async fn ensure_webhook_returns_error_when_listing_hooks_fails() {
        let responses = vec![ForgejoResponse {
            status: 500,
            body: "down".to_string(),
        }];
        let transport = MockTransport::new(responses);
        let client = ForgejoClient::with_transport(test_config(), transport);

        let err = client
            .ensure_webhook("repo")
            .await
            .expect_err("hook list failure should surface");
        assert!(matches!(err, ForgejoError::Response { status: 500, .. }));
    }

    #[tokio::test]
    async fn ensure_webhook_returns_error_when_create_hook_fails() {
        let responses = vec![
            ForgejoResponse {
                status: 200,
                body: "[]".to_string(),
            },
            ForgejoResponse {
                status: 500,
                body: "down".to_string(),
            },
        ];
        let transport = MockTransport::new(responses);
        let client = ForgejoClient::with_transport(test_config(), transport);

        let err = client
            .ensure_webhook("repo")
            .await
            .expect_err("hook create failure should surface");
        assert!(matches!(err, ForgejoError::Response { status: 500, .. }));
    }

    #[tokio::test]
    async fn create_repo_for_owner_rejects_invalid_response_payload() {
        let responses = vec![ForgejoResponse {
            status: 201,
            body: "{}".to_string(),
        }];
        let transport = MockTransport::new(responses);
        let client = ForgejoClient::with_transport(test_config(), transport);

        let err = client
            .create_repo_for_owner(
                "alice",
                ForgejoCreateRepo {
                    name: "demo".to_string(),
                    description: None,
                    private: None,
                    auto_init: None,
                },
            )
            .await
            .expect_err("invalid payload should fail");
        assert!(matches!(err, ForgejoError::Parse(_)));
    }

    #[test]
    fn forgejo_error_display_covers_parse_variant() {
        let err = ForgejoError::Parse("bad payload".to_string());
        assert_eq!(err.to_string(), "forgejo parse error: bad payload");
    }

    #[tokio::test]
    async fn ensure_user_propagates_unexpected_get_user_status() {
        let responses = vec![ForgejoResponse {
            status: 500,
            body: "down".to_string(),
        }];
        let transport = MockTransport::new(responses);
        let client = ForgejoClient::with_transport(test_config(), transport);
        let err = client
            .ensure_user(ForgejoCreateUser {
                username: "alice".to_string(),
                email: "alice@example.com".to_string(),
                password: "secret".to_string(),
                full_name: None,
                must_change_password: None,
                send_notify: None,
            })
            .await
            .expect_err("unexpected status should fail");
        assert!(matches!(err, ForgejoError::Response { status: 500, .. }));
    }

    #[tokio::test]
    async fn ensure_repo_for_owner_propagates_unexpected_lookup_status() {
        let responses = vec![ForgejoResponse {
            status: 500,
            body: "down".to_string(),
        }];
        let transport = MockTransport::new(responses);
        let client = ForgejoClient::with_transport(test_config(), transport);
        let err = client
            .ensure_repo_for_owner("alice", "demo", None)
            .await
            .expect_err("unexpected status should fail");
        assert!(matches!(err, ForgejoError::Response { status: 500, .. }));
    }

    #[tokio::test]
    async fn create_repo_for_owner_propagates_non_conflict_failure() {
        let responses = vec![ForgejoResponse {
            status: 503,
            body: "down".to_string(),
        }];
        let transport = MockTransport::new(responses);
        let client = ForgejoClient::with_transport(test_config(), transport);
        let err = client
            .create_repo_for_owner(
                "alice",
                ForgejoCreateRepo {
                    name: "demo".to_string(),
                    description: None,
                    private: None,
                    auto_init: None,
                },
            )
            .await
            .expect_err("unexpected status should fail");
        assert!(matches!(err, ForgejoError::Response { status: 503, .. }));
    }

    #[tokio::test]
    async fn ensure_repo_conflict_without_existing_repo_returns_not_found() {
        let responses = vec![
            ForgejoResponse {
                status: 404,
                body: "missing".to_string(),
            },
            ForgejoResponse {
                status: 409,
                body: "exists".to_string(),
            },
            ForgejoResponse {
                status: 404,
                body: "missing".to_string(),
            },
        ];
        let transport = MockTransport::new(responses);
        let client = ForgejoClient::with_transport(test_config(), transport);
        let err = client
            .ensure_repo("demo", None)
            .await
            .expect_err("conflict fallback should fail when repo is still missing");
        assert!(matches!(err, ForgejoError::NotFound(name) if name == "demo"));
    }

    #[tokio::test]
    async fn ensure_repo_user_fallback_conflict_and_error_paths_are_mapped() {
        let responses = vec![
            ForgejoResponse {
                status: 404,
                body: "missing".to_string(),
            },
            ForgejoResponse {
                status: 403,
                body: "forbidden".to_string(),
            },
            ForgejoResponse {
                status: 409,
                body: "exists".to_string(),
            },
            ForgejoResponse {
                status: 404,
                body: "missing".to_string(),
            },
        ];
        let transport = MockTransport::new(responses);
        let client = ForgejoClient::with_transport(test_config(), transport);
        let err = client
            .ensure_repo("demo", None)
            .await
            .expect_err("user fallback conflict should fail without existing repo");
        assert!(matches!(err, ForgejoError::NotFound(name) if name == "demo"));

        let responses = vec![
            ForgejoResponse {
                status: 404,
                body: "missing".to_string(),
            },
            ForgejoResponse {
                status: 403,
                body: "forbidden".to_string(),
            },
            ForgejoResponse {
                status: 500,
                body: "down".to_string(),
            },
        ];
        let transport = MockTransport::new(responses);
        let client = ForgejoClient::with_transport(test_config(), transport);
        let err = client
            .ensure_repo("demo", None)
            .await
            .expect_err("user fallback error should propagate");
        assert!(matches!(err, ForgejoError::Response { status: 500, .. }));
    }

    #[tokio::test]
    async fn reqwest_transport_maps_request_errors() {
        let transport = ReqwestTransport::new("token").expect("transport");
        let err = transport
            .send(ForgejoRequest {
                method: ForgejoMethod::Get,
                url: "http://[::1".to_string(),
                body: None,
            })
            .await
            .expect_err("invalid url should fail");
        assert!(matches!(err, ForgejoError::Request(_)));
    }

    #[tokio::test]
    async fn mock_transport_returns_request_error_when_queue_is_empty() {
        let transport = MockTransport::new(Vec::new());
        let err = transport
            .send(ForgejoRequest {
                method: ForgejoMethod::Get,
                url: "http://localhost/api/v1/user".to_string(),
                body: None,
            })
            .await
            .expect_err("missing response should fail");
        assert!(matches!(err, ForgejoError::Request(_)));
    }
}

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

#[derive(Clone)]
pub struct ReqwestTransport {
    client: reqwest::Client,
    token: String,
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
        let response = builder
            .send()
            .await
            .map_err(|err| ForgejoError::Request(err.to_string()))?;
        let status = response.status().as_u16();
        let body = response
            .text()
            .await
            .map_err(|err| ForgejoError::Request(err.to_string()))?;
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

    async fn get_repo(&self, name: &str) -> Result<Option<ForgejoRepo>, ForgejoError> {
        let url = join_url(
            &self.config.base_url,
            &format!("/api/v1/repos/{}/{}", self.config.owner, name),
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
                tracing::debug!(
                    status = response.status,
                    "org repo creation unavailable; falling back to user endpoint"
                );
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
        let url = join_url(
            &self.config.base_url,
            &format!("/api/v1/repos/{}/{}/hooks", self.config.owner, repo),
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
        let url = join_url(
            &self.config.base_url,
            &format!("/api/v1/repos/{}/{}/hooks", self.config.owner, repo),
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
}

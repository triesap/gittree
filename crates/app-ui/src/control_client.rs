use serde::{Deserialize, Serialize};
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use web_sys::{Headers, Request, RequestInit, RequestMode, Response};

const CONTROL_EVENT_KIND: u32 = 29_000;

#[derive(Debug, Clone)]
pub struct ControlRepoInput {
    pub name: String,
    pub owner: Option<String>,
    pub identifier: Option<String>,
    pub description: Option<String>,
    pub private: Option<bool>,
    pub pubkey: String,
    pub privkey: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ControlRepoResponse {
    pub owner: String,
    pub name: String,
    pub html_url: Option<String>,
}

#[derive(Debug)]
pub enum ControlClientError {
    MissingWindow,
    MissingToken,
    MissingEndpoint,
    Request(String),
    InvalidResponse(String),
    ControlFailed(String),
}

impl std::fmt::Display for ControlClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ControlClientError::MissingWindow => write!(f, "missing window"),
            ControlClientError::MissingToken => write!(f, "missing control token"),
            ControlClientError::MissingEndpoint => write!(f, "missing control url"),
            ControlClientError::Request(message) => write!(f, "request error: {message}"),
            ControlClientError::InvalidResponse(message) => {
                write!(f, "invalid response: {message}")
            }
            ControlClientError::ControlFailed(message) => {
                write!(f, "control request failed: {message}")
            }
        }
    }
}

impl std::error::Error for ControlClientError {}

#[derive(Debug, Serialize)]
struct ControlCreateRepoAction {
    action: &'static str,
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    owner: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    identifier: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    private: Option<bool>,
    pubkey: String,
    privkey: String,
}

#[derive(Debug, Serialize)]
struct ControlEventRequest {
    kind: u32,
    pubkey: String,
    content: String,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
enum ControlEventResponse {
    CreateRepo { repo: ControlRepoResponse },
}

pub fn control_event_endpoint(control_url: &str) -> Option<String> {
    let trimmed = control_url.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(format!(
            "{}/control/events",
            trimmed.trim_end_matches('/')
        ))
    }
}

pub async fn create_repo(
    control_url: &str,
    token: &str,
    input: ControlRepoInput,
) -> Result<ControlRepoResponse, ControlClientError> {
    let endpoint = control_event_endpoint(control_url)
        .ok_or(ControlClientError::MissingEndpoint)?;
    let token = token.trim();
    if token.is_empty() {
        return Err(ControlClientError::MissingToken);
    }

    let action = ControlCreateRepoAction {
        action: "create_repo",
        name: input.name,
        owner: input.owner,
        identifier: input.identifier,
        description: input.description,
        private: input.private,
        pubkey: input.pubkey,
        privkey: input.privkey,
    };
    let pubkey = action.pubkey.clone();
    let content = serde_json::to_string(&action)
        .map_err(|err| ControlClientError::InvalidResponse(err.to_string()))?;
    let request = ControlEventRequest {
        kind: CONTROL_EVENT_KIND,
        pubkey,
        content,
    };

    let body = serde_json::to_string(&request)
        .map_err(|err| ControlClientError::InvalidResponse(err.to_string()))?;

    let init = RequestInit::new();
    init.set_method("POST");
    init.set_mode(RequestMode::Cors);
    init.set_body(&JsValue::from_str(&body));

    let headers = Headers::new().map_err(request_error)?;
    headers
        .set("Authorization", &format!("Bearer {token}"))
        .map_err(request_error)?;
    headers.set("Accept", "application/json").map_err(request_error)?;
    headers
        .set("Content-Type", "application/json")
        .map_err(request_error)?;
    init.set_headers(&headers);

    let request =
        Request::new_with_str_and_init(&endpoint, &init).map_err(request_error)?;
    let window = web_sys::window().ok_or(ControlClientError::MissingWindow)?;
    let response = JsFuture::from(window.fetch_with_request(&request))
        .await
        .map_err(request_error)?;
    let response: Response = response.dyn_into().map_err(request_error)?;
    let status = response.status();
    let text = response.text().map_err(request_error)?;
    let text = JsFuture::from(text).await.map_err(request_error)?;
    let body = text.as_string().unwrap_or_default();

    if (200..300).contains(&status) {
        match serde_json::from_str::<ControlEventResponse>(&body)
            .map_err(|err| ControlClientError::InvalidResponse(err.to_string()))?
        {
            ControlEventResponse::CreateRepo { repo } => Ok(repo),
        }
    } else if body.trim().is_empty() {
        Err(ControlClientError::ControlFailed(format!(
            "status {status}"
        )))
    } else {
        Err(ControlClientError::ControlFailed(body))
    }
}

fn request_error(value: JsValue) -> ControlClientError {
    ControlClientError::Request(js_error(value))
}

fn js_error(value: JsValue) -> String {
    value
        .as_string()
        .unwrap_or_else(|| format!("{:?}", value))
}

#[cfg(test)]
mod tests {
    use super::{control_event_endpoint, ControlRepoInput, create_repo};

    #[test]
    fn control_event_endpoint_joins_path() {
        let endpoint =
            control_event_endpoint("http://localhost:8088/").expect("endpoint");
        assert_eq!(endpoint, "http://localhost:8088/control/events");
    }

    #[test]
    fn control_event_endpoint_rejects_empty() {
        assert!(control_event_endpoint("").is_none());
    }

    #[test]
    fn control_repo_input_is_sendable() {
        let input = ControlRepoInput {
            name: "hello".to_string(),
            owner: None,
            identifier: None,
            description: None,
            private: Some(true),
            pubkey: "11".repeat(32),
            privkey: "22".repeat(32),
        };
        let _ = input;
    }

    #[tokio::test]
    async fn create_repo_rejects_missing_token() {
        let input = ControlRepoInput {
            name: "hello".to_string(),
            owner: None,
            identifier: None,
            description: None,
            private: Some(true),
            pubkey: "11".repeat(32),
            privkey: "22".repeat(32),
        };
        let err = create_repo("http://localhost:8088", "", input)
            .await
            .expect_err("error");
        assert!(err.to_string().contains("missing control token"));
    }
}

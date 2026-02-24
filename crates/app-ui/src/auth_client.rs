use crate::auth::auth_header;
use gittree_app_core::Nip98Event;
use serde::Deserialize;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use web_sys::{Headers, Request, RequestInit, RequestMode, Response};

#[derive(Clone, Debug, Deserialize)]
pub struct SignupResponse {
    pub pubkey: String,
    pub username: String,
    pub status: String,
}

#[derive(Clone, Debug, Deserialize)]
struct SignupErrorResponse {
    pub error: String,
}

#[derive(Debug)]
pub enum AuthClientError {
    MissingWindow,
    Request(String),
    InvalidResponse(String),
    SignupFailed(String),
}

impl std::fmt::Display for AuthClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AuthClientError::MissingWindow => write!(f, "missing window"),
            AuthClientError::Request(message) => write!(f, "request error: {message}"),
            AuthClientError::InvalidResponse(message) => write!(f, "invalid response: {message}"),
            AuthClientError::SignupFailed(message) => write!(f, "signup failed: {message}"),
        }
    }
}

impl std::error::Error for AuthClientError {}

pub fn signup_endpoint(auth_url: &str) -> Option<String> {
    let trimmed = auth_url.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(format!("{}/v1/signup", trimmed.trim_end_matches('/')))
}

pub async fn signup(
    auth_endpoint: &str,
    event: Nip98Event,
) -> Result<SignupResponse, AuthClientError> {
    let header = auth_header(&event).map_err(|err| AuthClientError::Request(err.to_string()))?;
    let init = RequestInit::new();
    init.set_method("POST");
    init.set_mode(RequestMode::Cors);

    let headers = Headers::new().map_err(request_error)?;
    headers
        .set("Authorization", &header)
        .map_err(request_error)?;
    headers
        .set("Accept", "application/json")
        .map_err(request_error)?;
    init.set_headers(&headers);

    let request = Request::new_with_str_and_init(auth_endpoint, &init).map_err(request_error)?;
    let window = web_sys::window().ok_or(AuthClientError::MissingWindow)?;
    let response = JsFuture::from(window.fetch_with_request(&request))
        .await
        .map_err(request_error)?;
    let response: Response = response.dyn_into().map_err(request_error)?;
    let status = response.status();
    let text = response.text().map_err(request_error)?;
    let text = JsFuture::from(text).await.map_err(request_error)?;
    let body = text.as_string().unwrap_or_default();
    parse_signup_response(status, &body)
}

fn parse_signup_response(status: u16, body: &str) -> Result<SignupResponse, AuthClientError> {
    if (200..300).contains(&status) {
        serde_json::from_str::<SignupResponse>(body)
            .map_err(|err| AuthClientError::InvalidResponse(err.to_string()))
    } else {
        Err(AuthClientError::SignupFailed(parse_signup_error(body)))
    }
}

fn parse_signup_error(body: &str) -> String {
    if let Ok(parsed) = serde_json::from_str::<SignupErrorResponse>(body) {
        return parsed.error;
    }
    if body.trim().is_empty() {
        "unknown error".to_string()
    } else {
        body.to_string()
    }
}

fn request_error(value: JsValue) -> AuthClientError {
    AuthClientError::Request(js_error(value))
}

fn js_error(value: JsValue) -> String {
    value.as_string().unwrap_or_else(|| format!("{:?}", value))
}

#[cfg(test)]
mod tests {
    use super::{AuthClientError, parse_signup_error, parse_signup_response, signup_endpoint};

    #[test]
    fn signup_endpoint_joins_paths() {
        let endpoint = signup_endpoint("http://localhost:8089").expect("endpoint");
        assert_eq!(endpoint, "http://localhost:8089/v1/signup");
    }

    #[test]
    fn signup_endpoint_rejects_empty() {
        assert!(signup_endpoint("").is_none());
    }

    #[test]
    fn parse_signup_error_prefers_json() {
        let error = parse_signup_error("{\"error\":\"bad\"}");
        assert_eq!(error, "bad");
    }

    #[test]
    fn parse_signup_error_falls_back_to_body() {
        let error = parse_signup_error("plain");
        assert_eq!(error, "plain");
    }

    #[test]
    fn parse_signup_response_decodes_success_json() {
        let response = parse_signup_response(
            201,
            "{\"pubkey\":\"11\",\"username\":\"gt_demo\",\"status\":\"created\"}",
        )
        .expect("signup response");
        assert_eq!(response.pubkey, "11");
        assert_eq!(response.username, "gt_demo");
        assert_eq!(response.status, "created");
    }

    #[test]
    fn parse_signup_response_rejects_invalid_success_json() {
        let error = parse_signup_response(200, "{}").expect_err("invalid response");
        assert!(matches!(error, AuthClientError::InvalidResponse(_)));
    }

    #[test]
    fn parse_signup_response_converts_non_success_to_signup_failed() {
        let error =
            parse_signup_response(401, "{\"error\":\"denied\"}").expect_err("signup failed");
        match error {
            AuthClientError::SignupFailed(message) => assert_eq!(message, "denied"),
            other => panic!("unexpected error: {other}"),
        }
    }
}

use crate::auth::auth_header;
use gittree_app_core::{Nip98Event, Profile};
use serde::Deserialize;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use web_sys::{Headers, Request, RequestInit, RequestMode, Response};

#[derive(Clone, Debug, Deserialize)]
struct ProfileErrorResponse {
    pub error: String,
}

#[derive(Debug)]
pub enum ProfileClientError {
    MissingWindow,
    Request(String),
    InvalidResponse(String),
    ProfileFailed(String),
}

impl std::fmt::Display for ProfileClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProfileClientError::MissingWindow => write!(f, "missing window"),
            ProfileClientError::Request(message) => write!(f, "request error: {message}"),
            ProfileClientError::InvalidResponse(message) => {
                write!(f, "invalid response: {message}")
            }
            ProfileClientError::ProfileFailed(message) => write!(f, "profile failed: {message}"),
        }
    }
}

impl std::error::Error for ProfileClientError {}

pub fn profile_endpoint(auth_url: &str) -> Option<String> {
    let trimmed = auth_url.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(format!("{}/v1/profile", trimmed.trim_end_matches('/')))
}

pub fn public_profile_endpoint(auth_url: &str, npub: &str) -> Option<String> {
    let trimmed = auth_url.trim();
    let npub = npub.trim();
    if trimmed.is_empty() || npub.is_empty() {
        return None;
    }
    Some(format!(
        "{}/v1/profile/{}",
        trimmed.trim_end_matches('/'),
        npub
    ))
}

pub async fn fetch_profile(
    auth_endpoint: &str,
    event: Nip98Event,
) -> Result<Profile, ProfileClientError> {
    let header = auth_header(&event).map_err(|err| ProfileClientError::Request(err.to_string()))?;
    let init = RequestInit::new();
    init.set_method("GET");
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
    let window = web_sys::window().ok_or(ProfileClientError::MissingWindow)?;
    let response = JsFuture::from(window.fetch_with_request(&request))
        .await
        .map_err(request_error)?;
    let response: Response = response.dyn_into().map_err(request_error)?;
    read_profile_response(response).await
}

pub async fn fetch_public_profile(auth_endpoint: &str) -> Result<Profile, ProfileClientError> {
    let init = RequestInit::new();
    init.set_method("GET");
    init.set_mode(RequestMode::Cors);
    let headers = Headers::new().map_err(request_error)?;
    headers
        .set("Accept", "application/json")
        .map_err(request_error)?;
    init.set_headers(&headers);

    let request = Request::new_with_str_and_init(auth_endpoint, &init).map_err(request_error)?;
    let window = web_sys::window().ok_or(ProfileClientError::MissingWindow)?;
    let response = JsFuture::from(window.fetch_with_request(&request))
        .await
        .map_err(request_error)?;
    let response: Response = response.dyn_into().map_err(request_error)?;
    read_profile_response(response).await
}

pub async fn update_profile(
    auth_endpoint: &str,
    event: Nip98Event,
    body: Vec<u8>,
) -> Result<Profile, ProfileClientError> {
    let header = auth_header(&event).map_err(|err| ProfileClientError::Request(err.to_string()))?;
    let init = RequestInit::new();
    init.set_method("PATCH");
    init.set_mode(RequestMode::Cors);
    init.set_body(&JsValue::from(body));

    let headers = Headers::new().map_err(request_error)?;
    headers
        .set("Authorization", &header)
        .map_err(request_error)?;
    headers
        .set("Accept", "application/json")
        .map_err(request_error)?;
    headers
        .set("Content-Type", "application/json")
        .map_err(request_error)?;
    init.set_headers(&headers);

    let request = Request::new_with_str_and_init(auth_endpoint, &init).map_err(request_error)?;
    let window = web_sys::window().ok_or(ProfileClientError::MissingWindow)?;
    let response = JsFuture::from(window.fetch_with_request(&request))
        .await
        .map_err(request_error)?;
    let response: Response = response.dyn_into().map_err(request_error)?;
    read_profile_response(response).await
}

async fn read_profile_response(response: Response) -> Result<Profile, ProfileClientError> {
    let status = response.status();
    let text = response.text().map_err(request_error)?;
    let text = JsFuture::from(text).await.map_err(request_error)?;
    let body = text.as_string().unwrap_or_default();
    parse_profile_response(status, &body)
}

fn parse_profile_response(status: u16, body: &str) -> Result<Profile, ProfileClientError> {
    if (200..300).contains(&status) {
        serde_json::from_str::<Profile>(body)
            .map_err(|err| ProfileClientError::InvalidResponse(err.to_string()))
    } else {
        Err(ProfileClientError::ProfileFailed(parse_profile_error(body)))
    }
}

fn parse_profile_error(body: &str) -> String {
    if let Ok(parsed) = serde_json::from_str::<ProfileErrorResponse>(body) {
        return parsed.error;
    }
    if body.trim().is_empty() {
        "unknown error".to_string()
    } else {
        body.to_string()
    }
}

fn request_error(value: JsValue) -> ProfileClientError {
    ProfileClientError::Request(js_error(value))
}

fn js_error(value: JsValue) -> String {
    value.as_string().unwrap_or_else(|| format!("{:?}", value))
}

#[cfg(test)]
mod tests {
    use super::{
        ProfileClientError, parse_profile_error, parse_profile_response, profile_endpoint,
        public_profile_endpoint,
    };
    use gittree_app_core::{Profile, ProfileVisibility};

    #[test]
    fn profile_endpoint_joins_paths() {
        let endpoint = profile_endpoint("http://localhost:8089").expect("endpoint");
        assert_eq!(endpoint, "http://localhost:8089/v1/profile");
    }

    #[test]
    fn profile_endpoint_rejects_empty() {
        assert!(profile_endpoint("").is_none());
    }

    #[test]
    fn public_profile_endpoint_joins_paths() {
        let endpoint =
            public_profile_endpoint("http://localhost:8089", "npub1test").expect("endpoint");
        assert_eq!(endpoint, "http://localhost:8089/v1/profile/npub1test");
    }

    #[test]
    fn public_profile_endpoint_rejects_empty() {
        assert!(public_profile_endpoint("", "npub1").is_none());
        assert!(public_profile_endpoint("http://localhost:8089", "").is_none());
    }

    #[test]
    fn parse_profile_error_prefers_json() {
        let error = parse_profile_error("{\"error\":\"bad\"}");
        assert_eq!(error, "bad");
    }

    #[test]
    fn parse_profile_error_falls_back_to_body() {
        let error = parse_profile_error("plain");
        assert_eq!(error, "plain");
    }

    #[test]
    fn parse_profile_response_decodes_success_json() {
        let response = parse_profile_response(
            200,
            &serde_json::to_string(&sample_profile()).expect("json"),
        )
        .expect("profile");
        assert_eq!(response.username, "gt_demo");
        assert_eq!(response.visibility, ProfileVisibility::Public);
    }

    #[test]
    fn parse_profile_response_rejects_invalid_success_json() {
        let error = parse_profile_response(200, "{}").expect_err("invalid response");
        assert!(matches!(error, ProfileClientError::InvalidResponse(_)));
    }

    #[test]
    fn parse_profile_response_converts_non_success_to_profile_failed() {
        let error = parse_profile_response(404, "{\"error\":\"missing\"}").expect_err("missing");
        match error {
            ProfileClientError::ProfileFailed(message) => assert_eq!(message, "missing"),
            other => panic!("unexpected error: {other}"),
        }
    }

    fn sample_profile() -> Profile {
        Profile {
            pubkey: "11".repeat(32),
            username: "gt_demo".to_string(),
            display_name: Some("demo".to_string()),
            bio: Some("hello".to_string()),
            avatar_url: None,
            website_url: None,
            location: None,
            visibility: ProfileVisibility::Public,
            created_at: 1_700_000_000,
            updated_at: 1_700_000_001,
        }
    }
}

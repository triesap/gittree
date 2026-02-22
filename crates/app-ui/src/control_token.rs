#[cfg(target_arch = "wasm32")]
use wasm_bindgen::JsValue;
#[cfg(target_arch = "wasm32")]
use web_sys::{Storage, Window};

#[cfg(target_arch = "wasm32")]
const CONTROL_TOKEN_KEY: &str = "gittree_control_token";

#[derive(Debug)]
pub enum ControlTokenError {
    #[cfg(target_arch = "wasm32")]
    MissingWindow,
    #[cfg(target_arch = "wasm32")]
    MissingStorage,
    #[cfg(target_arch = "wasm32")]
    Js(String),
}

impl std::fmt::Display for ControlTokenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        #[cfg(target_arch = "wasm32")]
        {
            match self {
                ControlTokenError::MissingWindow => write!(f, "missing window"),
                ControlTokenError::MissingStorage => write!(f, "missing storage"),
                ControlTokenError::Js(message) => write!(f, "js error: {message}"),
            }
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = f;
            Ok(())
        }
    }
}

impl std::error::Error for ControlTokenError {}

pub fn load_control_token() -> Result<Option<String>, ControlTokenError> {
    load_control_token_inner()
}

pub fn store_control_token(token: &str) -> Result<(), ControlTokenError> {
    store_control_token_inner(token)
}

pub fn clear_control_token() -> Result<(), ControlTokenError> {
    clear_control_token_inner()
}

#[cfg(target_arch = "wasm32")]
fn load_control_token_inner() -> Result<Option<String>, ControlTokenError> {
    let storage = local_storage()?;
    let value = storage
        .get_item(CONTROL_TOKEN_KEY)
        .map_err(|err| ControlTokenError::Js(js_error(err)))?;
    let Some(value) = value else {
        return Ok(None);
    };
    let trimmed = value.trim();
    if trimmed.is_empty() {
        Ok(None)
    } else {
        Ok(Some(trimmed.to_string()))
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn load_control_token_inner() -> Result<Option<String>, ControlTokenError> {
    Ok(None)
}

#[cfg(target_arch = "wasm32")]
fn store_control_token_inner(token: &str) -> Result<(), ControlTokenError> {
    let storage = local_storage()?;
    let trimmed = token.trim();
    if trimmed.is_empty() {
        storage
            .remove_item(CONTROL_TOKEN_KEY)
            .map_err(|err| ControlTokenError::Js(js_error(err)))
    } else {
        storage
            .set_item(CONTROL_TOKEN_KEY, trimmed)
            .map_err(|err| ControlTokenError::Js(js_error(err)))
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn store_control_token_inner(_token: &str) -> Result<(), ControlTokenError> {
    Ok(())
}

#[cfg(target_arch = "wasm32")]
fn clear_control_token_inner() -> Result<(), ControlTokenError> {
    let storage = local_storage()?;
    storage
        .remove_item(CONTROL_TOKEN_KEY)
        .map_err(|err| ControlTokenError::Js(js_error(err)))
}

#[cfg(not(target_arch = "wasm32"))]
fn clear_control_token_inner() -> Result<(), ControlTokenError> {
    Ok(())
}

#[cfg(target_arch = "wasm32")]
fn local_storage() -> Result<Storage, ControlTokenError> {
    let window = window_ref()?;
    window
        .local_storage()
        .map_err(|err| ControlTokenError::Js(js_error(err)))?
        .ok_or(ControlTokenError::MissingStorage)
}

#[cfg(target_arch = "wasm32")]
fn window_ref() -> Result<Window, ControlTokenError> {
    web_sys::window().ok_or(ControlTokenError::MissingWindow)
}

#[cfg(target_arch = "wasm32")]
fn js_error(value: JsValue) -> String {
    value.as_string().unwrap_or_else(|| format!("{:?}", value))
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::{clear_control_token, load_control_token, store_control_token};

    #[test]
    fn native_control_token_load_returns_none() {
        assert_eq!(load_control_token().expect("load token"), None);
    }

    #[test]
    fn native_control_token_store_and_clear_are_noops() {
        store_control_token("token").expect("store token");
        clear_control_token().expect("clear token");
        assert_eq!(load_control_token().expect("load token"), None);
    }
}

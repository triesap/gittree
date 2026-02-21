#![forbid(unsafe_code)]

pub const APP_THEME_NAME: &str = "gittree";

#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GittreeAppThemeError {
    Unavailable,
}

pub type GittreeAppThemeResult<T> = Result<T, GittreeAppThemeError>;

impl GittreeAppThemeError {
    pub const fn message(&self) -> &'static str {
        match self {
            GittreeAppThemeError::Unavailable => "error.app.theme.unavailable",
        }
    }
}

impl std::fmt::Display for GittreeAppThemeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message())
    }
}

impl std::error::Error for GittreeAppThemeError {}

pub fn app_theme_init() -> GittreeAppThemeResult<&'static str> {
    app_theme_apply_name(APP_THEME_NAME)?;
    Ok(APP_THEME_NAME)
}

#[cfg(target_arch = "wasm32")]
fn app_theme_apply_name(name: &str) -> GittreeAppThemeResult<()> {
    use leptos::wasm_bindgen::JsCast;

    let Some(window) = web_sys::window() else {
        return Err(GittreeAppThemeError::Unavailable);
    };
    let Some(document) = window.document() else {
        return Err(GittreeAppThemeError::Unavailable);
    };
    let Some(root) = document.document_element() else {
        return Err(GittreeAppThemeError::Unavailable);
    };
    root.set_attribute("data-theme", name)
        .map_err(|_| GittreeAppThemeError::Unavailable)?;
    let html = root
        .dyn_into::<web_sys::HtmlElement>()
        .map_err(|_| GittreeAppThemeError::Unavailable)?;
    html.style()
        .set_property("color-scheme", "light")
        .map_err(|_| GittreeAppThemeError::Unavailable)?;
    Ok(())
}

#[cfg(not(target_arch = "wasm32"))]
fn app_theme_apply_name(_name: &str) -> GittreeAppThemeResult<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{APP_THEME_NAME, app_theme_init};

    #[test]
    fn theme_init_returns_name() {
        assert_eq!(app_theme_init().unwrap(), APP_THEME_NAME);
    }
}

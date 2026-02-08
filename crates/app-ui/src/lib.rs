#![forbid(unsafe_code)]

mod app;
mod auth;
mod auth_client;
mod control_client;
mod control_token;
mod i18n;
mod logging;
mod profile_client;
mod session;
mod theme;
pub mod server;

#[cfg(feature = "hydrate")]
mod entry;

pub use app::GittreeApp;
pub use i18n::{app_i18n_init, translate};
pub use logging::app_logging_init;
pub(crate) use theme::app_theme_init;
#[cfg(feature = "ssr")]
pub use server::AppUiState;

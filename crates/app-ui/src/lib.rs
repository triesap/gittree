#![forbid(unsafe_code)]

mod app;
mod i18n;
mod logging;
pub mod server;

#[cfg(feature = "hydrate")]
mod entry;

pub use app::GittreeApp;
pub use i18n::{app_i18n_init, translate};
pub use logging::app_logging_init;
#[cfg(feature = "ssr")]
pub use server::AppUiState;

use leptos::mount::mount_to_body;
use wasm_bindgen::prelude::wasm_bindgen;

use crate::{app_logging_init, app_theme_init, GittreeApp};

#[wasm_bindgen(start)]
pub fn start() {
    app_logging_init();
    let _ = app_theme_init();
    mount_to_body(GittreeApp);
}

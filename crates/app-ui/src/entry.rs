use leptos::mount::mount_to_body;
use wasm_bindgen::prelude::wasm_bindgen;

use crate::{app_logging_init, GittreeApp};

#[wasm_bindgen(start)]
pub fn start() {
    app_logging_init();
    mount_to_body(GittreeApp);
}

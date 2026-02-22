#[cfg(target_arch = "wasm32")]
use leptos::mount::mount_to_body;
#[cfg(target_arch = "wasm32")]
use wasm_bindgen::prelude::wasm_bindgen;

use crate::{app_logging_init, app_theme_init};
#[cfg(target_arch = "wasm32")]
use crate::GittreeApp;

fn start_with_mount(mount: impl FnOnce()) {
    app_logging_init();
    let _ = app_theme_init();
    mount();
}

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(start)]
pub fn start() {
    start_with_mount(|| mount_to_body(GittreeApp));
}

#[cfg(test)]
mod tests {
    #[test]
    fn start_with_mount_invokes_mount_once() {
        let mut mounts = 0;
        super::start_with_mount(|| mounts += 1);
        assert_eq!(mounts, 1);
    }
}

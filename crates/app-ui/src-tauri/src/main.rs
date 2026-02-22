// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn launch(run: impl FnOnce()) {
    run();
}

fn main() {
    launch(gittree_app_ui_tauri_lib::run);
}

#[cfg(test)]
mod tests {
    #[test]
    fn launch_invokes_runner() {
        let mut called = false;
        super::launch(|| called = true);
        assert!(called);
    }
}

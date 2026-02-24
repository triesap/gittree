#![forbid(unsafe_code)]

fn run_with(
    builder: tauri::Builder<tauri::Wry>,
    run_fn: impl FnOnce(tauri::Builder<tauri::Wry>) -> tauri::Result<()>,
) {
    run_fn(builder).expect("error while running tauri application");
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    run_with(tauri::Builder::default(), |builder| {
        builder.run(tauri::generate_context!())
    });
}

#[cfg(test)]
mod tests {
    #[test]
    fn run_with_invokes_runner() {
        let mut called = false;
        super::run_with(tauri::Builder::default(), |_builder| {
            called = true;
            Ok(())
        });
        assert!(called);
    }
}

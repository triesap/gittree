use gittree_state::init_observability;

fn with_env(key: &str, value: Option<&str>, run: &mut dyn FnMut()) {
    let previous = std::env::var_os(key);
    match value {
        Some(value) => {
            // SAFETY: test-only env mutation scoped to this helper.
            unsafe { std::env::set_var(key, value) };
        }
        None => {
            // SAFETY: test-only env mutation scoped to this helper.
            unsafe { std::env::remove_var(key) };
        }
    }

    run();

    match previous {
        Some(previous) => {
            // SAFETY: restore prior value after scoped mutation.
            unsafe { std::env::set_var(key, previous) };
        }
        None => {
            // SAFETY: restore unset state after scoped mutation.
            unsafe { std::env::remove_var(key) };
        }
    }
}

#[test]
fn init_observability_reports_reinit_error() {
    with_env("GITTREE_LOG_DIR", None, &mut || {
        with_env("GITTREE_LOG_JSON", Some("false"), &mut || {
            with_env("GITTREE_LOG_STDOUT", Some("true"), &mut || {
                let _ = init_observability().expect("first init");
                let err = init_observability().expect_err("second init should fail");
                assert!(err.to_string().contains("state observability error"));
            });
        });
    });
}

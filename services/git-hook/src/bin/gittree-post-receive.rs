use gittree_git_hook::{HookMode, run_hook_from_env};

fn init_observability(service: &str) -> Result<gittree_observability::ObservabilityHandle, String> {
    let config = gittree_observability::ObservabilityConfig::from_env(service)
        .map_err(|err| err.to_string())?;
    gittree_observability::init(&config).map_err(|err| err.to_string())
}

fn main() {
    dotenvy::dotenv().ok();
    if let Err(err) = run_with(
        "gittree-post-receive",
        HookMode::PostReceive,
        init_observability,
        run_hook_from_env,
    ) {
        eprintln!("{err}");
        std::process::exit(1);
    }
}

fn run_with<T, FInit, FHook>(
    service: &str,
    mode: HookMode,
    init_observability_fn: FInit,
    run_hook_fn: FHook,
) -> Result<(), String>
where
    FInit: FnOnce(&str) -> Result<T, String>,
    FHook: FnOnce(HookMode) -> Result<(), gittree_git_hook::HookServiceError>,
{
    let _observability = init_observability_fn(service)
        .map_err(|err| format!("git hook observability failed: {err}"))?;
    run_hook_fn(mode).map_err(|err| format!("git hook failed: {err}"))
}

#[cfg(test)]
mod tests {
    use super::{HookMode, run_with};

    #[test]
    fn run_with_returns_ok_on_success() {
        let result = run_with(
            "svc",
            HookMode::PostReceive,
            |_| Ok::<(), String>(()),
            |_| Ok(()),
        );
        assert!(result.is_ok());
    }

    #[test]
    fn run_with_reports_observability_error() {
        let err = run_with(
            "svc",
            HookMode::PostReceive,
            |_| Err::<(), _>("obs boom".to_string()),
            |_| Ok(()),
        )
        .expect_err("expected error");
        assert!(err.contains("git hook observability failed"));
    }

    #[test]
    fn run_with_reports_hook_error() {
        let err = run_with(
            "svc",
            HookMode::PostReceive,
            |_| Ok::<(), String>(()),
            |_| {
                Err(gittree_git_hook::HookServiceError::Core(
                    "hook boom".to_string(),
                ))
            },
        )
        .expect_err("expected error");
        assert!(err.contains("git hook failed"));
    }
}

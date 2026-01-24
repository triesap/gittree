use clap::{Parser, ValueEnum};
use std::path::PathBuf;

use gittree_git_hook::{HookConfig, HookConfigError, HookMode};

const ENV_HOOK_STDIN_FILE: &str = "GITTREE_HOOK_STDIN_FILE";

#[derive(Debug, Parser)]
#[command(name = "gittree-git-hook", version, about = "Gittree git hook runner")]
pub struct HookCli {
    #[arg(long, value_enum)]
    pub mode: Option<HookModeArg>,
    #[arg(long)]
    pub stdin_file: Option<PathBuf>,
    #[arg(long)]
    pub state_url: Option<String>,
    #[arg(long)]
    pub sync_url: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum HookModeArg {
    PreReceive,
    PostReceive,
}

impl From<HookModeArg> for HookMode {
    fn from(value: HookModeArg) -> Self {
        match value {
            HookModeArg::PreReceive => HookMode::PreReceive,
            HookModeArg::PostReceive => HookMode::PostReceive,
        }
    }
}

#[derive(Debug)]
pub struct HookRunConfig {
    pub hook: HookConfig,
    pub stdin_file: Option<PathBuf>,
}

impl HookRunConfig {
    pub fn from_env(cli: HookCli) -> Result<Self, HookConfigError> {
        let hook = HookConfig::from_env_with_overrides(
            cli.mode.map(Into::into),
            cli.state_url,
            cli.sync_url,
        )?;
        let stdin_file = cli.stdin_file.or_else(|| env_path(ENV_HOOK_STDIN_FILE));
        Ok(Self {
            hook,
            stdin_file,
        })
    }
}

fn env_path(key: &str) -> Option<PathBuf> {
    let value = std::env::var(key).ok()?;
    if value.trim().is_empty() {
        return None;
    }
    Some(PathBuf::from(value))
}

#[cfg(test)]
mod tests {
    use clap::Parser;
    use super::{HookCli, HookModeArg, HookRunConfig};
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn with_env_var<F: FnOnce()>(key: &str, value: &str, f: F) {
        let previous = std::env::var_os(key);
        // SAFETY: tests run single-threaded behind the env lock, and we restore the value after.
        unsafe {
            std::env::set_var(key, value);
        }
        f();
        match previous {
            Some(old) => unsafe {
                std::env::set_var(key, old);
            },
            None => unsafe {
                std::env::remove_var(key);
            },
        }
    }

    #[test]
    fn cli_parses_mode_and_stdin_file() {
        let cli = HookCli::try_parse_from([
            "gittree-git-hook",
            "--mode",
            "post-receive",
            "--stdin-file",
            "updates.txt",
        ])
        .expect("parse cli");
        assert_eq!(cli.mode, Some(HookModeArg::PostReceive));
        assert_eq!(cli.stdin_file.as_deref(), Some(std::path::Path::new("updates.txt")));
    }

    #[test]
    fn run_config_uses_env_state_url() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_env_var("GITTREE_STATE_URL", "http://127.0.0.1:8082", || {
            let cli = HookCli::try_parse_from(["gittree-git-hook"]).expect("parse cli");
            let config = HookRunConfig::from_env(cli).expect("config");
            assert_eq!(config.hook.state_url, "http://127.0.0.1:8082");
        });
    }

    #[test]
    fn run_config_reads_stdin_file_from_env() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        with_env_var("GITTREE_STATE_URL", "http://127.0.0.1:8082", || {
            with_env_var("GITTREE_HOOK_STDIN_FILE", "fixtures.txt", || {
                let cli = HookCli::try_parse_from(["gittree-git-hook"]).expect("parse cli");
                let config = HookRunConfig::from_env(cli).expect("config");
                assert_eq!(
                    config.stdin_file.as_deref(),
                    Some(std::path::Path::new("fixtures.txt"))
                );
            });
        });
    }
}

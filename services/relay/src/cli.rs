use std::ffi::OsString;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelayCli {
    pub config_path: Option<PathBuf>,
    pub bind: Option<String>,
    pub help: bool,
}

impl RelayCli {
    pub fn parse<I, T>(args: I) -> Result<Self, RelayCliError>
    where
        I: IntoIterator<Item = T>,
        T: Into<OsString>,
    {
        let mut config_path = None;
        let mut bind = None;
        let mut help = false;
        let mut iter = args.into_iter();
        let _ = iter.next();

        while let Some(arg) = iter.next() {
            let arg: OsString = arg.into();
            let value = arg.to_string_lossy();

            match value.as_ref() {
                "-h" | "--help" => {
                    help = true;
                }
                "-c" | "--config" => {
                    let next = iter.next().ok_or(RelayCliError::MissingValue("--config"))?;
                    config_path = Some(PathBuf::from(next.into()));
                }
                "-b" | "--bind" => {
                    let next = iter.next().ok_or(RelayCliError::MissingValue("--bind"))?;
                    bind = Some(next.into().to_string_lossy().to_string());
                }
                _ if value.starts_with("--config=") => {
                    let path = value.trim_start_matches("--config=");
                    if path.is_empty() {
                        return Err(RelayCliError::MissingValue("--config"));
                    }
                    config_path = Some(PathBuf::from(path));
                }
                _ if value.starts_with("--bind=") => {
                    let addr = value.trim_start_matches("--bind=");
                    if addr.is_empty() {
                        return Err(RelayCliError::MissingValue("--bind"));
                    }
                    bind = Some(addr.to_string());
                }
                _ => return Err(RelayCliError::UnknownFlag(value.to_string())),
            }
        }

        Ok(Self {
            config_path,
            bind,
            help,
        })
    }

    pub fn help_text() -> &'static str {
        "gittree-relay [--config <path>] [--bind <addr>]\n\nFlags:\n  -c, --config <path>  Path to services config toml\n  -b, --bind <addr>    Override relay bind address\n  -h, --help           Show this help message\n"
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RelayCliError {
    UnknownFlag(String),
    MissingValue(&'static str),
}

impl std::fmt::Display for RelayCliError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RelayCliError::UnknownFlag(flag) => write!(f, "unknown flag {flag}"),
            RelayCliError::MissingValue(flag) => write!(f, "missing value for {flag}"),
        }
    }
}

impl std::error::Error for RelayCliError {}

#[cfg(test)]
mod tests {
    use super::RelayCli;
    use super::RelayCliError;
    use std::path::PathBuf;

    #[test]
    fn parse_accepts_config_and_bind() {
        let args = [
            "gittree-relay",
            "--config",
            "config.toml",
            "--bind",
            "127.0.0.1:9000",
        ];
        let cli = RelayCli::parse(args).expect("parse cli");
        assert_eq!(cli.config_path, Some(PathBuf::from("config.toml")));
        assert_eq!(cli.bind, Some("127.0.0.1:9000".to_string()));
        assert!(!cli.help);
    }

    #[test]
    fn parse_accepts_equals_form() {
        let args = ["gittree-relay", "--config=cfg.toml", "--bind=0.0.0.0:8080"];
        let cli = RelayCli::parse(args).expect("parse cli");
        assert_eq!(cli.config_path, Some(PathBuf::from("cfg.toml")));
        assert_eq!(cli.bind, Some("0.0.0.0:8080".to_string()));
    }

    #[test]
    fn parse_sets_help_flag() {
        let cli = RelayCli::parse(["gittree-relay", "--help"]).expect("parse cli");
        assert!(cli.help);
    }

    #[test]
    fn parse_rejects_unknown_flag() {
        let err = RelayCli::parse(["gittree-relay", "--nope"]).unwrap_err();
        assert!(matches!(err, RelayCliError::UnknownFlag(_)));
    }

    #[test]
    fn parse_rejects_missing_value() {
        let err = RelayCli::parse(["gittree-relay", "--bind"]).unwrap_err();
        assert!(matches!(err, RelayCliError::MissingValue(_)));
    }
}

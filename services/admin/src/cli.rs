use std::ffi::OsString;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdminCli {
    pub command: Option<AdminCommand>,
    pub help: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdminCommand {
    Map {
        forgejo: String,
        pubkey: String,
        identifier: String,
    },
}

impl AdminCli {
    pub fn parse<I, T>(args: I) -> Result<Self, AdminCliError>
    where
        I: IntoIterator<Item = T>,
        T: Into<OsString>,
    {
        let mut help = false;
        let mut command = None;
        let mut iter = args.into_iter();
        let _ = iter.next();

        while let Some(arg) = iter.next() {
            let arg: OsString = arg.into();
            let value = arg.to_string_lossy();
            match value.as_ref() {
                "-h" | "--help" => {
                    help = true;
                }
                "map" => {
                    command = Some(parse_map(&mut iter)?);
                }
                _ => return Err(AdminCliError::UnknownCommand(value.to_string())),
            }
        }

        Ok(Self { command, help })
    }

    pub fn help_text() -> &'static str {
        "gittree-admin <command> [options]\n\nCommands:\n  map --forgejo <owner/repo> --pubkey <hex> --identifier <id>\n\nFlags:\n  -h, --help  Show this help message\n"
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdminCliError {
    UnknownCommand(String),
    UnknownFlag(String),
    MissingValue(&'static str),
    MissingCommand,
}

impl std::fmt::Display for AdminCliError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AdminCliError::UnknownCommand(value) => write!(f, "unknown command {value}"),
            AdminCliError::UnknownFlag(value) => write!(f, "unknown flag {value}"),
            AdminCliError::MissingValue(flag) => write!(f, "missing value for {flag}"),
            AdminCliError::MissingCommand => write!(f, "missing command"),
        }
    }
}

impl std::error::Error for AdminCliError {}

fn parse_map<I, T>(iter: &mut I) -> Result<AdminCommand, AdminCliError>
where
    I: Iterator<Item = T>,
    T: Into<OsString>,
{
    let mut forgejo = None;
    let mut pubkey = None;
    let mut identifier = None;

    while let Some(arg) = iter.next() {
        let arg: OsString = arg.into();
        let value = arg.to_string_lossy();
        match value.as_ref() {
            "--forgejo" => {
                let next = iter.next().ok_or(AdminCliError::MissingValue("--forgejo"))?;
                forgejo = Some(next.into().to_string_lossy().to_string());
            }
            "--pubkey" => {
                let next = iter.next().ok_or(AdminCliError::MissingValue("--pubkey"))?;
                pubkey = Some(next.into().to_string_lossy().to_string());
            }
            "--identifier" => {
                let next = iter.next().ok_or(AdminCliError::MissingValue("--identifier"))?;
                identifier = Some(next.into().to_string_lossy().to_string());
            }
            _ if value.starts_with("--forgejo=") => {
                let value = value.trim_start_matches("--forgejo=");
                if value.is_empty() {
                    return Err(AdminCliError::MissingValue("--forgejo"));
                }
                forgejo = Some(value.to_string());
            }
            _ if value.starts_with("--pubkey=") => {
                let value = value.trim_start_matches("--pubkey=");
                if value.is_empty() {
                    return Err(AdminCliError::MissingValue("--pubkey"));
                }
                pubkey = Some(value.to_string());
            }
            _ if value.starts_with("--identifier=") => {
                let value = value.trim_start_matches("--identifier=");
                if value.is_empty() {
                    return Err(AdminCliError::MissingValue("--identifier"));
                }
                identifier = Some(value.to_string());
            }
            _ => return Err(AdminCliError::UnknownFlag(value.to_string())),
        }
    }

    let forgejo = forgejo.ok_or(AdminCliError::MissingValue("--forgejo"))?;
    let pubkey = pubkey.ok_or(AdminCliError::MissingValue("--pubkey"))?;
    let identifier = identifier.ok_or(AdminCliError::MissingValue("--identifier"))?;

    Ok(AdminCommand::Map {
        forgejo,
        pubkey,
        identifier,
    })
}

#[cfg(test)]
mod tests {
    use super::{AdminCli, AdminCliError, AdminCommand};

    #[test]
    fn parse_accepts_map_command() {
        let pubkey = "11".repeat(32);
        let args = vec![
            "gittree-admin".to_string(),
            "map".to_string(),
            "--forgejo".to_string(),
            "owner/repo".to_string(),
            "--pubkey".to_string(),
            pubkey,
            "--identifier".to_string(),
            "repo".to_string(),
        ];
        let cli = AdminCli::parse(args).expect("parse");
        assert!(matches!(cli.command, Some(AdminCommand::Map { .. })));
    }

    #[test]
    fn parse_rejects_unknown_command() {
        let err = AdminCli::parse(["gittree-admin", "nope"]).unwrap_err();
        assert!(matches!(err, AdminCliError::UnknownCommand(_)));
    }

    #[test]
    fn parse_rejects_missing_values() {
        let err = AdminCli::parse(["gittree-admin", "map", "--forgejo"]).unwrap_err();
        assert!(matches!(err, AdminCliError::MissingValue(_)));
    }
}

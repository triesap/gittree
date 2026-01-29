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
    CreateUser {
        username: String,
        email: String,
        password: String,
        full_name: Option<String>,
        must_change_password: Option<bool>,
        send_notify: Option<bool>,
    },
    CreateOrg {
        owner: String,
        name: String,
        full_name: Option<String>,
        description: Option<String>,
        visibility: Option<String>,
    },
    CreateRepo {
        owner: String,
        name: String,
        description: Option<String>,
        private: Option<bool>,
        auto_init: Option<bool>,
    },
    CreatePull {
        owner: String,
        repo: String,
        head: String,
        base: String,
        title: String,
        body: Option<String>,
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
                "create-user" => {
                    command = Some(parse_create_user(&mut iter)?);
                }
                "create-org" => {
                    command = Some(parse_create_org(&mut iter)?);
                }
                "create-repo" => {
                    command = Some(parse_create_repo(&mut iter)?);
                }
                "create-pull" => {
                    command = Some(parse_create_pull(&mut iter)?);
                }
                _ => return Err(AdminCliError::UnknownCommand(value.to_string())),
            }
        }

        Ok(Self { command, help })
    }

    pub fn help_text() -> &'static str {
        "gittree-admin <command> [options]\n\nCommands:\n  map --forgejo <owner/repo> --pubkey <hex> --identifier <id>\n  create-user --username <name> --email <email> --password <password> [--full-name <name>] [--must-change-password] [--send-notify]\n  create-org --owner <user> --name <org> [--full-name <name>] [--description <text>] [--visibility <vis>]\n  create-repo --owner <user> --name <repo> [--description <text>] [--private] [--auto-init]\n  create-pull --owner <user> --repo <repo> --head <ref> --base <ref> --title <title> [--body <text>]\n\nFlags:\n  -h, --help  Show this help message\n"
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

fn parse_create_user<I, T>(iter: &mut I) -> Result<AdminCommand, AdminCliError>
where
    I: Iterator<Item = T>,
    T: Into<OsString>,
{
    let mut username = None;
    let mut email = None;
    let mut password = None;
    let mut full_name = None;
    let mut must_change_password = None;
    let mut send_notify = None;

    while let Some(arg) = iter.next() {
        let arg: OsString = arg.into();
        let value = arg.to_string_lossy();
        match value.as_ref() {
            "--username" => {
                let next = iter.next().ok_or(AdminCliError::MissingValue("--username"))?;
                username = Some(next.into().to_string_lossy().to_string());
            }
            "--email" => {
                let next = iter.next().ok_or(AdminCliError::MissingValue("--email"))?;
                email = Some(next.into().to_string_lossy().to_string());
            }
            "--password" => {
                let next = iter.next().ok_or(AdminCliError::MissingValue("--password"))?;
                password = Some(next.into().to_string_lossy().to_string());
            }
            "--full-name" => {
                let next = iter.next().ok_or(AdminCliError::MissingValue("--full-name"))?;
                full_name = Some(next.into().to_string_lossy().to_string());
            }
            "--must-change-password" => {
                must_change_password = Some(true);
            }
            "--send-notify" => {
                send_notify = Some(true);
            }
            _ if value.starts_with("--username=") => {
                let value = value.trim_start_matches("--username=");
                if value.is_empty() {
                    return Err(AdminCliError::MissingValue("--username"));
                }
                username = Some(value.to_string());
            }
            _ if value.starts_with("--email=") => {
                let value = value.trim_start_matches("--email=");
                if value.is_empty() {
                    return Err(AdminCliError::MissingValue("--email"));
                }
                email = Some(value.to_string());
            }
            _ if value.starts_with("--password=") => {
                let value = value.trim_start_matches("--password=");
                if value.is_empty() {
                    return Err(AdminCliError::MissingValue("--password"));
                }
                password = Some(value.to_string());
            }
            _ if value.starts_with("--full-name=") => {
                let value = value.trim_start_matches("--full-name=");
                if value.is_empty() {
                    return Err(AdminCliError::MissingValue("--full-name"));
                }
                full_name = Some(value.to_string());
            }
            _ => return Err(AdminCliError::UnknownFlag(value.to_string())),
        }
    }

    let username = username.ok_or(AdminCliError::MissingValue("--username"))?;
    let email = email.ok_or(AdminCliError::MissingValue("--email"))?;
    let password = password.ok_or(AdminCliError::MissingValue("--password"))?;

    Ok(AdminCommand::CreateUser {
        username,
        email,
        password,
        full_name,
        must_change_password,
        send_notify,
    })
}

fn parse_create_org<I, T>(iter: &mut I) -> Result<AdminCommand, AdminCliError>
where
    I: Iterator<Item = T>,
    T: Into<OsString>,
{
    let mut owner = None;
    let mut name = None;
    let mut full_name = None;
    let mut description = None;
    let mut visibility = None;

    while let Some(arg) = iter.next() {
        let arg: OsString = arg.into();
        let value = arg.to_string_lossy();
        match value.as_ref() {
            "--owner" => {
                let next = iter.next().ok_or(AdminCliError::MissingValue("--owner"))?;
                owner = Some(next.into().to_string_lossy().to_string());
            }
            "--name" => {
                let next = iter.next().ok_or(AdminCliError::MissingValue("--name"))?;
                name = Some(next.into().to_string_lossy().to_string());
            }
            "--full-name" => {
                let next = iter.next().ok_or(AdminCliError::MissingValue("--full-name"))?;
                full_name = Some(next.into().to_string_lossy().to_string());
            }
            "--description" => {
                let next = iter.next().ok_or(AdminCliError::MissingValue("--description"))?;
                description = Some(next.into().to_string_lossy().to_string());
            }
            "--visibility" => {
                let next = iter.next().ok_or(AdminCliError::MissingValue("--visibility"))?;
                visibility = Some(next.into().to_string_lossy().to_string());
            }
            _ if value.starts_with("--owner=") => {
                let value = value.trim_start_matches("--owner=");
                if value.is_empty() {
                    return Err(AdminCliError::MissingValue("--owner"));
                }
                owner = Some(value.to_string());
            }
            _ if value.starts_with("--name=") => {
                let value = value.trim_start_matches("--name=");
                if value.is_empty() {
                    return Err(AdminCliError::MissingValue("--name"));
                }
                name = Some(value.to_string());
            }
            _ if value.starts_with("--full-name=") => {
                let value = value.trim_start_matches("--full-name=");
                if value.is_empty() {
                    return Err(AdminCliError::MissingValue("--full-name"));
                }
                full_name = Some(value.to_string());
            }
            _ if value.starts_with("--description=") => {
                let value = value.trim_start_matches("--description=");
                if value.is_empty() {
                    return Err(AdminCliError::MissingValue("--description"));
                }
                description = Some(value.to_string());
            }
            _ if value.starts_with("--visibility=") => {
                let value = value.trim_start_matches("--visibility=");
                if value.is_empty() {
                    return Err(AdminCliError::MissingValue("--visibility"));
                }
                visibility = Some(value.to_string());
            }
            _ => return Err(AdminCliError::UnknownFlag(value.to_string())),
        }
    }

    let owner = owner.ok_or(AdminCliError::MissingValue("--owner"))?;
    let name = name.ok_or(AdminCliError::MissingValue("--name"))?;

    Ok(AdminCommand::CreateOrg {
        owner,
        name,
        full_name,
        description,
        visibility,
    })
}

fn parse_create_repo<I, T>(iter: &mut I) -> Result<AdminCommand, AdminCliError>
where
    I: Iterator<Item = T>,
    T: Into<OsString>,
{
    let mut owner = None;
    let mut name = None;
    let mut description = None;
    let mut private = None;
    let mut auto_init = None;

    while let Some(arg) = iter.next() {
        let arg: OsString = arg.into();
        let value = arg.to_string_lossy();
        match value.as_ref() {
            "--owner" => {
                let next = iter.next().ok_or(AdminCliError::MissingValue("--owner"))?;
                owner = Some(next.into().to_string_lossy().to_string());
            }
            "--name" => {
                let next = iter.next().ok_or(AdminCliError::MissingValue("--name"))?;
                name = Some(next.into().to_string_lossy().to_string());
            }
            "--description" => {
                let next = iter.next().ok_or(AdminCliError::MissingValue("--description"))?;
                description = Some(next.into().to_string_lossy().to_string());
            }
            "--private" => {
                private = Some(true);
            }
            "--auto-init" => {
                auto_init = Some(true);
            }
            _ if value.starts_with("--owner=") => {
                let value = value.trim_start_matches("--owner=");
                if value.is_empty() {
                    return Err(AdminCliError::MissingValue("--owner"));
                }
                owner = Some(value.to_string());
            }
            _ if value.starts_with("--name=") => {
                let value = value.trim_start_matches("--name=");
                if value.is_empty() {
                    return Err(AdminCliError::MissingValue("--name"));
                }
                name = Some(value.to_string());
            }
            _ if value.starts_with("--description=") => {
                let value = value.trim_start_matches("--description=");
                if value.is_empty() {
                    return Err(AdminCliError::MissingValue("--description"));
                }
                description = Some(value.to_string());
            }
            _ => return Err(AdminCliError::UnknownFlag(value.to_string())),
        }
    }

    let owner = owner.ok_or(AdminCliError::MissingValue("--owner"))?;
    let name = name.ok_or(AdminCliError::MissingValue("--name"))?;

    Ok(AdminCommand::CreateRepo {
        owner,
        name,
        description,
        private,
        auto_init,
    })
}

fn parse_create_pull<I, T>(iter: &mut I) -> Result<AdminCommand, AdminCliError>
where
    I: Iterator<Item = T>,
    T: Into<OsString>,
{
    let mut owner = None;
    let mut repo = None;
    let mut head = None;
    let mut base = None;
    let mut title = None;
    let mut body = None;

    while let Some(arg) = iter.next() {
        let arg: OsString = arg.into();
        let value = arg.to_string_lossy();
        match value.as_ref() {
            "--owner" => {
                let next = iter.next().ok_or(AdminCliError::MissingValue("--owner"))?;
                owner = Some(next.into().to_string_lossy().to_string());
            }
            "--repo" => {
                let next = iter.next().ok_or(AdminCliError::MissingValue("--repo"))?;
                repo = Some(next.into().to_string_lossy().to_string());
            }
            "--head" => {
                let next = iter.next().ok_or(AdminCliError::MissingValue("--head"))?;
                head = Some(next.into().to_string_lossy().to_string());
            }
            "--base" => {
                let next = iter.next().ok_or(AdminCliError::MissingValue("--base"))?;
                base = Some(next.into().to_string_lossy().to_string());
            }
            "--title" => {
                let next = iter.next().ok_or(AdminCliError::MissingValue("--title"))?;
                title = Some(next.into().to_string_lossy().to_string());
            }
            "--body" => {
                let next = iter.next().ok_or(AdminCliError::MissingValue("--body"))?;
                body = Some(next.into().to_string_lossy().to_string());
            }
            _ if value.starts_with("--owner=") => {
                let value = value.trim_start_matches("--owner=");
                if value.is_empty() {
                    return Err(AdminCliError::MissingValue("--owner"));
                }
                owner = Some(value.to_string());
            }
            _ if value.starts_with("--repo=") => {
                let value = value.trim_start_matches("--repo=");
                if value.is_empty() {
                    return Err(AdminCliError::MissingValue("--repo"));
                }
                repo = Some(value.to_string());
            }
            _ if value.starts_with("--head=") => {
                let value = value.trim_start_matches("--head=");
                if value.is_empty() {
                    return Err(AdminCliError::MissingValue("--head"));
                }
                head = Some(value.to_string());
            }
            _ if value.starts_with("--base=") => {
                let value = value.trim_start_matches("--base=");
                if value.is_empty() {
                    return Err(AdminCliError::MissingValue("--base"));
                }
                base = Some(value.to_string());
            }
            _ if value.starts_with("--title=") => {
                let value = value.trim_start_matches("--title=");
                if value.is_empty() {
                    return Err(AdminCliError::MissingValue("--title"));
                }
                title = Some(value.to_string());
            }
            _ if value.starts_with("--body=") => {
                let value = value.trim_start_matches("--body=");
                if value.is_empty() {
                    return Err(AdminCliError::MissingValue("--body"));
                }
                body = Some(value.to_string());
            }
            _ => return Err(AdminCliError::UnknownFlag(value.to_string())),
        }
    }

    let owner = owner.ok_or(AdminCliError::MissingValue("--owner"))?;
    let repo = repo.ok_or(AdminCliError::MissingValue("--repo"))?;
    let head = head.ok_or(AdminCliError::MissingValue("--head"))?;
    let base = base.ok_or(AdminCliError::MissingValue("--base"))?;
    let title = title.ok_or(AdminCliError::MissingValue("--title"))?;

    Ok(AdminCommand::CreatePull {
        owner,
        repo,
        head,
        base,
        title,
        body,
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

    #[test]
    fn parse_accepts_create_user_command() {
        let args = vec![
            "gittree-admin".to_string(),
            "create-user".to_string(),
            "--username".to_string(),
            "alice".to_string(),
            "--email".to_string(),
            "alice@example.com".to_string(),
            "--password".to_string(),
            "secret".to_string(),
        ];
        let cli = AdminCli::parse(args).expect("parse");
        assert!(matches!(cli.command, Some(AdminCommand::CreateUser { .. })));
    }

    #[test]
    fn parse_accepts_create_org_command() {
        let args = vec![
            "gittree-admin".to_string(),
            "create-org".to_string(),
            "--owner".to_string(),
            "root".to_string(),
            "--name".to_string(),
            "acme".to_string(),
        ];
        let cli = AdminCli::parse(args).expect("parse");
        assert!(matches!(cli.command, Some(AdminCommand::CreateOrg { .. })));
    }

    #[test]
    fn parse_accepts_create_repo_command() {
        let args = vec![
            "gittree-admin".to_string(),
            "create-repo".to_string(),
            "--owner".to_string(),
            "alice".to_string(),
            "--name".to_string(),
            "demo".to_string(),
            "--auto-init".to_string(),
        ];
        let cli = AdminCli::parse(args).expect("parse");
        assert!(matches!(cli.command, Some(AdminCommand::CreateRepo { .. })));
    }

    #[test]
    fn parse_accepts_create_pull_command() {
        let args = vec![
            "gittree-admin".to_string(),
            "create-pull".to_string(),
            "--owner".to_string(),
            "alice".to_string(),
            "--repo".to_string(),
            "demo".to_string(),
            "--head".to_string(),
            "feature".to_string(),
            "--base".to_string(),
            "main".to_string(),
            "--title".to_string(),
            "Update".to_string(),
        ];
        let cli = AdminCli::parse(args).expect("parse");
        assert!(matches!(cli.command, Some(AdminCommand::CreatePull { .. })));
    }
}

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
        let args = args.into_iter().map(Into::into).collect();
        Self::parse_from_os(args)
    }

    fn parse_from_os(args: Vec<OsString>) -> Result<Self, AdminCliError> {
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

fn parse_map(iter: &mut std::vec::IntoIter<OsString>) -> Result<AdminCommand, AdminCliError> {
    let mut forgejo = None;
    let mut pubkey = None;
    let mut identifier = None;

    while let Some(arg) = iter.next() {
        let value = arg.to_string_lossy();
        match value.as_ref() {
            "--forgejo" => {
                let next = iter
                    .next()
                    .ok_or(AdminCliError::MissingValue("--forgejo"))?;
                forgejo = Some(next.to_string_lossy().to_string());
            }
            "--pubkey" => {
                let next = iter.next().ok_or(AdminCliError::MissingValue("--pubkey"))?;
                pubkey = Some(next.to_string_lossy().to_string());
            }
            "--identifier" => {
                let next = iter
                    .next()
                    .ok_or(AdminCliError::MissingValue("--identifier"))?;
                identifier = Some(next.to_string_lossy().to_string());
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

fn parse_create_user(
    iter: &mut std::vec::IntoIter<OsString>,
) -> Result<AdminCommand, AdminCliError> {
    let mut username = None;
    let mut email = None;
    let mut password = None;
    let mut full_name = None;
    let mut must_change_password = None;
    let mut send_notify = None;

    while let Some(arg) = iter.next() {
        let value = arg.to_string_lossy();
        match value.as_ref() {
            "--username" => {
                let next = iter
                    .next()
                    .ok_or(AdminCliError::MissingValue("--username"))?;
                username = Some(next.to_string_lossy().to_string());
            }
            "--email" => {
                let next = iter.next().ok_or(AdminCliError::MissingValue("--email"))?;
                email = Some(next.to_string_lossy().to_string());
            }
            "--password" => {
                let next = iter
                    .next()
                    .ok_or(AdminCliError::MissingValue("--password"))?;
                password = Some(next.to_string_lossy().to_string());
            }
            "--full-name" => {
                let next = iter
                    .next()
                    .ok_or(AdminCliError::MissingValue("--full-name"))?;
                full_name = Some(next.to_string_lossy().to_string());
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

fn parse_create_org(
    iter: &mut std::vec::IntoIter<OsString>,
) -> Result<AdminCommand, AdminCliError> {
    let mut owner = None;
    let mut name = None;
    let mut full_name = None;
    let mut description = None;
    let mut visibility = None;

    while let Some(arg) = iter.next() {
        let value = arg.to_string_lossy();
        match value.as_ref() {
            "--owner" => {
                let next = iter.next().ok_or(AdminCliError::MissingValue("--owner"))?;
                owner = Some(next.to_string_lossy().to_string());
            }
            "--name" => {
                let next = iter.next().ok_or(AdminCliError::MissingValue("--name"))?;
                name = Some(next.to_string_lossy().to_string());
            }
            "--full-name" => {
                let next = iter
                    .next()
                    .ok_or(AdminCliError::MissingValue("--full-name"))?;
                full_name = Some(next.to_string_lossy().to_string());
            }
            "--description" => {
                let next = iter
                    .next()
                    .ok_or(AdminCliError::MissingValue("--description"))?;
                description = Some(next.to_string_lossy().to_string());
            }
            "--visibility" => {
                let next = iter
                    .next()
                    .ok_or(AdminCliError::MissingValue("--visibility"))?;
                visibility = Some(next.to_string_lossy().to_string());
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

fn parse_create_repo(
    iter: &mut std::vec::IntoIter<OsString>,
) -> Result<AdminCommand, AdminCliError> {
    let mut owner = None;
    let mut name = None;
    let mut description = None;
    let mut private = None;
    let mut auto_init = None;

    while let Some(arg) = iter.next() {
        let value = arg.to_string_lossy();
        match value.as_ref() {
            "--owner" => {
                let next = iter.next().ok_or(AdminCliError::MissingValue("--owner"))?;
                owner = Some(next.to_string_lossy().to_string());
            }
            "--name" => {
                let next = iter.next().ok_or(AdminCliError::MissingValue("--name"))?;
                name = Some(next.to_string_lossy().to_string());
            }
            "--description" => {
                let next = iter
                    .next()
                    .ok_or(AdminCliError::MissingValue("--description"))?;
                description = Some(next.to_string_lossy().to_string());
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

fn parse_create_pull(
    iter: &mut std::vec::IntoIter<OsString>,
) -> Result<AdminCommand, AdminCliError> {
    let mut owner = None;
    let mut repo = None;
    let mut head = None;
    let mut base = None;
    let mut title = None;
    let mut body = None;

    while let Some(arg) = iter.next() {
        let value = arg.to_string_lossy();
        match value.as_ref() {
            "--owner" => {
                let next = iter.next().ok_or(AdminCliError::MissingValue("--owner"))?;
                owner = Some(next.to_string_lossy().to_string());
            }
            "--repo" => {
                let next = iter.next().ok_or(AdminCliError::MissingValue("--repo"))?;
                repo = Some(next.to_string_lossy().to_string());
            }
            "--head" => {
                let next = iter.next().ok_or(AdminCliError::MissingValue("--head"))?;
                head = Some(next.to_string_lossy().to_string());
            }
            "--base" => {
                let next = iter.next().ok_or(AdminCliError::MissingValue("--base"))?;
                base = Some(next.to_string_lossy().to_string());
            }
            "--title" => {
                let next = iter.next().ok_or(AdminCliError::MissingValue("--title"))?;
                title = Some(next.to_string_lossy().to_string());
            }
            "--body" => {
                let next = iter.next().ok_or(AdminCliError::MissingValue("--body"))?;
                body = Some(next.to_string_lossy().to_string());
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
    use std::error::Error;

    fn string_args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_string()).collect()
    }

    #[test]
    fn parse_help_and_help_text_are_available() {
        let cli = AdminCli::parse(["gittree-admin", "--help"]).expect("parse");
        assert!(cli.help);
        assert!(cli.command.is_none());
        assert!(AdminCli::help_text().contains("create-user"));
        assert!(AdminCli::help_text().contains("--help"));
    }

    #[test]
    fn parse_rejects_unknown_command_and_missing_split_value() {
        let unknown = AdminCli::parse(["gittree-admin", "nope"]).expect_err("unknown command");
        assert!(matches!(unknown, AdminCliError::UnknownCommand(_)));

        let missing = AdminCli::parse(["gittree-admin", "map", "--forgejo"])
            .expect_err("missing split value");
        assert!(matches!(missing, AdminCliError::MissingValue("--forgejo")));
    }

    #[test]
    fn parse_map_accepts_equals_forms_and_rejects_unknown_flag() {
        let pubkey = "22".repeat(32);
        let cli = AdminCli::parse([
            "gittree-admin",
            "map",
            "--forgejo=owner/repo",
            &format!("--pubkey={pubkey}"),
            "--identifier=repo",
        ])
        .expect("parse");
        assert_eq!(
            cli.command,
            Some(AdminCommand::Map {
                forgejo: "owner/repo".to_string(),
                pubkey: "22".repeat(32),
                identifier: "repo".to_string(),
            })
        );

        let err = AdminCli::parse(["gittree-admin", "map", "--nope"]).expect_err("unknown flag");
        assert!(matches!(err, AdminCliError::UnknownFlag(value) if value == "--nope"));

        let empty = AdminCli::parse([
            "gittree-admin",
            "map",
            "--forgejo=",
            "--pubkey=aa",
            "--identifier=repo",
        ])
        .expect_err("empty forgejo");
        assert!(matches!(empty, AdminCliError::MissingValue("--forgejo")));
    }

    #[test]
    fn parse_map_rejects_missing_equals_values_for_all_required_fields() {
        let empty_pubkey = AdminCli::parse([
            "gittree-admin",
            "map",
            "--forgejo=owner/repo",
            "--pubkey=",
            "--identifier=repo",
        ])
        .expect_err("empty pubkey");
        assert!(matches!(
            empty_pubkey,
            AdminCliError::MissingValue("--pubkey")
        ));

        let empty_identifier = AdminCli::parse([
            "gittree-admin",
            "map",
            "--forgejo=owner/repo",
            "--pubkey=aa",
            "--identifier=",
        ])
        .expect_err("empty identifier");
        assert!(matches!(
            empty_identifier,
            AdminCliError::MissingValue("--identifier")
        ));

        let split_missing_cases = [
            (
                vec![
                    "gittree-admin",
                    "map",
                    "--forgejo",
                    "owner/repo",
                    "--pubkey",
                ],
                "--pubkey",
            ),
            (
                vec![
                    "gittree-admin",
                    "map",
                    "--forgejo",
                    "owner/repo",
                    "--pubkey",
                    "aa",
                    "--identifier",
                ],
                "--identifier",
            ),
        ];
        for (args, flag) in split_missing_cases {
            let err = AdminCli::parse(args).expect_err("missing split flag value");
            assert!(matches!(err, AdminCliError::MissingValue(missing) if missing == flag));
        }
    }

    #[test]
    fn parse_create_user_accepts_optional_fields_and_bool_flags() {
        let cli = AdminCli::parse([
            "gittree-admin",
            "create-user",
            "--username=alice",
            "--email=alice@example.com",
            "--password=secret",
            "--full-name=Alice",
            "--must-change-password",
            "--send-notify",
        ])
        .expect("parse");
        assert_eq!(
            cli.command,
            Some(AdminCommand::CreateUser {
                username: "alice".to_string(),
                email: "alice@example.com".to_string(),
                password: "secret".to_string(),
                full_name: Some("Alice".to_string()),
                must_change_password: Some(true),
                send_notify: Some(true),
            })
        );

        let missing = AdminCli::parse([
            "gittree-admin",
            "create-user",
            "--username=alice",
            "--email=",
        ])
        .expect_err("missing email");
        assert!(matches!(missing, AdminCliError::MissingValue("--email")));
    }

    #[test]
    fn parse_create_user_rejects_missing_optional_and_unknown_flags() {
        let split_optional = AdminCli::parse([
            "gittree-admin",
            "create-user",
            "--username",
            "alice",
            "--email",
            "alice@example.com",
            "--password",
            "secret",
            "--full-name",
            "Alice",
        ])
        .expect("split optional args");
        assert_eq!(
            split_optional.command,
            Some(AdminCommand::CreateUser {
                username: "alice".to_string(),
                email: "alice@example.com".to_string(),
                password: "secret".to_string(),
                full_name: Some("Alice".to_string()),
                must_change_password: None,
                send_notify: None,
            })
        );

        let missing_full_name = AdminCli::parse([
            "gittree-admin",
            "create-user",
            "--username=alice",
            "--email=alice@example.com",
            "--password=secret",
            "--full-name",
        ])
        .expect_err("missing full-name value");
        assert!(matches!(
            missing_full_name,
            AdminCliError::MissingValue("--full-name")
        ));

        let empty_username = AdminCli::parse([
            "gittree-admin",
            "create-user",
            "--username=",
            "--email=alice@example.com",
            "--password=secret",
        ])
        .expect_err("empty username");
        assert!(matches!(
            empty_username,
            AdminCliError::MissingValue("--username")
        ));

        let empty_password = AdminCli::parse([
            "gittree-admin",
            "create-user",
            "--username=alice",
            "--email=alice@example.com",
            "--password=",
        ])
        .expect_err("empty password");
        assert!(matches!(
            empty_password,
            AdminCliError::MissingValue("--password")
        ));

        let empty_full_name = AdminCli::parse([
            "gittree-admin",
            "create-user",
            "--username=alice",
            "--email=alice@example.com",
            "--password=secret",
            "--full-name=",
        ])
        .expect_err("empty full-name");
        assert!(matches!(
            empty_full_name,
            AdminCliError::MissingValue("--full-name")
        ));

        let unknown = AdminCli::parse([
            "gittree-admin",
            "create-user",
            "--username=alice",
            "--email=alice@example.com",
            "--password=secret",
            "--nope",
        ])
        .expect_err("unknown flag");
        assert!(matches!(unknown, AdminCliError::UnknownFlag(flag) if flag == "--nope"));

        let split_missing_cases = [
            (
                vec!["gittree-admin", "create-user", "--username"],
                "--username",
            ),
            (
                vec![
                    "gittree-admin",
                    "create-user",
                    "--username=alice",
                    "--email",
                ],
                "--email",
            ),
            (
                vec![
                    "gittree-admin",
                    "create-user",
                    "--username=alice",
                    "--email=alice@example.com",
                    "--password",
                ],
                "--password",
            ),
        ];
        for (args, flag) in split_missing_cases {
            let err = AdminCli::parse(args).expect_err("missing split flag value");
            assert!(matches!(err, AdminCliError::MissingValue(missing) if missing == flag));
        }
    }

    #[test]
    fn parse_create_org_accepts_equals_optional_and_missing_owner() {
        let cli = AdminCli::parse([
            "gittree-admin",
            "create-org",
            "--owner=root",
            "--name=acme",
            "--full-name=Acme Org",
            "--description=desc",
            "--visibility=private",
        ])
        .expect("parse");
        assert_eq!(
            cli.command,
            Some(AdminCommand::CreateOrg {
                owner: "root".to_string(),
                name: "acme".to_string(),
                full_name: Some("Acme Org".to_string()),
                description: Some("desc".to_string()),
                visibility: Some("private".to_string()),
            })
        );

        let missing = AdminCli::parse(["gittree-admin", "create-org", "--name=acme"])
            .expect_err("missing owner");
        assert!(matches!(missing, AdminCliError::MissingValue("--owner")));
    }

    #[test]
    fn parse_create_org_rejects_missing_optional_values_and_unknown_flag() {
        let split_optional = AdminCli::parse([
            "gittree-admin",
            "create-org",
            "--owner",
            "root",
            "--name",
            "acme",
            "--full-name",
            "Acme Org",
            "--description",
            "desc",
            "--visibility",
            "private",
        ])
        .expect("split optional args");
        assert_eq!(
            split_optional.command,
            Some(AdminCommand::CreateOrg {
                owner: "root".to_string(),
                name: "acme".to_string(),
                full_name: Some("Acme Org".to_string()),
                description: Some("desc".to_string()),
                visibility: Some("private".to_string()),
            })
        );

        let missing_cases = [
            (
                vec![
                    "gittree-admin",
                    "create-org",
                    "--owner=root",
                    "--name=acme",
                    "--full-name",
                ],
                "--full-name",
            ),
            (
                vec![
                    "gittree-admin",
                    "create-org",
                    "--owner=root",
                    "--name=acme",
                    "--description",
                ],
                "--description",
            ),
            (
                vec![
                    "gittree-admin",
                    "create-org",
                    "--owner=root",
                    "--name=acme",
                    "--visibility",
                ],
                "--visibility",
            ),
            (
                vec!["gittree-admin", "create-org", "--owner=", "--name=acme"],
                "--owner",
            ),
            (
                vec!["gittree-admin", "create-org", "--owner=root", "--name="],
                "--name",
            ),
            (
                vec![
                    "gittree-admin",
                    "create-org",
                    "--owner=root",
                    "--name=acme",
                    "--full-name=",
                ],
                "--full-name",
            ),
            (
                vec![
                    "gittree-admin",
                    "create-org",
                    "--owner=root",
                    "--name=acme",
                    "--description=",
                ],
                "--description",
            ),
            (
                vec![
                    "gittree-admin",
                    "create-org",
                    "--owner=root",
                    "--name=acme",
                    "--visibility=",
                ],
                "--visibility",
            ),
        ];

        for (args, flag) in missing_cases {
            let err = AdminCli::parse(args).expect_err("missing value");
            assert!(matches!(err, AdminCliError::MissingValue(missing) if missing == flag));
        }

        let unknown = AdminCli::parse([
            "gittree-admin",
            "create-org",
            "--owner=root",
            "--name=acme",
            "--nope",
        ])
        .expect_err("unknown flag");
        assert!(matches!(unknown, AdminCliError::UnknownFlag(flag) if flag == "--nope"));

        let split_missing_cases = [
            (vec!["gittree-admin", "create-org", "--owner"], "--owner"),
            (
                vec!["gittree-admin", "create-org", "--owner=root", "--name"],
                "--name",
            ),
        ];
        for (args, flag) in split_missing_cases {
            let err = AdminCli::parse(args).expect_err("missing split flag value");
            assert!(matches!(err, AdminCliError::MissingValue(missing) if missing == flag));
        }
    }

    #[test]
    fn parse_create_repo_accepts_flags_and_equals_values() {
        let cli = AdminCli::parse([
            "gittree-admin",
            "create-repo",
            "--owner=alice",
            "--name=demo",
            "--description=demo repo",
            "--private",
            "--auto-init",
        ])
        .expect("parse");
        assert_eq!(
            cli.command,
            Some(AdminCommand::CreateRepo {
                owner: "alice".to_string(),
                name: "demo".to_string(),
                description: Some("demo repo".to_string()),
                private: Some(true),
                auto_init: Some(true),
            })
        );

        let missing = AdminCli::parse(["gittree-admin", "create-repo", "--owner=alice"])
            .expect_err("missing name");
        assert!(matches!(missing, AdminCliError::MissingValue("--name")));
    }

    #[test]
    fn parse_create_repo_rejects_missing_optional_and_unknown_flags() {
        let split_description = AdminCli::parse([
            "gittree-admin",
            "create-repo",
            "--owner",
            "alice",
            "--name",
            "demo",
            "--description",
            "demo repo",
        ])
        .expect("split description");
        assert_eq!(
            split_description.command,
            Some(AdminCommand::CreateRepo {
                owner: "alice".to_string(),
                name: "demo".to_string(),
                description: Some("demo repo".to_string()),
                private: None,
                auto_init: None,
            })
        );

        let missing_description = AdminCli::parse([
            "gittree-admin",
            "create-repo",
            "--owner=alice",
            "--name=demo",
            "--description",
        ])
        .expect_err("missing description");
        assert!(matches!(
            missing_description,
            AdminCliError::MissingValue("--description")
        ));

        let empty_owner =
            AdminCli::parse(["gittree-admin", "create-repo", "--owner=", "--name=demo"])
                .expect_err("empty owner");
        assert!(matches!(
            empty_owner,
            AdminCliError::MissingValue("--owner")
        ));

        let empty_name =
            AdminCli::parse(["gittree-admin", "create-repo", "--owner=alice", "--name="])
                .expect_err("empty name");
        assert!(matches!(empty_name, AdminCliError::MissingValue("--name")));

        let empty_description = AdminCli::parse([
            "gittree-admin",
            "create-repo",
            "--owner=alice",
            "--name=demo",
            "--description=",
        ])
        .expect_err("empty description");
        assert!(matches!(
            empty_description,
            AdminCliError::MissingValue("--description")
        ));

        let unknown = AdminCli::parse([
            "gittree-admin",
            "create-repo",
            "--owner=alice",
            "--name=demo",
            "--nope",
        ])
        .expect_err("unknown flag");
        assert!(matches!(unknown, AdminCliError::UnknownFlag(flag) if flag == "--nope"));

        let split_missing_cases = [
            (vec!["gittree-admin", "create-repo", "--owner"], "--owner"),
            (
                vec!["gittree-admin", "create-repo", "--owner=alice", "--name"],
                "--name",
            ),
        ];
        for (args, flag) in split_missing_cases {
            let err = AdminCli::parse(args).expect_err("missing split flag value");
            assert!(matches!(err, AdminCliError::MissingValue(missing) if missing == flag));
        }
    }

    #[test]
    fn parse_create_pull_accepts_body_and_equals_values() {
        let cli = AdminCli::parse([
            "gittree-admin",
            "create-pull",
            "--owner=alice",
            "--repo=demo",
            "--head=feature",
            "--base=main",
            "--title=Update",
            "--body=details",
        ])
        .expect("parse");
        assert_eq!(
            cli.command,
            Some(AdminCommand::CreatePull {
                owner: "alice".to_string(),
                repo: "demo".to_string(),
                head: "feature".to_string(),
                base: "main".to_string(),
                title: "Update".to_string(),
                body: Some("details".to_string()),
            })
        );

        let missing = AdminCli::parse([
            "gittree-admin",
            "create-pull",
            "--owner=alice",
            "--repo=demo",
            "--head=feature",
            "--base=main",
            "--title=",
        ])
        .expect_err("missing title");
        assert!(matches!(missing, AdminCliError::MissingValue("--title")));
    }

    #[test]
    fn parse_create_pull_rejects_missing_and_unknown_flags() {
        let split_body = AdminCli::parse([
            "gittree-admin",
            "create-pull",
            "--owner",
            "alice",
            "--repo",
            "demo",
            "--head",
            "feature",
            "--base",
            "main",
            "--title",
            "Update",
            "--body",
            "details",
        ])
        .expect("split body");
        assert_eq!(
            split_body.command,
            Some(AdminCommand::CreatePull {
                owner: "alice".to_string(),
                repo: "demo".to_string(),
                head: "feature".to_string(),
                base: "main".to_string(),
                title: "Update".to_string(),
                body: Some("details".to_string()),
            })
        );

        let missing_body = AdminCli::parse([
            "gittree-admin",
            "create-pull",
            "--owner=alice",
            "--repo=demo",
            "--head=feature",
            "--base=main",
            "--title=Update",
            "--body",
        ])
        .expect_err("missing body");
        assert!(matches!(
            missing_body,
            AdminCliError::MissingValue("--body")
        ));

        let missing_cases = [
            (
                vec![
                    "gittree-admin",
                    "create-pull",
                    "--owner=",
                    "--repo=demo",
                    "--head=feature",
                    "--base=main",
                    "--title=Update",
                ],
                "--owner",
            ),
            (
                vec![
                    "gittree-admin",
                    "create-pull",
                    "--owner=alice",
                    "--repo=",
                    "--head=feature",
                    "--base=main",
                    "--title=Update",
                ],
                "--repo",
            ),
            (
                vec![
                    "gittree-admin",
                    "create-pull",
                    "--owner=alice",
                    "--repo=demo",
                    "--head=",
                    "--base=main",
                    "--title=Update",
                ],
                "--head",
            ),
            (
                vec![
                    "gittree-admin",
                    "create-pull",
                    "--owner=alice",
                    "--repo=demo",
                    "--head=feature",
                    "--base=",
                    "--title=Update",
                ],
                "--base",
            ),
            (
                vec![
                    "gittree-admin",
                    "create-pull",
                    "--owner=alice",
                    "--repo=demo",
                    "--head=feature",
                    "--base=main",
                    "--title=",
                ],
                "--title",
            ),
            (
                vec![
                    "gittree-admin",
                    "create-pull",
                    "--owner=alice",
                    "--repo=demo",
                    "--head=feature",
                    "--base=main",
                    "--title=Update",
                    "--body=",
                ],
                "--body",
            ),
        ];
        for (args, flag) in missing_cases {
            let err = AdminCli::parse(args).expect_err("missing value");
            assert!(matches!(err, AdminCliError::MissingValue(missing) if missing == flag));
        }

        let unknown = AdminCli::parse([
            "gittree-admin",
            "create-pull",
            "--owner=alice",
            "--repo=demo",
            "--head=feature",
            "--base=main",
            "--title=Update",
            "--nope",
        ])
        .expect_err("unknown flag");
        assert!(matches!(unknown, AdminCliError::UnknownFlag(flag) if flag == "--nope"));

        let split_missing_cases = [
            (vec!["gittree-admin", "create-pull", "--owner"], "--owner"),
            (
                vec!["gittree-admin", "create-pull", "--owner=alice", "--repo"],
                "--repo",
            ),
            (
                vec![
                    "gittree-admin",
                    "create-pull",
                    "--owner=alice",
                    "--repo=demo",
                    "--head",
                ],
                "--head",
            ),
            (
                vec![
                    "gittree-admin",
                    "create-pull",
                    "--owner=alice",
                    "--repo=demo",
                    "--head=feature",
                    "--base",
                ],
                "--base",
            ),
            (
                vec![
                    "gittree-admin",
                    "create-pull",
                    "--owner=alice",
                    "--repo=demo",
                    "--head=feature",
                    "--base=main",
                    "--title",
                ],
                "--title",
            ),
        ];
        for (args, flag) in split_missing_cases {
            let err = AdminCli::parse(args).expect_err("missing split flag value");
            assert!(matches!(err, AdminCliError::MissingValue(missing) if missing == flag));
        }
    }

    #[test]
    fn cli_error_display_and_source_are_stable() {
        assert_eq!(
            format!("{}", AdminCliError::UnknownCommand("nope".to_string())),
            "unknown command nope"
        );
        assert_eq!(
            format!("{}", AdminCliError::UnknownFlag("--bad".to_string())),
            "unknown flag --bad"
        );
        assert_eq!(
            format!("{}", AdminCliError::MissingValue("--name")),
            "missing value for --name"
        );
        assert_eq!(
            format!("{}", AdminCliError::MissingCommand),
            "missing command"
        );

        let err = AdminCliError::UnknownFlag("--bad".to_string());
        let source = err.source();
        assert!(source.is_none());
    }

    #[test]
    fn parse_map_with_string_args_covers_split_and_missing_required() {
        let pubkey = "33".repeat(32);
        let cli = AdminCli::parse(string_args(&[
            "gittree-admin",
            "map",
            "--forgejo",
            "owner/repo",
            "--pubkey",
            &pubkey,
            "--identifier",
            "repo",
        ]))
        .expect("parse map with split args");
        assert_eq!(
            cli.command,
            Some(AdminCommand::Map {
                forgejo: "owner/repo".to_string(),
                pubkey,
                identifier: "repo".to_string(),
            })
        );

        let missing_forgejo = AdminCli::parse(string_args(&[
            "gittree-admin",
            "map",
            "--pubkey",
            "aa",
            "--identifier",
            "repo",
        ]))
        .expect_err("missing forgejo");
        assert!(matches!(
            missing_forgejo,
            AdminCliError::MissingValue("--forgejo")
        ));

        let missing_pubkey = AdminCli::parse(string_args(&[
            "gittree-admin",
            "map",
            "--forgejo",
            "owner/repo",
            "--identifier",
            "repo",
        ]))
        .expect_err("missing pubkey");
        assert!(matches!(
            missing_pubkey,
            AdminCliError::MissingValue("--pubkey")
        ));

        let missing_identifier = AdminCli::parse(string_args(&[
            "gittree-admin",
            "map",
            "--forgejo",
            "owner/repo",
            "--pubkey",
            "aa",
        ]))
        .expect_err("missing identifier");
        assert!(matches!(
            missing_identifier,
            AdminCliError::MissingValue("--identifier")
        ));
    }

    #[test]
    fn parse_create_user_with_string_args_covers_split_and_missing_required() {
        let cli = AdminCli::parse(string_args(&[
            "gittree-admin",
            "create-user",
            "--username",
            "alice",
            "--email",
            "alice@example.com",
            "--password",
            "secret",
        ]))
        .expect("parse create-user with split args");
        assert_eq!(
            cli.command,
            Some(AdminCommand::CreateUser {
                username: "alice".to_string(),
                email: "alice@example.com".to_string(),
                password: "secret".to_string(),
                full_name: None,
                must_change_password: None,
                send_notify: None,
            })
        );

        let missing_username = AdminCli::parse(string_args(&[
            "gittree-admin",
            "create-user",
            "--email",
            "alice@example.com",
            "--password",
            "secret",
        ]))
        .expect_err("missing username");
        assert!(matches!(
            missing_username,
            AdminCliError::MissingValue("--username")
        ));

        let missing_email = AdminCli::parse(string_args(&[
            "gittree-admin",
            "create-user",
            "--username",
            "alice",
            "--password",
            "secret",
        ]))
        .expect_err("missing email");
        assert!(matches!(
            missing_email,
            AdminCliError::MissingValue("--email")
        ));

        let missing_password = AdminCli::parse(string_args(&[
            "gittree-admin",
            "create-user",
            "--username",
            "alice",
            "--email",
            "alice@example.com",
        ]))
        .expect_err("missing password");
        assert!(matches!(
            missing_password,
            AdminCliError::MissingValue("--password")
        ));
    }

    #[test]
    fn parse_org_repo_pull_with_string_args_covers_split_and_missing_required() {
        let org = AdminCli::parse(string_args(&[
            "gittree-admin",
            "create-org",
            "--owner",
            "root",
            "--name",
            "acme",
        ]))
        .expect("parse create-org");
        assert_eq!(
            org.command,
            Some(AdminCommand::CreateOrg {
                owner: "root".to_string(),
                name: "acme".to_string(),
                full_name: None,
                description: None,
                visibility: None,
            })
        );

        let missing_org_name = AdminCli::parse(string_args(&[
            "gittree-admin",
            "create-org",
            "--owner",
            "root",
        ]))
        .expect_err("missing org name");
        assert!(matches!(
            missing_org_name,
            AdminCliError::MissingValue("--name")
        ));

        let repo = AdminCli::parse(string_args(&[
            "gittree-admin",
            "create-repo",
            "--owner",
            "alice",
            "--name",
            "demo",
        ]))
        .expect("parse create-repo");
        assert_eq!(
            repo.command,
            Some(AdminCommand::CreateRepo {
                owner: "alice".to_string(),
                name: "demo".to_string(),
                description: None,
                private: None,
                auto_init: None,
            })
        );

        let missing_repo_owner = AdminCli::parse(string_args(&[
            "gittree-admin",
            "create-repo",
            "--name",
            "demo",
        ]))
        .expect_err("missing repo owner");
        assert!(matches!(
            missing_repo_owner,
            AdminCliError::MissingValue("--owner")
        ));

        let pull = AdminCli::parse(string_args(&[
            "gittree-admin",
            "create-pull",
            "--owner",
            "alice",
            "--repo",
            "demo",
            "--head",
            "feature",
            "--base",
            "main",
            "--title",
            "Update",
        ]))
        .expect("parse create-pull");
        assert_eq!(
            pull.command,
            Some(AdminCommand::CreatePull {
                owner: "alice".to_string(),
                repo: "demo".to_string(),
                head: "feature".to_string(),
                base: "main".to_string(),
                title: "Update".to_string(),
                body: None,
            })
        );

        let missing_pull_fields = [
            (
                vec![
                    "gittree-admin",
                    "create-pull",
                    "--repo",
                    "demo",
                    "--head",
                    "feature",
                    "--base",
                    "main",
                    "--title",
                    "Update",
                ],
                "--owner",
            ),
            (
                vec![
                    "gittree-admin",
                    "create-pull",
                    "--owner",
                    "alice",
                    "--head",
                    "feature",
                    "--base",
                    "main",
                    "--title",
                    "Update",
                ],
                "--repo",
            ),
            (
                vec![
                    "gittree-admin",
                    "create-pull",
                    "--owner",
                    "alice",
                    "--repo",
                    "demo",
                    "--base",
                    "main",
                    "--title",
                    "Update",
                ],
                "--head",
            ),
            (
                vec![
                    "gittree-admin",
                    "create-pull",
                    "--owner",
                    "alice",
                    "--repo",
                    "demo",
                    "--head",
                    "feature",
                    "--title",
                    "Update",
                ],
                "--base",
            ),
            (
                vec![
                    "gittree-admin",
                    "create-pull",
                    "--owner",
                    "alice",
                    "--repo",
                    "demo",
                    "--head",
                    "feature",
                    "--base",
                    "main",
                ],
                "--title",
            ),
        ];
        for (args, flag) in missing_pull_fields {
            let err =
                AdminCli::parse(string_args(args.as_slice())).expect_err("missing pull field");
            assert!(matches!(err, AdminCliError::MissingValue(value) if value == flag));
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedCommand {
    pub namespace: CommandNamespace,
    pub action: String,
    pub target: Option<String>,
    pub args: Vec<CommandArg>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandNamespace {
    Account,
    Profile,
    Repo,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandArg {
    Positional(String),
    KeyValue { key: String, value: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandParseError {
    MissingPrefix,
    EmptyCommand,
    InvalidNamespace(String),
    UnterminatedQuote,
    InvalidCommand(String),
    InvalidArgs(String),
}

impl std::fmt::Display for CommandParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CommandParseError::MissingPrefix => write!(f, "missing gittree command prefix"),
            CommandParseError::EmptyCommand => write!(f, "empty command"),
            CommandParseError::InvalidNamespace(value) => {
                write!(f, "invalid namespace: {value}")
            }
            CommandParseError::UnterminatedQuote => write!(f, "unterminated quote"),
            CommandParseError::InvalidCommand(value) => write!(f, "invalid command: {value}"),
            CommandParseError::InvalidArgs(value) => write!(f, "invalid args: {value}"),
        }
    }
}

impl std::error::Error for CommandParseError {}

pub fn parse_cli_command(input: &str) -> Result<ParsedCommand, CommandParseError> {
    let trimmed = input.trim();
    let rest = trimmed
        .strip_prefix("gittree ")
        .ok_or(CommandParseError::MissingPrefix)?;
    let tokens = tokenize(rest)?;
    if tokens.is_empty() {
        return Err(CommandParseError::EmptyCommand);
    }

    let namespace = parse_namespace(&tokens[0])?;
    let action = tokens.get(1).cloned().unwrap_or_default();
    if action.is_empty() {
        return Err(CommandParseError::InvalidCommand(
            "missing subcommand/action".to_string(),
        ));
    }

    let mut target = None;
    let mut args = Vec::new();

    for token in tokens.into_iter().skip(2) {
        if let Some((key, value)) = token.split_once('=') {
            args.push(CommandArg::KeyValue {
                key: key.to_string(),
                value: value.to_string(),
            });
            continue;
        }

        if target.is_none() && namespace == CommandNamespace::Repo {
            target = Some(token);
        } else {
            args.push(CommandArg::Positional(token));
        }
    }

    validate_shape(namespace, &action, target.as_deref(), &args)?;

    Ok(ParsedCommand {
        namespace,
        action,
        target,
        args,
    })
}

fn parse_namespace(value: &str) -> Result<CommandNamespace, CommandParseError> {
    match value {
        "account" => Ok(CommandNamespace::Account),
        "profile" => Ok(CommandNamespace::Profile),
        "repo" => Ok(CommandNamespace::Repo),
        other => Err(CommandParseError::InvalidNamespace(other.to_string())),
    }
}

fn validate_shape(
    namespace: CommandNamespace,
    action: &str,
    target: Option<&str>,
    args: &[CommandArg],
) -> Result<(), CommandParseError> {
    match namespace {
        CommandNamespace::Account => match action {
            "create" | "status" | "delete" if target.is_none() && args.is_empty() => Ok(()),
            "create" | "status" | "delete" => Err(CommandParseError::InvalidArgs(
                "account commands accept no target or args".to_string(),
            )),
            _ => Err(CommandParseError::InvalidCommand(action.to_string())),
        },
        CommandNamespace::Profile => match action {
            "set" if target.is_none() && !args.is_empty() => Ok(()),
            "set" => Err(CommandParseError::InvalidArgs(
                "profile set requires key=value args".to_string(),
            )),
            "visibility" if target.is_none() => match args {
                [CommandArg::Positional(value)] if value == "public" || value == "private" => {
                    Ok(())
                }
                _ => Err(CommandParseError::InvalidArgs(
                    "profile visibility requires public|private".to_string(),
                )),
            },
            "visibility" => Err(CommandParseError::InvalidArgs(
                "profile visibility accepts no target".to_string(),
            )),
            _ => Err(CommandParseError::InvalidCommand(action.to_string())),
        },
        CommandNamespace::Repo => match action {
            "create" | "announce" | "sync" | "archive" | "unarchive"
                if target.is_some() && args.is_empty() =>
            {
                Ok(())
            }
            "update" if target.is_some() && !args.is_empty() => Ok(()),
            "maintainers" if target.is_some() => match args {
                [CommandArg::Positional(sub), CommandArg::Positional(npub)]
                    if (sub == "add" || sub == "remove") && npub.starts_with("npub") =>
                {
                    Ok(())
                }
                _ => Err(CommandParseError::InvalidArgs(
                    "repo maintainers requires add|remove and npub".to_string(),
                )),
            },
            "create" | "announce" | "sync" | "archive" | "unarchive" => {
                Err(CommandParseError::InvalidArgs(
                    "repo command requires target and no extra args".to_string(),
                ))
            }
            "update" => Err(CommandParseError::InvalidArgs(
                "repo update requires target and key=value args".to_string(),
            )),
            "maintainers" => Err(CommandParseError::InvalidArgs(
                "repo maintainers requires target".to_string(),
            )),
            _ => Err(CommandParseError::InvalidCommand(action.to_string())),
        },
    }
}

fn tokenize(input: &str) -> Result<Vec<String>, CommandParseError> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    let mut escape = false;

    for ch in input.chars() {
        if escape {
            current.push(ch);
            escape = false;
            continue;
        }

        match ch {
            '\\' => {
                escape = true;
            }
            '"' => {
                in_quotes = !in_quotes;
            }
            ch if ch.is_whitespace() && !in_quotes => {
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
            }
            _ => current.push(ch),
        }
    }

    if in_quotes {
        return Err(CommandParseError::UnterminatedQuote);
    }

    if !current.is_empty() {
        tokens.push(current);
    }

    Ok(tokens)
}

#[cfg(test)]
mod tests {
    use super::{
        CommandArg, CommandNamespace, CommandParseError, ParsedCommand, parse_cli_command,
    };

    fn parsed(input: &str) -> ParsedCommand {
        parse_cli_command(input).expect("parse")
    }

    #[test]
    fn parses_account_create() {
        let cmd = parsed("gittree account create");
        assert_eq!(cmd.namespace, CommandNamespace::Account);
        assert_eq!(cmd.action, "create");
        assert_eq!(cmd.target, None);
        assert!(cmd.args.is_empty());
    }

    #[test]
    fn parses_profile_visibility_private() {
        let cmd = parsed("gittree profile visibility private");
        assert_eq!(cmd.namespace, CommandNamespace::Profile);
        assert_eq!(cmd.action, "visibility");
        assert_eq!(cmd.target, None);
        assert_eq!(cmd.args, vec![CommandArg::Positional("private".to_string())]);
    }

    #[test]
    fn parses_repo_update_with_key_values() {
        let cmd = parsed("gittree repo update my-repo description=hello website=gittr.ee");
        assert_eq!(cmd.namespace, CommandNamespace::Repo);
        assert_eq!(cmd.action, "update");
        assert_eq!(cmd.target.as_deref(), Some("my-repo"));
        assert_eq!(
            cmd.args,
            vec![
                CommandArg::KeyValue {
                    key: "description".to_string(),
                    value: "hello".to_string(),
                },
                CommandArg::KeyValue {
                    key: "website".to_string(),
                    value: "gittr.ee".to_string(),
                }
            ]
        );
    }

    #[test]
    fn parses_quoted_values() {
        let cmd = parsed("gittree profile set bio=\"hello world\"");
        assert_eq!(cmd.namespace, CommandNamespace::Profile);
        assert_eq!(
            cmd.args,
            vec![CommandArg::KeyValue {
                key: "bio".to_string(),
                value: "hello world".to_string(),
            }]
        );
    }

    #[test]
    fn rejects_missing_prefix() {
        let err = parse_cli_command("account create").expect_err("prefix required");
        assert_eq!(err, CommandParseError::MissingPrefix);
    }

    #[test]
    fn rejects_unknown_namespace() {
        let err = parse_cli_command("gittree unknown run").expect_err("invalid namespace");
        assert!(matches!(err, CommandParseError::InvalidNamespace(value) if value == "unknown"));
    }

    #[test]
    fn rejects_unterminated_quote() {
        let err = parse_cli_command("gittree profile set bio=\"hello").expect_err("quote");
        assert_eq!(err, CommandParseError::UnterminatedQuote);
    }

    #[test]
    fn rejects_invalid_repo_shape() {
        let err = parse_cli_command("gittree repo create").expect_err("target required");
        assert!(matches!(err, CommandParseError::InvalidArgs(message) if message.contains("target")));
    }

    #[test]
    fn rejects_invalid_profile_visibility() {
        let err =
            parse_cli_command("gittree profile visibility hidden").expect_err("visibility required");
        assert!(matches!(err, CommandParseError::InvalidArgs(message) if message.contains("public|private")));
    }
}

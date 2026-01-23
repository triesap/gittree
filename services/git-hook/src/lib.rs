use gittree_config::{ConfigError, ServicesConfig};

const ENV_STATE_URL: &str = "GITTREE_STATE_URL";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookConfig {
    pub state_url: String,
}

impl HookConfig {
    pub fn from_env() -> Result<Self, HookConfigError> {
        let _services = ServicesConfig::from_env_validated().map_err(HookConfigError::Config)?;
        let state_url =
            std::env::var(ENV_STATE_URL).map_err(|_| HookConfigError::MissingEnv(ENV_STATE_URL))?;
        Ok(Self { state_url })
    }
}

#[derive(Debug)]
pub enum HookConfigError {
    Config(ConfigError),
    MissingEnv(&'static str),
}

impl std::fmt::Display for HookConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HookConfigError::Config(err) => write!(f, "hook config error: {err}"),
            HookConfigError::MissingEnv(key) => write!(f, "missing env {key}"),
        }
    }
}

impl std::error::Error for HookConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            HookConfigError::Config(err) => Some(err),
            HookConfigError::MissingEnv(_) => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefUpdate {
    pub old: String,
    pub new: String,
    pub reference: String,
}

#[derive(Debug)]
pub enum HookError {
    InvalidLine(String),
}

impl std::fmt::Display for HookError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HookError::InvalidLine(line) => write!(f, "invalid ref line: {line}"),
        }
    }
}

impl std::error::Error for HookError {}

#[derive(Debug)]
pub enum HookServiceError {
    Config(HookConfigError),
    Parse(HookError),
}

impl std::fmt::Display for HookServiceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HookServiceError::Config(err) => write!(f, "hook config error: {err}"),
            HookServiceError::Parse(err) => write!(f, "hook parse error: {err}"),
        }
    }
}

impl std::error::Error for HookServiceError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            HookServiceError::Config(err) => Some(err),
            HookServiceError::Parse(err) => Some(err),
        }
    }
}

pub fn parse_updates(input: &str) -> Result<Vec<RefUpdate>, HookError> {
    let mut updates = Vec::new();
    for line in input.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let mut parts = line.split_whitespace();
        let old = parts
            .next()
            .ok_or_else(|| HookError::InvalidLine(line.to_string()))?;
        let new = parts
            .next()
            .ok_or_else(|| HookError::InvalidLine(line.to_string()))?;
        let reference = parts
            .next()
            .ok_or_else(|| HookError::InvalidLine(line.to_string()))?;
        updates.push(RefUpdate {
            old: old.to_string(),
            new: new.to_string(),
            reference: reference.to_string(),
        });
    }
    Ok(updates)
}

#[cfg(test)]
mod tests {
    use super::HookError;
    use super::parse_updates;

    #[test]
    fn parse_updates_accepts_lines() {
        let input = "old new refs/heads/main\nold2 new2 refs/tags/v1";
        let updates = parse_updates(input).expect("updates");
        assert_eq!(updates.len(), 2);
        assert_eq!(updates[0].reference, "refs/heads/main");
        assert_eq!(updates[1].new, "new2");
    }

    #[test]
    fn parse_updates_rejects_missing_fields() {
        let err = parse_updates("only-two parts").unwrap_err();
        assert!(matches!(err, HookError::InvalidLine(_)));
    }
}

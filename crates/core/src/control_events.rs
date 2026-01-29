use crate::{CoreError, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum ControlAction {
    CreateUser {
        username: String,
        email: String,
        password: String,
        admin: Option<bool>,
        must_change_password: Option<bool>,
    },
    CreateOrg {
        name: String,
        full_name: Option<String>,
        description: Option<String>,
    },
    CreateRepo {
        name: String,
        owner: Option<String>,
        description: Option<String>,
        private: Option<bool>,
    },
    CreatePullRequest {
        owner: String,
        repo: String,
        head: String,
        base: String,
        title: String,
        body: Option<String>,
        draft: Option<bool>,
    },
}

impl ControlAction {
    pub fn parse(kind: u32, content: &str, expected_kind: u32) -> Result<Self> {
        if kind != expected_kind {
            return Err(CoreError::InvalidField {
                field: "kind",
                value: kind.to_string(),
            });
        }
        if content.trim().is_empty() {
            return Err(CoreError::MissingField("content"));
        }
        let action: ControlAction = serde_json::from_str(content).map_err(|_| {
            CoreError::InvalidField {
                field: "content",
                value: "invalid json".to_string(),
            }
        })?;
        action.validate()?;
        Ok(action)
    }

    pub fn validate(&self) -> Result<()> {
        match self {
            ControlAction::CreateUser {
                username,
                email,
                password,
                ..
            } => {
                require_non_empty("username", username)?;
                require_non_empty("email", email)?;
                require_non_empty("password", password)?;
            }
            ControlAction::CreateOrg {
                name, full_name, ..
            } => {
                require_non_empty("name", name)?;
                if let Some(value) = full_name {
                    require_non_empty("full_name", value)?;
                }
            }
            ControlAction::CreateRepo { name, owner, .. } => {
                require_non_empty("name", name)?;
                if let Some(value) = owner {
                    require_non_empty("owner", value)?;
                }
            }
            ControlAction::CreatePullRequest {
                owner,
                repo,
                head,
                base,
                title,
                ..
            } => {
                require_non_empty("owner", owner)?;
                require_non_empty("repo", repo)?;
                require_non_empty("head", head)?;
                require_non_empty("base", base)?;
                require_non_empty("title", title)?;
            }
        }
        Ok(())
    }
}

fn require_non_empty(field: &'static str, value: &str) -> Result<()> {
    if value.trim().is_empty() {
        return Err(CoreError::InvalidField {
            field,
            value: "".to_string(),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::ControlAction;
    use crate::kinds::KIND_GITTREE_CONTROL;
    use crate::CoreError;

    #[test]
    fn parse_accepts_valid_payload() {
        let json = r#"{"action":"create_repo","name":"hello-ngit","owner":"gittree"}"#;
        let action =
            ControlAction::parse(KIND_GITTREE_CONTROL.0, json, KIND_GITTREE_CONTROL.0)
                .expect("action");
        assert!(matches!(action, ControlAction::CreateRepo { .. }));
    }

    #[test]
    fn parse_rejects_wrong_kind() {
        let json = r#"{"action":"create_repo","name":"hello-ngit","owner":"gittree"}"#;
        let err = ControlAction::parse(1, json, KIND_GITTREE_CONTROL.0).unwrap_err();
        assert!(matches!(
            err,
            CoreError::InvalidField { field: "kind", .. }
        ));
    }

    #[test]
    fn parse_rejects_invalid_json() {
        let err = ControlAction::parse(KIND_GITTREE_CONTROL.0, "not-json", KIND_GITTREE_CONTROL.0)
            .unwrap_err();
        assert!(matches!(
            err,
            CoreError::InvalidField {
                field: "content",
                ..
            }
        ));
    }

    #[test]
    fn parse_rejects_empty_required_field() {
        let json = r#"{"action":"create_org","name":""}"#;
        let err =
            ControlAction::parse(KIND_GITTREE_CONTROL.0, json, KIND_GITTREE_CONTROL.0).unwrap_err();
        assert!(matches!(
            err,
            CoreError::InvalidField {
                field: "name",
                ..
            }
        ));
    }
}

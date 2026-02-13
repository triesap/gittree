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
        identifier: Option<String>,
        description: Option<String>,
        private: Option<bool>,
        pubkey: String,
        privkey: String,
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
            ControlAction::CreateRepo {
                name,
                owner,
                identifier,
                pubkey,
                privkey,
                ..
            } => {
                require_non_empty("name", name)?;
                if let Some(value) = owner {
                    require_non_empty("owner", value)?;
                }
                if let Some(value) = identifier {
                    require_non_empty("identifier", value)?;
                }
                require_hex64("pubkey", pubkey)?;
                require_hex64("privkey", privkey)?;
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

fn require_hex64(field: &'static str, value: &str) -> Result<()> {
    if value.len() != 64 || !is_hex(value) {
        return Err(CoreError::InvalidField {
            field,
            value: value.to_string(),
        });
    }
    Ok(())
}

fn is_hex(value: &str) -> bool {
    value.as_bytes().iter().all(|b| b.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::ControlAction;
    use crate::kinds::KIND_GITTREE_CONTROL;
    use crate::CoreError;

    #[test]
    fn parse_accepts_valid_payload() {
        let json = r#"{"action":"create_repo","name":"hello-ngit","owner":"gittree","pubkey":"11e92f29b2e2d3c4b5a69788796a5b4c3d2e1f0a9b8c7d6e5f4a3b2c1d0e9f8a","privkey":"22e92f29b2e2d3c4b5a69788796a5b4c3d2e1f0a9b8c7d6e5f4a3b2c1d0e9f8b"}"#;
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
    fn parse_rejects_empty_content() {
        let err = ControlAction::parse(KIND_GITTREE_CONTROL.0, "  \n\t", KIND_GITTREE_CONTROL.0)
            .unwrap_err();
        assert!(matches!(err, CoreError::MissingField("content")));
    }

    #[test]
    fn parse_accepts_valid_create_user_payload() {
        let json = r#"{"action":"create_user","username":"alice","email":"alice@example.com","password":"secret-password"}"#;
        let action =
            ControlAction::parse(KIND_GITTREE_CONTROL.0, json, KIND_GITTREE_CONTROL.0)
                .expect("action");
        assert!(matches!(action, ControlAction::CreateUser { .. }));
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

    #[test]
    fn parse_rejects_empty_optional_full_name() {
        let json = r#"{"action":"create_org","name":"acme","full_name":"   "}"#;
        let err =
            ControlAction::parse(KIND_GITTREE_CONTROL.0, json, KIND_GITTREE_CONTROL.0).unwrap_err();
        assert!(matches!(
            err,
            CoreError::InvalidField {
                field: "full_name",
                ..
            }
        ));
    }

    #[test]
    fn parse_rejects_invalid_pubkey() {
        let json = r#"{"action":"create_repo","name":"hello-ngit","owner":"gittree","pubkey":"not-hex","privkey":"22e92f29b2e2d3c4b5a69788796a5b4c3d2e1f0a9b8c7d6e5f4a3b2c1d0e9f8b"}"#;
        let err =
            ControlAction::parse(KIND_GITTREE_CONTROL.0, json, KIND_GITTREE_CONTROL.0).unwrap_err();
        assert!(matches!(
            err,
            CoreError::InvalidField {
                field: "pubkey",
                ..
            }
        ));
    }

    #[test]
    fn parse_rejects_empty_optional_owner() {
        let json = r#"{"action":"create_repo","name":"hello-ngit","owner":"   ","pubkey":"11e92f29b2e2d3c4b5a69788796a5b4c3d2e1f0a9b8c7d6e5f4a3b2c1d0e9f8a","privkey":"22e92f29b2e2d3c4b5a69788796a5b4c3d2e1f0a9b8c7d6e5f4a3b2c1d0e9f8b"}"#;
        let err =
            ControlAction::parse(KIND_GITTREE_CONTROL.0, json, KIND_GITTREE_CONTROL.0).unwrap_err();
        assert!(matches!(
            err,
            CoreError::InvalidField {
                field: "owner",
                ..
            }
        ));
    }

    #[test]
    fn parse_rejects_empty_optional_identifier() {
        let json = r#"{"action":"create_repo","name":"hello-ngit","owner":"gittree","identifier":"  ","pubkey":"11e92f29b2e2d3c4b5a69788796a5b4c3d2e1f0a9b8c7d6e5f4a3b2c1d0e9f8a","privkey":"22e92f29b2e2d3c4b5a69788796a5b4c3d2e1f0a9b8c7d6e5f4a3b2c1d0e9f8b"}"#;
        let err =
            ControlAction::parse(KIND_GITTREE_CONTROL.0, json, KIND_GITTREE_CONTROL.0).unwrap_err();
        assert!(matches!(
            err,
            CoreError::InvalidField {
                field: "identifier",
                ..
            }
        ));
    }

    #[test]
    fn parse_accepts_valid_pull_request_payload() {
        let json = r#"{"action":"create_pull_request","owner":"gittree","repo":"repo-one","head":"feature-branch","base":"main","title":"my pull request"}"#;
        let action =
            ControlAction::parse(KIND_GITTREE_CONTROL.0, json, KIND_GITTREE_CONTROL.0)
                .expect("action");
        assert!(matches!(action, ControlAction::CreatePullRequest { .. }));
    }
}

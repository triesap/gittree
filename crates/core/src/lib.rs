#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum CoreError {
    MissingField(&'static str),
    InvalidField { field: &'static str, value: String },
    InvalidTag { tag: &'static str, value: String },
}

pub type Result<T> = std::result::Result<T, CoreError>;

impl std::fmt::Display for CoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CoreError::MissingField(field) => write!(f, "missing required field: {field}"),
            CoreError::InvalidField { field, value } => {
                write!(f, "invalid field {field}: {value}")
            }
            CoreError::InvalidTag { tag, value } => write!(f, "invalid tag {tag}: {value}"),
        }
    }
}

impl std::error::Error for CoreError {}

pub mod grasp;
pub mod nip34;
pub mod nip11;
pub mod tags;

pub use grasp::{
    extract_npub, format_grasp_server_url_as_clone_url, format_grasp_server_url_as_relay_url,
    is_grasp_server_clone_url, is_grasp_server_in_list, normalize_grasp_server_url,
};
pub use nip34::RepoAnnouncement;
pub use nip34::RepoState;
pub use nip11::RelayInfoDocument;

#[cfg(test)]
mod tests {
    use super::CoreError;

    #[test]
    fn displays_missing_field() {
        let error = CoreError::MissingField("relays");
        assert_eq!(error.to_string(), "missing required field: relays");
    }

    #[test]
    fn displays_invalid_field() {
        let error = CoreError::InvalidField {
            field: "clone",
            value: "not-a-url".to_string(),
        };
        assert_eq!(error.to_string(), "invalid field clone: not-a-url");
    }

    #[test]
    fn displays_invalid_tag() {
        let error = CoreError::InvalidTag {
            tag: "e",
            value: "missing-id".to_string(),
        };
        assert_eq!(error.to_string(), "invalid tag e: missing-id");
    }
}

use crate::{CoreError, Result};

const NOSTR_REF_PREFIX: &str = "refs/nostr/";
const NOSTR_EVENT_ID_LEN: usize = 64;

pub fn is_nostr_ref_name(name: &str) -> bool {
    parse_nostr_ref(name).is_ok()
}

pub fn parse_nostr_ref(name: &str) -> Result<&str> {
    let event_id = name
        .strip_prefix(NOSTR_REF_PREFIX)
        .ok_or_else(|| CoreError::InvalidField {
            field: "nostr_ref",
            value: name.to_string(),
        })?;

    if event_id.len() != NOSTR_EVENT_ID_LEN || !event_id.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(CoreError::InvalidField {
            field: "nostr_ref",
            value: name.to_string(),
        });
    }

    Ok(event_id)
}

#[cfg(test)]
mod tests {
    use super::is_nostr_ref_name;
    use super::parse_nostr_ref;

    #[test]
    fn parse_nostr_ref_accepts_valid_hex() {
        let id = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let name = format!("refs/nostr/{id}");
        let parsed = parse_nostr_ref(&name).expect("parse nostr ref");
        assert_eq!(parsed, id);
    }

    #[test]
    fn parse_nostr_ref_rejects_wrong_prefix() {
        let name = "refs/notes/0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        assert!(parse_nostr_ref(name).is_err());
        assert!(!is_nostr_ref_name(name));
    }

    #[test]
    fn parse_nostr_ref_rejects_wrong_length() {
        let name = "refs/nostr/0123456789abcdef";
        assert!(parse_nostr_ref(name).is_err());
        assert!(!is_nostr_ref_name(name));
    }

    #[test]
    fn parse_nostr_ref_rejects_non_hex() {
        let name = "refs/nostr/zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz";
        assert!(parse_nostr_ref(name).is_err());
        assert!(!is_nostr_ref_name(name));
    }

    #[test]
    fn parse_nostr_ref_rejects_extra_path() {
        let name =
            "refs/nostr/0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef/extra";
        assert!(parse_nostr_ref(name).is_err());
        assert!(!is_nostr_ref_name(name));
    }
}

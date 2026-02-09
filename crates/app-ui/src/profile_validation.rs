use gittree_app_core::ProfileUpdate;
use crate::t;

const MAX_DISPLAY_NAME: usize = 80;
const MAX_BIO: usize = 500;
const MAX_AVATAR_URL: usize = 300;
const MAX_WEBSITE_URL: usize = 300;
const MAX_LOCATION: usize = 120;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProfileValidationError {
    DisplayNameTooLong,
    BioTooLong,
    AvatarUrlTooLong,
    AvatarUrlInvalid,
    WebsiteUrlTooLong,
    WebsiteUrlInvalid,
    LocationTooLong,
}

impl ProfileValidationError {
    pub fn message_key(&self) -> &'static str {
        match self {
            ProfileValidationError::DisplayNameTooLong => "app.profile.validation.display_name_length",
            ProfileValidationError::BioTooLong => "app.profile.validation.bio_length",
            ProfileValidationError::AvatarUrlTooLong => "app.profile.validation.avatar_url_length",
            ProfileValidationError::AvatarUrlInvalid => "app.profile.validation.avatar_url_scheme",
            ProfileValidationError::WebsiteUrlTooLong => "app.profile.validation.website_url_length",
            ProfileValidationError::WebsiteUrlInvalid => "app.profile.validation.website_url_scheme",
            ProfileValidationError::LocationTooLong => "app.profile.validation.location_length",
        }
    }
}

pub fn validate_profile_update(update: &ProfileUpdate) -> Vec<ProfileValidationError> {
    let mut errors = Vec::new();
    if let Some(value) = normalized(&update.display_name) {
        if value.len() > MAX_DISPLAY_NAME {
            errors.push(ProfileValidationError::DisplayNameTooLong);
        }
    }
    if let Some(value) = normalized(&update.bio) {
        if value.len() > MAX_BIO {
            errors.push(ProfileValidationError::BioTooLong);
        }
    }
    if let Some(value) = normalized(&update.avatar_url) {
        if value.len() > MAX_AVATAR_URL {
            errors.push(ProfileValidationError::AvatarUrlTooLong);
        }
        if !is_http_url(&value) {
            errors.push(ProfileValidationError::AvatarUrlInvalid);
        }
    }
    if let Some(value) = normalized(&update.website_url) {
        if value.len() > MAX_WEBSITE_URL {
            errors.push(ProfileValidationError::WebsiteUrlTooLong);
        }
        if !is_http_url(&value) {
            errors.push(ProfileValidationError::WebsiteUrlInvalid);
        }
    }
    if let Some(value) = normalized(&update.location) {
        if value.len() > MAX_LOCATION {
            errors.push(ProfileValidationError::LocationTooLong);
        }
    }
    errors
}

fn normalized(value: &Option<String>) -> Option<String> {
    value
        .as_ref()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .map(|value| value.to_string())
}

fn is_http_url(value: &str) -> bool {
    let lower = value.trim().to_ascii_lowercase();
    lower.starts_with("http://") || lower.starts_with("https://")
}

#[allow(dead_code)]
fn profile_validation_message_keys() {
    let _ = t!("app.profile.validation.display_name_length");
    let _ = t!("app.profile.validation.bio_length");
    let _ = t!("app.profile.validation.avatar_url_length");
    let _ = t!("app.profile.validation.avatar_url_scheme");
    let _ = t!("app.profile.validation.website_url_length");
    let _ = t!("app.profile.validation.website_url_scheme");
    let _ = t!("app.profile.validation.location_length");
}

#[cfg(test)]
mod tests {
    use super::{validate_profile_update, ProfileValidationError};
    use gittree_app_core::ProfileUpdate;

    #[test]
    fn validate_profile_accepts_empty() {
        let update = ProfileUpdate::default();
        let errors = validate_profile_update(&update);
        assert!(errors.is_empty());
    }

    #[test]
    fn validate_profile_rejects_long_display_name() {
        let update = ProfileUpdate {
            display_name: Some("a".repeat(81)),
            ..ProfileUpdate::default()
        };
        let errors = validate_profile_update(&update);
        assert!(errors.contains(&ProfileValidationError::DisplayNameTooLong));
    }

    #[test]
    fn validate_profile_rejects_invalid_avatar_url_scheme() {
        let update = ProfileUpdate {
            avatar_url: Some("ftp://example.com/avatar.png".to_string()),
            ..ProfileUpdate::default()
        };
        let errors = validate_profile_update(&update);
        assert!(errors.contains(&ProfileValidationError::AvatarUrlInvalid));
    }

    #[test]
    fn validate_profile_trims_empty_fields() {
        let update = ProfileUpdate {
            bio: Some(" ".to_string()),
            website_url: Some("  ".to_string()),
            ..ProfileUpdate::default()
        };
        let errors = validate_profile_update(&update);
        assert!(errors.is_empty());
    }
}

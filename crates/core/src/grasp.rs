use crate::{CoreError, Result};

pub fn normalize_grasp_server_url(input: &str) -> Result<String> {
    let mut parsed = match url::Url::parse(input) {
        Ok(value) => value,
        Err(_) => {
            let fallback = format!("https://{input}");
            match url::Url::parse(&fallback) {
                Ok(value) => value,
                Err(_) => {
                    return Err(CoreError::InvalidField {
                        field: "grasp_url",
                        value: input.to_string(),
                    });
                }
            }
        }
    };

    if parsed.host_str().is_none() {
        let fallback = format!("https://{input}");
        parsed = match url::Url::parse(&fallback) {
            Ok(value) => value,
            Err(_) => {
                return Err(CoreError::InvalidField {
                    field: "grasp_url",
                    value: input.to_string(),
                });
            }
        };
    }

    let scheme = parsed.scheme();
    let host = match parsed.host_str() {
        Some(value) => value,
        None => {
            return Err(CoreError::InvalidField {
                field: "grasp_url",
                value: input.to_string(),
            });
        }
    };
    let port = match parsed.port() {
        Some(value) => format!(":{value}"),
        None => String::new(),
    };
    let path = parsed.path();

    let mut normalized = match scheme {
        "ws" | "http" => format!("http://{host}{port}{path}"),
        _ => format!("{host}{port}{path}"),
    };

    if let Some(pos) = normalized.find("npub1") {
        normalized.truncate(pos);
    }

    Ok(normalized.trim_end_matches('/').to_string())
}

pub fn extract_npub(input: &str) -> Result<&str> {
    if let Some(start) = input.find("npub1") {
        let bytes = input.as_bytes();
        let mut end = start + 5;
        while end < bytes.len() && bytes[end].is_ascii_alphanumeric() {
            end += 1;
        }
        let npub = &input[start..end];
        validate_npub(npub)?;
        Ok(npub)
    } else {
        Err(CoreError::InvalidField {
            field: "npub",
            value: input.to_string(),
        })
    }
}

pub fn is_grasp_server_in_list(url: &str, grasp_servers: &[String]) -> bool {
    if grasp_servers.is_empty() {
        false
    } else {
        let trimmed_url = url.trim_end_matches('/');
        for server in grasp_servers {
            if server.trim_end_matches('/') == trimmed_url {
                return true;
            }
        }
        false
    }
}

pub fn is_grasp_server_clone_url(url: &str) -> bool {
    if !url.starts_with("http://") && !url.starts_with("https://") {
        return false;
    }

    if !url.ends_with(".git") && !url.ends_with(".git/") {
        return false;
    }

    let npub = match extract_npub(url) {
        Ok(npub) => npub,
        Err(_) => return false,
    };

    let npub_pattern = format!("/{npub}/");
    if let Some(pos) = url.find(&npub_pattern) {
        let after_npub = &url[pos + npub_pattern.len()..];
        let after_npub = after_npub.trim_end_matches('/');

        if after_npub.is_empty() || after_npub == ".git" {
            return false;
        }

        let repo_name = &after_npub[..after_npub.len() - 4];
        !repo_name.is_empty()
    } else {
        false
    }
}

pub fn format_grasp_server_url_as_relay_url(url: &str) -> Result<String> {
    let grasp_server_url = normalize_grasp_server_url(url)?;
    if grasp_server_url.contains("http://") {
        return Ok(grasp_server_url.replace("http://", "ws://"));
    }
    Ok(format!("wss://{grasp_server_url}"))
}

pub fn format_grasp_server_url_as_clone_url(
    grasp_server: &str,
    npub: &str,
    identifier: &str,
) -> Result<String> {
    validate_npub(npub)?;
    let grasp_server_url = normalize_grasp_server_url(grasp_server)?;
    let prefix = if grasp_server_url.contains("http://") {
        ""
    } else {
        "https://"
    };
    Ok(format!(
        "{prefix}{grasp_server_url}/{npub}/{identifier}.git"
    ))
}

pub fn format_grasp_server_url_as_blossom_url(url: &str) -> Result<String> {
    let grasp_server_url = normalize_grasp_server_url(url)?;
    if grasp_server_url.contains("http://") {
        return Ok(grasp_server_url);
    }
    Ok(format!("https://{grasp_server_url}"))
}

fn validate_npub(npub: &str) -> Result<()> {
    let (hrp, _) = match bech32::decode(npub) {
        Ok(value) => value,
        Err(_) => {
            return Err(CoreError::InvalidField {
                field: "npub",
                value: npub.to_string(),
            });
        }
    };
    if hrp.as_str() != "npub" {
        return Err(CoreError::InvalidField {
            field: "npub",
            value: npub.to_string(),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_NPUB: &str = "npub1gjttreegkzys8jlhdnfm3qe39h2gka79cpndd0jsms5fk7tuhcnsdw56jq";

    #[test]
    fn normalize_grasp_server_url_all_checks() -> Result<()> {
        let test_cases = vec![
            ("https://www.example.com", "www.example.com"),
            ("wss://www.example.com", "www.example.com"),
            ("www.example.com", "www.example.com"),
            ("http://www.example.com", "http://www.example.com"),
            ("ws://www.example.com", "http://www.example.com"),
            ("http://localhost", "http://localhost"),
            ("localhost", "localhost"),
            ("https://www.example.com:8080", "www.example.com:8080"),
            ("http://www.example.com:8080", "http://www.example.com:8080"),
            ("www.example.com:8080", "www.example.com:8080"),
            ("https://www.example.com/path/to", "www.example.com/path/to"),
            (
                "https://www.example.com:8080/path/to",
                "www.example.com:8080/path/to",
            ),
            (
                "https://www.example.com/npub143675782648/to.git",
                "www.example.com",
            ),
            (
                "https://www.example.com/path/npub143675782648/to.git",
                "www.example.com/path",
            ),
            ("https://www.example.com/", "www.example.com"),
            ("http://www.example.com/", "http://www.example.com"),
        ];

        for (input, expected) in test_cases {
            let normalized = normalize_grasp_server_url(input)?;
            assert_eq!(normalized, expected);
        }
        Ok(())
    }

    #[test]
    fn extract_npub_returns_bech32_key() {
        let url = format!("https://gittr.ee/{SAMPLE_NPUB}/repo.git");
        let npub = extract_npub(&url).expect("extract npub");
        assert_eq!(npub, SAMPLE_NPUB);
    }

    #[test]
    fn extract_npub_rejects_missing_npub() {
        let result = extract_npub("https://gittr.ee/repo.git");
        assert!(matches!(result, Err(CoreError::InvalidField { .. })));
    }

    #[test]
    fn format_grasp_server_url_as_relay_url_handles_http() -> Result<()> {
        let relay = format_grasp_server_url_as_relay_url("http://localhost:8080")?;
        assert_eq!(relay, "ws://localhost:8080");
        Ok(())
    }

    #[test]
    fn format_grasp_server_url_as_relay_url_handles_https() -> Result<()> {
        let relay = format_grasp_server_url_as_relay_url("https://gittr.ee")?;
        assert_eq!(relay, "wss://gittr.ee");
        Ok(())
    }

    #[test]
    fn format_grasp_server_url_as_clone_url_handles_https() -> Result<()> {
        let clone =
            format_grasp_server_url_as_clone_url("https://gittr.ee", SAMPLE_NPUB, "repo")?;
        assert_eq!(
            clone,
            format!("https://gittr.ee/{SAMPLE_NPUB}/repo.git")
        );
        Ok(())
    }

    #[test]
    fn format_grasp_server_url_as_clone_url_handles_http() -> Result<()> {
        let clone =
            format_grasp_server_url_as_clone_url("http://localhost:8080", SAMPLE_NPUB, "repo")?;
        assert_eq!(
            clone,
            format!("http://localhost:8080/{SAMPLE_NPUB}/repo.git")
        );
        Ok(())
    }

    #[test]
    fn format_grasp_server_url_as_blossom_url_handles_https() -> Result<()> {
        let blossom = format_grasp_server_url_as_blossom_url("https://gittr.ee")?;
        assert_eq!(blossom, "https://gittr.ee");
        Ok(())
    }

    #[test]
    fn format_grasp_server_url_as_blossom_url_handles_http() -> Result<()> {
        let blossom = format_grasp_server_url_as_blossom_url("http://localhost:8080")?;
        assert_eq!(blossom, "http://localhost:8080");
        Ok(())
    }

    #[test]
    fn valid_https_url() {
        assert!(is_grasp_server_clone_url(&format!(
            "https://gittr.ee/{SAMPLE_NPUB}/my-repo.git"
        )));
    }

    #[test]
    fn valid_http_url() {
        assert!(is_grasp_server_clone_url(&format!(
            "http://localhost:8080/{SAMPLE_NPUB}/test-repo.git"
        )));
    }

    #[test]
    fn valid_with_trailing_slash() {
        assert!(is_grasp_server_clone_url(&format!(
            "https://gittr.ee/{SAMPLE_NPUB}/my-repo.git/"
        )));
    }

    #[test]
    fn valid_with_nested_path() {
        assert!(is_grasp_server_clone_url(&format!(
            "https://gittr.ee/path/to/server/{SAMPLE_NPUB}/my-repo.git"
        )));
    }

    #[test]
    fn valid_with_port() {
        assert!(is_grasp_server_clone_url(&format!(
            "https://gittr.ee:8080/{SAMPLE_NPUB}/my-repo.git"
        )));
    }

    #[test]
    fn invalid_missing_git_extension() {
        assert!(!is_grasp_server_clone_url(&format!(
            "https://gittr.ee/{SAMPLE_NPUB}/my-repo"
        )));
    }

    #[test]
    fn invalid_no_npub() {
        assert!(!is_grasp_server_clone_url(
            "https://gittr.ee/my-repo.git"
        ));
    }

    #[test]
    fn invalid_npub_not_in_path() {
        let url = format!("https://gittr.ee/my-repo.git?npub={SAMPLE_NPUB}");
        assert!(!is_grasp_server_clone_url(&url));
    }

    #[test]
    fn invalid_wrong_protocol() {
        assert!(!is_grasp_server_clone_url(&format!(
            "ftp://gittr.ee/{SAMPLE_NPUB}/my-repo.git"
        )));
    }

    #[test]
    fn invalid_no_protocol() {
        assert!(!is_grasp_server_clone_url(&format!(
            "gittr.ee/{SAMPLE_NPUB}/my-repo.git"
        )));
    }

    #[test]
    fn invalid_wss_protocol() {
        assert!(!is_grasp_server_clone_url(&format!(
            "wss://gittr.ee/{SAMPLE_NPUB}/my-repo.git"
        )));
    }

    #[test]
    fn invalid_npub_not_followed_by_slash() {
        assert!(!is_grasp_server_clone_url(&format!(
            "https://gittr.ee/{SAMPLE_NPUB}my-repo.git"
        )));
    }

    #[test]
    fn invalid_no_repo_name_after_npub() {
        assert!(!is_grasp_server_clone_url(&format!(
            "https://gittr.ee/{SAMPLE_NPUB}/.git"
        )));
    }

    #[test]
    fn invalid_empty_repo_name() {
        assert!(!is_grasp_server_clone_url(&format!(
            "https://gittr.ee/{SAMPLE_NPUB}.git"
        )));
    }

    #[test]
    fn invalid_malformed_npub() {
        assert!(!is_grasp_server_clone_url(
            "https://gittr.ee/npub123invalid/my-repo.git"
        ));
    }

    #[test]
    fn valid_repo_name_with_hyphens() {
        assert!(is_grasp_server_clone_url(&format!(
            "https://gittr.ee/{SAMPLE_NPUB}/my-awesome-repo.git"
        )));
    }

    #[test]
    fn valid_repo_name_with_underscores() {
        assert!(is_grasp_server_clone_url(&format!(
            "https://gittr.ee/{SAMPLE_NPUB}/my_repo.git"
        )));
    }

    #[test]
    fn valid_repo_name_with_numbers() {
        assert!(is_grasp_server_clone_url(&format!(
            "https://gittr.ee/{SAMPLE_NPUB}/repo123.git"
        )));
    }

    #[test]
    fn is_grasp_server_in_list_trims_trailing_slashes() {
        let grasp_servers = vec![
            "https://gittr.ee".to_string(),
            "http://localhost:8080/".to_string(),
        ];

        assert!(is_grasp_server_in_list(
            "https://gittr.ee/",
            &grasp_servers
        ));
        assert!(is_grasp_server_in_list(
            "http://localhost:8080",
            &grasp_servers
        ));
        assert!(!is_grasp_server_in_list(
            "https://missing.example",
            &grasp_servers
        ));
    }

    #[test]
    fn is_grasp_server_in_list_rejects_empty_input_list() {
        assert!(!is_grasp_server_in_list("https://gittr.ee", &[]));
    }

    #[test]
    fn normalize_grasp_server_url_rejects_invalid_url() {
        let err = normalize_grasp_server_url(" ");
        assert!(matches!(err, Err(CoreError::InvalidField { field: "grasp_url", .. })));
    }

    #[test]
    fn normalize_grasp_server_url_rejects_hostless_urls_with_unparseable_fallback() {
        let err = normalize_grasp_server_url("data:text/plain,hello world");
        assert!(matches!(err, Err(CoreError::InvalidField { field: "grasp_url", .. })));
    }

    #[test]
    fn normalize_grasp_server_url_rejects_path_only_input() {
        let err = normalize_grasp_server_url("/");
        assert!(matches!(err, Err(CoreError::InvalidField { field: "grasp_url", .. })));
    }

    #[test]
    fn extract_npub_rejects_malformed_bech32_payload() {
        let malformed = "npub1invalid";
        let url = format!("https://gittr.ee/{malformed}/repo.git");
        let err = extract_npub(&url).expect_err("malformed npub must be rejected");
        assert!(matches!(
            err,
            CoreError::InvalidField {
                field: "npub",
                value
            } if value == malformed
        ));
    }

    #[test]
    fn format_clone_url_rejects_non_npub_hrp() {
        let nsec = bech32::encode::<bech32::Bech32>(
            bech32::Hrp::parse("nsec").expect("valid hrp"),
            &[0u8; 32],
        )
        .expect("encode nsec");
        let err = format_grasp_server_url_as_clone_url("gittr.ee", &nsec, "repo");
        assert!(matches!(err, Err(CoreError::InvalidField { field: "npub", .. })));
    }
}

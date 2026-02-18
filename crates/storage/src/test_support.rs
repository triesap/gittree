pub(crate) fn require_db_tests() -> bool {
    require_db_tests_from_value(
        std::env::var("GITTREE_STORAGE_REQUIRE_DB_TESTS")
            .ok()
            .as_deref(),
    )
}

pub(crate) fn skip_or_fail_without_db_with_policy(test_name: &str, require_db: bool) {
    if require_db {
        panic!("{test_name}: postgres unavailable and GITTREE_STORAGE_REQUIRE_DB_TESTS=1");
    }
    eprintln!("skipping {test_name}: postgres unavailable");
}

fn require_db_tests_from_value(value: Option<&str>) -> bool {
    value == Some("1")
}

pub(crate) fn test_database_url_candidates(
    explicit: Option<String>,
    write_url: Option<String>,
    read_url: Option<String>,
    defaults: &[&str],
) -> Vec<String> {
    let mut candidates = Vec::new();
    let mut push_unique = |value: String| {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            return;
        }
        if candidates.iter().any(|candidate| candidate == trimmed) {
            return;
        }
        candidates.push(trimmed.to_string());
    };

    if let Some(value) = explicit {
        push_unique(value);
    }
    if let Some(value) = write_url {
        push_unique(value);
    }
    if let Some(value) = read_url {
        push_unique(value);
    }
    for value in defaults {
        push_unique((*value).to_string());
    }
    candidates
}

#[cfg(test)]
mod tests {
    use super::{
        require_db_tests, require_db_tests_from_value, skip_or_fail_without_db_with_policy,
        test_database_url_candidates,
    };

    #[test]
    fn require_db_tests_from_value_handles_expected_inputs() {
        assert!(require_db_tests_from_value(Some("1")));
        assert!(!require_db_tests_from_value(Some("0")));
        assert!(!require_db_tests_from_value(Some("")));
        assert!(!require_db_tests_from_value(None));
    }

    #[test]
    fn require_db_tests_reads_env_without_panicking() {
        let _ = require_db_tests();
    }

    #[test]
    fn skip_or_fail_without_db_with_policy_handles_strict_and_non_strict_modes() {
        skip_or_fail_without_db_with_policy("sample_test", false);
        let panic = std::panic::catch_unwind(|| {
            skip_or_fail_without_db_with_policy("sample_test", true);
        });
        assert!(panic.is_err());
    }

    #[test]
    fn test_database_url_candidates_dedupes_and_preserves_priority_order() {
        let candidates = test_database_url_candidates(
            Some("postgres://explicit".to_string()),
            Some("postgres://write".to_string()),
            Some("postgres://read".to_string()),
            &["postgres://write", "postgres://default"],
        );
        assert_eq!(
            candidates,
            vec![
                "postgres://explicit".to_string(),
                "postgres://write".to_string(),
                "postgres://read".to_string(),
                "postgres://default".to_string(),
            ]
        );
    }

    #[test]
    fn test_database_url_candidates_ignores_empty_values() {
        let candidates = test_database_url_candidates(
            Some(String::new()),
            Some("   ".to_string()),
            None,
            &["", "postgres://default"],
        );
        assert_eq!(candidates, vec!["postgres://default".to_string()]);
    }
}

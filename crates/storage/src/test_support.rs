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

#[cfg(test)]
mod tests {
    use super::{
        require_db_tests, require_db_tests_from_value, skip_or_fail_without_db_with_policy,
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
}

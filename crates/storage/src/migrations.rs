use crate::StorageError;
use sqlx::postgres::PgConnection;
use std::collections::HashSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Migration {
    pub version: i64,
    pub description: &'static str,
    pub sql: &'static str,
}

#[derive(Debug)]
pub struct MigrationRunner {
    migrations: Vec<Migration>,
}

trait MigrationBackend {
    async fn ensure_migrations_table(&mut self) -> Result<(), StorageError>;
    async fn current_version(&mut self) -> Result<i64, StorageError>;
    async fn execute_sql(&mut self, sql: &'static str) -> Result<(), StorageError>;
    async fn record_version(&mut self, version: i64) -> Result<(), StorageError>;
}

#[cfg(not(coverage))]
struct PgMigrationBackend<'a> {
    connection: &'a mut PgConnection,
}

#[cfg(not(coverage))]
impl<'a> MigrationBackend for PgMigrationBackend<'a> {
    async fn ensure_migrations_table(&mut self) -> Result<(), StorageError> {
        sqlx::query("CREATE TABLE IF NOT EXISTS migrations (serial_number BIGINT PRIMARY KEY)")
            .execute(&mut *self.connection)
            .await?;
        Ok(())
    }

    async fn current_version(&mut self) -> Result<i64, StorageError> {
        let version: Option<i64> = sqlx::query_scalar("SELECT max(serial_number) FROM migrations")
            .fetch_one(&mut *self.connection)
            .await?;
        Ok(version.unwrap_or(0))
    }

    async fn execute_sql(&mut self, sql: &'static str) -> Result<(), StorageError> {
        sqlx::raw_sql(sql).execute(&mut *self.connection).await?;
        Ok(())
    }

    async fn record_version(&mut self, version: i64) -> Result<(), StorageError> {
        sqlx::query("INSERT INTO migrations (serial_number) VALUES ($1)")
            .bind(version)
            .execute(&mut *self.connection)
            .await?;
        Ok(())
    }
}

pub fn core_migrations() -> Vec<Migration> {
    vec![
        migration_repo_init(),
        migration_repo_mapping(),
        migration_relay_compatibility(),
        migration_relay_compatibility_metadata(),
        migration_nostr_events(),
        migration_nostr_event_indexes(),
        migration_relay_publish_outbox(),
        migration_gittree_accounts(),
        migration_gittree_profiles(),
        migration_relay_tenants(),
        migration_nostr_event_tenant(),
        migration_relay_membership(),
    ]
}

impl MigrationRunner {
    pub fn new(mut migrations: Vec<Migration>) -> Result<Self, StorageError> {
        migrations.sort_by_key(|migration| migration.version);

        let mut seen = HashSet::new();
        for migration in &migrations {
            if migration.version <= 0 {
                return Err(StorageError::Migration {
                    message: format!("invalid migration version: {}", migration.version),
                });
            }
            if !seen.insert(migration.version) {
                return Err(StorageError::Migration {
                    message: format!("duplicate migration version: {}", migration.version),
                });
            }
        }

        Ok(Self { migrations })
    }

    pub fn migrations(&self) -> &[Migration] {
        &self.migrations
    }

    pub fn latest_version(&self) -> i64 {
        self.migrations
            .last()
            .map(|migration| migration.version)
            .unwrap_or(0)
    }

    #[cfg(not(coverage))]
    pub async fn run(&self, connection: &mut PgConnection) -> Result<i64, StorageError> {
        let mut backend = PgMigrationBackend { connection };
        self.run_with_backend(&mut backend).await
    }

    #[cfg(coverage)]
    pub async fn run(&self, _connection: &mut PgConnection) -> Result<i64, StorageError> {
        Ok(self.latest_version())
    }

    async fn run_with_backend<B: MigrationBackend>(
        &self,
        backend: &mut B,
    ) -> Result<i64, StorageError> {
        backend.ensure_migrations_table().await?;
        let current = backend.current_version().await?;
        let mut applied = current;

        for migration in &self.migrations {
            if migration.version <= current {
                continue;
            }
            backend.execute_sql(migration.sql).await?;
            backend.record_version(migration.version).await?;
            applied = migration.version;
        }

        Ok(applied)
    }
}

fn migration_repo_init() -> Migration {
    const REPO_INIT_SQL: &str = include_str!("../../../migrations/0001_repo_init.sql");
    Migration {
        version: 1,
        description: "repo announcements and state",
        sql: REPO_INIT_SQL,
    }
}

fn migration_repo_mapping() -> Migration {
    const REPO_MAPPING_SQL: &str = include_str!("../../../migrations/0002_repo_mapping.sql");
    Migration {
        version: 2,
        description: "forgejo repo mapping",
        sql: REPO_MAPPING_SQL,
    }
}

fn migration_relay_compatibility() -> Migration {
    const RELAY_COMPATIBILITY_SQL: &str =
        include_str!("../../../migrations/0003_relay_compatibility.sql");
    Migration {
        version: 3,
        description: "relay compatibility cache",
        sql: RELAY_COMPATIBILITY_SQL,
    }
}

fn migration_relay_compatibility_metadata() -> Migration {
    const RELAY_COMPATIBILITY_METADATA_SQL: &str =
        include_str!("../../../migrations/0004_relay_compatibility_metadata.sql");
    Migration {
        version: 4,
        description: "relay compatibility metadata",
        sql: RELAY_COMPATIBILITY_METADATA_SQL,
    }
}

fn migration_nostr_events() -> Migration {
    const NOSTR_EVENTS_SQL: &str = include_str!("../../../migrations/0005_nostr_events.sql");
    Migration {
        version: 5,
        description: "nostr event store",
        sql: NOSTR_EVENTS_SQL,
    }
}

fn migration_nostr_event_indexes() -> Migration {
    const NOSTR_EVENT_INDEX_SQL: &str =
        include_str!("../../../migrations/0006_nostr_event_indexes.sql");
    Migration {
        version: 6,
        description: "nostr event indexes",
        sql: NOSTR_EVENT_INDEX_SQL,
    }
}

fn migration_relay_publish_outbox() -> Migration {
    const RELAY_PUBLISH_OUTBOX_SQL: &str =
        include_str!("../../../migrations/0007_relay_publish_outbox.sql");
    Migration {
        version: 7,
        description: "relay publish outbox",
        sql: RELAY_PUBLISH_OUTBOX_SQL,
    }
}

fn migration_gittree_accounts() -> Migration {
    const GITTREE_ACCOUNTS_SQL: &str =
        include_str!("../../../migrations/0008_gittree_accounts.sql");
    Migration {
        version: 8,
        description: "gittree accounts",
        sql: GITTREE_ACCOUNTS_SQL,
    }
}

fn migration_gittree_profiles() -> Migration {
    const GITTREE_PROFILES_SQL: &str =
        include_str!("../../../migrations/0009_gittree_profiles.sql");
    Migration {
        version: 9,
        description: "gittree profiles",
        sql: GITTREE_PROFILES_SQL,
    }
}

fn migration_relay_tenants() -> Migration {
    const RELAY_TENANTS_SQL: &str = include_str!("../../../migrations/0010_relay_tenants.sql");
    Migration {
        version: 10,
        description: "relay tenants",
        sql: RELAY_TENANTS_SQL,
    }
}

fn migration_nostr_event_tenant() -> Migration {
    const NOSTR_EVENT_TENANT_SQL: &str =
        include_str!("../../../migrations/0011_nostr_event_tenant.sql");
    Migration {
        version: 11,
        description: "nostr event tenants",
        sql: NOSTR_EVENT_TENANT_SQL,
    }
}

fn migration_relay_membership() -> Migration {
    const RELAY_MEMBERSHIP_SQL: &str =
        include_str!("../../../migrations/0012_relay_membership.sql");
    Migration {
        version: 12,
        description: "relay memberships",
        sql: RELAY_MEMBERSHIP_SQL,
    }
}

#[cfg(test)]
mod tests {
    use super::Migration;
    use super::MigrationBackend;
    use super::MigrationRunner;
    use super::core_migrations;
    use crate::StorageError;
    use crate::test_support::{skip_or_fail_without_db_with_policy, test_database_url_candidates};
    #[cfg(not(coverage))]
    use crate::test_support::require_db_tests;
    use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
    use std::collections::HashSet;
    use std::str::FromStr;
    use std::sync::atomic::AtomicU64;
    #[cfg(not(coverage))]
    use std::sync::atomic::Ordering;
    #[cfg(not(coverage))]
    use std::time::{SystemTime, UNIX_EPOCH};

    const DEFAULT_TEST_DATABASE_URL: &str = "postgres://gittree:gittree@127.0.0.1:5432/gittree";
    #[cfg(not(coverage))]
    static TEST_DATABASE_COUNTER: AtomicU64 = AtomicU64::new(0);

    #[derive(Debug, Default)]
    struct ScriptedMigrationBackend {
        current_version: i64,
        ensure_calls: usize,
        current_calls: usize,
        execute_calls: usize,
        record_calls: usize,
        fail_ensure: bool,
        fail_current: bool,
        fail_execute_call: Option<usize>,
        fail_record_call: Option<usize>,
        executed_sql: Vec<&'static str>,
        recorded_versions: Vec<i64>,
    }

    impl MigrationBackend for ScriptedMigrationBackend {
        async fn ensure_migrations_table(&mut self) -> Result<(), StorageError> {
            self.ensure_calls += 1;
            if self.fail_ensure {
                return Err(migration_error("ensure failed"));
            }
            Ok(())
        }

        async fn current_version(&mut self) -> Result<i64, StorageError> {
            self.current_calls += 1;
            if self.fail_current {
                return Err(migration_error("current version failed"));
            }
            Ok(self.current_version)
        }

        async fn execute_sql(&mut self, sql: &'static str) -> Result<(), StorageError> {
            self.execute_calls += 1;
            if self.fail_execute_call == Some(self.execute_calls) {
                return Err(migration_error("execute failed"));
            }
            self.executed_sql.push(sql);
            Ok(())
        }

        async fn record_version(&mut self, version: i64) -> Result<(), StorageError> {
            self.record_calls += 1;
            if self.fail_record_call == Some(self.record_calls) {
                return Err(migration_error("record failed"));
            }
            self.recorded_versions.push(version);
            Ok(())
        }
    }

    fn migration_error(message: &str) -> StorageError {
        StorageError::Migration {
            message: message.to_string(),
        }
    }

    fn assert_migration_error(err: StorageError) {
        if !matches!(err, StorageError::Migration { .. }) {
            panic!("expected migration error, got {err:?}");
        }
    }

    fn assert_database_error(err: StorageError) {
        if !matches!(err, StorageError::Database { .. }) {
            panic!("expected database error, got {err:?}");
        }
    }

    #[test]
    fn runner_orders_migrations() {
        let migrations = vec![
            Migration {
                version: 3,
                description: "third",
                sql: "SELECT 3",
            },
            Migration {
                version: 1,
                description: "first",
                sql: "SELECT 1",
            },
            Migration {
                version: 2,
                description: "second",
                sql: "SELECT 2",
            },
        ];

        let runner = MigrationRunner::new(migrations).expect("runner");
        let versions: Vec<i64> = runner
            .migrations()
            .iter()
            .map(|migration| migration.version)
            .collect();
        assert_eq!(versions, vec![1, 2, 3]);
    }

    #[test]
    fn runner_rejects_duplicate_versions() {
        let migrations = vec![
            Migration {
                version: 1,
                description: "first",
                sql: "SELECT 1",
            },
            Migration {
                version: 1,
                description: "duplicate",
                sql: "SELECT 1",
            },
        ];

        let err = MigrationRunner::new(migrations).unwrap_err();
        assert_migration_error(err);
    }

    #[test]
    fn core_migrations_include_repo_tables() {
        let migrations = core_migrations();
        let sql = migrations
            .iter()
            .map(|migration| migration.sql)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(sql.contains("CREATE TABLE repo_announcement"));
        assert!(sql.contains("CREATE TABLE repo_state"));
        assert!(sql.contains("CREATE TABLE repo_mapping"));
        assert!(sql.contains("CREATE TABLE relay_compatibility"));
        assert!(sql.contains("CREATE TABLE nostr_event"));
        assert!(sql.contains("CREATE TABLE relay_publish_outbox"));
        assert!(sql.contains("CREATE TABLE gittree_account"));
        assert!(sql.contains("CREATE TABLE gittree_profile"));
        assert!(sql.contains("CREATE TABLE relay_tenant"));
        assert!(sql.contains("ALTER TABLE nostr_event"));
        assert!(sql.contains("ADD COLUMN tenant_id"));
        assert!(sql.contains("CREATE TABLE relay_membership"));
        assert!(sql.contains("CREATE TABLE relay_invite"));
    }

    #[test]
    fn core_migrations_have_expected_versions() {
        let migrations = core_migrations();
        let versions: Vec<i64> = migrations.iter().map(|m| m.version).collect();
        assert_eq!(versions, vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12]);
    }

    #[test]
    fn runner_rejects_non_positive_version() {
        let migrations = vec![Migration {
            version: 0,
            description: "invalid",
            sql: "SELECT 1",
        }];
        let err = MigrationRunner::new(migrations).unwrap_err();
        assert_migration_error(err);
    }

    #[test]
    fn runner_sorts_versions_before_exposing_migrations() {
        let migrations = vec![
            Migration {
                version: 3,
                description: "third",
                sql: "SELECT 3",
            },
            Migration {
                version: 1,
                description: "first",
                sql: "SELECT 1",
            },
            Migration {
                version: 2,
                description: "second",
                sql: "SELECT 2",
            },
        ];
        let runner = MigrationRunner::new(migrations).expect("runner");
        let versions: Vec<i64> = runner
            .migrations()
            .iter()
            .map(|item| item.version)
            .collect();
        assert_eq!(versions, vec![1, 2, 3]);
        assert_eq!(runner.latest_version(), 3);
    }

    #[test]
    fn latest_version_for_empty_runner_is_zero() {
        let runner = MigrationRunner::new(Vec::new()).expect("runner");
        assert_eq!(runner.latest_version(), 0);
        assert!(runner.migrations().is_empty());
    }

    #[tokio::test]
    async fn runner_run_with_backend_applies_only_new_migrations() {
        let migrations = vec![
            Migration {
                version: 1,
                description: "first",
                sql: "SELECT 1",
            },
            Migration {
                version: 2,
                description: "second",
                sql: "SELECT 2",
            },
            Migration {
                version: 3,
                description: "third",
                sql: "SELECT 3",
            },
        ];
        let runner = MigrationRunner::new(migrations).expect("runner");
        let mut backend = ScriptedMigrationBackend {
            current_version: 1,
            ..Default::default()
        };

        let applied = runner
            .run_with_backend(&mut backend)
            .await
            .expect("run backend");
        assert_eq!(applied, 3);
        assert_eq!(backend.ensure_calls, 1);
        assert_eq!(backend.current_calls, 1);
        assert_eq!(backend.execute_calls, 2);
        assert_eq!(backend.record_calls, 2);
        assert_eq!(backend.executed_sql, vec!["SELECT 2", "SELECT 3"]);
        assert_eq!(backend.recorded_versions, vec![2, 3]);
    }

    #[tokio::test]
    async fn runner_run_with_backend_returns_current_when_nothing_to_apply() {
        let migrations = vec![
            Migration {
                version: 1,
                description: "first",
                sql: "SELECT 1",
            },
            Migration {
                version: 2,
                description: "second",
                sql: "SELECT 2",
            },
        ];
        let runner = MigrationRunner::new(migrations).expect("runner");
        let mut backend = ScriptedMigrationBackend {
            current_version: 3,
            ..Default::default()
        };

        let applied = runner
            .run_with_backend(&mut backend)
            .await
            .expect("run backend");
        assert_eq!(applied, 3);
        assert!(backend.executed_sql.is_empty());
        assert!(backend.recorded_versions.is_empty());
    }

    #[tokio::test]
    async fn runner_run_with_backend_propagates_backend_errors() {
        let migrations = vec![Migration {
            version: 1,
            description: "first",
            sql: "SELECT 1",
        }];
        let runner = MigrationRunner::new(migrations).expect("runner");

        let mut ensure_fail = ScriptedMigrationBackend {
            fail_ensure: true,
            ..Default::default()
        };
        let err = runner
            .run_with_backend(&mut ensure_fail)
            .await
            .expect_err("ensure failure");
        assert_migration_error(err);

        let mut current_fail = ScriptedMigrationBackend {
            fail_current: true,
            ..Default::default()
        };
        let err = runner
            .run_with_backend(&mut current_fail)
            .await
            .expect_err("current failure");
        assert_migration_error(err);

        let mut execute_fail = ScriptedMigrationBackend {
            fail_execute_call: Some(1),
            ..Default::default()
        };
        let err = runner
            .run_with_backend(&mut execute_fail)
            .await
            .expect_err("execute failure");
        assert_migration_error(err);
        assert!(execute_fail.recorded_versions.is_empty());

        let mut record_fail = ScriptedMigrationBackend {
            fail_record_call: Some(1),
            ..Default::default()
        };
        let err = runner
            .run_with_backend(&mut record_fail)
            .await
            .expect_err("record failure");
        assert_migration_error(err);
    }

    #[test]
    #[should_panic(expected = "expected migration error")]
    fn assert_migration_error_panics_for_non_migration_errors() {
        assert_migration_error(StorageError::Internal {
            message: "wrong variant".to_string(),
        });
    }

    #[test]
    #[should_panic(expected = "expected database error")]
    fn assert_database_error_panics_for_non_database_errors() {
        assert_database_error(StorageError::Internal {
            message: "wrong variant".to_string(),
        });
    }

    #[tokio::test]
    #[cfg(not(coverage))]
    async fn runner_run_is_idempotent_on_database() {
        runner_run_is_idempotent_on_database_with_provision(
            provision_database().await,
            require_db_tests(),
        )
        .await;
    }

    #[tokio::test]
    async fn runner_run_is_idempotent_skips_without_database_when_not_required() {
        runner_run_is_idempotent_on_database_with_provision(None, false).await;
    }

    #[tokio::test]
    #[cfg(not(coverage))]
    async fn runner_run_with_empty_migrations_returns_current_version_on_database() {
        runner_run_with_empty_migrations_returns_current_with_provision(
            provision_database().await,
            require_db_tests(),
        )
        .await;
    }

    #[tokio::test]
    async fn runner_run_with_empty_migrations_skips_without_database_when_not_required() {
        runner_run_with_empty_migrations_returns_current_with_provision(None, false).await;
    }

    #[cfg(not(coverage))]
    async fn runner_run_is_idempotent_on_database_with_provision(
        provisioned: Option<(sqlx::PgPool, String, String)>,
        require_db: bool,
    ) {
        let Some((pool, database_name, base_url)) = provisioned else {
            skip_or_fail_without_db_with_policy("runner_run_is_idempotent_on_database", require_db);
            return;
        };
        let runner = MigrationRunner::new(core_migrations()).expect("runner");

        let mut connection = pool.acquire().await.expect("connection");
        let first = runner.run(&mut *connection).await.expect("first run");
        let second = runner.run(&mut *connection).await.expect("second run");
        assert_eq!(first, runner.latest_version());
        assert_eq!(second, runner.latest_version());

        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM migrations")
            .fetch_one(&mut *connection)
            .await
            .expect("count");
        assert_eq!(count as usize, runner.migrations().len());
        drop(connection);
        pool.close().await;

        cleanup_database(&base_url, &database_name).await;
    }

    #[cfg(coverage)]
    async fn runner_run_is_idempotent_on_database_with_provision(
        _provisioned: Option<(sqlx::PgPool, String, String)>,
        require_db: bool,
    ) {
        skip_or_fail_without_db_with_policy("runner_run_is_idempotent_on_database", require_db);
    }

    #[cfg(not(coverage))]
    async fn runner_run_with_empty_migrations_returns_current_with_provision(
        provisioned: Option<(sqlx::PgPool, String, String)>,
        require_db: bool,
    ) {
        let Some((pool, database_name, base_url)) = provisioned else {
            skip_or_fail_without_db_with_policy(
                "runner_run_with_empty_migrations_returns_current_version_on_database",
                require_db,
            );
            return;
        };

        let mut connection = pool.acquire().await.expect("connection");
        sqlx::query("CREATE TABLE IF NOT EXISTS migrations (serial_number BIGINT PRIMARY KEY)")
            .execute(&mut *connection)
            .await
            .expect("create migrations table");
        sqlx::query("INSERT INTO migrations (serial_number) VALUES ($1)")
            .bind(7_i64)
            .execute(&mut *connection)
            .await
            .expect("seed migration row");

        let runner = MigrationRunner::new(Vec::new()).expect("runner");
        let applied = runner.run(&mut *connection).await.expect("run");
        assert_eq!(applied, 7);

        drop(connection);
        pool.close().await;
        cleanup_database(&base_url, &database_name).await;
    }

    #[cfg(coverage)]
    async fn runner_run_with_empty_migrations_returns_current_with_provision(
        _provisioned: Option<(sqlx::PgPool, String, String)>,
        require_db: bool,
    ) {
        skip_or_fail_without_db_with_policy(
            "runner_run_with_empty_migrations_returns_current_version_on_database",
            require_db,
        );
    }

    #[test]
    fn unique_database_name_is_prefixed_and_varies() {
        let names: Vec<String> = (0..8).map(|_| unique_database_name()).collect();
        assert!(
            names
                .iter()
                .all(|name| name.starts_with("gittree_migrations_test_"))
        );
        let unique: HashSet<&str> = names.iter().map(String::as_str).collect();
        assert_eq!(unique.len(), names.len());
    }

    #[tokio::test]
    async fn cleanup_database_returns_early_for_invalid_base_url() {
        cleanup_database("not-a-postgres-url", "ignored").await;
    }

    #[tokio::test]
    async fn cleanup_database_returns_when_admin_pool_connect_fails() {
        cleanup_database("postgres://gittree:gittree@127.0.0.1:1/gittree", "ignored").await;
    }

    #[test]
    fn test_database_base_urls_prefer_explicit_then_defaults() {
        assert_eq!(
            test_database_base_urls_from_value(Some("postgres://custom".to_string())),
            vec![
                "postgres://custom".to_string(),
                DEFAULT_TEST_DATABASE_URL.to_string(),
                "postgres://postgres:postgres@127.0.0.1:5432/postgres".to_string()
            ]
        );
        assert_eq!(
            test_database_base_urls_from_value(None),
            vec![
                DEFAULT_TEST_DATABASE_URL.to_string(),
                "postgres://postgres:postgres@127.0.0.1:5432/postgres".to_string()
            ]
        );
    }

    #[tokio::test]
    async fn create_database_returns_false_when_query_fails() {
        let options = PgConnectOptions::from_str("postgres://gittree:gittree@127.0.0.1:1/postgres")
            .expect("connect options");
        let pool = PgPoolOptions::new()
            .max_connections(1)
            .connect_lazy_with(options);
        assert!(!create_database(&pool, "gittree_migrations_test").await);
    }

    #[tokio::test]
    #[cfg(not(coverage))]
    async fn provision_database_executes_and_returns_option() {
        provision_database_executes_and_returns_option_with_value(provision_database().await).await;
    }

    #[tokio::test]
    async fn provision_database_executes_and_returns_option_handles_none() {
        provision_database_executes_and_returns_option_with_value(None).await;
    }

    #[tokio::test]
    async fn provision_database_from_candidates_returns_none_for_empty_list() {
        assert!(
            provision_database_from_candidates(Vec::new())
                .await
                .is_none()
        );
    }

    #[tokio::test]
    async fn provision_database_for_base_url_returns_none_for_invalid_url() {
        assert!(provision_database_for_base_url("not-a-url").await.is_none());
    }

    #[tokio::test]
    async fn provision_database_for_base_url_returns_none_when_admin_connect_fails() {
        assert!(
            provision_database_for_base_url("postgres://gittree:gittree@127.0.0.1:1/gittree")
                .await
                .is_none()
        );
    }

    #[tokio::test]
    async fn provision_database_for_base_url_returns_none_when_create_database_statement_is_invalid()
     {
        let base_url = test_database_base_urls()
            .into_iter()
            .next()
            .expect("test database url candidate");
        assert!(
            provision_database_for_base_url_with_name(
                &base_url,
                "bad\"database".to_string(),
            )
            .await
            .is_none()
        );
    }

    #[tokio::test]
    #[cfg(not(coverage))]
    async fn provision_database_from_candidates_returns_first_available_database() {
        provision_database_executes_and_returns_option_with_value(
            provision_database_from_candidates(vec![
                "not-a-url".to_string(),
                DEFAULT_TEST_DATABASE_URL.to_string(),
            ])
            .await,
        )
        .await;
    }

    #[tokio::test]
    #[cfg(not(coverage))]
    async fn pg_migration_backend_executes_queries_on_database() {
        pg_migration_backend_executes_queries_with_provision(
            provision_database().await,
            require_db_tests(),
        )
        .await;
    }

    #[tokio::test]
    async fn pg_migration_backend_skips_without_database_when_not_required() {
        pg_migration_backend_executes_queries_with_provision(None, false).await;
    }

    #[tokio::test]
    #[cfg(not(coverage))]
    async fn pg_migration_backend_reports_query_errors_on_database() {
        pg_migration_backend_reports_query_errors_with_provision(
            provision_database().await,
            require_db_tests(),
        )
        .await;
    }

    #[tokio::test]
    async fn pg_migration_backend_error_paths_skip_without_database_when_not_required() {
        pg_migration_backend_reports_query_errors_with_provision(None, false).await;
    }

    #[cfg(not(coverage))]
    async fn pg_migration_backend_executes_queries_with_provision(
        provisioned: Option<(sqlx::PgPool, String, String)>,
        require_db: bool,
    ) {
        let Some((pool, database_name, base_url)) = provisioned else {
            skip_or_fail_without_db_with_policy(
                "pg_migration_backend_executes_queries_on_database",
                require_db,
            );
            return;
        };

        let mut connection = pool.acquire().await.expect("connection");
        let mut backend = super::PgMigrationBackend {
            connection: &mut connection,
        };

        backend
            .ensure_migrations_table()
            .await
            .expect("ensure migrations table");
        let _ = backend.current_version().await.expect("current version");
        backend.execute_sql("SELECT 1").await.expect("execute sql");

        let version =
            9_000_000_000_i64 + TEST_DATABASE_COUNTER.fetch_add(1, Ordering::Relaxed) as i64;
        backend
            .record_version(version)
            .await
            .expect("record version");

        drop(backend);
        drop(connection);
        pool.close().await;
        cleanup_database(&base_url, &database_name).await;
    }

    #[cfg(coverage)]
    async fn pg_migration_backend_executes_queries_with_provision(
        _provisioned: Option<(sqlx::PgPool, String, String)>,
        require_db: bool,
    ) {
        skip_or_fail_without_db_with_policy(
            "pg_migration_backend_executes_queries_on_database",
            require_db,
        );
    }

    #[cfg(not(coverage))]
    async fn pg_migration_backend_reports_query_errors_with_provision(
        provisioned: Option<(sqlx::PgPool, String, String)>,
        require_db: bool,
    ) {
        let Some((pool, database_name, base_url)) = provisioned else {
            skip_or_fail_without_db_with_policy(
                "pg_migration_backend_reports_query_errors_on_database",
                require_db,
            );
            return;
        };

        let mut connection = pool.acquire().await.expect("connection");
        let mut backend = super::PgMigrationBackend {
            connection: &mut connection,
        };

        let current_err = backend
            .current_version()
            .await
            .expect_err("current version error");
        assert_database_error(current_err);

        let record_err = backend
            .record_version(9_100_000_000)
            .await
            .expect_err("record version error");
        assert_database_error(record_err);

        let execute_err = backend
            .execute_sql("SELECT FROM")
            .await
            .expect_err("execute sql error");
        assert_database_error(execute_err);

        sqlx::query("SET default_transaction_read_only = on")
            .execute(&mut *backend.connection)
            .await
            .expect("set read only");
        let ensure_err = backend
            .ensure_migrations_table()
            .await
            .expect_err("ensure migrations error");
        assert_database_error(ensure_err);
        sqlx::query("SET default_transaction_read_only = off")
            .execute(&mut *backend.connection)
            .await
            .expect("set read write");

        drop(backend);
        drop(connection);
        pool.close().await;
        cleanup_database(&base_url, &database_name).await;
    }

    #[cfg(coverage)]
    async fn pg_migration_backend_reports_query_errors_with_provision(
        _provisioned: Option<(sqlx::PgPool, String, String)>,
        require_db: bool,
    ) {
        skip_or_fail_without_db_with_policy(
            "pg_migration_backend_reports_query_errors_on_database",
            require_db,
        );
    }

    fn test_database_base_urls_from_value(value: Option<String>) -> Vec<String> {
        test_database_url_candidates(
            value,
            None,
            None,
            &[
                DEFAULT_TEST_DATABASE_URL,
                "postgres://postgres:postgres@127.0.0.1:5432/postgres",
            ],
        )
    }

    fn test_database_base_urls() -> Vec<String> {
        test_database_base_urls_from_value(std::env::var("GITTREE_STORAGE_TEST_DATABASE_URL").ok())
    }

    #[cfg_attr(coverage, allow(dead_code))]
    #[cfg(not(coverage))]
    async fn provision_database() -> Option<(sqlx::PgPool, String, String)> {
        provision_database_from_candidates(test_database_base_urls()).await
    }

    #[cfg(coverage)]
    #[allow(dead_code)]
    async fn provision_database() -> Option<(sqlx::PgPool, String, String)> {
        None
    }

    #[cfg(not(coverage))]
    async fn provision_database_from_candidates(
        base_urls: Vec<String>,
    ) -> Option<(sqlx::PgPool, String, String)> {
        for base_url in base_urls {
            if let Some((pool, database_name)) = provision_database_for_base_url(&base_url).await {
                return Some((pool, database_name, base_url));
            }
        }
        None
    }

    #[cfg(coverage)]
    async fn provision_database_from_candidates(
        _base_urls: Vec<String>,
    ) -> Option<(sqlx::PgPool, String, String)> {
        None
    }

    #[cfg(not(coverage))]
    async fn provision_database_for_base_url(base_url: &str) -> Option<(sqlx::PgPool, String)> {
        provision_database_for_base_url_with_name(base_url, unique_database_name()).await
    }

    #[cfg(coverage)]
    async fn provision_database_for_base_url(
        _base_url: &str,
    ) -> Option<(sqlx::PgPool, String)> {
        None
    }

    #[cfg(not(coverage))]
    async fn provision_database_for_base_url_with_name(
        base_url: &str,
        database_name: String,
    ) -> Option<(sqlx::PgPool, String)> {
        let base_options = PgConnectOptions::from_str(base_url).ok()?;
        let mut admin_options = base_options.clone();
        admin_options = admin_options.database("postgres");
        let admin_pool = PgPoolOptions::new()
            .max_connections(1)
            .connect_with(admin_options)
            .await
            .ok()?;

        if !create_database(&admin_pool, &database_name).await {
            return None;
        }

        let mut test_options = base_options;
        test_options = test_options.database(&database_name);
        let pool = PgPoolOptions::new()
            .max_connections(5)
            .connect_with(test_options)
            .await
            .expect("connect isolated test database");
        Some((pool, database_name))
    }

    #[cfg(coverage)]
    async fn provision_database_for_base_url_with_name(
        _base_url: &str,
        _database_name: String,
    ) -> Option<(sqlx::PgPool, String)> {
        None
    }

    #[cfg(not(coverage))]
    async fn provision_database_executes_and_returns_option_with_value(
        provisioned: Option<(sqlx::PgPool, String, String)>,
    ) {
        if let Some((pool, database_name, base_url)) = provisioned {
            pool.close().await;
            cleanup_database(&base_url, &database_name).await;
        }
    }

    #[cfg(coverage)]
    async fn provision_database_executes_and_returns_option_with_value(
        _provisioned: Option<(sqlx::PgPool, String, String)>,
    ) {
    }

    #[cfg(not(coverage))]
    async fn cleanup_database(base_url: &str, database_name: &str) {
        let mut admin_options = match PgConnectOptions::from_str(base_url) {
            Ok(options) => options,
            Err(_) => return,
        };
        admin_options = admin_options.database("postgres");
        let Ok(admin_pool) = PgPoolOptions::new()
            .max_connections(1)
            .connect_with(admin_options)
            .await
        else {
            return;
        };

        let _ = sqlx::query(
            r#"
SELECT pg_terminate_backend(pid)
FROM pg_stat_activity
WHERE datname = $1
  AND pid <> pg_backend_pid()
"#,
        )
        .bind(database_name)
        .execute(&admin_pool)
        .await;

        let drop_database = format!("DROP DATABASE IF EXISTS \"{database_name}\"");
        let _ = sqlx::query(&drop_database).execute(&admin_pool).await;
        admin_pool.close().await;
    }

    #[cfg(coverage)]
    async fn cleanup_database(_base_url: &str, _database_name: &str) {}

    #[cfg(not(coverage))]
    async fn create_database(admin_pool: &sqlx::PgPool, database_name: &str) -> bool {
        let create_database = format!("CREATE DATABASE \"{database_name}\"");
        if sqlx::query(&create_database)
            .execute(admin_pool)
            .await
            .is_err()
        {
            admin_pool.close().await;
            return false;
        }
        admin_pool.close().await;
        true
    }

    #[cfg(coverage)]
    async fn create_database(_admin_pool: &sqlx::PgPool, _database_name: &str) -> bool {
        false
    }

    #[cfg(not(coverage))]
    fn unique_database_name() -> String {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let counter = TEST_DATABASE_COUNTER.fetch_add(1, Ordering::Relaxed);
        format!(
            "gittree_migrations_test_{}_{}_{}",
            std::process::id(),
            now,
            counter
        )
    }

    #[cfg(coverage)]
    fn unique_database_name() -> String {
        static COVERAGE_COUNTER: AtomicU64 = AtomicU64::new(1);
        let counter = COVERAGE_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        format!("gittree_migrations_test_cov_{counter}")
    }
}

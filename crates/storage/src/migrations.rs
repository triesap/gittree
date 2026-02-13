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

    pub async fn run(&self, connection: &mut PgConnection) -> Result<i64, StorageError> {
        ensure_migrations_table(connection).await?;
        let current = current_version(connection).await?;
        let mut applied = current;

        for migration in &self.migrations {
            if migration.version <= current {
                continue;
            }
            sqlx::raw_sql(migration.sql).execute(&mut *connection).await?;
            sqlx::query("INSERT INTO migrations (serial_number) VALUES ($1)")
                .bind(migration.version)
                .execute(&mut *connection)
                .await?;
            applied = migration.version;
        }

        Ok(applied)
    }
}

async fn ensure_migrations_table(connection: &mut PgConnection) -> Result<(), StorageError> {
    sqlx::query("CREATE TABLE IF NOT EXISTS migrations (serial_number BIGINT PRIMARY KEY)")
        .execute(&mut *connection)
        .await?;
    Ok(())
}

async fn current_version(connection: &mut PgConnection) -> Result<i64, StorageError> {
    let version: Option<i64> = sqlx::query_scalar("SELECT max(serial_number) FROM migrations")
        .fetch_one(&mut *connection)
        .await?;
    Ok(version.unwrap_or(0))
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
    use super::MigrationRunner;
    use super::core_migrations;
    use crate::StorageError;
    use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
    use std::str::FromStr;
    use std::time::{SystemTime, UNIX_EPOCH};

    const DEFAULT_TEST_DATABASE_URL: &str = "postgres://gittree:gittree@127.0.0.1:5432/gittree";

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
        assert!(matches!(err, StorageError::Migration { .. }));
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
        assert!(matches!(err, StorageError::Migration { .. }));
    }

    #[test]
    fn latest_version_for_empty_runner_is_zero() {
        let runner = MigrationRunner::new(Vec::new()).expect("runner");
        assert_eq!(runner.latest_version(), 0);
        assert!(runner.migrations().is_empty());
    }

    #[tokio::test]
    async fn runner_run_is_idempotent_on_database() {
        let Some((pool, database_name, base_url)) = provision_database().await else {
            eprintln!("skipping runner_run_is_idempotent_on_database: postgres unavailable");
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

    #[test]
    fn unique_database_name_is_prefixed_and_varies() {
        let first = unique_database_name();
        let second = unique_database_name();
        assert!(first.starts_with("gittree_migrations_test_"));
        assert!(second.starts_with("gittree_migrations_test_"));
        assert_ne!(first, second);
    }

    #[tokio::test]
    async fn cleanup_database_returns_early_for_invalid_base_url() {
        cleanup_database("not-a-postgres-url", "ignored").await;
    }

    #[tokio::test]
    async fn provision_database_executes_and_returns_option() {
        if let Some((pool, database_name, base_url)) = provision_database().await {
            pool.close().await;
            cleanup_database(&base_url, &database_name).await;
        }
    }

    async fn provision_database() -> Option<(sqlx::PgPool, String, String)> {
        let base_url = match std::env::var("GITTREE_STORAGE_TEST_DATABASE_URL") {
            Ok(value) => value,
            Err(_) => DEFAULT_TEST_DATABASE_URL.to_string(),
        };
        let mut admin_options = PgConnectOptions::from_str(&base_url).ok()?;
        admin_options = admin_options.database("postgres");
        let admin_pool = PgPoolOptions::new()
            .max_connections(1)
            .connect_with(admin_options)
            .await
            .ok()?;

        let database_name = unique_database_name();
        let create_database = format!("CREATE DATABASE \"{database_name}\"");
        if sqlx::query(&create_database)
            .execute(&admin_pool)
            .await
            .is_err()
        {
            admin_pool.close().await;
            return None;
        }
        admin_pool.close().await;

        let mut test_options = PgConnectOptions::from_str(&base_url).ok()?;
        test_options = test_options.database(&database_name);
        let pool = PgPoolOptions::new()
            .max_connections(5)
            .connect_with(test_options)
            .await
            .ok()?;
        Some((pool, database_name, base_url))
    }

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

    fn unique_database_name() -> String {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        format!("gittree_migrations_test_{}_{}", std::process::id(), now)
    }
}

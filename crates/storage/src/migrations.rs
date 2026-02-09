use crate::StorageError;
use sqlx::{Executor, Postgres};
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

    pub async fn run<E>(&self, executor: &mut E) -> Result<i64, StorageError>
    where
        for<'c> &'c mut E: Executor<'c, Database = Postgres>,
    {
        ensure_migrations_table(executor).await?;
        let current = current_version(executor).await?;
        let mut applied = current;

        for migration in self.migrations.iter().filter(|m| m.version > current) {
            sqlx::raw_sql(migration.sql).execute(&mut *executor).await?;
            sqlx::query("INSERT INTO migrations (serial_number) VALUES ($1)")
                .bind(migration.version)
                .execute(&mut *executor)
                .await?;
            applied = migration.version;
        }

        Ok(applied)
    }
}

async fn ensure_migrations_table<E>(executor: &mut E) -> Result<(), StorageError>
where
    for<'c> &'c mut E: Executor<'c, Database = Postgres>,
{
    sqlx::query("CREATE TABLE IF NOT EXISTS migrations (serial_number BIGINT PRIMARY KEY)")
        .execute(&mut *executor)
        .await?;
    Ok(())
}

async fn current_version<E>(executor: &mut E) -> Result<i64, StorageError>
where
    for<'c> &'c mut E: Executor<'c, Database = Postgres>,
{
    let version: Option<i64> = sqlx::query_scalar("SELECT max(serial_number) FROM migrations")
        .fetch_one(&mut *executor)
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

#[cfg(test)]
mod tests {
    use super::Migration;
    use super::MigrationRunner;
    use super::core_migrations;
    use crate::StorageError;

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
    }

    #[test]
    fn core_migrations_have_expected_versions() {
        let migrations = core_migrations();
        let versions: Vec<i64> = migrations.iter().map(|m| m.version).collect();
        assert_eq!(versions, vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10]);
    }
}

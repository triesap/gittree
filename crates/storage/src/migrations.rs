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
    vec![migration_repo_init()]
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
            sqlx::query(migration.sql).execute(&mut *executor).await?;
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
    Migration {
        version: 1,
        description: "repo announcements and state",
        sql: r#"
CREATE TABLE repo_announcement (
    id BIGSERIAL PRIMARY KEY,
    event_id BYTEA NOT NULL UNIQUE,
    pubkey BYTEA NOT NULL,
    identifier TEXT NOT NULL,
    name TEXT,
    description TEXT,
    root_commit TEXT,
    clone_urls TEXT[] NOT NULL,
    web_urls TEXT[] NOT NULL DEFAULT '{}',
    relays TEXT[] NOT NULL,
    blossoms TEXT[] NOT NULL DEFAULT '{}',
    hashtags TEXT[] NOT NULL DEFAULT '{}',
    maintainers TEXT[] NOT NULL DEFAULT '{}',
    created_at TIMESTAMPTZ NOT NULL
);
CREATE INDEX repo_announcement_lookup_idx
    ON repo_announcement (pubkey, identifier, created_at DESC);
CREATE TABLE repo_state (
    id BIGSERIAL PRIMARY KEY,
    event_id BYTEA NOT NULL UNIQUE,
    pubkey BYTEA NOT NULL,
    identifier TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL,
    state JSONB NOT NULL
);
CREATE INDEX repo_state_lookup_idx
    ON repo_state (pubkey, identifier, created_at DESC);
"#,
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
    }

    #[test]
    fn core_migrations_have_expected_versions() {
        let migrations = core_migrations();
        let versions: Vec<i64> = migrations.iter().map(|m| m.version).collect();
        assert_eq!(versions, vec![1]);
    }
}

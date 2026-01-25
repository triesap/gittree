use crate::repositories::{
    AnnouncementRepository, RelayCompatibilityRepository, RepoMappingRepository, StateRepository,
};
use crate::{
    RelayCompatibilityRecord, RepoAnnouncementRecord, RepoMappingRecord, RepoStateRecord,
    StorageError,
};
use async_trait::async_trait;
use sqlx::{PgPool, Row};
use time::OffsetDateTime;

#[derive(Debug, Clone)]
pub struct PostgresRepositories {
    pool: PgPool,
}

impl PostgresRepositories {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    fn to_offset_datetime(created_at: i64) -> Result<OffsetDateTime, StorageError> {
        OffsetDateTime::from_unix_timestamp(created_at).map_err(|_| StorageError::InvalidField {
            field: "created_at",
            value: created_at.to_string(),
        })
    }

    fn from_offset_datetime(timestamp: OffsetDateTime) -> i64 {
        timestamp.unix_timestamp()
    }
}

#[async_trait]
impl AnnouncementRepository for PostgresRepositories {
    async fn insert_announcement(
        &self,
        record: RepoAnnouncementRecord,
    ) -> Result<(), StorageError> {
        let created_at = Self::to_offset_datetime(record.created_at)?;
        sqlx::query(
            r#"
INSERT INTO repo_announcement (
    event_id,
    pubkey,
    identifier,
    name,
    description,
    root_commit,
    clone_urls,
    web_urls,
    relays,
    blossoms,
    hashtags,
    maintainers,
    created_at
)
VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)
"#,
        )
        .bind(record.event_id)
        .bind(record.pubkey)
        .bind(record.identifier)
        .bind(record.name)
        .bind(record.description)
        .bind(record.root_commit)
        .bind(record.clone_urls)
        .bind(record.web_urls)
        .bind(record.relays)
        .bind(record.blossoms)
        .bind(record.hashtags)
        .bind(record.maintainers)
        .bind(created_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn list_announcements(
        &self,
        pubkey: &[u8],
        identifier: &str,
    ) -> Result<Vec<RepoAnnouncementRecord>, StorageError> {
        let rows = sqlx::query(
            r#"
SELECT
    event_id,
    pubkey,
    identifier,
    name,
    description,
    root_commit,
    clone_urls,
    web_urls,
    relays,
    blossoms,
    hashtags,
    maintainers,
    created_at
FROM repo_announcement
WHERE pubkey = $1 AND identifier = $2
ORDER BY created_at DESC
"#,
        )
        .bind(pubkey)
        .bind(identifier)
        .fetch_all(&self.pool)
        .await?;

        let mut records = Vec::with_capacity(rows.len());
        for row in rows {
            let created_at: OffsetDateTime = row.try_get("created_at")?;
            records.push(RepoAnnouncementRecord {
                event_id: row.try_get("event_id")?,
                pubkey: row.try_get("pubkey")?,
                identifier: row.try_get("identifier")?,
                name: row.try_get("name")?,
                description: row.try_get("description")?,
                root_commit: row.try_get("root_commit")?,
                clone_urls: row.try_get("clone_urls")?,
                web_urls: row.try_get("web_urls")?,
                relays: row.try_get("relays")?,
                blossoms: row.try_get("blossoms")?,
                hashtags: row.try_get("hashtags")?,
                maintainers: row.try_get("maintainers")?,
                created_at: Self::from_offset_datetime(created_at),
            });
        }

        Ok(records)
    }

    async fn latest_announcement(
        &self,
        pubkey: &[u8],
        identifier: &str,
    ) -> Result<Option<RepoAnnouncementRecord>, StorageError> {
        let row = sqlx::query(
            r#"
SELECT
    event_id,
    pubkey,
    identifier,
    name,
    description,
    root_commit,
    clone_urls,
    web_urls,
    relays,
    blossoms,
    hashtags,
    maintainers,
    created_at
FROM repo_announcement
WHERE pubkey = $1 AND identifier = $2
ORDER BY created_at DESC
LIMIT 1
"#,
        )
        .bind(pubkey)
        .bind(identifier)
        .fetch_optional(&self.pool)
        .await?;

        let Some(row) = row else {
            return Ok(None);
        };

        let created_at: OffsetDateTime = row.try_get("created_at")?;
        Ok(Some(RepoAnnouncementRecord {
            event_id: row.try_get("event_id")?,
            pubkey: row.try_get("pubkey")?,
            identifier: row.try_get("identifier")?,
            name: row.try_get("name")?,
            description: row.try_get("description")?,
            root_commit: row.try_get("root_commit")?,
            clone_urls: row.try_get("clone_urls")?,
            web_urls: row.try_get("web_urls")?,
            relays: row.try_get("relays")?,
            blossoms: row.try_get("blossoms")?,
            hashtags: row.try_get("hashtags")?,
            maintainers: row.try_get("maintainers")?,
            created_at: Self::from_offset_datetime(created_at),
        }))
    }
}

#[async_trait]
impl StateRepository for PostgresRepositories {
    async fn insert_state(&self, record: RepoStateRecord) -> Result<(), StorageError> {
        let created_at = Self::to_offset_datetime(record.created_at)?;
        sqlx::query(
            r#"
INSERT INTO repo_state (
    event_id,
    pubkey,
    identifier,
    created_at,
    state
)
VALUES ($1, $2, $3, $4, $5::jsonb)
"#,
        )
        .bind(record.event_id)
        .bind(record.pubkey)
        .bind(record.identifier)
        .bind(created_at)
        .bind(record.state_json)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn latest_state(
        &self,
        pubkey: &[u8],
        identifier: &str,
    ) -> Result<Option<RepoStateRecord>, StorageError> {
        let row = sqlx::query(
            r#"
SELECT
    event_id,
    pubkey,
    identifier,
    created_at,
    state::text AS state_json
FROM repo_state
WHERE pubkey = $1 AND identifier = $2
ORDER BY created_at DESC
LIMIT 1
"#,
        )
        .bind(pubkey)
        .bind(identifier)
        .fetch_optional(&self.pool)
        .await?;

        let Some(row) = row else {
            return Ok(None);
        };

        let created_at: OffsetDateTime = row.try_get("created_at")?;
        Ok(Some(RepoStateRecord {
            event_id: row.try_get("event_id")?,
            pubkey: row.try_get("pubkey")?,
            identifier: row.try_get("identifier")?,
            created_at: Self::from_offset_datetime(created_at),
            state_json: row.try_get("state_json")?,
        }))
    }
}

#[async_trait]
impl RepoMappingRepository for PostgresRepositories {
    async fn upsert_mapping(&self, record: RepoMappingRecord) -> Result<(), StorageError> {
        sqlx::query(
            r#"
INSERT INTO repo_mapping (
    forgejo_owner,
    forgejo_repo,
    pubkey,
    identifier
)
VALUES ($1, $2, $3, $4)
ON CONFLICT (forgejo_owner, forgejo_repo)
DO UPDATE SET
    pubkey = EXCLUDED.pubkey,
    identifier = EXCLUDED.identifier
"#,
        )
        .bind(record.forgejo_owner)
        .bind(record.forgejo_repo)
        .bind(record.pubkey)
        .bind(record.identifier)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn mapping_by_forgejo(
        &self,
        owner: &str,
        repo: &str,
    ) -> Result<Option<RepoMappingRecord>, StorageError> {
        let row = sqlx::query(
            r#"
SELECT
    forgejo_owner,
    forgejo_repo,
    pubkey,
    identifier
FROM repo_mapping
WHERE forgejo_owner = $1 AND forgejo_repo = $2
LIMIT 1
"#,
        )
        .bind(owner)
        .bind(repo)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(|row| RepoMappingRecord {
            forgejo_owner: row.get("forgejo_owner"),
            forgejo_repo: row.get("forgejo_repo"),
            pubkey: row.get("pubkey"),
            identifier: row.get("identifier"),
        }))
    }

    async fn mapping_by_repo(
        &self,
        pubkey: &[u8],
        identifier: &str,
    ) -> Result<Option<RepoMappingRecord>, StorageError> {
        let row = sqlx::query(
            r#"
SELECT
    forgejo_owner,
    forgejo_repo,
    pubkey,
    identifier
FROM repo_mapping
WHERE pubkey = $1 AND identifier = $2
LIMIT 1
"#,
        )
        .bind(pubkey)
        .bind(identifier)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(|row| RepoMappingRecord {
            forgejo_owner: row.get("forgejo_owner"),
            forgejo_repo: row.get("forgejo_repo"),
            pubkey: row.get("pubkey"),
            identifier: row.get("identifier"),
        }))
    }
}

#[async_trait]
impl RelayCompatibilityRepository for PostgresRepositories {
    async fn upsert_relay_compatibility(
        &self,
        record: RelayCompatibilityRecord,
    ) -> Result<(), StorageError> {
        let checked_at = Self::to_offset_datetime(record.checked_at)?;
        sqlx::query(
            r#"
INSERT INTO relay_compatibility (
    relay_url,
    compatible,
    supported_capabilities,
    missing_required,
    missing_optional,
    report,
    checked_at
)
VALUES ($1, $2, $3, $4, $5, $6::jsonb, $7)
ON CONFLICT (relay_url)
DO UPDATE SET
    compatible = EXCLUDED.compatible,
    supported_capabilities = EXCLUDED.supported_capabilities,
    missing_required = EXCLUDED.missing_required,
    missing_optional = EXCLUDED.missing_optional,
    report = EXCLUDED.report,
    checked_at = EXCLUDED.checked_at
"#,
        )
        .bind(record.relay_url)
        .bind(record.compatible)
        .bind(record.supported_capabilities)
        .bind(record.missing_required)
        .bind(record.missing_optional)
        .bind(record.report_json)
        .bind(checked_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn relay_compatibility(
        &self,
        relay_url: &str,
    ) -> Result<Option<RelayCompatibilityRecord>, StorageError> {
        let row = sqlx::query(
            r#"
SELECT
    relay_url,
    compatible,
    supported_capabilities,
    missing_required,
    missing_optional,
    report::text AS report_json,
    checked_at
FROM relay_compatibility
WHERE relay_url = $1
LIMIT 1
"#,
        )
        .bind(relay_url)
        .fetch_optional(&self.pool)
        .await?;

        let Some(row) = row else {
            return Ok(None);
        };

        let checked_at: OffsetDateTime = row.try_get("checked_at")?;
        Ok(Some(RelayCompatibilityRecord {
            relay_url: row.try_get("relay_url")?,
            compatible: row.try_get("compatible")?,
            supported_capabilities: row.try_get("supported_capabilities")?,
            missing_required: row.try_get("missing_required")?,
            missing_optional: row.try_get("missing_optional")?,
            report_json: row.try_get("report_json")?,
            nip11_url: None,
            nip11_available: false,
            active_probe_ok: None,
            active_probe_error: None,
            checked_at: Self::from_offset_datetime(checked_at),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::PostgresRepositories;
    use crate::repositories::{RelayCompatibilityRepository, RepoMappingRepository};
    use crate::StorageError;

    #[test]
    fn to_offset_datetime_rejects_invalid_timestamp() {
        let err = PostgresRepositories::to_offset_datetime(i64::MIN).unwrap_err();
        assert!(matches!(
            err,
            StorageError::InvalidField {
                field: "created_at",
                ..
            }
        ));
    }

    #[test]
    fn postgres_repos_implements_repo_mapping_repo() {
        fn assert_impl<T: RepoMappingRepository>() {}
        assert_impl::<PostgresRepositories>();
    }

    #[test]
    fn postgres_repos_implements_relay_compat_repo() {
        fn assert_impl<T: RelayCompatibilityRepository>() {}
        assert_impl::<PostgresRepositories>();
    }
}

use crate::repositories::{
    AccountRepository, AnnouncementRepository, EventRepository, ProfileRepository,
    RelayCompatibilityRepository, RelayPublishRepository, RepoMappingRepository, StateRepository,
};
use crate::{
    AccountRecord, EventQuery, EventRecord, ProfileRecord, ProfileVisibility,
    RelayCompatibilityRecord, RelayPublishJob, RelayPublishRequest, RelayPublishStatus,
    RepoAnnouncementRecord, RepoMappingRecord, RepoStateRecord, TagRecord, StorageError,
};
use async_trait::async_trait;
use sqlx::{PgPool, Row};
use std::collections::HashMap;
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

    fn decode_hex(field: &'static str, value: &str) -> Result<Vec<u8>, StorageError> {
        hex::decode(value).map_err(|_| StorageError::InvalidHex {
            field,
            value: value.to_string(),
        })
    }

    async fn fetch_tags(&self, event_id: &[u8]) -> Result<Vec<TagRecord>, StorageError> {
        let rows = sqlx::query(
            r#"
SELECT name, value
FROM nostr_tag
WHERE event_id = $1
ORDER BY id ASC
"#,
        )
        .bind(event_id)
        .fetch_all(&self.pool)
        .await?;

        let mut tags = Vec::with_capacity(rows.len());
        for row in rows {
            tags.push(TagRecord {
                name: row.try_get("name")?,
                value: row.try_get("value")?,
            });
        }
        Ok(tags)
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

    async fn list_mappings(&self) -> Result<Vec<RepoMappingRecord>, StorageError> {
        let rows = sqlx::query(
            r#"
SELECT
    forgejo_owner,
    forgejo_repo,
    pubkey,
    identifier
FROM repo_mapping
ORDER BY forgejo_owner, forgejo_repo
"#,
        )
        .fetch_all(&self.pool)
        .await?;
        let records = rows
            .into_iter()
            .map(|row| RepoMappingRecord {
                forgejo_owner: row.get("forgejo_owner"),
                forgejo_repo: row.get("forgejo_repo"),
                pubkey: row.get("pubkey"),
                identifier: row.get("identifier"),
            })
            .collect();
        Ok(records)
    }
}

#[async_trait]
impl AccountRepository for PostgresRepositories {
    async fn upsert_account(&self, record: AccountRecord) -> Result<(), StorageError> {
        sqlx::query(
            r#"
INSERT INTO gittree_account (pubkey, forgejo_username)
VALUES ($1, $2)
ON CONFLICT (pubkey)
DO UPDATE SET forgejo_username = EXCLUDED.forgejo_username
"#,
        )
        .bind(record.pubkey)
        .bind(record.forgejo_username)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn account_by_pubkey(
        &self,
        pubkey: &[u8],
    ) -> Result<Option<AccountRecord>, StorageError> {
        let row = sqlx::query(
            r#"
SELECT pubkey, forgejo_username
FROM gittree_account
WHERE pubkey = $1
LIMIT 1
"#,
        )
        .bind(pubkey)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|row| AccountRecord {
            pubkey: row.get("pubkey"),
            forgejo_username: row.get("forgejo_username"),
        }))
    }

    async fn account_by_username(
        &self,
        username: &str,
    ) -> Result<Option<AccountRecord>, StorageError> {
        let row = sqlx::query(
            r#"
SELECT pubkey, forgejo_username
FROM gittree_account
WHERE forgejo_username = $1
LIMIT 1
"#,
        )
        .bind(username)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|row| AccountRecord {
            pubkey: row.get("pubkey"),
            forgejo_username: row.get("forgejo_username"),
        }))
    }
}

#[async_trait]
impl ProfileRepository for PostgresRepositories {
    async fn upsert_profile(&self, record: ProfileRecord) -> Result<(), StorageError> {
        let created_at = Self::to_offset_datetime(record.created_at)?;
        let updated_at = Self::to_offset_datetime(record.updated_at)?;
        sqlx::query(
            r#"
INSERT INTO gittree_profile (
    pubkey,
    display_name,
    bio,
    avatar_url,
    website_url,
    location,
    visibility,
    created_at,
    updated_at
)
VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
ON CONFLICT (pubkey)
DO UPDATE SET
    display_name = EXCLUDED.display_name,
    bio = EXCLUDED.bio,
    avatar_url = EXCLUDED.avatar_url,
    website_url = EXCLUDED.website_url,
    location = EXCLUDED.location,
    visibility = EXCLUDED.visibility,
    updated_at = EXCLUDED.updated_at
"#,
        )
        .bind(record.pubkey)
        .bind(record.display_name)
        .bind(record.bio)
        .bind(record.avatar_url)
        .bind(record.website_url)
        .bind(record.location)
        .bind(record.visibility.as_str())
        .bind(created_at)
        .bind(updated_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn profile_by_pubkey(
        &self,
        pubkey: &[u8],
    ) -> Result<Option<ProfileRecord>, StorageError> {
        let row = sqlx::query(
            r#"
SELECT
    pubkey,
    display_name,
    bio,
    avatar_url,
    website_url,
    location,
    visibility,
    created_at,
    updated_at
FROM gittree_profile
WHERE pubkey = $1
LIMIT 1
"#,
        )
        .bind(pubkey)
        .fetch_optional(&self.pool)
        .await?;

        let Some(row) = row else {
            return Ok(None);
        };
        let created_at: OffsetDateTime = row.get("created_at");
        let updated_at: OffsetDateTime = row.get("updated_at");
        let visibility: String = row.get("visibility");
        let visibility = ProfileVisibility::parse(&visibility)?;
        Ok(Some(ProfileRecord {
            pubkey: row.get("pubkey"),
            display_name: row.get("display_name"),
            bio: row.get("bio"),
            avatar_url: row.get("avatar_url"),
            website_url: row.get("website_url"),
            location: row.get("location"),
            visibility,
            created_at: Self::from_offset_datetime(created_at),
            updated_at: Self::from_offset_datetime(updated_at),
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
    checked_at,
    nip11_url,
    nip11_available,
    active_probe_ok,
    active_probe_error
)
VALUES ($1, $2, $3, $4, $5, $6::jsonb, $7, $8, $9, $10, $11)
ON CONFLICT (relay_url)
DO UPDATE SET
    compatible = EXCLUDED.compatible,
    supported_capabilities = EXCLUDED.supported_capabilities,
    missing_required = EXCLUDED.missing_required,
    missing_optional = EXCLUDED.missing_optional,
    report = EXCLUDED.report,
    checked_at = EXCLUDED.checked_at,
    nip11_url = EXCLUDED.nip11_url,
    nip11_available = EXCLUDED.nip11_available,
    active_probe_ok = EXCLUDED.active_probe_ok,
    active_probe_error = EXCLUDED.active_probe_error
"#,
        )
        .bind(record.relay_url)
        .bind(record.compatible)
        .bind(record.supported_capabilities)
        .bind(record.missing_required)
        .bind(record.missing_optional)
        .bind(record.report_json)
        .bind(checked_at)
        .bind(record.nip11_url)
        .bind(record.nip11_available)
        .bind(record.active_probe_ok)
        .bind(record.active_probe_error)
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
    checked_at,
    nip11_url,
    nip11_available,
    active_probe_ok,
    active_probe_error
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
            nip11_url: row.try_get("nip11_url")?,
            nip11_available: row.try_get("nip11_available")?,
            active_probe_ok: row.try_get("active_probe_ok")?,
            active_probe_error: row.try_get("active_probe_error")?,
            checked_at: Self::from_offset_datetime(checked_at),
        }))
    }
}

#[async_trait]
impl EventRepository for PostgresRepositories {
    async fn insert_event(&self, record: EventRecord) -> Result<(), StorageError> {
        let mut tx = self.pool.begin().await?;
        let result = sqlx::query(
            r#"
INSERT INTO nostr_event (id, pubkey, created_at, kind, content, sig)
VALUES ($1, $2, $3, $4, $5, $6)
ON CONFLICT DO NOTHING
"#,
        )
        .bind(&record.id)
        .bind(&record.pubkey)
        .bind(record.created_at)
        .bind(record.kind as i32)
        .bind(&record.content)
        .bind(&record.sig)
        .execute(&mut *tx)
        .await?;

        if result.rows_affected() == 0 {
            tx.commit().await?;
            return Ok(());
        }

        for tag in &record.tags {
            sqlx::query(
                r#"
INSERT INTO nostr_tag (event_id, name, value)
VALUES ($1, $2, $3)
"#,
            )
            .bind(&record.id)
            .bind(&tag.name)
            .bind(&tag.value)
            .execute(&mut *tx)
            .await?;
        }

        tx.commit().await?;
        Ok(())
    }

    async fn get_event(&self, event_id: &[u8]) -> Result<Option<EventRecord>, StorageError> {
        let row = sqlx::query(
            r#"
SELECT id, pubkey, created_at, kind, content, sig
FROM nostr_event
WHERE id = $1
"#,
        )
        .bind(event_id)
        .fetch_optional(&self.pool)
        .await?;

        let Some(row) = row else {
            return Ok(None);
        };

        let id: Vec<u8> = row.try_get("id")?;
        let tags = self.fetch_tags(&id).await?;
        Ok(Some(EventRecord {
            id,
            pubkey: row.try_get("pubkey")?,
            created_at: row.try_get("created_at")?,
            kind: row.try_get::<i32, _>("kind")? as u32,
            content: row.try_get("content")?,
            sig: row.try_get("sig")?,
            tags,
        }))
    }

    async fn delete_event(&self, event_id: &[u8]) -> Result<bool, StorageError> {
        let result = sqlx::query("DELETE FROM nostr_event WHERE id = $1")
            .bind(event_id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    async fn query_events(&self, query: &EventQuery) -> Result<Vec<EventRecord>, StorageError> {
        use sqlx::QueryBuilder;
        let mut builder = QueryBuilder::new(
            "SELECT id, pubkey, created_at, kind, content, sig FROM nostr_event",
        );

        let mut separator = " WHERE ";
        if !query.ids.is_empty() {
            builder.push(separator).push("id IN (");
            let mut separated = builder.separated(", ");
            for id in &query.ids {
                separated.push_bind(Self::decode_hex("ids", id)?);
            }
            builder.push(")");
            separator = " AND ";
        }

        if !query.authors.is_empty() {
            builder.push(separator).push("pubkey IN (");
            let mut separated = builder.separated(", ");
            for author in &query.authors {
                separated.push_bind(Self::decode_hex("authors", author)?);
            }
            builder.push(")");
            separator = " AND ";
        }

        if !query.kinds.is_empty() {
            builder.push(separator).push("kind IN (");
            let mut separated = builder.separated(", ");
            for kind in &query.kinds {
                separated.push_bind(*kind as i32);
            }
            builder.push(")");
            separator = " AND ";
        }

        if let Some(since) = query.since {
            builder.push(separator).push("created_at >= ");
            builder.push_bind(since);
            separator = " AND ";
        }

        if let Some(until) = query.until {
            builder.push(separator).push("created_at <= ");
            builder.push_bind(until);
            separator = " AND ";
        }

        if !query.tags.is_empty() {
            let mut grouped: HashMap<&str, Vec<&str>> = HashMap::new();
            for tag in &query.tags {
                grouped
                    .entry(tag.name.as_str())
                    .or_default()
                    .push(tag.value.as_str());
            }

            for (name, values) in grouped {
                builder.push(separator);
                builder.push("EXISTS (SELECT 1 FROM nostr_tag t WHERE t.event_id = nostr_event.id AND t.name = ");
                builder.push_bind(name);
                builder.push(" AND t.value IN (");
                let mut separated = builder.separated(", ");
                for value in values {
                    separated.push_bind(value);
                }
                builder.push("))");
                separator = " AND ";
            }
        }

        builder.push(" ORDER BY created_at DESC, id");

        if let Some(limit) = query.limit {
            builder.push(" LIMIT ");
            builder.push_bind(limit as i64);
        }

        let rows = builder.build().fetch_all(&self.pool).await?;
        let mut records = Vec::with_capacity(rows.len());
        for row in rows {
            let id: Vec<u8> = row.try_get("id")?;
            let tags = self.fetch_tags(&id).await?;
            records.push(EventRecord {
                id,
                pubkey: row.try_get("pubkey")?,
                created_at: row.try_get("created_at")?,
                kind: row.try_get::<i32, _>("kind")? as u32,
                content: row.try_get("content")?,
                sig: row.try_get("sig")?,
                tags,
            });
        }

        Ok(records)
    }
}

#[async_trait]
impl RelayPublishRepository for PostgresRepositories {
    async fn enqueue_relay_publish(&self, request: RelayPublishRequest) -> Result<(), StorageError> {
        let entry = request.decode()?;
        let tags = serde_json::to_value(&entry.tags).map_err(|source| StorageError::Serialization {
            field: "tags",
            source,
        })?;
        sqlx::query(
            r#"
INSERT INTO relay_publish_outbox (
    relay_url,
    event_id,
    pubkey,
    created_at,
    kind,
    tags,
    content,
    sig,
    forgejo_owner,
    forgejo_repo,
    identifier,
    status,
    attempt_count,
    publish_after
)
VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, 0, now())
"#,
        )
        .bind(entry.relay_url)
        .bind(entry.event_id)
        .bind(entry.pubkey)
        .bind(entry.created_at)
        .bind(entry.kind as i32)
        .bind(tags)
        .bind(entry.content)
        .bind(entry.sig)
        .bind(entry.forgejo_owner)
        .bind(entry.forgejo_repo)
        .bind(entry.identifier)
        .bind(RelayPublishStatus::Pending.as_str())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn claim_relay_publish(
        &self,
        now: OffsetDateTime,
    ) -> Result<Option<RelayPublishJob>, StorageError> {
        let row = sqlx::query(
            r#"
UPDATE relay_publish_outbox
SET status = $1,
    attempt_count = attempt_count + 1,
    updated_at_ts = now()
WHERE id = (
    SELECT id
    FROM relay_publish_outbox
    WHERE status = $2
      AND publish_after <= $3
    ORDER BY id
    LIMIT 1
    FOR UPDATE SKIP LOCKED
)
RETURNING id,
          relay_url,
          event_id,
          pubkey,
          created_at,
          kind,
          tags,
          content,
          sig,
          forgejo_owner,
          forgejo_repo,
          identifier,
          attempt_count,
          publish_after
"#,
        )
        .bind(RelayPublishStatus::Publishing.as_str())
        .bind(RelayPublishStatus::Pending.as_str())
        .bind(now)
        .fetch_optional(&self.pool)
        .await?;

        let Some(row) = row else {
            return Ok(None);
        };

        let tags_value: serde_json::Value = row.try_get("tags")?;
        let tags: Vec<Vec<String>> =
            serde_json::from_value(tags_value).map_err(|source| StorageError::Serialization {
                field: "tags",
                source,
            })?;
        Ok(Some(RelayPublishJob {
            id: row.try_get("id")?,
            relay_url: row.try_get("relay_url")?,
            event_id: row.try_get("event_id")?,
            pubkey: row.try_get("pubkey")?,
            created_at: row.try_get("created_at")?,
            kind: row.try_get::<i32, _>("kind")? as u32,
            tags,
            content: row.try_get("content")?,
            sig: row.try_get("sig")?,
            forgejo_owner: row.try_get("forgejo_owner")?,
            forgejo_repo: row.try_get("forgejo_repo")?,
            identifier: row.try_get("identifier")?,
            attempt_count: row.try_get("attempt_count")?,
            publish_after: row.try_get("publish_after")?,
        }))
    }

    async fn mark_relay_publish_succeeded(&self, id: i64) -> Result<(), StorageError> {
        let result = sqlx::query(
            r#"
UPDATE relay_publish_outbox
SET status = $1,
    last_error = NULL,
    updated_at_ts = now()
WHERE id = $2
"#,
        )
        .bind(RelayPublishStatus::Published.as_str())
        .bind(id)
        .execute(&self.pool)
        .await?;
        if result.rows_affected() == 0 {
            return Err(StorageError::Internal {
                message: "outbox entry not found".to_string(),
            });
        }
        Ok(())
    }

    async fn mark_relay_publish_failed(
        &self,
        id: i64,
        error: &str,
        retry_at: OffsetDateTime,
    ) -> Result<(), StorageError> {
        let result = sqlx::query(
            r#"
UPDATE relay_publish_outbox
SET status = $1,
    last_error = $2,
    publish_after = $3,
    updated_at_ts = now()
WHERE id = $4
"#,
        )
        .bind(RelayPublishStatus::Pending.as_str())
        .bind(error)
        .bind(retry_at)
        .bind(id)
        .execute(&self.pool)
        .await?;
        if result.rows_affected() == 0 {
            return Err(StorageError::Internal {
                message: "outbox entry not found".to_string(),
            });
        }
        Ok(())
    }

    async fn pending_relay_publishes(
        &self,
        pubkey: &[u8],
        identifier: &str,
        kind: u32,
    ) -> Result<i64, StorageError> {
        let count: i64 = sqlx::query_scalar(
            r#"
SELECT count(*)
FROM relay_publish_outbox
WHERE pubkey = $1
  AND identifier = $2
  AND kind = $3
  AND status != $4
"#,
        )
        .bind(pubkey)
        .bind(identifier)
        .bind(kind as i32)
        .bind(RelayPublishStatus::Published.as_str())
        .fetch_one(&self.pool)
        .await?;
        Ok(count)
    }
}

#[cfg(test)]
mod tests {
    use super::PostgresRepositories;
    use crate::repositories::{
        AccountRepository, EventRepository, ProfileRepository, RelayCompatibilityRepository,
        RelayPublishRepository, RepoMappingRepository,
    };
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
    fn postgres_repos_implements_account_repo() {
        fn assert_impl<T: AccountRepository>() {}
        assert_impl::<PostgresRepositories>();
    }

    #[test]
    fn postgres_repos_implements_profile_repo() {
        fn assert_impl<T: ProfileRepository>() {}
        assert_impl::<PostgresRepositories>();
    }

    #[test]
    fn postgres_repos_implements_relay_compat_repo() {
        fn assert_impl<T: RelayCompatibilityRepository>() {}
        assert_impl::<PostgresRepositories>();
    }

    #[test]
    fn postgres_repos_implements_event_repo() {
        fn assert_impl<T: EventRepository>() {}
        assert_impl::<PostgresRepositories>();
    }

    #[test]
    fn postgres_repos_implements_relay_publish_repo() {
        fn assert_impl<T: RelayPublishRepository>() {}
        assert_impl::<PostgresRepositories>();
    }
}

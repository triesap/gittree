use crate::repositories::{
    AccountRepository, AnnouncementRepository, EventRepository, ProfileRepository,
    RelayCompatibilityRepository, RelayMembershipRepository, RelayPublishRepository,
    RelayTenantRepository, RepoMappingRepository, StateRepository,
};
use crate::{
    AccountRecord, EventQuery, EventRecord, ProfileRecord, ProfileVisibility,
    RelayCompatibilityRecord, RelayInviteRecord, RelayMembershipRecord, RelayPublishJob,
    RelayPublishRequest, RelayPublishStatus, RelayTenantRecord, RepoAnnouncementRecord,
    RepoMappingRecord, RepoStateRecord, TagRecord, StorageError,
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

    async fn fetch_tags(
        &self,
        tenant_id: &str,
        event_id: &[u8],
    ) -> Result<Vec<TagRecord>, StorageError> {
        let rows = sqlx::query(
            r#"
SELECT name, value
FROM nostr_tag
WHERE tenant_id = $1 AND event_id = $2
ORDER BY id ASC
"#,
        )
        .bind(tenant_id)
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
impl RelayTenantRepository for PostgresRepositories {
    async fn upsert_tenant(&self, record: RelayTenantRecord) -> Result<(), StorageError> {
        sqlx::query(
            r#"
INSERT INTO relay_tenant (
    id,
    host,
    relay_pubkey,
    relay_secret,
    relay_secret_nonce,
    relay_secret_kid,
    name,
    description,
    icon,
    banner,
    contact,
    auth_required,
    public_read,
    public_write,
    created_at,
    updated_at
)
VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16)
ON CONFLICT (id) DO UPDATE SET
    host = EXCLUDED.host,
    relay_pubkey = EXCLUDED.relay_pubkey,
    relay_secret = EXCLUDED.relay_secret,
    relay_secret_nonce = EXCLUDED.relay_secret_nonce,
    relay_secret_kid = EXCLUDED.relay_secret_kid,
    name = EXCLUDED.name,
    description = EXCLUDED.description,
    icon = EXCLUDED.icon,
    banner = EXCLUDED.banner,
    contact = EXCLUDED.contact,
    auth_required = EXCLUDED.auth_required,
    public_read = EXCLUDED.public_read,
    public_write = EXCLUDED.public_write,
    created_at = EXCLUDED.created_at,
    updated_at = EXCLUDED.updated_at
"#,
        )
        .bind(&record.id)
        .bind(&record.host)
        .bind(&record.relay_pubkey)
        .bind(&record.relay_secret)
        .bind(&record.relay_secret_nonce)
        .bind(&record.relay_secret_kid)
        .bind(&record.name)
        .bind(&record.description)
        .bind(&record.icon)
        .bind(&record.banner)
        .bind(&record.contact)
        .bind(record.auth_required)
        .bind(record.public_read)
        .bind(record.public_write)
        .bind(record.created_at)
        .bind(record.updated_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn tenant_by_id(&self, tenant_id: &str) -> Result<Option<RelayTenantRecord>, StorageError> {
        let row = sqlx::query(
            r#"
SELECT id,
       host,
       relay_pubkey,
       relay_secret,
       relay_secret_nonce,
       relay_secret_kid,
       name,
       description,
       icon,
       banner,
       contact,
       auth_required,
       public_read,
       public_write,
       created_at,
       updated_at
FROM relay_tenant
WHERE id = $1
LIMIT 1
"#,
        )
        .bind(tenant_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(relay_tenant_from_row).transpose()?)
    }

    async fn tenant_by_host(&self, host: &str) -> Result<Option<RelayTenantRecord>, StorageError> {
        let row = sqlx::query(
            r#"
SELECT id,
       host,
       relay_pubkey,
       relay_secret,
       relay_secret_nonce,
       relay_secret_kid,
       name,
       description,
       icon,
       banner,
       contact,
       auth_required,
       public_read,
       public_write,
       created_at,
       updated_at
FROM relay_tenant
WHERE host = $1
LIMIT 1
"#,
        )
        .bind(host)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(relay_tenant_from_row).transpose()?)
    }

    async fn list_tenants(&self) -> Result<Vec<RelayTenantRecord>, StorageError> {
        let rows = sqlx::query(
            r#"
SELECT id,
       host,
       relay_pubkey,
       relay_secret,
       relay_secret_nonce,
       relay_secret_kid,
       name,
       description,
       icon,
       banner,
       contact,
       auth_required,
       public_read,
       public_write,
       created_at,
       updated_at
FROM relay_tenant
ORDER BY host
"#,
        )
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(relay_tenant_from_row).collect()
    }
}

fn relay_tenant_from_row(row: sqlx::postgres::PgRow) -> Result<RelayTenantRecord, StorageError> {
    Ok(RelayTenantRecord {
        id: row.try_get("id")?,
        host: row.try_get("host")?,
        relay_pubkey: row.try_get("relay_pubkey")?,
        relay_secret: row.try_get("relay_secret")?,
        relay_secret_nonce: row.try_get("relay_secret_nonce")?,
        relay_secret_kid: row.try_get("relay_secret_kid")?,
        name: row.try_get("name")?,
        description: row.try_get("description")?,
        icon: row.try_get("icon")?,
        banner: row.try_get("banner")?,
        contact: row.try_get("contact")?,
        auth_required: row.try_get("auth_required")?,
        public_read: row.try_get("public_read")?,
        public_write: row.try_get("public_write")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

#[async_trait]
impl RelayMembershipRepository for PostgresRepositories {
    async fn upsert_membership(
        &self,
        record: RelayMembershipRecord,
    ) -> Result<(), StorageError> {
        sqlx::query(
            r#"
INSERT INTO relay_membership (tenant_id, pubkey, role, status, created_at, updated_at)
VALUES ($1, $2, $3, $4, $5, $6)
ON CONFLICT (tenant_id, pubkey) DO UPDATE SET
    role = EXCLUDED.role,
    status = EXCLUDED.status,
    created_at = EXCLUDED.created_at,
    updated_at = EXCLUDED.updated_at
"#,
        )
        .bind(&record.tenant_id)
        .bind(&record.pubkey)
        .bind(&record.role)
        .bind(&record.status)
        .bind(record.created_at)
        .bind(record.updated_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn membership_by_pubkey(
        &self,
        tenant_id: &str,
        pubkey: &[u8],
    ) -> Result<Option<RelayMembershipRecord>, StorageError> {
        let row = sqlx::query(
            r#"
SELECT tenant_id, pubkey, role, status, created_at, updated_at
FROM relay_membership
WHERE tenant_id = $1 AND pubkey = $2
LIMIT 1
"#,
        )
        .bind(tenant_id)
        .bind(pubkey)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(relay_membership_from_row).transpose()?)
    }

    async fn list_memberships(
        &self,
        tenant_id: &str,
    ) -> Result<Vec<RelayMembershipRecord>, StorageError> {
        let rows = sqlx::query(
            r#"
SELECT tenant_id, pubkey, role, status, created_at, updated_at
FROM relay_membership
WHERE tenant_id = $1
ORDER BY pubkey
"#,
        )
        .bind(tenant_id)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(relay_membership_from_row).collect()
    }

    async fn remove_membership(
        &self,
        tenant_id: &str,
        pubkey: &[u8],
    ) -> Result<bool, StorageError> {
        let result = sqlx::query(
            r#"
DELETE FROM relay_membership
WHERE tenant_id = $1 AND pubkey = $2
"#,
        )
        .bind(tenant_id)
        .bind(pubkey)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() > 0)
    }

    async fn insert_invite(&self, record: RelayInviteRecord) -> Result<(), StorageError> {
        sqlx::query(
            r#"
INSERT INTO relay_invite (
    tenant_id,
    invite_code,
    role,
    inviter_pubkey,
    invitee_pubkey,
    expires_at,
    created_at
)
VALUES ($1, $2, $3, $4, $5, $6, $7)
"#,
        )
        .bind(&record.tenant_id)
        .bind(&record.invite_code)
        .bind(&record.role)
        .bind(&record.inviter_pubkey)
        .bind(&record.invitee_pubkey)
        .bind(record.expires_at)
        .bind(record.created_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn invite_by_code(
        &self,
        tenant_id: &str,
        invite_code: &str,
    ) -> Result<Option<RelayInviteRecord>, StorageError> {
        let row = sqlx::query(
            r#"
SELECT tenant_id,
       invite_code,
       role,
       inviter_pubkey,
       invitee_pubkey,
       expires_at,
       created_at
FROM relay_invite
WHERE tenant_id = $1 AND invite_code = $2
LIMIT 1
"#,
        )
        .bind(tenant_id)
        .bind(invite_code)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(relay_invite_from_row).transpose()?)
    }

    async fn delete_invite(
        &self,
        tenant_id: &str,
        invite_code: &str,
    ) -> Result<(), StorageError> {
        sqlx::query(
            r#"
DELETE FROM relay_invite
WHERE tenant_id = $1 AND invite_code = $2
"#,
        )
        .bind(tenant_id)
        .bind(invite_code)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}

fn relay_membership_from_row(
    row: sqlx::postgres::PgRow,
) -> Result<RelayMembershipRecord, StorageError> {
    Ok(RelayMembershipRecord {
        tenant_id: row.try_get("tenant_id")?,
        pubkey: row.try_get("pubkey")?,
        role: row.try_get("role")?,
        status: row.try_get("status")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

fn relay_invite_from_row(row: sqlx::postgres::PgRow) -> Result<RelayInviteRecord, StorageError> {
    Ok(RelayInviteRecord {
        tenant_id: row.try_get("tenant_id")?,
        invite_code: row.try_get("invite_code")?,
        role: row.try_get("role")?,
        inviter_pubkey: row.try_get("inviter_pubkey")?,
        invitee_pubkey: row.try_get("invitee_pubkey")?,
        expires_at: row.try_get("expires_at")?,
        created_at: row.try_get("created_at")?,
    })
}

#[async_trait]
impl EventRepository for PostgresRepositories {
    async fn insert_event(&self, record: EventRecord) -> Result<(), StorageError> {
        let mut tx = self.pool.begin().await?;
        let result = sqlx::query(
            r#"
INSERT INTO nostr_event (tenant_id, id, pubkey, created_at, kind, content, sig)
VALUES ($1, $2, $3, $4, $5, $6, $7)
ON CONFLICT DO NOTHING
"#,
        )
        .bind(&record.tenant_id)
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
INSERT INTO nostr_tag (tenant_id, event_id, name, value)
VALUES ($1, $2, $3, $4)
"#,
            )
            .bind(&record.tenant_id)
            .bind(&record.id)
            .bind(&tag.name)
            .bind(&tag.value)
            .execute(&mut *tx)
            .await?;
        }

        tx.commit().await?;
        Ok(())
    }

    async fn get_event(
        &self,
        tenant_id: &str,
        event_id: &[u8],
    ) -> Result<Option<EventRecord>, StorageError> {
        let row = sqlx::query(
            r#"
SELECT tenant_id, id, pubkey, created_at, kind, content, sig
FROM nostr_event
WHERE tenant_id = $1 AND id = $2
"#,
        )
        .bind(tenant_id)
        .bind(event_id)
        .fetch_optional(&self.pool)
        .await?;

        let Some(row) = row else {
            return Ok(None);
        };

        let id: Vec<u8> = row.try_get("id")?;
        let tags = self.fetch_tags(tenant_id, &id).await?;
        Ok(Some(EventRecord {
            tenant_id: row.try_get("tenant_id")?,
            id,
            pubkey: row.try_get("pubkey")?,
            created_at: row.try_get("created_at")?,
            kind: row.try_get::<i32, _>("kind")? as u32,
            content: row.try_get("content")?,
            sig: row.try_get("sig")?,
            tags,
        }))
    }

    async fn delete_event(&self, tenant_id: &str, event_id: &[u8]) -> Result<bool, StorageError> {
        let result = sqlx::query("DELETE FROM nostr_event WHERE tenant_id = $1 AND id = $2")
            .bind(tenant_id)
            .bind(event_id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    async fn query_events(&self, query: &EventQuery) -> Result<Vec<EventRecord>, StorageError> {
        use sqlx::QueryBuilder;
        let mut builder = QueryBuilder::new(
            "SELECT tenant_id, id, pubkey, created_at, kind, content, sig FROM nostr_event",
        );

        let mut separator = " WHERE ";
        if let Some(tenant_id) = &query.tenant_id {
            builder.push(separator).push("tenant_id = ");
            builder.push_bind(tenant_id);
            separator = " AND ";
        }
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
                builder.push("EXISTS (SELECT 1 FROM nostr_tag t WHERE t.event_id = nostr_event.id AND t.tenant_id = nostr_event.tenant_id AND t.name = ");
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
            let tenant_id: String = row.try_get("tenant_id")?;
            let tags = self.fetch_tags(&tenant_id, &id).await?;
            records.push(EventRecord {
                tenant_id,
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
        let tags = match serde_json::to_value(&entry.tags) {
            Ok(tags) => tags,
            Err(source) => {
                return Err(StorageError::Serialization {
                    field: "tags",
                    source,
                });
            }
        };
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
    use super::{
        PostgresRepositories, relay_invite_from_row, relay_membership_from_row,
        relay_tenant_from_row,
    };
    use crate::migrations::{MigrationRunner, core_migrations};
    use crate::repositories::{
        AccountRepository, AnnouncementRepository, EventRepository, ProfileRepository,
        RelayCompatibilityRepository, RelayMembershipRepository, RelayPublishRepository,
        RelayTenantRepository, RepoMappingRepository, StateRepository,
    };
    use crate::{
        AccountRecord, EventQuery, EventRecord, ProfileRecord, ProfileVisibility,
        RelayCompatibilityRecord, RelayInviteRecord, RelayMembershipRecord, RelayPublishRequest,
        RelayPublishStatus, RelayTenantRecord, RepoAnnouncementRecord, RepoMappingRecord,
        RepoStateRecord, StorageError, TagRecord,
    };
    use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
    use sqlx::PgPool;
    use std::str::FromStr;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    const DEFAULT_TEST_DATABASE_URL: &str = "postgres://gittree:gittree@127.0.0.1:5432/gittree";
    static TEST_DATABASE_COUNTER: AtomicU64 = AtomicU64::new(0);

    struct TestDatabase {
        base_url: String,
        database_name: String,
        pool: PgPool,
    }

    impl TestDatabase {
        async fn provision() -> Option<Self> {
            let base_url = match std::env::var("GITTREE_STORAGE_TEST_DATABASE_URL") {
                Ok(value) => value,
                Err(_) => DEFAULT_TEST_DATABASE_URL.to_string(),
            };
            let mut admin_options = match PgConnectOptions::from_str(&base_url) {
                Ok(options) => options,
                Err(_) => return None,
            };
            admin_options = admin_options.database("postgres");
            let admin_pool = match PgPoolOptions::new()
                .max_connections(1)
                .connect_with(admin_options)
                .await
            {
                Ok(pool) => pool,
                Err(_) => return None,
            };

            let database_name = unique_database_name();
            let create_database = format!("CREATE DATABASE \"{database_name}\"");
            if let Err(err) = sqlx::query(&create_database).execute(&admin_pool).await {
                panic!("failed creating test database {database_name}: {err}");
            }
            admin_pool.close().await;

            let mut test_options = PgConnectOptions::from_str(&base_url).expect("test options");
            test_options = test_options.database(&database_name);
            let pool = PgPoolOptions::new()
                .max_connections(5)
                .connect_with(test_options)
                .await
                .expect("connect test database");

            let runner = MigrationRunner::new(core_migrations()).expect("runner");
            let mut connection = pool.acquire().await.expect("connection");
            runner.run(&mut *connection).await.expect("migrations");
            drop(connection);

            Some(Self {
                base_url,
                database_name,
                pool,
            })
        }

        fn repositories(&self) -> PostgresRepositories {
            PostgresRepositories::new(self.pool.clone())
        }

        async fn cleanup(self) {
            self.pool.close().await;

            let mut admin_options = match PgConnectOptions::from_str(&self.base_url) {
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
            .bind(&self.database_name)
            .execute(&admin_pool)
            .await;

            let drop_database = format!("DROP DATABASE IF EXISTS \"{}\"", self.database_name);
            let _ = sqlx::query(&drop_database).execute(&admin_pool).await;
            admin_pool.close().await;
        }
    }

    fn unique_database_name() -> String {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let counter = TEST_DATABASE_COUNTER.fetch_add(1, Ordering::Relaxed);
        format!(
            "gittree_storage_test_{}_{}_{}",
            std::process::id(),
            now,
            counter
        )
    }

    fn unreachable_repositories() -> PostgresRepositories {
        let options =
            PgConnectOptions::from_str("postgres://gittree:gittree@127.0.0.1:1/gittree")
                .expect("connect options");
        let pool = PgPoolOptions::new()
            .max_connections(1)
            .min_connections(0)
            .acquire_timeout(Duration::from_millis(100))
            .connect_lazy_with(options);
        PostgresRepositories::new(pool)
    }

    fn assert_db_error(err: StorageError) {
        assert!(matches!(err, StorageError::Database { .. }));
    }

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
    fn offset_datetime_helpers_round_trip_unix_timestamps() {
        let timestamp = PostgresRepositories::to_offset_datetime(1_704_067_200).expect("timestamp");
        assert_eq!(
            PostgresRepositories::from_offset_datetime(timestamp),
            1_704_067_200
        );
    }

    #[test]
    fn decode_hex_reports_field_name_on_invalid_input() {
        let bytes = PostgresRepositories::decode_hex("authors", &"11".repeat(32))
            .expect("valid hex should decode");
        assert_eq!(bytes.len(), 32);

        let err = PostgresRepositories::decode_hex("authors", "not-hex").unwrap_err();
        assert!(matches!(
            err,
            StorageError::InvalidHex {
                field: "authors",
                ..
            }
        ));
    }

    #[tokio::test]
    async fn test_database_cleanup_returns_early_for_invalid_base_url() {
        let pool = PgPoolOptions::new()
            .max_connections(1)
            .connect_lazy("postgres://invalid:invalid@127.0.0.1:1/invalid")
            .expect("lazy pool");
        let harness = TestDatabase {
            base_url: "not-a-postgres-url".to_string(),
            database_name: unique_database_name(),
            pool,
        };
        harness.cleanup().await;
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

    #[test]
    fn postgres_repos_implements_relay_tenant_repo() {
        fn assert_impl<T: RelayTenantRepository>() {}
        assert_impl::<PostgresRepositories>();
    }

    #[test]
    fn postgres_repos_implements_relay_membership_repo() {
        fn assert_impl<T: RelayMembershipRepository>() {}
        assert_impl::<PostgresRepositories>();
    }

    #[tokio::test]
    async fn postgres_repositories_exercise_unreachable_database_paths() {
        let repositories = unreachable_repositories();
        let repo_pubkey = "11".repeat(32);

        let announcement = RepoAnnouncementRecord {
            event_id: vec![0x01; 32],
            pubkey: vec![0x11; 32],
            identifier: "repo".to_string(),
            name: Some("repo".to_string()),
            description: Some("description".to_string()),
            root_commit: Some("0123456789abcdef0123456789abcdef01234567".to_string()),
            clone_urls: vec!["https://git.example/repo.git".to_string()],
            web_urls: vec!["https://git.example/repo".to_string()],
            relays: vec!["wss://relay.example".to_string()],
            blossoms: vec!["https://blossom.example".to_string()],
            hashtags: vec!["nostr".to_string()],
            maintainers: vec!["aa".repeat(32)],
            created_at: 10,
        };
        assert_db_error(
            repositories
                .insert_announcement(announcement.clone())
                .await
                .expect_err("insert announcement"),
        );
        assert_db_error(
            repositories
                .list_announcements(&announcement.pubkey, "repo")
                .await
                .expect_err("list announcements"),
        );
        assert_db_error(
            repositories
                .latest_announcement(&announcement.pubkey, "repo")
                .await
                .expect_err("latest announcement"),
        );

        let state = RepoStateRecord {
            event_id: vec![0x02; 32],
            pubkey: vec![0x11; 32],
            identifier: "repo".to_string(),
            created_at: 11,
            state_json: "{\"HEAD\":\"ref: refs/heads/main\"}".to_string(),
        };
        assert_db_error(
            repositories
                .insert_state(state)
                .await
                .expect_err("insert state"),
        );
        assert_db_error(
            repositories
                .latest_state(&[0x11; 32], "repo")
                .await
                .expect_err("latest state"),
        );

        let mapping = RepoMappingRecord {
            forgejo_owner: "alice".to_string(),
            forgejo_repo: "repo".to_string(),
            pubkey: hex::decode(&repo_pubkey).expect("pubkey"),
            identifier: "repo".to_string(),
        };
        assert_db_error(
            repositories
                .upsert_mapping(mapping)
                .await
                .expect_err("upsert mapping"),
        );
        assert_db_error(
            repositories
                .mapping_by_forgejo("alice", "repo")
                .await
                .expect_err("mapping by forgejo"),
        );
        assert_db_error(
            repositories
                .mapping_by_repo(&[0x11; 32], "repo")
                .await
                .expect_err("mapping by repo"),
        );
        assert_db_error(
            repositories
                .list_mappings()
                .await
                .expect_err("list mappings"),
        );

        let account = AccountRecord::new(&"22".repeat(32), "alice").expect("account");
        assert_db_error(
            repositories
                .upsert_account(account.clone())
                .await
                .expect_err("upsert account"),
        );
        assert_db_error(
            repositories
                .account_by_pubkey(&account.pubkey)
                .await
                .expect_err("account by pubkey"),
        );
        assert_db_error(
            repositories
                .account_by_username("alice")
                .await
                .expect_err("account by username"),
        );

        let profile = ProfileRecord::new(
            &"22".repeat(32),
            Some("Alice".to_string()),
            Some("bio".to_string()),
            None,
            None,
            None,
            ProfileVisibility::Public,
            100,
            100,
        )
        .expect("profile");
        assert_db_error(
            repositories
                .upsert_profile(profile)
                .await
                .expect_err("upsert profile"),
        );
        assert_db_error(
            repositories
                .profile_by_pubkey(&account.pubkey)
                .await
                .expect_err("profile by pubkey"),
        );

        let compatibility = RelayCompatibilityRecord {
            relay_url: "wss://relay.example".to_string(),
            compatible: false,
            supported_capabilities: vec!["nip-01".to_string()],
            missing_required: vec!["nip-34".to_string()],
            missing_optional: vec!["nip-11".to_string()],
            report_json: "{\"relay_url\":\"wss://relay.example\"}".to_string(),
            nip11_url: Some("https://relay.example".to_string()),
            nip11_available: true,
            active_probe_ok: Some(false),
            active_probe_error: Some("timeout".to_string()),
            checked_at: 100,
        };
        assert_db_error(
            repositories
                .upsert_relay_compatibility(compatibility)
                .await
                .expect_err("upsert relay compatibility"),
        );
        assert_db_error(
            repositories
                .relay_compatibility("wss://relay.example")
                .await
                .expect_err("relay compatibility"),
        );

        let tenant = RelayTenantRecord::new(
            "tenant-1",
            "relay.tenant.local",
            &"66".repeat(32),
            vec![1, 2, 3, 4],
            vec![5, 6, 7, 8],
            "local",
            Some("Tenant 1".to_string()),
            Some("tenant description".to_string()),
            Some("https://example.com/icon.png".to_string()),
            Some("https://example.com/banner.png".to_string()),
            Some("ops@example.com".to_string()),
            true,
            false,
            false,
            100,
            100,
        )
        .expect("tenant");
        assert_db_error(
            repositories
                .upsert_tenant(tenant)
                .await
                .expect_err("upsert tenant"),
        );
        assert_db_error(
            repositories
                .tenant_by_id("tenant-1")
                .await
                .expect_err("tenant by id"),
        );
        assert_db_error(
            repositories
                .tenant_by_host("relay.tenant.local")
                .await
                .expect_err("tenant by host"),
        );
        assert_db_error(
            repositories
                .list_tenants()
                .await
                .expect_err("list tenants"),
        );

        let membership = RelayMembershipRecord::new(
            "tenant-1",
            &"77".repeat(32),
            "member",
            "active",
            110,
            110,
        )
        .expect("membership");
        assert_db_error(
            repositories
                .upsert_membership(membership.clone())
                .await
                .expect_err("upsert membership"),
        );
        assert_db_error(
            repositories
                .membership_by_pubkey("tenant-1", &membership.pubkey)
                .await
                .expect_err("membership by pubkey"),
        );
        assert_db_error(
            repositories
                .list_memberships("tenant-1")
                .await
                .expect_err("list memberships"),
        );
        assert_db_error(
            repositories
                .remove_membership("tenant-1", &membership.pubkey)
                .await
                .expect_err("remove membership"),
        );

        let invite = RelayInviteRecord::new(
            "tenant-1",
            "invite-1",
            "member",
            &"88".repeat(32),
            Some(&"99".repeat(32)),
            Some(500),
            120,
        )
        .expect("invite");
        assert_db_error(
            repositories
                .insert_invite(invite)
                .await
                .expect_err("insert invite"),
        );
        assert_db_error(
            repositories
                .invite_by_code("tenant-1", "invite-1")
                .await
                .expect_err("invite by code"),
        );
        assert_db_error(
            repositories
                .delete_invite("tenant-1", "invite-1")
                .await
                .expect_err("delete invite"),
        );

        let event = EventRecord::new(
            "tenant-1",
            &"aa".repeat(32),
            &"bb".repeat(32),
            200,
            1,
            "event-a",
            &"cc".repeat(64),
            vec![vec!["e".to_string(), "1".to_string()]],
        )
        .expect("event");
        assert_db_error(
            repositories
                .insert_event(event.clone())
                .await
                .expect_err("insert event"),
        );
        assert_db_error(
            repositories
                .get_event("tenant-1", &event.id)
                .await
                .expect_err("get event"),
        );
        assert_db_error(
            repositories
                .delete_event("tenant-1", &event.id)
                .await
                .expect_err("delete event"),
        );
        let mut query = EventQuery::for_tenant("tenant-1");
        query.ids.push("aa".repeat(32));
        query.authors.push("bb".repeat(32));
        query.kinds.push(1);
        query.tags.push(TagRecord::new("e", "1"));
        query.since = Some(100);
        query.until = Some(300);
        query.limit = Some(5);
        assert_db_error(
            repositories
                .query_events(&query)
                .await
                .expect_err("query events"),
        );

        let request = RelayPublishRequest {
            relay_url: "wss://relay.local".to_string(),
            event_id: "33".repeat(32),
            pubkey: "44".repeat(32),
            created_at: 123,
            kind: 30617,
            tags: vec![vec!["d".to_string(), "demo".to_string()]],
            content: String::new(),
            sig: "55".repeat(64),
            forgejo_owner: "alice".to_string(),
            forgejo_repo: "demo".to_string(),
            identifier: "demo".to_string(),
        };
        assert_db_error(
            repositories
                .enqueue_relay_publish(request)
                .await
                .expect_err("enqueue relay publish"),
        );
        assert_db_error(
            repositories
                .claim_relay_publish(time::OffsetDateTime::now_utc())
                .await
                .expect_err("claim relay publish"),
        );
        assert_db_error(
            repositories
                .mark_relay_publish_succeeded(1)
                .await
                .expect_err("mark relay publish succeeded"),
        );
        assert_db_error(
            repositories
                .mark_relay_publish_failed(1, "error", time::OffsetDateTime::now_utc())
                .await
                .expect_err("mark relay publish failed"),
        );
        assert_db_error(
            repositories
                .pending_relay_publishes(&[0x44; 32], "demo", 30617)
                .await
                .expect_err("pending relay publishes"),
        );
    }

    #[tokio::test]
    async fn postgres_repo_mapping_round_trip_db() {
        let Some(test_db) = TestDatabase::provision().await else {
            eprintln!("skipping postgres_repo_mapping_round_trip_db: postgres unavailable");
            return;
        };

        let repositories = test_db.repositories();
        let record = RepoMappingRecord {
            forgejo_owner: "alice".to_string(),
            forgejo_repo: "demo".to_string(),
            pubkey: vec![0x11; 32],
            identifier: "demo".to_string(),
        };

        repositories
            .upsert_mapping(record.clone())
            .await
            .expect("upsert");

        let by_forgejo = repositories
            .mapping_by_forgejo("alice", "demo")
            .await
            .expect("mapping by forgejo")
            .expect("record");
        assert_eq!(by_forgejo, record);

        let by_repo = repositories
            .mapping_by_repo(&record.pubkey, "demo")
            .await
            .expect("mapping by repo")
            .expect("record");
        assert_eq!(by_repo, record);

        let mappings = repositories.list_mappings().await.expect("list mappings");
        assert_eq!(mappings.len(), 1);
        test_db.cleanup().await;
    }

    #[tokio::test]
    async fn postgres_repositories_return_none_for_missing_rows_db() {
        let Some(test_db) = TestDatabase::provision().await else {
            eprintln!("skipping postgres_repositories_return_none_for_missing_rows_db: postgres unavailable");
            return;
        };

        let repositories = test_db.repositories();
        assert!(
            repositories
                .mapping_by_forgejo("missing", "repo")
                .await
                .expect("mapping by forgejo")
                .is_none()
        );
        assert!(
            repositories
                .mapping_by_repo(&[0x11; 32], "missing")
                .await
                .expect("mapping by repo")
                .is_none()
        );
        assert!(
            repositories
                .latest_announcement(&[0x11; 32], "missing")
                .await
                .expect("latest announcement")
                .is_none()
        );
        assert!(
            repositories
                .latest_state(&[0x11; 32], "missing")
                .await
                .expect("latest state")
                .is_none()
        );
        assert!(
            repositories
                .account_by_pubkey(&[0x22; 32])
                .await
                .expect("account by pubkey")
                .is_none()
        );
        assert!(
            repositories
                .account_by_username("missing")
                .await
                .expect("account by username")
                .is_none()
        );
        assert!(
            repositories
                .profile_by_pubkey(&[0x22; 32])
                .await
                .expect("profile by pubkey")
                .is_none()
        );
        assert!(
            repositories
                .relay_compatibility("wss://missing.local")
                .await
                .expect("relay compatibility")
                .is_none()
        );
        assert!(
            repositories
                .tenant_by_id("missing")
                .await
                .expect("tenant by id")
                .is_none()
        );
        assert!(
            repositories
                .tenant_by_host("missing.local")
                .await
                .expect("tenant by host")
                .is_none()
        );
        assert!(repositories.list_tenants().await.expect("list tenants").is_empty());
        assert!(
            repositories
                .membership_by_pubkey("missing", &[0x33; 32])
                .await
                .expect("membership by pubkey")
                .is_none()
        );
        assert!(
            repositories
                .list_memberships("missing")
                .await
                .expect("list memberships")
                .is_empty()
        );
        assert!(
            !repositories
                .remove_membership("missing", &[0x33; 32])
                .await
                .expect("remove membership")
        );
        assert!(
            repositories
                .invite_by_code("missing", "code")
                .await
                .expect("invite by code")
                .is_none()
        );
        repositories
            .delete_invite("missing", "code")
            .await
            .expect("delete missing invite");
        assert!(
            repositories
                .get_event("missing", &[0x44; 32])
                .await
                .expect("get event")
                .is_none()
        );
        assert!(
            !repositories
                .delete_event("missing", &[0x44; 32])
                .await
                .expect("delete event")
        );
        assert!(
            repositories
                .query_events(&EventQuery::for_tenant("missing"))
                .await
                .expect("query events")
                .is_empty()
        );
        assert!(
            repositories
                .claim_relay_publish(time::OffsetDateTime::now_utc())
                .await
                .expect("claim relay publish")
                .is_none()
        );
        let pending = repositories
            .pending_relay_publishes(&[0x44; 32], "missing", 30617)
            .await
            .expect("pending relay publishes");
        assert_eq!(pending, 0);

        test_db.cleanup().await;
    }

    #[tokio::test]
    async fn postgres_announcement_state_and_compatibility_round_trip_db() {
        let Some(test_db) = TestDatabase::provision().await else {
            eprintln!("skipping postgres_announcement_state_and_compatibility_round_trip_db: postgres unavailable");
            return;
        };

        let repositories = test_db.repositories();
        let pubkey = vec![0x51; 32];

        let announcement_old = RepoAnnouncementRecord {
            event_id: vec![0x41; 32],
            pubkey: pubkey.clone(),
            identifier: "repo".to_string(),
            name: Some("repo-old".to_string()),
            description: Some("description".to_string()),
            root_commit: Some("0123456789abcdef0123456789abcdef01234567".to_string()),
            clone_urls: vec!["https://git.example/repo.git".to_string()],
            web_urls: vec!["https://git.example/repo".to_string()],
            relays: vec!["wss://relay.example".to_string()],
            blossoms: vec!["https://blossom.example".to_string()],
            hashtags: vec!["nostr".to_string()],
            maintainers: vec!["aa".repeat(32)],
            created_at: 10,
        };
        let mut announcement_new = announcement_old.clone();
        announcement_new.event_id = vec![0x42; 32];
        announcement_new.name = Some("repo-new".to_string());
        announcement_new.created_at = 20;

        repositories
            .insert_announcement(announcement_old)
            .await
            .expect("insert old announcement");
        repositories
            .insert_announcement(announcement_new.clone())
            .await
            .expect("insert new announcement");

        let listed = repositories
            .list_announcements(&pubkey, "repo")
            .await
            .expect("list announcements");
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0].event_id, announcement_new.event_id);
        assert_eq!(listed[0].name.as_deref(), Some("repo-new"));

        let latest = repositories
            .latest_announcement(&pubkey, "repo")
            .await
            .expect("latest announcement")
            .expect("announcement");
        assert_eq!(latest.event_id, announcement_new.event_id);

        let state_old = RepoStateRecord {
            event_id: vec![0x43; 32],
            pubkey: pubkey.clone(),
            identifier: "repo".to_string(),
            created_at: 11,
            state_json: "{\"HEAD\":\"ref: refs/heads/main\"}".to_string(),
        };
        let mut state_new = state_old.clone();
        state_new.event_id = vec![0x44; 32];
        state_new.created_at = 21;
        state_new.state_json =
            "{\"HEAD\":\"ref: refs/heads/main\",\"refs/heads/main\":\"0123\"}".to_string();

        repositories
            .insert_state(state_old)
            .await
            .expect("insert old state");
        repositories
            .insert_state(state_new.clone())
            .await
            .expect("insert new state");

        let latest_state = repositories
            .latest_state(&pubkey, "repo")
            .await
            .expect("latest state")
            .expect("state");
        assert_eq!(latest_state.event_id, state_new.event_id);
        assert!(latest_state.state_json.contains("\"refs/heads/main\""));

        let compatibility_old = RelayCompatibilityRecord {
            relay_url: "wss://relay.example".to_string(),
            compatible: false,
            supported_capabilities: vec!["nip-01".to_string()],
            missing_required: vec!["nip-34".to_string()],
            missing_optional: vec!["nip-11".to_string()],
            report_json: "{\"relay_url\":\"wss://relay.example\"}".to_string(),
            nip11_url: Some("https://relay.example".to_string()),
            nip11_available: true,
            active_probe_ok: Some(false),
            active_probe_error: Some("timeout".to_string()),
            checked_at: 100,
        };
        repositories
            .upsert_relay_compatibility(compatibility_old)
            .await
            .expect("upsert old compatibility");

        let mut compatibility_new = RelayCompatibilityRecord {
            relay_url: "wss://relay.example".to_string(),
            compatible: true,
            supported_capabilities: vec!["nip-01".to_string(), "nip-34".to_string()],
            missing_required: Vec::new(),
            missing_optional: vec!["nip-11".to_string()],
            report_json: "{\"relay_url\":\"wss://relay.example\",\"compatible\":true}".to_string(),
            nip11_url: Some("https://relay.example".to_string()),
            nip11_available: true,
            active_probe_ok: Some(true),
            active_probe_error: None,
            checked_at: 200,
        };
        repositories
            .upsert_relay_compatibility(compatibility_new.clone())
            .await
            .expect("upsert new compatibility");

        let stored_compatibility = repositories
            .relay_compatibility("wss://relay.example")
            .await
            .expect("relay compatibility")
            .expect("compatibility");
        compatibility_new.report_json = stored_compatibility.report_json.clone();
        assert_eq!(stored_compatibility, compatibility_new);

        test_db.cleanup().await;
    }

    #[tokio::test]
    async fn postgres_account_and_profile_round_trip_db() {
        let Some(test_db) = TestDatabase::provision().await else {
            eprintln!("skipping postgres_account_and_profile_round_trip_db: postgres unavailable");
            return;
        };

        let repositories = test_db.repositories();
        let pubkey_hex = "22".repeat(32);
        let account = AccountRecord::new(&pubkey_hex, "alice").expect("account");
        repositories
            .upsert_account(account.clone())
            .await
            .expect("upsert account");

        let by_pubkey = repositories
            .account_by_pubkey(&account.pubkey)
            .await
            .expect("account by pubkey")
            .expect("account");
        assert_eq!(by_pubkey.forgejo_username, "alice");

        let by_username = repositories
            .account_by_username("alice")
            .await
            .expect("account by username")
            .expect("account");
        assert_eq!(by_username.pubkey, account.pubkey);

        let profile = ProfileRecord::new(
            &pubkey_hex,
            Some("Alice".to_string()),
            Some("bio".to_string()),
            None,
            None,
            None,
            ProfileVisibility::Private,
            100,
            100,
        )
        .expect("profile");
        repositories
            .upsert_profile(profile)
            .await
            .expect("upsert profile");

        let stored_profile = repositories
            .profile_by_pubkey(&account.pubkey)
            .await
            .expect("profile by pubkey")
            .expect("profile");
        assert_eq!(stored_profile.display_name.as_deref(), Some("Alice"));
        assert_eq!(stored_profile.visibility, ProfileVisibility::Private);
        test_db.cleanup().await;
    }

    #[tokio::test]
    async fn postgres_relay_publish_round_trip_db() {
        let Some(test_db) = TestDatabase::provision().await else {
            eprintln!("skipping postgres_relay_publish_round_trip_db: postgres unavailable");
            return;
        };

        let repositories = test_db.repositories();
        let request = RelayPublishRequest {
            relay_url: "wss://relay.local".to_string(),
            event_id: "33".repeat(32),
            pubkey: "44".repeat(32),
            created_at: 123,
            kind: 30617,
            tags: vec![vec!["d".to_string(), "demo".to_string()]],
            content: String::new(),
            sig: "55".repeat(64),
            forgejo_owner: "alice".to_string(),
            forgejo_repo: "demo".to_string(),
            identifier: "demo".to_string(),
        };

        repositories
            .enqueue_relay_publish(request)
            .await
            .expect("enqueue");

        let pending_before = repositories
            .pending_relay_publishes(&[0x44; 32], "demo", 30617)
            .await
            .expect("pending");
        assert_eq!(pending_before, 1);

        let now = time::OffsetDateTime::now_utc();
        let job = repositories
            .claim_relay_publish(now)
            .await
            .expect("claim")
            .expect("job");
        assert_eq!(job.forgejo_owner, "alice");
        assert_eq!(job.forgejo_repo, "demo");
        repositories
            .mark_relay_publish_succeeded(job.id)
            .await
            .expect("mark succeeded");

        let pending_after = repositories
            .pending_relay_publishes(&[0x44; 32], "demo", 30617)
            .await
            .expect("pending");
        assert_eq!(pending_after, 0);
        test_db.cleanup().await;
    }

    #[tokio::test]
    async fn postgres_tenant_membership_invite_round_trip_db() {
        let Some(test_db) = TestDatabase::provision().await else {
            eprintln!("skipping postgres_tenant_membership_invite_round_trip_db: postgres unavailable");
            return;
        };

        let repositories = test_db.repositories();
        let tenant = RelayTenantRecord::new(
            "tenant-1",
            "relay.tenant.local",
            &"66".repeat(32),
            vec![1, 2, 3, 4],
            vec![5, 6, 7, 8],
            "local",
            Some("Tenant 1".to_string()),
            Some("tenant description".to_string()),
            Some("https://example.com/icon.png".to_string()),
            Some("https://example.com/banner.png".to_string()),
            Some("ops@example.com".to_string()),
            true,
            false,
            false,
            100,
            100,
        )
        .expect("tenant");
        repositories
            .upsert_tenant(tenant.clone())
            .await
            .expect("upsert tenant");

        let by_id = repositories
            .tenant_by_id("tenant-1")
            .await
            .expect("tenant by id")
            .expect("tenant");
        assert_eq!(by_id.host, "relay.tenant.local");

        let by_host = repositories
            .tenant_by_host("relay.tenant.local")
            .await
            .expect("tenant by host")
            .expect("tenant");
        assert_eq!(by_host.id, "tenant-1");

        let tenants = repositories.list_tenants().await.expect("list tenants");
        assert_eq!(tenants.len(), 1);

        let membership = RelayMembershipRecord::new(
            "tenant-1",
            &"77".repeat(32),
            "member",
            "active",
            110,
            110,
        )
        .expect("membership");
        repositories
            .upsert_membership(membership.clone())
            .await
            .expect("upsert membership");

        let by_pubkey = repositories
            .membership_by_pubkey("tenant-1", &membership.pubkey)
            .await
            .expect("membership by pubkey")
            .expect("membership");
        assert_eq!(by_pubkey.role, "member");

        let memberships = repositories
            .list_memberships("tenant-1")
            .await
            .expect("list memberships");
        assert_eq!(memberships.len(), 1);

        let removed = repositories
            .remove_membership("tenant-1", &membership.pubkey)
            .await
            .expect("remove membership");
        assert!(removed);
        let removed_again = repositories
            .remove_membership("tenant-1", &membership.pubkey)
            .await
            .expect("remove membership second");
        assert!(!removed_again);

        let invite = RelayInviteRecord::new(
            "tenant-1",
            "invite-1",
            "member",
            &"88".repeat(32),
            Some(&"99".repeat(32)),
            Some(500),
            120,
        )
        .expect("invite");
        repositories
            .insert_invite(invite.clone())
            .await
            .expect("insert invite");

        let invite_lookup = repositories
            .invite_by_code("tenant-1", "invite-1")
            .await
            .expect("invite by code")
            .expect("invite");
        assert_eq!(invite_lookup.role, "member");

        repositories
            .delete_invite("tenant-1", "invite-1")
            .await
            .expect("delete invite");
        let invite_missing = repositories
            .invite_by_code("tenant-1", "invite-1")
            .await
            .expect("invite by code");
        assert!(invite_missing.is_none());

        test_db.cleanup().await;
    }

    #[tokio::test]
    async fn postgres_event_repository_round_trip_db() {
        let Some(test_db) = TestDatabase::provision().await else {
            eprintln!("skipping postgres_event_repository_round_trip_db: postgres unavailable");
            return;
        };

        let repositories = test_db.repositories();
        let event_a = EventRecord::new(
            "tenant-1",
            &"aa".repeat(32),
            &"bb".repeat(32),
            200,
            1,
            "event-a",
            &"cc".repeat(64),
            vec![
                vec!["e".to_string(), "1".to_string()],
                vec!["p".to_string(), "2".to_string()],
            ],
        )
        .expect("event a");
        let event_b = EventRecord::new(
            "tenant-1",
            &"dd".repeat(32),
            &"bb".repeat(32),
            300,
            2,
            "event-b",
            &"ee".repeat(64),
            vec![vec!["e".to_string(), "1".to_string()]],
        )
        .expect("event b");

        repositories
            .insert_event(event_a.clone())
            .await
            .expect("insert a");
        repositories
            .insert_event(event_b.clone())
            .await
            .expect("insert b");

        let fetched = repositories
            .get_event("tenant-1", &event_a.id)
            .await
            .expect("get event")
            .expect("event");
        assert_eq!(fetched.content, "event-a");
        assert_eq!(fetched.tags.len(), 2);

        let by_tenant = repositories
            .query_events(&EventQuery::for_tenant("tenant-1"))
            .await
            .expect("query tenant");
        assert_eq!(by_tenant.len(), 2);
        assert_eq!(by_tenant[0].created_at, 300);

        let by_id = repositories
            .query_events(&EventQuery::for_ids(vec!["aa".repeat(32)]))
            .await
            .expect("query ids");
        assert_eq!(by_id.len(), 1);
        assert_eq!(by_id[0].content, "event-a");

        let by_author = repositories
            .query_events(&EventQuery::for_authors(vec!["bb".repeat(32)]))
            .await
            .expect("query authors");
        assert_eq!(by_author.len(), 2);

        let by_kind = repositories
            .query_events(&EventQuery::for_kinds(vec![2]))
            .await
            .expect("query kinds");
        assert_eq!(by_kind.len(), 1);
        assert_eq!(by_kind[0].content, "event-b");

        let by_tag = repositories
            .query_events(&EventQuery::for_tag("e", vec!["1".to_string()]))
            .await
            .expect("query tags");
        assert_eq!(by_tag.len(), 2);

        let mut by_time = EventQuery::for_tenant("tenant-1");
        by_time.since = Some(250);
        by_time.until = Some(350);
        let by_time = repositories
            .query_events(&by_time)
            .await
            .expect("query by time");
        assert_eq!(by_time.len(), 1);
        assert_eq!(by_time[0].content, "event-b");

        let mut by_two_tags = EventQuery::for_tenant("tenant-1");
        by_two_tags.tags.push(TagRecord::new("e", "1"));
        by_two_tags.tags.push(TagRecord::new("p", "2"));
        let by_two_tags = repositories
            .query_events(&by_two_tags)
            .await
            .expect("query two tags");
        assert_eq!(by_two_tags.len(), 1);
        assert_eq!(by_two_tags[0].content, "event-a");

        let mut invalid_author_query = EventQuery::for_tenant("tenant-1");
        invalid_author_query.authors.push("not-hex".to_string());
        let invalid_author_err = repositories
            .query_events(&invalid_author_query)
            .await
            .expect_err("invalid author");
        assert!(matches!(
            invalid_author_err,
            StorageError::InvalidHex {
                field: "authors",
                ..
            }
        ));

        let mut limited_query = EventQuery::for_tenant("tenant-1");
        limited_query.limit = Some(1);
        limited_query.tags.push(TagRecord::new("p", "2"));
        let limited = repositories
            .query_events(&limited_query)
            .await
            .expect("query limited");
        assert_eq!(limited.len(), 1);
        assert_eq!(limited[0].content, "event-a");

        let deleted = repositories
            .delete_event("tenant-1", &event_a.id)
            .await
            .expect("delete event");
        assert!(deleted);
        let deleted_again = repositories
            .delete_event("tenant-1", &event_a.id)
            .await
            .expect("delete event again");
        assert!(!deleted_again);

        let missing = repositories
            .get_event("tenant-1", &event_a.id)
            .await
            .expect("get event");
        assert!(missing.is_none());

        test_db.cleanup().await;
    }

    #[tokio::test]
    async fn postgres_upserts_replace_existing_rows_db() {
        let Some(test_db) = TestDatabase::provision().await else {
            eprintln!("skipping postgres_upserts_replace_existing_rows_db: postgres unavailable");
            return;
        };

        let repositories = test_db.repositories();

        let first_mapping = RepoMappingRecord {
            forgejo_owner: "alice".to_string(),
            forgejo_repo: "demo".to_string(),
            pubkey: vec![0x11; 32],
            identifier: "demo".to_string(),
        };
        repositories
            .upsert_mapping(first_mapping.clone())
            .await
            .expect("insert mapping");

        let updated_mapping = RepoMappingRecord {
            forgejo_owner: "alice".to_string(),
            forgejo_repo: "demo".to_string(),
            pubkey: vec![0x22; 32],
            identifier: "demo-v2".to_string(),
        };
        repositories
            .upsert_mapping(updated_mapping.clone())
            .await
            .expect("update mapping");
        let by_forgejo = repositories
            .mapping_by_forgejo("alice", "demo")
            .await
            .expect("mapping by forgejo")
            .expect("mapping");
        assert_eq!(by_forgejo, updated_mapping);
        let by_repo = repositories
            .mapping_by_repo(&updated_mapping.pubkey, "demo-v2")
            .await
            .expect("mapping by repo")
            .expect("mapping");
        assert_eq!(by_repo, updated_mapping);
        let mappings = repositories.list_mappings().await.expect("list mappings");
        assert_eq!(mappings, vec![updated_mapping.clone()]);

        let account = AccountRecord::new(&"33".repeat(32), "alice").expect("account");
        repositories
            .upsert_account(account.clone())
            .await
            .expect("insert account");
        let updated_account = AccountRecord::new(&"33".repeat(32), "alice_2").expect("account");
        repositories
            .upsert_account(updated_account.clone())
            .await
            .expect("update account");
        assert!(
            repositories
                .account_by_username("alice")
                .await
                .expect("account by username")
                .is_none()
        );
        let by_username = repositories
            .account_by_username("alice_2")
            .await
            .expect("account by username")
            .expect("account");
        assert_eq!(by_username, updated_account);
        let by_pubkey = repositories
            .account_by_pubkey(&updated_account.pubkey)
            .await
            .expect("account by pubkey")
            .expect("account");
        assert_eq!(by_pubkey, updated_account);

        let profile = ProfileRecord::new(
            &"33".repeat(32),
            Some("Alice".to_string()),
            Some("bio-1".to_string()),
            None,
            None,
            None,
            ProfileVisibility::Public,
            1,
            1,
        )
        .expect("profile");
        repositories
            .upsert_profile(profile)
            .await
            .expect("insert profile");
        let updated_profile = ProfileRecord::new(
            &"33".repeat(32),
            Some("Alice 2".to_string()),
            Some("bio-2".to_string()),
            Some("https://example.com/avatar.png".to_string()),
            Some("https://example.com".to_string()),
            Some("earth".to_string()),
            ProfileVisibility::Private,
            1,
            2,
        )
        .expect("profile");
        repositories
            .upsert_profile(updated_profile.clone())
            .await
            .expect("update profile");
        let stored_profile = repositories
            .profile_by_pubkey(&updated_profile.pubkey)
            .await
            .expect("profile by pubkey")
            .expect("profile");
        assert_eq!(stored_profile, updated_profile);

        test_db.cleanup().await;
    }

    #[tokio::test]
    async fn postgres_decoder_helpers_report_row_errors_db() {
        let Some(test_db) = TestDatabase::provision().await else {
            eprintln!("skipping postgres_decoder_helpers_report_row_errors_db: postgres unavailable");
            return;
        };

        let bad_tenant_row = sqlx::query(
            r#"
SELECT
    1::integer AS id,
    'relay.example'::text AS host,
    E'\\x01'::bytea AS relay_pubkey,
    E'\\x02'::bytea AS relay_secret,
    E'\\x03'::bytea AS relay_secret_nonce,
    'kid'::text AS relay_secret_kid,
    NULL::text AS name,
    NULL::text AS description,
    NULL::text AS icon,
    NULL::text AS banner,
    NULL::text AS contact,
    true AS auth_required,
    true AS public_read,
    false AS public_write,
    10::bigint AS created_at,
    11::bigint AS updated_at
"#,
        )
        .fetch_one(&test_db.pool)
        .await
        .expect("bad tenant row");
        assert_db_error(relay_tenant_from_row(bad_tenant_row).expect_err("tenant row decode"));

        let bad_membership_row = sqlx::query(
            r#"
SELECT
    'tenant-1'::text AS tenant_id,
    E'\\x04'::bytea AS pubkey,
    9::integer AS role,
    'active'::text AS status,
    10::bigint AS created_at,
    11::bigint AS updated_at
"#,
        )
        .fetch_one(&test_db.pool)
        .await
        .expect("bad membership row");
        assert_db_error(
            relay_membership_from_row(bad_membership_row).expect_err("membership row decode"),
        );

        let bad_invite_row = sqlx::query(
            r#"
SELECT
    'tenant-1'::text AS tenant_id,
    7::integer AS invite_code,
    'member'::text AS role,
    E'\\x05'::bytea AS inviter_pubkey,
    NULL::bytea AS invitee_pubkey,
    NULL::bigint AS expires_at,
    12::bigint AS created_at
"#,
        )
        .fetch_one(&test_db.pool)
        .await
        .expect("bad invite row");
        assert_db_error(relay_invite_from_row(bad_invite_row).expect_err("invite row decode"));

        test_db.cleanup().await;
    }

    #[tokio::test]
    async fn postgres_relay_publish_claim_reports_invalid_tags_db() {
        let Some(test_db) = TestDatabase::provision().await else {
            eprintln!("skipping postgres_relay_publish_claim_reports_invalid_tags_db: postgres unavailable");
            return;
        };

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
VALUES ($1, $2, $3, $4, $5, $6::jsonb, $7, $8, $9, $10, $11, $12, 0, now())
"#,
        )
        .bind("wss://relay.example")
        .bind(vec![0x88_u8; 32])
        .bind(vec![0x99_u8; 32])
        .bind(123_i64)
        .bind(30617_i32)
        .bind("{\"bad\":\"shape\"}")
        .bind("content")
        .bind(vec![0xaa_u8; 64])
        .bind("alice")
        .bind("demo")
        .bind("demo")
        .bind(RelayPublishStatus::Pending.as_str())
        .execute(&test_db.pool)
        .await
        .expect("insert outbox row");

        let repositories = test_db.repositories();
        let claim_at = time::OffsetDateTime::now_utc() + time::Duration::minutes(1);
        let err = repositories
            .claim_relay_publish(claim_at)
            .await
            .expect_err("claim should fail for invalid tags json");
        assert!(matches!(err, StorageError::Serialization { field: "tags", .. }));

        test_db.cleanup().await;
    }

    #[tokio::test]
    async fn postgres_relay_publish_retry_and_missing_id_db() {
        let Some(test_db) = TestDatabase::provision().await else {
            eprintln!("skipping postgres_relay_publish_retry_and_missing_id_db: postgres unavailable");
            return;
        };

        let repositories = test_db.repositories();
        let request = RelayPublishRequest {
            relay_url: "wss://relay.local".to_string(),
            event_id: "33".repeat(32),
            pubkey: "44".repeat(32),
            created_at: 123,
            kind: 30617,
            tags: vec![vec!["d".to_string(), "demo".to_string()]],
            content: String::new(),
            sig: "55".repeat(64),
            forgejo_owner: "alice".to_string(),
            forgejo_repo: "demo".to_string(),
            identifier: "demo".to_string(),
        };
        repositories
            .enqueue_relay_publish(request)
            .await
            .expect("enqueue");

        // Add a cushion so the test does not race db-side `now()` assignment under heavy load.
        let now = time::OffsetDateTime::now_utc() + time::Duration::minutes(1);
        let claimed = repositories
            .claim_relay_publish(now)
            .await
            .expect("claim")
            .expect("job");
        let retry_at = now + time::Duration::minutes(5);
        repositories
            .mark_relay_publish_failed(claimed.id, "network timeout", retry_at)
            .await
            .expect("mark failed");

        let none_before_retry = repositories
            .claim_relay_publish(now + time::Duration::minutes(1))
            .await
            .expect("claim before retry");
        assert!(none_before_retry.is_none());

        let claimed_retry = repositories
            .claim_relay_publish(now + time::Duration::minutes(6))
            .await
            .expect("claim after retry")
            .expect("retry job");
        assert!(claimed_retry.attempt_count >= 2);

        repositories
            .mark_relay_publish_succeeded(claimed_retry.id)
            .await
            .expect("mark succeeded");

        let missing_mark_success = repositories
            .mark_relay_publish_succeeded(999_999)
            .await
            .expect_err("missing success id");
        assert!(matches!(missing_mark_success, StorageError::Internal { .. }));

        let missing_mark_failed = repositories
            .mark_relay_publish_failed(
                999_998,
                "missing",
                now + time::Duration::minutes(10),
            )
            .await
            .expect_err("missing failed id");
        assert!(matches!(missing_mark_failed, StorageError::Internal { .. }));

        test_db.cleanup().await;
    }
}

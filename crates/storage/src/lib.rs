pub mod account;
pub mod cache;
pub mod command_state;
pub mod config;
pub mod error;
pub mod event;
pub mod migrations;
pub mod outbox;
pub mod postgres;
pub mod profile;
pub mod queries;
pub mod relay_compat;
pub mod relay_membership;
pub mod relay_tenant;
pub mod repo;
pub mod repo_mapping;
pub mod repositories;
#[cfg(test)]
pub(crate) mod test_support;

pub use account::AccountRecord;
pub use cache::{CacheConfig, CachedRepositories};
pub use command_state::{
    AccountLifecycle, AccountStateRecord, CommandLogRecord, CommandStatus, ProfileStateRecord,
    ProfileVisibilityV1, RepoMaintainerV1Record, RepoStateV1Record, RepoVisibilityV1,
};
pub use config::StorageConfig;
pub use error::StorageError;
pub use event::{EventQuery, EventRecord, TagRecord};
pub use migrations::{Migration, MigrationRunner};
pub use outbox::{RelayPublishEntry, RelayPublishJob, RelayPublishRequest, RelayPublishStatus};
pub use postgres::PostgresRepositories;
pub use profile::{ProfileRecord, ProfileVisibility};
pub use queries::RepoFilter;
pub use relay_compat::{RelayCompatibilityRecord, RelayProbeMetadata};
pub use relay_membership::{RelayInviteRecord, RelayMembershipRecord};
pub use relay_tenant::RelayTenantRecord;
pub use repo::{RepoAnnouncementRecord, RepoStateRecord};
pub use repo_mapping::RepoMappingRecord;
pub use repositories::{
    AccountRepository, AnnouncementRepository, EventRepository, InMemoryRepositories,
    ProfileRepository, RelayCompatibilityRepository, RelayMembershipRepository,
    RelayPublishRepository, RelayTenantRepository, RepoMappingRepository, StateRepository,
};

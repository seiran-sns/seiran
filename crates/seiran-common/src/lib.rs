pub mod crypto;
pub mod db;
pub mod id;
pub mod traits;
pub mod auth;
pub mod secrets;
pub mod queue;
pub mod jobs;
pub mod ap;
pub mod atp;
pub mod jetstream_control;
pub mod jetstream_leader;
pub mod mention;
pub mod repository;
pub mod storage;
pub mod streaming;
pub mod system_actor;
pub mod username;

pub use db::{get_db_pool, run_migrations};
pub use id::generate_snowflake_id;
pub use system_actor::{ensure_system_proxy_actor, resolve_system_proxy_actor_id};
pub use username::{is_reserved_username, is_valid_local_username, PROXY_ACTOR_USERNAME, RESERVED_LOCAL_USERNAMES};

/// プロフィールのキーバリュー項目（#62）の最大件数。Mastodon 等のデフォルト（4件）に合わせる。
pub const MAX_PROFILE_FIELDS: usize = 4;
pub use auth::{LocalAuthProvider, AuthError};
pub use auth::local::VerifiedUser;
pub use secrets::{Secrets, SecretsFile, SecretsError};
pub use queue::{create_job_queue, InMemoryJobQueue, RedisJobQueue, WorkerEngine};
pub use queue::worker::{priority as job_priority, DeliveryConfig, InboxContext, JobContext};
pub use traits::{ApDeliveryKind, Job, JobQueue, PrevApReaction};
pub use atp::{AtpCommitService, AtpCommitError, AtpCommitEvent};
pub use ap::{ApClient, ApError};
pub use streaming::{StreamEvent, StreamHub};
pub use crypto::{decrypt as crypto_decrypt, encrypt as crypto_encrypt, CryptoError};
pub use repository::{
    CreateStorageProvider, PgStorageProviderRepository, StorageProvider,
    StorageProviderError, StorageProviderRepository, UpdateStorageProvider,
};
pub use repository::{PgSiteSettingsRepository, SiteSettingsRepository};
pub use storage::{
    process_image, ImageProcessingError, MediaKind, ProcessedImage,
    S3StorageClient, S3Error, select_provider, SelectorError,
    convert_audio_to_gray_video, ext_for_mime_type, is_allowed_video_or_audio_mime,
    probe_video_or_audio, sniff_mime_type, MediaProbeError, ProbedMedia,
    AUDIO_VIDEO_HEIGHT, AUDIO_VIDEO_WIDTH,
};
pub use repository::{
    CreateMediaFile, MediaFile, MediaFileError, MediaFileRepository, PgMediaFileRepository,
};


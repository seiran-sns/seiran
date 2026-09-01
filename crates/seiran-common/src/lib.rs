pub mod advisory_lock;
pub mod ap;
pub mod atp;
pub mod auth;
pub mod avatar;
pub mod crypto;
pub mod db;
pub mod follow_exec;
pub mod follow_target;
pub mod hashtag;
pub mod id;
pub mod jetstream_control;
pub mod jetstream_leader;
pub mod jobs;
pub mod lang;
pub mod local_domain;
pub mod mention;
pub mod net;
pub mod oembed_whitelist;
pub mod queue;
pub mod rate_limit;
pub mod repository;
pub mod secrets;
pub mod storage;
pub mod streaming;
pub mod system_actor;
pub mod totp;
pub mod traits;
pub mod username;

pub use db::{get_db_pool, run_migrations};
pub use id::generate_snowflake_id;
pub use lang::{
    is_supported_display_language, is_supported_language, SUPPORTED_DISPLAY_LANGUAGES,
    SUPPORTED_LANGUAGES,
};
pub use local_domain::{domain_candidate_from_host, resolve_local_domain, LocalDomain};
pub use system_actor::{
    ensure_relay_agent_actor, ensure_system_proxy_actor, resolve_relay_agent_actor_id,
    resolve_system_proxy_actor_id,
};
pub use username::{
    is_reserved_username, is_valid_local_username, strip_local_domain_suffix, PROXY_ACTOR_USERNAME,
    RESERVED_LOCAL_USERNAMES,
};

/// プロフィールのキーバリュー項目（#62）の最大件数。Mastodon 等のデフォルト（4件）に合わせる。
pub const MAX_PROFILE_FIELDS: usize = 4;

/// プロフィールの「別のアカウント」（alsoKnownAs、AP Moveの語彙をプロフィール表示・
/// 相互検証用途に転用したseiran独自拡張）の最大登録件数。
pub const MAX_ALSO_KNOWN_AS: usize = 10;
pub use ap::{ApClient, ApError};
pub use atp::{AtpCommitError, AtpCommitEvent, AtpCommitService};
pub use auth::local::VerifiedUser;
pub use auth::{AuthError, LocalAuthProvider, VerifiedAtpAccess, VerifiedAtpRefresh};
pub use crypto::{decrypt as crypto_decrypt, encrypt as crypto_encrypt, CryptoError};
pub use queue::worker::{
    priority as job_priority, DeliveryConfig, FollowExecConfig, InboxContext, JobContext,
};
pub use queue::{create_job_queue, InMemoryJobQueue, RedisJobQueue, WorkerEngine};
pub use repository::{
    CreateMediaFile, MediaFile, MediaFileError, MediaFileRepository, PgMediaFileRepository,
    ResolvedMediaFile,
};
pub use repository::{
    CreateStorageProvider, PgStorageProviderRepository, StorageProvider, StorageProviderError,
    StorageProviderRepository, UpdateStorageProvider,
};
pub use repository::{PgSiteSettingsRepository, SiteSettingsRepository};
pub use secrets::{Secrets, SecretsError, SecretsFile};
pub use storage::{
    convert_audio_to_gray_video, ext_for_mime_type, faststart_video,
    is_allowed_video_or_audio_mime, is_faststart_eligible_mime, prepare_image,
    probe_video_or_audio, select_provider, sniff_mime_type, ExifSanitizedImage, ImagePipeline,
    ImageProcessingError, MediaKind, MediaProbeError, ProbedMedia, ProcessedImage, S3Error,
    S3StorageClient, SelectorError, AUDIO_VIDEO_HEIGHT, AUDIO_VIDEO_WIDTH,
};
pub use streaming::{StreamEvent, StreamHub};
pub use traits::{ApDeliveryKind, Job, JobQueue, PrevApReaction};

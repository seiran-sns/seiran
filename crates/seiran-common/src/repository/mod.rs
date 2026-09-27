//! リポジトリ層: データベースへの CRUD をトレイトで抽象化する。
//!
//! ハンドラ・サービスは `Arc<dyn XxxRepository>` を受け取り、SQL に直接依存しない。
//! テストでは Mock 実装を差し込める。SQL は各 `Pg*Repository` の `impl` 内にのみ記述する。

/// ID カーソルによるページ指定（snowflake ID 降順の一覧に対する`until_id`/`since_id`規約。
/// `until_id`より古い・`since_id`より新しいものを最大`limit`件）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Page {
    pub limit: i64,
    pub until_id: Option<i64>,
    pub since_id: Option<i64>,
}

pub mod actor;
pub mod actor_search;
pub mod also_known_as;
pub mod ap_public;
pub mod app_token;
pub mod at_migration;
pub mod atp;
pub mod atp_preferences;
pub mod atp_session;
pub mod auth_rate_limit;
pub mod block;
pub mod bsky_ingest;
pub mod dm;
pub mod email_change;
pub mod email_short_code;
pub mod email_verification;
pub mod emoji;
pub mod follow;
pub mod follow_import;
pub mod hashtag;
pub mod instance_domain;
pub mod link_resolution;
pub mod list;
pub mod maintenance;
pub mod media_file;
pub mod mute;
pub mod note_extras;
pub mod notification;
pub mod passkey;
pub mod password_reset;
pub mod pinned_post;
pub mod poll;
pub mod post;
pub mod post_merge;
pub mod post_search;
pub mod rate_limit_log;
pub mod reaction;
pub mod relay;
pub mod remote_emoji;
pub mod remote_follow_snapshot;
pub mod remote_instance_meta;
pub mod report;
pub mod repost_mute;
pub mod rotation_key_backfill;
pub mod site_settings;
pub mod storage_provider;
pub mod totp;
pub mod user;

pub use actor::{
    Actor, ActorProfileRow, ActorRepository, BskyActorProfile, FediActorProfile,
    LocalProfileUpdate, NewLocalActor, PgActorRepository,
};
pub use also_known_as::{AlsoKnownAsRepository, AlsoKnownAsRow, PgAlsoKnownAsRepository};
pub use app_token::{AppTokenRepository, AppTokenRow, PgAppTokenRepository};
pub use at_migration::{
    AtMigrationRepository, AtMigrationRequestRow, NewMigrationRequest, PgAtMigrationRepository,
    StagedRecord,
};
pub use atp::{AtpReadRepository, PgAtpReadRepository, RepoEvent};
pub use atp_preferences::{AtpPreferencesRepository, PgAtpPreferencesRepository};
pub use atp_session::{AppPasswordRow, AtpSessionRepository, PgAtpSessionRepository};
pub use auth_rate_limit::{AuthRateLimitRepository, IpBlockRow, PgAuthRateLimitRepository};
pub use block::{BlockRepository, BlockedActorRow, PgBlockRepository};
pub use dm::{DmPeerSummary, DmRepository, PgDmRepository};
pub use email_change::{EmailChangeRepository, PgEmailChangeRepository};
pub use email_short_code::{EmailShortCodeRepository, PgEmailShortCodeRepository};
pub use email_verification::{EmailVerificationRepository, PgEmailVerificationRepository};
pub use emoji::{
    extract_shortcode_candidates, format_local_reaction_content, format_remote_reaction_content,
    parse_custom_emoji_shortcode, parse_reaction_shortcode_and_host,
};
pub use emoji::{EmojiRepository, EmojiRow, PgEmojiRepository};
pub use follow::{FollowListRow, FollowRepository, PgFollowRepository};
pub use follow_import::{
    FollowImportItemOutcome, FollowImportProgress, FollowImportRepository, FollowImportRequestRow,
    PgFollowImportRepository,
};
pub use hashtag::{HashtagRepository, PgHashtagRepository, PinnedHashtagRow};
pub use instance_domain::{ConfirmOutcome, InstanceDomainRepository, PgInstanceDomainRepository};
pub use link_resolution::{
    LinkResolutionRepository, LinkResolutionRow, PgLinkResolutionRepository,
};
pub use list::{ListMemberRow, ListRepository, ListRow, PgListRepository};
pub use media_file::{
    CreateMediaFile, MediaFile, MediaFileError, MediaFileRepository, PgMediaFileRepository,
    ResolvedMediaFile,
};
pub use mute::{MuteRepository, MutedActorRow, PgMuteRepository};
pub use notification::{
    NewNotification, NotificationKind, NotificationRepository, NotificationRow,
    PgNotificationRepository,
};
pub use password_reset::{PasswordResetRepository, PgPasswordResetRepository};
pub use pinned_post::{PgPinnedPostsRepository, PinnedPostsRepository, MAX_PINNED_POSTS};
pub use post::{
    find_by_ids_including_deleted as find_posts_by_ids_including_deleted,
    find_visible_by_ids as find_visible_posts_by_ids, DmSessionSummary, InsertFullParams,
    InsertRemoteWithDedupParams, InsertRepostParams, PgPostRepository, PostDeleteInfo,
    PostDeliveryMeta, PostRecord, PostRepository, PostSummary, ReferenceKind, RemoteAttachment,
    RepostEntry, RepostUndoInfo, TimelinePost,
};
pub use reaction::{
    NewReaction, PgReactionRepository, ReactionFeedRow, ReactionRepository, ReactionUpsert,
    ReactorInfo, StoredReaction,
};
pub use relay::{PgRelayRepository, Relay, RelayError, RelayRepository, RelayStatus};
pub use remote_emoji::{PgRemoteEmojiRepository, RemoteEmojiRepository, RemoteEmojiRow};
pub use remote_instance_meta::{
    PgRemoteInstanceMetaRepository, RemoteInstanceMeta, RemoteInstanceMetaRepository,
};
pub use repost_mute::{PgRepostMuteRepository, RepostMuteRepository, RepostMutedActorRow};
pub use rotation_key_backfill::{PgRotationKeyBackfillRepository, RotationKeyBackfillRepository};
pub use site_settings::{PgSiteSettingsRepository, SiteSettingsRepository};
pub use storage_provider::{
    CreateStorageProvider, PgStorageProviderRepository, StorageProvider, StorageProviderError,
    StorageProviderRepository, UpdateStorageProvider,
};
pub use totp::{PgTotpRepository, TotpRepository};
pub use user::{create_local_account, AdminUserRow, LoginRow, PgUserRepository, UserRepository};

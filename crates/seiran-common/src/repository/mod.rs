//! リポジトリ層: データベースへの CRUD をトレイトで抽象化する。
//!
//! ハンドラ・サービスは `Arc<dyn XxxRepository>` を受け取り、SQL に直接依存しない。
//! テストでは Mock 実装を差し込める。SQL は各 `Pg*Repository` の `impl` 内にのみ記述する。

pub mod actor;
pub mod app_token;
pub mod atp;
pub mod auth_rate_limit;
pub mod block;
pub mod dm;
pub mod email_change;
pub mod email_verification;
pub mod emoji;
pub mod follow;
pub mod hashtag;
pub mod instance_domain;
pub mod list;
pub mod media_file;
pub mod mute;
pub mod notification;
pub mod password_reset;
pub mod pinned_post;
pub mod post;
pub mod reaction;
pub mod relay;
pub mod remote_emoji;
pub mod remote_instance_meta;
pub mod site_settings;
pub mod storage_provider;
pub mod totp;
pub mod user;

pub use actor::{Actor, ActorProfileRow, ActorRepository, PgActorRepository};
pub use app_token::{AppTokenRepository, AppTokenRow, PgAppTokenRepository};
pub use atp::{AtpReadRepository, PgAtpReadRepository, RepoEvent};
pub use auth_rate_limit::{AuthRateLimitRepository, IpBlockRow, PgAuthRateLimitRepository};
pub use block::{BlockRepository, BlockedActorRow, PgBlockRepository};
pub use dm::{DmPeerSummary, DmRepository, PgDmRepository};
pub use email_change::{EmailChangeRepository, PgEmailChangeRepository};
pub use email_verification::{EmailVerificationRepository, PgEmailVerificationRepository};
pub use emoji::{extract_shortcode_candidates, parse_custom_emoji_shortcode};
pub use emoji::{EmojiRepository, EmojiRow, PgEmojiRepository};
pub use follow::{FollowListRow, FollowRepository, PgFollowRepository};
pub use hashtag::{HashtagRepository, PgHashtagRepository, PinnedHashtagRow};
pub use instance_domain::{ConfirmOutcome, InstanceDomainRepository, PgInstanceDomainRepository};
pub use list::{ListMemberRow, ListRepository, ListRow, PgListRepository};
pub use media_file::{
    CreateMediaFile, MediaFile, MediaFileError, MediaFileRepository, PgMediaFileRepository,
};
pub use mute::{MuteRepository, MutedActorRow, PgMuteRepository};
pub use notification::{
    NotificationKind, NotificationRepository, NotificationRow, PgNotificationRepository,
};
pub use password_reset::{PasswordResetRepository, PgPasswordResetRepository};
pub use pinned_post::{PgPinnedPostsRepository, PinnedPostsRepository, MAX_PINNED_POSTS};
pub use post::{
    DmSessionSummary, InsertFullParams, InsertRemoteWithDedupParams, PgPostRepository,
    PostDeleteInfo, PostDeliveryMeta, PostRecord, PostRepository, PostSummary, RepostEntry,
    RepostUndoInfo, TimelinePost,
};
pub use reaction::{PgReactionRepository, ReactionRepository, ReactorInfo};
pub use relay::{PgRelayRepository, Relay, RelayError, RelayRepository, RelayStatus};
pub use remote_emoji::{PgRemoteEmojiRepository, RemoteEmojiRepository, RemoteEmojiRow};
pub use remote_instance_meta::{
    PgRemoteInstanceMetaRepository, RemoteInstanceMeta, RemoteInstanceMetaRepository,
};
pub use site_settings::{PgSiteSettingsRepository, SiteSettingsRepository};
pub use storage_provider::{
    CreateStorageProvider, PgStorageProviderRepository, StorageProvider, StorageProviderError,
    StorageProviderRepository, UpdateStorageProvider,
};
pub use totp::{PgTotpRepository, TotpRepository};
pub use user::{AdminUserRow, LoginRow, PgUserRepository, UserRepository};

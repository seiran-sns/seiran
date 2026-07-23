//! リポジトリ層: データベースへの CRUD をトレイトで抽象化する。
//!
//! ハンドラ・サービスは `Arc<dyn XxxRepository>` を受け取り、SQL に直接依存しない。
//! テストでは Mock 実装を差し込める。SQL は各 `Pg*Repository` の `impl` 内にのみ記述する。

pub mod actor;
pub mod atp;
pub mod block;
pub mod dm;
pub mod email_verification;
pub mod emoji;
pub mod follow;
pub mod hashtag;
pub mod list;
pub mod media_file;
pub mod mute;
pub mod notification;
pub mod password_reset;
pub mod pinned_post;
pub mod post;
pub mod reaction;
pub mod site_settings;
pub mod storage_provider;
pub mod user;

pub use actor::{Actor, ActorProfileRow, ActorRepository, PgActorRepository};
pub use atp::{AtpReadRepository, PgAtpReadRepository, RepoEvent};
pub use block::{BlockRepository, BlockedActorRow, PgBlockRepository};
pub use dm::{DmPeerSummary, DmRepository, PgDmRepository};
pub use email_verification::{EmailVerificationRepository, PgEmailVerificationRepository};
pub use emoji::{EmojiRepository, EmojiRow, PgEmojiRepository};
pub use follow::{FollowRepository, PgFollowRepository};
pub use mute::{MuteRepository, MutedActorRow, PgMuteRepository};
pub use hashtag::{HashtagRepository, PgHashtagRepository, PinnedHashtagRow};
pub use list::{
    ListMemberRow, ListRepository, ListRow, PgListRepository, MAX_LISTS_PER_OWNER,
    MAX_MEMBERS_PER_LIST,
};
pub use media_file::{
    CreateMediaFile, MediaFile, MediaFileError, MediaFileRepository, PgMediaFileRepository,
};
pub use notification::{NotificationKind, NotificationRepository, NotificationRow, PgNotificationRepository};
pub use password_reset::{PasswordResetRepository, PgPasswordResetRepository};
pub use pinned_post::{PgPinnedPostsRepository, PinnedPostsRepository, MAX_PINNED_POSTS};
pub use post::{
    DmSessionSummary, InsertFullParams, InsertRemoteWithDedupParams, PgPostRepository, PostDeleteInfo,
    PostDeliveryMeta, PostRecord, PostRepository, PostSummary, RepostUndoInfo, TimelinePost,
};
pub use reaction::{PgReactionRepository, ReactionRepository, ReactorInfo};
pub use emoji::parse_custom_emoji_shortcode;
pub use site_settings::{PgSiteSettingsRepository, SiteSettingsRepository};
pub use storage_provider::{
    CreateStorageProvider, PgStorageProviderRepository, StorageProvider,
    StorageProviderError, StorageProviderRepository, UpdateStorageProvider,
};
pub use user::{AdminUserRow, LoginRow, PgUserRepository, UserRepository};

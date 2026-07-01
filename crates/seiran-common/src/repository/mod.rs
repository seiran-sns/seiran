//! リポジトリ層: データベースへの CRUD をトレイトで抽象化する。
//!
//! ハンドラ・サービスは `Arc<dyn XxxRepository>` を受け取り、SQL に直接依存しない。
//! テストでは Mock 実装を差し込める。SQL は各 `Pg*Repository` の `impl` 内にのみ記述する。

pub mod actor;
pub mod atp;
pub mod follow;
pub mod media_file;
pub mod post;
pub mod storage_provider;
pub mod user;

pub use actor::{Actor, ActorRepository, PgActorRepository};
pub use atp::{AtpReadRepository, PgAtpReadRepository, RepoEvent};
pub use follow::{FollowRepository, PgFollowRepository};
pub use media_file::{
    CreateMediaFile, MediaFile, MediaFileError, MediaFileRepository, PgMediaFileRepository,
};
pub use post::{PgPostRepository, PostRecord, PostRepository, PostSummary, TimelinePost};
pub use storage_provider::{
    CreateStorageProvider, PgStorageProviderRepository, StorageProvider,
    StorageProviderError, StorageProviderRepository, UpdateStorageProvider,
};
pub use user::{LoginRow, PgUserRepository, UserRepository};

//! ActivityPub 投稿配送モジュール
//!
//! ローカルユーザーのアクティビティ（Create/Announce/Undo/Update/Delete/リアクション）を
//! AP フォロワーの inbox へ HTTP Signatures 付きで配送する。
//!
//! # 構成（how/what 分離）
//! - `infra`: inbox 解決・並列 fan-out（配送の共通機構、how）
//! - `activity`: アクティビティ JSON の組み立て（DB・ネットワーク非依存の純関数、what）
//! - `note` / `announce` / `actor` / `reaction`: 種別ごとの配送オーケストレーション（how）
//! - `text`: 本文の HTML 変換（プレーンテキスト/メンション → HTML）

mod activity;
mod actor;
mod announce;
mod infra;
mod note;
mod reaction;
mod text;

pub use activity::{append_emoji_tags, apply_poll_to_note_object};
pub use actor::{deliver_delete_actor, deliver_update_actor};
pub use announce::{deliver_ap_announce, deliver_undo_announce};
pub use note::{
    deliver_delete_note, deliver_direct_message_to_ap, deliver_post_to_ap_followers,
    deliver_seiranpost_update,
};
pub use reaction::{deliver_ap_poll_vote, deliver_ap_reaction, deliver_ap_undo_reaction};
pub use text::{at_uri_to_bsky_app_url, plain_to_html, plain_to_html_with_mentions};

use futures_util::stream::{self, StreamExt};
use sqlx::{PgPool, Row};

use super::client::{ApClient, ApError};

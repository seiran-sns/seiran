//! ノート（投稿）関連ハンドラ。
//!
//! - `dto`: リクエスト/レスポンス型と DB 行 → レスポンスの素朴な変換
//! - `queries`: 複数ポストへの添付・リアクション・リポスト状態の一括解決（読み取り集約）
//! - `delivery`: Fedi（AP）/ Bsky（ATP）への配送オーケストレーション
//! - `validation`: 本文長・添付件数・リアクション内容の検証
//! - `creation`: 投稿作成（通常投稿・リポスト）のオーケストレーション
//! - `timelines`: home/local/social/global タイムライン取得
//! - `retrieval`: 単一ノート・スレッド文脈・返信一覧の取得（AP直接取得含む）
//! - `deletion`: 投稿・リポストの削除
//! - `reactions`: リアクション（作成・削除・集計）とリポスト一覧
//! - `pins`: プロフィールへのピン留め
//! - `poll`: アンケート投票
//! - `profile_material`: Bskyピン留め投稿・ATPプロフィール項目の取得（プロフィール表示用）
//!
//! このファイル（`mod.rs`）自体は各サブモジュールの宣言・再エクスポートと、
//! 全サブモジュールで共有する `use` のみを持つ。

pub mod creation;
pub mod deletion;
pub mod delivery;
pub mod dto;
pub mod pins;
pub mod poll;
pub mod profile_material;
pub mod queries;
pub mod reactions;
pub mod retrieval;
pub mod timelines;
pub mod validation;

pub use creation::create_note;
pub use deletion::{delete_note, delete_repost};
pub use dto::to_note_response;
pub use dto::{AttachmentResponse, LinkCardResponse, NoteResponse, ReactRequest, ReactionSummary};
pub use dto::{to_reaction_event_response, ProfileFeedItem, ReactionEventResponse};
pub use pins::{pin_note, unpin_note};
pub use poll::vote_poll;
pub(crate) use profile_material::fetch_atp_profile_material;
pub use profile_material::resolve_bsky_pinned_post;
pub use queries::{
    attach_poll_votes, attach_remote_instance_info, build_instance_cache, embed_quotes,
    embed_renotes, fetch_attachments_map, fetch_link_cards_map, fetch_reactions_map,
    resolve_mention_facets_in_place,
};
pub use reactions::{create_reaction, delete_reaction, frequent_reactions, note_reposts, reaction_actors};
pub use retrieval::{
    get_announce_redirect, get_note, get_note_ap, note_context, note_replies,
    resolve_note_reference,
};
pub use timelines::{global_timeline, home_timeline, local_timeline, social_timeline};
pub use validation::BSKY_MAX_TEXT_GRAPHEMES;

use std::collections::HashMap;
use std::sync::Arc;

use axum::{
    extract::{Path, Query, State},
    response::{IntoResponse, Response},
    Json,
};
use sqlx::Row;

use seiran_common::repository::{
    extract_shortcode_candidates, Actor, InsertFullParams, InsertRepostParams, NotificationKind,
    TimelinePost,
};
use seiran_common::streaming::{broadcast_poll_update, broadcast_reaction_update};
use seiran_common::{
    ap::{fetch_ap_history, plain_to_html_with_mentions},
    generate_snowflake_id,
    mention::{convert_mentions_for_bsky, extract_local_mention_actor_ids},
    ApDeliveryKind, PrevApReaction,
};

use crate::error::ApiError;
use crate::middleware::{AuthedUser, MaybeAuthedUser};
use crate::AppState;

use dto::{
    CreateNoteRequest, NoteContextResponse, NoteRepliesResponse, NoteUserInfo, TimelineQuery,
};

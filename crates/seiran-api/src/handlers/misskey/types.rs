//! Misskey 本家の `Note`/`UserLite`/`UserDetailed` に合わせたレスポンス型。
//!
//! `handlers::notes::NoteResponse` 等の既存カスタム型とは別物。フィールド名はすべて
//! Misskey 本家スキーマの camelCase に合わせる。現状は全フィールドの完全再現ではなく、
//! クライアントの基本描画（タイムライン・リアクション・フォロー）に必要な部分集合。

use std::collections::BTreeMap;

use serde::Serialize;

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct MisskeyUserLite {
    pub id: String,
    pub username: String,
    /// ローカルユーザーは `null`。
    pub host: Option<String>,
    pub name: Option<String>,
    pub avatar_url: Option<String>,
    pub is_bot: bool,
    pub is_cat: bool,
}

/// `misskey_dart` の `UserDetailedNotMe.fromJson`（`/api/users/show` が返す形）は
/// `followersCount`/`followingCount`/`notesCount` を non-nullable `int` として直接
/// キャストするため、欠けると Dart 側で `TypeError`（`type 'Null' is not a subtype of
/// type 'num' in type cast`）となる（実機で確認済み。`MisskeyDriveFile` と同種の問題）。
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct MisskeyUserDetailed {
    #[serde(flatten)]
    pub lite: MisskeyUserLite,
    pub created_at: String,
    pub description: Option<String>,
    pub banner_url: Option<String>,
    pub is_locked: bool,
    pub is_silenced: bool,
    pub is_suspended: bool,
    pub notes_count: i64,
    pub followers_count: i64,
    pub following_count: i64,
    /// フォロー/フォロワー一覧・数の公開範囲（本家 Misskey の設定機能に相当）。
    /// seiran はこの設定自体に未対応なため常に `"public"` を返す。クライアント
    /// （`misskey_dart` 等）はこの値が欠落していると非公開とみなし、
    /// `followersCount`/`followingCount` の数値表示を鍵アイコンに置き換える
    /// （実機で確認済み。値自体は正しく集計されているのに表示されない不具合の原因）。
    pub followers_visibility: String,
    pub following_visibility: String,
}

/// `/api/i`（自分自身）専用のレスポンス型。`UserDetailedNotMe` を返す `/api/users/show` とは
/// 別の、Misskey 本家の `MeDetailed` スキーマに合わせた自分専用フィールドを追加で持つ。
/// `misskey_dart`（Aria 等が使用）の生成コードは `notesCount`/`isModerator`/`isAdmin`/
/// `alwaysMarkNsfw`/`carefulBot`/`autoAcceptFollowed` を non-nullable 必須として直接
/// キャストするため、欠けると Dart 側で `TypeError`（例:
/// `type 'Null' is not a subtype of type 'num' in type cast`）となり未処理例外で
/// クライアントがフリーズする（実機で確認済み）。
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct MisskeyMeDetailed {
    #[serde(flatten)]
    pub detailed: MisskeyUserDetailed,
    pub is_moderator: bool,
    pub is_admin: bool,
    pub always_mark_nsfw: bool,
    pub careful_bot: bool,
    pub auto_accept_followed: bool,
}

/// `misskey_dart` の `DriveFile.fromJson` は `id`/`createdAt`/`name`/`type`/`md5`/`size`/
/// `isSensitive`/`properties`/`url` を non-nullable 必須としてキャストするため、欠けると
/// `MisskeyMeDetailed` と同様に Dart 側で `TypeError` となりタイムライン取得が例外落ちする
/// （実機で確認済み）。`md5` は seiran 内部で持つ `sha256` を代用する（クライアントは値の
/// 妥当性を検証せず単に文字列として保持するだけのため実害はない）。
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct MisskeyDriveFile {
    pub id: String,
    pub created_at: String,
    pub name: String,
    #[serde(rename = "type")]
    pub file_type: String,
    pub md5: String,
    pub size: i64,
    pub is_sensitive: bool,
    pub properties: MisskeyDriveFileProperties,
    pub url: String,
    pub thumbnail_url: String,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct MisskeyDriveFileProperties {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<i32>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct MisskeyNote {
    pub id: String,
    pub created_at: String,
    pub text: Option<String>,
    /// Content Warning。seiran は CW 未対応のため常に `null`。
    pub cw: Option<String>,
    pub user_id: String,
    pub user: MisskeyUserLite,
    pub reply_id: Option<String>,
    /// Misskey は「リノート」と「引用」を区別せず、どちらも `renoteId` + (引用なら)非空の
    /// `text` として表現する。seiran 内部の `repost_of_post_id`（リポスト）と
    /// `quote_of_post_id`（引用）はどちらもここに統合する。
    pub renote_id: Option<String>,
    /// seiran の可視性（`public`/`unlisted`/`followers_only`/`direct`）を Misskey 本家の語彙
    /// （`public`/`home`/`followers`/`specified`）にマッピングした値（`to_misskey_visibility`）。
    pub visibility: String,
    pub file_ids: Vec<String>,
    pub files: Vec<MisskeyDriveFile>,
    pub tags: Vec<String>,
    /// 本文中のカスタム絵文字インライン表示用（shortcode→url）。seiran は未対応のため常に空。
    pub emojis: BTreeMap<String, String>,
    /// 絵文字 → 件数。
    pub reactions: BTreeMap<String, i64>,
    /// カスタム絵文字リアクション（`:shortcode:`）→画像URL。本家 Misskey の
    /// `NoteEntityService`（`reactionEmojis: populateEmojis(reactionEmojiNames, host)`）に
    /// 相当。Unicode絵文字のリアクションはここに現れない（クライアント側はそのまま
    /// テキストとして描画する）。
    pub reaction_emojis: BTreeMap<String, String>,
    /// リノート元/引用元ノートの本体。`renoteId` はあるがこれが `null` のままだと、
    /// `misskey_dart` 等のクライアントは元ノートを解決できず「削除されたノート」の
    /// プレースホルダーを描画する（実機で確認済み）。孫リノート（リノートのリノート）は
    /// 埋め込まない（`embed_renotes` 参照、無限再帰・多段フェッチを避けるため）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub renote: Option<Box<MisskeyNote>>,
    pub renote_count: i64,
    pub replies_count: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uri: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// 認証ユーザーが付けたリアクション（絵文字）。未認証・未リアクション時は `null`。
    pub my_reaction: Option<String>,
}

/// `POST /api/i/notifications` のレスポンス要素。Misskey 本家の `Notification` エンティティ
/// （`packed 'Notification'`）に合わせる。型ごとに存在するフィールドが異なるため
/// 全フィールド `Option`（`None` は省略）にしている。
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct MisskeyNotification {
    pub id: String,
    pub created_at: String,
    #[serde(rename = "type")]
    pub kind: String,
    /// `notifierId` 相当。Misskey 本家は `userId` というフィールド名で通知の起点ユーザーIDを返す。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user: Option<MisskeyUserLite>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<MisskeyNote>,
    /// `type == "reaction"` の場合のみ。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reaction: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `misskey_dart` の `MeDetailed.fromJson`（Aria 等が `/api/i` のレスポンスをパースする際に
    /// 使用）が non-nullable 必須として直接キャストするフィールド一覧。1つでも欠けると
    /// Dart 側で未処理の `TypeError` となりアプリがフリーズする（実機で確認済みの回帰）。
    #[test]
    fn me_detailed_includes_all_misskey_dart_required_fields() {
        let me = MisskeyMeDetailed {
            detailed: MisskeyUserDetailed {
                lite: MisskeyUserLite {
                    id: "1".to_owned(),
                    username: "alice".to_owned(),
                    host: None,
                    name: None,
                    avatar_url: None,
                    is_bot: false,
                    is_cat: false,
                },
                created_at: "2026-01-01T00:00:00+00:00".to_owned(),
                description: None,
                banner_url: None,
                is_locked: false,
                is_silenced: false,
                is_suspended: false,
                notes_count: 0,
                followers_count: 0,
                following_count: 0,
                followers_visibility: "public".to_owned(),
                following_visibility: "public".to_owned(),
            },
            is_moderator: false,
            is_admin: false,
            always_mark_nsfw: false,
            careful_bot: false,
            auto_accept_followed: false,
        };
        let value = serde_json::to_value(&me).unwrap();
        for key in [
            "id",
            "username",
            "isBot",
            "isCat",
            "createdAt",
            "isLocked",
            "isSilenced",
            "isSuspended",
            "notesCount",
            "isModerator",
            "isAdmin",
            "alwaysMarkNsfw",
            "carefulBot",
            "autoAcceptFollowed",
        ] {
            assert!(
                value.get(key).is_some_and(|v| !v.is_null()),
                "必須フィールド `{key}` が欠けているか null です（misskey_dart 側で TypeError の原因になる）"
            );
        }
    }
}

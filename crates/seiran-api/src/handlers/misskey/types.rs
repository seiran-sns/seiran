//! Misskey 本家の `Note`/`UserLite`/`UserDetailed` に合わせたレスポンス型。
//!
//! `handlers::notes::NoteResponse` 等の既存カスタム型とは別物。フィールド名はすべて
//! Misskey 本家スキーマの camelCase に合わせる。現状は全フィールドの完全再現ではなく、
//! クライアントの基本描画（タイムライン・リアクション・フォロー）に必要な部分集合。

use std::collections::BTreeMap;

use serde::Serialize;

use crate::handlers::notes::dto::RemoteInstanceInfo;

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
    /// `name`（表示名）中のカスタム絵文字（`:shortcode:`）→画像URLマップ（#186）。
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub emojis: BTreeMap<String, String>,
    /// Misskey本家準拠のリモートインスタンス情報（`host`がある場合のみ設定されうる）。
    /// 現状 `to_misskey_note` 経由（notes系エンドポイント）でのみ埋まる。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instance: Option<RemoteInstanceInfo>,
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
    /// 閲覧者との関係情報。`viewer_actor_id` が解決できる場合（ログイン済み）のみ `Some`。
    /// `None` の場合 `#[serde(flatten)]` によりJSON上 `isFollowing` 等のキー自体が現れず、
    /// misskey_dart の `UserDetailed.fromJson` はこれを見て `UserDetailedNotMe`
    /// （関係情報なし）としてパースする（`isFollowing` キーの有無で分岐、値のnull/非nullでは
    /// ない）。詳細は `MisskeyUserRelations` のコメント参照。
    #[serde(flatten)]
    pub relations: Option<MisskeyUserRelations>,
}

/// `UserDetailedNotMeWithRelations`（本家Misskey）の関係情報部分。misskey_dart側は
/// これらを `required bool` として直接キャストするため、`isFollowing` キーが存在する
/// 場合は値が `null` であってはならない（`MisskeyDriveFile` 等と同種の non-nullable
/// 直接キャスト問題）。`hasPendingFollowRequestToYou` は常に `false`（seiranはローカル
/// アカウントの鍵アカウント機能自体を持たず、ローカルviewerへの受信フォローは常に
/// 即accepted。「pending」が発生しうるのは自分からリモート鍵アカウントへ申請中の
/// ケースのみ＝`hasPendingFollowRequestFromYou`側）。
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct MisskeyUserRelations {
    pub is_following: bool,
    pub is_followed: bool,
    pub has_pending_follow_request_from_you: bool,
    pub has_pending_follow_request_to_you: bool,
    pub is_blocking: bool,
    pub is_blocked: bool,
    pub is_muted: bool,
    pub is_renote_muted: bool,
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
    /// Content Warning。`posts.content_warning` をそのまま返す（プレーンリポストは通常CWを
    /// 持たないため `None` のまま）。
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
    /// 埋め込まない（`embed_referenced_notes` 参照、無限再帰・多段フェッチを避けるため）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub renote: Option<Box<MisskeyNote>>,
    /// 返信先ノートの本体。`replyId` はあるがこれが `null` のままだと、`renote` と同様に
    /// クライアントが「削除されたノート」のプレースホルダーを描画する（実機で確認済み）。
    /// 孫リプライ（返信先の、さらにその返信先）は埋め込まない。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reply: Option<Box<MisskeyNote>>,
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
    /// `type == "moveRefollowed" | "moveAlreadyFollowing"`（seiran独自拡張、ActivityPub
    /// Move受信時の再フォロー通知）の場合のみ。移転先アクターのID。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub related_user_id: Option<String>,
    /// 上記と同じく独自拡張専用。移転先アクターの詳細。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub related_user: Option<MisskeyUserLite>,
}

/// `POST /api/users/following`（フォロー中一覧）の要素。Misskey 本家の `Following` エンティティ
/// に合わせる（`followee`）。`POST /api/users/followers`（`follower`）と共通の形にするため
/// `subject` という名前で持ち、`endpoints.rs` 側でシリアライズ直前に `followee`/`follower` の
/// どちらのキー名で出すかを切り替える（`#[serde(flatten)]` ではキー名を動的にできないため）。
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct MisskeyFollowRelation {
    pub id: String,
    pub created_at: String,
    pub followee_id: String,
    pub follower_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub followee: Option<MisskeyUserDetailed>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub follower: Option<MisskeyUserDetailed>,
}

/// `POST /api/notes/reactions`（リアクションしたユーザー一覧）の要素。Misskey 本家の
/// `NoteReaction` エンティティに合わせる。
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct MisskeyNoteReaction {
    pub id: String,
    pub created_at: String,
    pub user: MisskeyUserLite,
    #[serde(rename = "type")]
    pub kind: String,
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
                    emojis: BTreeMap::new(),
                    instance: None,
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
                relations: None,
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

    fn base_user_detailed(relations: Option<MisskeyUserRelations>) -> MisskeyUserDetailed {
        MisskeyUserDetailed {
            lite: MisskeyUserLite {
                id: "1".to_owned(),
                username: "alice".to_owned(),
                host: None,
                name: None,
                avatar_url: None,
                is_bot: false,
                is_cat: false,
                emojis: BTreeMap::new(),
                instance: None,
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
            relations,
        }
    }

    /// `relations: None`（未ログイン等でviewerが解決できない場合）は、`isFollowing` 等の
    /// キー自体がJSON上に一切現れないこと（misskey_dart はキーの有無で
    /// `UserDetailedNotMe`/`UserDetailedNotMeWithRelations` を分岐するため）。
    #[test]
    fn user_detailed_omits_relation_keys_when_relations_is_none() {
        let value = serde_json::to_value(base_user_detailed(None)).unwrap();
        for key in [
            "isFollowing",
            "isFollowed",
            "hasPendingFollowRequestFromYou",
            "hasPendingFollowRequestToYou",
            "isBlocking",
            "isBlocked",
            "isMuted",
            "isRenoteMuted",
        ] {
            assert!(
                value.get(key).is_none(),
                "relations が None の時は `{key}` キー自体が存在してはならない"
            );
        }
    }

    /// `relations: Some(...)` の時は `isFollowing` 等がフラットに展開され、かつ
    /// non-nullable bool として（null にならず）出力されること。
    #[test]
    fn user_detailed_flattens_relation_keys_when_relations_is_some() {
        let relations = MisskeyUserRelations {
            is_following: true,
            is_followed: false,
            has_pending_follow_request_from_you: false,
            has_pending_follow_request_to_you: false,
            is_blocking: false,
            is_blocked: false,
            is_muted: false,
            is_renote_muted: false,
        };
        let value = serde_json::to_value(base_user_detailed(Some(relations))).unwrap();
        assert_eq!(value.get("isFollowing"), Some(&serde_json::json!(true)));
        for key in [
            "isFollowed",
            "hasPendingFollowRequestFromYou",
            "hasPendingFollowRequestToYou",
            "isBlocking",
            "isBlocked",
            "isMuted",
            "isRenoteMuted",
        ] {
            assert!(
                value.get(key).is_some_and(|v| v.is_boolean()),
                "relations が Some の時は `{key}` が non-null な bool でなければならない"
            );
        }
        // ネストしたオブジェクトとしてではなく、親レベルにフラットに展開されていること。
        assert!(value.get("relations").is_none());
    }

    /// Aria が固定している `misskey_dart` の `Following.fromJson` は、関連先の詳細が
    /// nullable でも `followeeId` と `followerId` 自体は non-nullable として読む。
    #[test]
    fn follow_relation_includes_both_required_actor_ids() {
        let relation = MisskeyFollowRelation {
            id: "3".to_owned(),
            created_at: "2026-01-01T00:00:00+00:00".to_owned(),
            followee_id: "2".to_owned(),
            follower_id: "1".to_owned(),
            followee: None,
            follower: None,
        };
        let value = serde_json::to_value(&relation).unwrap();

        assert_eq!(value["followeeId"], "2");
        assert_eq!(value["followerId"], "1");
    }
}

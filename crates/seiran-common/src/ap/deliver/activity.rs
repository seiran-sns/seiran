use super::infra::LocalActorAddress;
use super::note::fetch_attachment_documents;
use super::*;
use crate::repository::parse_reaction_shortcode_and_host;

// =====================================================================
// アクティビティ構築（what: 純関数・テスト対象）
// =====================================================================

/// AS2 の Public コレクション URI（`to`/`cc` に載せることで公開範囲を示す）。
const AS_PUBLIC: &str = "https://www.w3.org/ns/activitystreams#Public";

/// Create(Note) アクティビティの構築パラメータ。
pub(super) struct NoteActivityParams<'a> {
    pub(super) local_domain: &'a str,
    pub(super) post_id: i64,
    pub(super) content_html: &'a str,
    pub(super) published: &'a str,
    pub(super) attachments: Vec<serde_json::Value>,
    pub(super) quote_url: Option<&'a str>,
    pub(super) in_reply_to: Option<&'a str>,
    pub(super) seiran_uuid: Option<&'a str>,
    /// `seiranPost`拡張オブジェクト（他seiranサーバー間の投稿完全再現、#237）。
    /// `Some`の場合、Note object に `seiranPost` フィールドとして丸ごと埋め込む。
    pub(super) seiran_post: Option<crate::seiran_post::SeiranPost>,
    /// "public" | "unlisted" | "followers_only" | "direct"。to/cc の組み立てに使う
    /// （受信側の `classify_ap_visibility` と対称なマッピング）。
    pub(super) visibility: &'a str,
    /// 本文中のメンションから組み立てた `tag[]`（`{"type":"Mention","href":..,"name":..}`）。
    /// 空なら Note オブジェクトに `tag` フィールド自体を含めない。
    pub(super) tag: Vec<serde_json::Value>,
    /// `visibility="direct"`（DM）の場合の宛先アクターURI一覧。`to` に直接使う
    /// （フォロワーコレクションではなく実際の宛先個人のみへ配送するため）。
    /// direct以外では無視される。
    pub(super) direct_recipients: &'a [String],
    /// 本文中でメンションしたアクターURI一覧（`direct`以外の可視性で`to`に追加する）。
    /// フォロワーでない相手にもメンション通知の元となるアクティビティ自体を届けるため
    /// （`to`に含めるのはAP的な作法・実際の配送先は別途 `deliver_post_to_ap_followers` が解決する）。
    /// directでは無視される（`direct_recipients`が既に実際の宛先そのもののため）。
    pub(super) mention_recipients: &'a [String],
    /// アンケート（#228）。`Some`の場合、Note objectを`Question`型に切り替え`oneOf`/`anyOf`・
    /// `endTime`を組み立てる。`{multiple, options:[{name,votes}], endTime}`の形。
    pub(super) poll: Option<&'a serde_json::Value>,
    /// CW（閲覧注意）ガイド文（#229）。`Some`の場合、`summary`フィールドとして設定する
    /// （Mastodon/Misskey互換のCW表現。本文・添付・アンケート・引用は通常通り配送する）。
    pub(super) content_warning: Option<&'a str>,
}

/// 可視性から Create(Note)/Note 共通の to/cc を決める。
pub(super) fn visibility_to_to_cc(
    addr: &LocalActorAddress,
    visibility: &str,
    direct_recipients: &[String],
    mention_recipients: &[String],
) -> (Vec<String>, Vec<String>) {
    match visibility {
        "unlisted" => {
            let mut to = vec![addr.followers_uri.clone()];
            to.extend(mention_recipients.iter().cloned());
            (to, vec![AS_PUBLIC.to_string()])
        }
        // DMは実際の宛先個人のみへ配送する（フォロワーコレクション宛にはしない）。
        "direct" => (direct_recipients.to_vec(), vec![]),
        "followers_only" => {
            let mut to = vec![addr.followers_uri.clone()];
            to.extend(mention_recipients.iter().cloned());
            (to, vec![])
        }
        _ => {
            // メンション先はAP的な作法（Mastodon等）に合わせ cc ではなく to に含める。
            let mut to = vec![AS_PUBLIC.to_string()];
            to.extend(mention_recipients.iter().cloned());
            (to, vec![addr.followers_uri.clone()])
        }
    }
}

/// `note_obj` をアンケート付き投稿用に`Question`型へ書き換える（#228）。
/// `poll`は`{multiple, options:[{name,votes}], endTime}`（`posts.poll`と同じ形、
/// 受信側`normalize_ap_poll`と対称の構築）。
/// Create 配送だけでなく、`GET /notes/:id` の AP レスポンス（`handlers::notes::get_note_ap`）
/// からもフォロー関係なしの直接取得時に同じ変換が必要なため公開する。
pub fn apply_poll_to_note_object(note_obj: &mut serde_json::Value, poll: &serde_json::Value) {
    note_obj["type"] = serde_json::Value::String("Question".to_string());
    let multiple = poll["multiple"].as_bool().unwrap_or(false);
    let choices: Vec<serde_json::Value> = poll["options"]
        .as_array()
        .map(|options| {
            options
                .iter()
                .filter_map(|o| {
                    let name = o["name"].as_str()?;
                    let votes = o["votes"].as_i64().unwrap_or(0);
                    Some(serde_json::json!({
                        "type": "Note",
                        "name": name,
                        "replies": {
                            "type": "Collection",
                            "totalItems": votes
                        }
                    }))
                })
                .collect()
        })
        .unwrap_or_default();
    let key = if multiple { "anyOf" } else { "oneOf" };
    note_obj[key] = serde_json::Value::Array(choices);
    if let Some(end_time) = poll["endTime"].as_str() {
        note_obj["endTime"] = serde_json::Value::String(end_time.to_string());
    }
}

/// Create(Note)/Update(Note) 共通の Note オブジェクトを組み立てる（what: 純関数）。
fn build_note_object(addr: &LocalActorAddress, p: &NoteActivityParams) -> serde_json::Value {
    let note_id = format!("https://{}/notes/{}", p.local_domain, p.post_id);
    let (to, cc) = visibility_to_to_cc(
        addr,
        p.visibility,
        p.direct_recipients,
        p.mention_recipients,
    );

    let mut note_obj = serde_json::json!({
        "type": "Note",
        "id": note_id,
        "attributedTo": addr.actor_uri,
        "content": p.content_html,
        "published": p.published,
        "to": to,
        "cc": cc,
        "url": note_id
    });
    if !p.attachments.is_empty() {
        note_obj["attachment"] = serde_json::Value::Array(p.attachments.clone());
    }
    if !p.tag.is_empty() {
        note_obj["tag"] = serde_json::Value::Array(p.tag.clone());
    }
    if let Some(q_url) = p.quote_url {
        note_obj["quoteUrl"] = serde_json::Value::String(q_url.to_string());
        note_obj["_misskey_quote"] = serde_json::Value::String(q_url.to_string());
    }
    // リプライ先の AP Note URI（#38: これが無いとリモートで単独ポストに見える）
    if let Some(irt) = p.in_reply_to {
        note_obj["inReplyTo"] = serde_json::Value::String(irt.to_string());
    }
    if let Some(uuid) = p.seiran_uuid {
        note_obj["seiranUuid"] = serde_json::Value::String(uuid.to_string());
    }
    if let Some(sp) = &p.seiran_post {
        note_obj["seiranPost"] = sp.to_value();
    }
    if let Some(poll) = p.poll {
        apply_poll_to_note_object(&mut note_obj, poll);
    }
    if let Some(cw) = p.content_warning {
        note_obj["summary"] = serde_json::Value::String(cw.to_string());
    }
    note_obj
}

/// Create(Note) アクティビティを組み立てる。
pub(super) fn build_create_note_activity(
    addr: &LocalActorAddress,
    p: &NoteActivityParams,
) -> serde_json::Value {
    let activity_id = format!("https://{}/activities/{}", p.local_domain, p.post_id);
    let (to, cc) = visibility_to_to_cc(
        addr,
        p.visibility,
        p.direct_recipients,
        p.mention_recipients,
    );
    let note_obj = build_note_object(addr, p);

    serde_json::json!({
        "@context": "https://www.w3.org/ns/activitystreams",
        "type": "Create",
        "id": activity_id,
        "actor": addr.actor_uri,
        "published": p.published,
        "to": to,
        "cc": cc,
        "object": note_obj
    })
}

/// Update(Note) アクティビティを組み立てる（#237、狭いスコープの`seiranPost.counterpartPostId`
/// 補完専用）。Note オブジェクト自体は`build_create_note_activity`と同一の組み立てを使うため、
/// `p.seiran_post`に確定済みの`counterpartPostId`を積んで渡すこと。
/// アクティビティIDはCreateのもの（`/activities/{post_id}`）とは別の値にする
/// （同一IDのUpdateはMastodon等の実装で「本文編集」の再取得トリガーと解釈されうるため、
/// 意図的に区別する）。
pub(super) fn build_update_note_activity(
    addr: &LocalActorAddress,
    p: &NoteActivityParams,
    published: &str,
) -> serde_json::Value {
    let activity_id = format!(
        "https://{}/activities/seiranpost-update-{}",
        p.local_domain, p.post_id
    );
    let (to, cc) = visibility_to_to_cc(
        addr,
        p.visibility,
        p.direct_recipients,
        p.mention_recipients,
    );
    let note_obj = build_note_object(addr, p);

    serde_json::json!({
        "@context": "https://www.w3.org/ns/activitystreams",
        "type": "Update",
        "id": activity_id,
        "actor": addr.actor_uri,
        "published": published,
        "to": to,
        "cc": cc,
        "object": note_obj
    })
}

/// 添付ファイル 1 件分の AP Document オブジェクトを組み立てる。
pub(super) fn build_attachment_document(
    public_url: &str,
    storage_key: &str,
    mime_type: &str,
    width: Option<i32>,
    height: Option<i32>,
    blurhash: Option<&str>,
) -> serde_json::Value {
    let url = format!("{}/{}", public_url.trim_end_matches('/'), storage_key);
    let mut doc = serde_json::json!({
        "type": "Document",
        "mediaType": mime_type,
        "url": url,
    });
    if let (Some(w), Some(h)) = (width, height) {
        doc["width"] = serde_json::json!(w);
        doc["height"] = serde_json::json!(h);
    }
    if let Some(bh) = blurhash {
        doc["blurhash"] = serde_json::json!(bh);
    }
    doc
}

/// Announce アクティビティを組み立てる。`visibility` はリポスト自身の可視性
/// （"public"|"unlisted"、`create_repost` が元ポストから継承した値）。
pub(super) fn build_announce_activity(
    addr: &LocalActorAddress,
    announce_id: &str,
    original_ap_object_id: &str,
    published: &str,
    visibility: &str,
) -> serde_json::Value {
    let (to, cc) = visibility_to_to_cc(addr, visibility, &[], &[]);
    serde_json::json!({
        "@context": "https://www.w3.org/ns/activitystreams",
        "type": "Announce",
        "id": announce_id,
        "actor": addr.actor_uri,
        "published": published,
        "to": to,
        "cc": cc,
        "object": original_ap_object_id
    })
}

/// Undo(Announce) アクティビティを組み立てる。
pub(super) fn build_undo_announce_activity(
    addr: &LocalActorAddress,
    undo_id: &str,
    announce_id: &str,
    original_ap_object_id: &str,
    published: &str,
) -> serde_json::Value {
    serde_json::json!({
        "@context": "https://www.w3.org/ns/activitystreams",
        "type": "Undo",
        "id": undo_id,
        "actor": addr.actor_uri,
        "published": published,
        "to": ["https://www.w3.org/ns/activitystreams#Public"],
        "cc": [addr.followers_uri],
        "object": {
            "type": "Announce",
            "id": announce_id,
            "actor": addr.actor_uri,
            "object": original_ap_object_id
        }
    })
}

/// Delete(Note) アクティビティを組み立てる。
/// Bsky リモートポストのリポスト取り消し（Announce を送っていないケース）で、
/// `PostToFollowers` フォールバックで作成した Note 自体を撤回するために使う。
pub(super) fn build_delete_note_activity(
    addr: &LocalActorAddress,
    note_id: &str,
    activity_id: &str,
    published: &str,
) -> serde_json::Value {
    serde_json::json!({
        "@context": "https://www.w3.org/ns/activitystreams",
        "type": "Delete",
        "id": activity_id,
        "actor": addr.actor_uri,
        "published": published,
        "to": ["https://www.w3.org/ns/activitystreams#Public"],
        "cc": [addr.followers_uri],
        "object": note_id
    })
}

/// Delete(Actor) アクティビティを組み立てる。
pub(super) fn build_delete_actor_activity(
    addr: &LocalActorAddress,
    activity_id: &str,
    published: &str,
) -> serde_json::Value {
    serde_json::json!({
        "@context": "https://www.w3.org/ns/activitystreams",
        "type": "Delete",
        "id": activity_id,
        "actor": addr.actor_uri,
        "published": published,
        "to": ["https://www.w3.org/ns/activitystreams#Public"],
        "object": addr.actor_uri
    })
}

/// Update(Person) の object となる Person ドキュメントの構築パラメータ。
pub(super) struct PersonObjectParams<'a> {
    pub(super) local_domain: &'a str,
    pub(super) username: &'a str,
    pub(super) display_name: &'a str,
    pub(super) bio: Option<&'a str>,
    pub(super) avatar_url: Option<&'a str>,
    pub(super) avatar_mime_type: Option<&'a str>,
    pub(super) ap_public_key_pem: &'a str,
    pub(super) emoji_map: &'a serde_json::Value,
    /// `birth_date_public=true`の場合のみ`Some`（呼び出し元が既にフィルタ済みの値を渡す）。
    /// `vcard:bday`として公開する（Misskey互換、`crates/seiran-federation-inbox/src/handlers/actor.rs`
    /// と同じ表現）。
    pub(super) birth_date: Option<chrono::NaiveDate>,
}

/// Person ドキュメントを組み立てる。
/// `actor_handler`（federation-inbox の `GET /users/:username`）が返すものと同一構造にする。
pub(super) fn build_person_object(
    addr: &LocalActorAddress,
    p: &PersonObjectParams,
) -> serde_json::Value {
    let base = format!("https://{}", p.local_domain);
    let mut context = vec![
        serde_json::json!("https://www.w3.org/ns/activitystreams"),
        serde_json::json!("https://w3id.org/security/v1"),
    ];
    if p.birth_date.is_some() {
        context.push(serde_json::json!({"vcard": "http://www.w3.org/2006/vcard/ns#"}));
    }
    let mut person = serde_json::json!({
        "@context": context,
        "id": addr.actor_uri,
        "type": "Person",
        "preferredUsername": p.username,
        "name": p.display_name,
        "inbox": format!("{}/inbox", base),
        "outbox": format!("{}/users/{}/outbox", base, p.username),
        "followers": addr.followers_uri,
        "following": format!("{}/users/{}/following", base, p.username),
        "url": format!("{}/@{}", base, p.username),
        "publicKey": {
            "id": addr.key_id,
            "owner": addr.actor_uri,
            "publicKeyPem": p.ap_public_key_pem
        }
    });
    if let Some(b) = p.bio {
        person["summary"] = serde_json::Value::String(b.to_string());
    }
    if let Some(url) = p.avatar_url {
        person["icon"] = serde_json::json!({
            "type": "Image",
            "mediaType": p.avatar_mime_type.unwrap_or("image/jpeg"),
            "url": url
        });
    }
    // 表示名中のカスタム絵文字ショートコードをリモートが解決できるよう`tag`に付与する（#186）。
    let mut tags = Vec::new();
    append_emoji_tags(p.display_name, p.emoji_map, &mut tags, p.local_domain);
    if !tags.is_empty() {
        person["tag"] = serde_json::Value::Array(tags);
    }
    if let Some(bday) = p.birth_date {
        person["vcard:bday"] = serde_json::Value::String(bday.format("%Y-%m-%d").to_string());
    }
    person
}

/// Update(Person) アクティビティを組み立てる。
pub(super) fn build_update_actor_activity(
    addr: &LocalActorAddress,
    activity_id: &str,
    published: &str,
    person: serde_json::Value,
) -> serde_json::Value {
    serde_json::json!({
        "@context": "https://www.w3.org/ns/activitystreams",
        "type": "Update",
        "id": activity_id,
        "actor": addr.actor_uri,
        "published": published,
        "to": ["https://www.w3.org/ns/activitystreams#Public"],
        "cc": [addr.followers_uri],
        "object": person
    })
}

/// リアクション内容から送信する AP アクティビティ種別を決める。
/// ❤️ は EmojiReact 未対応の実装（Mastodon 等）にも通じる `Like` として送り、
/// それ以外は Misskey 互換の `EmojiReact` として送る。
pub(super) fn reaction_activity_type(content: &str) -> &'static str {
    if content == "❤️" {
        "Like"
    } else {
        "EmojiReact"
    }
}

/// Like/EmojiReact アクティビティ（またはその埋め込みオブジェクト）を組み立てる。
/// `emoji_url` があれば（カスタム絵文字リアクション）、Misskey/Fedibird 互換の
/// `tag: [{type: Emoji, name, icon: {url}}]` を付与する（受信側のパースは
/// `jobs::inbound_activity_process::build_emoji_map` / `extract_emoji_tag_url` を参照）。
/// `tag[].id` には絵文字の canonical URI（`{local_domain}/emojis/{shortcode}`）を付与する。
/// kmyblue（Mastodon系フォーク）は `ActivityPub::Parser::CustomEmojiParser#uri`（= `tag.id`）を
/// `URI.split` に通してドメイン判定するため、`id` が無いと例外で絵文字リアクション処理全体が
/// 失敗し、Unicode絵文字は届くのにカスタム絵文字だけ届かない不具合になる（#176）。
pub(crate) fn build_reaction_object(
    activity_type: &str,
    id: &str,
    actor_uri: &str,
    object_ap_id: &str,
    content: &str,
    emoji_url: Option<&str>,
    local_domain: &str,
) -> serde_json::Value {
    let mut obj = serde_json::json!({
        "type": activity_type,
        "id": id,
        "actor": actor_uri,
        "object": object_ap_id,
    });
    if activity_type == "EmojiReact" {
        obj["content"] = serde_json::Value::String(content.to_string());
        // Misskey 系フォークとの互換のため非標準フィールドも併記する。
        obj["_misskey_reaction"] = serde_json::Value::String(content.to_string());
        if let Some(url) = emoji_url {
            // `content`/`_misskey_reaction` はホスト付き（`:shortcode@.:` 等）だが、
            // `tag[].id`/`tag[].name` は本家Misskey準拠で常にホストなしの素の shortcode を使う。
            let shortcode = parse_reaction_shortcode_and_host(content)
                .map(|(shortcode, _)| shortcode)
                .unwrap_or_else(|| content.trim_matches(':'));
            obj["tag"] = serde_json::json!([{
                "id": format!("https://{}/emojis/{}", local_domain, shortcode),
                "type": "Emoji",
                "name": format!(":{shortcode}:"),
                "icon": { "type": "Image", "url": url },
            }]);
        }
    }
    obj
}

/// Undo(Like/EmojiReact) アクティビティを組み立てる。
pub(super) fn build_undo_reaction_activity(
    addr: &LocalActorAddress,
    undo_id: &str,
    published: &str,
    inner: serde_json::Value,
) -> serde_json::Value {
    serde_json::json!({
        "@context": "https://www.w3.org/ns/activitystreams",
        "type": "Undo",
        "id": undo_id,
        "actor": addr.actor_uri,
        "published": published,
        "to": ["https://www.w3.org/ns/activitystreams#Public"],
        "cc": [addr.followers_uri],
        "object": inner
    })
}

/// 投稿1件の配送に必要な共通データ（本文・作成日時・投稿者名・添付・付随メタ情報）。
/// `deliver_post_to_ap_followers` と `deliver_direct_message_to_ap` の両方で使う
/// 「投稿情報取得＋添付取得」のhowを1箇所にまとめたもの。
pub struct PostActivityBasis {
    pub body: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub username: String,
    pub seiran_uuid: Option<String>,
    pub visibility: String,
    pub emoji_map: serde_json::Value,
    pub attachments: Vec<serde_json::Value>,
    /// アンケート（#228）。`{multiple, options:[{name,votes}], endTime}`。無ければ`None`。
    pub poll: Option<serde_json::Value>,
    /// CW（閲覧注意）ガイド文（#229）。無ければ`None`。
    pub content_warning: Option<String>,
    /// ポストの言語（ISO 639-1）。`seiranPost.language`用。
    pub language: Option<String>,
    /// この投稿のATP側実体（`posts.at_uri`）。ATPコミットが未確定なら`None`
    /// （`seiranPost.counterpartPostId`は配送時点でこれが`Some`の時のみ埋め込む）。
    pub at_uri: Option<String>,
    /// 投稿者（ローカルアクター）のATP DID。ローカルアクターは登録時点で両プロトコルの
    /// IDを持つのが通常だが、ドメイン未確定のシングルホストモード期間中は`None`
    /// （`seiranPost.counterpartAuthorId`に必須のため、`None`ならseiranPost自体を省略する）。
    pub at_did: Option<String>,
}

pub async fn fetch_post_activity_basis(
    db: &PgPool,
    post_id: i64,
    actor_id: i64,
) -> Result<PostActivityBasis, ApError> {
    let row = sqlx::query(
        "SELECT p.body, p.created_at, p.seiran_post_uuid, a.username,
                p.visibility::text AS visibility, p.emoji_map, p.poll, p.content_warning,
                p.language, p.at_uri, a.at_did
         FROM posts p
         JOIN actors a ON a.id = p.actor_id
         WHERE p.id = $1 AND p.actor_id = $2 LIMIT 1",
    )
    .bind(post_id)
    .bind(actor_id)
    .fetch_optional(db)
    .await
    .map_err(|e| ApError::Other(format!("投稿情報取得エラー: {}", e)))?
    .ok_or_else(|| ApError::Other(format!("投稿 {} が見つかりません", post_id)))?;

    let body: String = row
        .try_get("body")
        .map_err(|e| ApError::Other(e.to_string()))?;
    let created_at: chrono::DateTime<chrono::Utc> = row
        .try_get("created_at")
        .map_err(|e| ApError::Other(e.to_string()))?;
    let username: String = row
        .try_get("username")
        .map_err(|e| ApError::Other(e.to_string()))?;
    let seiran_uuid: Option<String> = row.try_get("seiran_post_uuid").unwrap_or(None);
    let visibility: String = row
        .try_get("visibility")
        .unwrap_or_else(|_| "public".to_string());
    let emoji_map: serde_json::Value = row
        .try_get("emoji_map")
        .unwrap_or_else(|_| serde_json::json!({}));
    let attachments = fetch_attachment_documents(db, post_id).await?;
    let poll: Option<serde_json::Value> = row.try_get("poll").unwrap_or(None);
    let content_warning: Option<String> = row.try_get("content_warning").unwrap_or(None);
    let language: Option<String> = row.try_get("language").unwrap_or(None);
    let at_uri: Option<String> = row.try_get("at_uri").unwrap_or(None);
    let at_did: Option<String> = row.try_get("at_did").unwrap_or(None);

    Ok(PostActivityBasis {
        body,
        created_at,
        username,
        seiran_uuid,
        visibility,
        emoji_map,
        attachments,
        poll,
        content_warning,
        language,
        at_uri,
        at_did,
    })
}

/// `PostActivityBasis`から`seiranPost`拡張オブジェクトを組み立てる（#237）。
/// 投稿者がまだATP DIDを持たない（ドメイン未確定のシングルホストモード）場合は
/// `counterpartAuthorId`を埋められないため`None`（seiranPost自体を省略）を返す。
pub async fn build_seiran_post_for_basis(
    db: &PgPool,
    post_id: i64,
    basis: &PostActivityBasis,
) -> Result<Option<crate::seiran_post::SeiranPost>, ApError> {
    let Some(at_did) = basis.at_did.clone() else {
        return Ok(None);
    };
    let (attachments, link_cards) =
        crate::seiran_post::fetch_attachments_and_link_cards(db, post_id)
            .await
            .map_err(|e| ApError::Other(format!("seiranPost添付/リンクカード取得エラー: {}", e)))?;
    Ok(Some(crate::seiran_post::SeiranPost {
        body: basis.body.clone(),
        language: basis.language.clone(),
        visibility: basis.visibility.clone(),
        content_warning: basis.content_warning.clone(),
        emoji_map: basis.emoji_map.clone(),
        poll: basis.poll.clone(),
        counterpart_post_id: basis.at_uri.clone(),
        counterpart_author_id: at_did,
        attachments,
        link_cards,
    }))
}

/// 保存済み `posts.emoji_map`/`actors.emoji_map` のうち、今回配送する本文（投稿本文や
/// 表示名）に実際に現れるカスタム絵文字を ActivityPub `Emoji` tagへ変換して既存の
/// Mention/Hashtag tagへ追加する（#126）。`actors.emoji_map` にも同じ形式で使うため
/// （#186）`pub`にして `seiran-federation-inbox` の Actor ドキュメント生成からも呼ぶ。
/// `id` を付与する理由は `build_reaction_object` のコメント（#176）を参照。
pub fn append_emoji_tags(
    body: &str,
    emoji_map: &serde_json::Value,
    tags: &mut Vec<serde_json::Value>,
    local_domain: &str,
) {
    for shortcode in crate::repository::extract_shortcode_candidates(body) {
        let name = format!(":{}:", shortcode);
        let Some(url) = emoji_map.get(&name).and_then(serde_json::Value::as_str) else {
            continue;
        };
        tags.push(serde_json::json!({
            "id": format!("https://{}/emojis/{}", local_domain, shortcode),
            "type": "Emoji",
            "name": name,
            "icon": {
                "type": "Image",
                "url": url
            }
        }));
    }
}

/// 本文中のメンションを解決し、AP向けHTML化された本文と `tag[]`（AP Mention）、
/// メンション先アクターURI一覧（`kind==Mention`のみ、重複排除）を組み立てる。
/// 3つ目の戻り値は、フォロー関係に関係なくメンション先へ通知（配送）を届けるために使う
/// （`deliver_post_to_ap_followers` 参照）。
pub(super) async fn html_and_tags_for_body(
    body: &str,
    local_domain: &str,
    db: &PgPool,
    ap_client: &ApClient,
) -> (String, Vec<serde_json::Value>, Vec<String>) {
    let (converted, mentions) =
        crate::mention::convert_mentions_for_ap(body, local_domain, db, &ap_client.http).await;
    let html = plain_to_html_with_mentions(&converted, &mentions);
    let tag = crate::mention::ap_inline_mentions_to_tag_json(&mentions);
    let mut mention_uris: Vec<String> = mentions
        .iter()
        .filter(|m| m.kind == crate::mention::ApInlineSpanKind::Mention)
        .map(|m| m.href.clone())
        .collect();
    mention_uris.sort();
    mention_uris.dedup();
    (html, tag, mention_uris)
}

#[cfg(test)]
mod tests {
    use super::super::infra::local_actor_address;
    use super::*;

    fn addr() -> LocalActorAddress {
        local_actor_address("seiran.example", "alice")
    }

    #[test]
    fn create_note_activity_minimal() {
        let activity = build_create_note_activity(
            &addr(),
            &NoteActivityParams {
                local_domain: "seiran.example",
                post_id: 42,
                content_html: "<p>hello</p>",
                published: "2026-07-15T00:00:00+00:00",
                attachments: vec![],
                quote_url: None,
                in_reply_to: None,
                seiran_uuid: None,
                seiran_post: None,
                visibility: "public",
                tag: vec![],
                direct_recipients: &[],
                mention_recipients: &[],
                poll: None,
                content_warning: None,
            },
        );
        assert_eq!(activity["type"], "Create");
        assert_eq!(activity["id"], "https://seiran.example/activities/42");
        let note = &activity["object"];
        assert_eq!(note["type"], "Note");
        assert_eq!(note["id"], "https://seiran.example/notes/42");
        assert_eq!(note["content"], "<p>hello</p>");
        // オプション項目は付与されない
        assert!(note.get("attachment").is_none());
        assert!(note.get("quoteUrl").is_none());
        assert!(note.get("inReplyTo").is_none());
        assert!(note.get("seiranUuid").is_none());
    }

    #[test]
    fn create_note_activity_with_content_warning_sets_summary() {
        let activity = build_create_note_activity(
            &addr(),
            &NoteActivityParams {
                local_domain: "seiran.example",
                post_id: 42,
                content_html: "<p>hello</p>",
                published: "2026-07-15T00:00:00+00:00",
                attachments: vec![],
                quote_url: None,
                in_reply_to: None,
                seiran_uuid: None,
                seiran_post: None,
                visibility: "public",
                tag: vec![],
                direct_recipients: &[],
                mention_recipients: &[],
                poll: None,
                content_warning: Some("ネタバレ"),
            },
        );
        let note = &activity["object"];
        assert_eq!(note["type"], "Note");
        assert_eq!(note["summary"], "ネタバレ");
        assert_eq!(note["content"], "<p>hello</p>");
    }

    #[test]
    fn create_note_activity_without_content_warning_omits_summary() {
        let activity = build_create_note_activity(
            &addr(),
            &NoteActivityParams {
                local_domain: "seiran.example",
                post_id: 42,
                content_html: "<p>hello</p>",
                published: "2026-07-15T00:00:00+00:00",
                attachments: vec![],
                quote_url: None,
                in_reply_to: None,
                seiran_uuid: None,
                seiran_post: None,
                visibility: "public",
                tag: vec![],
                direct_recipients: &[],
                mention_recipients: &[],
                poll: None,
                content_warning: None,
            },
        );
        assert!(activity["object"].get("summary").is_none());
    }

    #[test]
    fn create_note_activity_with_single_choice_poll_becomes_question_with_one_of() {
        let poll = serde_json::json!({
            "multiple": false,
            "options": [
                {"name": "A", "votes": 0},
                {"name": "B", "votes": 0}
            ],
            "endTime": "2026-08-01T00:00:00+00:00"
        });
        let activity = build_create_note_activity(
            &addr(),
            &NoteActivityParams {
                local_domain: "seiran.example",
                post_id: 42,
                content_html: "<p>どっち？</p>",
                published: "2026-07-15T00:00:00+00:00",
                attachments: vec![],
                quote_url: None,
                in_reply_to: None,
                seiran_uuid: None,
                seiran_post: None,
                visibility: "public",
                tag: vec![],
                direct_recipients: &[],
                mention_recipients: &[],
                poll: Some(&poll),
                content_warning: None,
            },
        );
        let note = &activity["object"];
        assert_eq!(note["type"], "Question");
        assert!(note.get("anyOf").is_none());
        let one_of = note["oneOf"].as_array().unwrap();
        assert_eq!(one_of.len(), 2);
        assert_eq!(one_of[0]["type"], "Note");
        assert_eq!(one_of[0]["name"], "A");
        assert_eq!(one_of[0]["replies"]["totalItems"], 0);
        assert_eq!(one_of[1]["name"], "B");
        assert_eq!(note["endTime"], "2026-08-01T00:00:00+00:00");
    }

    #[test]
    fn create_note_activity_with_multiple_choice_poll_uses_any_of_and_omits_end_time() {
        let poll = serde_json::json!({
            "multiple": true,
            "options": [{"name": "A", "votes": 3}],
            "endTime": null
        });
        let activity = build_create_note_activity(
            &addr(),
            &NoteActivityParams {
                local_domain: "seiran.example",
                post_id: 42,
                content_html: "<p>複数選択</p>",
                published: "2026-07-15T00:00:00+00:00",
                attachments: vec![],
                quote_url: None,
                in_reply_to: None,
                seiran_uuid: None,
                seiran_post: None,
                visibility: "public",
                tag: vec![],
                direct_recipients: &[],
                mention_recipients: &[],
                poll: Some(&poll),
                content_warning: None,
            },
        );
        let note = &activity["object"];
        assert_eq!(note["type"], "Question");
        assert!(note.get("oneOf").is_none());
        let any_of = note["anyOf"].as_array().unwrap();
        assert_eq!(any_of[0]["replies"]["totalItems"], 3);
        assert!(note.get("endTime").is_none());
    }

    #[test]
    fn create_note_activity_embeds_seiran_post_when_present() {
        let seiran_post = crate::seiran_post::SeiranPost {
            body: "hello".to_string(),
            language: Some("ja".to_string()),
            visibility: "public".to_string(),
            content_warning: None,
            emoji_map: serde_json::json!({}),
            poll: None,
            counterpart_post_id: None,
            counterpart_author_id: "did:plc:alice".to_string(),
            attachments: vec![],
            link_cards: vec![],
        };
        let activity = build_create_note_activity(
            &addr(),
            &NoteActivityParams {
                local_domain: "seiran.example",
                post_id: 42,
                content_html: "<p>hello</p>",
                published: "2026-07-15T00:00:00+00:00",
                attachments: vec![],
                quote_url: None,
                in_reply_to: None,
                seiran_uuid: None,
                seiran_post: Some(seiran_post),
                visibility: "public",
                tag: vec![],
                direct_recipients: &[],
                mention_recipients: &[],
                poll: None,
                content_warning: None,
            },
        );
        let sp = &activity["object"]["seiranPost"];
        assert_eq!(sp["counterpartAuthorId"], "did:plc:alice");
        // ATP URI未確定時はcounterpartPostId自体を省略する。
        assert!(sp.get("counterpartPostId").is_none());
    }

    #[test]
    fn update_note_activity_has_distinct_id_and_type() {
        let activity = build_update_note_activity(
            &addr(),
            &NoteActivityParams {
                local_domain: "seiran.example",
                post_id: 42,
                content_html: "<p>hello</p>",
                published: "2026-07-15T00:00:00+00:00",
                attachments: vec![],
                quote_url: None,
                in_reply_to: None,
                seiran_uuid: None,
                seiran_post: None,
                visibility: "public",
                tag: vec![],
                direct_recipients: &[],
                mention_recipients: &[],
                poll: None,
                content_warning: None,
            },
            "2026-07-15T00:01:00+00:00",
        );
        assert_eq!(activity["type"], "Update");
        assert_eq!(
            activity["id"],
            "https://seiran.example/activities/seiranpost-update-42"
        );
        assert_eq!(activity["object"]["id"], "https://seiran.example/notes/42");
    }

    #[test]
    fn append_emoji_tags_only_includes_shortcodes_present_in_body() {
        let mut tags = vec![serde_json::json!({
            "type": "Mention",
            "name": "@bob",
            "href": "https://remote.example/users/bob"
        })];
        append_emoji_tags(
            "hello :ablob_glitch: :unknown:",
            &serde_json::json!({
                ":ablob_glitch:": "https://seiran.example/emoji/ablob_glitch.webp",
                ":unused:": "https://seiran.example/emoji/unused.webp"
            }),
            &mut tags,
            "seiran.example",
        );

        assert_eq!(tags.len(), 2);
        assert_eq!(tags[1]["type"], "Emoji");
        assert_eq!(tags[1]["name"], ":ablob_glitch:");
        assert_eq!(tags[1]["id"], "https://seiran.example/emojis/ablob_glitch");
        assert_eq!(
            tags[1]["icon"]["url"],
            "https://seiran.example/emoji/ablob_glitch.webp"
        );
    }

    #[test]
    fn create_note_activity_unlisted_to_cc() {
        let activity = build_create_note_activity(
            &addr(),
            &NoteActivityParams {
                local_domain: "seiran.example",
                post_id: 42,
                content_html: "<p>hello</p>",
                published: "2026-07-15T00:00:00+00:00",
                attachments: vec![],
                quote_url: None,
                in_reply_to: None,
                seiran_uuid: None,
                seiran_post: None,
                visibility: "unlisted",
                tag: vec![],
                direct_recipients: &[],
                mention_recipients: &[],
                poll: None,
                content_warning: None,
            },
        );
        assert_eq!(
            activity["to"],
            serde_json::json!(["https://seiran.example/users/alice/followers"])
        );
        assert_eq!(
            activity["cc"],
            serde_json::json!(["https://www.w3.org/ns/activitystreams#Public"])
        );
        assert_eq!(activity["object"]["to"], activity["to"]);
        assert_eq!(activity["object"]["cc"], activity["cc"]);
    }

    #[test]
    fn create_note_activity_followers_only_to_cc() {
        let activity = build_create_note_activity(
            &addr(),
            &NoteActivityParams {
                local_domain: "seiran.example",
                post_id: 42,
                content_html: "<p>hello</p>",
                published: "2026-07-15T00:00:00+00:00",
                attachments: vec![],
                quote_url: None,
                in_reply_to: None,
                seiran_uuid: None,
                seiran_post: None,
                visibility: "followers_only",
                tag: vec![],
                direct_recipients: &[],
                mention_recipients: &[],
                poll: None,
                content_warning: None,
            },
        );
        assert_eq!(
            activity["to"],
            serde_json::json!(["https://seiran.example/users/alice/followers"])
        );
        assert_eq!(activity["cc"], serde_json::json!(Vec::<String>::new()));
    }

    #[test]
    fn create_note_activity_with_quote_reply_uuid() {
        let activity = build_create_note_activity(
            &addr(),
            &NoteActivityParams {
                local_domain: "seiran.example",
                post_id: 42,
                content_html: "<p>hello</p>",
                published: "2026-07-15T00:00:00+00:00",
                attachments: vec![serde_json::json!({"type": "Document"})],
                quote_url: Some("https://other.example/notes/1"),
                in_reply_to: Some("https://other.example/notes/2"),
                seiran_uuid: Some("uuid-1234"),
                seiran_post: None,
                visibility: "public",
                tag: vec![],
                direct_recipients: &[],
                mention_recipients: &[],
                poll: None,
                content_warning: None,
            },
        );
        let note = &activity["object"];
        assert_eq!(note["quoteUrl"], "https://other.example/notes/1");
        assert_eq!(note["_misskey_quote"], "https://other.example/notes/1");
        assert_eq!(note["inReplyTo"], "https://other.example/notes/2");
        assert_eq!(note["seiranUuid"], "uuid-1234");
        assert_eq!(note["attachment"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn create_note_activity_public_includes_mention_recipients_in_to() {
        let activity = build_create_note_activity(
            &addr(),
            &NoteActivityParams {
                local_domain: "seiran.example",
                post_id: 42,
                content_html: "<p>hello @bob</p>",
                published: "2026-07-15T00:00:00+00:00",
                attachments: vec![],
                quote_url: None,
                in_reply_to: None,
                seiran_uuid: None,
                seiran_post: None,
                visibility: "public",
                tag: vec![],
                direct_recipients: &[],
                mention_recipients: &["https://other.example/users/bob".to_string()],
                poll: None,
                content_warning: None,
            },
        );
        // メンション先はフォロワーでなくても配送が届くよう to に含める（cc ではない）。
        assert_eq!(
            activity["to"],
            serde_json::json!([
                "https://www.w3.org/ns/activitystreams#Public",
                "https://other.example/users/bob"
            ])
        );
        assert_eq!(
            activity["cc"],
            serde_json::json!(["https://seiran.example/users/alice/followers"])
        );
    }

    #[test]
    fn create_note_activity_followers_only_includes_mention_recipients_in_to() {
        let activity = build_create_note_activity(
            &addr(),
            &NoteActivityParams {
                local_domain: "seiran.example",
                post_id: 42,
                content_html: "<p>hello @bob</p>",
                published: "2026-07-15T00:00:00+00:00",
                attachments: vec![],
                quote_url: None,
                in_reply_to: None,
                seiran_uuid: None,
                seiran_post: None,
                visibility: "followers_only",
                tag: vec![],
                direct_recipients: &[],
                mention_recipients: &["https://other.example/users/bob".to_string()],
                poll: None,
                content_warning: None,
            },
        );
        assert_eq!(
            activity["to"],
            serde_json::json!([
                "https://seiran.example/users/alice/followers",
                "https://other.example/users/bob"
            ])
        );
    }

    #[test]
    fn attachment_document_with_dimensions_and_blurhash() {
        let doc = build_attachment_document(
            "https://cdn.example/",
            "media/1.png",
            "image/png",
            Some(100),
            Some(200),
            Some("LKO2?U"),
        );
        assert_eq!(doc["type"], "Document");
        assert_eq!(doc["mediaType"], "image/png");
        // public_url 末尾スラッシュは正規化される
        assert_eq!(doc["url"], "https://cdn.example/media/1.png");
        assert_eq!(doc["width"], 100);
        assert_eq!(doc["height"], 200);
        assert_eq!(doc["blurhash"], "LKO2?U");
    }

    #[test]
    fn attachment_document_without_optional_fields() {
        let doc = build_attachment_document(
            "https://cdn.example",
            "media/1.mp4",
            "video/mp4",
            None,
            None,
            None,
        );
        assert!(doc.get("width").is_none());
        assert!(doc.get("blurhash").is_none());
    }

    #[test]
    fn announce_activity_shape() {
        let activity = build_announce_activity(
            &addr(),
            "https://seiran.example/announces/7",
            "https://other.example/notes/9",
            "2026-07-15T00:00:00+00:00",
            "public",
        );
        assert_eq!(activity["type"], "Announce");
        assert_eq!(activity["object"], "https://other.example/notes/9");
        assert_eq!(activity["actor"], "https://seiran.example/users/alice");
        assert_eq!(
            activity["cc"][0],
            "https://seiran.example/users/alice/followers"
        );
    }

    #[test]
    fn announce_activity_unlisted_to_cc() {
        let activity = build_announce_activity(
            &addr(),
            "https://seiran.example/announces/7",
            "https://other.example/notes/9",
            "2026-07-15T00:00:00+00:00",
            "unlisted",
        );
        assert_eq!(
            activity["to"],
            serde_json::json!(["https://seiran.example/users/alice/followers"])
        );
        assert_eq!(
            activity["cc"],
            serde_json::json!(["https://www.w3.org/ns/activitystreams#Public"])
        );
    }

    #[test]
    fn undo_announce_wraps_original_announce() {
        let activity = build_undo_announce_activity(
            &addr(),
            "https://seiran.example/undos/7",
            "https://seiran.example/announces/7",
            "https://other.example/notes/9",
            "2026-07-15T00:00:00+00:00",
        );
        assert_eq!(activity["type"], "Undo");
        assert_eq!(activity["object"]["type"], "Announce");
        assert_eq!(
            activity["object"]["id"],
            "https://seiran.example/announces/7"
        );
        assert_eq!(
            activity["object"]["object"],
            "https://other.example/notes/9"
        );
    }

    #[test]
    fn delete_actor_targets_own_actor_uri() {
        let activity = build_delete_actor_activity(
            &addr(),
            "https://seiran.example/activities/delete-actor-1",
            "2026-07-15T00:00:00+00:00",
        );
        assert_eq!(activity["type"], "Delete");
        assert_eq!(activity["actor"], activity["object"]);
    }

    #[test]
    fn person_object_optional_fields() {
        let a = addr();
        let minimal = build_person_object(
            &a,
            &PersonObjectParams {
                local_domain: "seiran.example",
                username: "alice",
                display_name: "Alice",
                bio: None,
                avatar_url: None,
                avatar_mime_type: None,
                ap_public_key_pem: "PEM",
                emoji_map: &serde_json::json!({}),
                birth_date: None,
            },
        );
        assert!(minimal.get("summary").is_none());
        assert!(minimal.get("icon").is_none());
        assert!(minimal.get("tag").is_none());
        assert_eq!(minimal["publicKey"]["publicKeyPem"], "PEM");

        let full = build_person_object(
            &a,
            &PersonObjectParams {
                local_domain: "seiran.example",
                username: "alice",
                display_name: "Alice",
                bio: Some("hi"),
                avatar_url: Some("https://cdn.example/a.png"),
                avatar_mime_type: Some("image/png"),
                ap_public_key_pem: "PEM",
                emoji_map: &serde_json::json!({}),
                birth_date: None,
            },
        );
        assert_eq!(full["summary"], "hi");
        assert_eq!(full["icon"]["mediaType"], "image/png");
    }

    #[test]
    fn person_object_includes_emoji_tag_from_display_name() {
        let a = addr();
        let person = build_person_object(
            &a,
            &PersonObjectParams {
                local_domain: "seiran.example",
                username: "alice",
                display_name: ":blobcat: Alice",
                bio: None,
                avatar_url: None,
                avatar_mime_type: None,
                ap_public_key_pem: "PEM",
                emoji_map: &serde_json::json!({":blobcat:": "https://cdn.example/blobcat.png"}),
                birth_date: None,
            },
        );
        assert_eq!(person["tag"][0]["type"], "Emoji");
        assert_eq!(person["tag"][0]["name"], ":blobcat:");
        assert_eq!(
            person["tag"][0]["icon"]["url"],
            "https://cdn.example/blobcat.png"
        );
    }

    #[test]
    fn reaction_type_heart_is_like_others_are_emoji_react() {
        assert_eq!(reaction_activity_type("❤️"), "Like");
        assert_eq!(reaction_activity_type("🎉"), "EmojiReact");
    }

    #[test]
    fn reaction_object_emoji_react_has_misskey_fields() {
        let like = build_reaction_object(
            "Like",
            "id1",
            "actor1",
            "obj1",
            "❤️",
            None,
            "seiran.example",
        );
        assert!(like.get("content").is_none());
        assert!(like.get("_misskey_reaction").is_none());

        let react = build_reaction_object(
            "EmojiReact",
            "id1",
            "actor1",
            "obj1",
            "🎉",
            None,
            "seiran.example",
        );
        assert_eq!(react["content"], "🎉");
        assert_eq!(react["_misskey_reaction"], "🎉");
        assert!(react.get("tag").is_none());
    }

    #[test]
    fn reaction_object_custom_emoji_includes_tag() {
        // content（DB正規形）はホスト付き（`:shortcode@.:`）だが、tag.id/tag.name は
        // 本家Misskey準拠で常にホストなしの素の shortcode を使う。
        let react = build_reaction_object(
            "EmojiReact",
            "id1",
            "actor1",
            "obj1",
            ":blobcat@.:",
            Some("https://example.com/blobcat.png"),
            "seiran.example",
        );
        assert_eq!(react["content"], ":blobcat@.:");
        assert_eq!(react["tag"][0]["type"], "Emoji");
        assert_eq!(react["tag"][0]["name"], ":blobcat:");
        // kmyblue（Mastodon系フォーク）の CustomEmojiParser#uri（= tag.id）が
        // URI.split に通されるため、id が無いと例外で処理が落ちる（#176）。
        assert_eq!(
            react["tag"][0]["id"],
            "https://seiran.example/emojis/blobcat"
        );
        assert_eq!(
            react["tag"][0]["icon"]["url"],
            "https://example.com/blobcat.png"
        );
    }

    #[test]
    fn reaction_object_custom_emoji_handles_legacy_hostless_content() {
        // レガシーデータ（ホスト情報なし `:shortcode:`）でも tag.name は同じ shortcode になる。
        let react = build_reaction_object(
            "EmojiReact",
            "id1",
            "actor1",
            "obj1",
            ":blobcat:",
            Some("https://example.com/blobcat.png"),
            "seiran.example",
        );
        assert_eq!(react["tag"][0]["name"], ":blobcat:");
        assert_eq!(
            react["tag"][0]["id"],
            "https://seiran.example/emojis/blobcat"
        );
    }
}

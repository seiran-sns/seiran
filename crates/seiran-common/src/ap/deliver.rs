//! ActivityPub 投稿配送モジュール
//!
//! ローカルユーザーのアクティビティ（Create/Announce/Undo/Update/Delete/リアクション）を
//! AP フォロワーの inbox へ HTTP Signatures 付きで配送する。
//!
//! # 構成（how/what 分離）
//! - `deliver_*`（公開関数）: 「何を配送するか」のオーケストレーション
//! - `build_*`（純関数）: アクティビティ JSON の組み立て。DB・ネットワーク非依存でテスト可能
//! - `fetch_*` / `fan_out_activity`（共通ヘルパー）: 配送に必要なデータ取得と署名 POST の実行

use futures_util::stream::{self, StreamExt};
use sqlx::{PgPool, Row};

use super::client::{ApClient, ApError};

// =====================================================================
// 共通ヘルパー（how: データ取得・署名 POST ファンアウト）
// =====================================================================

/// ローカルアクターの AP 上のアドレス一式。`local_domain` と `username` から決まる。
struct LocalActorAddress {
    actor_uri: String,
    key_id: String,
    followers_uri: String,
}

/// `at://did/collection/rkey` 形式の AT URI を Bsky.app URL に変換するヘルパー。
/// Fedi フォールバック配送（Bskyネイティブ投稿のリポスト等）で、outbox 表示と
/// push 配送の両方から共通利用する。
pub fn at_uri_to_bsky_app_url(at_uri: &str) -> String {
    let without_prefix = at_uri.strip_prefix("at://").unwrap_or(at_uri);
    let parts: Vec<&str> = without_prefix.splitn(3, '/').collect();
    if parts.len() >= 3 {
        let did = parts[0];
        let rkey = parts[2];
        format!("https://bsky.app/profile/{}/post/{}", did, rkey)
    } else {
        at_uri.to_string()
    }
}

fn local_actor_address(local_domain: &str, username: &str) -> LocalActorAddress {
    let actor_uri = format!("https://{}/users/{}", local_domain, username);
    LocalActorAddress {
        key_id: format!("{}#main-key", actor_uri),
        followers_uri: format!("{}/followers", actor_uri),
        actor_uri,
    }
}

/// アクター ID からユーザー名を取得する。
async fn fetch_username(db: &PgPool, actor_id: i64) -> Result<String, ApError> {
    let row = sqlx::query("SELECT username FROM actors WHERE id = $1 LIMIT 1")
        .bind(actor_id)
        .fetch_optional(db)
        .await
        .map_err(|e| ApError::Other(format!("アクター情報取得エラー: {}", e)))?
        .ok_or_else(|| ApError::Other(format!("アクター {} が見つかりません", actor_id)))?;
    row.try_get("username")
        .map_err(|e| ApError::Other(e.to_string()))
}

/// 指定アクターの AP フォロワー（actor_type='fedi'）の inbox URL 一覧を取得する。
async fn fetch_fedi_follower_inboxes(db: &PgPool, actor_id: i64) -> Result<Vec<String>, ApError> {
    let rows = sqlx::query(
        "SELECT a.ap_inbox_url
         FROM follows f
         JOIN actors a ON a.id = f.follower_actor_id
         WHERE f.target_actor_id = $1
           AND f.status = 'accepted'
           AND a.actor_type = 'fedi'
           AND a.ap_inbox_url IS NOT NULL",
    )
    .bind(actor_id)
    .fetch_all(db)
    .await
    .map_err(|e| ApError::Other(format!("フォロワー取得エラー: {}", e)))?;

    Ok(rows
        .iter()
        .filter_map(|r| r.try_get::<String, _>("ap_inbox_url").ok())
        .collect())
}

/// 参加中（status='accepted'）のFediverseリレー（#140）のinbox URL一覧を取得する。
async fn fetch_accepted_relay_inboxes(db: &PgPool) -> Result<Vec<String>, ApError> {
    let rows = sqlx::query("SELECT inbox_url FROM fediverse_relays WHERE status = 'accepted'")
        .fetch_all(db)
        .await
        .map_err(|e| ApError::Other(format!("リレー取得エラー: {}", e)))?;

    Ok(rows
        .iter()
        .filter_map(|r| r.try_get::<String, _>("inbox_url").ok())
        .collect())
}

/// メンション先アクターURI一覧を、フォロー関係と独立に inbox URL へ解決する
/// （Mastodon等のメンション個別配送相当）。DB既知の fedi アクターは DB から、
/// まだ一度も見たことのない相手はその場でアクタードキュメントを取得して解決する
/// （ここで新規に upsert はしない。既存の `resolve_fedi_mention_href` によるメンション
/// href 解決も同様に都度webfinger問い合わせのみで、DB保存は伴わない設計に合わせた）。
/// `local_domain` 自身宛（ローカルユーザーへの自己言及等）は除外する。
/// 個々の取得失敗は他の宛先解決を妨げないよう、ログのみでベストエフォートに扱う。
async fn fetch_inboxes_by_ap_uris(
    ap_client: &ApClient,
    db: &PgPool,
    local_domain: &str,
    ap_uris: &[String],
) -> Vec<String> {
    let local_prefix = format!("https://{}/", local_domain);
    let remote_uris: Vec<String> = ap_uris
        .iter()
        .filter(|u| !u.starts_with(&local_prefix))
        .cloned()
        .collect();
    if remote_uris.is_empty() {
        return Vec::new();
    }

    let known_rows = sqlx::query(
        "SELECT ap_uri, ap_inbox_url FROM actors WHERE ap_uri = ANY($1) AND actor_type = 'fedi'",
    )
    .bind(&remote_uris)
    .fetch_all(db)
    .await
    .unwrap_or_else(|e| {
        tracing::error!("[Deliver] メンション先アクター検索エラー: {}", e);
        Vec::new()
    });

    let mut inboxes = Vec::new();
    let mut known_uris = std::collections::HashSet::new();
    for row in &known_rows {
        if let Ok(uri) = row.try_get::<String, _>("ap_uri") {
            known_uris.insert(uri);
        }
        if let Ok(Some(inbox)) = row.try_get::<Option<String>, _>("ap_inbox_url") {
            inboxes.push(inbox);
        }
    }

    for uri in remote_uris.iter().filter(|u| !known_uris.contains(*u)) {
        match ap_client.fetch_actor(uri).await {
            Ok(actor) => {
                if let Some(inbox) = actor.inbox {
                    inboxes.push(inbox);
                }
            }
            Err(e) => {
                tracing::warn!(
                    "[Deliver] メンション先アクター({})の取得失敗、配送スキップ: {}",
                    uri,
                    e
                );
            }
        }
    }

    inboxes
}

/// アクティビティを inbox 群へ署名付き POST でファンアウトし、成功/失敗件数をログする。
///
/// 一部でも成功すれば `Ok`（受信側は activity id で重複排除するとはいえ、再送を最小限に
/// するため）。宛先が 1 件以上あり **全滅** した場合のみ `Err` を返し、ジョブキュー経由の
/// 呼び出しでは WorkerEngine のリトライに乗る。
async fn fan_out_activity(
    ap_client: &ApClient,
    inboxes: &[String],
    activity: &serde_json::Value,
    key_id: &str,
    ap_private_key_pem: &str,
    log_label: &str,
) -> Result<(), ApError> {
    if inboxes.is_empty() {
        return Ok(());
    }

    let body_str = serde_json::to_string(activity).map_err(ApError::Json)?;

    // 1件のポストにフォロワーが多数（数十〜数百inbox）いる場合、逐次POSTだと1件ずつ
    // 配送していた（応答の遅い相手が混ざると配送全体が線形に伸びる）。Workerジョブ実行の
    // 枠内（追加のtokio::spawnはしない）で`buffer_unordered`により同時ポーリングし、
    // 応答の遅い宛先が他の宛先をブロックしないようにする（docs/code_audit_2026-08-05.md P-3）。
    const MAX_CONCURRENT_DELIVERIES: usize = 8;
    let results: Vec<Result<(), ApError>> = stream::iter(inboxes.to_vec())
        .map(|inbox| {
            let body_str = body_str.clone();
            let key_id = key_id.to_owned();
            let ap_private_key_pem = ap_private_key_pem.to_owned();
            let log_label = log_label.to_owned();
            async move {
                ap_client
                    .sign_and_post(&inbox, &body_str, &key_id, &ap_private_key_pem)
                    .await
                    .map_err(|e| {
                        tracing::error!("[Deliver] {}: {} への配送失敗: {}", log_label, inbox, e);
                        e
                    })
            }
        })
        .buffer_unordered(MAX_CONCURRENT_DELIVERIES)
        .collect()
        .await;

    let ok = results.iter().filter(|r| r.is_ok()).count();
    let ng = results.len() - ok;

    if ng > 0 {
        tracing::warn!("[Deliver] {}: {}件成功 / {}件失敗", log_label, ok, ng);
    } else {
        tracing::info!("[Deliver] {}: {}件成功 / {}件失敗", log_label, ok, ng);
    }

    if ok == 0 && ng > 0 {
        return Err(ApError::Other(format!(
            "{}: 全 {} 件の配送に失敗",
            log_label, ng
        )));
    }
    Ok(())
}

// =====================================================================
// アクティビティ構築（what: 純関数・テスト対象）
// =====================================================================

/// AS2 の Public コレクション URI（`to`/`cc` に載せることで公開範囲を示す）。
const AS_PUBLIC: &str = "https://www.w3.org/ns/activitystreams#Public";

/// Create(Note) アクティビティの構築パラメータ。
struct NoteActivityParams<'a> {
    local_domain: &'a str,
    post_id: i64,
    content_html: &'a str,
    published: &'a str,
    attachments: Vec<serde_json::Value>,
    quote_url: Option<&'a str>,
    in_reply_to: Option<&'a str>,
    seiran_uuid: Option<&'a str>,
    /// "public" | "unlisted" | "followers_only" | "direct"。to/cc の組み立てに使う
    /// （受信側の `classify_ap_visibility` と対称なマッピング）。
    visibility: &'a str,
    /// 本文中のメンションから組み立てた `tag[]`（`{"type":"Mention","href":..,"name":..}`）。
    /// 空なら Note オブジェクトに `tag` フィールド自体を含めない。
    tag: Vec<serde_json::Value>,
    /// `visibility="direct"`（DM）の場合の宛先アクターURI一覧。`to` に直接使う
    /// （フォロワーコレクションではなく実際の宛先個人のみへ配送するため）。
    /// direct以外では無視される。
    direct_recipients: &'a [String],
    /// 本文中でメンションしたアクターURI一覧（`direct`以外の可視性で`to`に追加する）。
    /// フォロワーでない相手にもメンション通知の元となるアクティビティ自体を届けるため
    /// （`to`に含めるのはAP的な作法・実際の配送先は別途 `deliver_post_to_ap_followers` が解決する）。
    /// directでは無視される（`direct_recipients`が既に実際の宛先そのもののため）。
    mention_recipients: &'a [String],
}

/// 可視性から Create(Note)/Note 共通の to/cc を決める。
fn visibility_to_to_cc(
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

/// Create(Note) アクティビティを組み立てる。
fn build_create_note_activity(
    addr: &LocalActorAddress,
    p: &NoteActivityParams,
) -> serde_json::Value {
    let note_id = format!("https://{}/notes/{}", p.local_domain, p.post_id);
    let activity_id = format!("https://{}/activities/{}", p.local_domain, p.post_id);
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

/// 添付ファイル 1 件分の AP Document オブジェクトを組み立てる。
fn build_attachment_document(
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
fn build_announce_activity(
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
fn build_undo_announce_activity(
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
fn build_delete_note_activity(
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
fn build_delete_actor_activity(
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
struct PersonObjectParams<'a> {
    local_domain: &'a str,
    username: &'a str,
    display_name: &'a str,
    bio: Option<&'a str>,
    avatar_url: Option<&'a str>,
    avatar_mime_type: Option<&'a str>,
    ap_public_key_pem: &'a str,
    emoji_map: &'a serde_json::Value,
    /// `birth_date_public=true`の場合のみ`Some`（呼び出し元が既にフィルタ済みの値を渡す）。
    /// `vcard:bday`として公開する（Misskey互換、`crates/seiran-federation-inbox/src/handlers/actor.rs`
    /// と同じ表現）。
    birth_date: Option<chrono::NaiveDate>,
}

/// Person ドキュメントを組み立てる。
/// `actor_handler`（federation-inbox の `GET /users/:username`）が返すものと同一構造にする。
fn build_person_object(addr: &LocalActorAddress, p: &PersonObjectParams) -> serde_json::Value {
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
fn build_update_actor_activity(
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
fn reaction_activity_type(content: &str) -> &'static str {
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
fn build_reaction_object(
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
            let shortcode = content.trim_matches(':');
            obj["tag"] = serde_json::json!([{
                "id": format!("https://{}/emojis/{}", local_domain, shortcode),
                "type": "Emoji",
                "name": content,
                "icon": { "type": "Image", "url": url },
            }]);
        }
    }
    obj
}

/// Undo(Like/EmojiReact) アクティビティを組み立てる。
fn build_undo_reaction_activity(
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
struct PostActivityBasis {
    body: String,
    created_at: chrono::DateTime<chrono::Utc>,
    username: String,
    seiran_uuid: Option<String>,
    visibility: String,
    emoji_map: serde_json::Value,
    attachments: Vec<serde_json::Value>,
}

async fn fetch_post_activity_basis(
    db: &PgPool,
    post_id: i64,
    actor_id: i64,
) -> Result<PostActivityBasis, ApError> {
    let row = sqlx::query(
        "SELECT p.body, p.created_at, p.seiran_post_uuid, a.username,
                p.visibility::text AS visibility, p.emoji_map
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

    Ok(PostActivityBasis {
        body,
        created_at,
        username,
        seiran_uuid,
        visibility,
        emoji_map,
        attachments,
    })
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
async fn html_and_tags_for_body(
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

// =====================================================================
// 配送オーケストレーション（公開 API）
// =====================================================================

/// ローカル投稿を AP フォロワー全員の inbox へ配送する
///
/// `override_body` が `Some` の場合はその値を本文として使用する（AP向けメンション変換済みテキスト等）。
/// `None` の場合は DB の `posts.body` をそのまま使用する。
/// `quote_url` が `Some` の場合は Note に `quoteUrl` / `_misskey_quote` を付与する（引用投稿）。
/// seiran_post_uuid は DB の posts.seiran_post_uuid から自動取得して Note に付与する。
#[allow(clippy::too_many_arguments)]
pub async fn deliver_post_to_ap_followers(
    ap_client: &ApClient,
    db: &PgPool,
    post_id: i64,
    actor_id: i64,
    local_domain: &str,
    ap_private_key_pem: &str,
    override_body: Option<&str>,
    quote_url: Option<&str>,
    in_reply_to: Option<&str>,
) -> Result<(), ApError> {
    let basis = fetch_post_activity_basis(db, post_id, actor_id).await?;

    // DM（direct）はこの関数（フォロワー全体へのファンアウト）では扱わない。
    // `deliver_direct_message_to_ap` を使うこと（呼び出し元の実装ミスに対する最終ガード）。
    if basis.visibility == "direct" {
        tracing::warn!("[deliver_post_to_ap_followers] visibility=direct のポストが渡されたためスキップ（post_id={}）", post_id);
        return Ok(());
    }

    let body: String = override_body.map(str::to_owned).unwrap_or(basis.body);

    // override_body（リポストのフォールバックテキスト等、投稿者本人が書いた本文ではない合成テキスト）
    // の場合はメンション変換をせずそのまま HTML 化する。通常投稿（override_body なし）はここで
    // 本文中のメンションを解決し、`<a>` アンカーと `tag[]`（AP Mention）を組み立てる。
    let (content_html, mut tag, mention_uris): (String, Vec<serde_json::Value>, Vec<String>) =
        if override_body.is_some() {
            (plain_to_html(&body), Vec::new(), Vec::new())
        } else {
            html_and_tags_for_body(&body, local_domain, db, ap_client).await
        };
    append_emoji_tags(&body, &basis.emoji_map, &mut tag, local_domain);

    // 配送先はフォロワー + 本文中でメンションした相手（フォロワーでなくても通知を届ける）の和集合。
    let mut inboxes = fetch_fedi_follower_inboxes(db, actor_id).await?;
    for inbox in fetch_inboxes_by_ap_uris(ap_client, db, local_domain, &mention_uris).await {
        if !inboxes.contains(&inbox) {
            inboxes.push(inbox);
        }
    }
    // public投稿のみ、参加中のリレー（#140）にもファンアウトする。
    // unlisted/followers_only はリレー配送対象外（リレーは公開投稿の中継が目的のため）。
    if basis.visibility == "public" {
        for inbox in fetch_accepted_relay_inboxes(db).await? {
            if !inboxes.contains(&inbox) {
                inboxes.push(inbox);
            }
        }
    }
    if inboxes.is_empty() {
        return Ok(());
    }

    let addr = local_actor_address(local_domain, &basis.username);
    let activity = build_create_note_activity(
        &addr,
        &NoteActivityParams {
            local_domain,
            post_id,
            content_html: &content_html,
            published: &basis.created_at.to_rfc3339(),
            attachments: basis.attachments,
            quote_url,
            in_reply_to,
            seiran_uuid: basis.seiran_uuid.as_deref(),
            visibility: &basis.visibility,
            tag,
            direct_recipients: &[],
            mention_recipients: &mention_uris,
        },
    );

    fan_out_activity(
        ap_client,
        &inboxes,
        &activity,
        &addr.key_id,
        ap_private_key_pem,
        &format!(
            "Create(Note) post_id={} username={}",
            post_id, basis.username
        ),
    )
    .await
}

/// DM（`visibility='direct'`）投稿を、宛先（`post_recipients`）の中のFediアクターへのみ
/// 配送する。`deliver_post_to_ap_followers`（フォロワー全体へのファンアウト）とは異なり、
/// フォロワーコレクションではなく実際の宛先個人のinboxのみへCreate(Note)を送る。
pub async fn deliver_direct_message_to_ap(
    ap_client: &ApClient,
    db: &PgPool,
    post_id: i64,
    actor_id: i64,
    local_domain: &str,
    ap_private_key_pem: &str,
) -> Result<(), ApError> {
    let basis = fetch_post_activity_basis(db, post_id, actor_id).await?;

    let recipient_rows = sqlx::query(
        "SELECT a.ap_uri, a.ap_inbox_url
         FROM post_recipients pr JOIN actors a ON a.id = pr.actor_id
         WHERE pr.post_id = $1 AND a.actor_type = 'fedi' AND a.ap_uri IS NOT NULL AND a.ap_inbox_url IS NOT NULL",
    )
    .bind(post_id)
    .fetch_all(db)
    .await
    .map_err(|e| ApError::Other(format!("DM宛先取得エラー: {}", e)))?;

    if recipient_rows.is_empty() {
        return Ok(());
    }

    let direct_recipients: Vec<String> = recipient_rows
        .iter()
        .filter_map(|r| r.try_get::<String, _>("ap_uri").ok())
        .collect();
    let inboxes: Vec<String> = recipient_rows
        .iter()
        .filter_map(|r| r.try_get::<String, _>("ap_inbox_url").ok())
        .collect();

    let (content_html, mut tag, _mention_uris) =
        html_and_tags_for_body(&basis.body, local_domain, db, ap_client).await;
    append_emoji_tags(&basis.body, &basis.emoji_map, &mut tag, local_domain);

    let addr = local_actor_address(local_domain, &basis.username);
    let activity = build_create_note_activity(
        &addr,
        &NoteActivityParams {
            local_domain,
            post_id,
            content_html: &content_html,
            published: &basis.created_at.to_rfc3339(),
            attachments: basis.attachments,
            quote_url: None,
            in_reply_to: None,
            seiran_uuid: None,
            visibility: "direct",
            tag,
            direct_recipients: &direct_recipients,
            // directは`direct_recipients`が既に実際の宛先そのものなので無視される（visibility_to_to_cc参照）。
            mention_recipients: &[],
        },
    );

    fan_out_activity(
        ap_client,
        &inboxes,
        &activity,
        &addr.key_id,
        ap_private_key_pem,
        &format!(
            "Create(Note DM) post_id={} username={}",
            post_id, basis.username
        ),
    )
    .await
}

/// 投稿の添付ファイル群を AP Document オブジェクトのリストとして取得する。
async fn fetch_attachment_documents(
    db: &PgPool,
    post_id: i64,
) -> Result<Vec<serde_json::Value>, ApError> {
    let rows = sqlx::query(
        "SELECT mf.storage_key, mf.mime_type, mf.width, mf.height, mf.blurhash, sp.public_url
         FROM post_attachments pa
         JOIN media_files mf ON mf.id = pa.media_file_id
         JOIN storage_providers sp ON sp.id = mf.storage_provider_id
         WHERE pa.post_id = $1
         ORDER BY pa.position",
    )
    .bind(post_id)
    .fetch_all(db)
    .await
    .map_err(|e| ApError::Other(format!("添付取得エラー: {}", e)))?;

    Ok(rows
        .iter()
        .filter_map(|r| {
            let storage_key: String = r.try_get("storage_key").ok()?;
            let mime_type: String = r.try_get("mime_type").ok()?;
            let width: Option<i32> = r.try_get("width").ok()?;
            let height: Option<i32> = r.try_get("height").ok()?;
            let blurhash: Option<String> = r.try_get("blurhash").ok()?;
            let public_url: String = r.try_get("public_url").ok()?;
            Some(build_attachment_document(
                &public_url,
                &storage_key,
                &mime_type,
                width,
                height,
                blurhash.as_deref(),
            ))
        })
        .collect())
}

/// Announce 対象の元ポストが Fedi リモートである場合、その投稿者の inbox URL と actor URI を返す。
/// リアクション配送（`resolve_reaction_targets`）と同様、フォロワー配送だけでは元投稿者の
/// サーバーに Announce が届かず、ブースト数の反映や通知が発生しないため必要。
async fn resolve_announce_object_actor(
    db: &PgPool,
    original_ap_object_id: &str,
) -> Result<Option<(String, String)>, ApError> {
    let row = sqlx::query(
        "SELECT a.ap_inbox_url, a.ap_uri
         FROM posts p JOIN actors a ON a.id = p.actor_id
         WHERE p.ap_object_id = $1 AND a.actor_type = 'fedi' LIMIT 1",
    )
    .bind(original_ap_object_id)
    .fetch_optional(db)
    .await
    .map_err(|e| ApError::Other(format!("Announce対象ポスト取得エラー: {}", e)))?;

    let Some(row) = row else {
        return Ok(None);
    };
    let inbox: Option<String> = row.try_get("ap_inbox_url").unwrap_or(None);
    let actor_uri: Option<String> = row.try_get("ap_uri").unwrap_or(None);
    Ok(inbox.zip(actor_uri))
}

/// ローカルアクターの AP Announce アクティビティを Fedi フォロワー全員 + 元ポストの投稿者へ配送する
///
/// `original_ap_object_id` は Announce の対象（元ポストの AP URI）。
pub async fn deliver_ap_announce(
    ap_client: &ApClient,
    db: &PgPool,
    post_id: i64,
    actor_id: i64,
    local_domain: &str,
    ap_private_key_pem: &str,
    original_ap_object_id: &str,
) -> Result<(), ApError> {
    let username = fetch_username(db, actor_id).await?;
    let visibility: String = sqlx::query_scalar("SELECT visibility::text FROM posts WHERE id = $1")
        .bind(post_id)
        .fetch_optional(db)
        .await
        .map_err(|e| ApError::Other(e.to_string()))?
        .unwrap_or_else(|| "public".to_string());
    let object_actor = resolve_announce_object_actor(db, original_ap_object_id).await?;

    let mut inboxes: std::collections::HashSet<String> = fetch_fedi_follower_inboxes(db, actor_id)
        .await?
        .into_iter()
        .collect();
    if let Some((inbox, _)) = &object_actor {
        inboxes.insert(inbox.clone());
    }
    let inboxes: Vec<String> = inboxes.into_iter().collect();

    let addr = local_actor_address(local_domain, &username);
    let announce_id = format!("https://{}/announces/{}", local_domain, post_id);
    let mut activity = build_announce_activity(
        &addr,
        &announce_id,
        original_ap_object_id,
        &chrono::Utc::now().to_rfc3339(),
        &visibility,
    );
    // 元投稿者を明示的に cc へ含める（Mastodon 互換）。フォロワー配送だけでは
    // 相手サーバーがブースト数・通知に反映しないことがあるため。
    if let Some((_, actor_uri)) = &object_actor {
        if let Some(cc) = activity.get_mut("cc").and_then(|v| v.as_array_mut()) {
            if !cc.iter().any(|v| v.as_str() == Some(actor_uri.as_str())) {
                cc.push(serde_json::Value::String(actor_uri.clone()));
            }
        }
    }

    fan_out_activity(
        ap_client,
        &inboxes,
        &activity,
        &addr.key_id,
        ap_private_key_pem,
        &format!("Announce post_id={} username={}", post_id, username),
    )
    .await
}

/// ローカルアクターの AP Delete(Actor) アクティビティを Fedi フォロワー全員の inbox へ配送する。
/// アカウント退会時（#29）に呼び出し、リモートサーバーにフォロー解除とキャッシュ削除を促す。
pub async fn deliver_delete_actor(
    ap_client: &ApClient,
    db: &PgPool,
    actor_id: i64,
    local_domain: &str,
    ap_private_key_pem: &str,
) -> Result<(), ApError> {
    let username = fetch_username(db, actor_id).await?;
    let inboxes = fetch_fedi_follower_inboxes(db, actor_id).await?;

    let addr = local_actor_address(local_domain, &username);
    let activity_id = format!(
        "https://{}/activities/delete-actor-{}",
        local_domain, actor_id
    );
    let activity =
        build_delete_actor_activity(&addr, &activity_id, &chrono::Utc::now().to_rfc3339());

    fan_out_activity(
        ap_client,
        &inboxes,
        &activity,
        &addr.key_id,
        ap_private_key_pem,
        &format!("Delete(Actor) actor_id={} username={}", actor_id, username),
    )
    .await
}

/// ローカルアクターの AP Undo(Announce) を Fedi フォロワー全員 + 元ポストの投稿者へ配送する。
/// `announce_post_id` はリポスト投稿の posts.id、`original_ap_object_id` は元ポストの AP URI。
pub async fn deliver_undo_announce(
    ap_client: &ApClient,
    db: &PgPool,
    announce_post_id: i64,
    actor_id: i64,
    local_domain: &str,
    ap_private_key_pem: &str,
    original_ap_object_id: &str,
) -> Result<(), ApError> {
    let username = fetch_username(db, actor_id).await?;
    let object_actor = resolve_announce_object_actor(db, original_ap_object_id).await?;
    let mut inboxes: std::collections::HashSet<String> = fetch_fedi_follower_inboxes(db, actor_id)
        .await?
        .into_iter()
        .collect();
    if let Some((inbox, _)) = &object_actor {
        inboxes.insert(inbox.clone());
    }
    let inboxes: Vec<String> = inboxes.into_iter().collect();

    let addr = local_actor_address(local_domain, &username);
    let announce_id = format!("https://{}/announces/{}", local_domain, announce_post_id);
    let undo_id = format!("https://{}/undos/{}", local_domain, announce_post_id);
    let activity = build_undo_announce_activity(
        &addr,
        &undo_id,
        &announce_id,
        original_ap_object_id,
        &chrono::Utc::now().to_rfc3339(),
    );

    fan_out_activity(
        ap_client,
        &inboxes,
        &activity,
        &addr.key_id,
        ap_private_key_pem,
        &format!(
            "Undo(Announce) post_id={} username={}",
            announce_post_id, username
        ),
    )
    .await
}

/// ローカルアクターの AP Delete(Note) アクティビティを Fedi フォロワー全員の inbox へ配送する。
/// `post_id` はリポスト投稿の posts.id（`PostToFollowers` で送った Note の id
/// `https://{domain}/notes/{post_id}` と一致する）。
pub async fn deliver_delete_note(
    ap_client: &ApClient,
    db: &PgPool,
    post_id: i64,
    actor_id: i64,
    local_domain: &str,
    ap_private_key_pem: &str,
) -> Result<(), ApError> {
    let username = fetch_username(db, actor_id).await?;
    let inboxes = fetch_fedi_follower_inboxes(db, actor_id).await?;

    let addr = local_actor_address(local_domain, &username);
    let note_id = format!("https://{}/notes/{}", local_domain, post_id);
    let activity_id = format!(
        "https://{}/activities/delete-note-{}",
        local_domain, post_id
    );
    let activity = build_delete_note_activity(
        &addr,
        &note_id,
        &activity_id,
        &chrono::Utc::now().to_rfc3339(),
    );

    fan_out_activity(
        ap_client,
        &inboxes,
        &activity,
        &addr.key_id,
        ap_private_key_pem,
        &format!("Delete(Note) post_id={} username={}", post_id, username),
    )
    .await
}

/// ローカルアクターの AP Update(Person) アクティビティを Fedi フォロワー全員の inbox へ配送する。
///
/// プロフィール編集（display_name/bio/avatar）後に呼び出し、リモートインスタンスが
/// キャッシュ済みの Actor 情報をプルせずとも即時更新できるようにする。
pub async fn deliver_update_actor(
    ap_client: &ApClient,
    db: &PgPool,
    actor_id: i64,
    local_domain: &str,
    ap_private_key_pem: &str,
    ap_public_key_pem: &str,
) -> Result<(), ApError> {
    let row = sqlx::query(
        "SELECT a.username, a.display_name, a.bio, \
                COALESCE(rtrim(sp.public_url, '/') || '/' || mf.storage_key, a.avatar_url) AS avatar_url, \
                mf.mime_type AS avatar_mime_type, a.emoji_map, a.birth_date, a.birth_date_public \
         FROM actors a \
         LEFT JOIN media_files mf ON mf.id = a.avatar_media_id \
         LEFT JOIN storage_providers sp ON sp.id = mf.storage_provider_id \
         WHERE a.id = $1 LIMIT 1",
    )
    .bind(actor_id)
    .fetch_optional(db)
    .await
    .map_err(|e| ApError::Other(format!("アクター情報取得エラー: {}", e)))?
    .ok_or_else(|| ApError::Other(format!("アクター {} が見つかりません", actor_id)))?;

    let username: String = row
        .try_get("username")
        .map_err(|e| ApError::Other(e.to_string()))?;
    let display_name: String = row
        .try_get::<Option<String>, _>("display_name")
        .map_err(|e| ApError::Other(e.to_string()))?
        .unwrap_or_else(|| username.clone());
    let bio: Option<String> = row.try_get("bio").unwrap_or(None);
    let stored_avatar_url: Option<String> = row.try_get("avatar_url").unwrap_or(None);
    let avatar_url = Some(
        stored_avatar_url
            .clone()
            .unwrap_or_else(|| crate::avatar::fallback_avatar_url(local_domain, actor_id)),
    );
    let avatar_mime_type: Option<String> = if stored_avatar_url.is_some() {
        row.try_get("avatar_mime_type").unwrap_or(None)
    } else {
        Some("image/png".to_string())
    };
    let emoji_map: serde_json::Value = row
        .try_get("emoji_map")
        .unwrap_or_else(|_| serde_json::json!({}));
    let birth_date_public: bool = row.try_get("birth_date_public").unwrap_or(false);
    let birth_date = if birth_date_public {
        row.try_get::<Option<chrono::NaiveDate>, _>("birth_date")
            .unwrap_or(None)
    } else {
        None
    };

    let inboxes = fetch_fedi_follower_inboxes(db, actor_id).await?;
    if inboxes.is_empty() {
        return Ok(());
    }

    let addr = local_actor_address(local_domain, &username);
    let person = build_person_object(
        &addr,
        &PersonObjectParams {
            local_domain,
            username: &username,
            display_name: &display_name,
            bio: bio.as_deref(),
            avatar_url: avatar_url.as_deref(),
            avatar_mime_type: avatar_mime_type.as_deref(),
            ap_public_key_pem,
            emoji_map: &emoji_map,
            birth_date,
        },
    );

    // Update は編集の度に配送されうるため、activity id は毎回一意にする
    // （固定IDだと一部実装が2回目以降のUpdateを重複とみなして無視する）。
    let activity_id = format!(
        "https://{}/activities/update-actor-{}-{}",
        local_domain,
        actor_id,
        chrono::Utc::now().timestamp_millis()
    );
    let activity = build_update_actor_activity(
        &addr,
        &activity_id,
        &chrono::Utc::now().to_rfc3339(),
        person,
    );

    fan_out_activity(
        ap_client,
        &inboxes,
        &activity,
        &addr.key_id,
        ap_private_key_pem,
        &format!("Update(Actor) actor_id={} username={}", actor_id, username),
    )
    .await
}

/// リアクション配送先を解決する。
///
/// 配送先は (1) 対象ポストの著者（Fedi リモートの場合のみ）と (2) `reactor_actor_id`
/// の Fedi フォロワー全員、の inbox の和集合（重複排除）。対象ポストが AP 上の実体
/// （`ap_object_id`）を持たない場合（Bsky 由来など）は `None` を返し、配送不要とする。
async fn resolve_reaction_targets(
    db: &PgPool,
    post_id: i64,
    reactor_actor_id: i64,
) -> Result<Option<(String, Vec<String>)>, ApError> {
    let post_row = sqlx::query(
        "SELECT p.ap_object_id, a.actor_type::text AS actor_type, a.ap_inbox_url
         FROM posts p JOIN actors a ON a.id = p.actor_id
         WHERE p.id = $1 LIMIT 1",
    )
    .bind(post_id)
    .fetch_optional(db)
    .await
    .map_err(|e| ApError::Other(format!("対象ポスト取得エラー: {}", e)))?;

    let post_row = match post_row {
        Some(r) => r,
        None => return Ok(None),
    };

    let object_ap_id: Option<String> = post_row.try_get("ap_object_id").unwrap_or(None);
    let object_ap_id = match object_ap_id {
        Some(id) => id,
        None => return Ok(None),
    };
    let author_actor_type: String = post_row.try_get("actor_type").unwrap_or_default();
    let author_inbox: Option<String> = post_row.try_get("ap_inbox_url").unwrap_or(None);

    let mut inboxes: std::collections::HashSet<String> = std::collections::HashSet::new();
    if author_actor_type == "fedi" {
        if let Some(inbox) = author_inbox {
            inboxes.insert(inbox);
        }
    }

    inboxes.extend(fetch_fedi_follower_inboxes(db, reactor_actor_id).await?);

    Ok(Some((object_ap_id, inboxes.into_iter().collect())))
}

/// ローカルアクターの絵文字リアクション（Like/EmojiReact）を、対象ポストの著者
/// （Fedi リモートの場合のみ）と reactor 本人の Fedi フォロワー全員の inbox へ配送する。
///
/// `activity_id` は呼び出し元があらかじめ発行し `reactions.ap_activity_id` に保存した値と
/// 同一のものを渡すこと（後の Undo で参照するため）。
#[allow(clippy::too_many_arguments)]
pub async fn deliver_ap_reaction(
    ap_client: &ApClient,
    db: &PgPool,
    post_id: i64,
    actor_id: i64,
    local_domain: &str,
    ap_private_key_pem: &str,
    activity_id: &str,
    content: &str,
    emoji_url: Option<&str>,
) -> Result<(), ApError> {
    let (object_ap_id, inboxes) = match resolve_reaction_targets(db, post_id, actor_id).await? {
        Some(v) => v,
        None => return Ok(()),
    };

    let username = fetch_username(db, actor_id).await?;
    let addr = local_actor_address(local_domain, &username);
    let activity_type = reaction_activity_type(content);

    let mut activity = build_reaction_object(
        activity_type,
        activity_id,
        &addr.actor_uri,
        &object_ap_id,
        content,
        emoji_url,
        local_domain,
    );
    activity["@context"] =
        serde_json::Value::String("https://www.w3.org/ns/activitystreams".to_string());
    activity["published"] = serde_json::Value::String(chrono::Utc::now().to_rfc3339());
    activity["to"] = serde_json::json!(["https://www.w3.org/ns/activitystreams#Public"]);
    activity["cc"] = serde_json::json!([addr.followers_uri]);

    fan_out_activity(
        ap_client,
        &inboxes,
        &activity,
        &addr.key_id,
        ap_private_key_pem,
        &format!(
            "{} post_id={} actor_id={}",
            activity_type, post_id, actor_id
        ),
    )
    .await
}

/// リモートQuestionへの回答を、Mastodon互換の
/// `Create { object: Note { name, inReplyTo } }` として投稿者inboxへ送る。
pub async fn deliver_ap_poll_vote(
    ap_client: &ApClient,
    db: &PgPool,
    post_id: i64,
    actor_id: i64,
    local_domain: &str,
    ap_private_key_pem: &str,
    option_names: &[String],
) -> Result<(), ApError> {
    let row = sqlx::query(
        "SELECT p.ap_object_id, a.ap_inbox_url, a.ap_uri
         FROM posts p JOIN actors a ON a.id = p.actor_id
         WHERE p.id = $1 AND p.deleted_at IS NULL",
    )
    .bind(post_id)
    .fetch_optional(db)
    .await
    .map_err(|e| ApError::Other(format!("アンケート配送先取得エラー: {}", e)))?;
    let Some(row) = row else { return Ok(()) };
    let Some(question_id): Option<String> = row.try_get("ap_object_id").unwrap_or(None) else {
        return Ok(());
    };
    let Some(inbox): Option<String> = row.try_get("ap_inbox_url").unwrap_or(None) else {
        return Ok(());
    };
    let Some(author_uri): Option<String> = row.try_get("ap_uri").unwrap_or(None) else {
        return Ok(());
    };

    let username = fetch_username(db, actor_id).await?;
    let addr = local_actor_address(local_domain, &username);
    for (index, name) in option_names.iter().enumerate() {
        let activity_id = format!(
            "https://{}/activities/poll-vote-{}-{}-{}",
            local_domain, post_id, actor_id, index
        );
        let note_id = format!("{}/note", activity_id);
        let activity = serde_json::json!({
            "@context": "https://www.w3.org/ns/activitystreams",
            "id": activity_id,
            "type": "Create",
            "actor": addr.actor_uri,
            "to": [author_uri],
            "object": {
                "id": note_id,
                "type": "Note",
                "attributedTo": addr.actor_uri,
                "name": name,
                "inReplyTo": question_id,
                "to": [author_uri]
            }
        });
        fan_out_activity(
            ap_client,
            std::slice::from_ref(&inbox),
            &activity,
            &addr.key_id,
            ap_private_key_pem,
            &format!("PollVote post_id={} actor_id={}", post_id, actor_id),
        )
        .await?;
    }
    Ok(())
}

/// ローカルアクターの絵文字リアクション取消（Undo(Like)/Undo(EmojiReact)）を、
/// `deliver_ap_reaction` と同じ宛先集合（対象ポスト著者 + reactor 本人の Fedi フォロワー）へ配送する。
///
/// `prev_activity_id` / `content` は取り消し対象の元リアクションのもの
/// （`reactions.ap_activity_id` に保存されていた値とその時点の `content`）を渡すこと。
#[allow(clippy::too_many_arguments)]
pub async fn deliver_ap_undo_reaction(
    ap_client: &ApClient,
    db: &PgPool,
    post_id: i64,
    actor_id: i64,
    local_domain: &str,
    ap_private_key_pem: &str,
    prev_activity_id: &str,
    content: &str,
    emoji_url: Option<&str>,
) -> Result<(), ApError> {
    let (object_ap_id, inboxes) = match resolve_reaction_targets(db, post_id, actor_id).await? {
        Some(v) => v,
        None => return Ok(()),
    };

    let username = fetch_username(db, actor_id).await?;
    let addr = local_actor_address(local_domain, &username);
    let activity_type = reaction_activity_type(content);
    let inner = build_reaction_object(
        activity_type,
        prev_activity_id,
        &addr.actor_uri,
        &object_ap_id,
        content,
        emoji_url,
        local_domain,
    );

    let undo_id = format!(
        "https://{}/activities/undo-reactions/{}-{}-{}",
        local_domain,
        post_id,
        actor_id,
        chrono::Utc::now().timestamp_millis()
    );
    let activity =
        build_undo_reaction_activity(&addr, &undo_id, &chrono::Utc::now().to_rfc3339(), inner);

    fan_out_activity(
        ap_client,
        &inboxes,
        &activity,
        &addr.key_id,
        ap_private_key_pem,
        &format!(
            "Undo({}) post_id={} actor_id={}",
            activity_type, post_id, actor_id
        ),
    )
    .await
}

/// HTML の特殊文字をエスケープする（`plain_to_html`／`plain_to_html_with_mentions` 共通）。
fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// プレーンテキストを ActivityPub 向け HTML に変換する
///
/// 空行で段落分割し、改行を `<br>` に変換する。
pub fn plain_to_html(text: &str) -> String {
    let paragraphs: Vec<String> = text
        .split("\n\n")
        .map(|para| format!("<p>{}</p>", escape_html(para).replace('\n', "<br>")))
        .collect();
    paragraphs.join("")
}

/// プレーンテキストを ActivityPub 向け HTML に変換する（メンション/リンク span 対応版）。
///
/// `mentions` の `byte_start`/`byte_end`（`text` に対する UTF-8 バイトオフセット）区間を
/// `<a href="...">` に置き換えてから、`plain_to_html` と同じ段落分割・改行変換を行う。
/// `mentions` は `crate::mention::convert_mentions_for_ap` の戻り値をそのまま渡す想定
/// （byte_start 昇順・非重複であること）。
pub fn plain_to_html_with_mentions(
    text: &str,
    mentions: &[crate::mention::ApInlineMention],
) -> String {
    let mut linked = String::with_capacity(text.len() * 2);
    let mut last = 0usize;
    for m in mentions {
        if m.byte_start < last || m.byte_end > text.len() || m.byte_start > m.byte_end {
            // 不正な範囲（呼び出し側のバグ等）はそのスパンだけ無視して安全側に倒す
            continue;
        }
        linked.push_str(&escape_html(&text[last..m.byte_start]));
        let rel = match m.kind {
            crate::mention::ApInlineSpanKind::Mention => {
                r#" class="mention u-url" rel="nofollow noopener""#
            }
            // Mastodon 等が実際に送ってくる形式（`class="mention hashtag" rel="tag"`）に合わせる。
            // 受信側の `ap_content_to_markdown_body` はこの形式のアンカーを `#foo` として
            // 解決できることを確認済み（`docs/protocols.md` 6節・`jobs::inbound_activity_process`
            // のテスト参照）。
            crate::mention::ApInlineSpanKind::Hashtag => r#" class="mention hashtag" rel="tag""#,
            crate::mention::ApInlineSpanKind::Link => r#" rel="nofollow noopener""#,
        };
        linked.push_str(&format!(
            r#"<a href="{}"{}>{}</a>"#,
            escape_html(&m.href),
            rel,
            escape_html(&m.name)
        ));
        last = m.byte_end;
    }
    linked.push_str(&escape_html(&text[last..]));

    let paragraphs: Vec<String> = linked
        .split("\n\n")
        .map(|para| format!("<p>{}</p>", para.replace('\n', "<br>")))
        .collect();
    paragraphs.join("")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addr() -> LocalActorAddress {
        local_actor_address("seiran.example", "alice")
    }

    #[test]
    fn local_actor_address_builds_uris() {
        let a = addr();
        assert_eq!(a.actor_uri, "https://seiran.example/users/alice");
        assert_eq!(a.key_id, "https://seiran.example/users/alice#main-key");
        assert_eq!(
            a.followers_uri,
            "https://seiran.example/users/alice/followers"
        );
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
                visibility: "public",
                tag: vec![],
                direct_recipients: &[],
                mention_recipients: &[],
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
                visibility: "unlisted",
                tag: vec![],
                direct_recipients: &[],
                mention_recipients: &[],
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
                visibility: "followers_only",
                tag: vec![],
                direct_recipients: &[],
                mention_recipients: &[],
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
                visibility: "public",
                tag: vec![],
                direct_recipients: &[],
                mention_recipients: &[],
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
                visibility: "public",
                tag: vec![],
                direct_recipients: &[],
                mention_recipients: &["https://other.example/users/bob".to_string()],
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
                visibility: "followers_only",
                tag: vec![],
                direct_recipients: &[],
                mention_recipients: &["https://other.example/users/bob".to_string()],
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
        let react = build_reaction_object(
            "EmojiReact",
            "id1",
            "actor1",
            "obj1",
            ":blobcat:",
            Some("https://example.com/blobcat.png"),
            "seiran.example",
        );
        assert_eq!(react["content"], ":blobcat:");
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
    fn test_plain_to_html_single_paragraph() {
        assert_eq!(plain_to_html("Hello"), "<p>Hello</p>");
        assert_eq!(plain_to_html("Hello, world!"), "<p>Hello, world!</p>");
    }

    #[test]
    fn test_plain_to_html_double_newline() {
        assert_eq!(plain_to_html("Hello\n\nWorld"), "<p>Hello</p><p>World</p>");
        assert_eq!(
            plain_to_html("First\n\nSecond\n\nThird"),
            "<p>First</p><p>Second</p><p>Third</p>"
        );
        // 単一改行は <br> になる
        assert_eq!(plain_to_html("line1\nline2"), "<p>line1<br>line2</p>");
    }

    #[test]
    fn test_plain_to_html_no_xss() {
        let result = plain_to_html("<script>alert(1)</script>");
        // <script> タグがそのままHTMLとして出力されないこと
        assert!(!result.contains("<script>"));
        assert!(!result.contains("</script>"));
        assert!(result.contains("&lt;script&gt;"));
        assert_eq!(result, "<p>&lt;script&gt;alert(1)&lt;/script&gt;</p>");
    }

    #[test]
    fn plain_to_html_with_mentions_wraps_mention_in_anchor() {
        let text = "hello @alice@seiran.example bye";
        let mentions = [crate::mention::ApInlineMention {
            byte_start: 6,
            byte_end: 27,
            href: "https://seiran.example/users/alice".to_string(),
            name: "@alice@seiran.example".to_string(),
            kind: crate::mention::ApInlineSpanKind::Mention,
        }];
        let html = plain_to_html_with_mentions(text, &mentions);
        assert_eq!(
            html,
            r#"<p>hello <a href="https://seiran.example/users/alice" class="mention u-url" rel="nofollow noopener">@alice@seiran.example</a> bye</p>"#
        );
    }

    #[test]
    fn plain_to_html_with_mentions_non_mention_link_omits_mention_class() {
        let text = "see alice.bsky.social";
        let mentions = [crate::mention::ApInlineMention {
            byte_start: 4,
            byte_end: 21,
            href: "https://bsky.app/profile/alice.bsky.social".to_string(),
            name: "alice.bsky.social".to_string(),
            kind: crate::mention::ApInlineSpanKind::Link,
        }];
        let html = plain_to_html_with_mentions(text, &mentions);
        assert!(!html.contains("class=\"mention"));
        assert!(html.contains(r#"<a href="https://bsky.app/profile/alice.bsky.social" rel="nofollow noopener">alice.bsky.social</a>"#));
    }

    #[test]
    fn plain_to_html_with_mentions_escapes_surrounding_text() {
        let text = "<b>@alice</b>";
        let mentions = [crate::mention::ApInlineMention {
            byte_start: 3,
            byte_end: 9,
            href: "https://seiran.example/users/alice".to_string(),
            name: "@alice".to_string(),
            kind: crate::mention::ApInlineSpanKind::Mention,
        }];
        let html = plain_to_html_with_mentions(text, &mentions);
        assert!(html.starts_with("<p>&lt;b&gt;<a "));
        assert!(html.ends_with("</a>&lt;/b&gt;</p>"));
    }

    #[test]
    fn plain_to_html_with_mentions_out_of_range_span_is_skipped() {
        let text = "hi";
        let mentions = [crate::mention::ApInlineMention {
            byte_start: 0,
            byte_end: 100, // text の範囲外
            href: "https://example.com".to_string(),
            name: "x".to_string(),
            kind: crate::mention::ApInlineSpanKind::Mention,
        }];
        let html = plain_to_html_with_mentions(text, &mentions);
        assert_eq!(html, "<p>hi</p>");
    }

    #[test]
    fn plain_to_html_with_mentions_preserves_newlines() {
        let text = "@alice\nsecond line";
        let mentions = [crate::mention::ApInlineMention {
            byte_start: 0,
            byte_end: 6,
            href: "https://seiran.example/users/alice".to_string(),
            name: "@alice".to_string(),
            kind: crate::mention::ApInlineSpanKind::Mention,
        }];
        let html = plain_to_html_with_mentions(text, &mentions);
        assert!(html.contains("<br>second line"));
    }
}

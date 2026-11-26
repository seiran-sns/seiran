//! 受信した投稿の付帯情報（添付・CW/アンケート・ハッシュタグ・URLカード・返信/引用可否）を、
//! 投稿本体の INSERT と同じトランザクションで書く。
//!
//! 投稿本体だけを先に確定させ、付帯情報を別のクエリで「失敗したらログだけ出して飛ばす」形に
//! していると、DB 接続プールの枯渇などの一時的な失敗で添付の欠けた投稿が残る。しかも次の
//! 試行は「投稿は既にある」として何もしないので、欠けたまま直らない。本体と付帯情報を
//! 1つのトランザクションにまとめれば、失敗したときは何も残らず、呼び出し元のリトライで
//! 最初からやり直せる。
//!
//! ここに渡す値は、トランザクションを始める前にすべて揃えておくこと。外部への HTTP 呼び出し
//! （gate の取得など）やジョブキューへの積み込みをトランザクションの中で行うと、その間ずっと
//! DB 接続とロックを握り続けることになる。この関数は SQL しか発行しない。

use sqlx::PgConnection;

/// リモート投稿の添付1件（実体は取り込まず URL だけ保持する）。並び順は `PostExtras::attachments`
/// の添字。
#[derive(Debug, Clone, PartialEq)]
pub struct NewRemoteAttachment {
    pub url: String,
    pub mime_type: Option<String>,
    pub thumbnail_url: Option<String>,
    pub is_sensitive: bool,
    pub is_gif: bool,
}

/// 送信側が申告済みの URL カード1件（`post_link_cards`）。並び順は `PostExtras::link_cards` の添字。
#[derive(Debug, Clone, PartialEq)]
pub struct NewLinkCardInput {
    pub url: String,
    pub title: String,
    pub description: String,
    pub thumbnail_url: Option<String>,
}

/// 返信許可（threadgate）・引用可否（postgate）。
#[derive(Debug, Clone, PartialEq)]
pub struct BskyGates {
    pub reply_allow: Option<serde_json::Value>,
    pub quote_disabled: bool,
}

/// 投稿本体と同じトランザクションで書く付帯情報。
#[derive(Debug, Clone, Default)]
pub struct PostExtras {
    /// `Some` なら `posts.content_warning`/`poll` をこの値で上書きする。`None` なら触らない。
    pub content_metadata: Option<ContentMetadata>,
    /// ハッシュタグを抽出する本文。`None` ならハッシュタグを付けない。
    pub hashtag_text: Option<String>,
    pub attachments: Vec<NewRemoteAttachment>,
    pub link_cards: Vec<NewLinkCardInput>,
    /// `Some` なら `posts.bsky_reply_allow`/`bsky_quote_disabled` をこの値にする。
    pub bsky_gates: Option<BskyGates>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ContentMetadata {
    pub content_warning: Option<String>,
    pub poll: Option<serde_json::Value>,
}

/// `post_id` の投稿に付帯情報を書く。`conn` は投稿本体を INSERT したトランザクションを渡す。
///
/// どの INSERT も `ON CONFLICT DO NOTHING` にしてある。seiranPost の相互一致マージ（#237）で
/// 既存の行へ付けるときに、相手側が先に付けた添付・URLカードと位置がぶつかっても、
/// トランザクション全体を失敗させないため。
pub async fn apply_post_extras(
    conn: &mut PgConnection,
    post_id: i64,
    extras: &PostExtras,
) -> Result<(), sqlx::Error> {
    if let Some(meta) = &extras.content_metadata {
        sqlx::query(
            "UPDATE posts SET content_warning = $2, poll = $3,
                 poll_fetched_at = CASE WHEN $3 IS NOT NULL THEN created_at ELSE poll_fetched_at END
             WHERE id = $1",
        )
        .bind(post_id)
        .bind(meta.content_warning.as_deref())
        .bind(meta.poll.as_ref())
        .execute(&mut *conn)
        .await?;
    }

    if let Some(text) = &extras.hashtag_text {
        crate::repository::hashtag::link_post_in(&mut *conn, post_id, text).await?;
    }

    for (position, att) in extras.attachments.iter().enumerate() {
        sqlx::query(
            "INSERT INTO post_attachments (post_id, media_file_id, remote_url, remote_mime_type, remote_thumbnail_url, is_sensitive, is_gif, position)
             VALUES ($1, NULL, $2, $3, $4, $5, $6, $7)
             ON CONFLICT (post_id, position) DO NOTHING",
        )
        .bind(post_id)
        .bind(&att.url)
        .bind(att.mime_type.as_deref())
        .bind(att.thumbnail_url.as_deref())
        .bind(att.is_sensitive)
        .bind(att.is_gif)
        .bind(position as i16)
        .execute(&mut *conn)
        .await?;
    }

    for (position, card) in extras.link_cards.iter().enumerate() {
        sqlx::query(
            "INSERT INTO post_link_cards (post_id, position, url, title, description, thumbnail_url, embed_src, embed_type)
             VALUES ($1, $2, $3, $4, $5, $6, NULL, NULL)
             ON CONFLICT (post_id, position) DO NOTHING",
        )
        .bind(post_id)
        .bind(position as i16)
        .bind(&card.url)
        .bind(&card.title)
        .bind(&card.description)
        .bind(card.thumbnail_url.as_deref())
        .execute(&mut *conn)
        .await?;
    }

    if let Some(gates) = &extras.bsky_gates {
        sqlx::query(
            "UPDATE posts SET bsky_reply_allow = $1, bsky_quote_disabled = $2 WHERE id = $3",
        )
        .bind(gates.reply_allow.as_ref())
        .bind(gates.quote_disabled)
        .bind(post_id)
        .execute(&mut *conn)
        .await?;
    }

    Ok(())
}

/// seiranPost の申告済み URL カード（`linkCards[]`）を `PostExtras::link_cards` の形にする。
/// 件数は `crate::seiran_post` の上限で打ち切る。
pub fn link_cards_from_seiran_post(
    cards: &[crate::seiran_post::SeiranPostLinkCard],
) -> Vec<NewLinkCardInput> {
    cards
        .iter()
        .take(crate::seiran_post::MAX_LINK_CARDS_PER_POST)
        .map(|card| NewLinkCardInput {
            url: card.url.clone(),
            title: card.title.clone(),
            description: card.description.clone(),
            thumbnail_url: card.thumbnail_url.clone(),
        })
        .collect()
}

/// 解析済みの添付（`crate::atp::ParsedAttachment`）と NSFW 指定を `NewRemoteAttachment` にする。
pub fn attachments_from_parsed(
    attachments: &[(crate::atp::ParsedAttachment, bool)],
) -> Vec<NewRemoteAttachment> {
    attachments
        .iter()
        .map(|(att, is_sensitive)| NewRemoteAttachment {
            url: att.url.clone(),
            mime_type: Some(att.mime_type.clone()),
            thumbnail_url: att.thumbnail_url.clone(),
            is_sensitive: *is_sensitive,
            is_gif: att.is_gif,
        })
        .collect()
}

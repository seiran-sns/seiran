//! URL・AT URI・ユーザーIDを、Fedi/Bskyのアクター・投稿へ解決する共通ロジック。
//!
//! 元は`seiran-api`の`handlers::open_target`（SPA内リンク遷移、Misskey互換API `ap_show`とも
//! 共有）にあったが、bio/profile_fields内リンクのバックグラウンド解決（`jobs::link_resolve`）
//! からも同じURL分類・解決処理が必要になったため、`seiran-common`へ移動した
//! （挙動不変のリファクタ）。呼び出し元がそれぞれの応答形（SPA遷移パス・Misskey互換JSON・
//! `link_resolutions`キャッシュ行）へ変換する。

use std::sync::Arc;

use crate::ap::ApClient;
use crate::net::{fetch_validated_with_accept, FetchError};
use crate::repository::{Actor, ActorRepository, PostRepository};
use crate::target_resolve::{self, TargetResolveContext};
use crate::traits::{Job, JobQueue};
use crate::{job_priority, ApError};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParsedTarget {
    BskyPost(String),
    Actor(String),
    ActivityPubUrl(String),
}

/// `resolve_target`の解決結果。
pub enum ResolvedTarget {
    Actor(Box<Actor>),
    Post(i64),
}

#[derive(Debug)]
pub enum ResolveError {
    /// 解析・解決できない対象（未対応の形式・未対応のオブジェクト種別）。
    Invalid,
    /// 上流（リモートサーバー）からの取得に失敗。
    Upstream(String),
    /// DB等、内部エラー。
    Internal(String),
    /// Note取り込みをenqueueしたが、待機時間内に取り込みが完了しなかった
    /// （取り込み自体は継続しているため、呼び出し元は「後で確認してください」を返すこと）。
    ImportPending,
}

impl From<ApError> for ResolveError {
    fn from(e: ApError) -> Self {
        ResolveError::Upstream(e.to_string())
    }
}

impl From<FetchError> for ResolveError {
    fn from(e: FetchError) -> Self {
        ResolveError::Upstream(e.to_string())
    }
}

/// `raw`（URL・AT URI・ユーザーID等）をパースする。`https://bsky.app/profile/...`は
/// Bskyの投稿/アクターへ、`at://`は投稿AT URIへ、`did:plc:`/`@acct`はアクターへ、
/// それ以外の`http(s)://`はActivityPubオブジェクトURLとして分類する。
pub fn parse_target(raw: &str) -> Option<ParsedTarget> {
    let target = raw.trim();
    if target.starts_with("at://") {
        return parse_at_post_uri(target).then(|| ParsedTarget::BskyPost(target.to_string()));
    }
    if target.starts_with("did:plc:") {
        return Some(ParsedTarget::Actor(target.to_string()));
    }
    if target.starts_with('@') {
        let acct = target.trim_start_matches('@');
        return (!acct.is_empty()).then(|| ParsedTarget::Actor(target.to_string()));
    }

    let url = url::Url::parse(target).ok()?;
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }
    if url.host_str() == Some("bsky.app") {
        let parts: Vec<_> = url
            .path_segments()?
            .filter(|part| !part.is_empty())
            .collect();
        return match parts.as_slice() {
            ["profile", actor, "post", rkey] => Some(ParsedTarget::BskyPost(format!(
                "at://{actor}/app.bsky.feed.post/{rkey}"
            ))),
            ["profile", actor] => Some(ParsedTarget::Actor((*actor).to_string())),
            _ => None,
        };
    }
    Some(ParsedTarget::ActivityPubUrl(target.to_string()))
}

/// bio/profile_fields値（プレーンテキスト、またはサニタイズ済みHTMLの`href`属性値を含む
/// 生文字列）から、非同期解決の対象となりうる`http(s)://`URL・メンション記法
/// （`@user`/`@user@host`/`@handle.bsky.social`）を重複除去して抽出する（#リンク解決）。
/// メンション記法は`link_target::parse_target`が`ParsedTarget::Actor`として扱える形式のため、
/// URLと同じ`resolve_target`パイプラインへそのまま渡せる。
///
/// URL・メンションの2パターンを1回の走査でまとめて検出する（別々に2回走査すると、
/// `https://mastodon.social/@alice`のようなURLのパス部分に含まれる`@alice`を、URLとは
/// 独立に単体のメンションとして誤検出してしまう。1回の走査ならURLの方が左側から長く
/// マッチするため、その中の`@`を単独メンションの開始位置として再度検討することがない）。
/// メンション側は`frontend/src/lib/richTextPatterns.ts`の`MENTION_SOURCE`と同じ意味論
/// （直前がASCII英数字・アンダースコアの場合はメールアドレスの一部とみなしマッチしない。
/// 日本語等のCJK文字は「単語構成文字」扱いしない）だが、Rustの`regex`クレートは後読み
/// （lookbehind）をサポートしないため、直前の1文字を`(?:^|[^A-Za-z0-9_@])`として同じ
/// マッチに含めて判定する（`\w`はUnicode対応で日本語も単語構成文字とみなしてしまうため、
/// ASCII英数字・アンダースコアのみを明示的に除外する）。
pub fn extract_link_targets(text: &str) -> Vec<String> {
    static COMBINED_RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let re = COMBINED_RE.get_or_init(|| {
        regex::Regex::new(concat!(
            r#"(?P<url>https?://[^\s"'<>]+)"#,
            r"|(?:^|[^A-Za-z0-9_@])@(?P<mention>[A-Za-z0-9_-]+(?:\.[A-Za-z0-9-]+)*(?:@[A-Za-z0-9.-]+)?)",
        ))
        .expect("static regex must compile")
    });

    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for cap in re.captures_iter(text) {
        let target = if let Some(m) = cap.name("url") {
            m.as_str().to_string()
        } else if let Some(m) = cap.name("mention") {
            format!("@{}", m.as_str())
        } else {
            continue;
        };
        if seen.insert(target.clone()) {
            out.push(target);
        }
    }
    out
}

fn parse_at_post_uri(target: &str) -> bool {
    let parts: Vec<_> = target
        .trim_start_matches("at://")
        .split('/')
        .filter(|part| !part.is_empty())
        .collect();
    matches!(parts.as_slice(), [_, "app.bsky.feed.post", _])
}

/// `resolve_target`が必要とする依存の束。`seiran-api`の`AppState`・`seiran-common`の
/// ジョブ（`JobContext`）のどちらからも組み立てられる。
pub struct ResolveContext<'a> {
    pub actors: &'a dyn ActorRepository,
    pub posts: &'a dyn PostRepository,
    pub ap_client: &'a ApClient,
    pub job_queue: &'a Arc<dyn JobQueue>,
    pub db_pool: &'a sqlx::PgPool,
    pub local_domain: &'a str,
    pub system_signing_key: Option<(String, String)>,
}

impl ResolveContext<'_> {
    fn target_resolve_ctx(&self) -> TargetResolveContext<'_> {
        TargetResolveContext {
            actors: self.actors,
            ap_client: self.ap_client,
            local_domain: self.local_domain,
            system_signing_key: self.system_signing_key.clone(),
        }
    }
}

/// `target`（URL・AT URI・ユーザーID等）を解決する。ローカルDBに無ければ取り込み（フェッチ
/// ＋保存）まで行う。
pub async fn resolve_target(
    ctx: &ResolveContext<'_>,
    parsed: ParsedTarget,
) -> Result<ResolvedTarget, ResolveError> {
    match parsed {
        ParsedTarget::BskyPost(at_uri) => resolve_bsky_post(ctx, &at_uri).await,
        ParsedTarget::Actor(target) => resolve_actor(ctx, &target).await,
        ParsedTarget::ActivityPubUrl(url) => resolve_activitypub_url(ctx, &url).await,
    }
}

async fn resolve_actor(
    ctx: &ResolveContext<'_>,
    target: &str,
) -> Result<ResolvedTarget, ResolveError> {
    let actor = target_resolve::resolve_and_upsert_target(&ctx.target_resolve_ctx(), target)
        .await
        .map_err(|_| ResolveError::Invalid)?;
    Ok(ResolvedTarget::Actor(Box::new(actor)))
}

async fn resolve_bsky_post(
    ctx: &ResolveContext<'_>,
    at_uri: &str,
) -> Result<ResolvedTarget, ResolveError> {
    // bsky.app URLではハンドルがauthorityになる場合があるため、プロフィール取得でDIDへ正規化する。
    let parts: Vec<_> = at_uri.trim_start_matches("at://").split('/').collect();
    let profile = crate::atp::fetch_bsky_profile(&ctx.ap_client.http, parts[0])
        .await
        .map_err(|_| ResolveError::Invalid)?;
    let canonical_uri = format!("at://{}/app.bsky.feed.post/{}", profile.did, parts[2]);
    let post = crate::atp::fetch_single_bsky_post(&ctx.ap_client.http, &canonical_uri)
        .await
        .map_err(ResolveError::Upstream)?
        .ok_or(ResolveError::Invalid)?;
    let actor =
        target_resolve::resolve_and_upsert_target(&ctx.target_resolve_ctx(), &post.author_did)
            .await
            .map_err(|_| ResolveError::Invalid)?;
    let post_id = crate::atp::upsert_bsky_post(
        ctx.db_pool,
        ctx.job_queue,
        &ctx.ap_client.http,
        actor.id,
        &post,
    )
    .await
    .map_err(|e| ResolveError::Internal(format!("Bsky post保存失敗: {e}")))?;
    Ok(ResolvedTarget::Post(post_id))
}

async fn resolve_activitypub_url(
    ctx: &ResolveContext<'_>,
    url: &str,
) -> Result<ResolvedTarget, ResolveError> {
    if let Some(post_id) = ctx
        .posts
        .find_id_by_ap_or_at_uri(url)
        .await
        .map_err(|e| ResolveError::Internal(format!("投稿検索失敗: {e}")))?
    {
        return Ok(ResolvedTarget::Post(post_id));
    }

    let (body, _) = fetch_validated_with_accept(
        url,
        &[
            "application/activity+json",
            "application/ld+json",
            "application/json",
        ],
        "application/activity+json, application/ld+json",
    )
    .await?;
    let object: serde_json::Value =
        serde_json::from_slice(&body).map_err(|_| ResolveError::Invalid)?;
    let object_type = object["type"].as_str().unwrap_or("");
    if matches!(
        object_type,
        "Person" | "Service" | "Application" | "Organization" | "Group"
    ) {
        return resolve_actor(ctx, object["id"].as_str().unwrap_or(url)).await;
    }
    // Misskeyの素リノート（コメント無しブースト）は、notes URLへの直接アクセスや他鯖ミラー
    // URLからの302リダイレクトの結果として`Announce`（`object`は対象ノートのURI文字列）に
    // 行き着く。通常投稿としてではなく正しくリポストラッパーとして取り込む。
    if object_type == "Announce" {
        return resolve_announce(ctx, url, object).await;
    }
    if !matches!(object_type, "Note" | "Article" | "Question" | "Page") {
        return Err(ResolveError::Invalid);
    }

    let note_id = object["id"].as_str().unwrap_or(url);
    let actor = object["attributedTo"]
        .as_str()
        .or_else(|| object["attributedTo"].as_array()?.first()?.as_str())
        .ok_or(ResolveError::Invalid)?;
    let activity = serde_json::json!({
        "@context": "https://www.w3.org/ns/activitystreams",
        "id": format!("{note_id}#seiran-open"),
        "type": "Create",
        "actor": actor,
        "object": object,
    });
    enqueue_and_await_import(ctx, activity, note_id).await
}

/// フェッチしたAnnounce（Misskeyの素リノート・他鯖ミラー経由でのAnnounce解決を含む）を
/// リポストラッパーとして取り込む。既存のCreate用合成ラップとは異なり、フェッチしたAnnounce
/// オブジェクト自体が`handle_announce`の期待する形（`id`/`actor`/`object`/`to`/`cc`/`published`）
/// を満たすため、そのまま`InboundActivityProcess`へ積む。対象ポスト（`object`）が未取得なら
/// 受信側の参照解決が1段階だけフェッチする。対象の取得に失敗してもリポストの箱自体は保存
/// されるため、ここでの完了待ちは箱の保存だけを待てば良い。
async fn resolve_announce(
    ctx: &ResolveContext<'_>,
    url: &str,
    announce: serde_json::Value,
) -> Result<ResolvedTarget, ResolveError> {
    let announce_id = announce["id"].as_str().unwrap_or(url).to_string();
    if announce["actor"].as_str().is_none() {
        return Err(ResolveError::Invalid);
    }
    if announce["object"].as_str().is_none() {
        return Err(ResolveError::Invalid);
    }
    enqueue_and_await_import(ctx, announce, &announce_id).await
}

/// `Job::InboundActivityProcess`へ積み、`dedup_uri`（`ap_object_id`として保存されるはずの
/// URI）で該当投稿が保存されるまで短時間だけポーリングする。Note（Create経由）・
/// Announce（リポスト経由）の両方の「開く」経路で共有する。
async fn enqueue_and_await_import(
    ctx: &ResolveContext<'_>,
    activity: serde_json::Value,
    dedup_uri: &str,
) -> Result<ResolvedTarget, ResolveError> {
    ctx.job_queue
        .enqueue(
            Job::InboundActivityProcess {
                raw_activity: activity.to_string(),
            },
            job_priority::HIGH,
        )
        .await
        .map_err(|e| ResolveError::Internal(format!("投稿取り込みキュー投入失敗: {e}")))?;

    // インバウンド処理は既存のCreate/Announce経路を再利用する。短時間だけ完了を待ち、確実に解決する。
    for _ in 0..40 {
        if let Some(post_id) = ctx
            .posts
            .find_id_by_ap_or_at_uri(dedup_uri)
            .await
            .map_err(|e| ResolveError::Internal(format!("投稿検索失敗: {e}")))?
        {
            return Ok(ResolvedTarget::Post(post_id));
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    Err(ResolveError::ImportPending)
}

#[cfg(test)]
mod tests {
    use super::{extract_link_targets, parse_target, ParsedTarget};

    #[test]
    fn extract_urls_dedupes_and_finds_bare_and_href_urls() {
        let text = r#"見て <a href="https://mastodon.social/@alice">こちら</a> あと https://mastodon.social/@alice も同じ"#;
        assert_eq!(
            extract_link_targets(text),
            vec!["https://mastodon.social/@alice"]
        );
    }

    #[test]
    fn extract_mentions_finds_fedi_acct_and_bsky_handle_and_dedupes() {
        let text = "よろしく @alice@mastodon.social です。Bskyは@bob.bsky.social、あと@alice@mastodon.socialも同じ人。メールuser@example.comは無視。";
        assert_eq!(
            extract_link_targets(text),
            vec!["@alice@mastodon.social", "@bob.bsky.social"]
        );
    }

    #[test]
    fn extract_mentions_ignores_email_like_text() {
        assert!(extract_link_targets("contact: user@example.com").is_empty());
    }

    #[test]
    fn parses_bsky_post_url() {
        let ParsedTarget::BskyPost(uri) =
            parse_target("https://bsky.app/profile/alice.test/post/3abc").unwrap()
        else {
            panic!("post expected");
        };
        assert_eq!(uri, "at://alice.test/app.bsky.feed.post/3abc");
    }

    #[test]
    fn rejects_unrelated_bsky_url_and_non_post_at_uri() {
        assert!(parse_target("https://bsky.app/settings").is_none());
        assert!(parse_target("at://did:plc:x/app.bsky.actor.profile/self").is_none());
    }
}

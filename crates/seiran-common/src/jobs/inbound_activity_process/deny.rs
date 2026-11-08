//! 拒否サーバー（`crate::denied_host`）からの受信を捨てるかどうかの判定。
//!
//! 動作はユーザーロール`quarantine`に揃える。
//! - 投稿（Create・Announce）: ローカルの誰かがフォローしているか、リストのメンバーで
//!   なければ捨てる。
//! - 働きかけ（返信・引用・メンション・DM・リアクション・投票）: 宛先のローカルユーザー
//!   本人がフォローしていなければ捨てる。投稿の場合は、1人でも該当すれば投稿ごと捨てる
//!   （quarantine ならその投稿自体が作れないため）。リポストは働きかけに数えない。
//!
//! Follow は`follow.rs`で、Reject を返すために別に扱う。HTTP は 202 のまま受け取って
//! 捨てる（リレー経由で 4xx を返し続けると購読ごと切られるため）。

use super::note_input::extract_ap_quote_uri;
use super::*;

/// `activity`を捨てるべきなら`true`。
pub(super) async fn should_drop(
    activity: &serde_json::Value,
    inbox: &InboxContext,
) -> Result<bool, String> {
    let kind = activity["type"].as_str().unwrap_or("");
    if !matches!(kind, "Create" | "Announce" | "Like" | "EmojiReact") {
        return Ok(false);
    }
    let Some(actor_uri) = activity["actor"].as_str() else {
        return Ok(false);
    };
    let pool = &inbox.db_pool;
    if !crate::denied_host::is_denied_url(pool, actor_uri).await {
        return Ok(false);
    }
    // 未知のアクターは誰にもフォローされていない。
    let Some(actor) = inbox
        .actor_repo
        .find_by_ap_uri(actor_uri)
        .await
        .map_err(|e| format!("アクター検索エラー: {}", e))?
    else {
        return Ok(true);
    };

    let is_post = kind == "Announce" || (kind == "Create" && !is_poll_vote(activity));
    if is_post {
        let followed = crate::repository::denied_host::actor_is_locally_followed(pool, actor.id)
            .await
            .map_err(|e| format!("フォロー関係確認エラー: {}", e))?;
        if !followed {
            return Ok(true);
        }
    }
    if kind == "Announce" {
        return Ok(false);
    }

    let (post_uris, usernames) = interaction_targets(activity, &inbox.local_domain);
    let targets = crate::repository::denied_host::local_actor_ids_referenced(
        pool,
        &post_uris,
        &usernames,
        &inbox.local_domain,
    )
    .await
    .map_err(|e| format!("宛先検索エラー: {}", e))?;
    crate::repository::denied_host::any_local_not_following(pool, &targets, actor.id, false)
        .await
        .map_err(|e| format!("フォロー関係確認エラー: {}", e))
}

/// 受信経路での自動取得（リポスト対象・返信先・引用元の解決）で、`uri`の投稿を取り込んで
/// よいか。拒否サーバーの投稿は、取得済みで、かつ投稿者がローカルからフォローされている
/// 場合だけ認める（取りに行かない）。ユーザーの能動的な取得（「開く」・取得ボタン）は
/// この判定を通らない。
pub(crate) async fn passive_reference_allowed(
    uri: &str,
    inbox: &InboxContext,
) -> Result<bool, String> {
    let pool = &inbox.db_pool;
    if !crate::denied_host::is_denied_url(pool, uri).await {
        return Ok(true);
    }
    let Some((_, author_id)) = inbox
        .post_repo
        .find_id_and_actor_by_ap_object_id(uri)
        .await
        .map_err(|e| format!("投稿検索エラー: {}", e))?
    else {
        return Ok(false);
    };
    crate::repository::denied_host::actor_is_locally_followed(pool, author_id)
        .await
        .map_err(|e| format!("フォロー関係確認エラー: {}", e))
}

/// `mod.rs`の振り分けと同じ、投票（名前と返信先を持つ Note の Create）の判定。
fn is_poll_vote(activity: &serde_json::Value) -> bool {
    activity["object"]["type"].as_str() == Some("Note")
        && activity["object"]["name"].is_string()
        && activity["object"]["inReplyTo"].is_string()
}

/// 働きかけの対象になりうる投稿 URI（返信先・引用元・リアクション対象）と、宛先・
/// メンションに現れる自ドメインのユーザー名を取り出す。
fn interaction_targets(
    activity: &serde_json::Value,
    local_domain: &str,
) -> (Vec<String>, Vec<String>) {
    let mut post_uris = Vec::new();
    let mut usernames = Vec::new();
    let object = &activity["object"];

    if activity["type"].as_str() == Some("Create") {
        let id_of =
            |v: &serde_json::Value| v.as_str().or_else(|| v["id"].as_str()).map(str::to_string);
        post_uris.extend(id_of(&object["inReplyTo"]));
        let tags = object["tag"].as_array().cloned().unwrap_or_default();
        post_uris.extend(extract_ap_quote_uri(object, &tags));

        let mut addressees: Vec<String> = Vec::new();
        for v in [
            &activity["to"],
            &activity["cc"],
            &object["to"],
            &object["cc"],
        ] {
            addressees.extend(as_string_list(v));
        }
        addressees.extend(
            tags.iter()
                .filter(|t| t["type"].as_str() == Some("Mention"))
                .filter_map(|t| t["href"].as_str().map(str::to_string)),
        );
        usernames.extend(
            addressees
                .iter()
                .filter_map(|uri| crate::ap::extract_local_username(uri, local_domain))
                .map(str::to_string),
        );
    } else {
        post_uris.extend(
            object
                .as_str()
                .or_else(|| object["id"].as_str())
                .map(str::to_string),
        );
    }
    post_uris.sort();
    post_uris.dedup();
    usernames.sort();
    usernames.dedup();
    (post_uris, usernames)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collects_reply_quote_and_local_addressees() {
        let activity = serde_json::json!({
            "type": "Create",
            "actor": "https://spam.example/users/a",
            "to": ["https://local.test/users/alice"],
            "object": {
                "type": "Note",
                "inReplyTo": "https://local.test/notes/1",
                "quoteUrl": "https://other.example/notes/2",
                "cc": ["https://spam.example/users/a/followers", "https://other.example/users/x"],
                "tag": [{ "type": "Mention", "href": "https://local.test/users/bob" }]
            }
        });
        let (posts, users) = interaction_targets(&activity, "local.test");
        assert_eq!(
            posts,
            vec![
                "https://local.test/notes/1",
                "https://other.example/notes/2"
            ]
        );
        assert_eq!(users, vec!["alice", "bob"]);
    }

    #[test]
    fn reaction_targets_the_object_post() {
        let activity = serde_json::json!({
            "type": "Like",
            "actor": "https://spam.example/users/a",
            "object": "https://local.test/notes/1"
        });
        let (posts, users) = interaction_targets(&activity, "local.test");
        assert_eq!(posts, vec!["https://local.test/notes/1"]);
        assert!(users.is_empty());
    }
}

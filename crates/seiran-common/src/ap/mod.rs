//! ActivityPub (Fediverse) 統合通信エンジン共通モジュール

pub mod client;
pub mod collection;
pub mod deliver;
pub mod outbox;
pub mod webfinger;

pub use client::{
    build_emoji_map, classify_ap_visibility, ApActor, ApClient, ApError, PublicKeyInfo,
};
pub use collection::fetch_ap_collection_uris;
pub use deliver::{
    apply_poll_to_note_object, deliver_ap_announce, deliver_ap_poll_vote, deliver_ap_reaction,
    deliver_ap_undo_reaction, deliver_delete_actor, deliver_delete_note,
    deliver_direct_message_to_ap, deliver_post_to_ap_followers, deliver_undo_announce,
    deliver_update_actor, plain_to_html, plain_to_html_with_mentions,
};
pub use outbox::{fetch_ap_featured, fetch_ap_history, upsert_ap_note, ApNote};
pub use webfinger::{WebFingerLink, WebFingerResponse};

/// `https://{local_domain}/users/{username}` 形式の Actor URI からユーザー名部分を抽出する。
/// 自ドメインを名乗る Actor URI（配信ループバックやなりすまし）をリモートとして
/// upsert してしまうと、`ap_uri` を持たないローカル行と絶対に一致せず、影の重複
/// `fedi` 行が生成され続ける（#110）。リモートActor解決処理はこの関数で最初に
/// 自ドメイン判定を行い、該当すればローカル行の検索に切り替えること。
pub fn extract_local_username<'a>(actor_uri: &'a str, local_domain: &str) -> Option<&'a str> {
    let prefix = format!("https://{}/users/", local_domain);
    actor_uri
        .strip_prefix(&prefix)
        .filter(|rest| !rest.is_empty() && !rest.contains('/'))
}

#[cfg(test)]
mod tests {
    use super::extract_local_username;

    #[test]
    fn extract_local_username_matches_local_actor_uri() {
        assert_eq!(
            extract_local_username("https://seiran-beta.org/users/yuba", "seiran-beta.org"),
            Some("yuba")
        );
    }

    #[test]
    fn extract_local_username_rejects_other_domain() {
        assert_eq!(
            extract_local_username("https://example.social/users/yuba", "seiran-beta.org"),
            None
        );
    }

    #[test]
    fn extract_local_username_rejects_sub_path() {
        // .../users/{username}/followers のような下位パスはユーザー名そのものではない。
        assert_eq!(
            extract_local_username(
                "https://seiran-beta.org/users/yuba/followers",
                "seiran-beta.org"
            ),
            None
        );
    }

    #[test]
    fn extract_local_username_rejects_empty_username() {
        assert_eq!(
            extract_local_username("https://seiran-beta.org/users/", "seiran-beta.org"),
            None
        );
    }
}

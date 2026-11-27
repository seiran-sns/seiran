//! ATP リポジトリの self レコードで表すプライバシー設定を、ローカルアクターのプロパティ
//! （`actors` の列）へ反映する（ATP → actors）。
//!
//! bsky.app のプライバシー設定画面のスイッチは、PDS へ次のレコードを書く。
//! - 「ログアウトしたユーザーに自分のアカウントを表示しないよう求める」:
//!   `app.bsky.actor.profile/self` の self-label `!no-unauthenticated`
//! - 「他のユーザーにあなたの投稿の通知を許可する」:
//!   `app.bsky.notification.declaration/self` の `allowSubscriptions`
//! - 「アルゴリズムのおすすめから除外するよう求める」:
//!   `app.bsky.actor.contentVisibilityDeclaration/self` の `hideFromAlgorithmicRecommendations`
//!
//! seiran はこれらを `actors` に持ち、seiran 側から self レコードをコミットし直すとき
//! （`AtpCommitService::commit_profile` 等）はそこから組み立てる。外部クライアントの書き込みを
//! `actors` へ反映しないと、次に seiran 側からコミットしたときに外部で変えた設定が巻き戻る。
//!
//! 反映は、書き込まれた値ではなくリポジトリにある今のレコードを読み直して行う。書き込み・削除・
//! DID 転入・既存データの移行のどれも同じ関数で済み、レコードと `actors` が食い違わない。

use crate::atp::repo::decode_dagcbor_to_json;
use crate::repository::{atp, ActorRepository, AtpSelfSettings, PgActorRepository};
use sqlx::PgPool;

pub const PROFILE_COLLECTION: &str = "app.bsky.actor.profile";
pub const NOTIFICATION_DECLARATION_COLLECTION: &str = "app.bsky.notification.declaration";
pub const CONTENT_VISIBILITY_COLLECTION: &str = "app.bsky.actor.contentVisibilityDeclaration";

/// ログアウトしたユーザーに表示しないよう求める self-label。
pub const NO_UNAUTHENTICATED_LABEL: &str = "!no-unauthenticated";

/// `allowSubscriptions` が取りうる値。レコードが無い・値が不明なら `followers`（公式の既定値）。
pub const ALLOW_SUBSCRIPTIONS_VALUES: [&str; 3] = ["followers", "mutuals", "none"];
const DEFAULT_ALLOW_SUBSCRIPTIONS: &str = "followers";

/// このレコードの変更で `actors` のプライバシー設定が変わりうるか。
pub fn is_self_settings_record(collection: &str, rkey: &str) -> bool {
    rkey == "self"
        && matches!(
            collection,
            PROFILE_COLLECTION
                | NOTIFICATION_DECLARATION_COLLECTION
                | CONTENT_VISIBILITY_COLLECTION
        )
}

/// プロフィールレコードの self-label に `!no-unauthenticated` があるか。レコードが無ければ偽。
pub fn hides_from_unauthenticated(profile: Option<&serde_json::Value>) -> bool {
    profile
        .and_then(|p| p.get("labels"))
        .and_then(|l| l.get("values"))
        .and_then(|v| v.as_array())
        .is_some_and(|values| {
            values
                .iter()
                .any(|v| v.get("val").and_then(|s| s.as_str()) == Some(NO_UNAUTHENTICATED_LABEL))
        })
}

/// 通知の宣言レコードの `allowSubscriptions`。
pub fn allow_activity_subscriptions(declaration: Option<&serde_json::Value>) -> &'static str {
    let value = declaration
        .and_then(|d| d.get("allowSubscriptions"))
        .and_then(|v| v.as_str());
    ALLOW_SUBSCRIPTIONS_VALUES
        .into_iter()
        .find(|known| Some(*known) == value)
        .unwrap_or(DEFAULT_ALLOW_SUBSCRIPTIONS)
}

/// 表示の宣言レコードの `hideFromAlgorithmicRecommendations`。レコードが無ければ偽（公式の既定値）。
pub fn hides_from_algorithmic_recommendations(declaration: Option<&serde_json::Value>) -> bool {
    declaration
        .and_then(|d| d.get("hideFromAlgorithmicRecommendations"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

/// 3つの self レコードから設定を組み立てる。
pub fn settings_from_records(
    profile: Option<&serde_json::Value>,
    notification_declaration: Option<&serde_json::Value>,
    content_visibility: Option<&serde_json::Value>,
) -> AtpSelfSettings {
    AtpSelfSettings {
        hide_from_unauthenticated: hides_from_unauthenticated(profile),
        allow_activity_subscriptions: allow_activity_subscriptions(notification_declaration),
        hide_from_algorithmic_recommendations: hides_from_algorithmic_recommendations(
            content_visibility,
        ),
    }
}

/// `actor_id` のリポジトリにある `collection/self` レコード（JSON）。無ければ `None`。
async fn self_record(
    pool: &PgPool,
    actor_id: i64,
    collection: &str,
) -> Result<Option<serde_json::Value>, String> {
    let Some(cid) = atp::record_cid(pool, actor_id, collection, "self")
        .await
        .map_err(|e| e.to_string())?
    else {
        return Ok(None);
    };
    let Some(bytes) = atp::block_bytes(pool, actor_id, &cid)
        .await
        .map_err(|e| e.to_string())?
    else {
        return Ok(None);
    };
    decode_dagcbor_to_json(&bytes)
        .map(Some)
        .map_err(|e| e.to_string())
}

/// リポジトリにある今の self レコードを読み、`actors` のプライバシー設定へ反映する。
pub async fn sync_from_repo(pool: &PgPool, actor_id: i64) -> Result<(), String> {
    let profile = self_record(pool, actor_id, PROFILE_COLLECTION).await?;
    let notification = self_record(pool, actor_id, NOTIFICATION_DECLARATION_COLLECTION).await?;
    let visibility = self_record(pool, actor_id, CONTENT_VISIBILITY_COLLECTION).await?;
    let settings =
        settings_from_records(profile.as_ref(), notification.as_ref(), visibility.as_ref());
    PgActorRepository::new(pool.clone())
        .update_atp_self_settings(actor_id, &settings)
        .await
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn no_unauthenticated_label_is_read_from_profile_labels() {
        let labeled = json!({
            "displayName": "a",
            "labels": {
                "$type": "com.atproto.label.defs#selfLabels",
                "values": [{"val": "!no-unauthenticated"}],
            },
        });
        assert!(hides_from_unauthenticated(Some(&labeled)));
        assert!(!hides_from_unauthenticated(Some(
            &json!({"displayName": "a"})
        )));
        assert!(!hides_from_unauthenticated(None));
    }

    #[test]
    fn allow_subscriptions_defaults_to_followers() {
        assert_eq!(
            allow_activity_subscriptions(Some(&json!({"allowSubscriptions": "mutuals"}))),
            "mutuals"
        );
        assert_eq!(
            allow_activity_subscriptions(Some(&json!({"allowSubscriptions": "none"}))),
            "none"
        );
        assert_eq!(
            allow_activity_subscriptions(Some(&json!({"allowSubscriptions": "everyone"}))),
            "followers"
        );
        assert_eq!(allow_activity_subscriptions(None), "followers");
    }

    #[test]
    fn missing_records_mean_defaults() {
        assert_eq!(
            settings_from_records(None, None, None),
            AtpSelfSettings {
                hide_from_unauthenticated: false,
                allow_activity_subscriptions: "followers",
                hide_from_algorithmic_recommendations: false,
            }
        );
        assert!(hides_from_algorithmic_recommendations(Some(
            &json!({"hideFromAlgorithmicRecommendations": true})
        )));
    }

    #[test]
    fn only_self_records_of_known_collections_matter() {
        assert!(is_self_settings_record(PROFILE_COLLECTION, "self"));
        assert!(is_self_settings_record(
            NOTIFICATION_DECLARATION_COLLECTION,
            "self"
        ));
        assert!(!is_self_settings_record(PROFILE_COLLECTION, "other"));
        assert!(!is_self_settings_record("app.bsky.feed.like", "self"));
    }
}

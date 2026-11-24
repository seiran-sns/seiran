//! 受信 Follow への Accept / Reject の送信。
//!
//! 応答はジョブ（`ApDeliveryKind::FollowResponse`）として積み、Follow の処理（関係の確定・
//! 通知）とは切り離して送る。送信の失敗で Follow の処理をやり直すと、アクター解決や通知の
//! 作成まで繰り返すことになるため。相手が一時的に受け取れない間も数日はキューに残して
//! 送り直す（届かないと、こちらは「フォローされている」のに相手側は「申請中」のまま残る）。

use super::infra::*;
use super::ApSender;
use super::*;

use crate::queue::worker::priority;
use crate::traits::{ApDeliveryKind, Job, JobQueue};

/// 受信 Follow への応答の種類。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FollowResponseKind {
    Accept,
    Reject,
}

impl FollowResponseKind {
    fn activity_type(self) -> &'static str {
        match self {
            FollowResponseKind::Accept => "Accept",
            FollowResponseKind::Reject => "Reject",
        }
    }

    fn id_path(self) -> &'static str {
        match self {
            FollowResponseKind::Accept => "accepts",
            FollowResponseKind::Reject => "rejects",
        }
    }
}

/// Accept / Reject の送信先と中身。
pub struct FollowResponse<'a> {
    pub kind: FollowResponseKind,
    pub local_domain: &'a str,
    /// 応答するローカルアクター（Follow の object）。
    pub local_actor_id: i64,
    pub local_username: &'a str,
    /// 応答の object に入れる受信 Follow アクティビティ。
    pub follow_activity: &'a serde_json::Value,
    pub remote_inbox: &'a str,
}

/// Accept / Reject アクティビティを組み立てる。id は積む時点で決め、送り直しの間も同じ
/// アクティビティとして届くようにする。
pub fn build_follow_response_activity(r: &FollowResponse<'_>) -> serde_json::Value {
    let addr = local_actor_address(r.local_domain, r.local_username);
    let response_id = format!(
        "https://{}/{}/{}",
        r.local_domain,
        r.kind.id_path(),
        crate::generate_snowflake_id(chrono::Utc::now())
    );
    serde_json::json!({
        "@context": "https://www.w3.org/ns/activitystreams",
        "type": r.kind.activity_type(),
        "id": response_id,
        "actor": addr.actor_uri,
        "object": r.follow_activity,
    })
}

/// Accept / Reject の送信をジョブキューに積む。
pub async fn enqueue_follow_response(
    queue: &dyn JobQueue,
    r: &FollowResponse<'_>,
) -> Result<(), String> {
    let activity = build_follow_response_activity(r);
    queue
        .enqueue(
            Job::ApDelivery {
                actor_id: r.local_actor_id,
                kind: ApDeliveryKind::FollowResponse {
                    inbox: r.remote_inbox.to_string(),
                    activity,
                },
            },
            priority::HIGH,
        )
        .await
        .map_err(|e| {
            format!(
                "{} 送信ジョブの enqueue に失敗: {}",
                r.kind.activity_type(),
                e
            )
        })
}

/// 積まれた Accept / Reject を相手の inbox へ送る。
pub async fn deliver_follow_response(
    sender: &ApSender<'_>,
    inbox: &str,
    activity: &serde_json::Value,
) -> Result<(), ApError> {
    let username = fetch_username(sender.db, sender.actor_id).await?;
    let addr = local_actor_address(sender.local_domain, &username);
    let body = serde_json::to_string(activity)?;
    sender
        .ap_client
        .sign_and_post(inbox, &body, &addr.key_id, sender.private_key_pem)
        .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_accept_with_follow_as_object() {
        let follow = serde_json::json!({
            "type": "Follow",
            "id": "https://remote.example/follows/1",
            "actor": "https://remote.example/users/a",
            "object": "https://local.example/users/alice",
        });
        let r = FollowResponse {
            kind: FollowResponseKind::Accept,
            local_domain: "local.example",
            local_actor_id: 1,
            local_username: "alice",
            follow_activity: &follow,
            remote_inbox: "https://remote.example/inbox",
        };
        let activity = build_follow_response_activity(&r);
        assert_eq!(activity["type"], "Accept");
        assert_eq!(activity["actor"], "https://local.example/users/alice");
        assert_eq!(activity["object"], follow);
        assert!(activity["id"]
            .as_str()
            .unwrap()
            .starts_with("https://local.example/accepts/"));
    }

    #[test]
    fn builds_reject_id_under_rejects() {
        let follow = serde_json::json!({ "type": "Follow" });
        let r = FollowResponse {
            kind: FollowResponseKind::Reject,
            local_domain: "local.example",
            local_actor_id: 1,
            local_username: "alice",
            follow_activity: &follow,
            remote_inbox: "https://remote.example/inbox",
        };
        let activity = build_follow_response_activity(&r);
        assert_eq!(activity["type"], "Reject");
        assert!(activity["id"]
            .as_str()
            .unwrap()
            .starts_with("https://local.example/rejects/"));
    }
}

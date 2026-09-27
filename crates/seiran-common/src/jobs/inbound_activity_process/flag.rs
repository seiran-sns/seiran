use super::*;

/// リモートFediサーバーからローカルActor/投稿宛てに届いたActivityPub Flagを
/// 統一通報台帳へ取り込む。
pub(super) async fn handle_flag(
    activity: serde_json::Value,
    inbox: &InboxContext,
    ap_client: &ApClient,
) -> Result<(), String> {
    let actor_uri = activity["actor"]
        .as_str()
        .ok_or("Flag: actor がありません")?;
    let reporter = upsert_remote_fedi_actor(inbox, ap_client, actor_uri).await?;
    let objects: Vec<&str> = match &activity["object"] {
        serde_json::Value::String(v) => vec![v.as_str()],
        serde_json::Value::Array(v) => v.iter().filter_map(|x| x.as_str()).collect(),
        _ => Vec::new(),
    };
    let mut subject_actor_id = None;
    let mut subject_post_id = None;
    for object in objects {
        if let Some(id) = object
            .strip_prefix(&format!("https://{}/notes/", inbox.local_domain))
            .and_then(|v| v.parse::<i64>().ok())
        {
            let owner = crate::repository::post::author_of(&inbox.db_pool, id)
                .await
                .map_err(|e| format!("Flag: 投稿検索失敗: {}", e))?;
            if let Some(owner) = owner {
                subject_actor_id = Some(owner);
                subject_post_id = Some(id);
                break;
            }
        }
        if let Some(username) = crate::ap::extract_local_username(object, &inbox.local_domain) {
            if let Some(actor) = inbox
                .actor_repo
                .find_including_withdrawn_by_username_domain(username, &inbox.local_domain)
                .await
                .map_err(|e| format!("Flag: Actor検索失敗: {}", e))?
                .filter(|a| a.actor_type == "local")
            {
                subject_actor_id = Some(actor.id);
            }
        }
    }
    let Some(subject_actor_id) = subject_actor_id else {
        return Err("Flag: ローカルの通報対象を解決できません".into());
    };
    let raw = strip_html(activity["content"].as_str().unwrap_or(""));
    let mut reason_text = String::new();
    for ch in raw.chars().take(300) {
        if reason_text.len() + ch.len_utf8() > 1000 {
            break;
        }
        reason_text.push(ch);
    }
    let report_id = generate_snowflake_id(chrono::Utc::now());
    let report = crate::repository::report::NewReport {
        id: report_id,
        reporter_actor_id: reporter.actor_id,
        subject_type: if subject_post_id.is_some() {
            "post"
        } else {
            "actor"
        },
        subject_actor_id,
        subject_post_id,
        reason_type: "other",
        reason_text: &reason_text,
        destination: "local",
        remote_host: Some(&reporter.domain),
    };
    crate::repository::report::insert(&inbox.db_pool, &report)
        .await
        .map_err(|e| format!("Flag: 保存失敗: {}", e))?;
    Ok(())
}

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
            let owner: Option<i64> = sqlx::query_scalar("SELECT actor_id FROM posts WHERE id=$1")
                .bind(id)
                .fetch_optional(&inbox.db_pool)
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
                .find_by_username_domain(username, &inbox.local_domain)
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
    sqlx::query(
        "INSERT INTO reports(id,reporter_actor_id,subject_type,subject_actor_id,subject_post_id,\
         reason_type,reason_text,destination,remote_host) \
         VALUES($1,$2,$3::report_subject_type,$4,$5,'other',$6,'local',$7)",
    )
    .bind(report_id)
    .bind(reporter.actor_id)
    .bind(if subject_post_id.is_some() {
        "post"
    } else {
        "actor"
    })
    .bind(subject_actor_id)
    .bind(subject_post_id)
    .bind(reason_text)
    .bind(reporter.domain)
    .execute(&inbox.db_pool)
    .await
    .map_err(|e| format!("Flag: 保存失敗: {}", e))?;
    Ok(())
}

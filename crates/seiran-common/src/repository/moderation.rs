//! 調停者（admin・moderator）がユーザー操作メニューから行う、アクター単位の凍結状態・ロールの
//! 確認と変更。

use sqlx::PgPool;

/// アクターの凍結状態と、ローカルユーザーならロール（リモートは `None`）。アクターが無ければ `None`。
pub async fn actor_state(
    pool: &PgPool,
    actor_id: i64,
) -> Result<Option<(bool, Option<String>)>, sqlx::Error> {
    sqlx::query_as(
        "SELECT a.suspended_at IS NOT NULL, u.role::text FROM actors a \
         LEFT JOIN users u ON u.id = a.user_id WHERE a.id = $1",
    )
    .bind(actor_id)
    .fetch_optional(pool)
    .await
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeRoleOutcome {
    Changed,
    /// 対象のロールが `changeable` に入っていない、または操作者自身。
    Forbidden,
    /// アクターが無いか、リモートのアクター（ロールを持たない）。
    NotLocal,
}

/// ローカルユーザーのアクターのロールを `role` にする。今のロールが `changeable` に入っている
/// ときだけ変え、操作者自身（`requester_user_id`）は変えない（自分を降格して管理画面に
/// 戻れなくなる事故を防ぐ）。確認と更新を1文で行う。
pub async fn change_actor_role(
    pool: &PgPool,
    actor_id: i64,
    role: &str,
    changeable: &[&str],
    requester_user_id: i64,
) -> Result<ChangeRoleOutcome, sqlx::Error> {
    let (found, changed): (bool, bool) = sqlx::query_as(
        "WITH target AS (\
           SELECT u.id, u.role::text AS role FROM actors a JOIN users u ON u.id = a.user_id \
           WHERE a.id = $1 FOR UPDATE OF u), \
         updated AS (\
           UPDATE users u SET role = $2::user_role FROM target t \
           WHERE u.id = t.id AND t.role = ANY($3) AND u.id <> $4 RETURNING u.id) \
         SELECT EXISTS (SELECT 1 FROM target), EXISTS (SELECT 1 FROM updated)",
    )
    .bind(actor_id)
    .bind(role)
    .bind(changeable)
    .bind(requester_user_id)
    .fetch_one(pool)
    .await?;
    Ok(match (found, changed) {
        (_, true) => ChangeRoleOutcome::Changed,
        (true, false) => ChangeRoleOutcome::Forbidden,
        (false, false) => ChangeRoleOutcome::NotLocal,
    })
}

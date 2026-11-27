//! 調停者（admin・moderator）がユーザー操作メニューから行う、アクター単位の凍結状態・ロールの
//! 確認と変更。

use sqlx::PgPool;

use crate::edit_diff::Edit;

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
    /// 今のロールが、操作者が選ぶ前に見ていたロールとも選んだロールとも違う（別の画面で
    /// 変えられた、`crate::edit_diff`）。
    EditConflict,
}

/// ローカルユーザーのアクターのロールを `role` の差分で変える（`crate::edit_diff`）。今のロールが
/// `changeable` に入っているときだけ変え、操作者自身（`requester_user_id`）は変えない（自分を
/// 降格して管理画面に戻れなくなる事故を防ぐ）。今のロールが選ぶ前のロールとも選んだロールとも
/// 違えば変えない。確認と更新を1文で行う。
pub async fn change_actor_role(
    pool: &PgPool,
    actor_id: i64,
    role: &Edit<String>,
    changeable: &[&str],
    requester_user_id: i64,
) -> Result<ChangeRoleOutcome, sqlx::Error> {
    let (current, changed): (Option<String>, bool) = sqlx::query_as(
        "WITH target AS (\
           SELECT u.id, u.role::text AS role FROM actors a JOIN users u ON u.id = a.user_id \
           WHERE a.id = $1 FOR UPDATE OF u), \
         updated AS (\
           UPDATE users u SET role = $2::user_role FROM target t \
           WHERE u.id = t.id AND t.role = ANY($3) AND u.id <> $4 \
             AND (NOT $5 OR t.role = $6 OR t.role = $2) RETURNING u.id) \
         SELECT (SELECT role FROM target), EXISTS (SELECT 1 FROM updated)",
    )
    .bind(actor_id)
    .bind(role.target())
    .bind(changeable)
    .bind(requester_user_id)
    .bind(role.expected().is_some())
    .bind(role.expected())
    .fetch_one(pool)
    .await?;
    Ok(match (current, changed) {
        (_, true) => ChangeRoleOutcome::Changed,
        (None, false) => ChangeRoleOutcome::NotLocal,
        (Some(current), false)
            if role
                .expected()
                .is_some_and(|from| current != *from && current != *role.target()) =>
        {
            ChangeRoleOutcome::EditConflict
        }
        (Some(_), false) => ChangeRoleOutcome::Forbidden,
    })
}

use std::collections::HashMap;

use async_trait::async_trait;
use sqlx::PgPool;

/// フォロー中/フォロワー一覧の1行（アクター表示情報 + カーソル用 `follows.id`、#56）。
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct FollowListRow {
    /// カーソルページネーション用（`follows.id`、`until_id`/`since_id` に使う）。
    pub follow_id: i64,
    pub actor_id: i64,
    pub username: String,
    pub domain: String,
    pub display_name: Option<String>,
    pub avatar_url: Option<String>,
    /// Misskey互換API（`POST /api/users/following`・`followers`）の`createdAt`用。
    pub created_at: chrono::DateTime<chrono::Utc>,
}

#[async_trait]
pub trait FollowRepository: Send + Sync {
    /// フォローを pending で挿入する（既存なら status を pending に戻す）。
    /// 新規に挿入した場合は true、既存の関係を更新した場合は false を返す。
    async fn upsert_pending(
        &self,
        follower_actor_id: i64,
        target_actor_id: i64,
    ) -> Result<bool, sqlx::Error>;

    /// フォロー関係の status を取得する（未フォローなら None）。
    async fn find_status(
        &self,
        follower_actor_id: i64,
        target_actor_id: i64,
    ) -> Result<Option<String>, sqlx::Error>;

    /// リモートからのフォロー受信時に accepted で挿入する（重複なら何もしない）。
    async fn insert_accepted(
        &self,
        follower_actor_id: i64,
        target_actor_id: i64,
    ) -> Result<(), sqlx::Error>;

    /// pending のフォローを accepted に昇格させる（Accept 受信時）。
    async fn accept(
        &self,
        follower_actor_id: i64,
        target_actor_id: i64,
    ) -> Result<u64, sqlx::Error>;

    /// `insert_accepted`のリモートseiranアクター版（#238）。フォロー送信と同時に
    /// 楽観的確定する非鍵アカウント宛てで、ターゲットが既に結婚成立済み
    /// （真正なat_didを持つ）の場合、`atp_rkey`にATP側`commit_follow`の結果を
    /// 同時に記録する（`atp_rkey`が`None`なら`insert_accepted`と同じ）。
    async fn insert_accepted_with_rkey(
        &self,
        follower_actor_id: i64,
        target_actor_id: i64,
        atp_rkey: Option<&str>,
    ) -> Result<bool, sqlx::Error>;

    /// フォロー関係を削除する（Undo/Follow 受信時）。
    async fn delete_by_actors(
        &self,
        follower_actor_id: i64,
        target_actor_id: i64,
    ) -> Result<(), sqlx::Error>;

    /// フォロー関係の atp_rkey を取得する（アンフォロー時の ATP 削除に使用）。
    async fn find_atp_rkey(
        &self,
        follower_actor_id: i64,
        target_actor_id: i64,
    ) -> Result<Option<String>, sqlx::Error>;

    /// `candidate_ids` の各アクターへの follow status を一括取得する（未フォローのIDは
    /// 結果に含まれない）。タイムラインのper-note relationship付与でのN+1回避用。
    async fn find_statuses_among(
        &self,
        follower_actor_id: i64,
        candidate_ids: &[i64],
    ) -> Result<HashMap<i64, String>, sqlx::Error>;

    /// `target_actor_id` へフォロー中/フォロー申請中の `candidate_follower_ids` の
    /// follow status を一括取得する（未フォローのIDは結果に含まれない）。`isFollowed`
    /// （相手が自分をフォローしているか）をMisskey互換APIでN+1なしに算出するために使う。
    /// `find_statuses_among` と逆方向。
    async fn find_statuses_by_followers_among(
        &self,
        target_actor_id: i64,
        candidate_follower_ids: &[i64],
    ) -> Result<HashMap<i64, String>, sqlx::Error>;

    /// ATP フォロー完了後に accepted で挿入する（rkey を保存）。
    /// 新規に挿入した場合は true、既にフォロー済みだった場合は false を返す。
    async fn insert_accepted_bsky(
        &self,
        follower_actor_id: i64,
        target_actor_id: i64,
        atp_rkey: &str,
    ) -> Result<bool, sqlx::Error>;

    /// `target_actor_id` を accepted な status でフォローしているローカルアクターの ID 一覧を取得する。
    /// 新規投稿の realtime WebSocket 配信対象を決めるために使う。
    async fn find_accepted_local_follower_ids(
        &self,
        target_actor_id: i64,
    ) -> Result<Vec<i64>, sqlx::Error>;

    /// `find_accepted_local_follower_ids` に、ホームタイムラインのリプライ先フォロー条件
    /// （`post_reply_target_followed`、`repository::post`の`home_timeline`/`social_timeline`と
    /// 同じ判定をDB関数として共有）を追加したもの。新規投稿のホームタイムライン
    /// WebSocket配信対象（`home_recipients`）を決めるために使う。`reply_to_post_id`が`None`なら
    /// `find_accepted_local_follower_ids`と同じ結果になる。
    async fn find_home_recipient_ids(
        &self,
        target_actor_id: i64,
        reply_to_post_id: Option<i64>,
    ) -> Result<Vec<i64>, sqlx::Error>;

    /// `follower_actor_id` が accepted な status でフォローしている全ての
    /// `target_actor_id` を取得する（退会時、フォロー先全員への一括アンフォロー用）。
    async fn find_accepted_target_ids(
        &self,
        follower_actor_id: i64,
    ) -> Result<Vec<i64>, sqlx::Error>;

    /// `target_actor_id` をフォロー中/フォロー申請中（status問わず）のローカルアクター
    /// （実ユーザー・list-relayプロキシアクター含む）を `(follower_actor_id, status)` で
    /// 取得する。ActivityPub Move（引っ越し）受信時、フォロー関係を移転先へ付け替える
    /// 対象を洗い出すために使う。
    async fn find_all_local_followers_with_status(
        &self,
        target_actor_id: i64,
    ) -> Result<Vec<(i64, String)>, sqlx::Error>;

    /// `actor_id` の (following_count, follower_count) を返す（プロフィール画面表示用、#56）。
    /// status='accepted' のみをカウントする（pending は含まない）。
    async fn count_relations(&self, actor_id: i64) -> Result<(i64, i64), sqlx::Error>;

    /// `follower_actor_id` がフォロー中（status='accepted'）のアクター一覧を新しい順に返す
    /// （プロフィール画面の「フォロー中」タブ、#56）。`viewer_actor_id` が指定されていれば、
    /// 閲覧者からブロックされている等で非表示にすべきアクターを除外する。
    async fn list_following(
        &self,
        follower_actor_id: i64,
        viewer_actor_id: Option<i64>,
        limit: i64,
        until_id: Option<i64>,
        since_id: Option<i64>,
    ) -> Result<Vec<FollowListRow>, sqlx::Error>;

    /// `target_actor_id` をフォロー中（status='accepted'）のアクター一覧を新しい順に返す
    /// （プロフィール画面の「フォロワー」タブ、#56）。`viewer_actor_id` の扱いは `list_following` と同じ。
    async fn list_followers(
        &self,
        target_actor_id: i64,
        viewer_actor_id: Option<i64>,
        limit: i64,
        until_id: Option<i64>,
        since_id: Option<i64>,
    ) -> Result<Vec<FollowListRow>, sqlx::Error>;

    /// フォローを pending で挿入し、Fediverse から届いた生の Follow アクティビティ（JSON）を
    /// `pending_follow_activity` へ保存する（既存なら status/activity を上書き）。ロック中の
    /// ローカルアクター宛てに AP Follow を受信した際に使う。承認/拒否時、保存した活動を
    /// そのまま Accept/Reject の `object` として送り返せるようにするため。
    async fn upsert_pending_with_activity(
        &self,
        follower_actor_id: i64,
        target_actor_id: i64,
        activity: &serde_json::Value,
    ) -> Result<bool, sqlx::Error>;

    /// `upsert_pending_with_activity` で保存した Follow アクティビティを取得する。
    async fn find_pending_follow_activity(
        &self,
        follower_actor_id: i64,
        target_actor_id: i64,
    ) -> Result<Option<serde_json::Value>, sqlx::Error>;

    /// pending のフォローを accepted に昇格させる（承認制フォローの承認時）。`atp_rkey` が
    /// `Some` ならローカルフォロワーのATPコミット後のrkeyとして併せて保存する（fediフォロワーは
    /// `None`）。`accept`（Accept受信）と異なりAPIハンドラ/ジョブから明示的に呼ばれる。
    async fn accept_and_set_rkey(
        &self,
        follower_actor_id: i64,
        target_actor_id: i64,
        atp_rkey: Option<&str>,
    ) -> Result<u64, sqlx::Error>;

    /// `target_actor_id` 宛ての承認待ち（status='pending'）フォローリクエスト件数
    /// （設定アイコン・「承認待ちフォロー」メニュー項目のバッジ表示用）。
    async fn count_pending(&self, target_actor_id: i64) -> Result<i64, sqlx::Error>;

    /// `target_actor_id` 宛ての承認待ち（status='pending'）フォローリクエスト一覧を新しい順に返す
    /// （設定画面「承認待ちフォロー」、簡易一覧のためカーソルページネーションは持たない）。
    async fn list_pending_followers(
        &self,
        target_actor_id: i64,
        limit: i64,
    ) -> Result<Vec<FollowListRow>, sqlx::Error>;

    /// `target_actor_id` 宛ての承認待ちフォロー全件を `(follower_actor_id,
    /// pending_follow_activity)` で取得する（承認制OFF切替時の一括承認ジョブ用）。
    async fn find_pending_followers_raw(
        &self,
        target_actor_id: i64,
    ) -> Result<Vec<(i64, Option<serde_json::Value>)>, sqlx::Error>;
}

pub struct PgFollowRepository {
    pool: PgPool,
}

impl PgFollowRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl FollowRepository for PgFollowRepository {
    async fn upsert_pending(
        &self,
        follower_actor_id: i64,
        target_actor_id: i64,
    ) -> Result<bool, sqlx::Error> {
        // `xmax = 0` は「このコマンドで新規挿入された行か」の判定に使うPostgresの定石
        // （UPDATEされた既存行はxmaxに現在のトランザクションIDが入る）。
        let row: (bool,) = sqlx::query_as(
            "INSERT INTO follows (follower_actor_id, target_actor_id, status)
             VALUES ($1, $2, 'pending')
             ON CONFLICT (follower_actor_id, target_actor_id) DO UPDATE
               SET status = 'pending'
             RETURNING (xmax = 0)",
        )
        .bind(follower_actor_id)
        .bind(target_actor_id)
        .fetch_one(&self.pool)
        .await?;
        Ok(row.0)
    }

    async fn find_status(
        &self,
        follower_actor_id: i64,
        target_actor_id: i64,
    ) -> Result<Option<String>, sqlx::Error> {
        let row: Option<(String,)> = sqlx::query_as(
            "SELECT status FROM follows
             WHERE follower_actor_id = $1 AND target_actor_id = $2 LIMIT 1",
        )
        .bind(follower_actor_id)
        .bind(target_actor_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|r| r.0))
    }

    async fn find_statuses_among(
        &self,
        follower_actor_id: i64,
        candidate_ids: &[i64],
    ) -> Result<HashMap<i64, String>, sqlx::Error> {
        let rows: Vec<(i64, String)> = sqlx::query_as(
            "SELECT target_actor_id, status FROM follows
             WHERE follower_actor_id = $1 AND target_actor_id = ANY($2)",
        )
        .bind(follower_actor_id)
        .bind(candidate_ids)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().collect())
    }

    async fn find_statuses_by_followers_among(
        &self,
        target_actor_id: i64,
        candidate_follower_ids: &[i64],
    ) -> Result<HashMap<i64, String>, sqlx::Error> {
        let rows: Vec<(i64, String)> = sqlx::query_as(
            "SELECT follower_actor_id, status FROM follows
             WHERE target_actor_id = $1 AND follower_actor_id = ANY($2)",
        )
        .bind(target_actor_id)
        .bind(candidate_follower_ids)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().collect())
    }

    async fn insert_accepted(
        &self,
        follower_actor_id: i64,
        target_actor_id: i64,
    ) -> Result<(), sqlx::Error> {
        // followers_count/following_count は trg_follows_sync_counts トリガーが
        // follows への書き込みのたびに実測値へ再計算する（docs/database.md「非正規化カウンタ」参照）。
        sqlx::query(
            "INSERT INTO follows (follower_actor_id, target_actor_id, status)
             VALUES ($1, $2, 'accepted')
             ON CONFLICT (follower_actor_id, target_actor_id) DO NOTHING",
        )
        .bind(follower_actor_id)
        .bind(target_actor_id)
        .execute(&self.pool)
        .await
        .map(|_| ())
    }

    async fn insert_accepted_with_rkey(
        &self,
        follower_actor_id: i64,
        target_actor_id: i64,
        atp_rkey: Option<&str>,
    ) -> Result<bool, sqlx::Error> {
        sqlx::query(
            "INSERT INTO follows (follower_actor_id, target_actor_id, status, atp_rkey)
             VALUES ($1, $2, 'accepted', $3)
             ON CONFLICT (follower_actor_id, target_actor_id) DO NOTHING",
        )
        .bind(follower_actor_id)
        .bind(target_actor_id)
        .bind(atp_rkey)
        .execute(&self.pool)
        .await
        .map(|result| result.rows_affected() > 0)
    }

    async fn accept(
        &self,
        follower_actor_id: i64,
        target_actor_id: i64,
    ) -> Result<u64, sqlx::Error> {
        let result = sqlx::query(
            "UPDATE follows SET status = 'accepted'
             WHERE follower_actor_id = $1 AND target_actor_id = $2 AND status = 'pending'",
        )
        .bind(follower_actor_id)
        .bind(target_actor_id)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected())
    }

    async fn delete_by_actors(
        &self,
        follower_actor_id: i64,
        target_actor_id: i64,
    ) -> Result<(), sqlx::Error> {
        sqlx::query("DELETE FROM follows WHERE follower_actor_id = $1 AND target_actor_id = $2")
            .bind(follower_actor_id)
            .bind(target_actor_id)
            .execute(&self.pool)
            .await
            .map(|_| ())
    }

    async fn find_atp_rkey(
        &self,
        follower_actor_id: i64,
        target_actor_id: i64,
    ) -> Result<Option<String>, sqlx::Error> {
        let row: Option<(Option<String>,)> = sqlx::query_as(
            "SELECT atp_rkey FROM follows
             WHERE follower_actor_id = $1 AND target_actor_id = $2 LIMIT 1",
        )
        .bind(follower_actor_id)
        .bind(target_actor_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.and_then(|r| r.0))
    }

    async fn insert_accepted_bsky(
        &self,
        follower_actor_id: i64,
        target_actor_id: i64,
        atp_rkey: &str,
    ) -> Result<bool, sqlx::Error> {
        sqlx::query(
            "INSERT INTO follows (follower_actor_id, target_actor_id, status, atp_rkey)
             VALUES ($1, $2, 'accepted', $3)
             ON CONFLICT (follower_actor_id, target_actor_id) DO NOTHING",
        )
        .bind(follower_actor_id)
        .bind(target_actor_id)
        .bind(atp_rkey)
        .execute(&self.pool)
        .await
        .map(|result| result.rows_affected() > 0)
    }

    async fn find_accepted_local_follower_ids(
        &self,
        target_actor_id: i64,
    ) -> Result<Vec<i64>, sqlx::Error> {
        sqlx::query_scalar::<_, i64>(
            "SELECT f.follower_actor_id FROM follows f
             JOIN actors a ON a.id = f.follower_actor_id
             WHERE f.target_actor_id = $1 AND f.status = 'accepted' AND a.actor_type = 'local'",
        )
        .bind(target_actor_id)
        .fetch_all(&self.pool)
        .await
    }

    async fn find_home_recipient_ids(
        &self,
        target_actor_id: i64,
        reply_to_post_id: Option<i64>,
    ) -> Result<Vec<i64>, sqlx::Error> {
        sqlx::query_scalar::<_, i64>(
            "SELECT f.follower_actor_id FROM follows f
             JOIN actors a ON a.id = f.follower_actor_id
             WHERE f.target_actor_id = $1 AND f.status = 'accepted' AND a.actor_type = 'local'
               AND post_reply_target_followed(f.follower_actor_id, $2)",
        )
        .bind(target_actor_id)
        .bind(reply_to_post_id)
        .fetch_all(&self.pool)
        .await
    }

    async fn find_accepted_target_ids(
        &self,
        follower_actor_id: i64,
    ) -> Result<Vec<i64>, sqlx::Error> {
        sqlx::query_scalar::<_, i64>(
            "SELECT target_actor_id FROM follows
             WHERE follower_actor_id = $1 AND status = 'accepted'",
        )
        .bind(follower_actor_id)
        .fetch_all(&self.pool)
        .await
    }

    async fn find_all_local_followers_with_status(
        &self,
        target_actor_id: i64,
    ) -> Result<Vec<(i64, String)>, sqlx::Error> {
        sqlx::query_as::<_, (i64, String)>(
            "SELECT f.follower_actor_id, f.status FROM follows f
             JOIN actors a ON a.id = f.follower_actor_id
             WHERE f.target_actor_id = $1 AND a.actor_type = 'local'",
        )
        .bind(target_actor_id)
        .fetch_all(&self.pool)
        .await
    }

    async fn count_relations(&self, actor_id: i64) -> Result<(i64, i64), sqlx::Error> {
        // 非正規化カラム（actors.following_count/followers_count）を読む。
        // 書き込みはtrg_follows_sync_countsトリガー（followsへのINSERT/UPDATE/DELETE時に
        // 実測COUNT(*)で再計算）が一元的に行う。アプリ側はfollowsテーブルへの素朴な
        // INSERT/UPDATE/DELETEを発行するだけでよい（docs/database.md「非正規化カウンタ」参照）。
        let row: (i64, i64) =
            sqlx::query_as("SELECT following_count, followers_count FROM actors WHERE id = $1")
                .bind(actor_id)
                .fetch_one(&self.pool)
                .await?;
        Ok(row)
    }

    async fn list_following(
        &self,
        follower_actor_id: i64,
        viewer_actor_id: Option<i64>,
        limit: i64,
        until_id: Option<i64>,
        since_id: Option<i64>,
    ) -> Result<Vec<FollowListRow>, sqlx::Error> {
        sqlx::query_as::<_, FollowListRow>(
            "SELECT f.id AS follow_id, a.id AS actor_id, a.username, a.domain, a.display_name,
                    COALESCE(rtrim(sp.public_url, '/') || '/' || mf.storage_key, a.avatar_url) AS avatar_url,
                    f.created_at
             FROM follows f
             JOIN actors a ON a.id = f.target_actor_id
             LEFT JOIN media_files mf ON mf.id = a.avatar_media_id
             LEFT JOIN storage_providers sp ON sp.id = mf.storage_provider_id
             WHERE f.follower_actor_id = $1 AND f.status = 'accepted'
               AND ($2::bigint IS NULL OR NOT actor_is_hidden_for_viewer($2, a.id))
               AND ($3::bigint IS NULL OR f.id < $3)
               AND ($4::bigint IS NULL OR f.id > $4)
             ORDER BY f.id DESC
             LIMIT $5",
        )
        .bind(follower_actor_id)
        .bind(viewer_actor_id)
        .bind(until_id)
        .bind(since_id)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
    }

    async fn list_followers(
        &self,
        target_actor_id: i64,
        viewer_actor_id: Option<i64>,
        limit: i64,
        until_id: Option<i64>,
        since_id: Option<i64>,
    ) -> Result<Vec<FollowListRow>, sqlx::Error> {
        sqlx::query_as::<_, FollowListRow>(
            "SELECT f.id AS follow_id, a.id AS actor_id, a.username, a.domain, a.display_name,
                    COALESCE(rtrim(sp.public_url, '/') || '/' || mf.storage_key, a.avatar_url) AS avatar_url,
                    f.created_at
             FROM follows f
             JOIN actors a ON a.id = f.follower_actor_id
             LEFT JOIN media_files mf ON mf.id = a.avatar_media_id
             LEFT JOIN storage_providers sp ON sp.id = mf.storage_provider_id
             WHERE f.target_actor_id = $1 AND f.status = 'accepted'
               AND ($2::bigint IS NULL OR NOT actor_is_hidden_for_viewer($2, a.id))
               AND ($3::bigint IS NULL OR f.id < $3)
               AND ($4::bigint IS NULL OR f.id > $4)
             ORDER BY f.id DESC
             LIMIT $5",
        )
        .bind(target_actor_id)
        .bind(viewer_actor_id)
        .bind(until_id)
        .bind(since_id)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
    }

    async fn upsert_pending_with_activity(
        &self,
        follower_actor_id: i64,
        target_actor_id: i64,
        activity: &serde_json::Value,
    ) -> Result<bool, sqlx::Error> {
        let row: (bool,) = sqlx::query_as(
            "INSERT INTO follows (follower_actor_id, target_actor_id, status, pending_follow_activity)
             VALUES ($1, $2, 'pending', $3)
             ON CONFLICT (follower_actor_id, target_actor_id) DO UPDATE
               SET status = 'pending', pending_follow_activity = $3
             RETURNING (xmax = 0)",
        )
        .bind(follower_actor_id)
        .bind(target_actor_id)
        .bind(activity)
        .fetch_one(&self.pool)
        .await?;
        Ok(row.0)
    }

    async fn find_pending_follow_activity(
        &self,
        follower_actor_id: i64,
        target_actor_id: i64,
    ) -> Result<Option<serde_json::Value>, sqlx::Error> {
        let row: Option<(Option<serde_json::Value>,)> = sqlx::query_as(
            "SELECT pending_follow_activity FROM follows
             WHERE follower_actor_id = $1 AND target_actor_id = $2 LIMIT 1",
        )
        .bind(follower_actor_id)
        .bind(target_actor_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.and_then(|r| r.0))
    }

    async fn accept_and_set_rkey(
        &self,
        follower_actor_id: i64,
        target_actor_id: i64,
        atp_rkey: Option<&str>,
    ) -> Result<u64, sqlx::Error> {
        let result = sqlx::query(
            "UPDATE follows SET status = 'accepted', atp_rkey = COALESCE($3, atp_rkey)
             WHERE follower_actor_id = $1 AND target_actor_id = $2 AND status = 'pending'",
        )
        .bind(follower_actor_id)
        .bind(target_actor_id)
        .bind(atp_rkey)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected())
    }

    async fn count_pending(&self, target_actor_id: i64) -> Result<i64, sqlx::Error> {
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM follows WHERE target_actor_id = $1 AND status = 'pending'",
        )
        .bind(target_actor_id)
        .fetch_one(&self.pool)
        .await
    }

    async fn list_pending_followers(
        &self,
        target_actor_id: i64,
        limit: i64,
    ) -> Result<Vec<FollowListRow>, sqlx::Error> {
        sqlx::query_as::<_, FollowListRow>(
            "SELECT f.id AS follow_id, a.id AS actor_id, a.username, a.domain, a.display_name,
                    COALESCE(rtrim(sp.public_url, '/') || '/' || mf.storage_key, a.avatar_url) AS avatar_url,
                    f.created_at
             FROM follows f
             JOIN actors a ON a.id = f.follower_actor_id
             LEFT JOIN media_files mf ON mf.id = a.avatar_media_id
             LEFT JOIN storage_providers sp ON sp.id = mf.storage_provider_id
             WHERE f.target_actor_id = $1 AND f.status = 'pending'
             ORDER BY f.id DESC
             LIMIT $2",
        )
        .bind(target_actor_id)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
    }

    async fn find_pending_followers_raw(
        &self,
        target_actor_id: i64,
    ) -> Result<Vec<(i64, Option<serde_json::Value>)>, sqlx::Error> {
        sqlx::query_as::<_, (i64, Option<serde_json::Value>)>(
            "SELECT follower_actor_id, pending_follow_activity FROM follows
             WHERE target_actor_id = $1 AND status = 'pending'",
        )
        .bind(target_actor_id)
        .fetch_all(&self.pool)
        .await
    }
}

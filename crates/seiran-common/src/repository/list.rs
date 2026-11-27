use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::PgPool;

use super::TimelinePost;
use crate::edit_diff::{Edit, EditConflict};

/// リストの名前・公開設定の編集（`crate::edit_diff`）。
/// `None` の項目は変えない。
pub struct ListEdit {
    pub name: Option<Edit<String>>,
    pub is_public: Option<Edit<bool>>,
}

/// 差分を当てた結果。`was_public` はロック中に読んだ変更前の公開設定（ATP レコードの
/// 公開・取り下げを決めるのに使う）。
pub struct ListUpdated {
    pub name: String,
    pub was_public: bool,
    pub is_public: bool,
}

/// リストの1行。
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ListRow {
    pub id: i64,
    pub owner_actor_id: i64,
    pub name: String,
    pub is_public: bool,
    pub at_rkey: Option<String>,
    pub at_uri: Option<String>,
    pub at_cid: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// 一覧表示用に JOIN で同時取得するメンバー数。
    #[sqlx(default)]
    pub member_count: i64,
}

/// リストメンバーの1行（アクター情報込み、タイムライン等と共通のアバター解決パターンを使う）。
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ListMemberRow {
    pub actor_id: i64,
    pub username: String,
    pub domain: String,
    pub display_name: Option<String>,
    /// 表示名中のカスタム絵文字（`:shortcode:` → 画像URL、`emoji_map::to_string_map` で変換）。
    pub emoji_map: Option<serde_json::Value>,
    pub actor_type: String,
    pub avatar_url: Option<String>,
    pub added_at: DateTime<Utc>,
    pub at_rkey: Option<String>,
    pub at_uri: Option<String>,
}

#[async_trait]
pub trait ListRepository: Send + Sync {
    /// 新規リストを作成する。
    async fn create(
        &self,
        id: i64,
        owner_actor_id: i64,
        name: &str,
        is_public: bool,
        now: DateTime<Utc>,
    ) -> Result<(), sqlx::Error>;

    /// リストの名前・公開設定を更新する（所有者本人のみ）。更新できた行数を返す。
    /// 行をロックして名前・公開設定の差分を当てる（所有者本人のみ、`crate::edit_diff`）。
    /// 無ければ `Ok(Ok(None))`、当たらなければ何も変えず `Ok(Err(EditConflict))`。
    async fn update(
        &self,
        id: i64,
        owner_actor_id: i64,
        edit: ListEdit,
    ) -> Result<Result<Option<ListUpdated>, EditConflict>, sqlx::Error>;

    /// リストを削除する（所有者本人のみ）。削除できた行数を返す。
    async fn delete(&self, id: i64, owner_actor_id: i64) -> Result<u64, sqlx::Error>;

    /// IDでリストを取得する（所有者チェックは呼び出し側が行う）。
    async fn find_by_id(&self, id: i64) -> Result<Option<ListRow>, sqlx::Error>;

    /// 指定オーナーの全リスト一覧（管理画面用、公開/非公開問わず）。
    async fn list_by_owner(&self, owner_actor_id: i64) -> Result<Vec<ListRow>, sqlx::Error>;

    /// 指定オーナーの公開リストのみ（他人のプロフィール画面用）。
    async fn list_public_by_owner(&self, owner_actor_id: i64) -> Result<Vec<ListRow>, sqlx::Error>;

    /// 指定オーナーのリスト数（上限チェック用）。
    async fn count_by_owner(&self, owner_actor_id: i64) -> Result<i64, sqlx::Error>;

    /// メンバーを追加する（既に追加済みなら何もしない）。
    async fn add_member(
        &self,
        list_id: i64,
        actor_id: i64,
        now: DateTime<Utc>,
    ) -> Result<(), sqlx::Error>;

    /// メンバーを削除する。削除できたら `true`。
    async fn remove_member(&self, list_id: i64, actor_id: i64) -> Result<bool, sqlx::Error>;

    /// リストのメンバー一覧（アクター情報込み、追加日時降順）。
    async fn members(&self, list_id: i64) -> Result<Vec<ListMemberRow>, sqlx::Error>;

    /// 複数リストのメンバー actor_id を一括取得する（`(list_id, actor_id)`、リストごとに
    /// `members`と同じ並び順・退会者除外）。リスト一覧をメンバー付きで返す際のN+1回避用。
    async fn member_ids_of_lists(&self, list_ids: &[i64]) -> Result<Vec<(i64, i64)>, sqlx::Error>;

    /// リストのメンバー数（上限チェック用）。
    async fn member_count(&self, list_id: i64) -> Result<i64, sqlx::Error>;

    /// 指定アクターが、いずれかのリストに現在も所属しているか。
    /// プロキシフォローの要否・維持判定（参照カウント方式）に使う。
    async fn actor_referenced_by_any_list(&self, actor_id: i64) -> Result<bool, sqlx::Error>;

    /// 指定アクターをメンバーに含むリストのID一覧（`streaming::ChannelScope`構築用、
    /// `userList`チャンネルへのリアルタイム配信判定に使う）。
    async fn list_ids_containing_actor(&self, actor_id: i64) -> Result<Vec<i64>, sqlx::Error>;

    /// リストタイムライン（home_timeline と同じ形の `TimelinePost` を返す）。
    async fn timeline(
        &self,
        list_id: i64,
        limit: i64,
        until_id: Option<i64>,
        since_id: Option<i64>,
    ) -> Result<Vec<TimelinePost>, sqlx::Error>;

    /// `app.bsky.graph.list` コミット後の rkey/uri/cid を保存する。
    async fn set_atp_list_record(
        &self,
        list_id: i64,
        at_rkey: &str,
        at_uri: &str,
        at_cid: &str,
    ) -> Result<(), sqlx::Error>;

    /// 公開設定を戻した場合等、ATPレコード情報をクリアする。
    async fn clear_atp_list_record(&self, list_id: i64) -> Result<(), sqlx::Error>;

    /// `app.bsky.graph.listitem` コミット後の rkey/uri を保存する。
    async fn set_member_atp_record(
        &self,
        list_id: i64,
        actor_id: i64,
        at_rkey: &str,
        at_uri: &str,
    ) -> Result<(), sqlx::Error>;

    /// メンバーの listitem rkey を取得する（削除コミット時に必要）。
    async fn find_member_atp_rkey(
        &self,
        list_id: i64,
        actor_id: i64,
    ) -> Result<Option<String>, sqlx::Error>;

    /// メンバーの listitem rkey/uri をクリアする（非公開化・メンバー削除・リスト削除時）。
    async fn clear_member_atp_record(&self, list_id: i64, actor_id: i64)
        -> Result<(), sqlx::Error>;

    /// `at_rkey` が設定済みの（＝ATPにlistitemコミット済みの）メンバーの `(actor_id, at_rkey)` 一覧。
    /// リストの非公開化・削除時に、削除すべきlistitemを列挙するために使う。
    async fn members_with_atp_record(
        &self,
        list_id: i64,
    ) -> Result<Vec<(i64, String)>, sqlx::Error>;
}

pub struct PgListRepository {
    pool: PgPool,
}

impl PgListRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl ListRepository for PgListRepository {
    async fn create(
        &self,
        id: i64,
        owner_actor_id: i64,
        name: &str,
        is_public: bool,
        now: DateTime<Utc>,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO lists (id, owner_actor_id, name, is_public, created_at, updated_at)
             VALUES ($1, $2, $3, $4, $5, $5)",
        )
        .bind(id)
        .bind(owner_actor_id)
        .bind(name)
        .bind(is_public)
        .bind(now)
        .execute(&self.pool)
        .await
        .map(|_| ())
    }

    async fn update(
        &self,
        id: i64,
        owner_actor_id: i64,
        edit: ListEdit,
    ) -> Result<Result<Option<ListUpdated>, EditConflict>, sqlx::Error> {
        let mut tx = self.pool.begin().await?;
        let current: Option<(String, bool)> = sqlx::query_as(
            "SELECT name, is_public FROM lists
             WHERE id = $1 AND owner_actor_id = $2 FOR UPDATE",
        )
        .bind(id)
        .bind(owner_actor_id)
        .fetch_optional(&mut *tx)
        .await?;
        let Some((name, was_public)) = current else {
            return Ok(Ok(None));
        };
        fn apply<T: PartialEq + Clone>(
            edit: Option<Edit<T>>,
            current: T,
        ) -> Result<T, EditConflict> {
            edit.map_or(Ok(current.clone()), |e| e.apply_value(&current))
        }
        let applied =
            apply(edit.name, name).and_then(|name| Ok((name, apply(edit.is_public, was_public)?)));
        let (name, is_public) = match applied {
            Ok(v) => v,
            Err(conflict) => return Ok(Err(conflict)),
        };
        sqlx::query("UPDATE lists SET name = $2, is_public = $3, updated_at = NOW() WHERE id = $1")
            .bind(id)
            .bind(&name)
            .bind(is_public)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(Ok(Some(ListUpdated {
            name,
            was_public,
            is_public,
        })))
    }

    async fn delete(&self, id: i64, owner_actor_id: i64) -> Result<u64, sqlx::Error> {
        let result = sqlx::query("DELETE FROM lists WHERE id = $1 AND owner_actor_id = $2")
            .bind(id)
            .bind(owner_actor_id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected())
    }

    async fn find_by_id(&self, id: i64) -> Result<Option<ListRow>, sqlx::Error> {
        sqlx::query_as::<_, ListRow>(
            "SELECT l.id, l.owner_actor_id, l.name, l.is_public, l.at_rkey, l.at_uri, l.at_cid,
                    l.created_at, l.updated_at,
                    COALESCE((SELECT COUNT(*) FROM list_members lm WHERE lm.list_id = l.id), 0) AS member_count
             FROM lists l WHERE l.id = $1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
    }

    async fn list_by_owner(&self, owner_actor_id: i64) -> Result<Vec<ListRow>, sqlx::Error> {
        sqlx::query_as::<_, ListRow>(
            "SELECT l.id, l.owner_actor_id, l.name, l.is_public, l.at_rkey, l.at_uri, l.at_cid,
                    l.created_at, l.updated_at,
                    COALESCE((SELECT COUNT(*) FROM list_members lm WHERE lm.list_id = l.id), 0) AS member_count
             FROM lists l WHERE l.owner_actor_id = $1 ORDER BY l.created_at ASC",
        )
        .bind(owner_actor_id)
        .fetch_all(&self.pool)
        .await
    }

    async fn list_public_by_owner(&self, owner_actor_id: i64) -> Result<Vec<ListRow>, sqlx::Error> {
        sqlx::query_as::<_, ListRow>(
            "SELECT l.id, l.owner_actor_id, l.name, l.is_public, l.at_rkey, l.at_uri, l.at_cid,
                    l.created_at, l.updated_at,
                    COALESCE((SELECT COUNT(*) FROM list_members lm WHERE lm.list_id = l.id), 0) AS member_count
             FROM lists l WHERE l.owner_actor_id = $1 AND l.is_public = true ORDER BY l.created_at ASC",
        )
        .bind(owner_actor_id)
        .fetch_all(&self.pool)
        .await
    }

    async fn count_by_owner(&self, owner_actor_id: i64) -> Result<i64, sqlx::Error> {
        sqlx::query_scalar("SELECT COUNT(*) FROM lists WHERE owner_actor_id = $1")
            .bind(owner_actor_id)
            .fetch_one(&self.pool)
            .await
    }

    async fn add_member(
        &self,
        list_id: i64,
        actor_id: i64,
        now: DateTime<Utc>,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO list_members (list_id, actor_id, added_at) VALUES ($1, $2, $3)
             ON CONFLICT (list_id, actor_id) DO NOTHING",
        )
        .bind(list_id)
        .bind(actor_id)
        .bind(now)
        .execute(&self.pool)
        .await
        .map(|_| ())
    }

    async fn remove_member(&self, list_id: i64, actor_id: i64) -> Result<bool, sqlx::Error> {
        let result = sqlx::query("DELETE FROM list_members WHERE list_id = $1 AND actor_id = $2")
            .bind(list_id)
            .bind(actor_id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    async fn members(&self, list_id: i64) -> Result<Vec<ListMemberRow>, sqlx::Error> {
        sqlx::query_as::<_, ListMemberRow>(
            "SELECT a.id AS actor_id, a.username, a.domain, a.display_name, a.emoji_map,
                    a.actor_type::text AS actor_type,
                    COALESCE(rtrim(asp.public_url, '/') || '/' || amf.storage_key, a.avatar_url) AS avatar_url,
                    lm.added_at, lm.at_rkey, lm.at_uri
             FROM list_members lm
             JOIN actors a ON a.id = lm.actor_id
             LEFT JOIN media_files amf ON amf.id = a.avatar_media_id
             LEFT JOIN storage_providers asp ON asp.id = amf.storage_provider_id
             WHERE lm.list_id = $1 AND a.withdrawn_at IS NULL
             ORDER BY lm.added_at DESC",
        )
        .bind(list_id)
        .fetch_all(&self.pool)
        .await
    }

    async fn member_ids_of_lists(&self, list_ids: &[i64]) -> Result<Vec<(i64, i64)>, sqlx::Error> {
        sqlx::query_as(
            "SELECT lm.list_id, lm.actor_id
             FROM list_members lm
             JOIN actors a ON a.id = lm.actor_id
             WHERE lm.list_id = ANY($1) AND a.withdrawn_at IS NULL
             ORDER BY lm.list_id, lm.added_at DESC",
        )
        .bind(list_ids)
        .fetch_all(&self.pool)
        .await
    }

    async fn member_count(&self, list_id: i64) -> Result<i64, sqlx::Error> {
        sqlx::query_scalar("SELECT COUNT(*) FROM list_members WHERE list_id = $1")
            .bind(list_id)
            .fetch_one(&self.pool)
            .await
    }

    async fn actor_referenced_by_any_list(&self, actor_id: i64) -> Result<bool, sqlx::Error> {
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM list_members WHERE actor_id = $1)")
            .bind(actor_id)
            .fetch_one(&self.pool)
            .await
    }

    async fn list_ids_containing_actor(&self, actor_id: i64) -> Result<Vec<i64>, sqlx::Error> {
        sqlx::query_scalar("SELECT list_id FROM list_members WHERE actor_id = $1")
            .bind(actor_id)
            .fetch_all(&self.pool)
            .await
    }

    async fn timeline(
        &self,
        list_id: i64,
        limit: i64,
        until_id: Option<i64>,
        since_id: Option<i64>,
    ) -> Result<Vec<TimelinePost>, sqlx::Error> {
        // home_timeline と同じ候補抽出（feed.rs）。対象が「自分+follows」ではなく
        // 「list_members」になるだけ。
        let source = super::feed::FeedSource::ListMembers { list_id };
        let page = super::feed::FeedPage {
            limit,
            until_id,
            since_id,
        };
        let ids = super::feed::feed_candidate_ids(&self.pool, source, page).await?;
        super::feed::fetch_feed_posts(&self.pool, &ids).await
    }

    async fn set_atp_list_record(
        &self,
        list_id: i64,
        at_rkey: &str,
        at_uri: &str,
        at_cid: &str,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE lists SET at_rkey = $2, at_uri = $3, at_cid = $4, updated_at = NOW() WHERE id = $1",
        )
        .bind(list_id)
        .bind(at_rkey)
        .bind(at_uri)
        .bind(at_cid)
        .execute(&self.pool)
        .await
        .map(|_| ())
    }

    async fn clear_atp_list_record(&self, list_id: i64) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE lists SET at_rkey = NULL, at_uri = NULL, at_cid = NULL, updated_at = NOW() WHERE id = $1",
        )
        .bind(list_id)
        .execute(&self.pool)
        .await
        .map(|_| ())
    }

    async fn set_member_atp_record(
        &self,
        list_id: i64,
        actor_id: i64,
        at_rkey: &str,
        at_uri: &str,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE list_members SET at_rkey = $3, at_uri = $4 WHERE list_id = $1 AND actor_id = $2",
        )
        .bind(list_id)
        .bind(actor_id)
        .bind(at_rkey)
        .bind(at_uri)
        .execute(&self.pool)
        .await
        .map(|_| ())
    }

    async fn find_member_atp_rkey(
        &self,
        list_id: i64,
        actor_id: i64,
    ) -> Result<Option<String>, sqlx::Error> {
        sqlx::query_scalar("SELECT at_rkey FROM list_members WHERE list_id = $1 AND actor_id = $2")
            .bind(list_id)
            .bind(actor_id)
            .fetch_optional(&self.pool)
            .await
            .map(|opt| opt.flatten())
    }

    async fn clear_member_atp_record(
        &self,
        list_id: i64,
        actor_id: i64,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE list_members SET at_rkey = NULL, at_uri = NULL WHERE list_id = $1 AND actor_id = $2",
        )
        .bind(list_id)
        .bind(actor_id)
        .execute(&self.pool)
        .await
        .map(|_| ())
    }

    async fn members_with_atp_record(
        &self,
        list_id: i64,
    ) -> Result<Vec<(i64, String)>, sqlx::Error> {
        sqlx::query_as(
            "SELECT actor_id, at_rkey FROM list_members WHERE list_id = $1 AND at_rkey IS NOT NULL",
        )
        .bind(list_id)
        .fetch_all(&self.pool)
        .await
    }
}

//! リモートインスタンス（`actors.domain`単位）のnodeinfoキャッシュ。
//! `jobs::remote_instance_info_resolve` が取得結果を書き込み、
//! notes API（`NoteUserInfo.instance`）・Misskey互換API（`MisskeyUserLite.instance`）が
//! ドメイン単位でまとめて読み出す（#NoteCardリモートサーバー表示）。

use std::collections::HashMap;

use async_trait::async_trait;
use sqlx::PgPool;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct RemoteInstanceMeta {
    pub domain: String,
    pub software_name: Option<String>,
    pub node_name: Option<String>,
    /// 表示に使う最終的なテーマカラー（宣言値、または既知software代替色・
    /// 汎用デフォルトへフォールバック済みの値）。
    pub theme_color: Option<String>,
    /// `<link rel="icon">`（無ければ`/favicon.ico`）から取得できたサーバーアイコンURL。
    /// 取得できなければ`None`（フロントエンドは🌐絵文字にフォールバックする）。
    pub icon_url: Option<String>,
}

#[async_trait]
pub trait RemoteInstanceMetaRepository: Send + Sync {
    /// 指定ドメイン群のキャッシュ済み行をまとめて取得する（N+1回避）。
    /// キャッシュに無いドメインは戻り値に含まれない。
    async fn get_many(
        &self,
        domains: &[String],
    ) -> Result<HashMap<String, RemoteInstanceMeta>, sqlx::Error>;

    /// nodeinfo取得結果をキャッシュへ書き込む（初回取得・再取得のどちらも同じUPSERT）。
    #[allow(clippy::too_many_arguments)]
    async fn upsert(
        &self,
        domain: &str,
        software_name: Option<&str>,
        node_name: Option<&str>,
        theme_color: Option<&str>,
        icon_url: Option<&str>,
    ) -> Result<(), sqlx::Error>;
}

pub struct PgRemoteInstanceMetaRepository {
    pool: PgPool,
}

impl PgRemoteInstanceMetaRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl RemoteInstanceMetaRepository for PgRemoteInstanceMetaRepository {
    async fn get_many(
        &self,
        domains: &[String],
    ) -> Result<HashMap<String, RemoteInstanceMeta>, sqlx::Error> {
        if domains.is_empty() {
            return Ok(HashMap::new());
        }
        let rows = sqlx::query_as::<_, RemoteInstanceMeta>(
            "SELECT domain, software_name, node_name, theme_color, icon_url
             FROM remote_instance_meta
             WHERE domain = ANY($1)",
        )
        .bind(domains)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(|r| (r.domain.clone(), r)).collect())
    }

    async fn upsert(
        &self,
        domain: &str,
        software_name: Option<&str>,
        node_name: Option<&str>,
        theme_color: Option<&str>,
        icon_url: Option<&str>,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO remote_instance_meta (domain, software_name, node_name, theme_color, icon_url, fetched_at)
             VALUES ($1, $2, $3, $4, $5, now())
             ON CONFLICT (domain) DO UPDATE SET
                 software_name = EXCLUDED.software_name,
                 node_name = EXCLUDED.node_name,
                 theme_color = EXCLUDED.theme_color,
                 icon_url = EXCLUDED.icon_url,
                 fetched_at = EXCLUDED.fetched_at",
        )
        .bind(domain)
        .bind(software_name)
        .bind(node_name)
        .bind(theme_color)
        .bind(icon_url)
        .execute(&self.pool)
        .await
        .map(|_| ())
    }
}

//! 同じ人を指す2つの `actors` 行の統合（相互申告マージの「既に両方の行がある」場合、
//! `seiran_actor_merge`）。統合される側（absorbed）を参照する行をすべて残る側（survivor）へ
//! 付け替え、absorbed を物理削除する。
//!
//! 付け替える列は `ACTOR_REFERENCES` に列挙する。`actors` を参照する外部キーを足したら
//! ここにも足すこと（足し忘れると統合で absorbed と一緒に CASCADE で消える）。
//! `crates/seiran-api/tests/actor_merge_integration.rs` が、DB の外部キーと UNIQUE 制約が
//! この一覧と一致していることを検査する。

use sqlx::PgConnection;

/// `actors.id` を参照する列。
pub struct ActorReference {
    pub table: &'static str,
    pub column: &'static str,
    /// この列を含む UNIQUE 制約（主キーを含む）。付け替えると survivor 側の既存行と重複する
    /// absorbed 側の行は、付け替えずに消す。
    pub unique: &'static [UniqueKey],
}

/// UNIQUE 制約の、参照列以外のキー列と、部分インデックスの条件。
pub struct UniqueKey {
    pub others: &'static [&'static str],
    /// `{t}` を行の別名に置き換えて使う。
    pub predicate: Option<&'static str>,
}

const fn key(others: &'static [&'static str]) -> UniqueKey {
    UniqueKey {
        others,
        predicate: None,
    }
}

const fn plain(table: &'static str, column: &'static str) -> ActorReference {
    ActorReference {
        table,
        column,
        unique: &[],
    }
}

pub const ACTOR_REFERENCES: &[ActorReference] = &[
    ActorReference {
        table: "actor_also_known_as",
        column: "owner_actor_id",
        unique: &[key(&["target_actor_id"])],
    },
    ActorReference {
        table: "actor_also_known_as",
        column: "target_actor_id",
        unique: &[key(&["owner_actor_id"])],
    },
    plain("actors", "bridge_real_actor_id"),
    plain("actors", "seiran_pair_actor_id"),
    plain("at_migration_requests", "actor_id"),
    ActorReference {
        table: "atp_app_passwords",
        column: "actor_id",
        unique: &[UniqueKey {
            others: &["name"],
            predicate: Some("{t}.revoked_at IS NULL"),
        }],
    },
    ActorReference {
        table: "atp_blocks",
        column: "actor_id",
        unique: &[key(&["cid"])],
    },
    plain("atp_deferred_writes", "actor_id"),
    ActorReference {
        table: "atp_preferences",
        column: "actor_id",
        unique: &[key(&[])],
    },
    ActorReference {
        table: "atp_records",
        column: "actor_id",
        unique: &[key(&["collection", "rkey"])],
    },
    plain("atp_refresh_tokens", "actor_id"),
    plain("atp_repo_events", "actor_id"),
    ActorReference {
        table: "blocks",
        column: "blocked_actor_id",
        unique: &[key(&["blocker_actor_id"])],
    },
    ActorReference {
        table: "blocks",
        column: "blocker_actor_id",
        unique: &[key(&["blocked_actor_id"])],
    },
    ActorReference {
        table: "bookmarks",
        column: "actor_id",
        unique: &[key(&["post_id"])],
    },
    ActorReference {
        table: "dm_bsky_reactions",
        column: "actor_id",
        unique: &[key(&["post_id", "content"])],
    },
    ActorReference {
        table: "dm_hidden_messages",
        column: "actor_id",
        unique: &[key(&["post_id"])],
    },
    ActorReference {
        table: "dm_read_states",
        column: "actor_id",
        unique: &[key(&["thread_root_post_id"])],
    },
    ActorReference {
        table: "drive_file_sensitivity",
        column: "actor_id",
        unique: &[key(&["media_file_id"])],
    },
    plain("email_short_codes", "actor_id"),
    ActorReference {
        table: "follow_import_requests",
        column: "actor_id",
        unique: &[UniqueKey {
            others: &[],
            predicate: Some("{t}.status = 'running'"),
        }],
    },
    ActorReference {
        table: "follows",
        column: "follower_actor_id",
        unique: &[key(&["target_actor_id"])],
    },
    ActorReference {
        table: "follows",
        column: "target_actor_id",
        unique: &[key(&["follower_actor_id"])],
    },
    plain("link_resolutions", "resolved_actor_id"),
    ActorReference {
        table: "list_members",
        column: "actor_id",
        unique: &[key(&["list_id"])],
    },
    ActorReference {
        table: "lists",
        column: "owner_actor_id",
        unique: &[key(&["name"])],
    },
    plain("local_username_claims", "actor_id"),
    plain("media_files", "uploaded_by_actor_id"),
    ActorReference {
        table: "mutes",
        column: "muted_actor_id",
        unique: &[key(&["muter_actor_id"])],
    },
    ActorReference {
        table: "mutes",
        column: "muter_actor_id",
        unique: &[key(&["muted_actor_id"])],
    },
    plain("notifications", "notifier_actor_id"),
    plain("notifications", "recipient_actor_id"),
    plain("notifications", "related_actor_id"),
    ActorReference {
        table: "pending_did_provisions",
        column: "actor_id",
        unique: &[key(&[])],
    },
    ActorReference {
        table: "pinned_hashtags",
        column: "actor_id",
        unique: &[key(&["hashtag_id"])],
    },
    ActorReference {
        table: "pinned_posts",
        column: "actor_id",
        unique: &[key(&["post_id"])],
    },
    ActorReference {
        table: "poll_votes",
        column: "actor_id",
        unique: &[key(&["post_id", "option_index"])],
    },
    ActorReference {
        table: "post_recipients",
        column: "actor_id",
        unique: &[key(&["post_id"])],
    },
    ActorReference {
        table: "posts",
        column: "actor_id",
        unique: &[UniqueKey {
            others: &["repost_of_post_id"],
            predicate: Some("{t}.repost_of_post_id IS NOT NULL AND {t}.deleted_at IS NULL"),
        }],
    },
    ActorReference {
        table: "reactions",
        column: "actor_id",
        unique: &[key(&["post_id"])],
    },
    ActorReference {
        table: "remote_follow_snapshots",
        column: "actor_id",
        unique: &[key(&["direction"])],
    },
    plain("reports", "reporter_actor_id"),
    plain("reports", "subject_actor_id"),
    ActorReference {
        table: "repost_mutes",
        column: "muted_actor_id",
        unique: &[key(&["muter_actor_id"])],
    },
    ActorReference {
        table: "repost_mutes",
        column: "muter_actor_id",
        unique: &[key(&["muted_actor_id"])],
    },
    plain("scheduled_posts", "actor_id"),
    plain("search_log", "actor_id"),
    plain("user_contact_log", "actor_id"),
    plain("user_contact_log", "target_actor_id"),
];

/// 統合で自分自身を指すことになる行を消すテーブル（2つの参照列が同じ人を指す関係は無意味。
/// 例: absorbed が survivor をフォローしていた）。
const SELF_PAIRS: &[(&str, &str, &str)] = &[
    ("actor_also_known_as", "owner_actor_id", "target_actor_id"),
    ("blocks", "blocker_actor_id", "blocked_actor_id"),
    ("follows", "follower_actor_id", "target_actor_id"),
    ("mutes", "muter_actor_id", "muted_actor_id"),
    ("repost_mutes", "muter_actor_id", "muted_actor_id"),
];

/// absorbed から消した行の、統合後の survivor に引き継ぐ値。
pub struct AbsorbedActor {
    pub at_did: Option<String>,
    pub at_handle: Option<String>,
}

/// absorbed を参照する行を survivor へ付け替え、absorbed を削除する。呼び出し元の
/// トランザクションで実行し、survivor の `at_did` 等の設定はその後で行う（absorbed の削除前に
/// 設定すると `at_did`・`ap_uri` の UNIQUE に当たる）。
pub async fn absorb_actor(
    conn: &mut PgConnection,
    survivor: i64,
    absorbed: i64,
) -> Result<Option<AbsorbedActor>, sqlx::Error> {
    for r in ACTOR_REFERENCES {
        for key in r.unique {
            let mut cond: Vec<String> = key
                .others
                .iter()
                .map(|c| format!("x.{c} = t.{c}"))
                .collect();
            if let Some(p) = key.predicate {
                cond.push(p.replace("{t}", "t"));
                cond.push(p.replace("{t}", "x"));
            }
            let extra: String = cond.iter().map(|c| format!(" AND {c}")).collect();
            let sql = format!(
                "DELETE FROM {table} t WHERE t.{col} = $2 AND EXISTS \
                 (SELECT 1 FROM {table} x WHERE x.{col} = $1{extra})",
                table = r.table,
                col = r.column,
            );
            sqlx::query(sqlx::AssertSqlSafe(sql))
                .bind(survivor)
                .bind(absorbed)
                .execute(&mut *conn)
                .await?;
        }
        let sql = format!(
            "UPDATE {table} SET {col} = $1 WHERE {col} = $2",
            table = r.table,
            col = r.column,
        );
        sqlx::query(sqlx::AssertSqlSafe(sql))
            .bind(survivor)
            .bind(absorbed)
            .execute(&mut *conn)
            .await?;
    }
    for (table, a, b) in SELF_PAIRS {
        let sql = format!("DELETE FROM {table} WHERE {a} = $1 AND {b} = $1");
        sqlx::query(sqlx::AssertSqlSafe(sql))
            .bind(survivor)
            .execute(&mut *conn)
            .await?;
    }
    // 投稿数はトリガーが INSERT と論理削除でしか数え直さない
    sqlx::query(
        "UPDATE actors SET notes_count = \
         (SELECT count(*) FROM posts WHERE actor_id = $1 AND deleted_at IS NULL) \
         WHERE id = $1",
    )
    .bind(survivor)
    .execute(&mut *conn)
    .await?;

    let row: Option<(Option<String>, Option<String>)> =
        sqlx::query_as("DELETE FROM actors WHERE id = $1 RETURNING at_did, at_handle")
            .bind(absorbed)
            .fetch_optional(&mut *conn)
            .await?;
    Ok(row.map(|(at_did, at_handle)| AbsorbedActor { at_did, at_handle }))
}

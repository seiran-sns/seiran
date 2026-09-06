//! UNIQUE制約違反（PostgreSQL SQLSTATE 23505）を検知したらトランザクションの頭から
//! やり直す、汎用リトライヘルパー。
//!
//! リモートseiran連合（#236アクター統合・#237投稿マージ）は、「対応行を探す→無ければ
//! INSERT」という判定をAP側・ATP側がほぼ同時に行いうる。かつてはこの判定〜書き込みを
//! `pg_advisory_xact_lock`で直列化していたが、ロックキーの選び方が呼び出し経路ごとに
//! 微妙に異なり（例: ATP側は自己申告の有無でキーが`at_did`/相手のfedi IDのどちらかに
//! 揺れる）、一致しない組み合わせで直列化が効かない・デッドロックの懸念もあった。
//!
//! 代わりに、「結婚が成立するべき2行」が物理的に共存できないことを`actors`/`posts`の
//! 複合UNIQUE制約（`docs/protocols.md` 5節・11節参照）でDB側に保証させ、Rust側は
//! 制約違反を検知したらトランザクション全体をやり直すだけにする（マイケルの提案）。
//! Rust側の一意性判定ロジック自体に誤りがあった場合に無限リトライへ陥らないよう、
//! リトライ上限を設ける。

use std::future::Future;

/// リトライ上限（マイケル指定）。
pub const MAX_UNIQUE_VIOLATION_RETRIES: u32 = 5;

fn is_unique_violation(err: &sqlx::Error) -> bool {
    matches!(err, sqlx::Error::Database(db_err) if db_err.is_unique_violation())
}

/// `f`を最大`MAX_UNIQUE_VIOLATION_RETRIES`回まで実行する。`f`は呼び出しのたびに
/// 新しいトランザクションを開始しcommitまで行うこと（このヘルパー自体はトランザクション
/// を持たない）。UNIQUE制約違反以外のエラーは即座に伝播する。
pub async fn retry_on_unique_violation<T, F, Fut>(mut f: F) -> Result<T, sqlx::Error>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, sqlx::Error>>,
{
    for attempt in 1..=MAX_UNIQUE_VIOLATION_RETRIES {
        match f().await {
            Ok(v) => return Ok(v),
            Err(e) if is_unique_violation(&e) && attempt < MAX_UNIQUE_VIOLATION_RETRIES => {
                tracing::warn!(
                    "[unique_retry] UNIQUE制約違反のためトランザクションをやり直します \
                     (attempt={}/{}): {}",
                    attempt,
                    MAX_UNIQUE_VIOLATION_RETRIES,
                    e
                );
            }
            Err(e) => return Err(e),
        }
    }
    unreachable!("ループは最低1回Ok/Errのいずれかでreturnする")
}

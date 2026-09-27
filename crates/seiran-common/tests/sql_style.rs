//! SQL記述ルールの機械的検査（`docs/coding_rules.md` 2節）。
//!
//! 1. `x IN (SELECT ...)` / `x NOT IN (SELECT ...)` を使わず `EXISTS` / `NOT EXISTS` で書く。
//!    `NOT IN (SELECT ...)` はサブクエリ結果に NULL が1件でもあると条件全体が UNKNOWN
//!    （WHERE では偽）になる。`IN (SELECT ...)` は `NOT` を足すだけで同じ罠に落ちるので形ごと
//!    禁止する。リテラル列挙（`IN ('a', 'b')`）と `= ANY($1)` は対象外。
//! 2. SQL は Repository 層（`seiran-common/src/repository/`）に置き、ハンドラ・ジョブ・
//!    firehose には書かない。
//!
//! ワークスペースの全 `.rs`（コメント行を除く）と全マイグレーション（`--` コメントを除く）を走査する。

use std::path::{Path, PathBuf};

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("ワークスペースルートの解決に失敗")
}

fn collect_files(dir: &Path, exts: &[&str], out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if path.file_name().is_some_and(|n| n == "target") {
                continue;
            }
            collect_files(&path, exts, out);
        } else if path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| exts.contains(&e))
        {
            out.push(path);
        }
    }
}

/// コメント行を除いた本文を、空白を1つに畳んだ大文字の文字列にする（複数行にまたがる
/// `IN (\n    SELECT` も検出するため）。
fn normalized_code(content: &str, comment_prefix: &str) -> String {
    content
        .lines()
        .filter(|line| !line.trim_start().starts_with(comment_prefix))
        .collect::<Vec<_>>()
        .join(" ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_uppercase()
}

fn contains_in_subquery(code: &str) -> bool {
    // "IN (SELECT" と "IN(SELECT" の両方。`NOT IN` もこれに含まれる。
    let pattern_spaced = ["IN", " (", "SELECT"].concat();
    let pattern_tight = ["IN", "(", "SELECT"].concat();
    code.match_indices(&pattern_spaced)
        .chain(code.match_indices(&pattern_tight))
        .any(|(i, _)| {
            // `JOIN (SELECT`・`MIN(SELECT` 等の語の一部を誤検出しないよう、直前が語境界であること。
            i == 0
                || !code[..i]
                    .chars()
                    .next_back()
                    .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_')
        })
}

#[test]
fn no_in_subquery_in_sql() {
    let root = workspace_root();
    let mut rust_files = Vec::new();
    collect_files(&root.join("crates"), &["rs"], &mut rust_files);
    let mut sql_files = Vec::new();
    collect_files(
        &root.join("crates/seiran-common/migrations"),
        &["sql"],
        &mut sql_files,
    );
    assert!(!rust_files.is_empty(), "走査対象の .rs が見つからない");

    let this_file = Path::new(file!()).file_name().unwrap();
    let mut violations = Vec::new();
    for path in &rust_files {
        if path.file_name() == Some(this_file) {
            continue;
        }
        let content = std::fs::read_to_string(path).unwrap();
        if contains_in_subquery(&normalized_code(&content, "//")) {
            violations.push(path.clone());
        }
    }
    for path in &sql_files {
        let content = std::fs::read_to_string(path).unwrap();
        if contains_in_subquery(&normalized_code(&content, "--")) {
            violations.push(path.clone());
        }
    }
    assert!(
        violations.is_empty(),
        "IN (SELECT ...) / NOT IN (SELECT ...) は禁止です。EXISTS / NOT EXISTS で書き直してください:\n{}",
        violations
            .iter()
            .map(|p| format!("  {}", p.display()))
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn detects_in_subquery_variants() {
    assert!(contains_in_subquery("WHERE ID NOT IN (SELECT X FROM T)"));
    assert!(contains_in_subquery("WHERE ID IN(SELECT X FROM T)"));
    assert!(!contains_in_subquery("WHERE ID IN ('A', 'B')"));
    assert!(!contains_in_subquery("FROM A JOIN (SELECT 1) B"));
    assert!(!contains_in_subquery("WHERE ID = ANY($1)"));
}

/// SQL を書いてはいけない層（ワークスペースルートからの相対パス）。
const NO_SQL_DIRS: &[&str] = &[
    "crates/seiran-api/src",
    "crates/seiran-federation-inbox/src",
    "crates/seiran-common/src/jobs",
    "crates/seiran-atp-repo/src",
];

/// `sqlx::query*` の呼び出しと、クエリを組み立て・読むための型（`QueryBuilder`・`Row`）の使用。
/// `PgPool` を受け渡すだけなら違反にしない。
fn contains_raw_sql(code: &str) -> bool {
    let imports_query_items = code.lines().any(|line| {
        let line = line.trim_start();
        line.starts_with("use sqlx::")
            && ["query", "QueryBuilder", "Row"]
                .iter()
                .any(|item| line.contains(item))
    });
    imports_query_items
        || ["sqlx::query", "sqlx::QueryBuilder", "sqlx::Row"]
            .iter()
            .any(|p| code.contains(p))
}

#[test]
fn no_sql_outside_repository() {
    let root = workspace_root();
    let mut violations = Vec::new();
    for dir in NO_SQL_DIRS {
        let mut files = Vec::new();
        collect_files(&root.join(dir), &["rs"], &mut files);
        assert!(!files.is_empty(), "{dir} に .rs が見つからない");
        for path in files {
            let content = std::fs::read_to_string(&path).unwrap();
            let code: String = content
                .lines()
                .filter(|line| !line.trim_start().starts_with("//"))
                .collect::<Vec<_>>()
                .join("\n");
            if contains_raw_sql(&code) {
                violations.push(path);
            }
        }
    }
    assert!(
        violations.is_empty(),
        "ハンドラ・ジョブ・firehose に SQL を書かず、seiran-common の repository/ へ移してください:\n{}",
        violations
            .iter()
            .map(|p| format!("  {}", p.display()))
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn detects_raw_sql_usage() {
    assert!(contains_raw_sql("sqlx::query(\"SELECT 1\")"));
    assert!(contains_raw_sql("use sqlx::{QueryBuilder, Row};"));
    assert!(contains_raw_sql("r.try_get::<i64, _>(\"id\") // sqlx::Row"));
    assert!(!contains_raw_sql(
        "use sqlx::PgPool;\nfn f(pool: &sqlx::PgPool) {}"
    ));
}

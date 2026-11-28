//! 投稿検索の検索語の解析と、ローカル DB の検索。
//!
//! 検索語は語・引用符で囲んだ語句・括弧・縦棒（OR）・先頭の `-`（除外）・`+`（AND）だけを
//! 解釈して AST（`SearchCondition`）にする。`from:`・`since:` 等の指定子の形もただの語として
//! 扱う。条件は検索語とは別のパラメータ（`PostSearchFilters`）で指定するので、検索語の中の
//! 指定子が条件とかち合わないようにするため。同じ AST をローカル DB の SQL と AppView の
//! 検索式（`to_appview_query`）の両方に使い、どちらでも同じ意味にする。

use chrono::{DateTime, Utc};
use sqlx::{PgPool, Postgres, QueryBuilder};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SearchCondition {
    /// 語（引用符で囲んだ語句を含む。部分一致で探す）。
    Text(String),
    Not(Box<SearchCondition>),
    And(Vec<SearchCondition>),
    Or(Vec<SearchCondition>),
    True,
    False,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Token {
    Text(String),
    Control(char),
    Or,
}

struct Tokenizer<'a> {
    input: &'a str,
    pos: usize,
}

impl<'a> Tokenizer<'a> {
    fn new(input: &'a str) -> Self {
        Self { input, pos: 0 }
    }

    fn next(&mut self) -> Option<Token> {
        let base = self.pos;
        let chars: Vec<(usize, char)> = self.input[base..].char_indices().collect();
        let mut text = String::new();
        let mut quoted = false;
        let mut escaped = false;

        for (offset, ch) in chars {
            let absolute = base + offset;
            match ch {
                '"' if !escaped => {
                    if quoted {
                        self.pos = absolute + ch.len_utf8();
                        return Some(Token::Text(text));
                    }
                    if text.is_empty() {
                        quoted = true;
                        self.pos = absolute + ch.len_utf8();
                    } else {
                        self.pos = absolute;
                        return Some(Token::Text(text));
                    }
                }
                '\\' if !escaped => {
                    escaped = true;
                    self.pos = absolute + ch.len_utf8();
                }
                '(' | ')' | '|' if !quoted && !escaped => {
                    if text.is_empty() {
                        self.pos = absolute + ch.len_utf8();
                        return Some(if ch == '|' {
                            Token::Or
                        } else {
                            Token::Control(ch)
                        });
                    }
                    self.pos = absolute;
                    return Some(Token::Text(text));
                }
                ch if !quoted && !escaped && ch.is_whitespace() => {
                    self.pos = absolute + ch.len_utf8();
                    if !text.is_empty() {
                        return Some(Token::Text(text));
                    }
                }
                '+' | '-' if !quoted && !escaped && text.is_empty() => {
                    self.pos = absolute + ch.len_utf8();
                    return Some(Token::Control(ch));
                }
                _ => {
                    text.push(ch);
                    escaped = false;
                    self.pos = absolute + ch.len_utf8();
                }
            }
        }
        (!text.is_empty()).then_some(Token::Text(text))
    }
}

pub fn parse(query: &str) -> SearchCondition {
    optimize(parse_partial(&mut Tokenizer::new(query), true))
}

fn parse_partial(tokenizer: &mut Tokenizer<'_>, root: bool) -> SearchCondition {
    let mut current = SearchCondition::True;
    let mut join = Join::And;
    while let Some(token) = tokenizer.next() {
        match token {
            Token::Control('(') => {
                let found = parse_partial(tokenizer, false);
                current = combine(current, found, join);
                join = Join::And;
            }
            Token::Control(')') => {
                if !root {
                    break;
                }
                // An unmatched closing parenthesis behaves as if its opening pair
                // had been inserted at the beginning.
                join = Join::And;
            }
            Token::Control('-') => join = Join::Not,
            Token::Control('+') => join = Join::And,
            Token::Or => join = Join::Or,
            Token::Text(text) if !text.is_empty() => {
                current = combine(current, SearchCondition::Text(text), join);
                join = Join::And;
            }
            _ => {}
        }
    }
    current
}

#[derive(Clone, Copy)]
enum Join {
    And,
    Or,
    Not,
}

fn combine(left: SearchCondition, right: SearchCondition, join: Join) -> SearchCondition {
    let right = if matches!(join, Join::Not) {
        SearchCondition::Not(Box::new(right))
    } else {
        right
    };
    match (left, join) {
        (SearchCondition::True, _) => right,
        (SearchCondition::And(mut values), Join::And | Join::Not) => {
            values.push(right);
            SearchCondition::And(values)
        }
        (SearchCondition::Or(mut values), Join::Or) => {
            values.push(right);
            SearchCondition::Or(values)
        }
        (left, Join::Or) => SearchCondition::Or(vec![left, right]),
        (left, _) => SearchCondition::And(vec![left, right]),
    }
}

fn optimize(condition: SearchCondition) -> SearchCondition {
    match condition {
        SearchCondition::Not(inner) => match optimize(*inner) {
            SearchCondition::True => SearchCondition::False,
            SearchCondition::False => SearchCondition::True,
            SearchCondition::Not(inner) => *inner,
            value => SearchCondition::Not(Box::new(value)),
        },
        SearchCondition::And(values) => optimize_list(values, true),
        SearchCondition::Or(values) => optimize_list(values, false),
        value => value,
    }
}

fn optimize_list(values: Vec<SearchCondition>, and: bool) -> SearchCondition {
    let mut result = Vec::new();
    for value in values.into_iter().map(optimize) {
        if (and && value == SearchCondition::False) || (!and && value == SearchCondition::True) {
            return value;
        }
        if (and && value == SearchCondition::True) || (!and && value == SearchCondition::False) {
            continue;
        }
        let flattened = match (&value, and) {
            (SearchCondition::And(items), true) | (SearchCondition::Or(items), false) => {
                Some(items.clone())
            }
            _ => None,
        };
        if let Some(items) = flattened {
            for item in items {
                if !result.contains(&item) {
                    result.push(item);
                }
            }
        } else if !result.contains(&value) {
            result.push(value);
        }
    }
    match result.len() {
        0 => SearchCondition::True,
        1 => result.pop().expect("length checked"),
        _ if and => SearchCondition::And(result),
        _ => SearchCondition::Or(result),
    }
}

/// AST を SQL の条件として足す（`p` が posts）。語は本文の部分一致（大文字小文字を問わない）。
pub fn append_sql(condition: &SearchCondition, builder: &mut QueryBuilder<Postgres>) {
    match condition {
        SearchCondition::Text(value) => {
            builder
                .push("LOWER(p.body) LIKE ")
                .push_bind(format!("%{}%", escape_like(&value.to_lowercase())))
                .push(" ESCAPE '\\'");
        }
        SearchCondition::Not(inner) => {
            builder.push("NOT (");
            append_sql(inner, builder);
            builder.push(")");
        }
        SearchCondition::And(values) | SearchCondition::Or(values) => {
            let separator = if matches!(condition, SearchCondition::And(_)) {
                " AND "
            } else {
                " OR "
            };
            builder.push("(");
            for (index, value) in values.iter().enumerate() {
                if index > 0 {
                    builder.push(separator);
                }
                append_sql(value, builder);
            }
            builder.push(")");
        }
        SearchCondition::True => {
            builder.push("TRUE");
        }
        SearchCondition::False => {
            builder.push("FALSE");
        }
    }
}

/// AST を AppView（`app.bsky.feed.searchPosts`）の検索式に組み直す。語は必ず二重引用符で
/// 囲む（囲まないと `from:` 等が AppView の指定子として効き、条件欄から渡すパラメータと
/// かち合う）。AND は空白、OR は `|`（AppView は `OR` という語を OR として扱わない）、除外は
/// 先頭の `-`、入れ子は括弧。何も一致しえない式（`False`）なら `None`（問い合わせない）、
/// 語が無ければ `*`（`q` は必須）。語の中の `"` は空白にする（引用符を閉じてしまうため）。
pub fn to_appview_query(condition: &SearchCondition) -> Option<String> {
    fn join(values: &[SearchCondition], separator: &str) -> String {
        values
            .iter()
            .map(nested)
            .collect::<Vec<_>>()
            .join(separator)
    }
    // 入れ子の AND・OR は必ず括弧で囲む（AND と `|` の優先順位に頼らない）。
    fn nested(condition: &SearchCondition) -> String {
        match condition {
            SearchCondition::Text(value) => format!("\"{}\"", value.replace('"', " ")),
            SearchCondition::Not(inner) => format!("-{}", nested(inner)),
            SearchCondition::And(values) => format!("({})", join(values, " ")),
            SearchCondition::Or(values) => format!("({})", join(values, " | ")),
            // `optimize` 済みの AST では入れ子に現れない。
            SearchCondition::True => "*".to_owned(),
            SearchCondition::False => String::new(),
        }
    }
    match condition {
        SearchCondition::False => None,
        SearchCondition::True => Some("*".to_owned()),
        SearchCondition::And(values) => Some(join(values, " ")),
        SearchCondition::Or(values) => Some(join(values, " | ")),
        other => Some(nested(other)),
    }
}

fn escape_like(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

/// ローカル DB を検索し、閲覧者 `me`（`(actor_id, acct)`）に見える投稿の id を新しい順に返す。
/// 検索語とは別に指定する投稿検索の条件（検索画面の条件欄、Misskey の `userId`/`host`、
/// Mastodon の `account_id`）。種類どうしは AND で、空の項目は絞らない。投稿者・サーバー・
/// ハッシュタグが1つずつなのは、Misskey・Mastodon の互換 API と
/// Bluesky の `searchPosts` がどれも1つしか指定できないため（複数を OR にすると Bluesky に
/// 値ごとに問い合わせることになる）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PostSearchFilters {
    /// この時刻以降に作られた投稿。
    pub since: Option<DateTime<Utc>>,
    /// この時刻より前に作られた投稿。
    pub until: Option<DateTime<Utc>>,
    /// 投稿者。
    pub actor_id: Option<i64>,
    /// 投稿者のサーバー。
    pub server: ServerFilter,
    /// ハッシュタグ（`hashtags.name` と同じ正規化: 先頭 `#` なし・小文字）。
    pub hashtag: Option<String>,
}

/// 投稿者のサーバーの指定。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum ServerFilter {
    #[default]
    Any,
    /// このサーバーのローカルユーザー。
    Local,
    /// ほかの Fedi サーバー1つ（ホスト名、小文字）。
    Host(String),
    /// Fedi 全体（ローカルを含む。Bluesky だけのアカウント以外）。
    Fediverse,
    /// Bluesky 全体（AT Protocol だけのアカウント。`actors.domain` が空）。PDS ごとには
    /// 分けず1つのサーバーとして扱う。
    Bluesky,
}

impl ServerFilter {
    /// Bluesky の投稿が一致しうるか（AppView に問い合わせるか）。
    pub fn includes_bluesky(&self) -> bool {
        matches!(self, Self::Any | Self::Bluesky)
    }
}

impl PostSearchFilters {
    pub fn is_empty(&self) -> bool {
        self.since.is_none()
            && self.until.is_none()
            && self.actor_id.is_none()
            && self.server == ServerFilter::Any
            && self.hashtag.is_none()
    }
}

/// `filters` の条件を ` AND ...` として足す（`p` が posts、`a` が投稿者の actors）。
fn append_filters_sql(filters: &PostSearchFilters, sql: &mut QueryBuilder<Postgres>) {
    if let Some(since) = filters.since {
        sql.push(" AND p.created_at >= ").push_bind(since);
    }
    if let Some(until) = filters.until {
        sql.push(" AND p.created_at < ").push_bind(until);
    }
    if let Some(actor_id) = filters.actor_id {
        sql.push(" AND p.actor_id = ").push_bind(actor_id);
    }
    match &filters.server {
        ServerFilter::Any => {}
        ServerFilter::Local => {
            sql.push(" AND a.actor_type = 'local'");
        }
        ServerFilter::Host(host) => {
            sql.push(" AND a.actor_type != 'local' AND LOWER(a.domain) = ")
                .push_bind(host.clone());
        }
        ServerFilter::Fediverse => {
            sql.push(" AND a.domain <> ''");
        }
        ServerFilter::Bluesky => {
            sql.push(" AND a.actor_type != 'local' AND a.domain = ''");
        }
    }
    if let Some(hashtag) = &filters.hashtag {
        sql.push(
            " AND EXISTS (SELECT 1 FROM post_hashtags ph JOIN hashtags h ON h.id = ph.hashtag_id
                          WHERE ph.post_id = p.id AND h.name = ",
        )
        .push_bind(hashtag.clone())
        .push(")");
    }
}

/// ローカル DB の投稿検索。`query` は検索式（空なら条件だけで探す）。新しい順に
/// `fetch_limit` 件まで。
pub async fn search_local_post_ids(
    pool: &PgPool,
    query: &str,
    filters: &PostSearchFilters,
    fetch_limit: i64,
    until_id: Option<i64>,
    since_id: Option<i64>,
    me: Option<(i64, &str)>,
) -> Result<Vec<i64>, sqlx::Error> {
    // pg_bigm は LIKE のみ最適化対象（ILIKE 非対応）で、索引は LOWER(body) に張っているため
    // 大文字小文字を無視した部分一致は LOWER() LIKE LOWER() の形で書く（`append_sql`）。
    let condition = parse(query);
    let mut sql =
        QueryBuilder::new("SELECT p.id FROM posts p JOIN actors a ON a.id = p.actor_id WHERE ");
    append_sql(&condition, &mut sql);
    append_filters_sql(filters, &mut sql);
    sql.push(" AND p.deleted_at IS NULL");
    sql.push(" AND post_is_visible_to(")
        .push_bind(me.map(|(actor_id, _)| actor_id))
        .push(", p.actor_id, p.visibility::text, p.id, false)");
    if let Some(uid) = until_id {
        sql.push(" AND p.id < ").push_bind(uid);
    }
    if let Some(sid) = since_id {
        sql.push(" AND p.id > ").push_bind(sid);
    }
    sql.push(" ORDER BY p.id DESC LIMIT ")
        .push_bind(fetch_limit);
    sql.build_query_scalar().fetch_all(pool).await
}

/// `ids` のうち `filters` を満たす投稿（AppView の検索結果を、取り込んだ後に同じ条件で
/// 確かめる。AppView に渡せる条件は一部だけで、渡し方の意味も完全には一致しないため）。
pub async fn filter_post_ids(
    pool: &PgPool,
    ids: &[i64],
    filters: &PostSearchFilters,
) -> Result<Vec<i64>, sqlx::Error> {
    if filters.is_empty() || ids.is_empty() {
        return Ok(ids.to_vec());
    }
    let mut sql = QueryBuilder::new(
        "SELECT p.id FROM posts p JOIN actors a ON a.id = p.actor_id WHERE p.id = ANY(",
    );
    sql.push_bind(ids.to_vec()).push(")");
    append_filters_sql(filters, &mut sql);
    sql.build_query_scalar().fetch_all(pool).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(value: &str) -> SearchCondition {
        SearchCondition::Text(value.into())
    }

    #[test]
    fn parses_quotes_groups_bar_and_exclusion() {
        assert_eq!(
            parse(r#"cat ("exact phrase" | #rust) -spam"#),
            SearchCondition::And(vec![
                text("cat"),
                SearchCondition::Or(vec![text("exact phrase"), text("#rust")]),
                SearchCondition::Not(Box::new(text("spam"))),
            ])
        );
    }

    #[test]
    fn operator_like_words_and_or_keyword_are_plain_words() {
        assert_eq!(
            parse("from:alice since:2026-01-02 lang:ja OR"),
            SearchCondition::And(vec![
                text("from:alice"),
                text("since:2026-01-02"),
                text("lang:ja"),
                text("OR"),
            ])
        );
        assert_eq!(
            parse("a|b"),
            SearchCondition::Or(vec![text("a"), text("b")])
        );
    }

    #[test]
    fn balances_missing_parentheses() {
        assert_eq!(parse("(one | two"), parse("one | two)"));
    }

    #[test]
    fn deduplicates_terms() {
        assert_eq!(parse("hello hello"), text("hello"));
    }

    #[test]
    fn appview_query_quotes_every_word() {
        let q = |s: &str| to_appview_query(&parse(s));
        assert_eq!(
            q("cat from:bsky.app").as_deref(),
            Some(r#""cat" "from:bsky.app""#)
        );
        assert_eq!(
            q(r#"orange (kitten | "puppy dog") -spam"#).as_deref(),
            Some(r#""orange" ("kitten" | "puppy dog") -"spam""#)
        );
        assert_eq!(q("(a b) | c").as_deref(), Some(r#"("a" "b") | "c""#));
        assert_eq!(q("-(a | b)").as_deref(), Some(r#"-("a" | "b")"#));
        assert_eq!(q("").as_deref(), Some("*"));
        assert_eq!(q(r#"say \"hi"#).as_deref(), Some(r#""say" " hi""#));
    }
}

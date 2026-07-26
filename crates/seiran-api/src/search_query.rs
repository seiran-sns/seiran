//! Bluesky-compatible post search query parsing and SQL generation.

use chrono::{DateTime, NaiveDate, Utc};
use sqlx::{Postgres, QueryBuilder};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SearchCondition {
    Text(String),
    From(String),
    Mentions(String),
    Domain(String),
    Since(DateTime<Utc>),
    Until(DateTime<Utc>),
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
                        return Some(classify_text(text));
                    }
                }
                '\\' if !escaped => {
                    escaped = true;
                    self.pos = absolute + ch.len_utf8();
                }
                '(' | ')' if !quoted && !escaped => {
                    if text.is_empty() {
                        self.pos = absolute + ch.len_utf8();
                        return Some(Token::Control(ch));
                    }
                    self.pos = absolute;
                    return Some(classify_text(text));
                }
                ch if !quoted && !escaped && ch.is_whitespace() => {
                    self.pos = absolute + ch.len_utf8();
                    if !text.is_empty() {
                        return Some(classify_text(text));
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
        (!text.is_empty()).then(|| classify_text(text))
    }
}

fn classify_text(text: String) -> Token {
    if text.eq_ignore_ascii_case("or") {
        Token::Or
    } else {
        Token::Text(text)
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
                current = combine(current, parse_term(text), join);
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

fn parse_term(text: String) -> SearchCondition {
    let Some((operator, value)) = text.split_once(':') else {
        return SearchCondition::Text(text);
    };
    if value.is_empty() {
        return SearchCondition::Text(text);
    }
    match operator.to_ascii_lowercase().as_str() {
        "from" => SearchCondition::From(normalize_handle(value)),
        "mentions" => SearchCondition::Mentions(normalize_handle(value)),
        "domain" => SearchCondition::Domain(value.to_ascii_lowercase()),
        // Local and federated posts do not reliably declare a language.
        "lang" => SearchCondition::True,
        "since" => parse_date(value)
            .map(SearchCondition::Since)
            .unwrap_or(SearchCondition::Text(text)),
        "until" => parse_date(value)
            .map(SearchCondition::Until)
            .unwrap_or(SearchCondition::Text(text)),
        _ => SearchCondition::Text(text),
    }
}

fn parse_date(value: &str) -> Option<DateTime<Utc>> {
    if let Ok(value) = DateTime::parse_from_rfc3339(value) {
        return Some(value.with_timezone(&Utc));
    }
    NaiveDate::parse_from_str(value, "%Y-%m-%d")
        .ok()?
        .and_hms_opt(0, 0, 0)
        .map(|value| value.and_utc())
}

fn normalize_handle(value: &str) -> String {
    value.trim_start_matches('@').to_ascii_lowercase()
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

pub fn append_sql<'args>(
    condition: &SearchCondition,
    builder: &mut QueryBuilder<'args, Postgres>,
    me: Option<(i64, &str)>,
) {
    match condition {
        SearchCondition::Text(value) => {
            builder
                .push("LOWER(p.body) LIKE ")
                .push_bind(format!("%{}%", escape_like(&value.to_lowercase())))
                .push(" ESCAPE '\\'");
        }
        SearchCondition::From(value) => {
            if value.eq_ignore_ascii_case("me") {
                builder
                    .push("a.id = ")
                    .push_bind(me.map(|(actor_id, _)| actor_id).unwrap_or(i64::MIN));
            } else {
                builder.push("(");
                append_actor_handle_sql(builder, value, "a");
                builder.push(")");
            }
        }
        SearchCondition::Mentions(value) => {
            let value = if value.eq_ignore_ascii_case("me") {
                me.map(|(_, username)| username)
                    .unwrap_or("__unauthenticated_me__")
            } else {
                value
            };
            builder
                .push("LOWER(p.body) LIKE ")
                .push_bind(format!("%@{}%", escape_like(value)))
                .push(" ESCAPE '\\'");
        }
        SearchCondition::Domain(value) => {
            builder
                .push("LOWER(p.body) LIKE ")
                .push_bind(format!("%{}%", escape_like(value)))
                .push(" ESCAPE '\\'");
        }
        SearchCondition::Since(value) => {
            builder.push("p.created_at >= ").push_bind(*value);
        }
        SearchCondition::Until(value) => {
            builder.push("p.created_at < ").push_bind(*value);
        }
        SearchCondition::Not(inner) => {
            builder.push("NOT (");
            append_sql(inner, builder, me);
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
                append_sql(value, builder, me);
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

fn append_actor_handle_sql<'args>(
    builder: &mut QueryBuilder<'args, Postgres>,
    value: &str,
    alias: &str,
) {
    let (username, domain) = value.split_once('@').unwrap_or((value, ""));
    builder
        .push("LOWER(")
        .push(alias)
        .push(".username) = ")
        .push_bind(username.to_string());
    if !domain.is_empty() {
        builder
            .push(" AND LOWER(")
            .push(alias)
            .push(".domain) = ")
            .push_bind(domain.to_string());
    }
}

fn escape_like(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_operators_quotes_and_boolean_groups() {
        assert_eq!(
            parse(r#"from:@alice.example ("exact phrase" OR #rust) -domain:spam.example lang:ja"#),
            SearchCondition::And(vec![
                SearchCondition::From("alice.example".into()),
                SearchCondition::Or(vec![
                    SearchCondition::Text("exact phrase".into()),
                    SearchCondition::Text("#rust".into()),
                ]),
                SearchCondition::Not(Box::new(SearchCondition::Domain("spam.example".into()))),
            ])
        );
    }

    #[test]
    fn balances_missing_parentheses() {
        assert_eq!(parse("(one OR two"), parse("one OR two)"));
    }

    #[test]
    fn ignores_lang_and_deduplicates_terms() {
        assert_eq!(
            parse("lang:ja hello hello"),
            SearchCondition::Text("hello".into())
        );
        assert_eq!(parse("hello OR lang:ja"), SearchCondition::True);
        assert_eq!(parse("-lang:ja"), SearchCondition::False);
    }

    #[test]
    fn parses_dates() {
        assert!(matches!(
            parse("since:2026-01-02"),
            SearchCondition::Since(_)
        ));
        assert!(matches!(
            parse("until:2026-01-02T03:04:05Z"),
            SearchCondition::Until(_)
        ));
    }
}

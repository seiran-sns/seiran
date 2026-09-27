//! Mastodon クライアントのリクエスト形式の受け口とページネーション。
//!
//! Mastodon 本家（Rails）は同じパラメータを JSON ボディ・フォーム（`x-www-form-urlencoded`・
//! `multipart/form-data`）・クエリ文字列のどれで送っても受け付け、配列は `media_ids[]=1&media_ids[]=2`、
//! 入れ子は `poll[options][]=a` と書く。クライアントごとに送り方がばらばらなので、どの形でも
//! いったん `serde_json::Value` に正規化してから型へデシリアライズする。フォーム由来の値は
//! 数値・真偽値も文字列になるため、そうしたフィールドは `lenient` の関数で受ける。

use axum::{
    async_trait,
    body::Bytes,
    extract::{FromRequest, FromRequestParts, Multipart, OriginalUri, Request},
    http::{header, request::Parts, HeaderValue},
    response::{IntoResponse, Response},
    Json,
};
use serde::{de::DeserializeOwned, Serialize};
use serde_json::{Map, Value};

use seiran_common::repository::Page;

use crate::error::ApiError;

/// ボディ上限（通常のフォーム・JSON。メディアは別エンドポイントで受ける）。
const MAX_BODY_BYTES: usize = 2 * 1024 * 1024;

/// `a[b][]` 形式のキーを `["a", "b", ""]` に分解する（`""` は配列への追加）。
fn split_key(key: &str) -> Vec<&str> {
    let Some(open) = key.find('[') else {
        return vec![key];
    };
    let mut parts = vec![&key[..open]];
    let mut rest = &key[open..];
    while let Some(stripped) = rest.strip_prefix('[') {
        let Some(close) = stripped.find(']') else {
            break;
        };
        parts.push(&stripped[..close]);
        rest = &stripped[close + 1..];
    }
    parts
}

fn insert_path(target: &mut Map<String, Value>, path: &[&str], value: String) {
    let Some((&head, rest)) = path.split_first() else {
        return;
    };
    match rest.first() {
        None => {
            target.insert(head.to_owned(), Value::String(value));
        }
        Some(&"") => {
            let entry = target
                .entry(head.to_owned())
                .or_insert_with(|| Value::Array(Vec::new()));
            if !entry.is_array() {
                *entry = Value::Array(Vec::new());
            }
            if let Value::Array(items) = entry {
                items.push(Value::String(value));
            }
        }
        Some(_) => {
            let entry = target
                .entry(head.to_owned())
                .or_insert_with(|| Value::Object(Map::new()));
            if !entry.is_object() {
                *entry = Value::Object(Map::new());
            }
            if let Value::Object(child) = entry {
                insert_path(child, rest, value);
            }
        }
    }
}

/// `(キー, 値)` 列（フォーム・クエリ・multipart のテキストフィールド）を JSON オブジェクトにする。
pub fn pairs_to_value<I, K, V>(pairs: I) -> Map<String, Value>
where
    I: IntoIterator<Item = (K, V)>,
    K: AsRef<str>,
    V: Into<String>,
{
    let mut map = Map::new();
    for (k, v) in pairs {
        insert_path(&mut map, &split_key(k.as_ref()), v.into());
    }
    map
}

fn query_to_value(query: Option<&str>) -> Map<String, Value> {
    pairs_to_value(
        url::form_urlencoded::parse(query.unwrap_or_default().as_bytes())
            .map(|(k, v)| (k.into_owned(), v.into_owned())),
    )
}

fn deserialize<T: DeserializeOwned>(map: Map<String, Value>) -> Result<T, ApiError> {
    serde_json::from_value(Value::Object(map))
        .map_err(|e| ApiError::BadRequest(format!("VALIDATION_FAILED: {e}")))
}

/// GET 等のクエリ文字列（`id[]=1&id[]=2` の配列を含む）。
pub struct MastodonQuery<T>(pub T);

#[async_trait]
impl<T, S> FromRequestParts<S> for MastodonQuery<T>
where
    T: DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        deserialize(query_to_value(parts.uri.query()))
            .map(MastodonQuery)
            .map_err(IntoResponse::into_response)
    }
}

/// multipart で送られたファイル1つ。
pub struct UploadedPart {
    pub bytes: Vec<u8>,
    pub file_name: Option<String>,
}

/// POST/PUT/PATCH のパラメータを正規化したもの。ボディ（JSON・フォーム・multipart の
/// テキストフィールド）にクエリ文字列を重ねる（同名キーはボディ優先）。multipart のファイル
/// フィールドは `files` に入る（`update_credentials` の `avatar`/`header`）。
pub struct MastodonForm {
    pub params: Map<String, Value>,
    pub files: std::collections::HashMap<String, UploadedPart>,
}

impl MastodonForm {
    pub fn deserialize<T: DeserializeOwned>(&self) -> Result<T, ApiError> {
        deserialize(self.params.clone())
    }
}

#[async_trait]
impl<S> FromRequest<S> for MastodonForm
where
    S: Send + Sync,
{
    type Rejection = Response;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        let mut params = query_to_value(req.uri().query());
        let mut files = std::collections::HashMap::new();
        let content_type = req
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_ascii_lowercase();

        let body_map = if content_type.starts_with("multipart/form-data") {
            let mut multipart = Multipart::from_request(req, state)
                .await
                .map_err(IntoResponse::into_response)?;
            let mut pairs = Vec::new();
            while let Some(field) = multipart
                .next_field()
                .await
                .map_err(|e| ApiError::BadRequest(e.to_string()).into_response())?
            {
                let Some(name) = field.name().map(str::to_owned) else {
                    continue;
                };
                if let Some(file_name) = field.file_name().map(str::to_owned) {
                    let bytes = field
                        .bytes()
                        .await
                        .map_err(|e| ApiError::BadRequest(e.to_string()).into_response())?;
                    files.insert(
                        name,
                        UploadedPart {
                            bytes: bytes.to_vec(),
                            file_name: Some(file_name).filter(|n| !n.is_empty()),
                        },
                    );
                    continue;
                }
                let text = field
                    .text()
                    .await
                    .map_err(|e| ApiError::BadRequest(e.to_string()).into_response())?;
                pairs.push((name, text));
            }
            pairs_to_value(pairs)
        } else {
            let bytes = axum::body::to_bytes(req.into_body(), MAX_BODY_BYTES)
                .await
                .map_err(|e| ApiError::BadRequest(e.to_string()).into_response())?;
            body_to_value(&content_type, &bytes).map_err(IntoResponse::into_response)?
        };
        params.extend(body_map);
        Ok(MastodonForm { params, files })
    }
}

/// POST/PUT のパラメータを `T` にしたもの（`MastodonForm` のファイルを使わない版）。
pub struct MastodonParams<T>(pub T);

#[async_trait]
impl<T, S> FromRequest<S> for MastodonParams<T>
where
    T: DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = Response;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        let form = MastodonForm::from_request(req, state).await?;
        form.deserialize()
            .map(MastodonParams)
            .map_err(IntoResponse::into_response)
    }
}

fn body_to_value(content_type: &str, bytes: &Bytes) -> Result<Map<String, Value>, ApiError> {
    if bytes.iter().all(u8::is_ascii_whitespace) {
        return Ok(Map::new());
    }
    if content_type.starts_with("application/json") {
        return match serde_json::from_slice::<Value>(bytes) {
            Ok(Value::Object(map)) => Ok(map),
            Ok(_) => Err(ApiError::BadRequest("VALIDATION_FAILED".to_owned())),
            Err(e) => Err(ApiError::BadRequest(format!("VALIDATION_FAILED: {e}"))),
        };
    }
    // Content-Type 無し・`x-www-form-urlencoded` はフォームとして読む（Mastodon 本家と同じ）。
    Ok(pairs_to_value(
        url::form_urlencoded::parse(bytes).map(|(k, v)| (k.into_owned(), v.into_owned())),
    ))
}

/// フォーム由来で文字列になった数値・真偽値も受け付けるデシリアライザ群。
pub mod lenient {
    use serde::{Deserialize, Deserializer};
    use serde_json::Value;

    pub fn opt_bool<'de, D: Deserializer<'de>>(d: D) -> Result<Option<bool>, D::Error> {
        Ok(match Option::<Value>::deserialize(d)? {
            Some(Value::Bool(b)) => Some(b),
            Some(Value::String(s)) => match s.as_str() {
                "true" | "1" | "on" => Some(true),
                "false" | "0" | "off" | "" => Some(false),
                _ => None,
            },
            Some(Value::Number(n)) => Some(n.as_i64() != Some(0)),
            _ => None,
        })
    }

    pub fn opt_i64<'de, D: Deserializer<'de>>(d: D) -> Result<Option<i64>, D::Error> {
        Ok(match Option::<Value>::deserialize(d)? {
            Some(Value::Number(n)) => n.as_i64(),
            Some(Value::String(s)) => s.trim().parse().ok(),
            _ => None,
        })
    }

    /// ID（Mastodon では文字列だが、JSON で数値を送るクライアントもある）。
    pub fn opt_id<'de, D: Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
        Ok(match Option::<Value>::deserialize(d)? {
            Some(Value::String(s)) if !s.is_empty() => Some(s),
            Some(Value::Number(n)) => Some(n.to_string()),
            _ => None,
        })
    }

    /// 単一値・配列のどちらでも受ける ID/文字列の列。
    pub fn vec_string<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<String>, D::Error> {
        let to_string = |v: Value| match v {
            Value::String(s) => Some(s),
            Value::Number(n) => Some(n.to_string()),
            _ => None,
        };
        Ok(match Option::<Value>::deserialize(d)? {
            Some(Value::Array(items)) => items.into_iter().filter_map(to_string).collect(),
            Some(v) => to_string(v).into_iter().collect(),
            None => Vec::new(),
        })
    }
}

/// Mastodon のページ指定（`max_id`/`since_id`/`min_id`/`limit`）。
#[derive(serde::Deserialize, Default, Debug)]
pub struct PageParams {
    #[serde(default, deserialize_with = "lenient::opt_id")]
    pub max_id: Option<String>,
    #[serde(default, deserialize_with = "lenient::opt_id")]
    pub since_id: Option<String>,
    /// `min_id` は本来「直後のページを古い側から」だが、seiran のリポジトリは新しい側から
    /// 取るため `since_id` と同じに扱う（差分が `limit` を超えると間が抜け、クライアントの
    /// 「さらに読み込む」で埋めることになる）。
    #[serde(default, deserialize_with = "lenient::opt_id")]
    pub min_id: Option<String>,
    #[serde(default, deserialize_with = "lenient::opt_i64")]
    pub limit: Option<i64>,
}

impl PageParams {
    /// `limit` は1〜`max_limit` に丸める。
    pub fn page(&self, default_limit: i64, max_limit: i64) -> Page {
        let parse = |id: &Option<String>| id.as_deref().and_then(|s| s.parse::<i64>().ok());
        Page {
            limit: self.limit.unwrap_or(default_limit).clamp(1, max_limit),
            until_id: parse(&self.max_id),
            since_id: parse(&self.since_id).or_else(|| parse(&self.min_id)),
        }
    }
}

/// ページ付き一覧の応答。`Link` ヘッダー（`rel="next"`＝古い側、`rel="prev"`＝新しい側）を
/// 付ける。多くのクライアントは本文の ID ではなくこのヘッダーで次ページを取る。
/// `newest`/`oldest` はページングに使うカーソル値（一覧の先頭と末尾。フォロー一覧では
/// アクターIDではなくフォロー行のID）。
pub fn paginated<T: Serialize>(
    uri: &OriginalUri,
    local_domain: &str,
    items: Vec<T>,
    cursors: Option<(i64, i64)>,
) -> Response {
    let mut resp = Json(items).into_response();
    if let Some((newest, oldest)) = cursors {
        let base_pairs: Vec<(String, String)> =
            url::form_urlencoded::parse(uri.query().unwrap_or_default().as_bytes())
                .filter(|(k, _)| !matches!(k.as_ref(), "max_id" | "since_id" | "min_id"))
                .map(|(k, v)| (k.into_owned(), v.into_owned()))
                .collect();
        let link_to = |key: &str, id: i64| {
            let mut serializer = url::form_urlencoded::Serializer::new(String::new());
            for (k, v) in &base_pairs {
                serializer.append_pair(k, v);
            }
            serializer.append_pair(key, &id.to_string());
            format!(
                "https://{}{}?{}",
                local_domain,
                uri.path(),
                serializer.finish()
            )
        };
        let link = format!(
            r#"<{}>; rel="next", <{}>; rel="prev""#,
            link_to("max_id", oldest),
            link_to("min_id", newest)
        );
        if let Ok(value) = HeaderValue::from_str(&link) {
            resp.headers_mut().insert(header::LINK, value);
        }
    }
    resp
}

/// 一覧の先頭・末尾の ID（数値化できる場合）から `paginated` のカーソルを作る。
pub fn id_cursors<'a>(mut ids: impl DoubleEndedIterator<Item = &'a str>) -> Option<(i64, i64)> {
    let newest = ids.next()?.parse().ok()?;
    let oldest = ids.next_back().map_or(Some(newest), |s| s.parse().ok())?;
    Some((newest, oldest))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn form_pairs_become_nested_json() {
        let map = pairs_to_value([
            ("status", "hi"),
            ("media_ids[]", "1"),
            ("media_ids[]", "2"),
            ("poll[options][]", "a"),
            ("poll[options][]", "b"),
            ("poll[expires_in]", "300"),
        ]);
        assert_eq!(
            Value::Object(map),
            serde_json::json!({
                "status": "hi",
                "media_ids": ["1", "2"],
                "poll": {"options": ["a", "b"], "expires_in": "300"},
            })
        );
    }

    #[test]
    fn page_uses_min_id_when_since_id_absent() {
        let p = PageParams {
            min_id: Some("5".into()),
            limit: Some(500),
            ..PageParams::default()
        }
        .page(20, 40);
        assert_eq!((p.limit, p.since_id, p.until_id), (40, Some(5), None));
    }

    #[test]
    fn lenient_bool_accepts_form_strings() {
        #[derive(serde::Deserialize)]
        struct B {
            #[serde(default, deserialize_with = "lenient::opt_bool")]
            v: Option<bool>,
        }
        let b: B = serde_json::from_value(serde_json::json!({"v": "true"})).unwrap();
        assert_eq!(b.v, Some(true));
        let b: B = serde_json::from_value(serde_json::json!({"v": false})).unwrap();
        assert_eq!(b.v, Some(false));
    }

    #[test]
    fn id_cursors_take_first_and_last() {
        assert_eq!(id_cursors(["9", "5", "3"].into_iter()), Some((9, 3)));
        assert_eq!(id_cursors(["9"].into_iter()), Some((9, 9)));
        assert_eq!(id_cursors(std::iter::empty()), None);
    }
}

//! 設定画面・管理画面の保存を「画面を開いた時点からの差分」として適用する。
//!
//! 同じ設定を別のタブ・ウィンドウ・デバイスでも開いていると、画面の値を丸ごと保存する
//! 方式では、後から保存した画面が、先に保存された変更を古い値で上書きしてしまう。そこで
//! 画面は項目ごとに「読み込んだ時点の値 `from` → 保存したい値 `to`」だけを送り、サーバーは
//! トランザクションの中で現在値を読んで（ロックして）差分を当て、当てられなければ保存しない。
//!
//! - 値（真偽・数値・選択肢・ID 等）: 現在値が `from` か `to` なら `to` にする。それ以外は
//!   別の画面が別の値にしたということなので衝突。`from` と `to` が同じなら現在値のまま。
//! - テキスト: `from` を共通祖先にした行単位の 3-way マージ。別の画面の変更と重ならなければ
//!   両方を取り込み、重なれば衝突。
//! - 集合（タグ・拒否サーバー等、順序と重複に意味の無い一覧）: `from` から `to` で足した要素を
//!   足し、除いた要素を除く。別の画面の変更とは衝突しようがない。
//! - オブジェクト（予約投稿の内容）: キーごとに値かテキストとして当てる。
//!
//! 衝突した保存は全体を取りやめ、`EDIT_CONFLICT`（409）を返す。画面は入力を残したまま
//! 「他の画面から変更されているようです」と知らせ、読み込み直してもらう。

use serde::Deserialize;

/// 1項目の編集。
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(untagged)]
pub enum Edit<T> {
    /// 画面に読み込んだ時点の値 `from` を `to` に変えた。
    Change { from: T, to: T },
    /// 編集前の値を伴わない指定。現在値にかかわらず上書きする。差分を送らない版の画面
    /// （無停止更新の間に古い版を開いたままのタブ）と、画面が現在値を持たない書き込み専用の
    /// 項目（パスワード・API キー）が使う。
    Set(T),
}

/// 省略された項目を既定値での上書きとして受ける（`#[serde(default)]`。差分を送らない版の
/// 画面が項目を省略して既定値を意味していたエンドポイントのため）。
impl<T: Default> Default for Edit<T> {
    fn default() -> Self {
        Edit::Set(T::default())
    }
}

/// 差分を現在値に当てられなかった。
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("edit conflicts with a concurrent change")]
pub struct EditConflict;

impl<T> Edit<T> {
    /// 保存したい値（検証用。差分が当たるかどうかは見ない）。
    pub fn target(&self) -> &T {
        match self {
            Edit::Change { to, .. } | Edit::Set(to) => to,
        }
    }

    /// 差分なら編集前の値、上書きなら `None`。1文の `UPDATE` で差分を当てるとき、
    /// `WHERE` に「現在値が編集前か編集後の値」を足すのに使う（`apply_value` と同じ規則）。
    pub fn expected(&self) -> Option<&T> {
        match self {
            Edit::Change { from, .. } => Some(from),
            Edit::Set(_) => None,
        }
    }

    /// 保存したい値だけを `f` で正規化する。`from` は保存済みの値と突き合わせるので変えない
    /// （正規化の規則ができる前に保存された値でも、画面が読んだとおりの値なら一致させるため）。
    pub fn map_target(self, f: impl FnOnce(T) -> T) -> Self {
        match self {
            Edit::Change { from, to } => Edit::Change { from, to: f(to) },
            Edit::Set(to) => Edit::Set(f(to)),
        }
    }
}

impl<T: PartialEq + Clone> Edit<T> {
    /// この画面では変えていない（`from` と `to` が同じ）差分か。1文の `UPDATE` で当てる
    /// ときは、これなら書き込まずに済ませる（`apply_value` と同じく現在値のまま）。
    pub fn is_unchanged(&self) -> bool {
        matches!(self, Edit::Change { from, to } if from == to)
    }

    /// 値の差分を当てる。現在値が `from`（誰も変えていない）か `to`（別の画面が同じ値に
    /// した）なら `to`、それ以外は衝突。`from` と `to` が同じ（この画面では変えていない）
    /// なら現在値のまま。
    pub fn apply_value(self, current: &T) -> Result<T, EditConflict> {
        match self {
            Edit::Change { from, to } if from == to => Ok(current.clone()),
            Edit::Change { from, to } => {
                if *current == from || *current == to {
                    Ok(to)
                } else {
                    Err(EditConflict)
                }
            }
            Edit::Set(to) => Ok(to),
        }
    }
}

impl Edit<String> {
    /// テキストの差分を当てる。`from` を共通祖先、現在値と `to` を両側にした行単位の 3-way
    /// マージで、両側の変更が重なったら衝突。
    pub fn apply_text(self, current: &str) -> Result<String, EditConflict> {
        match self {
            Edit::Change { from, to } => merge_text(&from, current, &to),
            Edit::Set(to) => Ok(to),
        }
    }
}

impl Edit<Option<String>> {
    /// `None` を空文字列と同じに扱ってテキストの差分を当てる（未設定の自己紹介等）。結果が
    /// 空なら `None`。
    pub fn apply_optional_text(
        self,
        current: Option<&str>,
    ) -> Result<Option<String>, EditConflict> {
        let edit = match self {
            Edit::Change { from, to } => Edit::Change {
                from: from.unwrap_or_default(),
                to: to.unwrap_or_default(),
            },
            Edit::Set(to) => Edit::Set(to.unwrap_or_default()),
        };
        let merged = edit.apply_text(current.unwrap_or_default())?;
        Ok((!merged.is_empty()).then_some(merged))
    }
}

impl<T: PartialEq + Clone> Edit<Vec<T>> {
    /// 集合の差分を当てる。`from` から `to` で除いた要素を現在値から除き、足した要素を
    /// （まだ無ければ）末尾に足す。衝突しない。
    pub fn apply_set(self, current: &[T]) -> Vec<T> {
        match self {
            Edit::Change { from, to } => {
                let mut merged: Vec<T> = current
                    .iter()
                    .filter(|v| to.contains(v) || !from.contains(v))
                    .cloned()
                    .collect();
                for v in to {
                    if !from.contains(&v) && !merged.contains(&v) {
                        merged.push(v);
                    }
                }
                merged
            }
            Edit::Set(to) => to,
        }
    }
}

/// JSON オブジェクトの差分をキーごとに当てる。`text_keys` のキーは文字列（`null` は空文字列と
/// 同じ）として 3-way マージし、それ以外は値として当てる。無いキーは `null` と同じに扱う。
pub fn merge_object(
    base: &serde_json::Map<String, serde_json::Value>,
    edited: &serde_json::Map<String, serde_json::Value>,
    current: &serde_json::Map<String, serde_json::Value>,
    text_keys: &[&str],
) -> Result<serde_json::Map<String, serde_json::Value>, EditConflict> {
    use serde_json::Value;
    let null = Value::Null;
    let mut keys: Vec<&String> = base
        .keys()
        .chain(edited.keys())
        .chain(current.keys())
        .collect();
    keys.sort();
    keys.dedup();
    let mut merged = serde_json::Map::new();
    for key in keys {
        let (b, e, c) = (
            base.get(key).unwrap_or(&null),
            edited.get(key).unwrap_or(&null),
            current.get(key).unwrap_or(&null),
        );
        let value = if b == e {
            c.clone()
        } else if c == b || c == e {
            e.clone()
        } else if text_keys.contains(&key.as_str()) {
            let text = |v: &Value| match v {
                Value::String(s) => Some(s.clone()),
                Value::Null => Some(String::new()),
                _ => None,
            };
            let (Some(b), Some(e), Some(c)) = (text(b), text(e), text(c)) else {
                return Err(EditConflict);
            };
            let m = merge_text(&b, &c, &e)?;
            if m.is_empty() {
                Value::Null
            } else {
                Value::String(m)
            }
        } else {
            return Err(EditConflict);
        };
        if !value.is_null() || current.contains_key(key) || edited.contains_key(key) {
            merged.insert(key.clone(), value);
        }
    }
    Ok(merged)
}

/// 行単位の 3-way マージ。
pub fn merge_text(base: &str, current: &str, edited: &str) -> Result<String, EditConflict> {
    if current == base || current == edited {
        return Ok(edited.to_string());
    }
    if edited == base {
        return Ok(current.to_string());
    }
    diffy::merge(base, edited, current).map_err(|_| EditConflict)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn change<T>(from: T, to: T) -> Edit<T> {
        Edit::Change { from, to }
    }

    #[test]
    fn value_applies_when_current_is_from_or_to() {
        assert_eq!(change(2, 1).apply_value(&2), Ok(1));
        assert_eq!(change(2, 1).apply_value(&1), Ok(1));
        assert_eq!(change(2, 1).apply_value(&3), Err(EditConflict));
    }

    #[test]
    fn unchanged_value_keeps_current() {
        assert_eq!(change(2, 2).apply_value(&3), Ok(3));
    }

    #[test]
    fn set_ignores_current() {
        assert_eq!(Edit::Set(1).apply_value(&3), Ok(1));
        assert_eq!(
            Edit::Set("b".to_string()).apply_text("a"),
            Ok("b".to_string())
        );
    }

    #[test]
    fn text_merges_non_overlapping_changes() {
        let base = "one\ntwo\nthree\nfour\n";
        let current = "ONE\ntwo\nthree\nfour\n";
        let edited = "one\ntwo\nthree\nFOUR\n";
        assert_eq!(
            change(base.to_string(), edited.to_string()).apply_text(current),
            Ok("ONE\ntwo\nthree\nFOUR\n".to_string())
        );
    }

    #[test]
    fn text_conflicts_on_overlapping_changes() {
        assert_eq!(
            change("a\nb\n".to_string(), "a\nB\n".to_string()).apply_text("a\nX\n"),
            Err(EditConflict)
        );
        // 1行だけのテキストは値と同じく、両側が変えていれば衝突
        assert_eq!(
            change("a".to_string(), "b".to_string()).apply_text("c"),
            Err(EditConflict)
        );
    }

    #[test]
    fn text_keeps_concurrent_change_when_not_edited_here() {
        assert_eq!(
            change("a".to_string(), "a".to_string()).apply_text("c"),
            Ok("c".to_string())
        );
    }

    #[test]
    fn optional_text_treats_none_as_empty() {
        assert_eq!(
            change(None, Some("hi".to_string())).apply_optional_text(None),
            Ok(Some("hi".to_string()))
        );
        assert_eq!(
            change(Some("hi".to_string()), Some(String::new())).apply_optional_text(Some("hi")),
            Ok(None)
        );
    }

    #[test]
    fn set_merges_additions_and_removals_without_conflict() {
        let edit = change(vec!["a", "b", "c"], vec!["a", "c", "d"]);
        // 別の画面が e を足し a を除いていても、両方の変更が残る
        assert_eq!(edit.apply_set(&["b", "c", "e"]), vec!["c", "e", "d"]);
    }

    #[test]
    fn object_merges_per_key() {
        let obj = |v: serde_json::Value| v.as_object().unwrap().clone();
        let base =
            obj(serde_json::json!({"text": "a\nx\nb\n", "visibility": "public", "cw": null}));
        let edited =
            obj(serde_json::json!({"text": "a\nx\nB\n", "visibility": "public", "cw": null}));
        let current =
            obj(serde_json::json!({"text": "A\nx\nb\n", "visibility": "home", "cw": null}));
        let merged = merge_object(&base, &edited, &current, &["text"]).unwrap();
        assert_eq!(merged["text"], "A\nx\nB\n");
        assert_eq!(merged["visibility"], "home");
        let edited = obj(serde_json::json!({"text": "a\nx\nb\n", "visibility": "followers"}));
        assert_eq!(
            merge_object(&base, &edited, &current, &["text"]),
            Err(EditConflict)
        );
    }

    #[test]
    fn deserializes_both_forms() {
        let e: Edit<String> = serde_json::from_str(r#"{"from":"a","to":"b"}"#).unwrap();
        assert_eq!(e, change("a".to_string(), "b".to_string()));
        let e: Edit<String> = serde_json::from_str(r#""b""#).unwrap();
        assert_eq!(e, Edit::Set("b".to_string()));
        let e: Edit<Option<String>> = serde_json::from_str(r#"{"from":null,"to":"1"}"#).unwrap();
        assert_eq!(e, change(None, Some("1".to_string())));
        let e: Edit<bool> = serde_json::from_str("true").unwrap();
        assert_eq!(e, Edit::Set(true));
    }
}

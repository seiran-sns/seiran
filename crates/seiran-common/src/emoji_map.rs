//! `actors.emoji_map` / `posts.emoji_map`（JSONB、`:shortcode:` → 画像URL）の API 応答向け変換。
//!
//! 表示名を返すアクター一覧系 API は、どれもこのマップを `emojis` として添えて返す。
//! フロントは表示名を共通コンポーネント（`DisplayName`）で描画し、ここで渡した
//! マップでショートコードを画像に展開する。マップを添え忘れると、その画面だけ
//! ショートコードが文字のまま出る。

use std::collections::HashMap;

/// JSONB のマップを `HashMap<String, String>` にする。`None`・オブジェクト以外・
/// 文字列以外の値は捨てる（壊れた値があっても一覧表示自体は失敗させない）。
pub fn to_string_map(v: Option<&serde_json::Value>) -> HashMap<String, String> {
    v.and_then(|v| v.as_object())
        .map(|obj| {
            obj.iter()
                .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_string_values_only() {
        let v = serde_json::json!({":a:": "https://x/a.png", ":b:": 1});
        let m = to_string_map(Some(&v));
        assert_eq!(m.len(), 1);
        assert_eq!(m[":a:"], "https://x/a.png");
    }

    #[test]
    fn none_and_non_object_are_empty() {
        assert!(to_string_map(None).is_empty());
        assert!(to_string_map(Some(&serde_json::json!([]))).is_empty());
    }
}

//! 「今後は表示しない」で非表示にできるガイド（確認ダイアログ等）のキー定義。
//!
//! ユーザーごとの非表示状態は `users.hidden_guides` に保存し、設定画面
//! 「おせっかいなガイドの非表示」で一覧・解除できる。フロントの表示名は
//! `account:guidesSettings.guides.<キー>` の翻訳キーで持つ。新しいガイドを足すときは
//! ここへ追加し、翻訳キーも追加する。

/// 非表示にできるガイドのキー。
pub const HIDEABLE_GUIDES: [&str; 1] = [
    // 「削除して編集再投稿」の確認ダイアログ。
    "redraft_note",
];

pub fn is_hideable_guide(key: &str) -> bool {
    HIDEABLE_GUIDES.contains(&key)
}

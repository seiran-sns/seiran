//! 共有作業ディレクトリ（`SEIRAN_WORK_DIR`、未設定なら `./work`）。
//!
//! API が受け取ったファイルを、別プロセスのワーカーが後から読むための置き場所
//! （絵文字インポートのZIP等）。役割を分けて複数プロセスで動かす構成では、すべての
//! プロセスが同じディレクトリを見られるようにすること（Docker ならボリュームを共有する）。
//! blue-green の切り替えをまたいで処理が続くこともあるので、新旧のプロセスでも共有する。

use std::path::PathBuf;

pub fn work_dir() -> PathBuf {
    std::env::var_os("SEIRAN_WORK_DIR")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("./work"))
}

/// 絵文字インポートのZIPの置き場所。
pub fn emoji_import_zip_path(request_id: i64) -> PathBuf {
    work_dir()
        .join("emoji-import")
        .join(format!("{request_id}.zip"))
}

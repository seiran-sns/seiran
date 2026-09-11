//! ワークスペースルートの`Cargo.toml`から`[workspace.package].version_suffix`を読み取り、
//! `SEIRAN_VERSION_SUFFIX`ビルド時環境変数として払い出す（`src/version.rs`参照）。
//! Cargoの`version.workspace = true`はこの独自キーを継承対象にしないため、自前で読む。

use std::path::Path;

fn main() {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR未設定");
    // crates/seiran-common から2階層上がワークスペースルート。
    let workspace_root = Path::new(&manifest_dir)
        .parent()
        .and_then(Path::parent)
        .expect("ワークスペースルートの解決に失敗");
    let workspace_toml_path = workspace_root.join("Cargo.toml");

    println!("cargo:rerun-if-changed={}", workspace_toml_path.display());

    let content = std::fs::read_to_string(&workspace_toml_path)
        .unwrap_or_else(|e| panic!("{}の読み込みに失敗: {}", workspace_toml_path.display(), e));
    // トップレベルドキュメントは`toml::Value`ではなく`toml::Table`としてパースする
    // （`Value::from_str`は単一のTOML値用で、複数セクションを持つドキュメント全体には
    // 使えない。「unexpected content, expected nothing」で失敗する）。
    let parsed: toml::Table = content
        .parse()
        .unwrap_or_else(|e| panic!("{}のパースに失敗: {}", workspace_toml_path.display(), e));

    let suffix = parsed
        .get("workspace")
        .and_then(|w| w.get("package"))
        .and_then(|p| p.get("version_suffix"))
        .and_then(|v| v.as_str())
        .unwrap_or("");

    println!("cargo:rustc-env=SEIRAN_VERSION_SUFFIX={}", suffix);
}

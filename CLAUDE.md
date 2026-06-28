# seiran プロジェクト固有ルール ＆ 環境前提

## 開発環境の前提条件
- **DB関連ツール**: システムに `docker compose`、`psql`、および `pg_config` がインストール済みであり、ローカル開発環境で利用可能です。

## 開発フロー・コミットルール

### 設計文書の同期（必須）
コードに変更を加えた場合、**必ず** 対応する設計文書を同期してからコミットすること。

- **アーキテクチャ・設計の変更** → `docs/02_architecture_and_overall_design.md` を更新
- **DBスキーマの変更** → `docs/01_database_schema_blueprint.md` を更新
- **プロトコル仕様の変更** → `docs/03_multi_protocol_engine_specification.md` を更新
- **新機能の完了** → `docs/06_development_roadmap.md` の該当チェックボックスに `[x]` を入れる

> **ルール**: 設計文書の更新とコードの変更は **同じコミット** に含めること。設計文書を後回しにしない。

### コミット前の確認フロー
1. コード実装・修正
2. `cargo build` でコンパイルエラーがないことを確認
3. 関連する設計文書を更新（上記の対応表に従う）
4. `docs/06_development_roadmap.md` の進捗チェックを更新
5. マイケルに画面・動作確認を依頼してから `main` ブランチへコミット

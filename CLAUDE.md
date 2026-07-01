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
2. 新しいマイグレーションファイルを追加した場合は `cargo sqlx migrate run` でローカル DB に適用する（後述）
3. `cargo build` でコンパイルエラーがないことを確認
4. `sqlx::query!` を追加・変更した場合は `cargo sqlx prepare --workspace` を実行して `.sqlx/` キャッシュを更新する（忘れると Docker ビルドが失敗する）
5. 関連する設計文書を更新（上記の対応表に従う）
6. `docs/06_development_roadmap.md` の進捗チェックを更新
7. マイケルに画面・動作確認を依頼してから `main` ブランチへコミット

### マイグレーションの適用方法（必読）

**`psql -f migration.sql` で直接流してはならない。** これをやると `_sqlx_migrations` テーブルに記録が残らず、API コンテナ起動時に「未適用」と判断されて再実行しようとして失敗する。

**必ず以下のコマンドを使う:**

```bash
cargo sqlx migrate run
```

このコマンドは:
- スキーマを DB に適用する
- `_sqlx_migrations` に記録を残す（API コンテナ起動時に「適用済み」と認識される）

`cargo sqlx prepare --workspace` は `.sqlx/` キャッシュを更新するだけで、マイグレーションは実行しない。

### sqlx オフラインキャッシュについて

`sqlx::query!` マクロはコンパイル時に DB に接続して SQL の型チェックを行う。Docker ビルド環境には DB がないため、`.sqlx/` キャッシュ（オフラインモード）を使う。

- **キャッシュ更新が必要なタイミング**: `sqlx::query!` / `sqlx::query_as!` / `sqlx::query_scalar!` 等を追加・変更したとき
- **更新コマンド**: `cargo sqlx prepare --workspace`（ローカル DB が起動している状態で実行）
- **`.sqlx/` ディレクトリは必ず git にコミットする**
- Dockerfile には `SQLX_OFFLINE=true` が設定済み

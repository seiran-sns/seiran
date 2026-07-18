# seiran

ActivityPub（Fediverse）と AT Protocol（Bluesky）という2つの異なる宇宙を、ユーザーにプロトコルの壁を意識させることなく1つの3ペインUIへ融和させる、マルチプロトコル分散型SNSサーバーです。

Fediverse インスタンスとしても、Bluesky の PDS（Personal Data Server）としても振る舞い、両プロトコルの投稿・フォロー・リアクションを1つの統一データモデルで扱います。詳しい設計思想は [`docs/concept.md`](docs/concept.md) を参照してください。

## 主な特徴

- **統一アクター/ポストモデル**: ローカルユーザー・他 seiran サーバー・Fedi・Bsky・各種ブリッジアカウントを1つのスキーマで表現
- **自前 PDS 実装**: ユーザーごとに `did:plc` を発行し、MST コミット・署名・Relay 配信を自前で行う
- **クロスプロトコル配送**: リプライ・リポスト・引用・リアクションをプロトコルの境界を越えて自動変換配送
- **重複排除**: ループバック・他 seiran サーバー間・ブリッジ経由の重複投稿をシナリオ別にマージ/リンク
- **3ペインUI**: 画面文脈に応じて右ペインが動的に変形するダイアログ駆動UI
- **Misskey API 互換レイヤー**: MiAuth 認証・Misskey 準拠エンドポイントにより既存の Misskey クライアントから利用可能

## 技術スタック

- **バックエンド**: Rust（axum, sqlx, tokio）。単一バイナリ `seiran-server` を `--role` で使い分けるモノリス/マイクロサービス両対応構成
- **データベース**: PostgreSQL
- **非同期ジョブキュー**: InMemory / Redis を切り替え可能
- **フロントエンド**: React + Vite + TypeScript
- **メディアストレージ**: S3互換オブジェクトストレージ

## ドキュメント

| ドキュメント | 内容 |
|---|---|
| [`docs/concept.md`](docs/concept.md) | サービスコンセプト・メンタルモデル |
| [`docs/architecture.md`](docs/architecture.md) | システムアーキテクチャ（crate構成・ロール分割・認証・ジョブキュー・ストレージ） |
| [`docs/database.md`](docs/database.md) | DBスキーマ設計 |
| [`docs/protocols.md`](docs/protocols.md) | ActivityPub / AT Protocol 実装仕様・クロスプロトコル配送・重複排除 |
| [`docs/ui_spec.md`](docs/ui_spec.md) | 3ペインUI仕様 |
| [`docs/roadmap.md`](docs/roadmap.md) | 開発ロードマップ |
| [`docs/coding_rules.md`](docs/coding_rules.md) | コーディングルール |

## 開発環境セットアップ

前提: Rust ツールチェイン、Node.js、Docker（`docker compose`）、`psql`/`pg_config`、`ffmpeg`/`ffprobe`（動画・音声添付の処理に使用）が利用可能なこと。

```bash
# 1. 環境変数ファイルを作成
cp .env.example .env
# 必要な値（LOCAL_DOMAIN, POSTGRES_* 等）を編集

# 2. DB を起動（単一コンテナ構成。DBだけ使う）
docker compose -f docker-compose.mono.yml up -d db

# 3. マイグレーション適用（psql -f では実行しないこと。理由は CLAUDE.md 参照）
cargo sqlx migrate run

# 4. バックエンド起動（role=all で全機能を1プロセスに統合）
cargo run -p seiran-server

# 5. フロントエンド起動（別ターミナル）
cd frontend
npm install
npm run dev
```

初回起動後、`GET /api/setup/status` の案内に従いフロントエンドから管理者アカウントを作成してください。

### 本番構成

- `docker-compose.mono.yml`: 単一コンテナで `seiran-server`（role=all）を起動する小規模構成
- `docker-compose.yml`: `api` / `federation-inbox` / `worker` / `atp-repo` をロールごとに分離し、Redis でジョブキューを共有するスケールアウト構成

いずれの構成を使うかは `docs/architecture.md` 3節を参照してください。

## ライセンス

[AGPL-3.0](LICENSE)

# 開発ロードマップ

進捗管理用のチェックリスト。完了済みフェーズは概要のみ、未完了項目は詳細に残す。
機能追加を完了したら該当箇所に `[x]` を入れ、コードの変更と同じコミットに含めること（`/home/yuba/seiran/CLAUDE.md` 参照）。

## 完了済みフェーズ（概要）

- [x] **フェーズ1: DBスキーマ ＆ 統一ID採番** — `posts`/`actors`/`follows` 等の統一エンティティ設計、Snowflake ID採番エンジン。詳細: `docs/database.md`
- [x] **フェーズ2: ローカル認証 ＆ MiAuth互換** — Argon2+JWT、MiAuth、メール確認・パスワードリセット、`secrets.toml` 自動生成。詳細: `docs/architecture.md` 4節
- [x] **フェーズ3: ジョブキュー ＆ 統合バイナリ化** — `JobQueue` trait、InMemory/Redis切替、`seiran-server` の `--role` 分割。詳細: `docs/architecture.md` 3・5節
- [x] **フェーズ4: マルチプロトコル通信エンジン** — AP/ATP双方向フェデレーション、クロスプロトコル配送（リポスト・引用・リプライ）、リアクション相互配送。詳細: `docs/protocols.md`
- [x] **フェーズ4.5: フロントエンドMVP** — React+Vite+TypeScript初期版、ローカル/ホームタイムライン、投稿・フォロー・プロフィールの基本画面
- [x] **フェーズ4.6: メディア・管理機能** — S3互換オブジェクトストレージ統合、画像/動画/音声アップロード、管理画面（ユーザー・絵文字・ストレージ設定）
- [x] **フェーズ5: 重複排除・マージエンジン** — ループバック/他seiran間/一般ブリッジの3シナリオ対応。詳細: `docs/protocols.md` 5節
- [x] **フェーズ6: 検索セッション管理** — `SearchSession`、ブレンドアルゴリズム（InMemory実装のみ）。詳細: `docs/architecture.md` 6節
- [x] **フェーズ7: 3ペインUI ＆ Misskey API互換** — 3ペインレイアウト、リアクション・通知・ピン留め・リスト機能、Misskey互換エンドポイント一式。詳細: `docs/ui_spec.md`, `docs/protocols.md` 6節

## 未完了・今後の課題

### プロトコル

- [ ] **ゼロトラストハンドシェイク（リモートseiranアクター専用検証）**
  - [ ] Bioの `seiran_signature: [ATP_DID]` パターン検出ロジック
  - [ ] 相手ドメインの `/.well-known/seiran/verify-actor` への検証リクエスト
  - [ ] 検証成功時の `actor_type = 'remote_seiran'` 昇格と `seiran_pair_actor_id` の相互紐付け
- [ ] **リモートseiran特権初期同期**
  - [ ] `/api/seiran/v1/posts/export` エンドポイント
  - [ ] 相手サーバーからの生データ一括インポート（最大300件）
- [ ] **他seiranサーバー間マージの ATP 経路対応** — `seiran_post_uuid` を Bsky レコード本体にも埋め込み、Jetstream経由で先に取り込まれた投稿ともマージできるようにする（`docs/protocols.md` 5節の既知の制約）
- [ ] **`actor_metadata_resolve` ジョブの実装** — 現状ハンドラはスタブ、enqueueする箇所も無い。`/verify-actor` ハンドシェイク検証・Webfinger解決・アバター等のキャッシュを実処理として実装する
- [ ] **`inbound_activity_process` のドメイン単位レート制限**
- [ ] **トレンド集計** — バックエンド未着手（フロントエンドはプレースホルダのみ表示）

### インフラ・パフォーマンス

- [ ] **`RedisSessionStore`** — 検索セッションのRedis保存（現状InMemoryのみ、スケールアウト時に必要）
- [ ] **Turnstile 自然人判別**（優先度: 低） — `TURNSTILE_SECRET_KEY` 設定時のみ有効化、登録/ログイン/パスワードリセットでの検証

### サードパーティクライアント互換

- [ ] **Misskeyストリーミングのチャンネル購読対応**（現状は認証ユーザー宛て一律ブロードキャストのみ）
- [ ] **フロントエンドのMisskeyスキーマへの追従改修**、検証済み旧カスタムエンドポイントの整理
- [ ] APIレスポンスの `bio` 末尾に本尊URLを自動挿入するフォールバック（ZonePane/Miria/Aria等の非Misskey互換画面向け）

### テスト・QA

- [ ] 重複排除（シナリオ2マージ処理）のユニットテスト
- [ ] 未来補正タイムスタンプ採番のテスト
- [ ] 検索ブレンドアルゴリズムの挙動テスト
- [ ] 連合（Federation）統合テスト（モックAP/ATPサーバー、他seiranハンドシェイク・特権同期のテスト）
- [ ] 高負荷・スケールアウト検証（`RedisJobQueue` + `RedisSessionStore` 環境での動作確認、プロダクションビルド・デプロイ手順の検証）

既存の結合テスト基盤: `crates/seiran-api/tests/`（実DB + 実 `seiran_api::router` を使用、`#[ignore]` で通常の `cargo test` から除外し `cargo test -p seiran-api --test <name> -- --ignored` で明示実行）。

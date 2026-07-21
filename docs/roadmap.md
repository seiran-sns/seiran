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
- [x] **フェーズ7.5: フロントエンド国際化 ＆ エラーメッセージ改善** — react-i18next導入（英語/日本語、ブラウザ言語設定への自動追従）、バックエンドエラーコード全種の日英メッセージ化、トースト通知、401時の自動ログアウト＋ログイン画面誘導。詳細: `docs/architecture.md` 8節
- [x] **フェーズ7.6: 本文中のリンク・メンションのクリック可能化** — Bsky facet（`#link`/`#mention`）・AP `<a href>` を内部リンクマーカー`[text](url)`としてMisskey API互換の`text`に埋め込み、フロント`RichText`コンポーネントでMarkdownリンク・生URL・`@mention`をクリック可能な要素へ変換。Bskyメンションはハンドル可変性に対応するため表示時に都度DID解決（`Job::ResolveBskyMention`による先行解決込み）。送信側（seiranユーザー投稿→Fedi/Bsky配送）もローカル/Bskyハンドル/Fediverse形式すべてのメンションでfacet・AP `tag[]`+アンカーを付与し、Bsky配信時は変換後テキストの文字数上限（300書記素/3000バイト）を投稿受理前に同期検証する。詳細: `docs/protocols.md` 6節
- [x] **フェーズ7.7: 投稿詳細・プロフィールページのOGP対応** — SPAの素のindex.htmlには`<meta>`が無いため、`/notes/:id`・`/@:handle`（AP Accept除く）は常にバックエンド（`seiran-api`）がSPAのindex.htmlを取得してOGP `<meta>` + Twitter Cardを注入して返す（bot判定は行わず未知のクローラーにも対応、投稿/アクター未発見時は`<meta>`無しでSPAをそのまま返す）。詳細: `docs/architecture.md` 8.1節
- [x] **フェーズ7.8: ハッシュタグ機能** — `hashtags`/`post_hashtags`/`pinned_hashtags` によるポスト⇔タグのm:n永続化。ローカル投稿・AP受信・Bsky受信いずれも最終的な `posts.body` から共通のスキャン（`seiran_common::hashtag::extract_hashtags`）で抽出するため、出自を問わず同じハッシュタイムライン（`/tags/:name`）に合流する。ハッシュタイムライン画面から「ホーム画面に追加」（`pinned_hashtags`、ホームのフィードタブ化）・「このハッシュタグでポスト」（`ComposerContext.openCompose` によるプリフィル投稿ダイアログ）。送信側（ローカル投稿→Bsky/AP配送）も `app.bsky.richtext.facet#tag`・AP `{"type":"Hashtag"}` タグ（自インスタンスの `/tags/:name` へのアンカー）を付与し、他クライアント上でも本物のハッシュタグとして認識される。受信側はMastodon等がハッシュタグアンカーにも`class="mention hashtag"`を付与する（メンションと`mention`トークンを共有する）ケースを`rel="tag"`で判別し誤ってメンション扱いしないようにする回帰修正込み。詳細: `docs/database.md`、`docs/protocols.md` 6節
- [x] **フェーズ7.9: ダイレクトメッセージ機能** — `visibility='direct'`投稿を`posts`にそのまま格納し宛先（`post_recipients`）・スレッド起点伝播コピー（`thread_root_post_id`）・既読状態（`dm_read_states`）で管理。Fedi宛先は宛先個人のみへのAP配送、Bsky宛先は`chat.bsky.convo`（自己署名サービス認証JWT、送信は`Job::BskyDmSend`、受信は`seiran-atp-repo::bsky_dm_poll`の定期ポーリング）。Bsky宛先は1対1のみ・文字数上限1000書記素・メディア添付不可。フロントエンドは`MessagesPage`（右ペイン=セッション一覧、中央ペイン=時刻順メッセージ履歴+送信フォーム）、`RecipientPicker`（宛先chip入力）、左ペイン未読バッジ。詳細: `docs/database.md`、`docs/protocols.md` 9節、`docs/ui_spec.md` 2.5節

## 未完了・今後の課題

### フロントエンド

- [ ] **言語切り替えUI** — 現状はブラウザの言語設定への自動追従のみ。ユーザーが手動で言語を選べるUIは未実装
- [ ] **ユーザー製翻訳ファイルの適用・配布機能** — ユーザーが独自の言語ファイル（`i18n/locales/{lng}/*.json` と同形式）を作成し、アプリに読み込ませて適用・配布できるようにする構想。現状の名前空間分割構成は `i18n.addResourceBundle()` によるこの拡張を見据えたもの

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
- [ ] **ユーザー設定に「Bsky DM受信許可」項目を追加** — 現状 `chat.bsky.actor.declaration` の `allowIncoming` は登録時・バックフィルとも `"all"` 固定でコミットする（`docs/protocols.md` 9節）。ユーザーが `"all"`/`"following"`/`"none"` を選べる設定画面UIとAPIを追加する

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
- [x] 検索ブレンドアルゴリズムの挙動テスト
- [ ] 連合（Federation）統合テスト（モックAP/ATPサーバー、他seiranハンドシェイク・特権同期のテスト）
- [ ] 高負荷・スケールアウト検証（`RedisJobQueue` + `RedisSessionStore` 環境での動作確認、プロダクションビルド・デプロイ手順の検証）
- [x] Playwright E2E基盤の構築（`e2e/`、スタブPLCサーバー、E2E専用DB）と新規登録フローの疎通テスト
- [x] E2Eテストの拡充（ログイン、投稿、フォロー、返信、リアクション、検索、プロフィール編集、ハッシュタグ）
- [x] Fedi配送のE2E化（投稿・返信・リポストがacceptedフォロワーのinboxへ正しいアクティビティで配送されることを、スタブFediアクター＋実HTTP Signaturesで検証）
- [ ] 通知UI実装後にE2E化（`NotificationsPage` は現状プレースホルダで未実装）
- [ ] Bsky側の配送E2E（ローカルPDSコミット自体は既存テストで間接的に検証済みだが、リモートBskyアクターからのフォロー受理を経由した配送は未検証）

既存の結合テスト基盤: `crates/seiran-api/tests/`（実DB + 実 `seiran_api::router` を使用、`#[ignore]` で通常の `cargo test` から除外し `cargo test -p seiran-api --test <name> -- --ignored` で明示実行）。

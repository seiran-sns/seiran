# 開発ロードマップ

機能を完了したら該当項目に `[x]` を入れ、コード変更と同じコミットに含める。完了項目は1行の概要と参照先だけを書き、仕様の詳細は設計文書に置く。

## 未完了

### フロントエンド

- [ ] **ユーザー製翻訳ファイルの適用・配布** — `i18n/locales/{lng}/*.json` と同形式のファイルを `i18n.addResourceBundle()` で読み込ませる構想（名前空間分割はこの拡張を見据えたもの）
- [ ] **プッシュ通知** — Service Worker・購読管理API・VAPID鍵・通知許諾UI

### プロトコル

- [ ] **投稿マージ時のアクター結婚（#237）** — 投稿の相互申告マージは投稿者が既に同一actor行のときだけ成立する。投稿者が未結婚だと孤立行のまま残る（`claimed_*` は保持されるので、アクター結婚が後で成立すれば再突合できる）
- [ ] **結婚成立後のATPフォロー追いコミット（#238）** — フォロー時点で未結婚だった非承認制のリモートseiranアクターは、後で結婚しても `follows.atp_rkey` が空のまま（AP側フォローは機能する）
- [ ] **リモートseiran特権初期同期** — `/api/seiran/v1/posts/export` と相手サーバーからの一括インポート（最大300件）
- [ ] **`inbound_activity_process` のドメイン単位レート制限**
- [ ] **トレンド集計** — バックエンド未着手（フロントエンドはプレースホルダのみ）
- [ ] **「Bsky DM受信許可」設定** — `chat.bsky.actor.declaration` の `allowIncoming` は現状 `"all"` 固定。`"all"`/`"following"`/`"none"` を選べるUIとAPI（`docs/protocols.md` 9節）
- [ ] **公開リストタイムラインのブロック/ミュートフィルタ** — `list.rs::timeline` は閲覧者情報を持たない設計のため、閲覧制御全体の見直しが必要（`docs/protocols.md` 10節）

### インフラ・パフォーマンス

- [ ] **`RedisSessionStore`** — 検索セッションのRedis保存（スケールアウト時に必要）
- [ ] **Turnstile 自然人判別**（優先度: 低）

### サードパーティクライアント互換

- [ ] フロントエンドのMisskeyスキーマへの追従、旧カスタムエンドポイントの整理
- [ ] `bio` 末尾への実ユーザーURL自動挿入（ZonePane/Miria/Aria等の非Misskey互換画面向けフォールバック）

### テスト・QA

- [ ] 重複排除（シナリオ2マージ処理）のユニットテスト
- [ ] 未来補正タイムスタンプ採番のテスト
- [ ] 連合統合テスト（モックAP/ATPサーバー、他seiranハンドシェイク・特権同期）
- [ ] 高負荷・スケールアウト検証（`RedisJobQueue` + `RedisSessionStore`、プロダクションビルド・デプロイ手順）
- [ ] Bsky受信メンション通知のE2E — firehose は本物のJetstreamに接続する設計で、E2Eにイベント注入用モックが無い

結合テスト: `crates/seiran-api/tests/*_integration.rs`（`#[ignore]`、実DBを使う。CI では E2E 用DBで実行）。

## 完了

### 基盤フェーズ

- [x] **フェーズ1: DBスキーマ・統一ID採番** — `docs/database.md`
- [x] **フェーズ2: ローカル認証・MiAuth互換** — `docs/architecture.md` 4節
- [x] **フェーズ3: ジョブキュー・統合バイナリ** — `docs/architecture.md` 3・5節
- [x] **フェーズ4: マルチプロトコル通信エンジン** — AP/ATP双方向連合、クロスプロトコル配送。`docs/protocols.md`
- [x] **フェーズ4.5: フロントエンドMVP**
- [x] **フェーズ4.6: メディア・管理機能** — S3互換ストレージ、画像/動画/音声、管理画面
- [x] **フェーズ5: 重複排除・マージエンジン** — `docs/protocols.md` 5節
- [x] **フェーズ6: 検索セッション管理** — `docs/architecture.md` 6節
- [x] **フェーズ7: 3ペインUI・Misskey API互換** — `docs/ui_spec.md`、`docs/protocols.md` 7節
- [x] **frontend/backend共通バージョン管理・互換性チェック** — `docs/architecture.md` 2.1節
- [x] **PWA対応** — サイト設定からmanifestを動的生成。`docs/architecture.md`
- [x] **国際化** — 日英中（繁/簡）韓西独仏、エラーコードの多言語化。`docs/architecture.md` 8節
- [x] **pg_bigm による本文検索** — `docs/database.md`
- [x] **コード改善大会（2026-09-26）** — DRY・レースコンディション・SQL・責務分割・SSRF。規約は `docs/coding_rules.md` 2節

### 認証・アカウント

- [x] **TOTP二段階認証・複数パスキー（#65）** — `docs/architecture.md` 4節
- [x] **メールアドレス変更（#59）** — `docs/database.md`
- [x] **アプリトークン（#60）** — `docs/database.md`
- [x] **認証・操作レート制限（#223）** — 資格情報種類数制限、IP自動ブロック、Turnstile、ロール別上限。`docs/architecture.md`
- [x] **退会済みアクターの扱い（#242）** — ログイン・表示・連合エンドポイントから除外。`docs/database.md`
- [x] **ユーザー凍結（ローカル・リモート共通）** — `docs/database.md`「`actors.suspended_at`」
- [x] **既存Bluesky DIDの転入・seiranからの転出** — `docs/account_migration.md`
- [x] **フォロー承認制（`actors.is_locked`）** — `docs/protocols.md`「フォロー承認制」
- [x] **プライバシー設定「Bskyのおすすめから除外」** — `docs/protocols.md` 3節
- [x] **ロール `emoji-editor`・管理画面のトピック別アクセス制御（#179）** — `docs/database.md`

### 投稿・表示

- [x] **本文のリンク・メンション・ハッシュタグ** — `docs/protocols.md` 6節
- [x] **ハッシュタグタイムライン・ホームへのピン留め** — `docs/database.md`
- [x] **Fedi投稿のHTML構造保持（#233）** — `posts.content_html`。`docs/protocols.md` 6節
- [x] **カスタム絵文字（本文・表示名・bio・CW・リアクション・Misskey互換API）** — `docs/protocols.md`、`docs/ui_spec.md`
- [x] **リモート絵文字のカタログ化・インポート（#73）** — `docs/ui_spec.md` 2.8節
- [x] **Unicode絵文字の twemoji 統一表示** — `docs/ui_spec.md`
- [x] **添付（複数ファイル・Bsky embed選択・ライトボックス）（#227, #64, #153）** — `docs/protocols.md` 3節、`docs/ui_spec.md` 2.2b節
- [x] **URLカード（OGP・oEmbed・x.com）** — `docs/protocols.md`、`docs/ui_spec.md`
- [x] **GIFアニメの自動再生統一** — `post_attachments.is_gif`
- [x] **CW（#229）・アンケート（#228）・リモートアンケートの生存監視** — `docs/protocols.md` 3節
- [x] **ポストの言語プロパティ** — `docs/protocols.md` 3節
- [x] **引用（#116, #134）・リポストのURLカード化（#132）** — `docs/protocols.md` 4節
- [x] **返信・引用・リポスト・リアクション件数表示** — `docs/database.md`
- [x] **縦に長い投稿の折りたたみ** — `docs/ui_spec.md`
- [x] **NoteCardのリモートサーバー表示** — `docs/ui_spec.md`
- [x] **未取り込み参照の表示とその場取り込み（#230-234）** — `docs/protocols.md` 1節
- [x] **brid.gy ブリッジポスト・ブリッジユーザー** — `docs/protocols.md` 5節
- [x] **「開く」（URL・ID・QR・OCR）（#165）** — `docs/architecture.md`
- [x] **プロフィール（bio HTML・key-value・バナー・別のアカウント・生年月日）** — `docs/ui_spec.md` 2.2節、`docs/protocols.md`
- [x] **未設定アバターの生成（#211）** — `docs/architecture.md`
- [x] **リモートメディアプロキシ（#87）** — `docs/architecture.md`
- [x] **OGP（投稿詳細・プロフィール）** — `docs/architecture.md` 8.1節

### タイムライン・検索

- [x] **ホーム/ローカル/ソーシャル/グローバル（#78）と公開範囲（#91, #105）** — `docs/database.md`、`docs/ui_spec.md`
- [x] **ホームTLのリプライ先フォロー条件** — `post_reply_target_followed`。`docs/database.md`
- [x] **タブ選択・スクロール位置の保持（#90）** — `docs/ui_spec.md` 2.4節・2.6節
- [x] **投稿検索（Bluesky AppView統合・検索式）（#146, #101）** — `docs/architecture.md` 6節
- [x] **用途別ユーザー検索** — `docs/architecture.md`

### ソーシャル機能

- [x] **フォロー・フォロー中/フォロワー一覧（リモート全件取得含む）（#56, #68）** — `docs/protocols.md` 2節
- [x] **フォローインポート** — `docs/architecture.md` 5節
- [x] **リスト** — `docs/database.md`
- [x] **ブロック・ミュート・リポストミュート** — `docs/protocols.md` 10節
- [x] **通知（リアクション・メンション・返信・リポスト・引用）と通知プレビュー** — `docs/protocols.md` 8節、`docs/ui_spec.md` 2.1節
- [x] **統一通報（#107）** — ActivityPub Flag / Bluesky Moderation Service転送
- [x] **ダイレクトメッセージ（リッチ表示・絵文字リアクション・Bsky DM）** — `docs/protocols.md` 9節、`docs/ui_spec.md` 2.5節
- [x] **ポスト詳細画面（タブ・返信ツリー・ログイン不要）（#226）** — `docs/ui_spec.md` 2.3節
- [x] **設定画面（#55）** — `docs/ui_spec.md` 2.7節

### 連合

- [x] **Authorized Fetch（署名付きGET）** — `docs/protocols.md`「署名付きGET」
- [x] **反応アクティビティの会話参加者への配送（#235）** — `docs/protocols.md` 2節
- [x] **AP Move 受信・alsoKnownAs** — `docs/protocols.md` 2節
- [x] **Fediverseリレー参加（#140）** — `docs/protocols.md`
- [x] **リモートseiranアクターの相互申告マージ（#236）** — `docs/protocols.md` 11節
- [x] **投稿のAP/ATPロスレス往復（`seiranPost`、#237）** — `docs/protocols.md` 5節
- [x] **リモートseiranへのフォローのATP同期（#238）** — `docs/protocols.md`
- [x] **Bskyリポストのタイムライン反映・リポスト/引用通知（#206）** — `docs/protocols.md` 8節
- [x] **ATP標準クライアントからの投稿・PDS各種XRPC（repo/sync/server/identity・プロキシ）** — `docs/protocols.md` 3・8節

### サードパーティクライアント互換

- [x] **Misskey互換API（Aria 等）** — `visibility` 語彙のマッピング、ストリーミングのチャンネル購読を含む。`docs/protocols.md` 7節

### テスト・QA

- [x] Playwright E2E 基盤と主要画面・連合配送の E2E
- [x] frontend ユニットテスト（vitest + jsdom）
- [x] CI での rustfmt・clippy・lint・ユニット・結合・E2E（失敗時 trace 保存）

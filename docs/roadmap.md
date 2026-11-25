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
- [ ] 高負荷・スケールアウト検証（`RedisJobQueue` + `RedisSessionStore`）
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
- [x] **メールの多言語化（#309）** — 送信するメールを表示言語の8言語で送る。`docs/architecture.md` 11節「メールの言語」
- [x] **pg_bigm による本文検索** — `docs/database.md`
- [x] **予約投稿（タイマー投稿）** — 投稿欄の🕒で予約日時を指定して予約し、「機能」＞「予約投稿」で編集・再予約・今すぐ投稿・削除する。予約日時から24時間を過ぎても投稿できなかった予約は「投稿できなかった予約」として残る。保持数はロール別レート制限で設定する。Mastodon 互換（`scheduled_at`・`scheduled_statuses`）・Misskey 互換（`notes/drafts/*`）にも対応。`docs/architecture.md` 11節・`docs/database.md`・`docs/ui_spec.md` 2.4b/2.7a節・`docs/protocols.md` 7/7.1節
- [x] **削除して編集再投稿** — 自分のポストを即削除し、内容を下書きに引き継いでコンポーザを開く。確認ガイドは `users.hidden_guides` で非表示にできる（設定画面 `/settings/guides`）。Misskey 互換は `notes/delete` → `notes/create`。`docs/ui_spec.md` 2.2b節・`docs/protocols.md` 7節
- [x] **コード改善大会（2026-09-26）** — DRY・レースコンディション・SQL・責務分割・SSRF。規約は `docs/coding_rules.md` 2節
- [x] **公開イメージと本番・単騎運用の無停止更新** — 単一イメージ（`SEIRAN_ROLE`）、Docker Hub・GHCR へのリリース、blue-green デプロイ、Redis によるプロセス間共有、常駐処理の担当決め、graceful shutdown。`docs/architecture.md` 3・3.1節

### 認証・アカウント

- [x] **TOTP二段階認証・複数パスキー（#65）** — `docs/architecture.md` 4節
- [x] **メールアドレス変更（#59）** — `docs/database.md`
- [x] **アプリトークン（#60）** — `docs/database.md`
- [x] **アプリパスワード（Bluesky クライアント向け）** — 設定画面での発行・無効化、無効化したパスワードのセッション失効。`docs/protocols.md` 3節
- [x] **認証・操作レート制限（#223）** — 資格情報種類数制限、IP自動ブロック、Turnstile、ロール別上限。`docs/architecture.md`
- [x] **新規登録の受付停止スイッチ・ユーザー数上限（PDSあたりDID数制限対策）** — `docs/architecture.md`
- [x] **退会済みアクターの扱い（#242）** — ログイン・表示・連合エンドポイントから除外。`docs/database.md`
- [x] **ユーザー凍結（ローカル・リモート共通）** — `docs/database.md`「`actors.suspended_at`」
- [x] **既存Bluesky DIDの転入・seiranからの転出** — `docs/account_migration.md`
- [x] **フォロー承認制（`actors.is_locked`）** — `docs/protocols.md`「フォロー承認制」
- [x] **プライバシー設定「Bskyのおすすめから除外」** — `docs/protocols.md` 3節
- [x] **ロール `emoji-editor`・管理画面のトピック別アクセス制御（#179）** — `docs/database.md`
- [x] **制限ロール `silence`（パブリック投稿不可）・`quarantine`（加えてフォロワー以外への働きかけ不可、新規フォロー上限24時間10人、ハッシュタグタイムラインに出ない）** — `docs/architecture.md`
- [x] **管理画面のロール別レート制限をマトリクス形式の独立タブへ** — `docs/ui_spec.md` 2.8節
- [x] **管理画面の SMTP 接続確認・テストメール送信** — `docs/ui_spec.md` 2.8節
- [x] **利用規約同意（新規登録・Bluesky転入共通、管理画面で文面設定）** — `docs/database.md`「利用規約同意」、`docs/ui_spec.md` 7節
- [x] **「このサイトについて」（運営者・管理者・問い合わせ先・利用規約、`/about`）** — `docs/ui_spec.md` 4.2節

### 投稿・表示

- [x] **本文のリンク・メンション・ハッシュタグ** — `docs/protocols.md` 6節
- [x] **ハッシュタグタイムライン・ホームへのピン留め** — `docs/database.md`
- [x] **Fedi投稿のHTML構造保持（#233）** — `posts.content_html`。`docs/protocols.md` 6節
- [x] **カスタム絵文字（本文・表示名・bio・CW・リアクション・Misskey互換API）** — `docs/protocols.md`、`docs/ui_spec.md`
- [x] **リモート絵文字のカタログ化・インポート（#73）** — `docs/ui_spec.md` 2.8節
- [x] **カスタム絵文字の Misskey 形式 ZIP インポート（非同期ジョブ・進捗表示・中止）/エクスポート** — `docs/architecture.md` 7節、`docs/ui_spec.md`
- [x] **Unicode絵文字の twemoji 統一表示** — `docs/ui_spec.md`
- [x] **添付（複数ファイル・Bsky embed選択・ライトボックス）（#227, #64, #153）** — `docs/protocols.md` 3節、`docs/ui_spec.md` 2.2b節
- [x] **URLカード（OGP・oEmbed・x.com）** — `docs/protocols.md`、`docs/ui_spec.md`
- [x] **GIFアニメの自動再生統一** — `post_attachments.is_gif`
- [x] **CW（#229）・アンケート（#228）・リモートアンケートの生存監視** — `docs/protocols.md` 3節
- [x] **ポストの言語プロパティ** — `docs/protocols.md` 3節
- [x] **引用（#116, #134）・リポストのURLカード化（#132）** — `docs/protocols.md` 4節
- [x] **Bskyポストを引用したFedi向け本文の末尾に引用元本文を `<blockquote>` で付ける（#344）** — `docs/protocols.md` 4節
- [x] **返信・引用・リポスト・リアクション件数表示** — `docs/database.md`
- [x] **リポストラッパー宛てのリアクションを元投稿のリアクションと合算表示** — `docs/ui_spec.md` 6.4節、`docs/protocols.md` 7節
- [x] **リポスト・リアクションの連打対策（送信元10秒クールダウン・受信側5分の通知抑制）とリポスト通知の元ポスト参照** — `docs/architecture.md` 4節、`docs/protocols.md` 8節
- [x] **NoteCardのクイックリアクション（ボタンバーの❤️＋よく使う絵文字のポップアップ。マウスはホバー中のカード・タッチは上端が見えているカードのうちいちばん上に出す）** — `docs/ui_spec.md` 2.2b節
- [x] **未読通知の表示（メニュー「通知」・スマホ下部🔔の件数バッジ、通知欄で未読だった通知を10秒黄色く目立たせる）** — `docs/ui_spec.md`「未読通知バッジ」
- [x] **ギガ節約モード（端末ごとに、投稿の添付の画像・動画をクリックするまで読み込まない。読み込む前は blurhash か無地）** — `docs/ui_spec.md`「ギガ節約モード」
- [x] **ワンタッチリアクションバー（クイックリアクションのポップアップ）の表示・非表示をアカウントごとに切り替える設定（設定画面「表示」）** — `docs/ui_spec.md` 2.2b節・`/settings/appearance`
- [x] **絵文字リアクションを使わず❤️だけで反応する設定（アカウントごと、設定画面「表示」）** — `docs/ui_spec.md` 2.2b節・`/settings/appearance`
- [x] **低コントラストモード（刺激の少ない配色とモノクロ線画のアイコン）の端末ごとの表示設定** — `docs/ui_spec.md` `/settings/appearance`
- [x] **「よく使う絵文字」の時間帯別ランキング（今の時間帯→隣の時間帯→全時間帯の順、閲覧者の地域の0時から4時間ごと）** — `docs/ui_spec.md`（絵文字リアクションピッカー）
- [x] **縦に長い投稿の折りたたみ** — `docs/ui_spec.md`
- [x] **NoteCardのリモートサーバー表示** — `docs/ui_spec.md`
- [x] **未取り込み参照の表示とその場取り込み（#230-234）** — `docs/protocols.md` 1節
- [x] **brid.gy ブリッジポスト・ブリッジユーザー** — `docs/protocols.md` 5節
- [x] **ブリッジポストのリアクション合算表示** — `docs/protocols.md` 5節
- [x] **「開く」（URL・ID・QR・OCR）（#165）** — `docs/architecture.md`
- [x] **プロフィール（bio HTML・key-value・バナー・別のアカウント・生年月日）** — `docs/ui_spec.md` 2.2節、`docs/protocols.md`
- [x] **未設定アバターの生成（#211）** — `docs/architecture.md`
- [x] **リモートメディアプロキシ（#87）** — `docs/architecture.md`
- [x] **OGP（投稿詳細・プロフィール・サイト全体、多言語）** — `docs/architecture.md` 8.1節

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
- [x] **拒否サーバー（弱いサーバーブロック。quarantine 相当の受信制限、Fedi・ATP PDS 対象）** — `docs/architecture.md`「拒否サーバー」

### サードパーティクライアント互換

- [x] **Misskey互換API（Aria 等）** — `visibility` 語彙のマッピング、ストリーミングのチャンネル購読を含む。`docs/protocols.md` 7節
- [x] **Misskey旧来 app 認証フロー（SocialHub Web 等）** — `app/create` → `auth/session/generate` → `auth/session/userkey`。`oauth_apps` を Mastodon 互換 OAuth と共用。`docs/protocols.md` 7節
- [x] **Mastodon互換API（Tusky・Ice Cubes・Elk 等）** — OAuth（PKCE 対応）、タイムライン・ハッシュタグ・検索・投稿/アカウント詳細の閲覧、投稿・返信・引用・お気に入り・リポスト・フォロー・メディア添付、ブロック/ミュート・プロフィール編集・投票・ピン留め・ブックマーク、ストリーミング（投稿の編集は Bluesky ポストとの整合のため対応しない）。`docs/protocols.md` 7.1節
- [x] **Fedibird 互換の絵文字リアクション（Scia 等）** — `fedibird_capabilities` の `emoji_reaction`、投稿の `emoji_reactions`、`PUT`/`DELETE statuses/:id/emoji_reactions/:name`、`emoji_reactioned_by`、`emoji_reaction` 通知とストリーミング。`docs/protocols.md` 7.1節

### テスト・QA

- [x] Playwright E2E 基盤と主要画面・連合配送の E2E
- [x] frontend ユニットテスト（vitest + jsdom）
- [x] CI での rustfmt・clippy・lint・ユニット・結合・E2E（失敗時 trace 保存）

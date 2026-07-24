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
- [x] **フェーズ7.10: ブロック・ミュート機能** — プロフィール画面に対ユーザー操作メニュー（`ActionsMenu`、フォロー中/フォロー・ミュート・ブロックを統合、フォローは独立ボタンも併設）を新設。ブロックはBsky準拠の定義（フォロー関係強制解除＋相互完全非表示）を採用し、相手がBskyなら`app.bsky.graph.block`コミット、相手がFediならAP `Block`配送。タイムライン・通知の相互非表示・プロフィール本文/key-valueの非表示はSQL関数`actor_is_hidden_for_viewer`と`is_blocked_by`判定に集約、フォロー・リプライ・リアクション・引用投稿・リポスト・DM送信の書き込みもAPIレベルで拒否する。相手発ブロック（Fedi/Bskyリモートユーザーが自分をブロックした場合）も検知して同じ制限を対称に働かせる：Fedi側はAP `Block`受信時に記録、Bsky側は`app.bsky.graph.block`の無絞り込みJetstream監視（`seiran-atp-repo::bsky_block_watch`）でリアルタイム検知する。ミュートはFedi/Bsky共通のローカル効果のみ（AP/ATP配送なし）。詳細: `docs/database.md`、`docs/protocols.md` 10節、`docs/ui_spec.md` 2.2節
- [x] **フェーズ7.11: メンション通知** — 本文中で`@username`形式によりローカルユーザーが言及された場合に通知（`type="mention"`）を作る。投稿の出自（ローカル投稿・Fedi受信・Bsky受信）ごとに解決経路を持ち、いずれも自己メンションは通知しない。通知一覧・クイック通知パネルへのリアルタイム反映は既存のフォロー/リアクション通知と同じ仕組み（`NotificationRepository`・`StreamHub`）に統合。詳細: `docs/protocols.md` 8節
- [x] **フェーズ7.12: リプライ通知 ＆ 通知パネルのリンク化** — 自分の投稿に返信が付いた場合に通知（`type="reply"`）を作る。投稿の出自（ローカル投稿・Fedi受信・Bsky受信）ごとに解決経路を持ち、リプライ先がローカルユーザーの投稿の場合のみ通知、自己リプライは通知しない。あわせてクイック通知パネル（`NotificationsPanel`）を全面的にリンク化: 通知者のユーザー名は種別によらず常にプロフィールページへのリンク、リプライ・リアクション・メンション通知は通知文全体が対象ポストへのリンクになる。詳細: `docs/protocols.md` 8節、`docs/ui_spec.md` 2.1節
- [x] **フェーズ7.13: カスタム絵文字リアクション** — ローカルユーザーがカスタム絵文字（`:shortcode:`）でリアクションできるようにした。バックエンドは`validate_reaction_content`をUnicode/カスタムの判別のみ行う純関数に整理し、`create_reaction`が`EmojiRepository::find_url_by_shortcode`でURL解決・実在確認する。AP配送はMisskey/Fedibird互換の`EmojiReact`＋`tag: [{type: Emoji, ...}]`まで対応（受信側の`build_emoji_map`と対称）、ATPはLike＋`emoji`拡張フィールドのベストエフォートのまま。フロントは`ReactionPicker`を刷新し、`Modal`内の`EmojiPickerPanel`（検索欄＋よく使う/絵文字/カスタムのタブ＋グリッド）に統合。Unicode絵文字データセット（`unicode-emoji-json`）は`React.lazy`で遅延ロード。「よく使う」は自分の現在のリアクションを`GET /api/reactions/frequent`で頻度集計した近似値。あわせて`POST /api/admin/emojis`の500エラー（`media_file_id`をJS `Number()`変換すると53bit精度の壁でsnowflake IDが破損し外部キー違反になっていた）を、リクエストボディを文字列で受けてサーバー側でparseする方式に修正。管理画面の絵文字一覧に画像プレビューも追加。絵文字ZIPインポート（`/api/admin/emojis/import`）はボディサイズ上限未設定で大きいZIPが`multipart/form-data`解析エラーになる不具合、およびアニメーションGIF/WebP/APNGが`process_image`で静止画WebPへ変換されてしまう不具合（`image`クレート0.25はアニメーション書き出し未対応のため、アニメーション画像はリサイズ・再エンコードせず元バイト列のまま保存する方式に修正）も解消。加えて`ReactionChips`の各チップにホバーすると、そのリアクションを付けたアクター一覧（アイコン＋名前）をポップオーバー表示する機能（`GET /api/notes/:id/reactions/:content/actors`）を追加。詳細: `docs/protocols.md`、`docs/database.md`、`docs/ui_spec.md` 2.2b節
- [x] **フェーズ7.14: 「リモートで表示」バナー** — ポスト詳細・プロフィール画面に、対象がローカルアクターでない場合の共通バナー（`RemoteBanner`）を追加。Fedi由来はAP URI、Bsky由来は`https://bsky.app/profile/{did}[/post/{rkey}]`へ別タブで遷移するリンクを表示する。`NoteResponse`に`remoteUrl`（`posts.ap_object_id`/`at_uri`から算出）を追加。詳細: `docs/ui_spec.md` 3.3節
- [x] **フェーズ7.15: プロフィール画面のフォロー中/フォロワー一覧（#56）** — プロフィール右ペインをタブシート化（【投稿】【フォロー中】【フォロワー】、`Tabs`コンポーネント）し、中央ペインのフォロー数・フォロワー数バッジをクリックすると対応タブへ切り替わる。バックエンドは`FollowRepository::list_following`/`list_followers`（`follows.id`によるカーソルページネーション、`actor_is_hidden_for_viewer`でブロック関係を除外）と`count_relations`を追加、`GET /api/users/following`・`/api/users/followers`・`ProfileResponse.following_count`/`follower_count`として公開。DB未登録のリモートアクター（`actor_id`を持たない）はフォロー一覧タブ自体を出さず従来通り投稿一覧のみ表示する。詳細: `docs/ui_spec.md` 2.2節
- [x] **フェーズ7.16: 設定画面（#55）** — メインメニューに「設定」を新設し、`/settings`（メニュー）・`/settings/account`（アカウント設定）・`/settings/mutes-blocks`（ミュート・ブロック管理）・`/settings/appearance`（表示設定）を追加。アカウント設定はメール/DID表示、現在のパスワード確認付きパスワード変更（`POST /api/account/change-password`、`LocalAuthProvider::verify_password`/`hash_password`を再利用）、退会（旧プロフィール編集画面から移動）を集約する。ミュート・ブロック管理は`MuteRepository::list_muted`/`BlockRepository::list_blocked`（新規追加、最大200件・カーソルページネーションなし）による対象者一覧＋解除ボタンをタブ切り替えで表示する。表示設定は言語（自動/日本語/英語）を`POST /api/account/language`で`users.language_preference`に保存し、`i18n.changeLanguage()`で即時反映する。アプリトークン（発行済みMiAuthトークンの一覧・無効化）は現状バックエンドがトークンをDB永続化していないため未実装、設定メニューに「近日公開」表示のみ残す（#60で切り出し済み）。詳細: `docs/ui_spec.md` 2.7節
- [x] **メールアドレス変更（#59）** — アカウント設定（`/settings/account`）に新アドレス入力フォームを追加。`email_changes`テーブル（`password_resets`と同型のワンタイムトークン方式、`user_id`紐付き）に変更リクエストを保存し新アドレス宛に確認メールを送信、`POST /api/account/email/confirm-change`でリンク踏み時点のトークン消費と`users.email`更新を行う（`/verify-email-change?token=...`がフロントの着地先）。既存の新規登録用`email_verifications`はuser_idを持たないため使い回さず専用テーブルとした。詳細: `docs/database.md`、`docs/ui_spec.md` 2.7節
- [x] **添付画像のライトボックス表示（#64）** — `NoteCard`の添付画像クリックを新規タブ遷移からページ内ライトボックス（`ImageLightbox`）表示に変更。バックエンドの変更なし（フロントエンドのみ）。詳細: `docs/ui_spec.md` 2.2b節
- [x] **リモートFediユーザーのフォロー中/フォロワー全件取得・表示（#68）** — プロフィール画面で、`follows`テーブル（seiranが認知している関係のみ）とは独立に、相手のAPアクタードキュメントの`following`/`followers`OrderedCollectionへ直接問い合わせて全件取得する。短タイムアウト（200ms）の同期取得を試み、失敗/タイムアウト時は`Job::RemoteFollowListSync`をバックグラウンドで積み`remote_follow_snapshots`テーブルへキャッシュ、次回リロードで反映される。未登録アクターは`Job::RemoteActorResolve`でプロフィールを解決する。フロントは`ProfilePage`でプロフィール取得直後にタブが開かれる前から先読みを開始し（`remoteFollowSummaryCache`）、「フォロー中/フォロワー」タブにローカルDB未把握の項目を見出しで分けず同じ見た目の1つのリストとして混ぜて表示（既知アクターはアバター等付き、未知はハンドル文字列のみ）。プロフィールカードのフォロー中/フォロワー人数もローカル・リモートをブレンドした実数（`total_count`）を表示する。詳細: `docs/protocols.md` 2節、`docs/database.md`

## 未完了・今後の課題

### フロントエンド

- [x] **言語切り替えUI** — 設定画面「表示」（`/settings/appearance`、#55）で自動/日本語/英語を選択可能。詳細は上記フェーズ7.16、`docs/ui_spec.md` 2.7節
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
- [ ] **リアクション一覧表示でのブロック/ミュート除外** — `fetch_reactions_map` は対象外（`docs/protocols.md` 10節）
- [ ] **公開リストタイムラインのブロック/ミュートフィルタリング** — `list.rs::timeline` は「閲覧者情報を持たない」設計のため未対応。対応するには閲覧制御全体の見直しが必要（`docs/protocols.md` 10節）

### インフラ・パフォーマンス

- [ ] **`RedisSessionStore`** — 検索セッションのRedis保存（現状InMemoryのみ、スケールアウト時に必要）
- [ ] **Turnstile 自然人判別**（優先度: 低） — `TURNSTILE_SECRET_KEY` 設定時のみ有効化、登録/ログイン/パスワードリセットでの検証

### サードパーティクライアント互換

- [ ] **Misskeyストリーミングのチャンネル購読対応**（現状は認証ユーザー宛て一律ブロードキャストのみ）
- [ ] **フロントエンドのMisskeyスキーマへの追従改修**、検証済み旧カスタムエンドポイントの整理
- [ ] APIレスポンスの `bio` 末尾に本尊URLを自動挿入するフォールバック（ZonePane/Miria/Aria等の非Misskey互換画面向け）
- [x] **`visibility` の値語彙をMisskey本家（`public`/`home`/`followers`/`specified`）にマッピング**。詳細: `docs/protocols.md` 7節

### テスト・QA

- [ ] 重複排除（シナリオ2マージ処理）のユニットテスト
- [ ] 未来補正タイムスタンプ採番のテスト
- [x] 検索ブレンドアルゴリズムの挙動テスト
- [ ] 連合（Federation）統合テスト（モックAP/ATPサーバー、他seiranハンドシェイク・特権同期のテスト）
- [ ] 高負荷・スケールアウト検証（`RedisJobQueue` + `RedisSessionStore` 環境での動作確認、プロダクションビルド・デプロイ手順の検証）
- [x] Playwright E2E基盤の構築（`e2e/`、スタブPLCサーバー、E2E専用DB）と新規登録フローの疎通テスト
- [x] E2Eテストの拡充（ログイン、投稿、フォロー、返信、リアクション、検索、プロフィール編集、ハッシュタグ）
- [x] Fedi配送のE2E化（投稿・返信・リポストがacceptedフォロワーのinboxへ正しいアクティビティで配送されることを、スタブFediアクター＋実HTTP Signaturesで検証）
- [x] フロントエンドのユニットテスト基盤（vitest + jsdom）を導入し、`lib/format.ts`・`lib/reaction.ts`・`lib/richTextPatterns.ts`・`api/client.ts`（`getErrorMessage`/`cursorParams`/`throwIfError`/`parseJsonBody`）・`NoteCard`/`PostComposer`内の純関数にテストを追加（`npm test`）
- [x] 管理画面（`/admin`）のE2E化（アクセス制御、サイト設定変更・永続化確認、ユーザー凍結/凍結解除）
- [x] リスト機能（`/settings/lists`）のE2E化（作成・改名・メンバー追加/削除・削除）
- [x] クイック通知（ホーム右ペイン`NotificationsPanel`）のE2E化（他ユーザーのリアクションがWS経由でリアルタイムに一覧へ反映されることを検証）。`NotificationsPage`（`/notifications`）は中央ペインに`NotificationsPanel`を表示する形で実装済み、専用ページへ直接遷移した場合の表示もE2Eで検証
- [x] ピン留め・リポスト取消のUI側状態変化のE2E化（ボタン表示のトグル確認）
- [x] Bsky側の配送E2E（リモートBskyアクターからのフォロー受理をポーリング方式（`getFollowers`、`seiran-atp-repo::bsky_follower_poll`）で検知し、投稿の`subscribeRepos`配送までを通しで検証）
- [x] メンション通知のE2E化（ローカル投稿・Fedi受信）。ローカルは`@username`投稿で相手に通知が届くこと・自己メンションで通知されないことを検証、Fedi受信はスタブFediアクターから`tag[].type=="Mention"`付きCreateを送りメンション通知が届くことを検証（`e2e/tests/notifications.spec.ts`）
- [ ] Bsky受信のメンション通知のE2E化 — `seiran-atp-repo::firehose`は本物のJetstreamサーバーへ接続する設計で、E2E側にイベント注入用のモックが無いため現状のE2E基盤では自動テストできない。実装（`save_bsky_post`内の通知処理）とcurlでの手動確認のみ
- [x] プロフィール画面のフォロー中/フォロワータブのE2E化（#56、`e2e/tests/follow.spec.ts`）。ユーザー間フォロー後、双方のプロフィールでフォロー数/フォロワー数バッジから右ペインのタブが切り替わり相手アクターが一覧表示されることを検証
- [x] 設定画面のE2E化（#55、`e2e/tests/settings.spec.ts`）。設定メニューからアカウント設定への遷移とDID表示、現在パスワード誤り時のエラー表示から正しいパスワードでの変更成功・新パスワードでのログインまでの一連、ミュート・ブロック一覧の表示とタブ切り替え・解除操作、表示設定での言語切り替え（英語選択→保存確認→自動に戻す→`/api/auth/me`の`language_preference`検証）を検証

既存の結合テスト基盤: `crates/seiran-api/tests/`（実DB + 実 `seiran_api::router` を使用、`#[ignore]` で通常の `cargo test` から除外し `cargo test -p seiran-api --test <name> -- --ignored` で明示実行）。

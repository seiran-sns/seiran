# マルチプロトコル実装

対象読者: ActivityPub / AT Protocol の実装やクロスプロトコル配送ロジックに触れる開発者。
「今、何が実装されていて、どう動くか」だけを書く。不具合修正の経緯や日付は書かない（`git log` 参照）。

## 1. フォロー時の初期同期

新規フォローが成立すると `Job::ActorHistorySync` が積まれ、相手の過去ログを非同期でバックフィルする（過去30日間 / 最大300件、ベストエフォート）。フォロー後のタイムライン表示は常にローカルDBからの読み取りのみで完結し、外部APIを都度叩かない（`docs/database.md` 4節、`docs/concept.md` 参照）。

- AT Protocol: 相手の DID から AppView の `getAuthorFeed` を叩いて取得。
- ActivityPub: 相手の Outbox（`GET /users/:username/outbox`）をページングして取得。

ノート詳細画面から前後投稿を見に行くオンデマンド同期も同じ仕組みを利用する。

## 2. ActivityPub (Fedi) 統合

### 構成
- `seiran-common::ap`: プロトコル非依存の共通ロジック
  - `client.rs` — `ApClient`（`reqwest::Client` + 公開鍵キャッシュ）。アクターフェッチ、HTTP Signatures 検証・署名、可視性判定（to/cc → 4値）、カスタム絵文字 tag 解析
  - `deliver.rs` — ローカル投稿のAP配送。`build_*`（純関数、アクティビティJSON組み立て）と `deliver_*`（DB取得+署名POSTのオーケストレーション）に分離
  - `outbox.rs` / `webfinger.rs` — 過去ログ同期・アウトバウンドWebFinger解決
- `seiran-federation-inbox::handlers`: HTTP層
  - `inbox.rs` — Inbox受信の入口。**署名検証のみ同期実行**し、実処理は `Job::InboundActivityProcess` としてキューに委譲（受信レイテンシを低く保つため）
  - `actor.rs` / `outbox.rs` / `webfinger.rs` / `nodeinfo.rs` / `featured.rs` / `lists.rs` — 公開エンドポイント

### Inbox で処理する Activity 種別
実処理は `seiran-common::jobs::inbound_activity_process`。

| type | 処理概要 |
|---|---|
| `Follow` | ローカルアクター実在確認 → リモートアクターupsert → `follows` に accepted 状態でINSERT（即時承認）→ 通知 → `Accept` を返送 |
| `Create`(Note) | リモートアクターupsert → HTML→プレーンテキスト変換 → 絵文字tag解析 → 可視性判定 → **重複排除**（3節参照）→ `posts` にINSERT → 添付URL保存 → フォロワーへWS配信 |
| `Accept`(Follow) | `follows.status` を `accepted` に更新、通知 |
| `Undo` | `object.type` で分岐: `Like`/`EmojiReact`→リアクション削除、`Announce`→リポスト論理削除、`Follow`→フォロー解除 |
| `Announce` | リポスト保存。元ポストが未登録なら `fetch_object` でリモート取得してから紐付け |
| `Like` \| `EmojiReact` | Misskey は絵文字リアクションも `type:"Like"` 固定で送るため、**wire type ではなく `content`/`_misskey_reaction` の有無**で判定する |

### 公開エンドポイント
`GET /users/:username`（Actor文書）、`GET /users/:username/outbox`（`?page=true`でOrderedCollectionPage）、`GET /.well-known/webfinger`、`GET /.well-known/nodeinfo` + `GET /nodeinfo/2.1`、featured（ピン留め）・lists（公開リスト）の各コレクション。

### HTTP Signatures 検証
1. `Digest` ヘッダー必須（SHA-256ボディハッシュと一致確認）
2. `Signature` の `headers=` に `digest` が含まれることを確認
3. `keyId` のアクターURIと `activity.actor` の一致確認
4. `keyId` から公開鍵PEM取得（キャッシュあり）してRSA-SHA256検証
5. 検証OK後、実処理はジョブキューへ委譲するのみ

### 配送
`Job::ApDelivery{actor_id, kind}`（優先度高、最大10回リトライの指数バックオフ）。宛先は `follows` の `status='accepted' AND actor_type='fedi'` の `ap_inbox_url` 一覧（リアクションは対象ポスト著者のinboxも追加）。全inboxへ署名付きPOSTをファンアウトし、**1件でも成功すればOk**（全滅時のみリトライ対象）。秘密鍵未設定時はリトライしても直らないため即座に破棄。

## 3. AT Protocol (Bsky) 統合

seiran は**自前 PDS を実装**しており、外部PDS（bsky.social等）は使わない。

### 構成
- `seiran-common::atp`
  - `repo.rs` — MST構築、TID生成(rkey)、P-256署名によるcommit生成、CARv1エンコード、各種レコード型のDAG-CBORエンコード、`subscribeRepos`フレーム構築(`#commit`/`#identity`/`#account`/`#error`)
  - `service.rs` — `AtpCommitService`。共通コミットパイプライン `commit_record_inner` + `commit_post`/`commit_repost`/`commit_like`/`commit_follow`/`commit_graph_list(item)`/各種delete/`commit_quote`/`commit_profile`
  - `plc.rs` — `did:plc` genesis operation生成・plc.directory登録
  - `did_resolve.rs` — サービス間認証JWT検証用のDID解決
  - `service_auth.rs` — 外部サービス呼び出し用の自己署名JWT(ES256、low-S正規化必須)
- `seiran-atp-repo::firehose` — Jetstream WebSocketクライアント本体
- `seiran-api::handlers::xrpc::{repo,server,sync}` — `getBlob`/`getRepo`/`subscribeRepos`/`describeServer`/`resolveHandle` 等

ローカルユーザーの投稿は `AtpCommitService` が**ジョブキューを介さず直接** MSTコミット・署名し、`atp_repo_events` にイベント記録、公式Relay（`bsky.network`）へ `requestCrawl` を送って購読される。

### DID解決・PLC登録・ハンドル検証（アカウント登録時）
1. ローカルでP-256鍵生成、`did:plc:xxx` をローカル計算のみで確定
2. Cloudflare API で `_atproto.{username}.{domain}` TXTレコードをセット（ハンドル検証用）
3. `plc.directory` へ genesis operation をPOST
4. 失敗時は新しい鍵で再生成し最大3回リトライ

`com.atproto.identity.resolveHandle` は `{username}.{local_domain}` 形式のみ対応し、DBの `actors.at_did` から即答する（自PDS管轄のみ、外部問い合わせ不要）。

### MSTコミット・subscribeRepos（`commit_record_inner` が共通パイプライン）
1. アクターの `at_repo_cid`/`at_repo_rev`/`at_repo_data_cid` と署名鍵PEMをDBから取得
2. 既存の全レコード（`posts` + `atp_records`）をロードし、新規レコード追加でMST再構築
3. 新しいrev(TID)でcommit生成、P-256署名（low-S正規化必須）
4. 差分CARをエンコード
5. トランザクション内で `atp_blocks`/`actors`/`atp_records`/`posts`(該当時)/`atp_repo_events` を更新
6. `subscribeRepos` 用フレームを生成しzstd圧縮して `atp_repo_events.frame_bytes` に保存、commit後にWebSocket配信（複数レプリカ時はRedis Pub/Subブリッジ経由）

`GET /xrpc/com.atproto.sync.subscribeRepos` は `cursor` 指定時に `atp_repo_events` から未送信分を送信し、以降はリアルタイムbroadcastを購読する。

> `atp_repository_publish` ジョブ（外部PDSへのミラーリング用に定義されている）は enqueue する呼び出し箇所が存在せず、実質デッドコードになっている。

### Jetstream 経由の取り込み（`seiran-atp-repo::firehose`）
`wss://jetstream1.us-east.bsky.network/subscribe?wantedCollections=app.bsky.feed.post&wantedCollections=app.bsky.feed.like` に接続。

- **wantedDids絞り込み**: ローカルユーザーがフォロー中、またはいずれかのリストのメンバーであるBsky DIDの集合を30秒間隔でポーリングし変化があれば再接続。無関係な投稿・Likeの際限ない取り込みを防ぐための必須の絞り込み。
- **リーダー選出**: 複数プロセス起動時の重複接続を避けるため、Redisベースの `JetstreamLeaderElector` でリース制御。モノリスモードはRedis無しでも常時接続、split-role構成はRedis障害時にフェイルクローズ。
- **cursor永続化**: 直近処理イベントの `time_us` を `site_settings`（汎用KV）に5秒間隔で保存し、再接続時に引き継ぐ（プロセス停止中のイベント取りこぼし防止）。
- 保存対象は wantedDids に含まれるDIDのみ。投稿は同梱の `record.text`/`record.createdAt` をそのまま使う（AppView再取得不要）。`app.bsky.embed.images`/`video`/`recordWithMedia` を解析しCDN URLを組み立てて添付保存。
- Like（`app.bsky.feed.like`）は create/delete で `reactions` へINSERT/DELETE、通知・リアルタイム配信。

### uploadBlob / getBlob・動画パイプライン
`getBlob` はCIDのmultihashからsha256を逆算し `media_files`/`atp_blobs` を検索してCDN URLへリダイレクトする（ストレージ本体を自前で再配信しない）。

動画・音声は原本をそのまま保存（トランスコードなし）、ffmpegでメタデータとサムネイルのみ抽出。`deliver_to_bsky=true` の場合、Bsky公式動画パイプライン（`app.bsky.video.uploadVideo`）へ提出する。**音声ファイルはBskyに専用embedが無いため、グレー背景の静止画+音声トラックのmp4に変換**してから動画として提出する。提出は非同期で `Job::BskyVideoPoll` が完了をポーリングし、間に合わなければ `app.bsky.embed.external`（URLカード）にフォールバックする。動画添付投稿は結合未確定の間 `Job::BskyPostCommitDeferred` でBskyコミット自体を遅延させ、早すぎるコミットによるexternal固定化を防ぐ。

## 4. クロスプロトコル配送ルール

中核ロジックは `seiran-api::handlers::notes::delivery`。`classify_post` が元ポストの出自を判定する: `actors.domain == local_domain` ならローカル、それ以外は `(ap_object_id有無, at_uri有無)` から `FediRemote`/`BskyRemote`/`LocalOrSeiran`（両方あり＝他seiranサーバー）に分類する。

- **リプライ**: 元ポストが Fedi リモートのみなら Bsky 配信しない。Bsky リモートのみなら Fedi 配信しない。親の可視性が `followers_only` ならリプライも継承する。
- **引用**: 元ポストが Fedi リモートのみの場合、Bsky側は `app.bsky.embed.external`（URLカード。at_uri/cidがあれば `app.bsky.embed.record`）。AP側は `ap_object_id` があればそれをquoteUrlに、無ければ bsky.app URL に変換。
- **リポスト**: 元ポストが `ap_object_id` を持つなら Fedi へは `Announce`。持たず `at_uri` のみ(Bskyリモート)ならテキスト投稿（「🔁 author: bsky.app URL」）にフォールバック。Bsky側も同様の非対称フォールバックがある。`visibility` が `followers_only`/`direct` の場合、フォロワー限定配信を持たない Bsky へのリポストはスキップする。

## 5. 重複排除・マージ（水際防御）

同じ内容の投稿が複数ルートで自サーバーに届くケースへの対処。3シナリオ:

1. **ループバック**（自サーバー投稿の逆輸入）: 受信Noteの `id`/`url` が `https://{local_domain}/notes/{id}` パターンに一致すれば `parent_original_post_id` にセットしてINSERT（重複許容 + リンク）。
2. **他seiranサーバー間マージ**: 送信側は投稿作成時に `seiran_post_uuid`（UUID v4）を生成しAP Noteに `seiranUuid` として埋め込む。受信側は `find_by_seiran_uuid` で既存行を検索し、あれば新規INSERTせず `ap_object_id` をUPDATEするのみ。
   - **既知の制約**: `seiran_post_uuid` は ATP 側（Bskyレコード本体）には埋め込まれていない。そのため Jetstream 経由で先に取り込まれた投稿に後から AP の `Create` が届いても `find_by_seiran_uuid` は一致せず、**別行として新規INSERTされる**（マージされない）。現状「AP側が先」の場合のみ機能する。
3. **一般ブリッジ重複**: Noteの `url` が `https://bsky.app/profile/{did}/post/{rkey}` 形式なら `at://` URIへ変換し既存ポストを検索、あれば `parent_original_post_id` にリンク（重複許容 + リンク）。

## 6. Misskey API 互換レイヤー

`middleware::misskey_auth_bridge` が `Authorization` ヘッダー未指定時にJSONボディ/クエリの `i` を検出し `Authorization: Bearer` を合成する。`handlers::misskey`（`endpoints.rs`/`convert.rs`/`types.rs`）が Misskey ワイヤー形式のエンドポイントを提供する。

対応済み: `POST /api/meta`（サーバー検出）、MiAuthフロー、`POST /api/i`、`/api/users/show`、`/api/notes/show`、`/api/notes/local-timeline`・`timeline`、`/api/notes/reactions/create`・`delete`、`/api/notes/unrenote`、`/api/following/create`・`delete`、`/api/i/notifications`（DB永続化、`untilId`/`sinceId`カーソル）、`GET /api/emojis`（未認証公開）。

書き込み系は既存の `handlers::notes`/`handlers::follows` をそのまま呼び出し、レスポンスだけMisskey形状に整形する。

**既知の非互換点**: `visibility` は常に `"public"` 固定、`cw` は常に `null`、書き込み系のエラー形状はMisskey本家のエラーID体系を再現していない、本文中カスタム絵文字インライン用 `emojis` マップは常に空。ストリーミングはMisskeyのチャンネル購読方式ではなく単純な認証ユーザー宛てブロードキャストのみ。

## 7. 通知・リアルタイム配信

`seiran-common::streaming::StreamHub`（プロセス内 `tokio::broadcast`、容量512）が `{"type":kind,"body":body}` を配信する。`GET /api/streaming?token=<JWT>` でWebSocket接続し、`recipients` に自分の actor_id が含まれるイベントのみ転送される。

`notifications` テーブルへの書き込みは、ローカルリアクション作成・AP/ATP inbound（Follow/Accept/Reaction）の各経路から行われる。種別は `Follow`/`Reaction`/`FollowRequestAccepted` の3種のみ。WebSocketは「新着があった」というシグナル配信のみに用い、実データは常に `POST /api/i/notifications`（REST、`sinceId`付き）から再取得する（一覧表示とスキーマを統一するため）。

## 8. 未実装・スコープ外の機能

- **ゼロトラストハンドシェイク**（他seiranサーバー間の `/verify-actor` 検証、`remote_seiran` への昇格）: 未実装。`actors.seiran_pair_actor_id` はスキーマ上・読み取りコードは存在するが書き込みロジックが無い（常にNULL）。
- **`actor_metadata_resolve` ジョブ**: ハンドラはdispatchに登録されているが中身はスタブ（即座に `Ok(())`）。enqueueする呼び出し箇所がプロダクションコードに存在しない。
- **トレンド集計**: 完全に未着手（テーブル・エンドポイントとも存在しない）。
- **Misskey互換ストリーミング（チャンネル購読方式）**: 未着手、現状はブロードキャストのみ。
- **ドメイン単位のレート制限**（`inbound_activity_process` 向け）: 未実装。現状 `actor_history_sync` キューのみドメイン単位の同時実行制限を持つ。
- **リモートFedi/Bskyユーザー自身の公開リストのオンデマンド取得**: 未実装（`public_lists` はローカルユーザーのみ対象）。

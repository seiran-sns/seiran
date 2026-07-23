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
| `Follow` | ローカルアクター実在確認 → **ブロック済みチェック**（こちらが送信者をブロック中ならAcceptを送らずサイレントに無視）→ リモートアクターupsert → `follows` に accepted 状態でINSERT（即時承認）→ 通知 → `Accept` を返送 |
| `Create`(Note) | リモートアクターupsert → HTML→内部リンクマーカー付きプレーンテキスト変換（6節参照）→ 絵文字tag解析 → 可視性判定 → **重複排除**（3節参照）→ `posts` にINSERT → 添付URL保存 → フォロワーへWS配信 |
| `Accept`(Follow) | `follows.status` を `accepted` に更新、通知 |
| `Block` | リモートアクターupsert → ブロックされた側がブロックした側をフォローしていた関係があれば解消（`blocks` テーブルには書き込まない、通知も生成しない。11節参照） |
| `Undo` | `object.type` で分岐: `Like`/`EmojiReact`→リアクション削除、`Announce`→リポスト論理削除、`Follow`→フォロー解除、`Block`→ログのみ（DB上の巻き戻し対象なし） |
| `Delete` | `object`（文字列URIまたは`{"type":"Tombstone","id":...}`）の`ap_object_id`に一致する投稿を論理削除。**送信元アクター（`activity.actor`、HTTP Signature検証済み）が投稿者本人と一致する場合のみ**削除する（なりすまし対策）。一致する投稿が無い場合（アクター自身のDelete等）は無視。リモートアクター自体の退会（`Delete(Actor)`）は未対応 |
| `Announce` | リポスト保存。元ポストが未登録なら `fetch_object` でリモート取得してから紐付け |
| `Like` \| `EmojiReact` | Misskey は絵文字リアクションも `type:"Like"` 固定で送るため、**wire type ではなく `content`/`_misskey_reaction` の有無**で判定する |

### 公開エンドポイント
`GET /users/:username`（Actor文書）、`GET /users/:username/outbox`（`?page=true`でOrderedCollectionPage）、`GET /.well-known/webfinger`、`GET /.well-known/nodeinfo` + `GET /nodeinfo/2.1`、featured（ピン留め）・lists（公開リスト）の各コレクション。

### HTTP Signatures 検証
1. `Digest` ヘッダー必須（SHA-256ボディハッシュと一致確認）
2. `Signature` の `headers=` に `digest` が含まれることを確認
3. `keyId` のアクターURIと `activity.actor` の一致確認
4. `keyId` から公開鍵PEM取得（TTL付きキャッシュ、既定1時間）してRSA-SHA256検証。キャッシュ済み鍵での検証に失敗した場合はキャッシュを無視して1回だけ再フェッチし再検証する（リモートの鍵ローテーション対応）
5. 検証OK後、実処理はジョブキューへ委譲するのみ

### 配送
`Job::ApDelivery{actor_id, kind}`（優先度高、最大10回リトライの指数バックオフ）。宛先は `follows` の `status='accepted' AND actor_type='fedi'` の `ap_inbox_url` 一覧（リアクションは対象ポスト著者のinboxも追加）。全inboxへ署名付きPOSTをファンアウトし、**1件でも成功すればOk**（全滅時のみリトライ対象）。秘密鍵未設定時はリトライしても直らないため即座に破棄。

通常投稿（`PostToFollowers`、DM以外）は、上記フォロワーに加え**本文中でメンションした相手のinbox**もフォロー関係と無関係に配送先へ加える（`crates/seiran-common/src/ap/deliver.rs::fetch_inboxes_by_ap_uris`）。メンション先が既知（DB上に`actor_type='fedi'`の行がある）ならDBから、未知ならその場でアクタードキュメントを取得してinboxを解決する（DBへの保存は伴わない）。`to`にもメンション先のactor URIを含める（Mastodon等と同様の作法）。メンション先の取得に失敗した場合はそのメンション先だけをスキップし、他の配送は妨げない。

### カスタム絵文字リアクションの送信（`EmojiReact`）
ローカルユーザーがカスタム絵文字（`:shortcode:`）でリアクションすると、`build_reaction_object`（`ap/deliver.rs`）が Misskey/Fedibird 互換の `tag: [{"type":"Emoji","name":":shortcode:","icon":{"type":"Image","url":...}}]` を付与した `EmojiReact` を組み立てる。`content`/`_misskey_reaction` には `:shortcode:` 形式の文字列をそのまま載せる。受信側の `build_emoji_map`/`extract_emoji_tag_url`（`ap/client.rs`・`jobs/inbound_activity_process.rs`）と対称的なペアになっている。画像URLの解決は `EmojiRepository::find_url_by_shortcode`（`custom_emojis`/`media_files`/`storage_providers` を JOIN）で行い、未登録shortcodeは `INVALID_REACTION_CONTENT`/`UNKNOWN_EMOJI` として拒否する（`handlers/notes/validation.rs`・`handlers/notes/mod.rs::create_reaction`）。ATP（Bsky）はカスタム絵文字非対応のため、`commit_like` の `emoji` 拡張フィールドに `:shortcode:` 文字列をベストエフォートで載せるのみ（画像は送らない）。

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

**ATPハンドルは常に小文字**（`seiran_common::username::to_atp_username`）。`actors.username` 自体は表示上大文字を許可するが、PLC genesis の `alsoKnownAs`・Cloudflare TXTレコード・`resolveHandle`/`.well-known/atproto-did` の応答・`#identity` ブロードキャストは全てこの小文字化した値を使う。DNS/HTTPホスト名は経路上（プロキシ・リゾルバ・Bluesky側の正規化）で小文字化されるため、大文字混じりのハンドルを一度でも `alsoKnownAs` に載せると恒久的に解決不能（bsky.app上で`handle.invalid`）になる実障害が過去にあった。`ActorRepository::find_by_username_domain`/`find_did_by_username_domain` は `LOWER()` 比較で大文字小文字を区別しない（ユーザー名の大文字小文字違いだけで衝突するのを防ぐため）。

### MSTコミット・subscribeRepos（`commit_record_inner` が共通パイプライン）
1. アクターの `at_repo_cid`/`at_repo_rev`/`at_repo_data_cid` と署名鍵PEMをDBから取得
2. 既存の全レコード（`posts` + `atp_records`）をロードし、新規レコード追加でMST再構築
3. 新しいrev(TID)でcommit生成、P-256署名（low-S正規化必須）
4. 差分CARをエンコード
5. トランザクション内で `atp_blocks`/`actors`/`atp_records`/`posts`(該当時)/`atp_repo_events` を更新
6. `subscribeRepos` 用フレームを生成しzstd圧縮して `atp_repo_events.frame_bytes` に保存、commit後にWebSocket配信（複数レプリカ時はRedis Pub/Subブリッジ経由）

`GET /xrpc/com.atproto.sync.subscribeRepos` は `cursor` 指定時に `atp_repo_events` から未送信分を500件ずつページングして送り切ってから、以降はリアルタイムbroadcastを購読する（1回のクエリで最大500件しか返らないため、tipから500件以上遅れたcursorでの再接続でも取りこぼさないようループする必要がある）。

> `atp_repository_publish` ジョブ（外部PDSへのミラーリング用に定義されている）は enqueue する呼び出し箇所が存在せず、実質デッドコードになっている。

### Bsky公式Relayの新規PDSアカウント数上限に注意
Bsky公式Relay（`bsky.network`）は新規（未検証）PDSに対してホスト単位のアカウント数上限を設けており、上限を超えて登録されたアカウント（作成順で後の方）は `host-throttled` 扱いとなり、そのアカウントのコミットは `subscribeRepos` の配信対象から意図的に除外される（PDS側にエラーは一切返らず、`requestCrawl` も200 OKを返し続けるため、PDS側のログからは検知できない）。「特定ユーザーだけ投稿がbsky.appに反映されない」という報告を受けたら、まずこの上限超過を疑う。indigoの`cmd/relay/relay/account.go`にロジックがあり、ローカルでindigo/relayを動かして自PDSのホストレコード（`account_count`/`account_limit`）を直接確認することで検証できる。上限緩和にはBsky公式のPDS Administrators Discordへの参加・申請が必要（[Early Access Federation for Self-Hosters](https://docs.bsky.app/blog/self-host-federation)参照）。

### Jetstream 経由の取り込み（`seiran-atp-repo::firehose`）
`wss://jetstream1.us-east.bsky.network/subscribe?wantedCollections=app.bsky.feed.post&wantedCollections=app.bsky.feed.like` に接続。

- **wantedDids絞り込み**: ローカルユーザーがフォロー中、またはいずれかのリストのメンバーであるBsky DIDの集合を30秒間隔でポーリングし変化があれば再接続。無関係な投稿・Likeの際限ない取り込みを防ぐための必須の絞り込み。
- **リーダー選出**: 複数プロセス起動時の重複接続を避けるため、Redisベースの `JetstreamLeaderElector` でリース制御。モノリスモードはRedis無しでも常時接続、split-role構成はRedis障害時にフェイルクローズ。
- **cursor永続化**: 直近処理イベントの `time_us` を `site_settings`（汎用KV）に5秒間隔で保存し、再接続時に引き継ぐ（プロセス停止中のイベント取りこぼし防止）。
- 保存対象は wantedDids に含まれるDIDのみ。投稿は同梱の `record.text`/`record.createdAt` をそのまま使う（AppView再取得不要）。`app.bsky.embed.images`/`video`/`recordWithMedia` を解析しCDN URLを組み立てて添付保存。`record.facets`（`#link`/`#mention`/`#tag`）は6節の方式で処理する。
- Like（`app.bsky.feed.like`）は create/delete で `reactions` へINSERT/DELETE、通知・リアルタイム配信。
- `app.bsky.feed.post` の delete commit（`operation:"delete"`）は `at://{did}/app.bsky.feed.post/{rkey}` を組み立て、一致する `posts.at_uri` を論理削除する。`at_uri` 自体がイベント発行元の `did` から組み立てられるためLikeと同様になりすましは原理上不可能（他者のdidの投稿を指せない）。取り込んでいない投稿（フォロー対象外だった等）の delete イベントは無視。

### uploadBlob / getBlob・動画パイプライン
`getBlob` はCIDのmultihashからsha256を逆算し `media_files`/`atp_blobs` を検索してCDN URLへリダイレクトする（ストレージ本体を自前で再配信しない）。

動画・音声は原本をそのまま保存（トランスコードなし）、ffmpegでメタデータとサムネイルのみ抽出。`deliver_to_bsky=true` の場合、Bsky公式動画パイプライン（`app.bsky.video.uploadVideo`）へ提出する。**音声ファイルはBskyに専用embedが無いため、グレー背景の静止画+音声トラックのmp4に変換**してから動画として提出する。提出は非同期で `Job::BskyVideoPoll` が完了をポーリングし、間に合わなければ `app.bsky.embed.external`（URLカード）にフォールバックする。動画添付投稿は結合未確定の間 `Job::BskyPostCommitDeferred` でBskyコミット自体を遅延させ、早すぎるコミットによるexternal固定化を防ぐ。

### フォロワー検知ポーリング（`seiran-atp-repo::bsky_follower_poll`）
リモート Bsky アクターがローカルユーザーをフォローしたことを検知する経路。Jetstream の `wantedDids` は投稿・Likeの「発行者DID」でのフィルタであり、フォロー元（＝新規に自分をフォローしてきたアクター）を事前に知る手段が無いため、Jetstream購読では検知できない。そのため `app.bsky.graph.getFollowers`（AppView公開エンドポイント、認証不要）をローカルBskyリンク済みユーザーごとに`BSKY_FOLLOWER_POLL_INTERVAL_SECS`環境変数（デフォルト60秒）間隔でポーリングし、`follows`テーブルの既存フォロワー集合との差分から新規フォローを検知する常駐タスク（`seiran-atp-repo::run`内で`tokio::spawn`）。

- **baseline seed機構**: 機能導入時点で既に実フォロー済みの全フォロワーが初回ポーリングで一斉に「新規フォロー」と誤検出され通知が大量発生するのを防ぐため、`actors.bsky_followers_baseline_done_at`（NULL=未シード）をアクター単位のマーカーとして使う。未シードのユーザーは初回ポーリングで全フォロワーページを辿って `follows` へ無通知でINSERTするだけに留め、完了後にマーカーを立てる。以降のポーリングはbaseline済みとして扱い、新規フォロワーのみ通知する。
- **ページング**: `getFollowers`は新しい順で返る前提で、baseline済みなら既知フォロワーに到達した時点でそのユーザーの処理を打ち切る（`STEADY_STATE_MAX_PAGES=20`が安全上限）。未baselineなら`HARD_MAX_PAGES=1000`まで辿り切る。
- 新規フォロワーはDID未知なら`getFollowers`のレスポンス（handle/displayName/avatar）でそのまま`upsert_remote_bsky`する（`fetch_bsky_profile`への追加往復は不要）。通知は`source_uri`に`bsky-follow:{follower_actor_id}:{local_actor_id}`を付与し、複数インスタンス同時ポーリング時の重複INSERTを部分ユニークインデックス経由で防ぐ。
- **スコープ外**: Bsky側のアンフォロー検出（`follows`からの削除）は未実装。

## 4. クロスプロトコル配送ルール

中核ロジックは `seiran-api::handlers::notes::delivery`。`classify_post` が元ポストの出自を判定する: `actors.domain == local_domain` ならローカル、それ以外は `(ap_object_id有無, at_uri有無)` から `FediRemote`/`BskyRemote`/`LocalOrSeiran`（両方あり＝他seiranサーバー）に分類する。

- **リプライ**: 元ポストが Fedi リモートのみなら Bsky 配信しない。Bsky リモートのみなら Fedi 配信しない。親の可視性が `followers_only` ならリプライも継承する。
- **引用**: 元ポストが Fedi リモートのみの場合、Bsky側は `app.bsky.embed.external`（URLカード。at_uri/cidがあれば `app.bsky.embed.record`）。AP側は `ap_object_id` があればそれをquoteUrlに、無ければ bsky.app URL に変換。
- **リポスト**: 元ポストが `ap_object_id` を持つなら Fedi へは `Announce`。持たず `at_uri` のみ(Bskyリモート)ならテキスト投稿（「🔁 author: bsky.app URL」）にフォールバック。Bsky側も同様の非対称フォールバックがある。`visibility` が `followers_only`/`direct` の場合、フォロワー限定配信を持たない Bsky へのリポストはスキップする。
- **投稿削除**（`DELETE /api/notes/:id`、本人のみ）: DB上は論理削除（`posts.deleted_at`）のみで、リアクション・他ユーザーによるリポスト・通知等の関連行はカスケード削除しない（読み取り側が一貫して`deleted_at IS NULL`を見る設計）。配送は「実際に配送済みだった経路」にのみ行う: `deliver_fedi`が真かつ`visibility != 'direct'`なら`ApDeliveryKind::DeleteNote`（フォロワー全員へ`Delete(Note)`）をenqueue、`at_rkey`が保存済みならBsky側レコードを`delete_atp_post`で削除。`direct`（DM）投稿は`DeleteNote`がフォロワー配送しか持たないため配送対象外（本来の宛先には届かない、既知の制約）。

## 5. 重複排除・マージ（水際防御）

同じ内容の投稿が複数ルートで自サーバーに届くケースへの対処。3シナリオ:

1. **ループバック**（自サーバー投稿の逆輸入）: 受信Noteの `id`/`url` が `https://{local_domain}/notes/{id}` パターンに一致すれば `parent_original_post_id` にセットしてINSERT（重複許容 + リンク）。
2. **他seiranサーバー間マージ**: 送信側は投稿作成時に `seiran_post_uuid`（UUID v4）を生成しAP Noteに `seiranUuid` として埋め込む。受信側は `find_by_seiran_uuid` で既存行を検索し、あれば新規INSERTせず `ap_object_id` をUPDATEするのみ。
   - **既知の制約**: `seiran_post_uuid` は ATP 側（Bskyレコード本体）には埋め込まれていない。そのため Jetstream 経由で先に取り込まれた投稿に後から AP の `Create` が届いても `find_by_seiran_uuid` は一致せず、**別行として新規INSERTされる**（マージされない）。現状「AP側が先」の場合のみ機能する。
3. **一般ブリッジ重複**: Noteの `url` が `https://bsky.app/profile/{did}/post/{rkey}` 形式なら `at://` URIへ変換し既存ポストを検索、あれば `parent_original_post_id` にリンク（重複許容 + リンク）。

## 6. 本文中のリンク・メンション表現

Bluesky facet・ActivityPub `<a href>` が示すリンク情報を、Misskey API互換（`NoteResponse.text`はプレーンテキストのまま）を保ちつつ画面上でクリック可能にするため、Misskey本家のMFM同様「`text`フィールドの中に内部リンクマーカーを埋め込み、フロントがパースする」方式を採る。

### 内部リンクマーカー
`[表示テキスト](URL)`（Markdownリンク記法）をURLリンクのマーカーとして使う。`URL`が`/`始まり（`//`除く）ならフロント（`RichText`コンポーネント、`frontend/src/components/note/RichText.tsx`）は内部ルーティング、`https?://`ならタブ外部リンクとして描画する。

- **Bsky `#link` facet**: `crates/seiran-atp-repo/src/firehose.rs` の `apply_link_facets` が、facetの `byteStart`/`byteEnd` が指すテキスト範囲を `[元テキスト](facet.uri)` に書き換えてから `posts.body` へ保存する（受信時に確定。URLは不変なので都度解決不要）。
- **AP `<a href>`**: `crates/seiran-common/src/jobs/inbound_activity_process.rs` の `ap_content_to_markdown_body` が `content` のHTMLをタグ除去する際、`<a href="URL">text</a>` を `[text](URL)` に変換する（Mention以外のアンカー。ハッシュタグアンカーもここに含まれ、リモートインスタンスのタグページへの外部リンクになる）。`<br>`/`</p>`/`</div>` は改行として保持し（`\n`/`\n\n`）、Mastodon等がcontentを複数段落のHTMLで表現しても本文の改行が失われないようにする（`tag_break_text`/`normalize_whitespace_preserving_newlines`）。

### メンションは内部リンクマーカーで包まない
フロントの `RichText` コンポーネントが `@user@host`（Fediverse形式）・`@handle.bsky.social`（Bskyハンドル形式）のパターンを自動検出し `/@...` へのプロフィールリンクに変換するため、メンションは `[text](url)` で包まず `@handle` 形式のプレーンテキストのまま `text` に埋め込む。**メンションを一般URLリンクの経路（`[text](href)`）に落とすと、リンク先がリモートアクターの本拠地サーバー（プロフィールURL）になってしまうため、必ずこの経路で処理する。**

- **AP Mention**: `ap_content_to_markdown_body`（`resolve_ap_mention_text`）が3段階でメンション文字列を解決する。
  1. `<a href>` が `tag`配列 Mention の `href` と完全一致 → その `name` を使う
  2. 一致しないが `<a>` の `class` に `mention`/`u-url` トークンがある（Mastodon等は `<a href>` に人間向けプロフィールURL、`tag[].href` にAPアクターURIを使い分け、両者が食い違うことがある）→ `<a href>` と `tag[].href` の**ホスト名が一致する** Mention を優先的に探し、無ければユーザー名一致のみへフォールバックする（`find_mention_name_by_inner_text`）。**ホスト名を先に見るのは、同一Note内に同名ユーザーの Mention が複数存在するケース**（例: 投稿者自身への自己言及 `@yuba` と別インスタンスの `@yuba@fedibird.com` が同居、実機確認）**でユーザー名だけの判定だと誤った方に一致してしまうため**
  3. 上記いずれにも該当しないが `class` から見てメンションらしい → `<a>` の内側テキスト（例: `@bob`、投稿元インスタンス内の相対メンション表記でドメイン省略のことがある）に、投稿者アクターの `domain`（`sender_domain`）を補って `@bob@sender_domain` の完全修飾形にする

  解決した `tag.name` がドメイン省略（`@yuba` のように単一`@`のみ）の場合は `qualify_mention_name` が `tag.href` のホスト名を補って完全修飾する。**Misskeyは投稿者自身への自己言及メンションで `name` をローカルドメイン省略で送ってくることがある**（実機確認: `attributedTo` と同一アクターへのMentionで `name: "@yuba"` のみ）。

  `class` に `mention`/`u-url` が無い通常の `<a href>` は、この解決を試みず通常のURLリンク（`[text](href)`）として扱う。Fediverseのハンドルはほぼ不変なので受信時に確定してよく、DB照会は不要。
- **Bsky `#mention` facet**: facetにはDIDしか無く、ハンドルは可変（DIDが不変の識別子）なため、`posts.body` は書き換えず、`{byteStart, byteEnd, did}` を `posts.mention_facets`（JSONB配列）に保存する。表示時（`NoteResponse` 生成時）に都度DIDを解決してハンドルへ置換する（`crates/seiran-api/src/handlers/notes/dto.rs` の `apply_mention_facets`）。未解決のDIDは投稿時点の表示テキストのまま返す。
  - **N+1回避**: タイムライン等でまとめて複数投稿を返す箇所は、`crates/seiran-api/src/handlers/notes/queries.rs` の `resolve_mention_facets_in_place` が登場する全DIDを1回の `IN` 句クエリでバッチ解決してから `to_note_response` を呼ぶ。
  - **未知DIDの先行解決**: ローカル `actors` に無いDIDは `Job::ResolveBskyMention` をキューに積み、Bsky AppViewから非同期でプロフィールを取得して `actors` へupsertする（ベストエフォート。次回表示までに間に合わなくても実害はない＝その回は元テキストのまま表示されるだけ）。

### 送信（seiranユーザー投稿 → Fedi/Bsky）のメンション/リンク解決
`crates/seiran-common/src/mention.rs` が本文中の `@...` メンション・生URL（`http(s)://` から空白/`<>()[]` の手前まで）を配信先プロトコルごとに解決する。`@`直前のメールアドレス誤判定ガードはASCII英数字のみ見る（`is_ascii_alphanumeric()`）。Unicode版 `is_alphanumeric()` だと日本語等の文字も真になり、「文章@handle」のようにCJK文字に直接続くメンションを誤ってメールアドレスの一部とみなしスキップしてしまう（実機確認: 全角括弧直後にスペース無しで続くメンションが完全に無処理になっていた）。

DID解決は常に公開AppView（`app.bsky.actor.getProfile` / `com.atproto.identity.resolveHandle`、`public.api.bsky.app`）を使う。`bsky.brid.gy` は `com.atproto.identity.resolveHandle` を実装していない（`MethodNotImplemented`、実機確認）ため、ブリッジ済みハンドル（`{user}.{domain}.ap.brid.gy`等）のDID解決にも使わない。

- **Bsky向け（`convert_mentions_for_bsky`）**:
  - 生URL（`https://example.com` 等） → テキストは変更せず `app.bsky.richtext.facet#link` を付ける。
  - `@username`（ローカル、ドメイン省略） → `@username.{local_domain}` に展開し、DIDが取れれば `app.bsky.richtext.facet#mention`。
  - `@username@{local_domain}`（ローカルユーザーのFedi表記） → ローカルユーザーだとわかっているので上と同じ `@username.{local_domain}` に変換する（Fedi表記のままBskyに出さない）。
  - `@handle.tld`（AT Protocolハンドル形式） → テキストは変更しない。`.{local_domain}` サフィックスならローカルユーザーとしてDID解決、そうでなければ公開AppViewでハンドル→DIDを解決しmention facetを付ける。
  - `@user@domain`（他ドメインのFediverse形式） → brid.gyハンドル（`{user}.{domain}.ap.brid.gy`）を組み立て公開AppViewでDID解決できればmention facet。解決できない場合はテキストは `@user@domain` のまま変えず、代わりに `app.bsky.richtext.facet#link` を付ける（リンク先は既知のfediアクターなら本拠地URL=`actors.ap_uri`、未知なら自ドメインのリモートプロフィールページ `https://{local_domain}/@user@domain`）。
- **AP向け（`convert_mentions_for_ap`）**: 戻り値は `(変換後テキスト, Vec<ApInlineMention>)`。各スパンは `href`・表示名・`is_mention`（`tag[]` に載せるか）を持つ。
  - 生URL → テキストは変更せず、`is_mention: false` のリンクスパンとして追加する（`<a>` 化されるが `tag[]` には載らない）。
  - ローカル `@username`（ドメイン省略） → 外部から見て意味を持つよう `@username@{local_domain}` に qualify し、ローカルアクターURI（`https://{local_domain}/users/{username}`）への Mention にする。
  - `@username.{local_domain}`（ローカルユーザーのBskyハンドル表記） → ローカルユーザーだとわかっているので brid.gy 解決は試みず、上と同じ `@username@{local_domain}` の Mention に変換する（Bsky表記のままFediに出さない）。
  - `@user@domain`（他ドメイン） → テキストは変更しない。DB（既知アクターの `ap_uri`）または webfinger（`https://{domain}/.well-known/webfinger?resource=acct:user@domain`）で href を解決できた場合のみ Mention を追加する。
  - `@handle.tld`（他ドメインのBskyハンドル表記） → brid.gy webfinger（`acct:{handle}@bsky.brid.gy`）で解決できれば `@handle.tld@bsky.brid.gy` の Mention、できなければ `bsky.app/profile/{handle}` への単なるリンク（`is_mention: false`）。**ブリッジは対象アカウントがbrid.gyでの連携を有効化していない限り存在しない**（Bsky上に実在するアカウントでも無条件にブリッジされるわけではない）ため、この経路は珍しくない。
  - Note の `content` HTML は `crates/seiran-common/src/ap/deliver.rs` の `plain_to_html_with_mentions` が上記スパンを `<a href="...">` へ変換して組み立てる（`is_mention` なスパンのみ `class="mention u-url"` を付ける）。`is_mention` なスパンは `tag[]`（`{"type":"Mention","href":...,"name":...}`）にも追加する。この変換・`tag[]` 組み立ては push配送（`deliver_post_to_ap_followers`、`override_body` 未指定時のみ）と pull取得（AP直接フェッチ `get_note_ap`）の両方で共有し、両者が食い違わないようにしている。リポストのフォールバックテキスト（`override_body` 指定時）はメンション変換をせずプレーンにHTML化する。

### Bsky向け本文の文字数上限と受理タイミング
`app.bsky.feed.post` の本文上限は書記素クラスタ数300・バイト数3000。メンション変換（`@user` → `@user.example.com` 等）でテキストが伸びうるため、`crates/seiran-api/src/handlers/notes/mod.rs` の `create_regular_post` は投稿をDBへINSERTする前に `convert_mentions_for_bsky` を同期的に実行し、**変換後テキスト**に対してこの上限を検証する（`validate_text_length`）。超過時は投稿自体を作らず `TEXT_TOO_LONG` エラーを返す。Bsky非配信時は元の入力テキストに対する緩い上限（3000書記素・10000バイト、Fedi向け）のみ検証する。

### 既知の制約
- ローカル投稿者が生テキストに書いた `@mention` は、`posts.body` 自体は書き換わらない（AP/Bsky配送用のコピーにのみ `mention.rs` の変換がかかる）。フロントの `RichText` が本文中のプレーンな `@handle` パターンを直接検出してリンク化することでこれを補っている。
- 内部リンクマーカーとして `[text](url)` を採用しているため、投稿本文がたまたまこの形式の文字列を含む場合は意図せずリンク化されうる（許容している）。
- 送信時に生URLの自動リンク化（`app.bsky.richtext.facet#link` / AP `<a>`）は対応済みだが、ユーザーが手書きした `[text](url)`（Markdownリンク記法）のリンク化には未対応。

### ハッシュタグ
ハッシュタグはポストとm:nの関係を持つ永続化オブジェクト（`hashtags`/`post_hashtags`、`docs/database.md` 参照）として扱い、検索の即席表示ではなく専用のハッシュタイムライン（`GET /api/hashtags/:name/timeline`）の主軸にする。

- **送信側（ローカル投稿 → Bsky/AP）**: `convert_mentions_for_bsky`/`convert_mentions_for_ap`（`mention.rs`）は本文中の `@`（メンション）・`h`（URL）に加えて `#`（ハッシュタグ）もスキャンする（共通ヘルパー `scan_hashtag`。境界・除外ルールは `extract_hashtags` と同じだが、表示用テキストなので大文字小文字は保持する）。
  - Bsky: `app.bsky.richtext.facet#tag`（`tag` フィールドは `#` を除いた本体、大文字小文字保持）を本文中の出現位置ごとに付与する。
  - AP: `<a href="https://{local_domain}/tags/{正規化タグ}" class="mention hashtag" rel="tag">#タグ</a>` というアンカー（リンク先は自インスタンスのハッシュタイムライン）と、`tag[]` への `{"type":"Hashtag","href":...,"name":"#タグ"}` エントリを追加する。アンカー組み立て・`tag[]` 組み立ては push配送（`ap::deliver`）と pull取得（`get_note_ap`）の両方で `ap_inline_mentions_to_tag_json` を共有する。
- **受信側の分類**: Mastodon等はハッシュタグアンカーにも `class="mention hashtag"` を付与する（メンションと `mention` トークンを共有する）ため、`class` だけでメンション判定すると `#foo` が壊れたメンション文字列（`@#foo@sender_domain`）に誤変換される。`ap_content_to_markdown_body`（`inbound_activity_process.rs`）は `rel="tag"` または `class` の `hashtag` トークンを検出したら、メンション解決ロジックより先に「ハッシュタグである」と判定して通常のURLリンク（`[#foo](url)`）として扱う。
- **抽出（DB永続化）**: プロトコル別の特別処理を持たず、ローカル投稿・AP受信・Bsky受信いずれも「最終的な `posts.body` テキストを1回スキャンする」共通経路（`seiran_common::hashtag::extract_hashtags`）でハッシュタグを抽出し `HashtagRepository::link_post` でDBへリンクする。AP由来のハッシュタグアンカーは `[#foo](リモートのタグページURL)` というMarkdownリンクに変換されるが、リンクテキスト部分に `#foo` が残るためこの共通スキャンだけで取りこぼしなく検出できる。Bsky受信の `app.bsky.richtext.facet#tag`（`JetstreamFacetFeature::Tag`）は本文中に既に `#foo` がプレーンで載っているため、facet自体の値は今も参照しない。
- **表示側**: フロントの `RichText` は `#foo` パターンとMarkdownリンクのリンクテキストが `#タグ` 形状の場合の両方を検出し、いずれも自インスタンスの `/tags/foo` へのリンクとして描画する（AP由来のハッシュタグアンカーもリモートのタグページへは飛ばさず、同列に扱う）。
- **ホーム画面への追加**: `pinned_hashtags` にユーザーごとのピン留めを保存し、ホーム画面のフィードタブとして表示する（`pinned_posts`/`lists` と同じ「ユーザーごとの永続ショートカット」の設計思想）。

## 7. Misskey API 互換レイヤー

`middleware::misskey_auth_bridge` が `Authorization` ヘッダー未指定時にJSONボディ/クエリの `i` を検出し `Authorization: Bearer` を合成する。`handlers::misskey`（`endpoints.rs`/`convert.rs`/`types.rs`）が Misskey ワイヤー形式のエンドポイントを提供する。

対応済み: `POST /api/meta`（サーバー検出）、MiAuthフロー、`POST /api/i`、`/api/users/show`、`/api/users/notes`（プロフィール画面のノートタブ、`timeline_by_actor`を使用）、`/api/notes/show`、`/api/notes/local-timeline`・`timeline`、`/api/notes/reactions/create`・`delete`、`/api/notes/unrenote`、`/api/following/create`・`delete`、`/api/i/notifications`（DB永続化、`untilId`/`sinceId`カーソル）、`GET /api/emojis`（未認証公開）。

書き込み系は既存の `handlers::notes`/`handlers::follows` をそのまま呼び出し、レスポンスだけMisskey形状に整形する。

**既知の非互換点**: `visibility` は常に `"public"` 固定、`cw` は常に `null`、書き込み系のエラー形状はMisskey本家のエラーID体系を再現していない、本文中カスタム絵文字インライン用 `emojis` マップは常に空。ストリーミングはMisskeyのチャンネル購読方式ではなく単純な認証ユーザー宛てブロードキャストのみ。`MisskeyDriveFile.isSensitive` は概念自体をDBに持たないため常に `false`。

**`misskey_dart`（Aria等）の non-nullable 直接キャスト対策**: `misskey_dart` の生成コード（`*.g.dart`）は本家スキーマの必須フィールドを `as String`/`as num` 等で直接キャストするため、JSONでキーが欠けたり `null` だと Dart 側で未処理の `TypeError` となりクライアントが落ちる（サーバー側のバリデーションエラーとは別の失敗モード）。`MisskeyMeDetailed`（`notesCount` 等）に続き `MisskeyDriveFile`（`createdAt`/`md5`/`size`/`isSensitive`/`properties`）、`MisskeyUserDetailed`（`/api/users/show`・`/api/i` 共通、`notesCount`/`followersCount`/`followingCount`）でも踏んだため、Misskey互換型を追加・変更する際は本家スキーマの必須/任意を都度 `misskey_dart` のソースで確認すること。`md5` は seiran 内部で持つ `sha256` を代用し、リモート添付など元データが無い場合は空文字列/0を返す（クライアントは値を検証せず保持するだけのため実害はない）。

## 8. 通知・リアルタイム配信

`seiran-common::streaming::StreamHub`（プロセス内 `tokio::broadcast`、容量512）が `{"type":kind,"body":body}` を配信する。`GET /api/streaming?token=<JWT>` でWebSocket接続し、`recipients` に自分の actor_id が含まれるイベントのみ転送される。

`notifications` テーブルへの書き込みは、ローカルリアクション作成・AP/ATP inbound（Follow/Accept/Reaction）の各経路から行われる。種別は `Follow`/`Reaction`/`FollowRequestAccepted`/`Mention`/`Reply` の5種。WebSocketは基本的に「新着があった」というシグナル配信のみに用い、実データは常に `POST /api/i/notifications`（REST、`sinceId`付き）から再取得する（一覧表示とスキーマを統一するため）。

### リアクション通知の重複排除（`reaction_id`）
ローカルユーザーが ATP 実体（`at_uri`/`at_cid`）を持つ投稿へリアクションすると、(1) `notes::create_reaction` がその場でローカル通知を即時INSERTし、(2) 同じリアクションを非同期で `AtpCommitService::commit_like` が `app.bsky.feed.like` としてコミットし、それが自分自身の firehose 受信（`seiran-atp-repo::firehose::handle_inbound_like_create`）で戻ってきて再度通知INSERTを試みる、という2経路が走る。この2つは「経路が違うだけの同一操作」であり、素朴に両方INSERTすると通知が重複表示される。

これを防ぐため、`reactions.id`（`GENERATED ALWAYS AS IDENTITY`）を「リアクション実体の識別子」として2経路で共有する。ローカルINSERT時に採番された `reactions.id` を (a) `notifications.reaction_id` に保存し、(b) `commit_like` が `app.bsky.feed.like` レコードの非標準拡張フィールド `seiranReactionId` として埋め込む（`emoji` 拡張フィールドと同じ流儀）。自分自身の firehose 受信時、このLikeが `seiranReactionId` を持っていればそれをそのまま `notifications.reaction_id` として渡し、`idx_notifications_reaction_id`（`reaction_id IS NOT NULL` の部分UNIQUEインデックス、`ON CONFLICT DO NOTHING`）で2つ目のINSERTが弾かれる。

`source_uri` によるUNIQUE制約（既存）とは目的が異なる: `source_uri` は「他人発のイベントの複線受信対策」（Doc6既知の課題）だが、`reaction_id` は「自分が起点の同一操作が別経路で戻ってくることの対策」。他人（他インスタンスのMisskey/Mastodonユーザーや他のBskyユーザー）からのリアクションには `seiranReactionId` が付かないため `reaction_id` は常に `NULL` で、同じ投稿に複数の絵文字で連投する（通知欄に文章のようなものを書く遊び）動作は妨げない。

例外として `followAccepted`（`jobs::inbound_activity_process::handle_accept`、Fediフォローリクエストが相手から承諾された）はペイロード（`actor.username`/`actor.domain`）自体をフロントエンドが利用する。Fediフォローは常に `pending` で開始し、相手の `Accept` が非同期で届くまで承認待ち状態が続く（`handlers::follows::follow_fedi`）ため、`StreamingContext` が受信時に `stores/followStatusStore`（`username`+`domain` を正規化したキー、`lib/format.ts` の `profileQuery` と同じロジック）を直接更新し、`pending` → `accepted` へその場で切り替える。手動リロードや通知一覧の再取得を待たずに反映するための例外であり、通知の永続化・一覧表示自体は他の種別と同じ経路を通る。フォロー状態の表示側（`frontend/src/pages/ProfilePage.tsx` のフォローボタン、`frontend/src/components/note/NoteCard.tsx` のタイムライン上のフォロースイッチ）はいずれもこの共有ストアを `useSyncExternalStore` で参照する設計のため、自分の操作・WebSocket経由の承認のいずれでも、同一アクターを表示中の全コンポーネントが同時に反映される（詳細は `docs/architecture.md` のフロントエンド構成節）。

### メンション通知
本文中で `@username` 形式によりローカルユーザーが言及された場合、`notifications`（`type="mention"`, `note_id`=言及元投稿）を作る。配信設定（Bsky/AP接続の有無）とは無関係に、投稿の出自（ローカル/Fedi受信/Bsky受信）ごとに以下で解決する。自己メンションは通知しない。

- **ローカル投稿**（`handlers::notes::create_regular_post`）: `mention::extract_local_mention_actor_ids` が本文を走査し、`@username`（ドメイン省略）・`@username.{local_domain}`（AT Protocol ハンドル表記）・`@username@{local_domain}`（Fediverse表記）のいずれかで書かれたローカルアクターの `actor_id` を重複除去して返す。6節の配信用メンション変換（`convert_mentions_for_bsky`/`convert_mentions_for_ap`）は配信対象プロトコルが有効な場合のみ呼ばれるため、これとは独立した専用スキャンとして常に実行する。
- **Fedi受信**（`jobs::inbound_activity_process::handle_create_note`）: `tag[]` の `Mention` エントリのうち、`href` が `https://{local_domain}/users/{username}` を指すものを、DM宛先解決と同じ「URI末尾セグメントをusernameとみなして `find_by_username_domain` で解決する」方式で判定する。
- **Bsky受信**（`seiran-atp-repo::firehose::save_bsky_post`）: 保存済みの `mention_facets`（6節）の各 `did` を `actors.at_did` で引き、`actor_type = 'local'` なら通知する。

いずれの経路も `source_uri` は渡さない（1投稿に複数の宛先がありうるため、投稿の一意識別子を共有すると2人目以降が `notifications.source_uri` の部分UNIQUEインデックスで弾かれてしまう。posts 自体の重複排除は各経路で別途完結しているため、このブロックへの到達自体が新規保存時のみに限られ、重複INSERT対策は不要）。

### リプライ通知
自分の投稿に返信が付いた場合、`notifications`（`type="reply"`, `note_id`=返信投稿自体）を作る。可視性・配信設定とは無関係に常に処理し、リプライ先投稿者がローカルユーザーの場合のみ通知する。自己リプライは通知しない。本文中に相手への `@username` を書いた場合はメンション通知とは別に両方生成されうる（Misskey/Mastodon等と同様の挙動）。

- **ローカル投稿**（`handlers::notes::create_regular_post`）: リプライ先解決（`resolve_reply_context`）が返す `ReplyContext::parent_local_actor_id`（リプライ先投稿の `PostDeliveryMeta::domain` が自ドメインの場合のみ `Some`）を宛先に使う。
- **Fedi受信**（`jobs::inbound_activity_process::handle_create_note`）: `note["inReplyTo"]` から解決した `reply_to_post_id` の投稿者を `PostRepository::find_delivery_meta` で引き、`domain` が自ドメインなら通知する。
- **Bsky受信**（`seiran-atp-repo::firehose::save_bsky_post`）: `record.reply.parent.uri` から解決した `reply_to_post_id` の投稿者が `actor_type = 'local'` なら通知する。

いずれの経路も `source_uri` は渡さない（1リプライにつき宛先は常に1人だが、メンション通知と実装を揃えるため統一している）。

## 9. ダイレクトメッセージ

`visibility='direct'`の投稿をそのまま`posts`に格納する方式でDMを実現する（`docs/database.md`の「ダイレクトメッセージ関連」節も参照）。Misskey APIクライアントも同じ投稿テーブルを読み書きするため、Bsky DMも含めてMisskey互換の投稿・タイムライン取得APIでそのまま扱える。

### 宛先・スレッド・タイムライン除外
- 宛先は`post_recipients`（post_id/actor_id）に持つ。投稿作成API（`POST /api/notes/create`、Misskey互換では`visibleUserIds`も同じ意味で受け付ける）が`visibility=direct`のとき`recipient_actor_ids`必須。
- スレッド起点（`posts.thread_root_post_id`）は再帰クエリではなく伝播コピー方式。新規direct投稿作成時、親（`reply_to_post_id`）が`direct`ならその`thread_root_post_id`をそのままコピーし、親が`direct`でなければ自分自身のIDを設定する。
- 各タイムライン系クエリ（`home_timeline`/`local_timeline`/`timeline_by_actor`等）の`direct`閲覧制御は「投稿者本人 or `post_recipients`の宛先」のみ（`followers_only`とは異なりフォロワーには見せない）。`exclude_direct`クエリパラメータ（Misskey互換のためデフォルト`false`）を付けると宛先者でも一切表示しない。seiranフロントエンドは常にこれを付与する。
- リスト・ピン留めタイムラインは閲覧者情報を持たない/宛先チェックの構造上の理由から、`direct`を無条件で除外する（`repository::list::timeline`、`repository::pinned_post::list_timeline_by_actor`）。

### Fedi受信（`jobs::inbound_activity_process::handle_create_note`）
`to`/`cc`から`classify_ap_visibility`が`direct`と判定した場合、通常投稿受信経路とは別に以下を行う。
- `note["inReplyTo"]`から`reply_to_post_id`を解決する（`find_id_by_ap_or_at_uri`。DM以外の通常投稿にも設定するようになった。以前はFedi受信投稿は`reply_to_post_id`を一切保存しない実装だった）。
- `to`に含まれるローカルアクターURIから宛先を解決し`post_recipients`へ保存する。ローカルユーザーの`actors.ap_uri`は登録時に設定されない（都度`https://{local_domain}/users/{username}`として動的組み立てされる）ため`find_by_ap_uri`では引っかからない。`handle_follow`と同じくURI末尾のセグメントをusernameとみなし`find_by_username_domain`で解決する。
- `reply_to_post_id`の親が`direct`ならその`thread_root_post_id`を継承、そうでなければ自分自身のIDをスレッド起点とする（伝播コピー方式はローカル投稿と共通）。
- WS配信は宛先のみ（フォロワーには配信しない）。

### 配送
- Fedi宛先: `ap::deliver_direct_message_to_ap`が`post_recipients`の中のFediアクターのinboxのみへCreate(Note)を送る（`to`は宛先アクターURIのみ、フォロワーコレクションではない）。`Job::ApDeliveryKind::DirectMessage`経由。
- Bsky宛先: `jobs::bsky_dm_send`が`chat.bsky.convo.sendMessage`で送信する（`Job::BskyDmSend`）。1スレッドにつき1回だけ`chat.bsky.convo.getConvoForMembers`でconvoIdを解決し`bsky_convo_links`にキャッシュする。認証は自己署名サービス認証JWT（`docs/skill_atp_rust_programming.md` §17、`aud`はfragment無しの`did:web:api.bsky.chat`）。Bsky宛先は1対1のみ（宛先にBskyアクターが1人でも含まれる場合、他の宛先との同居はAPIレベルで拒否）。
- WS配信: `direct`投稿は`delivery::broadcast_direct_message`で投稿者本人+宛先のみに配信する（通常投稿の`broadcast_new_note`はフォロワー全体に配信するため、DMには使わないこと。本文漏洩防止）。

### Bsky受信ポーリング（`seiran-atp-repo::bsky_dm_poll`）
`chat.bsky.convo`はJetstreamに乗らない（私信のため公開ファイヤホースに含まれない）ため、ローカルBskyリンク済みユーザーごとに60秒間隔で`listConvos`→新着があれば`getMessages`をポーリングして取り込む常駐タスク（`seiran-atp-repo::run`内で`tokio::spawn`）。`bsky_convo_links.last_synced_message_id`を重複取り込み防止カーソルに使う。取り込んだメッセージは`posts`（visibility=direct、thread_root_post_id・post_recipients設定）へ保存しWS配信する（送信者が自分自身のメッセージは`BskyDmSend`側で既に保存済みのためスキップ）。グループ会話（`kind=groupConvo`）は対象外。

2026-07-20実機確認: `@ethilen.bsky.social`との送受信を実地テスト済み（送信・受信ポーリングとも正常動作）。

### 未読管理
`dm_read_states`（actor_id, thread_root_post_id, last_read_post_id）でスレッド別の既読カーソルを持つ。左ペインバッジは「未読のあるセッション数」（`DmRepository::unread_session_count`）。

### `chat.bsky.actor.declaration`（Bsky DM受信許可）
Bluesky公式クライアントは相手のPDSから`chat.bsky.actor.declaration`（rkey固定`self`、`allowIncoming: "all"|"none"|"following"`）を取得してDM送信可否を判定する。このレコードが無いと保守的に送信をブロックする（実機確認: 未コミット状態のseiranユーザーへ公式クライアントからDMを送ろうとすると宛先候補がグレーアウトする）。`AtpCommitService::commit_chat_declaration`が`allowIncoming: "all"`固定でコミットする。新規ユーザー登録時（`handlers::auth::register`）と、起動時のバックフィル（`spawn_startup_tasks`→`backfill_chat_declarations`、未コミットのローカルユーザーを検出して一括実行）の両方から呼ばれる。ユーザーが値を選べる設定UIは未実装（`docs/roadmap.md`参照）。

## 10. ブロック・ミュート

### 定義
- **ミュート**: Fedi/Bsky共通で「自分のタイムライン・通知から相手を隠すだけのローカル効果」。相手には一切通知されず、AP/ATP配送は発生しない（`mutes`テーブルへのINSERT/DELETEのみ）。
- **ブロック**: seiranではBsky準拠の定義（フォロー関係の強制解除＋相互完全非表示）を採用する。Fediの「片方向拒否ブロック」とMisskey的「ミュート」を合わせた効果になるため、ブロック実行時は相手のプロトコルに応じて以下を行う。
  - 相手がBsky: `app.bsky.graph.block`をコミット（`AtpCommitService::commit_block`）。
  - 相手がFedi: AP `Block`アクティビティを配送する。
  - いずれの場合もローカルの`blocks`テーブルへの1行挿入により、タイムライン・通知の相互非表示（`actor_is_hidden_for_viewer`、`docs/database.md`参照）と書き込みガード（下記）の両方が有効になる。

### 書き込みガード
ブロック関係にある場合、以下の書き込み操作をAPIレベルで拒否する（`handlers::target_resolve::check_not_blocked`）。
- フォロー作成（`follows.rs::follow_local`/`follow_bsky`/`follow_fedi`）
- リプライ作成（`notes::delivery::resolve_reply_context`）
- リアクション作成（`notes::create_reaction`）
- 引用投稿・リポスト作成（`notes::mod::create_regular_post`/`create_repost`）
- DM送信（`notes::mod::create_regular_post`、`visibility=="direct"`の宛先ループ）

### プロフィール表示の制限
相手からブロックされている（`is_blocked_by`）場合、自己紹介文（`bio`）・プロフィールのキーバリュー項目（`profile_fields`）を`build_profile_response`が空にして返す。投稿一覧（`recent_posts`/`pinned_posts`）は元々`actor_is_hidden_for_viewer`によるタイムラインクエリのフィルタで空になる。

### AP受信時のフォロー拒否
`inbound_activity_process::handle_follow`は、こちらが送信者をブロック中であれば`Accept`を送らずサイレントに無視する（Fedi標準の片方向拒否ブロックを実現）。

### 相手発ブロックの検知
自分がブロックした場合だけでなく、**Fedi/Bskyリモートユーザーが自分をブロックした場合**も`blocks`テーブルへ記録し、上記の相互非表示・書き込みガードを対称に働かせる。
- **Fedi側**: AP `Block`アクティビティを受信（`inbound_activity_process::handle_block`）した時点で`blocks`へ`(blocker_actor_id=相手, blocked_actor_id=ローカル)`をINSERTする。`Undo(Block)`受信時（`handle_undo`）にDELETEする。
- **Bsky側**: Bluesky公式APIには「自分をブロックしている人一覧」を返すエンドポイントが無い（プライバシー保護のため意図的に非公開）ため、ポーリングでは検知できない。代わりに`seiran-atp-repo::bsky_block_watch`が、`app.bsky.graph.block`のみを対象とした**無絞り込み**Jetstream接続（`wantedDids`を使わない、実測で全世界約2件/秒程度）を張り、`record.subject`がローカルユーザーの`at_did`と一致するイベントだけを拾って`blocks`へ記録する。削除（Undo相当）はJetstreamの`delete`イベントに`subject`が同梱されない仕様のため、create時に`commit.rkey`を`blocks.atp_rkey`へ保存しておき、`(blocker_actor_id, atp_rkey)`の組で逆引きして削除する（`BlockRepository::delete_by_blocker_and_rkey`）。post/like用の既存Jetstream接続（`wantedDids`で絞り込み）とは独立したリーダー選出（`jetstream_leader::JetstreamLeaderElector`のリースキーをパラメータ化、`bsky_block_watch`専用キーを使用）で動く別接続。

### スコープ外
- **リアクション一覧表示でのブロック/ミュート除外**: 未実装（`fetch_reactions_map`は対象外）。
- **公開リストタイムライン（`list.rs::timeline`）でのフィルタリング**: 未実装。リストタイムラインは「閲覧者情報を持たない（誰が見ても同じ内容）」設計のため、viewer概念自体が無く、フィルタ追加には閲覧制御全体の見直しが必要。

## 11. 未実装・スコープ外の機能

- **ゼロトラストハンドシェイク**（他seiranサーバー間の `/verify-actor` 検証、`remote_seiran` への昇格）: 未実装。`actors.seiran_pair_actor_id` はスキーマ上・読み取りコードは存在するが書き込みロジックが無い（常にNULL）。
- **`actor_metadata_resolve` ジョブ**: ハンドラはdispatchに登録されているが中身はスタブ（即座に `Ok(())`）。enqueueする呼び出し箇所がプロダクションコードに存在しない。
- **トレンド集計**: 完全に未着手（テーブル・エンドポイントとも存在しない）。
- **Misskey互換ストリーミング（チャンネル購読方式）**: 未着手、現状はブロードキャストのみ。
- **ドメイン単位のレート制限**（`inbound_activity_process` 向け）: 未実装。現状 `actor_history_sync` キューのみドメイン単位の同時実行制限を持つ。
- **リモートFedi/Bskyユーザー自身の公開リストのオンデマンド取得**: 未実装（`public_lists` はローカルユーザーのみ対象）。
- **ブロック・ミュート関連の未実装項目**: 10節「スコープ外」参照（リアクション一覧でのブロック/ミュート除外、公開リストタイムラインでのフィルタリング）。

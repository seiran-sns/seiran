# マルチプロトコル実装

対象読者: ActivityPub / AT Protocol の実装やクロスプロトコル配送ロジックに触れる開発者。現在の実装と動作だけを書く（経緯は `git log`）。

## URL・IDからの対象解決（`POST /api/open`、#165）

`{ "target": string }` を受け、`{ "kind": "actor" | "post", "path": string }` を返す。

- bsky.app プロフィールURL・`did:plc:` → AppView でプロフィール取得し actor を upsert。
- bsky.app 投稿URL・AT URI → DID へ正規化して単一投稿を upsert。
- その他の HTTP(S) URL → ActivityStreams 表現を取得し、Actor は WebFinger/AP アクター解決、Note/Article/Question/Page は `InboundActivityProcess` の Create 経路で取り込む。
- `Announce` はリポストラッパーとして取り込む（`open_target::open_announce` → `handle_announce`）。Misskey の素リノートは `notes/{id}` では `quoteUrl` 付き Note（空リプ引用扱い）だが、他鯖ミラーURLや `notes/{id}/activity` では `Announce` になるため。対象ノートの取得に失敗してもラッパーは保存されるので、「開く」はラッパーの保存だけを待つ。

### pending参照の遅延解決

取り込み時に取得できず `pending` で保存された参照（4節「引用受信」）は、次の2経路で再取得する。どちらも `jobs::inbound_activity_process::resolve_pending_reference_with_timeout` を使い、成功なら `*_post_id` を、404/410 なら `ref_status = gone` を書く（それ以外の失敗は `pending` のまま）。`gone` は再取得しない。

- 投稿詳細取得時（`GET /api/notes/:id`、`retrieval::resolve_pending_post_references`）: 返信・引用・リポストを並行して各最大1秒。未ログイン閲覧（OGP等）も通るため短い。
- 手動（`POST /api/notes/:id/resolve-reference`、body `{"kind": "reply"|"quote"|"repost"}`、要認証）: 最大8秒。応答は `{"status": "resolved"|"pending"|"gone"|"none", "post_id": string|null}`。

## 通報配送

通報はまずローカル管理者に届き、対象がリモートなら管理者が任意に Fedi/Bsky へ転送する。成功時のみ `reports.forwarded_at` を記録する。

- Fedi: 通報者を `actor` とした AP `Flag` を対象 Actor の Inbox へ署名付きで送る。Flag はアカウント単位しか表せないため `object` は対象 Actor の URI のみとし、投稿通報では投稿URLを `content`（`[分類]`・自由記述の後）に付記する。
- Bluesky: `com.atproto.moderation.createReport` を Moderation Service へ送る。ユーザーは `repoRef`、投稿は AT URI/CID の `strongRef`。`reasonType` は `tools.ozone.report.defs#<reason_type>`（39種）をそのまま渡す。

## 1. フォロー時の初期同期

フォロー成立時に `Job::ActorHistorySync` が相手の過去30日分をベストエフォートで取り込む（Bsky: AppView `getAuthorFeed` で最大300件、AP: Outbox をページングして最大30件）。保存は通常の受信経路（Bsky: `upsert_bsky_post`、AP: `save_ap_note_core`）を通る。AP 側はアクター・絵文字解決のフェッチが1件ずつ発生するため件数を絞っている。タイムライン表示はローカルDBだけで完結し、外部APIを都度叩かない（`docs/database.md` 4節）。ノート詳細の前後投稿のオンデマンド同期も同じ仕組みを使う。

## 2. ActivityPub (Fedi) 統合

### 受理するオブジェクト型（`note_save::is_supported_note_type`）

`save_ap_note_core`（Create 受信・参照解決の両方が通る唯一の保存経路）は `Note`/`Question`/`Article` を受理する。`Article` はブログ系実装や bridgy-fed の Web ブリッジ（`web.brid.gy/r/<元URL>`）が使う型で、拒否するとそれらへのリポスト・引用・返信の参照解決が常に失敗する。

- `Article` の `name`（タイトル）は `note_save::prepend_article_title` が本文先頭に前置する（`content_html` には `<h3>`、`body` にはタイトル行＋空行）。他の型の `name` は無視する。
- `Article` は元記事を読めて初めて価値があるため、自身の `id` を常にURLカード候補の先頭に加える（`queue_link_cards_for_post` の `primary_url`）。web.brid.gy 等の `id` はブラウザでは実記事へ 301 するので、`Job::OgpFetch` はリダイレクト先の OGP を取得でき、カードの `url` は brid.gy の `id` のまま実記事のタイトル等が出る。`seiranPost.linkCards[]` を持つ投稿ではこの処理をしない。

### Fedi投稿のCW・アンケート・閲覧注意画像

`summary` を CW、`Question.oneOf`/`anyOf` をアンケートとして保存する。添付または投稿全体の `sensitive` が真なら画像を閲覧注意として返す。

### Fedi投稿のURLカード

AP には embed 概念が無いため、`save_ap_note_core` が本文（Markdown `[text](url)`）のリンクから `extract_link_card_urls` で最大5件（`MAX_LINK_CARDS_PER_POST`。重複・画像記法・`#` 始まりのハッシュタグリンクを除く）を抽出し、各URLに `Job::OgpFetch`（LOW）を積む。Bsky と違い1投稿に複数枚のカードが付きうる。

`net::fetch_ogp` は1回のページ取得で OGP（`og:title`/`og:description`/`og:image`）と oEmbed discovery（`<link rel="alternate" type="...json+oembed">` → JSON → `html` の iframe src）の両方を処理し、取得できた分を `post_link_cards` へ保存する。取得失敗は投稿の保存を妨げない。

- 文字コード: `net::decode_html_body` が `Content-Type` の `charset` → HTML 先頭1024バイト内の `<meta charset>`/`http-equiv` → UTF-8 の順で判定する。EUC-JP/Shift_JIS の日本語サイトがあるため。リモートインスタンスのトップページ取得（`remote_instance_info_resolve::fetch_homepage_meta`）も同じ関数を使う。
- `type` 属性は SoundCloud 等が非準拠の `text/json+oembed` を使うため、`json+oembed` の部分一致で判定する。
- `site_settings.oembed_allowed_domains` の各行は「domain」または「domain,oembedエンドポイントURL」。後者は discovery タグを出さないサイト（Vimeo 等）向けで、HTML を見ずにそのエンドポイントへ `?url=<URL>&format=json` で問い合わせる（`OembedWhitelist::fixed_endpoint_for`、`net::build_fixed_oembed_url`）。
- iframe src は行の domain で後方一致判定し（`oembed_whitelist.rs`、TTL 60秒キャッシュ）、許可された場合だけ `post_link_cards.embed_src`/`embed_type` へ保存する（フロントは `embedSrc` の有無でプレーヤー表示に振り分ける）。
- HTTP 取得は SSRF 対策込みの `net::fetch_validated_with_accept`（非公開IP拒否・リダイレクト先の再検証）を使う。oEmbed エンドポイントも同じ検証を通す。

### 構成
- `seiran-common::ap`: プロトコル共通ロジック
  - `client.rs` — `ApClient`（アクター取得、HTTP Signatures 検証・署名、to/cc → 可視性4値、絵文字 tag 解析）
  - `deliver/` — ローカル投稿の AP 配送。`build_*`（アクティビティJSONを組み立てる純関数）と `deliver_*`（DB取得＋署名POST）
  - `outbox.rs` / `webfinger.rs` — 過去ログ同期・WebFinger 解決
- `seiran-federation-inbox::handlers`: HTTP層
  - `inbox.rs` — 署名検証だけを同期で行い、実処理は `Job::InboundActivityProcess` に委譲する（受信レイテンシを抑えるため）
  - `actor.rs` / `outbox.rs` / `webfinger.rs` / `nodeinfo.rs` / `featured.rs` / `lists.rs` — 公開エンドポイント

### Inbox で処理する Activity 種別
実処理は `seiran-common::jobs::inbound_activity_process`。

| type | 処理概要 |
|---|---|
| `Follow` | ローカルアクター確認 → こちらが送信者をブロック中なら無視 → アクター upsert → ターゲットが承認制（`is_locked`）なら生アクティビティごと pending で保存し承認待ち通知、そうでなければ accepted で保存し通知して `Accept` を返す |
| `Create`(Note) | アクター upsert → HTML を内部リンクマーカー付きテキストへ変換（6節）→ 絵文字 tag 解析（欠落時は同一ドメインの `remote_emojis` で補完）→ 可視性判定 → 重複排除（5節）→ 保存 → URLカード・添付 → WS 配信 |
| `Accept`(Follow) | `follows.status = accepted`、通知。`object` は埋め込み Follow と URI 文字列の両方を受理する。送信する Follow ID は `activities/follow/{local_actor_id}-{remote_actor_id}` なので URI 形式でも関係を復元でき、`Accept.actor` が送信先と一致するか検証する（Mitra 対応） |
| `Block` | アクター upsert → ブロックした側へのフォロー関係を解消（`blocks` には書かず通知もしない。10節） |
| `Undo` | `Like`/`EmojiReact` → リアクション削除、`Announce` → リポスト論理削除、`Follow` → フォロー解除、`Block` → 何もしない |
| `Delete` | `object`（URI または Tombstone）に一致する投稿を、署名済みの送信者が投稿者本人の場合だけ論理削除する。`Delete(Actor)` は未対応 |
| `Update` | `Question`（票数更新）だけ受理する。`Delete` と同じ本人確認の上で `posts.poll` を更新し、`poll_update_received=true`・`poll_fetched_at=now()` をセットして `pollUpdated` を配信する（3節「アンケート」）。本文再編集は無視 |
| `Announce` | リポスト保存。元ポストが未登録なら `resolve_reference` で1段だけ取得を試みる（404/410 → `gone`、他の失敗 → `pending`）。取得の成否によらずリポストの箱は保存する。取得した元ポストは `save_ap_note_core` で保存するが、DMスレッド解決・通知・WS配信はしない。元ポスト未解決の間はリポスト通知もしない |
| `Like` \| `EmojiReact` | Misskey は絵文字リアクションも `type:"Like"` で送るため、wire type ではなく `content`/`_misskey_reaction` の有無で判定する |
| `Move` | 下記「アカウント引っ越し（Move）の受信」 |

### フォロー承認制（`actors.is_locked`）
Mastodon/Misskey 準拠の `manuallyApprovesFollowers`。Actor 文書に同名フィールドで出す。投稿の公開範囲には影響せず、フォローの成立に本人の承認を要求するだけ。

pending のまま留まる経路は2つ。どちらも承認待ち通知（`NotificationKind::FollowRequest`、WS `followRequest`）を送る。
- ローカル → ローカル（`follow_exec::follow_local`）: 承認まで ATP フォローもコミットしない。
- Fedi からの `Follow`: `Accept` を送らず、生アクティビティを `follows.pending_follow_activity` に保存する。

承認・拒否は `seiran_common::follow_approval::{approve_pending_follow, reject_pending_follow}`（単発API `POST /api/follow-requests/:follower_actor_id/{accept,reject}` と、承認制OFF時の一括承認 `Job::FollowRequestsBulkAccept` から呼ぶ）。承認時、フォロワーがローカルならここで ATP フォローをコミットし（`accept_and_set_rkey`）、Fedi なら保存済みアクティビティを `object` に `Accept` を送る（無ければ最小限の Follow を組み立てる）。拒否時は Fedi なら `Reject` を送り、行を削除する。

AT Protocol には非公開アカウントの概念が無いため、Bluesky 側から直接 DID をフォローされるのは防げない。

### 公開エンドポイント
`GET /users/:username`（Actor 文書）、`/users/:username/outbox`（`?page=true` で OrderedCollectionPage）、`/.well-known/webfinger`、`/.well-known/nodeinfo` + `/nodeinfo/2.0`・`/nodeinfo/2.1`、featured（ピン留め）・lists（公開リスト）。

- `GET /users/:username` はブラウザ（Accept に `activity+json`/`ld+json` を含まない）を `/@:username` へ 302 する。リモートに残る古いプロフィール記録が `/users/:username` を actor URL として持っているため（`docs/architecture.md` 8.2節）。同じ理由で WebFinger は `resource` に `https://{domain}/users/{username}` 形式も受け付ける。
- outbox と featured は匿名アクセスなので `followers_only`/`direct` を含めない（総数も同様）。添付の `Document` 化と公開 Note/Create の組み立ては `handlers::ap_collection` で共有する。
- outbox の各項目は push 配送した種別と一致させる。リポスト行は、元ポストが `ap_object_id` を持てば `Announce`（`id` = 自身の `ap_object_id`、`object` = 元ポスト、`cc` に元投稿者）、`at_uri` のみなら Fedi フォールバックと同じ本文（「🔁 author: bsky.app URL」）の `Create(Note)`。リポスト行の `body` は空なので、そのまま Note にすると push 済みの `Announce` とは別の空 Note がリモートに現れる。
- `/.well-known/nodeinfo` は `2.0`・`2.1` 両方のリンクを返す。クライアントによっては discovery の `links` を見ずに決め打ちで `/nodeinfo/2.0` を叩く実装があり（Mewk等）、`2.1` しか公開していないとそれだけで「非対応サーバー」と判定される。本体のJSONは両バージョンとも同じ内容で `version` フィールドのみ異なる。
- `/nodeinfo/2.0`・`2.1` の `metadata.features` に `"emoji_reaction"` を含める。kmyblue は既知 software 以外の絵文字リアクション対応をこれで判定するため。
- `services`（`inbound`/`outbound`、RSS/Atom等未実装のため常に空配列）と、本家 Misskey が必ず持つ `metadata.maxNoteTextLength`・`disableRegistration`・`disableLocalTimeline`・`disableGlobalTimeline`・`emailRequiredForSignup`・`enableEmail`・`enableServiceWorker`（Web Push/Service Worker未実装のため常に`false`）も同梱する。これらが丸ごと欠けていると、厳密な型のJSONデコーダを使う Misskey クライアント（Mewk 等）が必須フィールド欠落で例外を投げ、nodeinfo自体の取得に成功していても「Misskey互換サーバーではない」と判定される。`maxNoteTextLength`は配信先により実際の上限が変わる（Bsky配信あり: 300書記素、Fedi限定: 3000書記素）ため、デフォルト設定（Bsky配信あり）の上限である`300`を広告する。

**リモート nodeinfo の取得**: `jobs::remote_instance_info_resolve` が相手の `/.well-known/nodeinfo` → 本体を取得し、`software.name`/`metadata.nodeName`/`metadata.themeColor` を `remote_instance_meta` にキャッシュする（NoteCard のサーバー表示）。`themeColor` を宣言しない software（fedibird/kmyblue/mitra/akkoma/littlefedi/concrnt-ap-bridge）の代替色もここで決める。Bsky はこの経路を使わない。

- このジョブは未キャッシュのドメインにしか積まれないため、起動時タスク `backfill_remote_instance_meta` が、汎用デフォルト色（`#e4e4e7`）のまま固有色表に載った software の行と、`software_name` が NULL の行を再解決する。固有色表に無い software は対象外。
- discovery 文書は `application/jrd+json` で返す実装（concrnt-ap-bridge 等）があるため、`fetch_validated_with_accept` の `ACCEPT_JSON` にこれを含める。含めないと非対応サーバーとして恒久キャッシュされる。

### HTTP Signatures 検証
1. `Digest` ヘッダー必須（ボディの SHA-256 と一致確認）
2. `Signature` の `headers=` に `digest` を含むこと
3. `keyId` のアクターURIと `activity.actor` が一致すること
4. `keyId` から公開鍵 PEM を取得（既定1時間キャッシュ）して RSA-SHA256 検証。キャッシュ鍵で失敗したら1回だけ再取得して再検証する（鍵ローテーション対応）。PEM は `trim()` してからパースする（Pleroma は末尾に空行を付けることがあり、`rsa` クレートが拒否するため）
5. 署名者が凍結済み（`actors.suspended_at`）なら 403
6. 以降はジョブに委譲する。ジョブ側でも `activity.actor`（リレー転送では署名者と異なりうる）が凍結済みなら Follow/Create/Like/EmojiReact/Announce を破棄する

### 署名付きGET（Authorized Fetch対応）
Authorized Fetch（secure mode）のインスタンスは未署名 GET に 401 を返す。これは公開鍵取得（受信検証）にも及ぶため、対応しないとフォロー・受信・プロフィール表示がすべて失敗する。

`ApClient` の `signed_get`/`fetch_object`/`fetch_actor_signed`/`get_maybe_signed` は `(request-target) host date` の3項目で署名する（POST 用と違い digest/content-type を含めない。Misskey の `createSignedGet` と同形）。鍵は list-relay プロキシアクターのもの（`system_actor::system_signing_key`）を使う。

適用箇所（署名鍵を渡せない呼び出し元だけ未署名にフォールバック）:
- `resolve_reference`、`upsert_remote_fedi_actor`（全受信経路の送信元アクター解決）
- フォロー実行（`follow_exec::execute_follow`）、`target_resolve`、`jobs::remote_actor_resolve`、フォロー一覧の同期・ライブ取得（`remote_follow_list_sync`、`ap::collection::fetch_ap_collection_uris`、`users::fetch_remote_follow_live`）
- メンション先 inbox 解決（`deliver::infra::fetch_inboxes_by_ap_uris`）、過去ログ/featured 取得（`ap::outbox::fetch_ap_history`/`fetch_ap_featured`）、Move/alsoKnownAs（`jobs::move_actor`/`also_known_as_sync`/`also_known_as_verify`）
- 受信側の署名検証（`ApClient::verify_signature` → `get_public_key_pem`、鍵は `AppState::system_signing_key()`）

`ApActor.featured` は URL 文字列（Mastodon 等）とインライン OrderedCollection（bridgy-fed 等）の両方がありうるため `serde_json::Value` で受け、`fetch_ap_featured` が前者なら取得し後者ならそのまま使う。

### 配送
`Job::ApDelivery{actor_id, kind}`（優先度高、指数バックオフで最大10回リトライ）。基本の宛先は `follows` の `status='accepted'` かつ Fedi 系アクター（`actor_type IN ('fedi','remote_seiran')`）の `ap_inbox_url`。全 inbox へ署名付き POST をファンアウトし、1件でも成功すれば Ok（全滅時だけリトライ）。秘密鍵が未設定ならリトライしても直らないので破棄する。

**反応アクティビティ（絵文字リアクション・返信・引用・リポストとその Undo）** は、自分のフォロワーに加えて対象ポストの「会話の参加者」にも配送する（`deliver/infra.rs::resolve_conversation_broadcast_inboxes`）。対象ポストはリアクションならその投稿、返信/引用/リポストなら参照先。次の inbox の和集合:
1. 対象ポストの投稿者（Fedi の場合）とそのフォロワー
2. 対象ポストへの子ポスト（リポスト・返信・引用）の投稿者（Fedi の場合）とそのフォロワー
3. 対象ポストに付いたリアクションの reactor（Fedi のみ）

こうしないと、投稿者や会話の参加者のサーバーでブースト数・リアクション数が更新されない。DM（`deliver_direct_message_to_ap`）は対象外で、宛先は `post_recipients` のみ。

通常投稿（`PostToFollowers`）は本文でメンションした相手の inbox もフォロー関係と無関係に加える（`fetch_inboxes_by_ap_uris`）。既知なら DB から、未知ならアクター文書を取得して解決する（DBには保存しない）。`to` にもメンション先の actor URI を入れる。取得に失敗したメンション先はスキップする。

### リモートFediアクターのフォロー中/フォロワー全件取得（#68）
`GET /api/users/remote-follow-summary?actor_id=&direction=following|followers` は、`follows`（seiran が知っている関係）とは独立に、相手の `following`/`followers` OrderedCollection を `first`/`next` を辿って取得する（`ap::collection::fetch_ap_collection_uris`）。

- 同期取得は 200ms・最大500件。成功したら `remote_follow_snapshots` を上書きし、そのまま返す。
- 失敗・タイムアウト（非公開設定を含む）なら既存スナップショットを返し、`Job::RemoteFollowListSync`（低優先度、最大5000件）を積む。結果は次回表示で反映される。
- `RemoteFollowListSync` の投入は `(actor_id, direction)` ごとに API プロセス内で10分抑制する（`AppState::remote_follow_sync_recent`）。フォロー数の多いアクターを何度も開くと大量の `RemoteActorResolve` が低優先度キューを埋め、同じ優先度の `AlsoKnownAsVerify` 等が進まなくなるため。
- 各アイテムはローカルDBに `ap_uri` があれば表示名・アバター付き、無ければハンドル文字列のみ。未登録の URI には `Job::RemoteActorResolve{uri}`（低優先度）を積み、`actors` へ upsert する（フォロー関係は作らない）。
- `RemoteActorResolve` の投入は `remote_actor_resolve::should_enqueue`（プロセス内で URI ごとに1時間）で抑制する。API ハンドラとジョブの両方から積まれるため、状態はジョブモジュール側に持つ。恒久的に解決できない URI（404/410）はDBに入らないので、この抑制がネガティブキャッシュも兼ねる。
- ローカル・Bsky アクター（`ap_uri` を持たない）は対象外。HTTP エラーは「非公開」とみなして空を返す。
- フロントは `ProfilePage` がプロフィール取得直後にタブ選択を待たず先読みする（`lib/remoteFollowSummaryCache.ts`）。`FollowListPanel` はローカル把握分とリモート取得分を1つのリストに混ぜて表示し、人数は `total_count`（ローカル件数とリモート件数の大きい方、`blended_follow_count`）で上書きする。

### カスタム絵文字リアクションの送信（`EmojiReact`）
`reactions.content` の正規形は Misskey 準拠の `:shortcode@host:`（ローカルは `:shortcode@.:`、リモート絵文字は reactor のドメイン。`docs/database.md`）。

- 送信: `build_reaction_object`（`deliver/activity.rs`）が `tag: [{"type":"Emoji","name":":shortcode:","icon":{...}}]` 付きの `EmojiReact` を組み立てる。`content`/`_misskey_reaction` にはホスト付き正規形を載せるが、`tag[].id`/`tag[].name` は Misskey に合わせてホスト無しの shortcode にする（`parse_reaction_shortcode_and_host` で分離）。
- 受信: `handle_reaction` は `content` からホストを除いた shortcode で tag を照合する（`build_emoji_map`/`extract_emoji_tag_url`）。
- 画像URLは `EmojiRepository::find_url_by_shortcode` で解決し、未登録なら `INVALID_REACTION_CONTENT`/`UNKNOWN_EMOJI`。他サーバー由来のリモートホスト付き shortcode（`:name@host:`）は、`validate_reaction_content` がホストを捨てて同名のローカル絵文字として扱う（ローカルに無ければ `UNKNOWN_EMOJI`）。
- ATP はカスタム絵文字非対応のため、`commit_like` の `emoji` 拡張フィールドに正規形を載せるだけ（画像は送らない）。

### 投稿本文のカスタム絵文字
- ローカル投稿は作成時に本文の `:shortcode:` を `custom_emojis` と照合し `posts.emoji_map` へ保存する。AP 配送では本文に現れるものを Emoji tag にして Mention/Hashtag tag と併送し、`GET /notes/{id}` の Note にも同じ tag を含める（`object.id` を再取得する実装向け）。
- AP 受信は Emoji tag が第一の情報源。tag が欠落していれば同一ドメインの `remote_emojis` で補完し、それでも未解決の shortcode があれば `object.id` の Note を取得して tag を補う（リレーが埋め込み Note の tag を省く場合）。
- Jetstream 受信も本文 shortcode をローカル `custom_emojis` と照合して `emoji_map` を保存する。

### アカウント引っ越し（Move）の受信
`inbound_activity_process::handle_move`。送信側（自アカウントの引っ越し）は未実装。

- `actor`（移転元、`object` と同一でなければ無視）→ `target`（移転先）として扱う。
- 移転先のアクター文書の `alsoKnownAs`（文字列・配列どちらも可）に移転元が含まれる場合だけ処理する（なりすまし対策）。
- 移転元が `actors` に無ければ移す関係が無いので無視する。
- `find_all_local_followers_with_status` で移転元をフォロー中・申請中のローカルアクター（実ユーザーと list-relay プロキシアクター）を取得し、1件ずつ移転先へ付け替える。既に移転先をフォロー中なら移転元の行を消すだけ。そうでなければそのフォロワーとして `Follow` を送り、移転元の行を消して移転先を `upsert_pending` する。
- 実ユーザーには `moveRefollowed`（フォローし直した）または `moveAlreadyFollowing`（既にフォロー済み）通知を送る（seiran 独自。`notifier_actor_id` = 移転元、`related_actor_id` = 移転先）。
- 移転元を含むリストでは、移転元を外して移転先を加える（ATP 側の公開リスト同期は未対応）。

### プロフィールの「別のアカウント」（alsoKnownAs）
AP Move の `alsoKnownAs` の語彙を、同一人物の複数アカウントの相互リンク表示に転用した seiran 独自拡張（`actor_also_known_as` テーブル）。owner はローカルユーザー（自己登録）とリモート Fedi アクター（本人の Actor 文書の `alsoKnownAs` を取り込んだもの）。

- ローカル: プロフィール編集から `POST /api/users/also-known-as`（`target` はユーザー名/`@user@domain`/URL/DID、`resolve_and_upsert_target` で解決）、`DELETE /api/users/also-known-as/:actor_id`。上限 `MAX_ALSO_KNOWN_AS`（10件）。
- リモート Fedi: プロフィール表示のたびに `Job::RemoteAlsoKnownAsSync`（低優先度）が Actor 文書の `alsoKnownAs` を解決して同期する（自ドメインはローカルDB、`did:` は Bsky、他は Fedi アクターとして upsert。消えたエントリは削除し、既存エントリの検証結果は保持）。API からは変更できない。
- 表示: `ProfileResponse.also_known_as`（Bsky アクターのプロフィールは対象外）。
- 相互検証（✅）: 相手（ローカル・Fedi のみ）も逆向きにこちらを登録していれば `verified=true`。Bsky は DID 文書の `alsoKnownAs` がハンドル対応専用で任意 URI を列挙できないため対象外。
  - 表示時再検証: プロフィール表示のたびに、ローカル owner ならエントリごとに `Job::AlsoKnownAsVerify`、リモート owner なら `RemoteAlsoKnownAsSync`（同期後に各エントリへ `AlsoKnownAsVerify`）を積み、`verified`/`last_checked_at` を更新する。表示は常にキャッシュ値を返す（`docs/architecture.md`）。
  - ローカルのターゲットは DB で確認する。Fedi のターゲットは Actor 文書を取得し、`ApActor::claims_also_known_as`（Move 検証と共用）で owner の URI を含むか確認する。
- AP 文書への公開: `GET /users/:username` の `alsoKnownAs` に登録済みアカウントを自己申告として載せる（ローカルは自ドメインの actor URI、Fedi は `ap_uri`、Bsky は `did:...`）。検証は読み手側の責務。
- 相手が Misskey なら「設定→その他→アカウントの移行」の「移行元のアカウント」にこちらのハンドルを追加すると、相手の `alsoKnownAs` にこちらが載る。

## 3. AT Protocol (Bsky) 統合

seiran は自前の PDS を実装しており、外部 PDS（bsky.social 等）は使わない。

### 構成
- `seiran-common::atp`
  - `repo.rs` — MST 構築、TID（rkey）生成、P-256 署名の commit、CARv1、各レコードの DAG-CBOR、`subscribeRepos` フレーム（`#commit`/`#identity`/`#account`/`#error`）
  - `service.rs` — `AtpCommitService`。共通パイプライン `commit_record_inner` と `commit_post`（通常・引用・CW 共通、内容は `PostCommit`）/`commit_repost`/`commit_like`/`commit_follow`/`commit_graph_list(item)`/各種 delete/`commit_profile`
  - `plc.rs` — `did:plc` genesis operation の生成・登録
  - `did_resolve.rs` — サービス間認証 JWT 検証用の DID 解決
  - `service_auth.rs` — 外部サービス呼び出し用の自己署名 JWT（ES256、low-S 正規化必須）
- `seiran-atp-repo::firehose` — Jetstream クライアント
- `seiran-api::handlers::xrpc::{repo,server,sync}` — repo: `getRecord`/`listRecords`/`describeRepo`/`uploadBlob`、server: `describeServer`/`resolveHandle`/セッション・アプリパスワード系、sync: `getRepo`/`getBlob`/`listBlobs`/`listRepos`/`getLatestCommit`/`subscribeRepos`

ローカルユーザーの投稿は `AtpCommitService` がジョブキューを介さず直接コミットし、`atp_repo_events` に記録して、公式 Relay（`bsky.network`）へ `requestCrawl` を送る。

**CORS**: `router` の `CorsLayer` は、パスが `/xrpc/` か `/.well-known/` で始まれば全オリジンを許可する。XRPC は bsky.app 等の外部クライアントがブラウザから直接叩く公開 API だから（公式 PDS も `Access-Control-Allow-Origin: *`）。`/api/*` は `FRONTEND_ORIGIN` と自ドメインのみ。`allow_headers` は `Any`: bsky.app は `atproto-proxy`/`x-bsky-topics` 等のカスタムヘッダーをメソッドごとに送ってくるため、列挙すると増えるたびにプリフライトで弾かれる。`allow_credentials` を付けていない（Cookie 不使用）ので `Any` で安全。

### DID解決・PLC登録・ハンドル検証（アカウント登録時）
1. P-256 鍵を生成し、`did:plc:xxx` をローカル計算で確定
2. Cloudflare API で `_atproto.{username}.{domain}` TXT をセット
3. `plc.directory` へ genesis operation を POST
4. 失敗したら新しい鍵で最大3回リトライ

`com.atproto.identity.resolveHandle` は `{username}.{local_domain}` なら `actors.at_did` から即答し、それ以外は `resolve_external_handle` が DNS TXT（`_atproto.{handle}`、Cloudflare DoH）と `https://{handle}/.well-known/atproto-did` を並行で試す（各5秒、両方失敗で404）。bsky.app はログイン中の PDS に任意ハンドルの解決を投げるため、自ドメイン外を拒否するとクライアント側が壊れる。

ATP ハンドルは常に小文字（`username::to_atp_username`）。`actors.username` は大文字を許すが、genesis の `alsoKnownAs`・TXT・`resolveHandle`/`atproto-did` の応答・`#identity` はすべて小文字化した値を使う。ホスト名は経路上で小文字化されるので、大文字混じりのハンドルを `alsoKnownAs` に載せると恒久的に `handle.invalid` になる。`find_by_username_domain`/`find_did_by_username_domain` は `LOWER()` で比較する。

### MSTコミット・subscribeRepos（`commit_record_inner`）
1. アクターの `at_repo_cid`/`at_repo_rev`/`at_repo_data_cid` と署名鍵を取得
2. 既存の全レコード（`posts` + `atp_records`）をロードし、新規レコードを加えて MST を再構築
3. 新しい rev（TID）で commit を作り P-256 署名（low-S 正規化必須）
4. 差分 CAR をエンコード
5. トランザクション内で `atp_blocks`/`actors`/`atp_records`/`posts`/`atp_repo_events` を更新
6. `subscribeRepos` フレームを zstd 圧縮して `atp_repo_events.frame_bytes` に保存し、commit 後に配信（複数レプリカでは Redis Pub/Sub 経由）

`subscribeRepos` は `cursor` 指定時、`atp_repo_events` の未送信分を500件ずつ送り切ってからリアルタイム配信に移る。

> `atp_repository_publish` ジョブは enqueue する箇所が無く、使われていない。

### レコード一覧・同期系エンドポイント
Clearsky 等は firehose の代わりに PDS へ直接 `listRecords` を叩くため、次を実装している。

- `repo.listRecords` — `repo`（DID か自ドメインのハンドル）+ `collection` を rkey 順にページング（`cursor`/`reverse`/`limit` 1-100、既定50）。`app.bsky.feed.post` は `posts`、他は `atp_records` を起点に `atp_blocks` をデコードして返す。
- `repo.describeRepo` — handle/did/コレクション一覧/`didDoc`（`did:plc` なら plc.directory から取得）。
- `sync.listRepos` — `at_did` を持つアクターを id 順に返す。
- `sync.getLatestCommit` — `at_repo_cid`/`at_repo_rev`。
- `sync.listBlobs` — その DID がアップロードした `media_files` の CID。CID は保持せず `sha256` から再構築する（`cid_from_sha256_hex`）。

### postgate・getTrends のスタブ応答
- `repo.getRecord?collection=app.bsky.feed.postgate`（`get_record_postgate`）— 実レコードが無ければ合成レコードを返す。仕様上は「不在＝制限なし」だが、404 だと bsky.app が引用不可として扱うことがあるため。CID は実際に DAG-CBOR エンコードして計算する。ローカルユーザーは常に `embeddingRules: []`（postgate 作成機能が無い）。リモート Bsky の投稿は `posts.bsky_quote_disabled` から合成し、未取得なら「制限なし」。実レコードがあればそちらを優先する。
- `app.bsky.unspecced.getTrends` — 常に `{"trends": []}`。無いと `atproto-proxy` ヘッダー無しの呼び出しが 404 になる。

### セッション認証（外部ATプロトコルクライアント）
公式 Bluesky アプリ等が seiran アカウントへ直接ログインするための認証系。Misskey 互換ログイン（`sub: "local|{user_id}"`）とは別で、`LocalAuthProvider` の `generate_atp_session`/`verify_atp_access_token`/`verify_atp_refresh_token`（`auth/local.rs`）が同じ secret で HS256 署名する（`sub` が DID なので衝突しない）。

- `createSession` はメインパスワードと専用アプリパスワード（`xxxx-xxxx-xxxx-xxxx`、`atp_app_passwords` に argon2 ハッシュ）の両方を受け付ける（公式 PDS と同じ。bsky.app 自身がメインパスワードで呼ぶ）。`createAppPassword`/`listAppPasswords`/`revokeAppPassword` は seiran の通常トークンで保護する。
- どちらで認証したかをセッション JWT の `privileged` クレームに記録する（メインパスワードなら真、`refreshSession` は引き継ぐ、クレームの無い旧トークンは偽）。アプリパスワードはサードパーティに渡すものなので、偽のセッションには PLC 操作（`requestPlcOperationSignature`/`signPlcOperation`/`submitPlcOperation`）と `deactivateAccount` を許さず 403 `APP_PASSWORD_NOT_PERMITTED` を返す（`xrpc::require_privileged_session`。公式 PDS と同じ制限）。
- `createSession` は accessJwt（2時間）と refreshJwt（90日）を発行する。identifier 解決失敗やパスワード未設定でもダミーハッシュ照合をしてから同じ 401 を返し、アカウントの存否を漏らさない。応答の `email`/`emailConfirmed` は、メール登録済みなら常に confirmed（seiran は確認済みフラグを持たない）。転入元が seiran の DID 転入はこの値を使う。
- `refreshSession` — 新しいペアを発行し、古い refreshJwt の `jti` を失効させる（`atp_refresh_tokens`）。
- `deleteSession` — refreshJwt の `jti` を失効させる。
- `getSession` — did/handle を返す。
- `jsonwebtoken` の `Validation::default()` は `aud` クレームがあるだけで拒否するため、`AtpSessionClaims` の検証では `set_audience` を必ず呼ぶ。

### 書き込み系エンドポイント（外部ATプロトコルクライアント）
accessJwt で任意コレクションに書き込める。`authenticate_atp_write`（`xrpc/repo.rs`）が、検証済み DID と `repo` の一致を確認する。

- `createRecord`/`putRecord`/`applyWrites` の `app.bsky.feed.post` は `post_from_record::create_post_from_record` を通る。クライアントのレコード（`text`/`facets`/`embed`/`reply`）を `posts` に変換し、`insert_full`・ハッシュタグ・`mention_facets`・通知・Fedi 配送（`PostToFollowers`）まで行ってから、クライアントのレコードをそのまま `commit_post_record` でコミットする。seiran の投稿 API（`commit_post`）と違い embed を再構築しない。可視性は常に `public`。
  - `facets` は `atp::facets::apply_bsky_facets` で解析する（`#link` は本文の Markdown リンクへ、`#mention` は `mention_facets` へ）。
  - `embed` の blob は `resolve_blob_media_id` が CID の multihash から `sha256` を求めて `media_files` と突き合わせる。見つからない blob（他 PDS 由来等）はその添付だけ欠落させる。
  - 引用（`embed.record`/`recordWithMedia`）と `reply.parent.uri` はローカルDBの投稿に解決する（未取得なら通常投稿として保存）。ブロック関係にある相手への引用・返信は拒否する。
  - Bsky に編集機能は無いので `putRecord`/`applyWrites#update` も新規作成として扱う（既存の `at_uri` と衝突する rkey は UNIQUE 違反）。
- `deleteRecord`/`applyWrites#delete` の `app.bsky.feed.post` は `post_from_record::delete_post_by_rkey` が `DELETE /api/notes/:id` と同じ処理をする（本人以外は `NOT_YOUR_POST`、論理削除、Fedi 配送済みなら `DeleteNote`、最後に ATP から削除）。
- その他のコレクションは `commit_generic_record`/`delete_atp_record_generic` が汎用に扱う（`json_to_ipld` で DAG-CBOR 化。blob 参照は `collect_blob_cids` が再帰的に集めて `subscribeRepos` の `blobs` に載せる）。
- `applyWrites` は要素ごとに別 commit で順に処理する（仕様と異なる簡易実装。途中で失敗すると前の要素はコミット済みのまま残る）。
- `app.bsky.actor.profile` はコミットに加えて `sync_profile_to_actors` が `actors`（`display_name`/`bio`/`avatar_media_id`/`banner_media_id`）にも反映する。`commit_profile` は `actors` → レコードの片方向なので、これが無いと外部クライアントでの編集が seiran に反映されず、次に seiran でプロフィールを編集したとき NULL の `avatar_media_id` で再コミットして画像が消える。avatar/banner の blob は `resolve_blob_media_id` で `media_files` に解決する。逆方向の `commit_profile`/`encode_bsky_actor_profile` も banner を含める（DAG-CBOR のキー順は `avatar` の後、`createdAt` の前）。AP 側も Actor 文書と Update(Person) の `image` に banner を載せる。

### uploadBlob の2つの呼び出し元
`xrpc_upload_blob` は JWT で呼び出し元を区別する。まず `verify_atp_access_token`（通常セッション、HS256）を試し、通れば本人の添付アップロードとしてそのまま保存する。失敗したらサービス間認証 JWT（ES256、`iss`/`aud`/`lxm`。Bsky 動画パイプラインのトランスコード完了コールバック）として検証する。後者は、DID 本人なら誰でも自己署名 JWT を作れるため、進行中の動画ジョブ（`bsky_video_status='pending'`）が無い DID を拒否する（`store_uploaded_blob` の `require_pending_video_job`）。

保存先は seiran UI のアップロードと同じ `media_files`（`blurhash = NULL`）なので、`resolve_blob_media_id` やプロフィール同期はアップロード経路を区別しない。同じ sha256 があれば `last_uploaded_at` だけ更新する（孤立ファイルGCの生存判定用）。

### クライアント設定（`app.bsky.actor.getPreferences`/`putPreferences`）
本人の accessJwt 必須。`preferences` は MST に入らないプライベートデータで、`atp_preferences`（`actor_id` PK、JSONB）に中身を解釈せず保存する。`putPreferences` は全置換。bsky.app の年齢確認（`#personalDetailsPref`）はこれが無いと動かない。

### XRPCプロキシ（`atproto-proxy`ヘッダー）
`getTimeline`/`searchPosts`/`listNotifications` 等の AppView 専用メソッドは、PDS が AppView への透過プロキシとして振る舞うことで提供する。

- 明示ルートに無い XRPC は `xrpc/proxy.rs::xrpc_proxy_fallback` が受ける。`atproto-proxy: <did>#<service-id>` が無ければ 404（`MethodNotImplemented`）。
- `resolve_service_endpoint` が `<did>` の DID 文書から `#<service-id>` の `serviceEndpoint` を解決する。
- クライアントの accessJwt は転送せず、ユーザーの `at_signing_key_pem` で短命のサービス間認証 JWT を署名し直す（`sign_service_auth_jwt`）。`aud` はサービス DID のみでフラグメントを含めない（含めると `BadJwtAudience`）。
- 応答（ステータス・Content-Type・ボディ）はそのまま返す。

### Bsky公式Relayの新規PDSアカウント数上限
公式 Relay は未検証 PDS にホスト単位のアカウント数上限を設けており、超過分（作成順で後のアカウント）は `host-throttled` となってコミットが配信されない。PDS にはエラーが返らず `requestCrawl` も 200 のままなので、ログでは検知できない。「特定ユーザーの投稿だけ bsky.app に出ない」ときはまずこれを疑う（ロジックは indigo の `cmd/relay/relay/account.go`）。

- `https://bsky.network/xrpc/com.atproto.sync.getHostStatus?hostname=<host>` で `accountCount`/`status` は分かるが、上限値は非公開。
- 緩和の申請先は `github.com/bluesky-social/pds` の issue（例: [#357](https://github.com/bluesky-social/pds/issues/357)）。応答は不安定で、緩和されても throttle 済みアカウントは解除されない。
- `AccountTakedown` とは別症状。`public.api.bsky.app` の `app.bsky.actor.getProfile?actor={did}` で切り分ける: `AccountTakedown` ならモデレーションによる停止、PDS にプロフィールがあるのに `Profile not found` なら `host-throttled` の疑い。複数アカウントの傾向を見ると区別しやすい。

### Jetstream 経由の取り込み（`seiran-atp-repo::firehose`）
`wss://jetstream1.us-east.bsky.network/subscribe?wantedCollections=app.bsky.feed.post&wantedCollections=app.bsky.feed.like` に接続する。

- **wantedDids**: ローカルユーザーのフォロー先、またはいずれかのリストのメンバーである DID の集合を30秒ごとに確認し、変化があれば再接続する。無関係な投稿を取り込まないための必須の絞り込み。
- **凍結アクター**: 投稿の保存対象判定に `suspended_at IS NULL` を含め、リポスト・いいねも凍結済みなら破棄する。ATP には署名アクセス拒否に相当する仕組みが無いため、取り込み抑止が実質的な enforcement。
- **リーダー選出**: 複数プロセスでの重複接続を避けるため Redis の `JetstreamLeaderElector` でリース制御する。モノリスは Redis 無しでも常時接続、split-role は Redis 障害時に接続しない（フェイルクローズ）。
- **cursor**: 処理済みイベントの `time_us` を `site_settings` に5秒ごとに保存し、再接続時に引き継ぐ。
- **投稿**: 同梱の `record.text`/`record.createdAt` をそのまま使う。`embed.images`/`video`/`recordWithMedia` は CDN URL を組み立てて添付保存する。`embed.external` のうち GIF ピッカー由来（Tenor/Klipy）は `t.gifs.bsky.app`/`k.gifs.bsky.app` の MP4（Klipy は WebM）URL に変換して添付にし、それ以外は `post_link_cards`（`position=0`）に保存する。`external` には iframe 情報が無いので、保存後に `Job::LinkCardEmbedResolve`（LOW）が oEmbed discovery で `embed_src`/`embed_type` を後から埋める。`record.facets` は6節の方式で処理する。
- **GIFアニメ**: GIF ピッカー由来（上記）と、GIF の直接アップロード由来（動画パイプラインで MP4 化され `embed.video` に `presentation:"gif"` が付く）の2系統がある。どちらも `post_attachments.is_gif=TRUE` で保存し、フロントは自動再生・ミュート・ループ・コントロール無しで表示する。
- **Like**: create/delete で `reactions` を INSERT/DELETE し、通知・配信する。
- **投稿の delete**: `at://{did}/app.bsky.feed.post/{rkey}` に一致する投稿を論理削除する。URI をイベント発行元の DID から組み立てるので他人の投稿は指せない。未取り込みの投稿は無視する。
- **Repost**: `handle_inbound_repost_create` がタイムライン投稿として保存する（AP の `Announce` と対称）。対象が未取り込みなら `app.bsky.feed.getPosts` で著者ごと取得してから `repost_of_post_id` でリンクする。`at_uri` はリポストレコード自体の URI、`visibility` は `public`。delete は投稿と同じ `at_uri` ベースの論理削除（`soft_delete_by_at_uri`）。対象がローカル投稿なら通知する。
- **AppView 直接取得（`fetch_single_bsky_post`/`upsert_bsky_post`）**: リポスト対象・検索結果・ピン留め同期・「開く」で使う。新規作成時は Jetstream と同じ解析（`atp::embed::parse_bsky_embed_attachments`/`parse_bsky_embed_link_card`）で添付・URLカードを復元し、`LinkCardEmbedResolve` を積む。

### 返信許可（threadgate）・引用可否（postgate）
リモート Bsky 投稿について、閲覧者が返信・引用できるかを評価し、NoteCard のボタンをグレーアウトする。

- **取得**: `upsert_bsky_post` が新規保存直後に `fetch_bsky_gates` で同じ rkey の threadgate/postgate を取得し、`posts.bsky_reply_allow`（threadgate の `allow` 配列。不在なら NULL = 制限なし）と `posts.bsky_quote_disabled`（postgate に `#disableRule` があるか。postgate は全員可/全員不可の二値）に保存する。
- **評価**（`queries::attach_reply_quote_gates`）: `NoteResponse` 組み立て後に一括評価し `replyBlocked`/`quoteBlocked` に反映する（未ログイン時はしない）。ルールは OR、投稿者自身は常に許可。
  - `#mentionRule` — `mention_facets` に閲覧者の DID があるか。
  - `#followingRule` — `follows` に author → viewer の行があるか（firehose で取り込み済み）。
  - `#listRule` — ローカル所有のリストは即判定。リモート所有は `bsky_remote_list_membership_cache`（24時間）を見て、無ければ `Job::BskyListMembershipResolve` を積み、今回は「制限なし」として扱う（誤ってグレーアウトしない）。
- フロント（`NoteCardActions`）はボタンを無効化し「投稿者が返信（引用）を許可していません」と表示する。可視性由来の `isPrivateQuoteTarget` とは別フラグ。

### getBlob・動画パイプライン
`getBlob` は CID から sha256 を求めて `media_files` を探し、CDN URL へリダイレクトする（自前で再配信しない）。

動画・音声は原本のまま保存し、ffmpeg でメタデータとサムネイルだけ抽出する。`deliver_to_bsky=true` なら Bsky 動画パイプライン（`app.bsky.video.uploadVideo`）へ提出する。音声は Bsky に専用 embed が無いため、グレー背景の静止画＋音声の mp4 に変換して提出する。`Job::BskyVideoPoll` が完了を待ち、間に合わなければ `embed.external`（URLカード）にフォールバックする。動画付き投稿は `Job::BskyPostCommitDeferred` で Bsky コミットを遅らせ、早すぎるコミットで external に固定されるのを防ぐ。

### Bsky embed選択（#227）
ATP は1投稿に embed を1種類（画像最大4枚 / 動画1本 / 外部リンクカード1件）しか持てないため、静止画・アニメGIF・動画・本文URLのどれを Bsky 向けにするかを選ぶ（引用＋静止画は例外、下記）。

- **選択（`CreateNoteRequest.bsky_embed_choice`、`dto.rs::BskyEmbedChoice`）**: `Poll`／`Images`（非アニメ静止画グループ、最大4枚）／`Attachment{id}`（アニメGIF・動画・音声の1件）／`Url{url}`（本文中のURL）。省略すると `delivery.rs::resolve_bsky_embed` がアンケート → 静止画 → アニメGIF → 動画/音声 → 本文URL の順（同種内は先頭）で自動選択する。送らないクライアント（Misskey 互換等）があるのでバックエンドは省略を許し、「候補が複数あるのに未選択」はフロント（`PostComposer`）が送信ボタンを無効化して防ぐ。
- **アニメGIF判定**: `media_files.is_animated_image`。音声は Bsky では動画 embed になるので「動画」候補として扱う。
- **URL選択**: `resolve_bsky_embed` が選ばれた URL の OGP を同期取得して `embed.external` を組み立て、同じ内容を `post_link_cards`（`position=0`）にも保存する（oEmbed で許可されれば `embed_src`/`embed_type` も）。静止画等と違い URL は選んで初めてカードになるので、seiran の表示にも反映するため。本文から URL を消しても選択は有効なまま残る（フロントも他を選ぶまでその項目を残す）。OGP 取得に失敗しても title/description 空の External でコミットする。
- **URL選択の AP 配送への反映（`fedi_url_append_needed`）**: AP には embed が無いので、選択 URL が本文に無ければ AP 配送用の本文（`PostToFollowers.body` の上書き。`posts.body` は変えない）の末尾に `\n\n{url}` を足す（クロスプロトコル引用の `ApQuote::AppendUrl` と同じ仕組み）。引用投稿では `bsky_embed_choice` 自体を使わないので対象外。
- **引用＋静止画（`embed.recordWithMedia`）**: 引用投稿に静止画があれば、選択によらず先頭4枚を `recordWithMedia` の media として一緒に配送する（`delivery::collect_bsky_quote_images`）。動画・GIF・URL・アンケートは対象外（動画は `BskyPostCommitDeferred` との組み合わせが複雑なため未対応）。Fedi リモート引用のフォールバック（`BskyEmbed::External`）は external が media と併記できないので対象外。
- **動画パイプライン待ち**: 選択された動画/音声の `bsky_video_status` が `ready`/`failed` でなければ、`resolve_bsky_embed` は `Pending(media_file_id)` を返し、`Job::BskyPostCommitDeferred` がその1件の状態だけを見てコミットする（`ready` → Video embed、`failed` か70秒超過 → 簡易視聴ページ `/api/media/{id}/watch` のリンクカード）。
- **チェックボックスでのURLカード（`link_card_urls`、`delivery::attach_link_cards_from_urls`）**: ラジオボタン（単一選択）は「Bsky 配送オンかつ CW でない」ときだけ出る。それ以外でも本文に URL があれば、seiran は1投稿に複数カードを持てるのでチェックボックスを出し、選んだ URL を順に OGP 取得して `post_link_cards`（`position=0..N`）に保存する（Bsky 向けのサムネイル再ホストはしない）。孤児化の扱いはラジオと同じ。ラジオが出せる状態に切り替わった瞬間、チェック済みで最も前の URL が `bskyEmbedChoice` に引き継がれる。
- **AP 配送への反映（`fedi_link_card_urls_append_needed`）**: `link_card_urls` は Bsky 配送・CW の状態によらず使われるので、常に判定する。本文（引用なら `quote_body`）に無いチェック済み URL を出現順に集め、`fedi_append_url` と合わせて AP 配送用本文の末尾に改行区切りで足す。
- **投稿直後の反映**: `create_regular_post` は `deliver_regular_post` の後に `fetch_link_cards_map` でカードを読み戻して `NoteResponse.link_cards` に入れる。投稿者の即時表示とWS配信の両方でリロード無しにカードが出る。

### アンケート（#228）
Fediverse 仕様のアンケート（選択肢2〜10・単一/複数選択・期限なし/日時指定/経過時間）。Fedi 受信の Question と同じ `posts.poll`（`{multiple, options:[{name,votes}], endTime}`）・`poll_votes`・投票API（`POST /api/notes/:id/poll-vote`）を使う。

- **受理（`CreateNoteRequest.poll`）**: 選択肢2〜10件、各1〜100書記素（`validate_poll_choices`）。期限は `expiresAtIso`（絶対時刻）→ `expiresAt`（epoch ミリ秒、Misskey 互換）→ `expiresInSeconds`（相対秒）の優先順で `endTime` に変換する。DM では `POLL_NOT_ALLOWED_FOR_DM`。添付との併用は許す。
- **AP**: `deliver/activity.rs::apply_poll_to_note_object` が `object.type` を `Question` にし、`oneOf`/`anyOf` に `{"type":"Note","name","replies":{"type":"Collection","totalItems"}}` を並べ、`endTime` を設定する（受信側 `normalize_ap_poll` と対称）。フォロワーでないリモートは投稿URLを直接取得するので、`GET /notes/:id`（`get_note_ap`）も同じ変換をする。リモートからの投票は `handle_poll_vote` がローカル・リモート共通に処理する。
- **Bsky（`resolve_poll_embed`）**: ATP にアンケートは無いので、この投稿の詳細ページ（`https://{local_domain}/notes/{post_id}`）を指す `embed.external` を付ける。`title` は空（投稿の言語を決められないため）、`description` は選択肢の箇条書き（`- 選択肢A\n- 選択肢B`。作成時の得票は0で、embed は後から更新できないため得票は載せない）、`thumb` 無し。自分自身を指すカードになるので `post_link_cards` には保存しない。Bsky ユーザーはこのページで投票できない。
- **優先順位**: Bsky embed 候補の中でアンケートが常に最優先。
- **ATP 受信側（`firehose.rs::insert_or_merge_bsky_post_once`）**: `seiranPost.linkCards[]` が空のとき `embed.external` を URL カードとして保存するフォールバックがあるが、`poll` を持つ投稿ではしない。その external は必ずアンケートの代替表現だから。

#### リモートアンケートの生存監視

`posts.poll` は取り込み時点のスナップショットなので、push と pull の2経路で票数に追従する（スキーマは `docs/database.md`「リモートアンケートの生存監視」）。

- **push**: `inbound_activity_process::update::handle_update` が `Update(Question)` を受理し、送信者が投稿者本人であることを確認して、`posts.poll`・`poll_update_received=true`・`poll_fetched_at=now()` を更新する。
- **pull**: `Update(Question)` を送らない実装向け。投稿を読み込む各経路（`queries::enqueue_stale_poll_fetches`、`attach_remote_instance_info` の呼び出しすべてに隣接）が、リポスト・引用越しを含め「poll を持つリモート投稿で `poll_update_received=false`」のものを `find_stale_remote_poll_post_ids` に照会し、`Job::PollFetch` を積む。締切前は `poll_fetched_at` が10分より古いもの、締切後（`poll.closed` 優先、無ければ `endTime`）は締切以降まだ取得していないもの。締切前に取り逃した票数を締切後に取り戻せるよう、締切済みでも一律には除外しない。ジョブはシステム署名鍵で Note を再取得して更新する。`Question` でなくなっていたら `poll_fetched_at` だけ進める。
- **反映**: どちらも `broadcast_poll_update` で `pollUpdated` を配信する（ローカル投票と同じイベント）。

### CW（閲覧注意、#229）
`posts.content_warning`（Fedi 受信の CW と同じ列）。アンケートと違い、Bsky では他のすべての embed 候補を上書きする。

- **受理（`CreateNoteRequest.content_warning`、Misskey の `cw` も可）**: `validate_cw` が空文字（trim 後）禁止・100書記素以内を検証する。DM では `CW_NOT_ALLOWED_FOR_DM`。
- **AP**: `object.summary` に入れる。本文・添付・アンケート・引用は通常どおり送り、受信側クライアントが CW UI を出す。
- **Bsky（`build_cw_bsky_embed`）**: embed 候補も引用 embed も見ず、常に次の1件だけをコミットする。
  - `text` は投稿本文ではなく CW ガイド文（メンション変換・facet もガイド文に対して行う）
  - embed は詳細ページURL＋`#open_cw`、`title` は固定の `"Open"`、description/thumb 無し
  - 引用投稿でも CW を優先する（隠れた本文・添付・引用は seiran の詳細ページで見る）
  - `fedi_url_append_needed` も呼ばない

### フォロワー検知ポーリング（`seiran-atp-repo::bsky_follower_poll`）
Jetstream の `wantedDids` は発行者 DID の絞り込みなので、新たに自分をフォローしてきたアクターは事前に分からず検知できない。代わりに `app.bsky.graph.getFollowers`（認証不要）を Bsky 連携済みのローカルユーザーごとに `BSKY_FOLLOWER_POLL_INTERVAL_SECS`（既定60秒）間隔でポーリングし、`follows` との差分で新規フォローを検知する（`seiran-atp-repo::run` 内の常駐タスク）。

- **baseline**: 既存フォロワー全員が初回に「新規」として通知されないよう、`actors.bsky_followers_baseline_done_at` が NULL のユーザーは全ページを辿って無通知で `follows` に入れ、完了後にマーカーを立てる。
- **ページング**: 新しい順の前提で、baseline 済みなら既知フォロワーに達したら打ち切る（上限 `STEADY_STATE_MAX_PAGES=20`）。未 baseline は `HARD_MAX_PAGES=1000` まで。
- 未知の DID は `getFollowers` の応答（handle/displayName/avatar）で `upsert_remote_bsky` する。通知の `source_uri` は `bsky-follow:{follower_actor_id}:{local_actor_id}` で、複数インスタンスの同時ポーリングによる重複を部分ユニークインデックスで防ぐ。
- Bsky 側のアンフォロー検出は未実装。

### 生年月日（`actors.birth_date`/`birth_date_public`）
AP と ATP で可視性の位置づけが違うため、同じ値を配送先ごとに別ルールで扱う。

- **AP**: `birth_date_public=true` のときだけ Actor に `vcard:bday`（`@context` に `"vcard": "http://www.w3.org/2006/vcard/ns#"`）を含める（Misskey の `ApRendererService` と同じ表現）。既定は `false`（seiran 独自の公開設定）。Actor 文書（`actor.rs`）と Update(Person)（`build_person_object`）の両方で同じ判定をする。
- **ATP**: `#personalDetailsPref` は `birth_date_public` と無関係に常に非公開（本人の `getPreferences` のみ）。`putPreferences` で受け取ると `birth_date` を更新するが、公開設定は変えない。

### ポストの言語（`app.bsky.feed.post`の`langs`）
ISO 639-1 の2文字コード。Bsky 配送にだけ使い、AP には送らない。

- **選択肢（`CreateNoteRequest.language`）**: `SUPPORTED_LANGUAGES`（ja/en/zh/ko/es/de/fr）。表示言語（`SUPPORTED_DISPLAY_LANGUAGES`）と違い中国語は `zh` のみ。未対応は `UNSUPPORTED_LANGUAGE`。省略時は `posts.language` を NULL にし、`langs` を省略する。
- **反映**: `PostCommit.lang` → `encode_bsky_feed_post`。`Some` なら1要素の `langs`、`None` なら省略。`BskyPostCommitDeferred` でも `posts.language` を読み直して渡す。
- **フロントの既定値**: 表示言語を `i18n.postLanguageBase()` で7言語に丸めた値（`zh-Hant`/`zh-Hans` → `zh`）。配送先トグルのような「最後に送った値」方式ではない。

### アルゴリズムレコメンドからの除外（`app.bsky.actor.contentVisibilityDeclaration`）
設定画面「プライバシー」から、Discover フィード等のレコメンドから自分の投稿を除外するよう要求する。`GET`/`POST /api/account/content-visibility` が `actors.hide_from_algorithmic_recommendations` を読み書きし、更新時に `commit_content_visibility` が `app.bsky.actor.contentVisibilityDeclaration/self`（`hideFromAlgorithmicRecommendations` のみ）をコミットする。`chat.bsky.actor.declaration` と同じ「self キーの単一 boolean レコード」で、`atp_records` の有無で create/update を決める。レコードが無ければ `false` 扱い（公式仕様）。AP に対応概念は無い。

### 既存DID転入（移行元PDSとの通信）
全体の流れと設計判断は `docs/account_migration.md`。ここでは移行元 PDS（PDS A）への XRPC と SSRF 対策だけを書く。

`atp::migration_client`（AppView 用の `atp::client` とは別）が、PDS A 発行の accessJwt/refreshJwt で次を呼ぶ。

| XRPCメソッド | 用途 |
|---|---|
| `com.atproto.server.createSession` | ID/PW 認証（`create_session_with_2fa` は `authFactorToken` 対応）。`AuthFactorTokenRequired` なら `awaiting_source_2fa` へ |
| `com.atproto.sync.getRepo` | リポジトリを CARv1 で取得（`atp::car`/`mst_walk` でデコード） |
| `com.atproto.sync.listBlobs` / `getBlob` | blob 一覧と個別取得 |
| `com.atproto.identity.requestPlcOperationSignature` | PDS A の登録メールへ確認コードを送らせる |
| `com.atproto.identity.signPlcOperation` | 確認コードを添えて PLC 操作に署名させる。`rotationKeys`/`alsoKnownAs`/`verificationMethods`/`services` は省略時の挙動が仕様上不定なので、全部明示して渡す |
| `com.atproto.identity.submitPlcOperation` | plc.directory へ提出。この成功が転入の不可逆境界（DID の service endpoint が seiran に切り替わる） |
| `com.atproto.server.deactivateAccount` | 転入後に PDS A 側を無効化（ベストエフォート） |

**SSRF 対策（`atp::did_resolve`）**:
- `resolve_service_endpoint`（DID 起点）: 初めて PDS A に接続するとき。DID 文書の `service` からエンドポイントを解決し、非公開IPを拒否して検証済みIPへ接続する。
- `resolve_stored_endpoint`（保存済みURL）: それ以降すべての呼び出し。`submitPlcOperation` 後は DID 文書が seiran を指すため、DID から再解決すると PDS A ではなく seiran 自身に繋がってしまう。保存済み `source_pds_endpoint` の文字列に対して形式・スキーム・非公開IPの検証だけをする。

### 転出元API（seiranが転出元として応答する側）
他 PDS が seiran から DID を引き出すときの応答（`xrpc/identity.rs`・`server.rs`）。設計判断は `docs/account_migration.md` 6節。PLC 操作と `deactivateAccount` はメインパスワードのセッション限定（3節「セッション認証」）。

| XRPCメソッド | 用途 |
|---|---|
| `com.atproto.server.checkAccountStatus` | `activated`/`repoCommit`/`indexedRecords` 等 |
| `com.atproto.identity.getRecommendedDidCredentials` | 現在の `rotationKeys`（アカウント専用鍵＋サーバー共有鍵）等 |
| `com.atproto.identity.requestPlcOperationSignature` | 登録メールへ6桁コードを送る。SMTP 未設定・送信失敗ならコードを発行せず（発行済みなら失効させ）空応答 |
| `com.atproto.identity.signPlcOperation` | コードを検証し、アカウント専用ローテーションキーで署名して返す（提出はしない）。有効なコードが1件も無ければ検証を省く（SMTP 未設定のサーバーではメインパスワードだけが防壁になることを運営者が受け入れている前提） |
| `com.atproto.identity.submitPlcOperation` | plc.directory へ提出（不可逆境界）。`#identity`/`#account` を発火し `did_moved_out_at` を設定 |
| `com.atproto.server.deactivateAccount` | `did_moved_out_at` を設定 |
| `com.atproto.server.createSession`（`authFactorToken`） | メール2FA。SMTP 未設定なら省く |

## 4. クロスプロトコル配送ルール

中核は `seiran-api::handlers::notes::delivery`。`classify_post` が元ポストの出自を判定する: `actors.domain == local_domain` ならローカル、それ以外は `(ap_object_id 有無, at_uri 有無)` で `FediRemote`/`BskyRemote`/`LocalOrSeiran`（両方あり＝他 seiran）。

- **リプライ**: 配送可否（`resolve_reply_context` の `reply_delivery_allowed`）は分類ではなく親の実体の有無を見る。`ap_object_id` が無ければ Fedi に、`at_uri` が無ければ Bsky に送らない（ローカル投稿で Bsky 配送しなかった場合も含む）。実体の無いプロトコルへ送ると親と無関係な独立投稿になるため。親が `followers_only` ならリプライも継承する。
- **引用**: 元ポストの `at_uri`/`at_cid` が揃えば Bsky は `embed.record` でネイティブ引用する（静止画があれば `recordWithMedia`、3節「Bsky embed選択」）。Fedi のみ、または `at_cid` 未取得なら、投稿者名・本文・先頭画像（無ければアバター）を持つ `embed.external` にフォールバックする。AP は `ap_object_id` があれば Misskey 互換の `quoteUrl`/`_misskey_quote` で送り、Bsky にしか実体が無ければ bsky.app URL を本文末尾に足す。配送する Note と `/notes/:id` の Note は同じ本文・引用フィールドを返す。
- **引用受信**: AP は `quoteUrl` → `_misskey_quote` → `tag[].rel=https://misskey-hub.net/ns#_misskey_quote` の順に引用 URI を取り出し、本文に自動付加された同一投稿へのフォールバック表現を除去する。
  - 同一判定（`quote_uri_matches`）は完全一致に加え、ホストと末尾の status ID（英数字6文字以上）が両方含まれていれば同一とみなす。Fedibird は `quoteUrl` に `/users/{user}/statuses/{id}`、本文に `/@{user}/{id}` を使うことがあり、AP には別表記 URL を同一視する正規化手続きが無いため、命名規則に頼ったヒューリスティック。
  - フォールバック表現は `RE:`/`QT:`（Fedibird/Misskey）と `RE `/`QT `（kmyblue の一部）があり、位置も末尾（Fedibird/Misskey）と先頭（kmyblue 標準、`<p class="quote-inline">`）があるので、それぞれ独立に判定する。`class="quote-inline"` は `sanitize_ap_content_html` が `class` を剥がすため、サニタイズ前の HTML でしか使えない（`strip_quote_inline_paragraph_html`）。本文と同じ行にある場合は本文を巻き込むので除去しない。
  - Bsky は `embed.record.record.uri` と `recordWithMedia.record.record.uri` を取り出す。
  - 引用先がDBにあれば `quote_of_post_id` を設定し、無ければ `resolve_reference` で1段だけ取得を試みる。それでも無ければ `quote_of_ap_uri`/`quote_of_ref_status`（`pending`/`gone`）を記録して通常投稿として保存する（フォールバック行は解決の成否によらず除去する）。`inReplyTo` も同じで `reply_to_ap_uri`/`reply_to_ref_status` に記録する。1段取得したノート自身の参照はさらに辿らず、DB照合だけで記録する（無限再帰防止）。
- **リポスト**: 元ポストが `ap_object_id` を持てば Fedi へ `Announce`、`at_uri` のみなら「🔁 author: bsky.app URL」のテキスト投稿。Fedi リモート投稿を Bsky へ送るときは本文を「🔁」だけにし、元の `ap_object_id` を `embed.external` で付ける（title は元投稿者の表示名とID、description は本文、thumb は先頭画像か投稿者アイコン）。`followers_only`/`direct` は Bsky へ送らない。`Announce`/`Undo(Announce)` は元投稿者（Fedi の場合）にも送り `cc` にも入れる（相手側のブースト数・通知のため）。`Announce` の `id` は `/announces/:id`（ブラウザで開くと `/notes/:id` へリダイレクト、`docs/architecture.md` 8.1節）。
- **投稿削除**（`DELETE /api/notes/:id`、本人のみ）: `posts.deleted_at` を立てるだけで、リアクション・リポスト・通知等は消さない（読み取り側が `deleted_at IS NULL` を見る）。配送は実際に送った経路だけ: `deliver_fedi` が真で `direct` でなければ `DeleteNote`（フォロワーへ `Delete(Note)`）、`at_rkey` があれば `delete_atp_post`。DM は `DeleteNote` がフォロワー配送しか持たないため送らない（宛先には届かない、既知の制約）。

## 5. 重複排除・マージ（水際防御）

同じ投稿が複数の経路で届く場合の扱い。3つのシナリオがある。

1. **ループバック**（自サーバー投稿の逆輸入）: 受信 Note の `id`/`url` が `https://{local_domain}/notes/{id}` なら `parent_original_post_id` を設定して INSERT する（重複を許してリンク）。
2. **他 seiran サーバー間マージ**（#237）: 内部トークンは使わず、AP Note・ATP post の両方に埋め込む `seiranPost`（後述）で、その投稿が相手プロトコルで持つ真正なID（`counterpartPostId`）を相互に申告させ、両方の申告が一致したときだけ1行にまとめる。
   - **相互一致**（11節のアクター統合と同型）: 新着の実ID Y の申告が X を指し、既存行（`ap_object_id=X`）の申告（`claimed_at_uri`）が Y を指し返していればマージ（`at_uri=Y` を確定し `claimed_at_uri` をクリア）。そうでなければ `ap_object_id=NULL, at_uri=Y, claimed_ap_object_id=X` で新規行にする。一方的な申告でマージすると、他人の投稿の実IDを自分の投稿の相手として申告するだけで乗っ取れてしまう。
   - **投稿者の一致**: 両投稿の投稿者が既に同一の actor 行に解決されている場合だけマージする（`insert_remote_with_dedup`/Jetstream `save_bsky_post`）。未結婚の投稿者同士をその場で結婚させる処理は未実装で、その場合は `claimed_*` を持った孤立行として残る（後でアクターが結婚すれば再突合できる）。
   - **レース対策**: AP と ATP でほぼ同時に届くと、双方が「相手の申告はまだ無い」と判断して2行に分かれうる。`posts` の複合 UNIQUE `posts_mutual_claim_key`（`(COALESCE(ap_object_id, claimed_ap_object_id), COALESCE(at_uri, claimed_at_uri))`、`deleted_at IS NULL` の部分インデックス）で相互申告し合う2行の共存を禁じ、違反したら `unique_retry::retry_on_unique_violation` がトランザクションを最大5回やり直す（`insert_remote_with_dedup`・`save_bsky_post`・`Update(Note)` 受理の3経路）。`Update(Note)` 受理は、相手を SELECT で確認する前に自分の申告を書くとその書き込みが相手の INSERT と衝突しうるので、先に SELECT してから「マージ」か「申告の記録」かを分ける（`claim_or_find_seiranpost_merge_target`）。
   - **配送側（非対称、後から Update で補う）**: `counterpartPostId` には確定済みのIDだけを載せる。AP object id は作成時に確定するが、ATP URI は動画パイプライン等（`BskyPostCommitDeferred`）で遅れることがある。AP 配送は ATP コミットを待たずにすぐ行い、ATP URI が未確定なら `counterpartPostId` だけを欠いた `seiranPost` を送る。ATP コミットが済んだら、`counterpartPostId` を入れた `seiranPost` を持つ `Update(Note)` を AP フォロワーへ送る。ATP 側は作成時点で AP object id が確定しているので常に同梱できる。Update より先に ATP 側が届くと一時的に孤立行が2つできるが、Update の受理で解消される。
   - **`Update(Note)` の受理**: `seiranPost.counterpartPostId` を持つものだけを受理し、`claimed_at_uri` の更新とマージ再判定だけに使う（本文や CW の変更は反映しない）。送信者が投稿者本人であることを確認する。アンケートの `Update(Question)` とは別区分。
   - **マージ成立時のクリーンアップ**（2段階）: `finalize_post_merge` が1トランザクションで、削除予定行の `ap_object_id`/`at_uri` を NULL にしてから正規行に確定値をセットし（UNIQUE 制約は NULL 同士を衝突とみなさない）、削除予定行に `parent_original_post_id` と `deleted_at` を設定する。`deleted_at` によって削除予定行はすぐ表示から消え、`trg_posts_relation_counts_delete` が親投稿の返信/引用/リポスト数の二重加算を1つ分補正する。関連テーブル（`reactions`・`notifications` 等）の FK 付け替えと物理削除は `Job::PostMergeCleanup` が非同期に行う（`post_attachments`/`post_link_cards` は複合PKなので付け替えず物理削除に任せる）。このジョブが `reply_to_post_id`/`quote_of_post_id`/`repost_of_post_id` を付け替える操作はトリガーの発火条件（INSERT、`deleted_at` の NULL → 非NULL）に当たらないので、親のカウンタはジョブが手動で -1/+1 する。
   - `app.bsky.feed.post` に独自フィールドを足しても、Jetstream・AppView は未知フィールドを保持して透過する。
3. **一般ブリッジ重複**: Note の `url`（文字列・配列どちらも `extract_ap_note_urls` が扱う。Bridgy Fed は `[リダイレクタ文字列, {href:"at://...", rel:"canonical"}]`）が `https://bsky.app/profile/{did}/post/{rkey}` なら `at://` に変換して既存ポストを探し、あれば `parent_original_post_id` でリンクする。

**リモート Bsky アクターの発見**: AppView `getProfile` で得たプロフィールの保存は、firehose の未知 DID 解決・Bsky フォロー・相手解決ジョブのすべてで `seiran_actor_merge::discover_bsky_profile`（`org.seiran.actor.declaration` の取得 → 相互申告マージ込みの保存 → 昇格）を通す。バナーもここで保存する。

**リモート Fedi アクターのプロフィール**: AP Actor 文書から保存用の値（username・表示名・アバター・バナー・自己紹介・絵文字・プロフィール項目・`seiranAtDid`）を作る処理は `FediActorProfile::from_ap_actor`（`ap::client`）に集約している。`preferredUsername` が無ければエラーにする。AP 仕様は Actor URI のパス構造を規定しておらず、Misskey のように末尾が不透明な内部IDの実装もあるため、URI 末尾で代用すると誤った username になる。WebFinger（`user@domain`）経由で解決した Actor だけはその名前とドメインを使う。`inbox` の無い Actor は保存しない。

**外部指定URLへの接続（SSRF対策）**: 連合用の共有 HTTP クライアント（`net::federation_client_builder`）は、名前解決の結果に非公開IP（loopback・private・link-local・CGNAT・IPv6 ULA/link-local、IPv4-mapped/NAT64/6to4/IPv4互換に埋め込まれた非公開 IPv4 を含む）が1つでもあれば接続せず、リダイレクト先も同様に検査する。reqwest は IP リテラルのホストをリゾルバに渡さないので、`ApClient` の取得・配送（`fetch_actor`・署名付きGET・`sign_and_post`・WebFinger）は送信前に `net::ensure_public_url` で IP リテラルも検査する。対象は受信署名の `keyId`・フォロー対象・`ap/show` の `uri` など第三者が指定できるURL。E2E のスタブ（127.0.0.1）向けに限り `SEIRAN_ALLOW_PRIVATE_NETWORK=true` で無効化できる。運用者が設定する内部ホスト（`FRONTEND_ORIGIN` 等）には専用の内部通信クライアントを使う。

**Actor 解決の自ドメインガード**: `upsert_remote_fedi_actor`/`resolve_fedi` は、URI が自ドメインの `https://{local_domain}/users/{username}` なら `extract_local_username` で判定してローカル行を返す（`fedi` 行を作らない）。ローカル行は同じ `ap_uri` を持つので、ガードを通らなくても `ON CONFLICT (ap_uri)` で重複は防がれる。

**Bsky 側 Actor 解決の自 DID ガード**: `follow_bsky`/`resolve_bsky`/`fetch_bsky_profile_from_appview`/`persist_appview_posts` 等は、得た DID が `find_by_did` でローカル行に当たればそれを返し、`upsert_remote_bsky` を呼ばない。ローカルユーザーの Bsky ハンドル表記（`{username}.{local_domain}`）は他の ATP ハンドルと区別できずこの経路に入りうるが、`upsert_remote_bsky` の `ON CONFLICT (at_did) DO UPDATE` がローカル行の `username` をドット付きのハンドルで上書きしてしまうため。

### `seiranPost`拡張オブジェクト（#237）

seiran の投稿は AP の Note や ATP の post より表現力が高い（CW・投票・画像単位の NSFW・画像と動画の混在・複数URLカード等）。標準フィールドは他実装向けの互換表現として維持しつつ、同じ構造の `seiranPost` を AP Note・ATP post の両方に埋め込み、受信側 seiran はこれがあれば標準フィールドを無視してこちらから `posts` 行を組み立てる（無ければ標準フィールドから変換する）。

```jsonc
{
  "body": "変形前の生テキスト。ただしローカルメンションは @user@local_domain に完全修飾する（ドメイン省略の @user のままだと受信側で別ユーザーへのメンションになる。build_seiran_post_for_basis / build_seiran_post_for_atp_commit が convert_mentions_for_ap で変換）",
  "language": "ja" | null,
  "visibility": "public" | "unlisted" | "followers_only" | "direct",
  "contentWarning": "CW原文" | null,
  "emojiMap": { ":shortcode:": "画像URL" },
  "poll": { "...": "posts.poll のJSONBそのまま" } | null,

  "counterpartPostId": "この投稿が相手プロトコルで持つ真正なID（AP object id または AT URI）",
  "counterpartAuthorId": "投稿者が相手プロトコルで持つ真正なID（AP actor URI または AT DID）",

  "attachments": [
    { "url": "...", "kind": "image | video | audio", "isSensitive": "bool", "isGif": "bool", "mimeType": "...", "width": "int | null", "height": "int | null", "blurhash": "... | null" }
  ],
  "linkCards": [
    { "url": "...", "title": "...", "description": "...", "thumbnailUrl": "... | null" }
  ]
}
```

- 返信・引用・リポスト先は含めない。標準の参照機構（`inReplyTo`/`quoteUrl`、`reply.parent`/`embed.record`）に任せる（送信側の内部IDは受信側で解決できない）。
- DM のスレッド起点、返信・引用制限は含めない（前者は相手側で再現でき、後者はローカル投稿に設定機能が無い）。
- `linkCards[].embedSrc`/`embedType` は含めない。送信元の申告どおりに iframe を埋め込むと、受信側の `oembed_allowed_domains` を迂回した XSS の入口になる。受信側は `title`/`description`/`thumbnailUrl` を `post_link_cards` に直接入れ（`seiran_post::insert_seiran_post_link_cards`、AP/ATP 共通）、`embed_src`/`embed_type` は NULL のままにする。
- `attachments[].altText` は含めない（ローカル投稿に alt 設定機能が無い）。受信側は `is_sensitive`/`is_gif` を反映するが、`width`/`height`/`blurhash` は `post_attachments` に列が無いため反映しない。
- バージョニングはしない。seiran 同士の通信だけなので、変更は上位互換にする。

### ブリッジポスト（brid.gy等、別実体のまま保持）

Bridgy Fed 等が別プロトコルへ自動変換したコピー投稿（ブリッジポスト）は、上のマージと違い1行に統合しない。別サーバーの別実体で、リンクも片方向なので統合が確実にできず、AP→ATP/ATP→AP の組み合わせでロジックが複雑になりすぎるため。代わりに `posts` の次の列で相互にリンクする。

- `bridge_of_post_id`: ブリッジポスト行が持つ、解決済みの元ポストの id（未解決なら NULL）。
- `bridged_original_uri`: ブリッジポスト行が持つ元ポストの識別子の生値（ATP → AP のブリッジなら元の `at://`、AP → ATP なら元の AP URL）。ブリッジポストかどうかはこの列の NOT NULL で判定する（解決前は `bridge_of_post_id` が NULL のため）。
- `ap_bridge_post_id`/`atp_bridge_post_id`: 元ポスト行が持つ、AP 側/ATP 側のブリッジポストの id。seiran の投稿は両側に独立したブリッジを持ちうるので2列。

**検出**:
- AP（Bridgy Fed の Note）: `url` の `rel: "canonical"` の `href`（`at://`）を `extract_bridge_target_at_uri` が取り出す。無ければ `https://` の値を `bsky_app_url_to_at_uri` で変換する。
- ATP（Bridgy Fed の post）: 非標準フィールド `bridgyOriginalUrl`。seiran 自身の投稿なら `ap_object_id` と一致するが、`ap_object_id` と表示用URLが異なる実装では一致しないことがある（既知の制約）。

**解決**: 元ポストがDBにあれば `bridge_post::resolve_bridge_target` が即座に双方の列を設定する。無ければ `bridged_original_uri` だけ保存して `Job::FetchBridgeOriginal` を積む。また、どの投稿も新規確定時に `bridge_post::link_pending_bridges_for_new_original` で自分を待つ未解決ブリッジポストを索引（`idx_posts_bridge_pending`）から探して結ぶ（ジョブが失敗しても、元ポストが通常経路で届けば解決される）。

**検索**: 元ポストが未解決のブリッジポストは出さず、解決済みなら元ポストの id に置き換えてから重複排除する（`search::resolve_bridge_ids_for_search`）。

**表示**: ブリッジポストの詳細には「ブリッジポストです【元ポストを表示】」バナーを「リモートで表示」と並べて出す。返信・リアクションしようとすると確認ダイアログを挟む。

**リポスト・引用の配送先**（`delivery::redirect_bridge_post_meta`、`with_bridge_delivery_targets`）:
- 対象がブリッジポスト: 元ポストへのリポスト/引用として扱う（`repost_of_post_id`/`quote_of_post_id` は元ポスト）。
- 対象が対向側にブリッジポストを持つ元ポスト: 配送先の識別子（`ap_object_id`/`at_uri`/`at_cid`）をブリッジポストのものに差し替える。元ポストが `LocalOrSeiran`（両プロトコルにネイティブ実体を持つ）なら差し替えない。

### ブリッジユーザー（アクターの実ユーザーへのリンク）

Bridgy Fed はアクターも投影する。ブリッジユーザーから実ユーザーへのリンクは `actors.bridge_real_actor_id`（フロントの導線は `docs/ui_spec.md` 3節）。逆参照の列は持たない。メンション・フォローは常に実ユーザーを直接の宛先にでき（`mention.rs` が `{username}.{domain}.ap.brid.gy` をその場で組み立てる）、ブリッジポストのように対向側へブリッジ経由で送る必要が無いため。

**検出**（`jobs::bridge_user_link_resolve`）:
- AP 側（`bsky.brid.gy`、Bluesky ユーザーの投影）: `ap_uri` が `https://bsky.brid.gy/ap/{did}` なので DID を取得無しで取り出せる。
- ATP 側（`*.ap.brid.gy`、Fedi ユーザーの投影）: ハンドル `{username}.{domain}.ap.brid.gy` から username/domain を復元し、`ApClient::resolve_webfinger` で actor URI を得る。プロフィールの `bridgyOriginalUrl` は表示用URLで actor URI と一致するとは限らない（Misskey 等）ため使わない。

**解決**: 実ユーザーがDBにあればすぐリンクし、無ければ取得して upsert する（AP 側は `fetch_bsky_profile` → `upsert_remote_bsky`、ATP 側は `RemoteActorResolve` のパイプライン）。

**トリガー**: プロフィール表示のたびに、未解決のブリッジユーザーへ `Job::BridgeUserLinkResolve` を積む（表示時再検証）。ブリッジ関係は変わらないので、解決済みならジョブは即終了する。過剰な WebFinger を避けるため `RemoteActorResolve` と同じ1時間のクールダウンを設ける。

## 6. 本文中のリンク・メンション表現

Bluesky facet・AP `<a href>` のリンク情報を、Misskey 互換（`text` はプレーンテキスト）を保ったままクリック可能にするため、Misskey の MFM と同様に `text` に内部リンクマーカーを埋め込み、フロントがパースする。

### 内部リンクマーカー
`[表示テキスト](URL)`。`URL` が `/` 始まり（`//` を除く）ならフロント（`RichText`）は内部ルーティング、`https?://` なら外部リンクとして描画する。

- **Bsky `#link` facet**: `atp/facets.rs::apply_link_facets` が facet の `byteStart`/`byteEnd` の範囲を `[元テキスト](facet.uri)` に書き換えてから保存する（URL は不変なので受信時に確定）。Bsky 投稿を保存するすべての経路（Jetstream、過去ログ同期・ピン留め同期・検索結果）がこの関数を通す。AppView の `getAuthorFeed`/`getPosts` も `record.facets` を持つので `apply_bsky_post_facets` で同じ処理をする。
- **AP `<a href>`**: `ap_content_to_markdown_body` が HTML のタグを除く際、メンション以外の `<a href="URL">text</a>` を `[text](URL)` にする（ハッシュタグアンカーもここ。リモートのタグページへのリンクになる）。`<br>`/`</p>`/`</div>` は改行として残す（`tag_break_text`/`normalize_whitespace_preserving_newlines`）。タグ除去後に HTML 文字参照（名前付き・10進・16進）をデコードする。

### メンションは内部リンクマーカーで包まない
フロントの `RichText` が `@user@host`・`@handle.bsky.social` を検出して `/@...` へのリンクにするので、メンションは `@handle` のプレーンテキストで埋め込む。`[text](href)` にするとリンク先が相手の本拠地サーバーのプロフィールURLになってしまう。

- **AP Mention**: `resolve_ap_mention_text` が3段階で解決する。
  1. `<a href>` が `tag` の Mention の `href` と完全一致 → その `name`
  2. 一致しないが `<a>` の `class` に `mention`/`u-url` がある（Mastodon 等は `<a href>` に人間向けURL、`tag[].href` に actor URI を使い分ける）→ ホスト名が一致する Mention を優先し、無ければユーザー名一致へフォールバック（`find_mention_name_by_inner_text`）。同じ Note に同名ユーザーの Mention が複数ありうる（自己言及の `@yuba` と別インスタンスの `@yuba@fedibird.com` 等）ので、ユーザー名だけでは誤る。
  3. どれにも当たらないが `class` からメンションらしい → 内側テキスト（`@bob` 等、ドメイン省略がある）に送信者のドメインを補って `@bob@sender_domain` にする

  解決した `name` がドメイン省略なら `qualify_mention_name` が `tag.href` のホストを補う（Misskey は自己言及で `name` をドメイン省略で送ることがある）。`class` に `mention`/`u-url` の無い `<a>` は通常のリンクとして扱う。Fedi のハンドルはほぼ不変なので受信時に確定する。
- **Bsky `#mention` facet**: facet には DID しか無く、ハンドルは可変なので本文は書き換えず、`{byteStart, byteEnd, did}` を `posts.mention_facets` に保存し、`NoteResponse` 生成時に DID をハンドルへ置き換える（`dto.rs::apply_mention_facets`）。未解決の DID は投稿時の表示テキストのまま。
  - タイムライン等では `queries.rs::resolve_mention_facets_in_place` が全 DID を1クエリでまとめて解決する。
  - 未知の DID は能動的に upsert しない（`docs/database.md` の `bsky_actor_is_engaged`）。

### 送信（seiranユーザー投稿 → Fedi/Bsky）のメンション/リンク解決
`mention.rs` が本文中の `@...` と生URL（`http(s)://` から空白/`<>()[]` の手前まで）を配送先ごとに解決する。`@` 直前のメールアドレス判定は ASCII 英数字だけを見る（`is_ascii_alphanumeric()`）。Unicode 版だと「文章@handle」のように CJK 文字に続くメンションをメールアドレスとみなしてしまう。

DID 解決は常に公開 AppView（`public.api.bsky.app` の `getProfile`/`resolveHandle`）を使う。`bsky.brid.gy` は `resolveHandle` を実装していないので使わない。

- **Bsky 向け（`convert_mentions_for_bsky`）**:
  - 生URL → テキストはそのまま、`facet#link` を付ける。
  - `@username`（ローカル） → `@username.{local_domain}` に展開し、DID が取れれば `facet#mention`。
  - `@username@{local_domain}` → 上と同じく `@username.{local_domain}` に変換する（Fedi 表記のまま Bsky に出さない）。
  - `@handle.tld` → テキストはそのまま。`.{local_domain}` ならローカルとして、そうでなければ AppView で DID を解決して `facet#mention`。
  - `@user@domain`（他ドメイン） → まず `actors` を引き、`at_did` を知っている相手（結婚済みの `remote_seiran`）ならその DID を使う。知らなければ brid.gy ハンドル（`{user}.{domain}.ap.brid.gy`）で DID 解決を試み、それも失敗したらテキストはそのまま `facet#link`（既知なら `actors.ap_uri`、未知なら `https://{local_domain}/@user@domain`）。
- **AP 向け（`convert_mentions_for_ap`）**: `(変換後テキスト, Vec<ApInlineMention>)` を返す。各スパンは `href`・表示名・`is_mention`（`tag[]` に載せるか）を持つ。
  - 生URL → テキストはそのまま、`is_mention: false` のリンク。
  - ローカル `@username` → `@username@{local_domain}` にし、`https://{local_domain}/users/{username}` への Mention。
  - `@username.{local_domain}` → brid.gy を試さず、上と同じ Mention にする（Bsky 表記のまま Fedi に出さない）。
  - `@user@domain` → テキストはそのまま。DB の `ap_uri` か WebFinger で href が取れたときだけ Mention。
  - `@handle.tld`（他の Bsky ハンドル） → brid.gy の WebFinger（`acct:{handle}@bsky.brid.gy`）で解決できれば `@handle.tld@bsky.brid.gy` の Mention、できなければ `bsky.app/profile/{handle}` へのリンク。ブリッジは相手が brid.gy 連携を有効にしていないと存在しないので、後者は珍しくない。
  - `deliver/text.rs::plain_to_html_with_mentions` がスパンを `<a>` にし（Mention だけ `class="mention u-url"`）、Mention を `tag[]` にも加える。push 配送（`override_body` 未指定時）と `get_note_ap` の両方でこれを共有する。リポストのフォールバック本文（`override_body` 指定時）はメンション変換しない。

### Bsky向け本文の文字数上限
`app.bsky.feed.post` の本文は300書記素・3000バイトまで。メンション変換でテキストが伸びるので、`create_regular_post` は INSERT 前に `convert_mentions_for_bsky` を実行し、変換後テキストで上限を検証する（超過なら `TEXT_TOO_LONG`）。Bsky に送らない場合は入力テキストに対する緩い上限（3000書記素・10000バイト）だけを見る。

### 既知の制約
- ローカル投稿の `@mention` は `posts.body` 自体を書き換えない（配送用コピーだけ変換する）。表示はフロントの `RichText` がプレーンな `@handle` を検出して補う。
- 本文にたまたま `[text](url)` 形式の文字列があるとリンク化される（許容）。
- 送信時の生URL自動リンク化には対応しているが、手書きの `[text](url)` はリンク化しない。

### ハッシュタグ
ハッシュタグはポストと m:n の永続オブジェクト（`hashtags`/`post_hashtags`）で、ハッシュタイムライン（`GET /api/hashtags/:name/timeline`）の主軸にする。

- **送信**: `convert_mentions_for_bsky`/`convert_mentions_for_ap` は `#` もスキャンする（`scan_hashtag`。境界・除外ルールは `extract_hashtags` と同じで、表示用なので大文字小文字は保つ）。
  - Bsky: 出現位置ごとに `facet#tag`（`tag` は `#` を除いた本体）。
  - AP: `<a href="https://{local_domain}/tags/{正規化タグ}" class="mention hashtag" rel="tag">#タグ</a>` と `tag[]` の `{"type":"Hashtag",...}`。push と `get_note_ap` で `ap_inline_mentions_to_tag_json` を共有する。
- **受信の分類**: Mastodon 等はハッシュタグアンカーにも `class="mention hashtag"` を付けるので、`class` だけでメンション判定すると `#foo` が `@#foo@sender_domain` に化ける。`rel="tag"` か `class` の `hashtag` を見たら、メンション解決より先にハッシュタグとして通常リンク（`[#foo](url)`）にする。
- **抽出**: 出自を問わず、最終的な `posts.body` を `hashtag::extract_hashtags` で1回スキャンし `HashtagRepository::link_post` でリンクする。AP 由来のアンカーも `[#foo](url)` のリンクテキストに `#foo` が残るのでこれで拾える。Bsky の `facet#tag` は本文に `#foo` があるので参照しない。
- **表示**: フロントの `RichText` は `#foo` とリンクテキストが `#タグ` のMarkdownリンクの両方を、自インスタンスの `/tags/foo` へのリンクにする。
- **ホームへの追加**: `pinned_hashtags` に保存し、ホームのフィードタブとして出す。

### seiran Web UIでのリッチ表示（`content_html`）

リモート Fedi 投稿は、`body` とは別に `sanitize_ap_content_html` が `Note.content` の構造を保ってクレンジングした `posts.content_html` を持つ。`body` は Misskey 互換 API・Bsky 配送・検索・ハッシュタグ抽出の前提なので変えない。`content_html` は seiran Web UI の表示専用で、フロントは値があれば `RichHtml`、無ければ `body` の `RichText` で描画する。

- **許可タグ**: `br p div a b i s code pre blockquote ruby rt rp h1 h2 h3 figure img ul ol li small center`。
- **許可属性**: `a` は `href`、`img` は `src alt width height`、全タグで `style` の `text-align: left|right|center|justify` だけ（他のCSSがあれば属性ごと除去）。`class` は除去。`href`/`src` は `http`/`https` のみ（`ammonia`）。`rel`/`target` は保持せず、フロントが `target="_blank" rel="nofollow noopener noreferrer"` を固定で付ける。
- **メンション/ハッシュタグの `<a>`**: `rewrite_mention_hashtag_hrefs` が `body` と同じ解決ロジックで `href` だけを内部パス（`/@user@host`、`/tags/{小文字化タグ}`）に書き換える。`RichHtml` は `/@`・`/tags/` 始まりをアプリ内遷移にする。
- **MFM 装飾関数**: Misskey の HTML 変換で `blur`/`spin` 等はすべて `<i>` に縮退するので区別できない。`ruby` だけは `<ruby><rt>` になるので許可している。
- **引用フォールバック行の除去**: `body` と同様に `content_html` でも除去する（`save_ap_note_core`）。
  1. `strip_quote_inline_paragraph_html`（サニタイズ前の HTML で `<p class="quote-inline">` を除く。kmyblue の先頭パターン）
  2. `strip_quote_fallback_line_html_leading`（先頭ブロックをテキストで判定。class の無い kmyblue 系）
  3. `strip_quote_fallback_line_html`（末尾ブロック。Fedibird/Misskey と kmyblue の末尾パターン）

  行の区切りは直近の `<br>` と `<p>`（先頭版は `</p>`）の近い方で近似する。`<br>` だけを見ると、段落だけで区切られた投稿で本文全体を1行と誤認して消してしまう。マーカーは `RE:`/`QT:` と `RE `/`QT ` の両方（`starts_with_quote_marker`）、一致判定は `quote_uri_matches`。URLカードの抽出はこの除去後の `body` に対して行うので、除去に失敗すると引用とURLカードが二重に出る。
- **制約**: 元の HTML は保存しないので、この機能より前に受信した投稿の `content_html` は NULL のまま。ローカル・Bsky 投稿も常に NULL。

## 6.1 投稿検索とBluesky AppView

`GET /api/notes/search` は、初回検索でローカルDBと AppView の `app.bsky.feed.searchPosts` から `limit` 件ずつ取り、AppView の結果は author を `actors`、投稿を `posts` へ upsert してから、ローカル結果と ID 降順でマージ・重複排除して `limit` 件返す。AppView 障害時はローカル結果だけにする。

Misskey 向けの `POST /api/notes/search` も同じ `search::search_post_ids_by_cursor`（ブレンド・ブリッジポスト解決）と検索回数制限（`check_search_rate_limit`、初回のみ）を使い、`query`/`limit`/`sinceId`/`untilId` を受け付ける。`POST /api/notes/search-by-tag` は `tag`/`limit`/`sinceId`/`untilId` でハッシュタイムラインを返す（Aria 等）。

`until_id` 指定時は対象の `created_at` を AppView の `until` に渡し、DBにも `p.id < until_id` を適用してブレンドする。`since_id` は Misskey 互換の逆方向ページングで、AppView を使わずDBの `p.id > since_id` だけを返す。既存フロントの過去掘りは `session_id` バッファも使える。

## 7. Misskey API 互換レイヤー

`middleware::misskey_auth_bridge` は、`Authorization` ヘッダーが無ければ JSON ボディ/クエリの `i` から `Authorization: Bearer` を合成する。`handlers::misskey`（`endpoints.rs`/`convert.rs`/`types.rs`）が Misskey 形式のエンドポイントを提供する。データ取得・検証・副作用はカスタム API と共通の関数を使い、Misskey 側はレスポンス整形だけを持つ（`docs/coding_rules.md` 2節）。`POST /api/drive/files/create` は multipart なのでブリッジの対象外で、ハンドラが multipart の `i` を読む（misskey_dart の `postWithBinary` はトークンを multipart フィールドで送る）。

**対応エンドポイント**: `meta`、MiAuth、`i`、`users/show`（`userIds` 指定時は配列、`userId`/`username` 指定時は単一。`UsersShowResponse` の untagged で切り替え）、`users/notes`、`users/following`・`followers`（`MisskeyFollowRelation` が `follower`/`followee` の片方だけを出す）、`users/reactions`、`users/lists/list`・`show`、`notes/show`・`create`・`reactions`・`reactions/create`・`delete`・`unrenote`・`mentions`・`search`・`search-by-tag`・`polls/vote`、`notes/local-timeline`・`timeline`・`hybrid-timeline`（ソーシャル）・`global-timeline`・`user-list-timeline`、`following/create`・`delete`、`i/notifications`、`ap/show`、`stats`、`endpoints`、`emojis`（GET/POST）、`drive/files/create`・`files`・`files/show`・`folders`、`notifications/create`。カスタム API と同じパスの `GET` とはメソッドで共存する。

**ドライブ（`drive/files`・`files/show`・`files/update`・`files/attached-notes`・`folders`・`folders/show`・`folders/create`）**: seiran のドライブはフォルダ階層を持たない（`media_files` に `folder_id` が無い）フラットな構造のため、`drive/files` は `folderId` の指定を無視し、常にアップロード者自身の全ファイルをカーソルページネーションで返す（Mewk作者と合意済みのいい加減な互換実装：「存在しないフォルダ」ではなく「指定されたフォルダの中身は常にルート＝全ファイル」として振る舞う）。`drive/folders` は常に空配列（ルート直下にフォルダは無い）。`drive/folders/show` は何を指定されても`{id: 指定されたfolderId, name: "Drive", parentId: null}` 相当のダミーを返して常に成功、`drive/folders/create` も何も永続化せずその場で生成したIDを含むダミーの`DriveFolder`を返して常に成功する（フォルダは持てないが、作成・参照そのものを失敗させるとクライアントの添付整理フローが止まるため）。`drive/files/update` も同じ方針で、`folderId`（フォルダ間移動）・`name`・`isSensitive`・`comment`はいずれも保存先が無いため無視し、ファイルの存在・所有権だけ検証して現在の`DriveFile`をそのまま返す（Aria等の「ファイルをフォルダ移動」操作を404にせずルートに留め置く）。`drive/files/show`・`files/update`・`files/attached-notes`はいずれもアップロード者本人のファイルのみ対象。`drive/files/attached-notes`は`post_attachments`を実際に引いて添付投稿を返す（ここは本物のデータで実装）。

**`notifications/create`**: 本家 Misskey はクライアント発の自由記述通知（`body`/`header`/`icon`）をユーザー自身に送れるが、seiran の `notifications` テーブルは固定種別のシステム通知しか持たないため、内容を検証せず受理するだけで何も保存せず `204` を返す。

**認証（MiAuth、`handlers::miauth`）**: third-party クライアントが `GET /miauth/:session_id` を開く → SPA の `/connect/:session_id` へ 303（callback URL は `is_valid_callback` で https+パブリックホストかネイティブ URI スキームのみ許可）→ SPA がログイン中ユーザーの Bearer 認証で `POST /api/miauth/:session_id/authorize` を呼び認可成立 → クライアントが `POST /api/miauth/:session_id/check`（パスベース）または `POST /api/miauth/check`（ボディベース、seiran 独自フロント用）で一度きりトークンを取得する。認可待ちセッションはプロセス内メモリ（`AppState.miauth_sessions`）。

**認証（旧来の app 認証フロー、`handlers::misskey::app_auth`）**: SocialHub Web 等、MiAuth 非対応の Misskey クライアント向け。`POST /api/app/create` でアプリを登録（`oauth_apps` を Mastodon 互換 OAuth と共用、`client_secret_hash` に `UNIQUE` 制約を持たせ `appSecret` 単体で引ける）→ `POST /api/auth/session/generate`（`{appSecret}`）が `misskey_auth_sessions` にセッションを発行し `{token, url}` を返す（`url` は `/auth/:token`）→ third-party クライアントが `url` を開くと SPA の `/misskey-connect/:token` へ 303（アプリ名は `GET /api/auth-sessions/:token` でサーバー登録内容から表示、URL クエリは信用しない）→ 承認で `POST /api/auth-sessions/:token/authorize`（Bearer 認証）→ `POST /api/auth/session/userkey`（`{appSecret, token}`）が承認済みセッションを一度きり消費してトークンを発行する（`generate_app_token`、`app_tokens` に記録）。認可待ちセッションは MiAuth と異なり DB 永続化（`oauth_authorization_codes` と同じ理由、サーバー再起動をまたぐ必要があるため）。

**`notes/create`**: カスタム API の `create_note` をそのまま使う。`CreateNoteRequest` は `fileIds`/`replyId`/`renoteId`/`visibleUserIds` を `#[serde(alias)]` で受ける。`visibility` は Misskey 語彙（`public`/`home`/`followers`/`specified`）も受け、`normalize_misskey_visibility` が seiran 語彙に正規化する（応答側の `to_misskey_visibility` と対称）。

**`notes/mentions`**: Aria の通知画面の「メンション」（`visibility` 省略）と「指名」（`specified`）タブ。seiran は本文中のメンションを永続化していないので、`mentions_timeline` は `notifications`（`mention`/`reply`）と `post_recipients`（DM の宛先）の和集合で代用する。DM は本文に `@username` を要求しないため、新規 DM の最初の1通は `notifications` に出ず、`post_recipients` を別に見る必要がある。「指名」は `post_recipients` 側だけ。

**`following/create`・`delete`**: カスタム API（`create_follow`/`delete_follow`）へ `userId` を `actorId` として委譲する。`actorId` の宛先は `ap_uri` を `at_did` より優先し、両方を持つリモート seiran アクターはプロフィール画面と同じく AP フォローで成立させる（ATP 側は相手サーバーの相互処理に任せる）。応答は `204` ではなく対象の `UserLite`（misskey_dart が戻り値を Map として直接キャストするため）で、`misskey_user_lite_response` が `build_user_detailed(state, actor, None).lite` を返す。`following/invalidate`・`update` は未実装。

**`notes/polls/vote`**: カスタム API の `vote_poll` に `option_indexes: vec![choice]` として委譲し、204 を返す（Misskey は複数選択でも1回に1つの `choice` を送る）。

**`ap/show`**: 「ほかのアカウントで開く」。「開く」機能と共通の `open_target::resolve_open_target` で `uri`（AP ID・URL・`@user@host`・AT URI 等。未取得なら取り込む）を解決し、`{"type": "Note"|"User", "object": {...}}`（`ApShowResponse`）で返す。

**`users/lists/list`・`show`、`notes/user-list-timeline`**: カスタム API と同じ `ListRepository` と公開範囲チェック（非公開リストは所有者のみ、`NO_SUCH_LIST`）。`list` は `userId` 指定時はその人の公開リスト、省略時は自分の全リスト。`userIds` は0件でも `[]` を返す（misskey_dart が直接キャストする）。リストを開いた画面は `show` と `user-list-timeline` の両方を呼ぶ。

**`users/reactions`**: `reactions_by_actor_for_feed`（カスタム API と共通、可視性フィルタ済み）でリアクション一覧を取り、対象ノートを `fetch_referenced_notes` で埋め込む。対象ノートが取れない行は除外する。`note` を持つ `MisskeyUserReaction` を使う（`notes/reactions` 用の `MisskeyNoteReaction` とは別）。

**`notes/reactions`**: `type` 省略時は空配列（集計が単一絵文字指定前提のため）。`notes/reactions/create` の `reaction` はそのまま `validate_reaction_content` に渡す（内部表現が Misskey と同じ `:shortcode@.:` なので変換不要。リモートホスト付きはホストを捨てて同名のローカル絵文字として扱う）。

**`stats`**: `notesCount`/`usersCount`（と同値の `original*`）はローカルの実数（削除済み投稿・退会済みユーザー・リモートを除く）。`instances`・`driveUsage*` は0固定。

**未実装機能のスタブ**: `announcements`、`users/featured-notes`・`clips`・`pages`・`flashs`・`gallery/posts` は `empty_list_stub`（常に `[]`）。エンドポイントが無いと Aria が該当タブをエラー表示にするため。実装するときは個別ハンドラに差し替える。

**`endpoints`・`emojis`**: `endpoints` は実装済み API 名を返す。Aria はここに `emojis` があるときだけ `POST /api/emojis` を呼ぶので、`GET /api/emojis` と同じ `fetch_public_emojis` を GET/POST で返す。

**`drive/files/create`**: `DriveFileResponse` は seiran 独自のフィールドに、misskey_dart の `DriveFile.fromJson` が必須とする `createdAt`/`name`/`type`/`md5`/`isSensitive`/`properties{width,height}` を同居させた1つの型。`thumbnailUrl` は本体の `url`（縮小版を持たない）。ファイル名は独立した multipart フィールド `name` を優先する（`postWithBinary` は Content-Disposition に filename を付けない）。

**`MisskeyNote`**（`convert::to_misskey_note`）:
- `uri` は Misskey に合わせローカルでは常に `null`。seiran はローカル投稿にも `ap_object_id` を持つので、出自は `domain == local_domain` で判定する。`url` は AP 優先、無ければ bsky.app URL。ローカルは両方 `null`。
- `emojis` は `posts.emoji_map` と `actors.emoji_map` を統合する（AP 投稿では本文の絵文字が actor 側の map にしか無いことがある）。
- `renote`/`reply` は `renoteId`/`replyId` から対象を一括取得して埋め込み（`embed_referenced_notes`、可視性は `find_visible_posts_by_ids`）、孫階層まで1回だけ追加で埋める。ID だけではクライアントが「削除されたノート」と表示するため。孫まで必要なのは、引用ポストの単純リポストでリポストが1階層を消費するから。ひ孫は埋めない。
- `cw` は `posts.content_warning`、`files[].isSensitive` は `post_attachments.is_sensitive`。
- `poll` は `to_misskey_poll` が `{expiresAt, multiple, choices:[{isVoted,text,votes}]}` に変換する（無ければ `null`）。`isVoted` は `poll_votes` から一括取得する。

**`MisskeyUserDetailed`**:
- misskey_dart は `isFollowing` キーの有無で関係情報あり/なしの型を切り替える。閲覧者が分かる場合（`users/show`・`following`・`followers`）だけ `MisskeyUserRelations` を `#[serde(flatten)]` で出し、8つの関係フィールドはどれも null にしない。`hasPendingFollowRequestToYou` は常に `false`。`i`（`build_me_detailed`）には関係フィールドを出さない。
- `uri`/`url` は `remote_user_uri_url` が AP 優先、無ければ bsky.app で組み立てる（ローカルは `null`）。Aria はこれでリモートユーザーのバナーを出す。
- `followersVisibility`/`followingVisibility` は常に `"public"`（未対応の設定。欠けるとクライアントが数を鍵アイコンにする）。
- ローカルの解決済みアバターが空なら `https://{LOCAL_DOMAIN}/api/avatars/{actor_id}`（`?v=5`、PNG）を返す。リモートの未設定アバターは代替しない。

**misskey_dart の直接キャスト**: 生成コードは必須フィールドを `as String` 等で直接キャストするので、キーの欠落や `null` でクライアントが落ちる。互換型を追加・変更するときは必須/任意を misskey_dart のソースで確認する。`md5` は `sha256` で代用し、元データが無ければ空文字列/0 を返す。

**通知の `user.avatarUrl`**: ローカルユーザーは `avatar_media_id → media_files → storage_providers` から解決する（`actors.avatar_url` はリモート用の生URL）。

**`meta` の `mediaProxyUrl`**: `site_settings.media_proxy_url` が未設定なら `https://{local_domain}/proxy`。空だと Aria が `{mediaProxyUrl}/image.webp?url=...` で不正なURLを組み立てる。値は `/proxy` まで含む完全なエンドポイントで、seiran のフロント（`utils/mediaProxy.ts`）も `{mediaProxyUrl}?url=...` とそのまま使う。

**代替アバターの ATP blob**: `app.bsky.actor.profile.avatar` は実在する blob を要求するので、`avatar_media_id` が無いローカルユーザーには `fallback_avatar_atp_blob`（`avatar.rs`）が生成 PNG の SHA-256 から CID を作って参照する。PNG は保存せず、`getBlob` で要求 CID が再生成結果と一致したらその場で返す（`actor_id` から決定論的に生成するので CID は安定する）。`ATP_BACKFILL_UNSET_AVATAR_PROFILES_ONCE=1` は、この仕組みより前に avatar 無しでコミットされたプロフィールを一度だけ再コミットする起動オプション。

**既知の非互換**: 書き込み系のエラー形状は Misskey のエラーID体系を再現していない。

## 7.1 Mastodon API 互換レイヤー

`handlers::mastodon` が Mastodon REST API（`/api/v1/*`・`/api/v2/*`・`/oauth/*`）を提供し、Tusky・Ice Cubes・Elk・Phanpy 等のクライアントからタイムライン・ハッシュタグ・検索・投稿/アカウント詳細の閲覧と、投稿・返信・引用・お気に入り・リポスト・フォロー・ブロック/ミュート・プロフィール編集・投票・ピン留め・ブックマーク、ストリーミングでの新着受信をできるようにする。Misskey 互換と同じく、取得・検証・副作用はカスタム API と共通の関数（リポジトリ関数、`create_note`・`create_reaction`・`remove_reaction`・`delete_repost`・`delete_note`・`create_follow`・`delete_follow`・`create_drive_file`・`resolve_open_target`・`search_post_ids_by_cursor`）を使い、Mastodon 側は入力の解釈と応答整形だけを持つ。書き込み系は既存ハンドラを呼んだ後、対象を読み直して `Status`/`Relationship` を返す（投稿作成の応答からは `id` だけを取り出す）。

**認証（OAuth 2.0 Authorization Code、PKCE 対応）**: `POST /api/v1/apps` で登録 → ブラウザで `GET /oauth/authorize`（クライアントと `redirect_uri` の完全一致を検証して SPA の `/oauth-connect` へ同じクエリで 303）→ SPA が `GET /api/oauth/apps/:client_id` で登録済みのアプリ名を表示し（クエリのアプリ名は信用しない）、承認で `POST /api/oauth/authorize`（通常の Bearer 認証）が認可コードを発行してリダイレクト先を返す → クライアントが `POST /oauth/token` で交換する。`urn:ietf:wg:oauth:2.0:oob` ならコードを画面に表示する。トークンは MiAuth と同じ無期限 JWT（`generate_app_token`）で `app_tokens` に記録するので、既存の `extract_auth` がそのまま検証し、設定画面の連携アプリ一覧から無効化できる。`POST /oauth/revoke` は発行元アプリ自身のトークンだけを失効させる。交換時はクライアントシークレットか PKCE ベリファイアのどちらかが必須（PKCE で認可したならベリファイアが必須）。登録できる `redirect_uri` は `https`・ループバックの `http`・ネイティブアプリのカスタムスキーム・OOB（外部ホストの平文 `http` とスクリプト実行系スキームは拒否）。`grant_type` は `authorization_code` のみ（`client_credentials`・`password` は未対応）。scope は記録するが強制しない（MiAuth と同じ）。

**入力（`extract.rs`）**: Mastodon クライアントは同じパラメータを JSON・`x-www-form-urlencoded`・multipart・クエリ文字列のどれでも送り、配列を `media_ids[]=1`、入れ子を `poll[options][]=a` と書くので、`MastodonParams`/`MastodonQuery` がすべて `serde_json::Value` に正規化してからデシリアライズする。フォーム由来で文字列になった数値・真偽値・ID は `lenient::*` で受ける。

**ページネーション**: `max_id` → `until_id`、`since_id`・`min_id` → `since_id`（`min_id` の「直後のページを古い側から」は再現せず、差分が `limit` を超えると間が抜けてクライアントの「さらに読み込む」で埋める）。一覧は `Link` ヘッダー（`rel="next"` に `max_id`、`rel="prev"` に `min_id`）を付ける。カーソルは変換・絞り込み前の行 ID で作る（リポスト元が見えないリポストの除外や `exclude_replies` で行が減っても次ページがずれない）。フォロー一覧はアカウント ID ではなく `follows.id` をカーソルにする。

**エラー**: `mastodon::error_shape`（`route_layer`）が `ApiError` の応答を `{"error": "CODE"}` に書き換える（クライアントは `error` を文字列として読む）。既に `error` が文字列の OAuth エラー（`{"error", "error_description"}`）は通す。

**CORS**: `/api/v1/*`・`/api/v2/*`・`/oauth/token`・`/oauth/revoke` はブラウザ上の Web クライアント（任意のオリジン）から直接叩かれるので、`/xrpc/*` と同じくオリジン制限の対象外（Bearer 認証だけで Cookie を使わない）。SPA 専用の `/api/oauth/*` は対象外にしない。

**`Status`（`convert::to_status`）**:
- `content` はリモート Fedi 投稿なら `content_html`（内部パスの `href="/..."` を `https://{local_domain}/...` に戻す）、それ以外は `body` を `text_to_html` で HTML にする（生 URL・内部リンクマーカー `[text](url)`・`@メンション`（`https://{local_domain}/@handle` へ）・`#タグ`（`/tags/{小文字}` へ、`class="mention hashtag"`）をリンクにし、空行で `<p>`、改行で `<br>`）。URL・ハッシュタグの走査は送信時と同じ `mention::scan_url`/`scan_hashtag`。
- `uri` はローカルなら `https://{local_domain}/notes/{id}`、リモートは AP ID か AT URI。`url` はローカルは同じ URL、リモートは AP ID か bsky.app URL。
- 可視性は `unlisted`→`unlisted`、`followers_only`→`private`、`direct`→`direct`（投稿時は逆変換）。タイムラインは DM を含めない（`exclude_direct = true`）。
- **お気に入り（favourite）は `❤️` リアクション**（Bsky の like・AP の `Like` も受信時に `❤️` になる）。`favourited` は自分が `❤️` で反応済みか、`favourites_count` は絵文字を問わないリアクション総数（Mastodon クライアントには絵文字リアクション欄が無いため）。お気に入りは既存リアクションを `❤️` に置き換え、取り消しは `❤️` だけを外す。
- リポストは `reblog` に元投稿、引用は Mastodon 4.5 の `quote: {state, quoted_status}`（見えない引用元は `state: "deleted"`）。引用ポストのリポストのために2階層目の引用まで埋める。リポスト元が見えないリポストは空の投稿になるので一覧から除く。`quote_approval.current_user` は公開・未収載なら `automatic`、それ以外は `denied`（未ログインは `unknown`）。
- `in_reply_to_account_id` は返信先を可視性判定つきで一括取得して埋める。`mentions` は常に空（seiran は本文中のメンションを永続化していない）。`card` は最初の URL カード。添付の `description` は常に `null`（代替テキストを保存しない）。

**`Account`**: `acct` はローカルが `username`、Bsky（`domain` 空）がハンドル、他は `username@domain`（`username::actor_handle` と同じ規則）。アバター未設定は `/api/avatars/:id`、ヘッダー未設定は `/api/headers/missing.png`（1x1 透明 PNG。空文字だと Swift 系クライアントの URL デコードが失敗する）。`note`・`fields[].value` も `text_to_html`。

**投稿（`POST /api/v1/statuses`）**: `status`・`in_reply_to_id`・`quoted_status_id`（Fedibird 等の `quote_id` も可）・`media_ids[]`・`poll[options][]`/`poll[expires_in]`/`poll[multiple]`・`spoiler_text`（CW）・`visibility`・`language`（`SUPPORTED_LANGUAGES` 外は付けない）を `CreateNoteRequest` に変換する。`direct` の宛先は本文のメンションを既知のアクター（`@user`→ローカル、`@user@host`、`.` を含む単独ハンドル→Bsky）に引き当てる（引き当たらなければ `DIRECT_REQUIRES_KNOWN_MENTION`）。`sensitive`（ローカル投稿の添付に閲覧注意を付けられない）・`scheduled_at`・`Idempotency-Key` は無視する。削除（`DELETE /api/v1/statuses/:id`）は下書き復元用に元の本文を `text` に入れて返す。

**その他のエンドポイント**:
- インスタンス: `GET /api/v1/instance`・`/api/v2/instance`（`version` は `4.5.0 (compatible; seiran x.y.z)`、`api_versions.mastodon` は7。クライアントはこれで引用 UI 等の機能有無を決める）、`custom_emojis`、`preferences`。
- タイムライン: `timelines/home`・`public`（`local=true` でローカル、それ以外はグローバル）・`tag/:hashtag`・`list/:id`、`tags/:name`、`lists`・`lists/:id`。
- 閲覧: `statuses/:id`・`context`（祖先は返信先を最大40件たどる。子孫は `thread_descendants` のうち返信でつながるものを古い順）・`reblogged_by`・`favourited_by`、`accounts/:id`・`statuses`（`pinned`・`exclude_replies`・`exclude_reblogs`・`only_media`）・`followers`・`following`・`relationships`・`lookup`（未知の `acct` は `resolve_open_target` で取り込む）・`search`、`GET /api/v2/search`（URL は `resolve=true` のとき「開く」と同じ解決。投稿検索の回数制限はカスタム API と共通。`offset` によるページングは未対応で、`max_id` 無しの `offset` には空を返す）。
- 操作: `accounts/:id/follow`・`unfollow`（既にフォロー済みでも関係を返す）、`statuses/:id/favourite`・`unfavourite`・`reblog`・`unreblog`、`POST /api/v1/media`・`/api/v2/media`（`create_drive_file` に委譲。同期処理なので常に 200）、`GET`/`PUT /api/v1/media/:id`（更新内容は保存しない）。
- 通知: `GET /api/v1/notifications`。`follow`→`follow`、`followRequest`→`follow_request`、`mention`/`reply`→`mention`、`repost`→`reblog`（`status` はリポストされた自分の投稿）、`quote`→`quote`、`❤️` リアクション→`favourite`、他のリアクション→`pleroma:emoji_reaction`（`emoji`・`emoji_url` 付き）。引っ越し関連等は出さない。既読化しない。
- ブロック・ミュート: `accounts/:id/block`・`unblock`・`mute`・`unmute` はカスタム API と共通の `blocks::block_actor`/`unblock_actor`・`mutes::mute_actor`/`unmute_actor`（対象文字列の解決と処理本体を分けたもの）を呼んで関係を返す。ミュートの `notifications`・`duration` は無視する（常に通知も含めて無期限）。`GET /api/v1/blocks`・`mutes` はページングせず全件。
- プロフィール編集（`PATCH /api/v1/accounts/update_credentials`）: `display_name`・`note`（自己紹介）・`fields_attributes`（フォームの添字付きオブジェクトと JSON の配列の両方）・`locked`・`avatar`/`header`（multipart のファイル）を受ける。画像は `drive::store_uploaded_file`（カスタム API のアップロードと共通、`avatar`/`banner` として保存）、更新は `users::update_profile`・`account::update_lock` に委譲する（AP `Update`・ATP プロフィールコミット・承認制解除時の一括承認も同じ）。`bot`・`discoverable`・`source[...]` は無視する。
- 投票: `GET /api/v1/polls/:id`・`POST /api/v1/polls/:id/votes`（`choices[]`）。投票 ID は投稿 ID と同じで、`vote_poll` に委譲する。
- ピン留め: `statuses/:id/pin`・`unpin` は `pin_note`/`unpin_note`（Fedi featured・Bsky `pinnedPost` 反映込み）に委譲する。`Status.pinned` は閲覧者自身のピン留めかどうか。
- ブックマーク: `statuses/:id/bookmark`・`unbookmark`、`GET /api/v1/bookmarks`（`bookmarks` テーブル。本人だけの保存で通知・配送は無い。一覧のカーソルはブックマーク ID、見えなくなった投稿は除く）。`Status.bookmarked` を返す。seiran の Web UI にはまだ出していない。
- 未実装機能のスタブ（常に `[]`）: `filters`（v1/v2）・`announcements`・`favourites`・`conversations`・`followed_tags`・`featured_tags`・`endorsements`・`scheduled_statuses`・`follow_requests`・`domain_blocks`・`trends/*`・`suggestions`（v1/v2）。`markers` は `{}`。404 だと起動時やタブで画面ごとエラーにするクライアントがあるため。

**ストリーミング（`GET /api/v1/streaming`、WebSocket）**: トークンはクエリ `access_token`・`Authorization`・`Sec-WebSocket-Protocol`（その場合は同じ値をプロトコルとして応答する）の順に探す。ストリームは `stream` クエリか `{"type":"subscribe"|"unsubscribe","stream":...,"tag":...,"list":...}` で指定し、`user`（ホーム＋通知）・`user:notification`・`public`/`public:remote`（グローバル）・`public:local`・`hashtag`（`tag`）・`list`（`list`、所有者か公開リストのみ）に対応する。配信元は SPA と同じ `StreamHub` で、タイムライン新着はチャンネル方式の `ChannelScope::matches` で当てはめ、投稿 ID から閲覧者視点の `Status` を組み直して `{"stream":[...],"event":"update","payload":"<Status の JSON 文字列>"}` で送る（見えない投稿は `find_status` が弾く）。通知は、`StreamHub` の配信が通知 ID を持たず通知の INSERT より先に届く経路もあるため、自分宛てのイベントを合図に0.8秒待ってから通知テーブルの新着を読み `notification` で送る（接続時点の最新を基準にし、合図を取りこぼした分は30秒ごとの確認で拾う）。インスタンス情報の `urls.streaming_api`（v1）・`configuration.urls.streaming`（v2）は `wss://{local_domain}`。nginx は `/api/v1/streaming` に Upgrade ヘッダーを通す。

**既知の非互換**: 投稿の編集（`PUT /api/v1/statuses/:id`）は対応しない（seiran のポストは Bluesky のポストでもあり、再編集できないことが前提のため）。ストリーミングは削除（`delete`）・DM（`direct`）・編集のイベントを送らず、SSE 版（`/api/v1/streaming/user` 等）は無い。

## 8. 通知・リアルタイム配信

`streaming::StreamHub`（プロセス内 `tokio::broadcast`、容量512）が `{"type":kind,"body":body}` を配信する。接続は `GET /api/streaming?token=<JWT>`。配信方式は2つ。

- **`recipients` 方式**（通知・DM・`noteUpdated`・`pollUpdated`）: `StreamEvent.recipients` に自分の actor_id を含むイベントだけを各コネクションが転送する。購読操作は不要。
  - `pollUpdated`（`broadcast_poll_update`。ローカル投票と AP 受信の両方から送る）は `{"postId","poll":<posts.poll>}` を著者＋著者をフォロー中のローカルアクターへ送る（`broadcast_reaction_update` と同じ宛先）。`votedByMe` は閲覧者ごとに違うので含めない。フロントは `StreamingContext` が共有ストア `stores/pollVoteStore.ts` を直接更新し、表示中の全 NoteCard に反映する（自分の投票済み選択肢は保ったまま票数だけ差し替える）。
- **チャンネル方式**（タイムライン新着、Misskey 互換）: `{"type":"connect","body":{"channel":"localTimeline","id":"<uuid>","params":{}}}` で購読し、`{"type":"channel","body":{"id":"<uuid>","type":"note","body":{...}}}` で届く。`disconnect` で解除。チャンネルは `homeTimeline`/`localTimeline`/`hybridTimeline`（ソーシャル）/`globalTimeline`/`userList`（`params.listId`）/`hashtag`（`params.tag`）。
  - 送信側（`delivery::broadcast_new_note`、`handle_create_note`、firehose `save_bsky_post`）は投稿ごとに1回 `ChannelScope`（`is_local`・`visibility`・`home_recipients`（著者＋承認済みローカルフォロワー）・所属リスト・ハッシュタグ）を作って `publish_channel_note` で送り、各コネクションは `ChannelScope::matches` で照合する（コネクションごとのDB問い合わせは無い）。条件は対応する REST のタイムラインクエリに合わせる。
  - `userList` の `connect` は所有者か公開リストのみ許可する。
  - リプライの `home_recipients` は、リプライ先の投稿者もフォローしている（または本人の）フォロワーに絞る（`find_home_recipient_ids` が `post_reply_target_followed` を使い、REST と基準を共有する）。
  - 制限: ブロック/ミュートはチャンネル配信では考慮しない（コネクション数に比例するDBコストになる。通知は INSERT 時に判定する）。`userList` はメンバーシップだけで判定する（リストTLに閲覧者の概念が無いため。`direct` は除外）。
  - DM はチャンネルではなく `recipients` 方式（`publish_note`）で送る。

`notifications` の種別は `Follow`/`Reaction`/`FollowRequest`/`FollowRequestAccepted`/`Mention`/`Reply`/`Repost`/`Quote`/`MoveRefollowed`/`MoveAlreadyFollowing`。`FollowRequest` は Misskey API では `receiveFollowRequest`、`Move*` は seiran 独自。ローカルフォローは `follows` への新規挿入時だけ `Follow` 通知を作る。WebSocket は「新着がある」というシグナルだけに使い、実データは常に `POST /api/i/notifications`（`sinceId`）で取り直す（一覧とスキーマを揃えるため）。

**`followAccepted`** は例外で、ペイロード（`actor.username`/`actor.domain`）をフロントが直接使う。Fedi フォロー（`follow_fedi`）は相手が承認制（`manuallyApprovesFollowers: true`）のときだけ `pending` で始め、それ以外は Misskey と同じく送信と同時に `accepted` にする（相手の Accept を待たない楽観的確定。Aria はフォロー操作の1秒後に一度だけ再取得するので、`pending` のままだとボタンが処理中に固まる）。承認制の相手の `pending` → `accepted` は、`StreamingContext` が `followAccepted` を受けて `stores/followStatusStore` を更新し、その相手を表示中のコンポーネント（フォローボタン・NoteCard のスイッチ）すべてに即座に反映する。

### リアクション通知の重複排除（`reaction_id`）
ローカルユーザーが ATP 実体を持つ投稿にリアクションすると、(1) `create_reaction` がローカル通知を INSERT し、(2) `commit_like` がコミットした Like が自分の firehose 受信（`handle_inbound_like_create`）で戻ってきて再び通知を INSERT しようとする。同じ操作の別経路なので、`reactions.id` を両経路で共有して重複を防ぐ: ローカル INSERT 時の `reactions.id` を `notifications.reaction_id` に入れ、`commit_like` が Like レコードの拡張フィールド `seiranReactionId` にも載せる。firehose で戻った Like の `seiranReactionId` を `reaction_id` として渡すと、部分 UNIQUE の `idx_notifications_reaction_id`（`ON CONFLICT DO NOTHING`）で2件目が弾かれる。

`source_uri` の UNIQUE は他人発のイベントの複線受信対策で、`reaction_id` は自分発の操作が戻ってくることへの対策。他人のリアクションは `reaction_id` が NULL なので、同じ投稿への絵文字の連投は妨げない。

### メンション通知
本文でローカルユーザーが言及されたら `type="mention"`（`note_id` = 言及元）を作る。配送設定と無関係に、出自ごとに次で解決する。自己メンションは通知しない。リプライ先の投稿者へのメンションはリプライ通知と重複するので作らない（他の宛先へのメンションは通知する）。

- **ローカル投稿**: `mention::extract_local_mention_actor_ids` が `@username`・`@username.{local_domain}`・`@username@{local_domain}` のローカルアクターを重複除去して返す。配送用の変換（`convert_mentions_for_*`）は配送先があるときしか呼ばれないので、独立に常に実行する。
- **Fedi 受信**: `tag[]` の Mention のうち `href` が自ドメインの `/users/{username}` を指すものを `extract_local_username` で判定する。URI 末尾だけを見るとリモートの同名ユーザー宛を取り違えるので、ホスト名まで確認する。
- **Bsky 受信**: `mention_facets` の各 `did` を引き、`actor_type = 'local'` なら通知する。

`source_uri` は渡さない。1投稿に複数の宛先がありうるので、投稿の識別子を共有すると2人目以降が部分 UNIQUE で弾かれる（投稿自体の重複排除は各経路で済んでいる）。

### リプライ通知
自分の投稿に返信が付いたら `type="reply"`（`note_id` = 返信）を作る。リプライ先の投稿者がローカルの場合だけ。自己リプライは通知しない。

- **ローカル投稿**: `ReplyContext::parent_local_actor_id`（親の `domain` が自ドメインなら `Some`）。
- **Fedi 受信**: `inReplyTo` から解決した親の投稿者を `find_delivery_meta` で引き、`domain` が自ドメインなら通知する。
- **Bsky 受信**: `reply.parent.uri` から解決した親の投稿者が `local` なら通知する。

`source_uri` はメンション通知に揃えて渡さない。

### リポスト・引用通知
ローカル投稿が他人にリポスト・引用されたら `type="repost"`/`"quote"`（`note_id` = 新しいリポスト/引用投稿）を作る。自己リポスト・自己引用、リモート投稿者宛は作らない。ローカル作成（`create_repost`/`create_regular_post`）と AP 受信（`handle_announce`/`handle_create_note`）で、対象の `PostDeliveryMeta` がローカルなら作る。再配送は通知生成の前に重複チェックで止まる。

Bsky は Jetstream の `app.bsky.feed.repost` の `subject.uri` がローカル投稿ならリポスト通知、取り込んだ投稿の `embed.record.uri` がローカル投稿なら引用通知を作る。リポストレコード/引用投稿の `at://` を `source_uri` にして多重受信を防ぐ。

- `repost` の `note_id` は本文の無いリポストラッパーで、その可視性はリポスト元と独立。`build_notifications` は Misskey（`NotificationEntityService#packInternal`）と同じく、ラッパーには可視性チェックをかけずに `note` として pack し、`note.renote`（リポスト元）の埋め込みは通常の pack 処理（可視性チェック込み）に任せる。この入れ子を崩すとクライアントがリノートとして描画できず「不明」になる。フロント（`NotificationsPanel.tsx` の `resolveTargetNoteId`）は `note.renote` を優先する。
- アンリポストでラッパーは論理削除されるが、通知行は残す（過去の出来事の記録）。`build_notifications` はラッパーを `find_by_id_including_deleted` で取る。`find_by_id` だと取り消し済みラッパーが取れず、`note.renote` まで失われる。
- `notifications.type` は seiran 語彙（`repost`/`quote`/`reaction`/`follow`/`followRequestAccepted`/`mention`/`reply`）で統一し、Misskey API の応答直前に `repost` → `renote` だけ変換する（`to_misskey_notification_type`。Misskey に `repost` は無い）。seiran のフロントもこの API の利用者なので `"renote"` で判定する。

## 9. ダイレクトメッセージ

`visibility='direct'` の投稿として `posts` に格納する（`docs/database.md`「ダイレクトメッセージ関連」）。Misskey 互換の投稿・タイムライン API でもそのまま扱える。

### 宛先・スレッド・タイムライン除外
- 宛先は `post_recipients`。`POST /api/notes/create` は `visibility=direct` のとき `recipient_actor_ids`（Misskey の `visibleUserIds`）を必須とし、存在しない ID は 400 `INVALID_RECIPIENT_ACTOR_ID`。
- `handlers::dm::sessions`・`thread_messages` のノート組み立ては通常投稿と同じ `build_note_responses` を使い、`thread_messages` はさらに Bsky 側リアクション（`dm_bsky_reactions`）の合算と、メッセージごとの宛先一覧（`NoteResponse.recipients`、`DmPeerResponse` と同形）を載せる。宛先はメッセージごとに違いうるので、フロントの「宛先:」表示（3人以上のスレッド、`docs/ui_spec.md` 2.5節）はこれを使う。
- スレッド起点（`thread_root_post_id`）は伝播コピー方式: 親が `direct` なら親の値をコピーし、そうでなければ自分の ID。
- タイムラインの `direct` の閲覧は投稿者本人か宛先のみ（フォロワーには見せない）。`exclude_direct`（Misskey 互換のため既定 `false`）を付けると宛先にも出さず、seiran のフロントは常に付ける。判定は `post_is_visible_to`。
- リスト・ピン留めのタイムラインは `direct` を無条件で除く。

### Fedi受信（`save_ap_note_core`、Create 直接受信時のみ）
`classify_ap_visibility` が `direct` と判定したら次を行う。参照解決で取得した投稿は inbox に配送されたものではなく宛先情報を信頼できないので、これらをしない。
- `to` のローカルアクター URI から宛先を解決して `post_recipients` に入れる。ローカルの `actors.ap_uri` では引けないので、`extract_local_username` でホスト名まで確認してから `find_by_username_domain` で解決する。リモートの宛先（自ドメイン・送信者以外）は受信を止めないよう `Job::DmRecipientResolve` に回し、未知アクターも upsert してから追加する。
- 親が `direct` なら、送信者が親の当事者（投稿者か宛先）であることを `post_is_visible_to` で確認してから `thread_root_post_id` を継承する。当事者でなければ受信を拒否する（`to`/`inReplyTo` は自由に申告できるので、確認しないと第三者が他人の DM スレッドに紛れ込める）。
- WS 配信は宛先だけ。

### 配送
- Fedi: `deliver_direct_message_to_ap`（`ApDeliveryKind::DirectMessage`）が宛先の Fedi アクターの inbox だけに Create(Note) を送る（`to` は宛先のみ）。
- Bsky: `Job::BskyDmSend` が `chat.bsky.convo.sendMessage` で送る。スレッドごとに1回 `getConvoForMembers` で convoId を解決して `bsky_convo_links` にキャッシュする。認証は自己署名のサービス間 JWT（`aud` はフラグメント無しの `did:web:api.bsky.chat`、`docs/skill_atp_rust_programming.md` §17）。Bsky 宛は1対1のみ（Bsky アクターを含む宛先に他の宛先は同居できない）。
- WS: `delivery::broadcast_direct_message` で投稿者と宛先だけに送る。フォロワー全体に送る `broadcast_new_note` を使うと本文が漏れる。

### メッセージ画面の表示
`MessageContent` は `NoteCard` の本文表示（CW・添付・リンクカード・引用）を流用し、アンケートは結果表示だけ（DM では投票不可）。リアクションは吹き出しの外側下部に LINE 風（個数表示なし、同じ絵文字は個数分並べる）で `MessageReactions` が描く。

### DMメッセージへの絵文字リアクション
fedi/local のみのメッセージは通常投稿と同じ `reactions`（1メッセージ1ユーザー1個）と `create_reaction`/`delete_reaction` を使う。可視性チェックが宛先以外を弾くので DM 専用 API は要らない。

- **Fedi 配送**: 通常のリアクション配送は `to: Public` と reactor のフォロワー全員に送るので、DM に使うとメッセージの存在が漏れる。`resolve_reaction_targets` は対象が `direct` なら、宛先を「投稿者＋`post_recipients`」（reactor 自身を除く）の Fedi inbox に絞り、`to` もその ap_uri だけにし、`cc`（フォロワー）を付けない（`build_undo_reaction_activity` は既定で `cc` を付けるので明示的に外す）。投稿者を含めるのは、自分宛メッセージへのリアクションで宛先が自分だけになり配送先が0件になるのを避けるため。
- **WS**: 通常の `broadcast_reaction_update` はフォロワーにも送るので、DM では `broadcast_dm_reaction_update`（投稿者＋宛先のみ）を使う。呼び出し元（`create_reaction`/`delete_reaction`、`handle_reaction`、`handle_undo`）は対象の `visibility` で使い分ける。
- **フロント**: `MessagesPage` は NoteCard と同じ `registerReaction`/`noteUpdated`/`applyReactionUpdate` を使う。Bsky 宛は `bsky_dm_poll` が実際に変化を検知したときだけ同じ `noteUpdated` をローカルユーザーへ送る。`reactorActorId` は常に相手なので、自分の `reactedByMe` は変わらない（自分の変更は API 応答で反映済み）。
- **Bsky 宛**（`dm_bsky_reactions`）: `chat.bsky.convo.addReaction` は「1ユーザーが異なる Unicode 絵文字を複数付けられる（メッセージ全体で最大5件、同じ絵文字は1個）」ので、`reactions` とは別に `dm_bsky_reactions::{create_dm_bsky_reaction, delete_dm_bsky_reaction}`（`POST`/`DELETE /api/dm/messages/:id/reactions[/:content]`）を用意する。カスタム絵文字は `BSKY_REACTION_UNICODE_ONLY`、5件到達は `BSKY_REACTION_LIMIT_EXCEEDED`、重複は `BSKY_REACTION_ALREADY_EXISTS`。ローカル発は `Job::BskyDmReactionAdd`/`Remove` が `addReaction`/`removeReaction` へ送る（`posts.bsky_message_id` 必須）。相手発は `bsky_dm_poll` が取り込む。フロントのピッカーは Bsky 宛では `unicodeOnly`。

### DMメッセージの削除・「隠す」
fedi/local のみのメッセージは通常の削除（`DELETE /api/notes/:id`）。Bsky 宛は相手のメッセージを削除できない仕様なので、`hide_dm_message`（`POST /api/dm/messages/:id/hide`）で閲覧者ごとに非表示にし（`dm_hidden_messages`）、`Job::BskyDmHide` で `chat.bsky.convo.deleteMessageForSelf` も送る。自分のメッセージにも相手のメッセージにも使える。

### Bsky受信ポーリング（`seiran-atp-repo::bsky_dm_poll`）
`chat.bsky.convo` は Jetstream に乗らないので、`at_did`/`at_signing_key_pem` を持つ全ローカルアクターについて60秒ごとに `listConvos` → 会話ごとに `getMessages` を取り込む（DID 転入アカウントも自動的に対象）。`bsky_convo_links.last_synced_message_id` をカーソルにする。取り込んだメッセージは `direct` 投稿として保存して WS 配信する（自分が送ったものは `BskyDmSend` 側で保存済みなので飛ばす）。グループ会話は対象外。

- **初回同期**: `last_synced_message_id` が未設定の会話に限り、`getMessages` の `cursor` で過去へ遡る（転入で持ち込んだ長い履歴のため）。1会話あたり最大20ページ・2000件（`MAX_INITIAL_SYNC_PAGES`/`MAX_INITIAL_SYNC_MESSAGES`）。同期済みの会話は最新1ページだけを見る。
- **リアクション同期**: 相手がリアクションを付け外ししても新着は増えないので、新着の有無によらず、取得した全メッセージの `reactions` で `dm_bsky_reactions` をメッセージ単位に置き換える（`sync_message_reactions` → `DmRepository::sync_bsky_reactions`）。
- **1件の取り込み**: 投稿の INSERT（`bsky_message_id` の `ON CONFLICT DO NOTHING`）・`post_recipients`・カーソル前進を1トランザクションでコミットする。途中で失敗しても再取り込みは未コミット分だけで、UNIQUE 制約が同時ポーリングの二重取り込みを防ぐ。

### 未読管理
`dm_read_states`（actor_id, thread_root_post_id, last_read_post_id）でスレッドごとの既読カーソルを持つ。左ペインのバッジは未読のあるセッション数（`unread_session_count`）。

### ミュート・ブロック相手のDM
`DmRepository::sessions`・`unread_session_count` は、自分以外の参加者全員が「自分がミュート済み」か「自分との間にブロック関係がある（方向不問）」ならそのスレッドを除く（1人でも該当しなければ表示）。`thread_messages` はスレッドの `is_participant` に加えてメッセージごとに `post_is_visible_to` で確認する（スレッド起点を共有する別のメッセージまで見えないように）。

### DM投稿URLへの直接アクセス
`GET /api/notes/:id` が `direct` の投稿を返すときは `thread_root_post_id` を付ける。フロント（`NoteDetailPage.tsx`）はそれを受けたら `/messages/:threadRootPostId` へ `replace` で遷移する。見えなければ `NOT_FOUND`。

### `chat.bsky.actor.declaration`（Bsky DM受信許可）
公式クライアントは相手 PDS の `chat.bsky.actor.declaration`（rkey `self`、`allowIncoming: "all"|"none"|"following"`）で DM 送信可否を判断し、レコードが無いと送信をブロックする。`commit_chat_declaration` が `"all"` 固定でコミットする。値を選べる設定は未実装（`docs/roadmap.md`）。

新規ローカルアカウントの ATP リポジトリには、通常登録（`register`）・初期管理者作成（`setup`）とも `auth::publish_initial_atp_records` がプロフィール・`chat.bsky.actor.declaration`・`org.seiran.actor.declaration` をコミットし `#identity` を送る。起動時の `backfill_chat_declarations` は未コミットのローカルユーザーに宣言を補う。

## 10. ブロック・ミュート

### 定義
- **ミュート**: 自分のタイムライン・通知から相手を隠すだけのローカル効果。相手に通知せず、配送もしない（`mutes` の INSERT/DELETE のみ）。
- **ブロック**: Bsky 準拠（フォロー関係の強制解除＋相互完全非表示）。Fedi の「片方向拒否」と Misskey の「ミュート」を合わせた効果になる。相手の実体ごとに独立に送る:
  - `at_did` があれば `app.bsky.graph.block` をコミット（`commit_block`）
  - `ap_uri` があれば AP `Block` を配送
  - 結婚済み（`remote_seiran`）は両方に送る
  - `blocks` への1行で、相互非表示（`actor_is_hidden_for_viewer`）と書き込みガードの両方が効く。

### 書き込みガード
ブロック関係があれば API で拒否する（`target_resolve::check_not_blocked`）: フォロー作成、リプライ作成（加えて返信先が閲覧者から見えるかを `find_by_id_for_viewer` で確認し、見えない `followers_only`/`direct` への返信を拒否）、リアクション、引用・リポスト、DM 送信。

### プロフィール表示の制限
相手からブロックされている（`is_blocked_by`）と、`build_profile_response` は `bio` と `profile_fields` を空にする。投稿一覧は `actor_is_hidden_for_viewer` で元々空になる。

### AP受信時のフォロー拒否
`handle_follow` は、こちらが送信者をブロック中なら `Accept` を送らず無視する。

### 相手発ブロックの検知
リモートユーザーが自分をブロックした場合も `blocks` に記録し、同じ制限を対称に効かせる。
- **Fedi**: `Block` 受信で `(blocker=相手, blocked=ローカル)` を INSERT、`Undo(Block)` で DELETE。
- **Bsky**: 「自分をブロックしている人」を返す API は無い（意図的に非公開）ので、`seiran-atp-repo::bsky_block_watch` が `app.bsky.graph.block` だけを対象にした絞り込み無しの Jetstream 接続（全世界で約2件/秒）を張り、`record.subject` がローカルユーザーの DID のものを記録する。Jetstream の `delete` には `subject` が無いので、create 時の `commit.rkey` を `blocks.atp_rkey` に保存し、`(blocker_actor_id, atp_rkey)` で逆引きして消す（`delete_by_blocker_and_rkey`）。投稿用の接続とは別のリーダー選出キーで動く。

### リアクション表示でのブロック・ミュート除外
ミュート・ブロックしている相手のリアクションは、集計（`fetch_reactions_map`）と「誰が付けたか」一覧（`GET /api/notes/:id/reactions/:content/actors`、Misskey `notes/reactions`）から `actor_is_hidden_for_viewer` で除く（Misskey 互換クライアントも同じ API を見るので、フロントではなく API で除く）。WebSocket の `noteUpdated` の集計には適用しない（全員に同じ payload を送る設計で、閲覧者ごとに再計算すると重い）。そのため再取得まで件数が一時的にずれることがある。

### スコープ外
公開リストタイムライン（`list.rs::timeline`）のフィルタは未実装。リストTLは閲覧者の概念を持たない設計のため、閲覧制御全体の見直しが要る。

## 11. リモートseiranアクターの相互申告マージ（#236）

他 seiran サーバーのユーザーは AP と ATP の両方の実体を持つ。AP 経由・ATP 経由のどちらで先に（または同時に）見つかっても、1つの `actors` 行（`actor_type='remote_seiran'`、「結婚」）に収束させる。チャレンジ検証のようなハンドシェイク専用の仕組みは持たない。

- **自己申告**: AP は Actor 文書の拡張フィールド `seiranAtDid` で自分の DID を、ATP は独自コレクション `org.seiran.actor.declaration`（rkey `self`）の `apActorUri` で自分の Actor URI を申告する。`app.bsky.actor.profile` に載せないのは、他クライアントがプロフィールを丸ごと上書きすると独自フィールドが消えるため。取得は公開 AppView が独自 NSID を中継しないので、DID を `atproto_pds` に解決してその PDS に直接 `getRecord` する（`atp::client::fetch_seiran_actor_declaration`）。
- **発見時はすぐ INSERT、検証は後**: AP で u（ATP 相手 v を申告）を見つけたら `fedi` 型で `ap_uri=u`・`claimed_at_did=v` として INSERT し、v を取りに行く `Job::ActorMetadataResolve` を積む（結婚を早めるための先行取得で、成立の必須条件ではない）。ATP 側も対称に `bsky` 型で `at_did=v`・`claimed_ap_uri=u`。このジョブ（`jobs::actor_metadata_resolve`）は必ず相手側の申告（DID 側は `fetch_seiran_actor_declaration`、AP 側は取得した Actor 文書の `seiranAtDid`）を使う。呼び出し元が持つ値を渡すと下の相互一致が自己参照になり、一方的な申告だけで結婚が成立してしまう。
- **発見後の共通処理**: 結婚成立時の Jetstream `wantedDids` 再構築と、未成立かつ申告ありのときの `ActorMetadataResolve` 投入は `seiran_actor_merge::promote_after_discovery` に集約し、`follow_exec`・`inbound_activity_process`・`firehose::resolve_or_upsert_bsky_actor`・`actor_metadata_resolve`（`claimed` に None を渡して無限投入を防ぐ）から呼ぶ。`find_by_did` が既存行を返す経路はこの発見処理を通らない（両側の行が既に別々にある場合の対処はスコープ外）。
- **相互一致で結婚**: 相手の実体（真正な `ap_uri`、または DID 解決・署名済みコミットで裏付けられた `at_did`）がDBにあり、かつその既存行の申告が今回見つけた実体を指し返している場合だけ結婚させる。成立したら `remote_seiran` に昇格し `claimed_*` をクリアする。不一致・申告なしなら単独の `fedi`/`bsky` 行のまま残す。一方的な申告を受け入れると、他人の実在 URI を騙って結び付ける偽装ができてしまう。
- **結婚済みの行の保護**: 結婚済みの行を再訪問したとき（`existing_id` 分岐）は、`claimed_*` を復活させず `username` も上書きしない。結婚ロジックを通らない `ActorRepository::upsert_remote_bsky`（フォロワーポーリング・検索・ターゲット解決等）も、結婚済みの行の `username` を ATP ハンドル形式で上書きしない。`at_handle`（プロフィールの Bsky ID 表示用）は対象外で常に最新にする。
- **チャレンジ検証が不要な理由**: AP の Actor 文書はドメインの TLS で、ATP の宣言レコードは DID の署名済みコミットで、それぞれ発行者の真正性が担保されている。相互申告の一致を確認すれば、攻撃者は相手の鍵やドメインを持たない限り「相手に自分を名指しさせる」ことができない。
- **レース対策**: `actors` の複合 UNIQUE `actors_mutual_claim_key`（`(COALESCE(ap_uri, claimed_ap_uri), COALESCE(at_did, claimed_at_did))`、`actor_type IN ('fedi','bsky','remote_seiran')` の部分インデックス）で、相互に申告し合う2行の共存を禁じる。申告の無い行は NULL の非等価性で抵触しない。違反したら `unique_retry::retry_on_unique_violation` がトランザクションを最大5回やり直す（`discover_fedi_actor`/`discover_bsky_actor`）。advisory lock ではなく制約にしているのは、ロックキーの選び方次第で直列化が効かなくなる余地を無くすため。
- **Fedi 系アクターの判定**: AP の inbox を持つのは `fedi` と `remote_seiran` の両方なので、AP 配送・DM・メンション等の宛先判定は `actor_type IN ('fedi', 'remote_seiran')` にする。`lists.rs` のプロキシフォロー関連だけは `fedi` 固定が正しい（`remote_seiran` は `wantedDids` 経由で ATP から投稿を受け取れるので、AP のプロキシフォローが要らない）。
- **フォローの AP/ATP 同期（#238）**: 承認制は AP にしかないので、リモート seiran へのフォローは常に AP で送る（`follow_fedi`）。相手が非承認制で既に結婚済みなら同時に ATP の `commit_follow` もして `follows.atp_rkey` に記録し、承認制なら `Accept` 受信時（`handle_accept`）に同じことをする。アンフォロー・退会時の一括アンフォローは `atp_rkey` の有無で ATP 側の解除を判断するので対称に動く。
- **未実装**: 投稿マージ（5節）の場で未結婚の投稿者同士を `counterpartAuthorId` で結婚させる処理。フォロー時に未結婚で後から結婚した相手への ATP フォローの追いコミット。`actors.seiran_pair_actor_id` は使っておらず（常に NULL）、削除を検討している。

## 12. 未実装・スコープ外の機能

- **トレンド集計**: テーブル・エンドポイントとも無い。
- **`inbound_activity_process` のドメイン単位レート制限**: 無い（`actor_history_sync` だけドメイン単位の同時実行制限を持つ）。
- **リモートユーザーの公開リストの取得**: 無い（`public_lists` はローカルのみ）。
- **ブロック・ミュート**: 10節「スコープ外」。

# ActivityPubアンケート回答

リモートの `Question` へのローカル回答は、選択肢ごとに `Create(Note)`（`name` = 選択肢名、`inReplyTo` = Question ID）として投稿者の Inbox へ送る。同じ形式の回答を受信したら `poll_votes` に冪等に保存し、ローカルの集計を更新する。

# Fediverseリレー参加

管理者が登録した HTTPS の inbox URL へ、専用ローカル actor `https://{domain}/users/relay-agent` から署名付き Follow を送る。Accept/Reject は Follow の activity ID と照合して状態を更新し、離脱時は元の Follow を内包した Undo を送る。

- Accept を返さず配送を始める実装があるため、登録 inbox と同一 origin のリレー鍵で正しく署名された配送を受けたら参加成立（`accepted`）とみなす。
- 通常の受信は署名者と `activity.actor` の一致を必須とするが、リレー配送は元投稿者を `activity.actor` のままリレーが署名するので、この登録済み同一 origin の場合に限り不一致を許す。
- 管理 API は Snowflake ID を文字列で返し、離脱 API のパスにもそのまま使う（JavaScript の数値丸め対策）。
- `accepted` のリレーには `visibility='public'` のローカル投稿だけを、通常配送と同じ署名・再試行経路で追加配送する。

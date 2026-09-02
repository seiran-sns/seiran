# マルチプロトコル実装

## URL・IDからの対象解決（`POST /api/open`、#165）

認証済みSPAは `{ "target": string }` を送り、`{ "kind": "actor" | "post", "path": string }` を受け取る。bsky.appプロフィールURLと`did:plc:`はAppViewプロフィール取得後にアクターをupsertし、bsky.app投稿URLとAT URIはDIDへ正規化して単一投稿をupsertする。一般のHTTP(S) URLはActivityStreams表現を取得し、Actorなら既存のWebFinger/APアクター解決、Note/Article/Question/Pageなら既存の`InboundActivityProcess` Create経路で取り込む。

フェッチしたオブジェクトが`Announce`型の場合はリポストラッパーとして取り込む（#232）。Misskeyの素リノート（コメント無しブースト）は、`notes/{id}`への直接アクセスでは`quoteUrl`付きの`Note`（空リプ引用として扱われる）になるが、他鯖ミラーURL（ローカルにコンテンツを持たないサーバーが元サーバーの`/activity`へ302リダイレクトする）や`notes/{id}/activity`への直接アクセスでは`Announce`（`object`は対象ノートのURI文字列）として得られる。`open_target::open_announce`はこのAnnounceオブジェクトをそのまま`InboundActivityProcess`（既存の`handle_announce`経路）へ積む。対象ノート（`object`）が未取得なら`resolve_reference`が1段階だけフェッチを試みるが、失敗してもリポストの箱自体は保存される（4節「リポスト」参照）ため、「開く」のポーリングは箱の保存だけを待てば完了する。

### pending参照の遅延解決（#233）

取り込み時点のフェッチ失敗で`pending`のまま保存された参照（4節「引用受信」参照）は、以下2つの経路でその場フェッチを再試行できる。いずれも`jobs::inbound_activity_process::resolve_pending_reference_with_timeout`を共有し、成功時は`*_post_id`を、404/410を新たに確認できた時は`ref_status`を`gone`へ更新する（それ以外の失敗ならDBは`pending`のまま据え置く）。`gone`確定後はどちらの経路も再フェッチを試みない。

- **投稿詳細取得時の受動的フェッチ**（`GET /api/notes/:id`、`handlers::notes::retrieval::resolve_pending_post_references`）: リプライ/引用/リポストの3種を並行して最大1秒ずつ試みる。未ログイン閲覧（OGP等）でも通る経路のため短めに設定している。
- **手動「取り込む」API**（`POST /api/notes/:id/resolve-reference`、body `{"kind": "reply"|"quote"|"repost"}`、認証必須）: ユーザーが明示的に待つ操作のため最大8秒。レスポンスは`{"status": "resolved"|"pending"|"gone"|"none", "post_id": string|null}`。

対象読者: ActivityPub / AT Protocol の実装やクロスプロトコル配送ロジックに触れる開発者。
「今、何が実装されていて、どう動くか」だけを書く。不具合修正の経緯や日付は書かない（`git log` 参照）。

## 通報配送

通報はまずすべてローカル管理者に届き、対象がリモートの場合のみ管理者が任意にFedi/Bskyへ
転送する（通報者自身は送信先を選ばない）。

リモートFediへの転送は、通報者を `actor` としたActivityPub `Flag` を対象ActorのInboxへ
HTTP Signature付きで送る。ActivityPubのFlagはアカウント単位の通報しか表現できないため、
`object` は常に対象ActorのURI一つのみとし、投稿通報の場合は対象投稿のURLを `content`
（理由分類 `[分類]` ・自由記述に続けて）に付記する。Blueskyへの転送は
`com.atproto.moderation.createReport` をBluesky Moderation Serviceへ送り、ユーザー対象は
`repoRef`、投稿対象はAT URI/CIDの`strongRef`とする。`reasonType` は
`tools.ozone.report.defs#<reason_type>`（Bluesky公式の2段階理由分類、39種）をそのまま渡す。
成功時のみ`reports.forwarded_at`を記録する。

## 1. フォロー時の初期同期

新規フォローが成立すると `Job::ActorHistorySync` が積まれ、相手の過去ログを非同期でバックフィルする（過去30日間 / 最大300件、ベストエフォート）。フォロー後のタイムライン表示は常にローカルDBからの読み取りのみで完結し、外部APIを都度叩かない（`docs/database.md` 4節、`docs/concept.md` 参照）。

- AT Protocol: 相手の DID から AppView の `getAuthorFeed` を叩いて取得。
- ActivityPub: 相手の Outbox（`GET /users/:username/outbox`）をページングして取得。

ノート詳細画面から前後投稿を見に行くオンデマンド同期も同じ仕組みを利用する。

## 2. ActivityPub (Fedi) 統合

### Fedi投稿のCW・アンケート・閲覧注意画像

受信した`Note`/`Question`の`summary`をCW、`Question.oneOf`/`anyOf`をアンケートとして
正規化して保存する。添付の`sensitive`または投稿全体の`sensitive`が真なら、その画像を
閲覧注意としてAPIへ返す。アンケートは外部サーバー上の集計結果を表示し、seiranからの投票配送は
現時点では行わない。

### Fedi投稿のURLカード（`jobs::inbound_activity_process::note_save::save_ap_note_core`）

APには`app.bsky.embed.external`のような明示的なembed概念が無いため、`posts` INSERT後に
本文（`ap_content_to_markdown_body`が生成したMarkdown `[text](url)`）中のリンクURLから
`extract_link_card_urls`で最大5件（`MAX_LINK_CARDS_PER_POST`、重複排除・画像記法`![...]()`と
表示テキストが`#`始まりのハッシュタグリンクは除外）を抽出する。この抽出とOGP取得ジョブの
enqueueは`save_ap_note_core`に一本化されており、Create直接受信・参照解決経由（リプライ/
引用/リポスト対象の1段階フェッチ、上記「Inbox で処理する Activity 種別」表の`Announce`欄
参照）のどちらで保存された投稿でも必ず実行される。Bskyは1投稿につき最大1件だが、
Fediは本文中の複数リンクぶん**複数件のURLカードが並ぶことがある**のが特徴。

抽出した各URLは一律`Job::OgpFetch`をpriority::LOWで積み、非同期で
OGP（`og:title`/`og:description`/`og:image`、正規表現による簡易メタタグ抽出）に加えて
oEmbed discovery（`<link rel="alternate" type=".../json+oembed">`の検出→JSON取得→
`html`フィールドからiframe src抽出、`crates/seiran-common/src/net.rs`の`fetch_ogp`が同じ
ページ取得で両方処理する）も行い、取得できた分だけ`post_link_cards`へ保存する。
取得したHTMLはUTF-8固定ではデコードせず、`net::decode_html_body`がHTTPレスポンスヘッダー
`Content-Type`の`charset`パラメータ→無ければHTML先頭（HTML5仕様のprescanに合わせ1024バイト
まで）の`<meta charset="...">`/`<meta http-equiv="Content-Type" content="...charset=...">`の
順で文字コードを検出し（`encoding_rs`でデコード）、いずれも無ければUTF-8にフォールバックする。
日本語圏のECサイト等にEUC-JP/Shift_JISでHTMLを返すものが少なくないため
（実例: 楽天市場の商品ページは`charset=EUC-JP`）、これを行わないとタイトル・説明文が
文字化けする。同じ`decode_html_body`はリモートインスタンスのトップページ取得
（`jobs::remote_instance_info_resolve::fetch_homepage_meta`、サーバー名・favicon取得）でも使う
（`crates/seiran-common/src/jobs/ogp_fetch.rs`）。`type`属性は仕様上`application/json+oembed`
だがSoundCloud等が非準拠の`text/json+oembed`を使うため、プレフィックスを問わず
`json+oembed`部分一致で判定する。取得失敗（DNS/SSRF拒否/非対応Content-Type等の
恒久的失敗）は静かに諦め、投稿自体の保存は妨げない。

Vimeoのように、oEmbed自体は提供するがHTMLにdiscoveryタグを載せていないサイト向けに、
`site_settings.oembed_allowed_domains`の各行は「domain」または
「domain,oembedエンドポイントURL」の形式を取れる。後者が指定されたドメインのURLは
HTML discoveryを試みず、常にそのエンドポイントへ`?url=<対象URL>&format=json`付きで
直接アクセスする（`oembed_whitelist::OembedWhitelist::fixed_endpoint_for`が解決、
`net::build_fixed_oembed_url`がクエリを組み立てる）。

oEmbedで見つかったiframe srcは、行の左側（domain）を許可ドメインとして後方一致判定し
（`crates/seiran-common/src/oembed_whitelist.rs`がTTL 60秒でキャッシュ）、許可された
場合のみ`post_link_cards.embed_src`/`embed_type`へ保存する（フロント`LinkCard.tsx`は
`embedSrc`の有無だけで埋め込みプレーヤー表示に振り分ける、`docs/ui_spec.md`参照）。
HTTPフェッチはSSRF対策込みの`seiran_common::net::fetch_validated_with_accept`
（`/proxy`・リモート絵文字インポートと共有、private/loopback/link-local等のIPを拒否し
リダイレクト先も毎回再検証する）を使う。oEmbedエンドポイント自体（discovery経由・
固定エンドポイントいずれも）も外部指定URLのため同じSSRF検証を通す。

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
| `Create`(Note) | リモートアクターupsert → HTML→内部リンクマーカー付きプレーンテキスト変換（6節参照）→ 絵文字tag解析（tag欠落時は同一ドメインの`remote_emojis`から本文shortcodeを補完）→ 可視性判定 → **重複排除**（3節参照）→ `posts` にINSERT → URLカード抽出（後述）・添付URL保存 → フォロワーへWS配信 |
| `Accept`(Follow) | `follows.status` を `accepted` に更新、通知。`object` は埋め込み Follow と URI 文字列の両形式を受理する。送信する Follow ID は `activities/follow/{local_actor_id}-{remote_actor_id}` とし、URI形式の応答でも関係を一意に復元した上で `Accept.actor` と送信先リモートactorの一致を検証する（Mitra互換、#200） |
| `Block` | リモートアクターupsert → ブロックされた側がブロックした側をフォローしていた関係があれば解消（`blocks` テーブルには書き込まない、通知も生成しない。11節参照） |
| `Undo` | `object.type` で分岐: `Like`/`EmojiReact`→リアクション削除、`Announce`→リポスト論理削除、`Follow`→フォロー解除、`Block`→ログのみ（DB上の巻き戻し対象なし） |
| `Delete` | `object`（文字列URIまたは`{"type":"Tombstone","id":...}`）の`ap_object_id`に一致する投稿を論理削除。**送信元アクター（`activity.actor`、HTTP Signature検証済み）が投稿者本人と一致する場合のみ**削除する（なりすまし対策）。一致する投稿が無い場合（アクター自身のDelete等）は無視。リモートアクター自体の退会（`Delete(Actor)`）は未対応 |
| `Update` | `object.type == "Question"`（アンケート票数更新）のみ受理する。`Delete`と同じなりすまし対策の上で`posts.poll`を更新し、`poll_update_received=true`・`poll_fetched_at=now()`をセットして`pollUpdated` WebSocketイベントを配信する。本文再編集の`Update`（`object.type == "Note"`等）は未対応で無視する。詳細は「アンケート」節「リモートアンケートの生存監視」参照 |
| `Announce` | リポスト保存。元ポストが未登録なら `resolve_reference`（`jobs::inbound_activity_process::reference`）で1段階だけリモート取得を試みてから紐付け。取得失敗（404/410は`gone`、それ以外は`pending`。#230/#231）でもリポストの箱（wrapper post行）自体は必ず保存する。取得できた場合の元ポスト保存処理（絵文字tag解析・引用/リプライ解決・可視性判定・CW/投票・ハッシュタグ・URLカード抽出・添付URL保存）は`save_ap_note_core`（`jobs::inbound_activity_process::note_save`、`Create`(Note)と共通の保存経路）で行うが、DMスレッド解決・通知・WS配信は行わない。元ポストが未解決（pending/gone）の間はリポスト通知も行わない |
| `Like` \| `EmojiReact` | Misskey は絵文字リアクションも `type:"Like"` 固定で送るため、**wire type ではなく `content`/`_misskey_reaction` の有無**で判定する |
| `Move` | アカウント引っ越しの受信処理（第1段階、送信側=引っ越し実行UIは未実装）。詳細は下記「アカウント引っ越し（Move）の受信」節参照 |

### 公開エンドポイント
`GET /users/:username`（Actor文書）、`GET /users/:username/outbox`（`?page=true`でOrderedCollectionPage）、`GET /.well-known/webfinger`、`GET /.well-known/nodeinfo` + `GET /nodeinfo/2.1`、featured（ピン留め）・lists（公開リスト）の各コレクション。

`outbox`の各投稿は`posts.ap_object_id`が実際にpush配送された種別と一致するよう組み立てる: `repost_of_post_id`がある行は、元ポストが`ap_object_id`を持てば`Announce`（`id`=自身の`ap_object_id`、`object`=元ポストの`ap_object_id`、`cc`に元投稿者のactor URIも含める）、元ポストが`at_uri`のみ(Bskyネイティブ)ならFediフォールバックと同じ本文（「🔁 author: bsky.app URL」）を持つ`Create(Note)`として表現する。リポスト行の`body`列は常に空文字列のため、これを無視して素通しで`Create(Note)`化すると、push配送済みの`Announce`とは別のAP object idを持つ空の`Note`がリモートに重複出現する。

`GET /nodeinfo/2.1`の`metadata.features`には`"emoji_reaction"`を含める。kmyblue（Mastodonフォーク）はカスタム絵文字リアクション対応の可否を、既知softwareリスト（Misskey系等）に載っていないインスタンスに対してはこのフィールドで判定するため（#167）。

**リモートnodeinfoの取得（受信側）**: 自分の`GET /nodeinfo/2.1`とは逆に、リモートFedi/seiran間連合の相手サーバーの`/.well-known/nodeinfo` → 実体ドキュメントを`jobs::remote_instance_info_resolve`が取得し、`software.name`/`metadata.nodeName`/`metadata.themeColor`を`remote_instance_meta`へキャッシュする（NoteCardリモートサーバー表示、`docs/database.md`参照）。`themeColor`未宣言時のfedibird/kmyblue/mitra/akkoma向け代替色もこのジョブ内で解決する。Bskyはこの経路を使わない（`docs/database.md`参照）。

### HTTP Signatures 検証
1. `Digest` ヘッダー必須（SHA-256ボディハッシュと一致確認）
2. `Signature` の `headers=` に `digest` が含まれることを確認
3. `keyId` のアクターURIと `activity.actor` の一致確認
4. `keyId` から公開鍵PEM取得（TTL付きキャッシュ、既定1時間）してRSA-SHA256検証。キャッシュ済み鍵での検証に失敗した場合はキャッシュを無視して1回だけ再フェッチし再検証する（リモートの鍵ローテーション対応）
5. 検証OK後、実処理はジョブキューへ委譲するのみ

### 署名付きGET（Authorized Fetch対応）
MastodonのAuthorized Fetch（`AUTHORIZED_FETCH=true`）等secure modeを有効にしたインスタンス
（例: songbird.cloud）は、未署名GETに401 `Request not signed`を返す。これはNote/Actor取得
だけでなく**受信検証（`verify_signature`が`keyId`へGETする公開鍵取得）にも及ぶ**ため、
対応していないと該当インスタンスとのフォロー・投稿受信・プロフィール表示等が軒並み失敗する。

`ApClient`はこれに対応するHTTP Signatures付きGET（`signed_get`/`fetch_object`/
`fetch_actor_signed`/`get_maybe_signed`）を持つ。署名対象は`(request-target) host date`の
3つのみ（POST用署名と異なりdigest/content-typeは含めない。Misskeyの
`ApRequestCreator#createSignedGet`と同形）。署名鍵はlist-relayプロキシアクター
（`system_actor::system_signing_key`）を流用し、専用の鍵ペアは持たない。

適用箇所（呼び出し元が`Option<(&str, &str)>`の署名鍵を渡せない場合のみ未署名にフォールバック）:
- `resolve_reference`（リプライ/引用/リポストの1段階フェッチ）、`upsert_remote_fedi_actor`
  （投稿・リアクション送信元アクターの解決。Follow/Create/Like/EmojiReact/Announce/Block/
  Flag/PollVote/Moveの全受信経路が共有）
- フォロー実行（`follow_exec::execute_follow`）、ターゲット解決（`handlers::target_resolve`）、
  未知アクター解決ジョブ（`jobs::remote_actor_resolve`）、フォロー中/フォロワー一覧同期・
  ライブ取得（`jobs::remote_follow_list_sync`、`ap::collection::fetch_ap_collection_uris`、
  `handlers::users::fetch_remote_follow_live`）
- メンション先inbox解決（`ap::deliver::infra::fetch_inboxes_by_ap_uris`）、過去ログ/featured
  取得（`ap::outbox::fetch_ap_history`/`fetch_ap_featured`）、Move/alsoKnownAs関連
  （`jobs::move_actor`/`also_known_as_sync`/`also_known_as_verify`）
- **受信側のHTTP Signature検証**（`ApClient::verify_signature`→`get_public_key_pem`。
  `seiran-federation-inbox::AppState::system_signing_key()`から渡す）

`ApActor.featured`は実装によりURL文字列（Mastodon等）とOrderedCollectionオブジェクトのインライン埋め込み
（bridgy-fed等）の両方があり得るため`serde_json::Value`で受ける。`fetch_ap_featured`は前者ならURL先を
別途GET、後者ならそのまま使う（他の`ApActor`フィールドと異なり型を緩めているのはbridgy-fedの実例に
合わせたもの）。

### 配送
`Job::ApDelivery{actor_id, kind}`（優先度高、最大10回リトライの指数バックオフ）。宛先は `follows` の `status='accepted' AND actor_type='fedi'` の `ap_inbox_url` 一覧が基本。全inboxへ署名付きPOSTをファンアウトし、**1件でも成功すればOk**（全滅時のみリトライ対象）。秘密鍵未設定時はリトライしても直らないため即座に破棄。

**反応アクティビティ（絵文字リアクション・返信・引用・リポスト。Undoを含む）の配送先拡張（#235）**: 上記の宛先に加え、対象ポスト（絵文字リアクションはリアクション対象そのもの、返信/引用は`reply_to_post_id`/`quote_of_post_id`、リポストは`repost_of_post_id`の参照先）を巡る「会話の参加者」全体へも配送する（`crates/seiran-common/src/ap/deliver/infra.rs::resolve_conversation_broadcast_inboxes`）。具体的には次の inbox の和集合:
1. 対象ポストの受信者 = 投稿者自身（Fedi remoteの場合のみ）とそのフォロワー
2. 対象ポストへの子ポスト（リポストラッパー・返信・引用）がある場合、その投稿者自身（Fedi remoteの場合のみ）とそのフォロワー
3. 対象ポストに付いている絵文字リアクションの reactor（Fedi remoteのみ）

自分のフォロワーにしか見えない反応が、対象ポストの投稿者・その会話に既に参加している人々には一切届かない（ブースト数・リアクション数がリモート側で更新されない）問題への対応。DM（`visibility='direct'`、`deliver_direct_message_to_ap`）はこの拡張の対象外（宛先は`post_recipients`のみ）。

通常投稿（`PostToFollowers`、DM以外）は、上記フォロワーに加え**本文中でメンションした相手のinbox**もフォロー関係と無関係に配送先へ加える（`crates/seiran-common/src/ap/deliver/infra.rs::fetch_inboxes_by_ap_uris`）。メンション先が既知（DB上に`actor_type='fedi'`の行がある）ならDBから、未知ならその場でアクタードキュメントを取得してinboxを解決する（DBへの保存は伴わない）。`to`にもメンション先のactor URIを含める（Mastodon等と同様の作法）。メンション先の取得に失敗した場合はそのメンション先だけをスキップし、他の配送は妨げない。

### リモートFediアクターのフォロー中/フォロワー全件取得（#68）
プロフィール画面で `GET /api/users/remote-follow-summary?actor_id=&direction=following|followers` を叩くと、`follows` テーブル（seiranが認知している関係のみ）とは独立に、相手のアクタードキュメントが持つ `following`/`followers` OrderedCollection URL（`ApActor.following`/`ApActor.followers`）へ直接問い合わせ、コレクション全体（`first`/`next` をページ辿り）を取得する（`seiran-common::ap::collection::fetch_ap_collection_uris`）。

- 同期取得は200msタイムアウト・最大500件のキャップ付き。成功すれば `remote_follow_snapshots` テーブル（`docs/database.md` 参照）へ丸ごと上書き保存しつつその場でレスポンスに含める。
- タイムアウト・取得失敗（非公開設定を含む）の場合は、既存スナップショット（あれば）を返しつつ `Job::RemoteFollowListSync{actor_id, direction}`（優先度低）を積む。このジョブはキャップ5000件でバックグラウンド全件取得し、スナップショットを更新する。次回リロード時に反映される。
- 同一 `(actor_id, direction)` への `RemoteFollowListSync` 投入は、APIプロセス内メモリのクールダウン（10分、`AppState::remote_follow_sync_recent`）で抑制する。フォロー数の多いアクターのプロフィールを短時間に何度もリロードしても、そのたびに最大5000件の`RemoteActorResolve`を積む重いジョブが積み直されないようにするため（#229。抑制されなければ`RemoteActorResolve`が低優先度キューを埋め尽くし、同じ優先度の`AlsoKnownAsVerify`等が飢餓状態になる）。
- レスポンスの各アイテムはローカルDBに `ap_uri` が登録済みなら display_name/avatar_url 等を付与し、未登録の URI はハンドル文字列のみの簡易表示にする（全件のプロフィールを都度リモート取得すると負荷・レイテンシが過大なため）。未登録の URI は同期取得・`RemoteFollowListSync` の双方で `Job::RemoteActorResolve{uri}`（優先度低）を積み、バックグラウンドでプロフィールを解決・`actors` へ upsert する（フォロー関係は作らない。次回表示からリッチ表示になる）。
- ローカルアクター・Bskyアクター（`ap_uri` を持たない）は対象外。Mastodon等はフォロー/フォロワー一覧をアカウント設定で非公開にできるため、HTTPエラーはエラー扱いにせず「非公開」として静かに空を返す。
- フロントエンドは `FollowListPanel`（タブが開かれた時点）でのAPIコールに加え、`ProfilePage` がプロフィール取得完了直後（タブ選択前）に `getRemoteFollowSummary` で先読みを開始する（`frontend/src/lib/remoteFollowSummaryCache.ts`）。タブを開いた瞬間に読み込み待ちが発生する体感を減らすための先読みキャッシュで、`FollowListPanel` はキャッシュ済みならそれを再利用する。
- `FollowListPanel` はローカルDB把握分（`follows`）とリモート直接取得分を見出しで分けず、既知/未知を問わず同じ見た目の1つのリストとして連結表示する。プロフィールカードのフォロー中/フォロワー人数は、レスポンスの `total_count`（ローカルの `follows` 件数とリモート取得件数のうち大きい方、`blended_follow_count`）で上書きし、`ProfilePage` が `getRemoteFollowSummary` の結果を先読み時点から購読して反映する。

### カスタム絵文字リアクションの送信（`EmojiReact`）
`reactions.content` の正規形は本家Misskey準拠で `:shortcode@host:`（ローカルは `:shortcode@.:`、Fedi受信のリモート絵文字はリアクション実行者の解決済みドメインで `:shortcode@{domain}:`。`docs/database.md`参照）。ローカルユーザーがカスタム絵文字でリアクションすると、`build_reaction_object`（`ap/deliver/activity.rs`）が Misskey/Fedibird 互換の `tag: [{"type":"Emoji","name":":shortcode:","icon":{"type":"Image","url":...}}]` を付与した `EmojiReact` を組み立てる。`content`/`_misskey_reaction` にはホスト付き正規形をそのまま載せるが、**`tag[].id`/`tag[].name` は本家Misskey準拠で常にホストなしの素の shortcode**（`parse_reaction_shortcode_and_host`でホスト部分を分離してから組み立てる）というAP wire特有の非対称性がある。受信側の `build_emoji_map`/`extract_emoji_tag_url`（`ap/client.rs`・`jobs/inbound_activity_process`）もこの非対称性を前提に、`handle_reaction`がワイヤ`content`からホスト部分を除いた素shortcodeでtag照合する。画像URLの解決は `EmojiRepository::find_url_by_shortcode`（`custom_emojis`/`media_files`/`storage_providers` を JOIN）で行い、未登録shortcodeは `INVALID_REACTION_CONTENT`/`UNKNOWN_EMOJI` として拒否する（`handlers/notes/validation.rs`・`handlers/notes/reactions.rs::create_reaction`）。ローカル絵文字ピッカーからはリモートホスト付きshortcode（`:shortcode@remote.example:`）を選べないため、`validate_reaction_content`はこれを拒否する。ATP（Bsky）はカスタム絵文字非対応のため、`commit_like` の `emoji` 拡張フィールドにホスト付き正規形をベストエフォートで載せるのみ（画像は送らない）。

### 投稿本文のカスタム絵文字
ローカル投稿は作成時に本文の`:shortcode:`を`custom_emojis`と一括照合して`posts.emoji_map`へ保存する。ActivityPub配送時は保存済みmapのうち配送本文に実際に現れるものを`tag: [{"type":"Emoji","name":":shortcode:","icon":{"type":"Image","url":...}}]`へ変換し、Mention/Hashtag tagと併送する。受信側が`object.id`を再取得する実装でも情報を失わないよう、canonicalな`GET /notes/{id}`のNote表現にも同じEmoji tagを含める。AP受信はNoteのEmoji tagを第一情報源とし、送信元がtagを欠落させた場合は同一ドメインから過去に収集した`remote_emojis`で本文shortcodeを補完する。さらにリレー等がCreateの埋め込みNoteから未知のEmoji tagを省略している場合は、未解決shortcodeを検出したときだけ`object.id`のcanonical NoteをAP取得し、そこからtagを補完する（#148）。Bluesky Jetstream受信でも、本文shortcodeをローカル`custom_emojis`と照合して`emoji_map`を保存する（いずれも#126）。

### アカウント引っ越し（Move）の受信（第1段階）
`jobs::inbound_activity_process::handle_move`。送信側（自分のアカウントを他インスタンスへ引っ越す操作）は未実装で、他サーバーからの`Move`受信のみ対応する。

- **アクティビティ形式**: Mastodon実装慣習に合わせ、`actor`（移転元本人、`object`と同一）→`target`（移転先URI）として扱う。`object`が`actor`と異なる場合は処理しない。
- **なりすまし対策**: `target`のアクター文書を取得し、`alsoKnownAs`（`ApActor::also_known_as`、単一文字列/配列いずれの形式も受理）に移転元の`actor`URIが含まれている場合のみ処理する。含まれていない（移転先が引っ越しに同意していない）場合はログのみで無視する。
- **移転元が未知の場合**: `actors`にAP URIで見つからない（誰もフォロー・リスト登録していない）場合は移行すべき関係が無いため無視する。
- **フォロー関係の付け替え**: `FollowRepository::find_all_local_followers_with_status`で移転元をフォロー中/フォロー申請中（status問わず）のローカルアクター全員（実ユーザーと、リスト機能の`list-relay`プロキシアクター（10節参照）の両方を含む）を取得し、1件ずつ移転先へ付け替える。
  - 既に移転先をフォロー中/フォロー申請中なら、移転元の`follows`行を削除するだけ（重複フォローしない）。
  - そうでなければ、当該フォロワー自身の身元でフォロー先へ`Follow`を送信し、移転元の`follows`行を削除して移転先へ`upsert_pending`する。
  - `list-relay`プロキシアクターも「フォロワーの一種」としてこのループで自然に付け替わるため、移転元をリストに入れていたことによる代理フォロー（1節・10節参照）も同じ経路でカバーされる。
  - 実ユーザー（`actors.user_id`が`Some`）宛にのみ、結果に応じて`notifications.type = "moveRefollowed"`（フォローし直した）または`"moveAlreadyFollowing"`（既にフォロー済みだった）を生成する。Misskey APIには無いseiran独自拡張で、`notifier_actor_id`=移転元、`related_actor_id`（`notifications`テーブル拡張列）=移転先を指す。8節参照。
- **リストメンバーシップの付け替え**: `ListRepository::list_ids_containing_actor`で移転元を含むリストを列挙し、各リストで移転元を`remove_member`・移転先を`add_member`する（ATP側の公開リスト同期は対象外、未対応）。

### プロフィールの「別のアカウント」（alsoKnownAs）
AP Moveの`alsoKnownAs`と同じ語彙を、引っ越し検証とは独立に「同一人物が持つ複数アカウントの相互リンク表示」用途へ転用したseiran独自拡張。`actor_also_known_as`テーブル（`docs/database.md`参照）で管理する。owner（登録主）にはローカルユーザー（プロフィール編集画面での自己登録）とリモートFediアクター（本人のAP actor文書が公開する`alsoKnownAs`を取り込んだもの）の2種類がある。

- **ローカルユーザーの登録**: プロフィール編集画面（リストのメンバー追加UIを流用）から`POST /api/users/also-known-as`（`target`はユーザー名/`@user@domain`/URL/DID、`handlers::target_resolve::resolve_and_upsert_target`で解決）で追加、`DELETE /api/users/also-known-as/:actor_id`で削除。上限は`MAX_ALSO_KNOWN_AS`（10件）。
- **リモートFediアクターの取り込み**: リモートFediアクターのプロフィール表示のたびに`Job::RemoteAlsoKnownAsSync{owner_actor_id}`（優先度低）を積み、本人のAP actor文書を取得して`alsoKnownAs`配列の各URIを解決（自ドメインはローカルDB参照、`did:`はBsky、それ以外の`https://`はFediアクターとして`jobs::remote_actor_resolve`と同様にupsert）し、`actor_also_known_as`へ同期する（無くなったエントリは削除、新規のみ追加。既存エントリの`verified`/`last_checked_at`は保持）。このAPI経由の登録・削除は行わない（本人のAP文書の内容をそのまま反映するのみ）。
- **表示**: `GET /api/users/profile`のレスポンス（`ProfileResponse.also_known_as`）に含まれる。ローカル・リモートFedi両方のプロフィールで「別のアカウント」として登録済みアカウント一覧をアイコン付きで表示する（bskyアクターのプロフィールは対象外）。
- **相互検証（✅バッジ）**: 相手側（fedi/ローカルのみ、bskyは対象外——Bsky DID documentの`alsoKnownAs`はハンドル↔DID対応専用で任意URI列挙の仕組みが無いため）も逆向きにこちらを`also_known_as`として指定していれば`verified=true`として✅を表示する。
  - **表示時再検証パターン**: プロフィール表示のたびに、ローカルownerなら登録エントリごとに`Job::AlsoKnownAsVerify{owner_actor_id, target_actor_id}`を、リモートFedi ownerなら上記の`RemoteAlsoKnownAsSync`（同期後に取り込んだ各エントリへ`AlsoKnownAsVerify`を積む）を積み、キャッシュ済みの`verified`/`last_checked_at`（`actor_also_known_as`テーブル）を更新する。表示自体は常にキャッシュ値を返すだけで、ユーザーが情報の古さを感じてリロードする頃には反映されている想定（`docs/architecture.md`参照。このパターンは今後の他機能でも再利用予定）。
  - ローカルターゲットはDB直接参照（相手の`actor_also_known_as`にこちらの`actor_id`があるか）で完結。fediターゲットは`ApClient::fetch_actor`でリモートのAP actor文書を取得し、`ApActor::claims_also_known_as`（Move検証と共用）でowner側のURI（ownerがリモートなら`actors.ap_uri`、ローカルなら自ドメインから組み立てたURI）が含まれるか確認する。
- **AP文書への公開**: `GET /users/:username`のレスポンスに、ローカルユーザーが登録した「別のアカウント」を自己申告として`alsoKnownAs`に含める（検証は読み手側の責務、Mastodon等と同じ流儀）。ターゲット種別に応じてURI形式を作り分ける: ローカルは自ドメインのactor URI、fediは保存済みの`ap_uri`、bskyは`did:...`形式（Bridgy Fedと同じ流儀、実機確認済み）。
- **Misskey側の設定方法**: 相手がMisskeyの場合、「設定→その他→アカウントの移行」の「移行元のアカウント」欄にこちらのハンドルを追加すると、Misskey側のAP actor文書の`alsoKnownAs`にこちらのURIが載る。Mastodon等は対応方法がサーバーにより異なる。

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
- `seiran-api::handlers::xrpc::{repo,server,sync}` — `getRecord`/`listRecords`/`describeRepo`/`uploadBlob`（repo）、`describeServer`/`resolveHandle`/`createSession`/`refreshSession`/`deleteSession`/`getSession`/`createAppPassword`/`listAppPasswords`/`revokeAppPassword`（server）、`getRepo`/`getBlob`/`listBlobs`/`listRepos`/`getLatestCommit`/`subscribeRepos`（sync）

ローカルユーザーの投稿は `AtpCommitService` が**ジョブキューを介さず直接** MSTコミット・署名し、`atp_repo_events` にイベント記録、公式Relay（`bsky.network`）へ `requestCrawl` を送って購読される。

**CORS**: `crates/seiran-api/src/lib.rs::router` の `CorsLayer`（`allow_origin`のpredicate）は、リクエストパスが`/xrpc/`または`/.well-known/`で始まる場合は無条件でオリジンを許可する。`/api/*`（seiranネイティブAPI、フロントエンド専用）は`FRONTEND_ORIGIN`＋自ドメインのみに制限するが、AT ProtocolのXRPCは仕様上bsky.app等の外部クライアントがブラウザから直接叩くことを前提とした公開APIのため対象外にする必要がある（公式Bluesky PDSも`Access-Control-Allow-Origin: *`を返す）。この分岐が無いと、bsky.appのログイン画面で「サービスに接続できません」となりATクライアントからのアクセスが一切成立しない。`allow_headers`も`Any`（ワイルドカード）にしている。bsky.appは`atproto-proxy`/`x-bsky-topics`等、事前に予測しづらいカスタムヘッダーを様々なXRPCメソッドで送ってくるため、個別ヘッダーを列挙するとヘッダーが増えるたびにプリフライトで弾かれるモグラ叩きになる（2026-08-31実機確認、`x-bsky-topics`未許可で`getTrends`失敗）。`allow_credentials`を付与していない（Cookie不使用）ため`Any`にしても安全（`docs/coding_rules.md`参照）。

### DID解決・PLC登録・ハンドル検証（アカウント登録時）
1. ローカルでP-256鍵生成、`did:plc:xxx` をローカル計算のみで確定
2. Cloudflare API で `_atproto.{username}.{domain}` TXTレコードをセット（ハンドル検証用）
3. `plc.directory` へ genesis operation をPOST
4. 失敗時は新しい鍵で再生成し最大3回リトライ

`com.atproto.identity.resolveHandle` は `{username}.{local_domain}` 形式ならDBの `actors.at_did` から即答する（自PDS管轄）。それ以外のハンドルは `seiran_common::atp::resolve_external_handle` がDNS TXT（`_atproto.{handle}`、Cloudflare DNS-over-HTTPS経由）とHTTP well-known（`https://{handle}/.well-known/atproto-did`）を並行で試して解決する（各5秒タイムアウト、両方失敗時は404）。bsky.app等のクライアントはログイン中PDSに任意ハンドルの`resolveHandle`を投げてくるため、自ドメイン外を無条件で400拒否すると呼び出し元が壊れる（該当ハンドルのプロフィールページがフリーズする等）。

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

### レコード一覧・同期系エンドポイント（サードパーティインデクサー対応）
Clearsky等のサードパーティツールは firehose を購読し続ける代わりに、PDS へ直接 `listRecords` を叩いて投稿履歴を取得することがあるため、以下を実装している。

- `GET /xrpc/com.atproto.repo.listRecords` — `repo`（DIDまたは`{username}.{local_domain}`ハンドル）+ `collection` を指定し rkey 順にページングする（`cursor`/`reverse`/`limit`(1-100, デフォルト50) 対応）。`app.bsky.feed.post` は `posts` テーブル、それ以外は `atp_records` テーブルを起点にし、いずれも `atp_blocks` のDAG-CBORをデコードして `value` を返す。
- `GET /xrpc/com.atproto.repo.describeRepo` — `handle`/`did`/保持コレクション一覧（`app.bsky.feed.post`を含む）/`didDoc`（`did:plc`ならplc.directoryへプロキシ取得）を返す。
- `GET /xrpc/com.atproto.sync.listRepos` — `at_did IS NOT NULL` なアクター（＝ATPリポジトリを持つ）をid順にページングして返す。PDSクローラーがアカウント一覧を発見するためのエンドポイント。
- `GET /xrpc/com.atproto.sync.getLatestCommit` — `actors.at_repo_cid`/`at_repo_rev` をそのまま返す。
- `GET /xrpc/com.atproto.sync.listBlobs` — `atp_blobs`（動画パイプライン提出物）のCID一覧をid順にページングして返す。

### postgateスタブ応答・getTrendsスタブ
seiranはpostgate（引用可否の制限）・トレンド機能の作成/実装を持たないが、bsky.appクライアントがこれらのエンドポイントを叩いた際に保守的にエラー扱いされるのを防ぐため、無害なスタブ応答を返す。

- `GET /xrpc/com.atproto.repo.getRecord?collection=app.bsky.feed.postgate`（`crates/seiran-api/src/handlers/xrpc/repo.rs`の`get_record_postgate`）— `atp_records`に実レコードが無い場合は合成レコードを返す。AT Protocol仕様では「レコード不在=制限なし」のはずだが、404のままだとbsky.app側が引用不可として扱うことがあった（2026-08-31 マイケル報告）。CIDは`encode_generic_record`で実際にDAG-CBORエンコードした値から計算する（ダミー値ではない）。実レコードが存在する場合（将来postgate作成機能を実装した場合）はそちらを優先する。`repo`がローカルユーザーなら常に`embeddingRules: []`（postgate作成機能自体が無いため）。`repo`がリモートBskyユーザーの場合は`posts.bsky_quote_disabled`（`fetch_bsky_gates`が投稿取り込み時に取得済み）を見て正しい値を合成する（2026-09-01 マイケル指摘: 以前はリモート投稿についても無条件で「制限なし」を返しており、実際に引用不可な投稿でも嘘の応答になっていた）。該当ポストを未取得の場合は判断材料が無いため「制限なし」のまま（フェイルオープン）。
- `GET /xrpc/app.bsky.unspecced.getTrends`（`xrpc_get_trends`）— 常に`{"trends": []}`を返す。未実装だと`atproto-proxy`ヘッダー無しの直接呼び出しが`xrpc_proxy_fallback`の`MethodNotImplemented`（404）になる。

### セッション認証（外部ATプロトコルクライアント対応）
公式Blueskyアプリ等の外部ATプロトコルクライアントがseiranアカウントへ直接ログインできるようにする仕組み。既存のMisskey API互換ログイン（`LocalAuthProvider`、`sub: "local|{user_id}"`のJWT）とは完全に別の認証系で、`LocalAuthProvider`に追加したメソッド群（`generate_atp_session`/`verify_atp_access_token`/`verify_atp_refresh_token`、`crates/seiran-common/src/auth/local.rs`）が同じ`secret`でHS256署名・検証する（`sub`にDIDそのものを積むため既存の`sub.strip_prefix("local|")`とは自然に衝突しない）。

- `createSession`は**本アカウントのメインパスワードと、`com.atproto.server.createAppPassword`（既存のseiranログイン、`Authorization: Bearer`の自社トークンで保護）で発行する専用アプリパスワード（`xxxx-xxxx-xxxx-xxxx`形式、`atp_app_passwords`テーブルにargon2ハッシュで保存）の両方を照合対象とする**（公式Bluesky PDS準拠。bsky.app自身もメインパスワードで`createSession`を呼んでおり、PDSはメインパスワードを拒否していない。アプリパスワードはサードパーティに安全に権限を渡すための任意のオプション）。`listAppPasswords`/`revokeAppPassword`も同じ自社トークン認証で保護する。
- `POST /xrpc/com.atproto.server.createSession` — `identifier`（ハンドルまたはDID）+ `password`でログインし、accessJwt（2時間）とrefreshJwt（90日）を発行する。identifier解決失敗・パスワード未設定時もダミーハッシュ照合を行ってから同一の`AuthenticationRequired`（401）を返し、アカウント存在有無が応答やタイミングから漏れないようにする。
- `POST /xrpc/com.atproto.server.refreshSession` — refreshJwtを検証し新しいペアを発行する。古いrefreshJwtの`jti`は同時に失効させる（ワンタイム・ローテーション、`atp_refresh_tokens`テーブルで管理）。
- `POST /xrpc/com.atproto.server.deleteSession` — refreshJwtの`jti`を失効させる（ログアウト）。
- `GET /xrpc/com.atproto.server.getSession` — accessJwtを検証し、現在のセッション情報（did/handle）を返す。
- **`jsonwebtoken`の`Validation::default()`は`validate_aud: true`かつ`aud: None`のため、クレームに`aud`が存在するだけで`InvalidAudience`として一律拒否する**。`aud`クレームを積むJWT（`AtpSessionClaims`）を検証する際は`set_audience`の明示呼び出しが必須。

### 書き込み系エンドポイント（外部ATプロトコルクライアントからの直接書き込み対応）
`createSession`で取得したaccessJwtを使い、外部ATクライアントが任意コレクションのレコードを直接書き込めるようにする。共通の認証ゲート（`crates/seiran-api/src/handlers/xrpc/repo.rs`の`authenticate_atp_write`）が、accessJwt検証済みDIDと`repo`パラメータが一致するかを確認し、他人のリポジトリへの書き込みを拒否する。

- `POST /xrpc/com.atproto.repo.createRecord`/`putRecord`/`applyWrites`が`app.bsky.feed.post`を受けた場合、`post_from_record::create_post_from_record`（`crates/seiran-api/src/handlers/xrpc/post_from_record.rs`）の専用パイプラインを経由する。ATP標準クライアント（bsky.app等）が組み立てたレコード（`text`/`facets`/`embed`/`reply`）を`posts`テーブルへ変換し、`insert_full`でINSERT・ハッシュタグ抽出・mention_facets保存・リプライ/引用/メンション通知・Fedi配送（`ApDeliveryKind::PostToFollowers`をenqueue）まで行った上で、クライアント提供の`record`をそのままDAG-CBORエンコードして`AtpCommitService::commit_post_record`でATPリポジトリへコミットする（`posts.at_uri`/`at_cid`/`at_rkey`も同時更新）。`commit_post`（seiranネイティブ投稿APIがDBの添付情報からembedを再構築する経路）とは異なり、embedを再構築しない点が特徴。可視性は常に`public`固定（Bskyプロトコルに可視性の概念が無いため）。
  - `facets`は6節と共通の`seiran_common::atp::facets`（`apply_bsky_facets`）で解析し、`#link`は本文へMarkdownリンクとして焼き込み、`#mention`は`mention_facets`カラムへ保存する。
  - `embed`の画像・動画blobは`resolve_blob`（`post_from_record.rs`）で解決する。`media_files`（seiranネイティブAPI経由のアップロード）にあれば`post_attachments.media_file_id`で参照、無ければ`atp_blobs`（`uploadBlob`経由の直接アップロード、下記参照）を検索しCDN直リンクURLを`remote_url`として保存する（`atp_blobs`は`media_files`と別テーブルで外部キー参照できないため）。
  - `embed.record`/`recordWithMedia`（引用）・`reply.parent.uri`は、それぞれ`parse_bsky_embed_quote_uri`・`find_id_by_at_uri`でローカルDBの`quote_of_post_id`/`reply_to_post_id`に解決する（未取得の引用/リプライ先は通常投稿として保存、6節のJetstream受信と同じ方針）。ブロック関係にある引用先・リプライ先は拒否する。
  - `putRecord`/`applyWrites#update`もこの経路を通すが、Bskyには投稿編集機能が無いため常に新規作成として扱う（既存の`at_uri`と衝突する`rkey`を指定した場合は`posts.at_uri`のUNIQUE制約でエラーになる）。
- `deleteRecord`/`applyWrites#delete`が`app.bsky.feed.post`を受けた場合、`post_from_record::delete_post_by_rkey`が`handlers::notes::delete_note`（`DELETE /api/notes/:id`）と同じ処理を行う: `rkey`から`at_uri`を組み立てて対象投稿を特定（本人以外の投稿は`NOT_YOUR_POST`で拒否）、`soft_delete_by_id`で論理削除、実際にFediへ配送済みだった場合のみ`ApDeliveryKind::DeleteNote`をenqueue、最後に`delete_atp_record_generic`（他コレクションの`deleteRecord`と同じ経路）でATPリポジトリからも削除する。
- それ以外のコレクション（`app.bsky.graph.list`等）は`AtpCommitService::commit_generic_record`/`delete_atp_record_generic`が汎用に処理する。受け取ったレコードJSONを`seiran_common::atp::json_to_ipld`でDAG-CBORへ変換してコミットする（`getRecord`/`listRecords`が使う`ipld_to_json`の逆変換）。`{"$type":"blob","ref":{"$link":cid}}`パターンは`collect_blob_cids`が再帰的に検出し、`subscribeRepos`フレームの`blobs`に積む。
- `applyWrites`は**単一commitではなく、要素ごとに個別のcommitとして順番に処理する**（AT Protocol仕様とは異なる簡易実装）。途中の要素が失敗すると、それより前の要素は既にコミット済みのまま残る（部分適用、全体ロールバックなし）。
- `app.bsky.actor.profile`のみ特別扱いする。`createRecord`/`putRecord`/`applyWrites`が受け取った場合、ATPリポジトリへのコミットに加えて`sync_profile_to_actors`（`crates/seiran-api/src/handlers/xrpc/repo.rs`）が`actors`テーブル（`display_name`/`bio`/`avatar_media_id`/`banner_media_id`）にも反映する。既存の`AtpCommitService::commit_profile`は「`actors`→ATPレコード」の片方向だったため、これが無いと外部ATクライアント（bsky.app等）からのプロフィール編集がseiranのUI/APIに反映されない不整合が生じる。`avatar`/`banner`はCID参照（`{"$type":"blob","ref":{"$link":cid}}`）なので、CIDのmultihash（sha256そのもの）を逆算して`media_files.sha256`と突き合わせ、対応する行があれば`avatar_media_id`/`banner_media_id`をそこに向ける（`resolve_blob_media_id`、`xrpc_get_blob`と同じ手法）。対応する`media_files`行が無い場合（bsky.app経由で新規アップロードされ`atp_blobs`にしか無い画像等）は該当フィールドを更新せず既存値を維持する（`atp_blobs`→`media_files`への変換は未実装）。

### uploadBlob（画像・動画アップロード）の2つの呼び出し元
`POST /xrpc/com.atproto.repo.uploadBlob`（`crates/seiran-api/src/handlers/xrpc/repo.rs`の`xrpc_upload_blob`）は`Authorization`ヘッダーのJWTを見て2種類の呼び出し元を区別する。まず`local_auth.verify_atp_access_token`（`createSession`発行の通常セッションJWT、HS256）での検証を試み、成功すればATP標準クライアント本人（bsky.app等が投稿へ画像/動画を添付する際の通常経路）として扱いバイト列を無条件でS3へ保存する。失敗した場合のみ、既存のサービス間認証JWT検証（ES256、`iss`/`aud`/`lxm`。Bsky公式動画パイプライン`app.bsky.video.uploadVideo`がトランスコード完了後に呼び戻すコールバック専用）にフォールバックする。後者は「進行中の動画パイプラインジョブ（`media_files.bsky_video_status='pending'`）が無いDIDからの呼び出しを拒否」という悪用防止チェック（DID本人なら誰でも自己署名JWTを作れるため）を維持するが、前者はユーザー本人であることを認証済みのためこのチェックは不要（`store_uploaded_blob`の`require_pending_video_job`引数で分岐）。

保存先は`atp_blobs`テーブル（`media_files`とは別、`docs/database.md`参照）。`com.atproto.repo.createRecord`側の`resolve_blob`（前述）がこの`atp_blobs`を検索してCDN直リンクURLへ解決する。

### クライアント設定（`app.bsky.actor.getPreferences`/`putPreferences`）
`GET`/`POST /xrpc/app.bsky.actor.{get,put}Preferences` はaccessJwt認証必須（本人のみ）。`preferences`配列はATPリポジトリのMSTには入らないプライベートデータで、`atp_preferences`テーブル（`actor_id` PRIMARY KEY、`preferences` JSONB）に不透明な配列としてそのまま保存・返却する（中身の`$type`ごとの意味は解釈しない）。`putPreferences`は全置換（差分マージではなく配列を丸ごと差し替える、AT Protocol仕様通り）。Bluesky公式の年齢確認フロー（`#personalDetailsPref`の`birthDate`）はこのエンドポイントが無いと動作せず、bsky.appから「生年月日の設定を読み込むことができませんでした」エラーになる。

### XRPCプロキシ（`atproto-proxy`ヘッダー）
`app.bsky.feed.getTimeline`/`searchPosts`/`app.bsky.notification.listNotifications`等のAppView専用メソッドは、PDS自身が実装するのではなく、**PDSがAppView（`api.bsky.app`等）への透過プロキシとして振る舞う**ことで実現される。これが無いとBluesky公式クライアントはログインできてもタイムライン・検索・通知が「接続できません」で軒並み失敗する（2026-08-20 マイケル実機確認）。

- 明示的な`.route()`にマッチしなかったXRPCリクエストは`Router::fallback`（`crates/seiran-api/src/handlers/xrpc/proxy.rs`の`xrpc_proxy_fallback`）が受け止める。`atproto-proxy: <did>#<service-id>`ヘッダー（例: `did:web:api.bsky.app#bsky_appview`）が無ければ`MethodNotImplemented`（404）を返す。
- ヘッダーがあれば、`resolve_service_endpoint`（`crates/seiran-common/src/atp/did_resolve.rs`）が`<did>`のDIDドキュメントの`service`配列から`#<service-id>`に対応する`serviceEndpoint`を解決する。
- クライアントのaccessJwtをそのまま転送するのではなく、ユーザーの`at_signing_key_pem`で新たに短命サービス間認証JWTを署名して転送する（`sign_service_auth_jwt`、`uploadBlob`コールバック検証と共通のロジック）。**`aud`クレームはサービスDIDのみ（`#service-id`フラグメントを含めない）**にすること。フラグメント込みで署名すると対象サービス側で`BadJwtAudience`になる（実機確認）。`#service-id`はエンドポイント解決にのみ使う。
- レスポンス（ステータス・Content-Type・ボディ）はそのままクライアントへ返す。

### Bsky公式Relayの新規PDSアカウント数上限に注意
Bsky公式Relay（`bsky.network`）は新規（未検証）PDSに対してホスト単位のアカウント数上限を設けており、上限を超えて登録されたアカウント（作成順で後の方）は `host-throttled` 扱いとなり、そのアカウントのコミットは `subscribeRepos` の配信対象から意図的に除外される（PDS側にエラーは一切返らず、`requestCrawl` も200 OKを返し続けるため、PDS側のログからは検知できない）。「特定ユーザーだけ投稿がbsky.appに反映されない」という報告を受けたら、まずこの上限超過を疑う。indigoの`cmd/relay/relay/account.go`にロジックがあり、ローカルでindigo/relayを動かして自PDSのホストレコード（`account_count`/`account_limit`）を直接確認することで検証できる。

`GET https://bsky.network/xrpc/com.atproto.sync.getHostStatus?hostname=seiran-beta.org` で自ホストの `accountCount`/`status` は取得できるが、`accountLimit`（上限値そのもの）は非公開で分からない。

**上限緩和の申請先は `github.com/bluesky-social/pds` リポジトリのissue**（Discordではない。例: [#357](https://github.com/bluesky-social/pds/issues/357)）。ただし対応は不安定であてにできない: 上限緩和自体はされることがあっても、既に `host-throttled` になった個別アカウントのステータス解除はされない。issueが応答なく放置され続けることもある（例: [#359](https://github.com/bluesky-social/pds/issues/359)）。恒久的な解決手段として期待しないこと。

**`AccountTakedown` と `host-throttled` は別症状であり切り分けが必要**。`public.api.bsky.app` の `app.bsky.actor.getProfile?actor={did}` を対象DIDに叩いて判定する:
- `{"error":"AccountTakedown","message":"Account has been suspended"}` → モデレーションによる個別アカウントの意図的な停止（`host-throttled`とは無関係）。異議申し立ての実効的な窓口は無い
- `{"error":"InvalidRequest","message":"Profile not found"}`（PDS側には `app.bsky.actor.profile` レコードが存在するにもかかわらず）→ `host-throttled` が疑わしい

複数アカウントの一括切り分けは、`com.atproto.repo.listRecords` で対象アクター一覧のDIDを集め、各DIDに対して上記 `getProfile` を順に叩いて `error` フィールドで分類するとよい（1件ずつでは判別しにくいが、傾向を見れば `AccountTakedown`（個別・少数）と `Profile not found`（複数アカウントにまたがる場合は上限超過を疑う）を区別しやすい）。

### Jetstream 経由の取り込み（`seiran-atp-repo::firehose`）
`wss://jetstream1.us-east.bsky.network/subscribe?wantedCollections=app.bsky.feed.post&wantedCollections=app.bsky.feed.like` に接続。

- **wantedDids絞り込み**: ローカルユーザーがフォロー中、またはいずれかのリストのメンバーであるBsky DIDの集合を30秒間隔でポーリングし変化があれば再接続。無関係な投稿・Likeの際限ない取り込みを防ぐための必須の絞り込み。
- **リーダー選出**: 複数プロセス起動時の重複接続を避けるため、Redisベースの `JetstreamLeaderElector` でリース制御。モノリスモードはRedis無しでも常時接続、split-role構成はRedis障害時にフェイルクローズ。
- **cursor永続化**: 直近処理イベントの `time_us` を `site_settings`（汎用KV）に5秒間隔で保存し、再接続時に引き継ぐ（プロセス停止中のイベント取りこぼし防止）。
- 保存対象は wantedDids に含まれるDIDのみ。投稿は同梱の `record.text`/`record.createdAt` をそのまま使う（AppView再取得不要）。`app.bsky.embed.images`/`video`/`recordWithMedia` を解析しCDN URLを組み立てて添付保存。`app.bsky.embed.external` のうち、Bluesky GIFピッカーが生成するTenor/Klipy URLは、クエリに埋め込まれた動画識別子から `t.gifs.bsky.app` / `k.gifs.bsky.app` のMP4（MP4がないKlipyはWebM）URLへ変換して添付保存する。GIF判定に失敗した`external`（YouTube/Spotify/x.com/一般URL等）は、`url`/`title`/`description`/`thumb`を`post_link_cards`（`docs/database.md`参照、`position=0`固定）にそのまま保存する。`app.bsky.embed.external`にはiframe情報が無いため、INSERT成功後に非同期の`Job::LinkCardEmbedResolve{post_id, position: 0, url}`（`priority::LOW`）をenqueueし、oEmbed discoveryで見つかったembed srcをホワイトリスト判定した上で`embed_src`/`embed_type`だけをUPDATEする（`crates/seiran-common/src/jobs/link_card_embed_resolve.rs`）。フロントは`embedSrc`の有無で埋め込みプレーヤー表示/x.com/一般URLの3種に振り分ける（`frontend/src/components/note/LinkCard.tsx`）。`record.facets`（`#link`/`#mention`/`#tag`）は6節の方式で処理する。
- **GIFアニメの2つの経路**: (1) Tenor/Klipy GIFピッカー由来（上記の`app.bsky.embed.external`、Bluesky動画CDNのMP4/WebM URLへ変換）。(2) GIFファイル直接アップロード由来。Bluesky動画パイプラインでMP4にトランスコードされ`app.bsky.embed.video`として配信されるが、元がGIFだったことを示す`presentation:"gif"`が付与される（通常の動画添付との唯一の違い）。いずれも`post_attachments.is_gif=TRUE`で保存し、フロントは`HlsVideo`の`isGif` propで自動再生・ミュート・ループ・コントロール無し表示に切り替える（`docs/database.md`参照）。
- Like（`app.bsky.feed.like`）は create/delete で `reactions` へINSERT/DELETE、通知・リアルタイム配信。
- `app.bsky.feed.post` の delete commit（`operation:"delete"`）は `at://{did}/app.bsky.feed.post/{rkey}` を組み立て、一致する `posts.at_uri` を論理削除する。`at_uri` 自体がイベント発行元の `did` から組み立てられるためLikeと同様になりすましは原理上不可能（他者のdidの投稿を指せない）。取り込んでいない投稿（フォロー対象外だった等）の delete イベントは無視。
- **Repost（`app.bsky.feed.repost`）はタイムライン投稿として`posts`に保存する**（`handle_inbound_repost_create`、Fediverseの`Announce`受信〔`handle_announce`〕と対称の処理）。リポスト対象がDBに未取り込み（`wantedDids`絞り込みで元々購読対象外だった投稿等）なら`app.bsky.feed.getPosts`でAppViewから直接フェッチして著者ごと保存してから`repost_of_post_id`でリンクする。`posts.at_uri`にリポストレコード自体のURI（`at://{did}/app.bsky.feed.repost/{rkey}`）を保存し、Fedi版の`ap_object_id`と対になる（Bskyのリポストに可視性の概念は無いため`visibility`は常に`public`固定）。delete commitは`app.bsky.feed.post`と同じ`at_uri`ベースの論理削除（`handle_inbound_post_delete`／`soft_delete_by_at_uri`はコレクションを問わず共用）。対象がローカル投稿の場合のみ通知も作る。
- **AppView直接フェッチ経路（`fetch_single_bsky_post`/`upsert_bsky_post`、`seiran-common::atp::client`）の添付復元**: 上記のリポスト未取り込みフェッチに加え、検索結果保存・ピン留め投稿同期・「開く」機能（`POST /api/open`）でも同じ`fetch_single_bsky_post`/`upsert_bsky_post`を使う。取得した`record.embed`は、Jetstream経由の通常投稿取り込みと同じ解析ロジック（`seiran-common::atp::embed`の`parse_bsky_embed_attachments`/`parse_bsky_embed_link_card`、画像・動画・GIF・URLカードに対応）で添付・URLカードへ復元する（新規作成時のみ。既存投稿への`upsert`はスキップ）。`upsert_bsky_post`はJetstream経由と同様、URLカードINSERT成功後に`Job::LinkCardEmbedResolve`をenqueueする（呼び出し元は`Arc<dyn JobQueue>`を引数で渡す）。

### 返信許可（threadgate）・引用可否（postgate）のグレーアウト表示
リモートBsky投稿に対し、閲覧中ユーザーが実際に返信・引用できるかを`posts`テーブルに保存されたゲート情報から評価し、NoteCardの返信/引用ボタンをグレーアウト＋ツールチップ表示する。

- **取得**: `upsert_bsky_post`（`seiran-common::atp::client`、投稿を新規保存した直後のみ）が`fetch_bsky_gates`で投稿と同じrkeyの`app.bsky.feed.threadgate`/`app.bsky.feed.postgate`を`com.atproto.repo.getRecord`（AppView経由）で個別取得し、`posts.bsky_reply_allow`（threadgateの`allow`配列そのもの、レコード不在なら`NULL`=制限なし）・`posts.bsky_quote_disabled`（postgateの`embeddingRules`に`#disableRule`が含まれるか）へ保存する。postgateは仕様上「全員可」「全員不可」の二値のみで部分許可は無い。
- **評価**（`queries::attach_reply_quote_gates`、`crates/seiran-api/src/handlers/notes/queries.rs`）: タイムライン/詳細/返信一覧等の`NoteResponse`組み立て後に一括評価し`replyBlocked`/`quoteBlocked`へ反映する（未ログイン時は評価しない）。`bsky_reply_allow`のルールはOR条件、投稿者自身は常に許可:
  - `#mentionRule` — 投稿の`mention_facets`に閲覧者のDIDが含まれるか（追加問い合わせ不要）。
  - `#followingRule` — `follows`テーブルに`author→viewer`の行があるか（追加問い合わせ不要。リモート投稿者が閲覧者をフォロー中なら、その関係はfirehose経由で既に`follows`に取り込まれている）。
  - `#listRule` — リスト所有者がseiranローカルユーザーなら`lists`/`list_members`で即判定。リモート所有リストは`bsky_remote_list_membership_cache`（24時間TTL）を参照し、未登録/期限切れなら`Job::BskyListMembershipResolve`（`app.bsky.graph.getList`をページングして全メンバーDIDを取得）を積んでフェイルオープン（今回は「制限なし」として扱い、誤ってボタンをグレーアウトしない。次回表示時にはキャッシュが埋まっている）。
- フロント（`NoteCardActions`）は`isGateReplyBlocked`/`isGateQuoteBlocked`でボタンを`disabled`にし、ツールチップ「投稿者が返信（引用）を許可していません」を表示する。可視性由来の`isPrivateQuoteTarget`（followers_only/direct）とは理由が異なる別フラグ。

### uploadBlob / getBlob・動画パイプライン
`getBlob` はCIDのmultihashからsha256を逆算し `media_files`/`atp_blobs` を検索してCDN URLへリダイレクトする（ストレージ本体を自前で再配信しない）。

動画・音声は原本をそのまま保存（トランスコードなし）、ffmpegでメタデータとサムネイルのみ抽出。`deliver_to_bsky=true` の場合、Bsky公式動画パイプライン（`app.bsky.video.uploadVideo`）へ提出する。**音声ファイルはBskyに専用embedが無いため、グレー背景の静止画+音声トラックのmp4に変換**してから動画として提出する。提出は非同期で `Job::BskyVideoPoll` が完了をポーリングし、間に合わなければ `app.bsky.embed.external`（URLカード）にフォールバックする。動画添付投稿は結合未確定の間 `Job::BskyPostCommitDeferred` でBskyコミット自体を遅延させ、早すぎるコミットによるexternal固定化を防ぐ。

### Bsky embed選択（#227）
AT Protocolは1投稿につきembedを1種類（画像最大4枚 / 動画1本 / 外部リンクカード1件）しか持てないため、ローカル投稿が静止画・アニメGIF・動画・本文URLの複数を同時に使っている場合、どれをBsky向けembedにするかを選ぶ必要がある（引用＋静止画は例外、下記「引用投稿と静止画添付の共存」参照）。

- **選択の単位（`CreateNoteRequest.bsky_embed_choice`、`crates/seiran-api/src/handlers/notes/dto.rs::BskyEmbedChoice`）**: `Poll`（アンケート、#228）／`Images`（添付済みの非アニメ静止画グループ全体、最大4枚）／`Attachment{id}`（特定の添付ファイル1件、アニメGIFまたは動画/音声のいずれか）／`Url{url}`（本文中の特定URL）の4種類。省略可能で、その場合は`resolve_bsky_embed`（`crates/seiran-api/src/handlers/notes/delivery.rs`）が固定優先順位（アンケート→静止画→アニメGIF→動画/音声→本文URL、いずれも添付順・出現順が最も早いもの）で自動選択する。Misskey互換API等、本フィールドを送らないクライアントとの後方互換のため、バックエンドはこの省略を許可し「選択必須」のハードエラーは持たない。「候補が2種類以上あるのに選ばせない」という制約は、seiran自身のフロントエンド（`PostComposer`）が送信ボタンを`disabled`にすることでのみ担保する。
- **アニメGIF判定**: `media_files.is_animated_image`（`storage::image::ImagePipeline::AnimatedPassthrough`由来の場合に`true`、`docs/database.md`参照）で、ローカルアップロードの静止画/アニメGIFを区別する。音声添付は動画同様の枠（「動画」候補）として扱う（音声→グレー背景動画変換機能により実際にBsky video embedになるため）。
- **URL選択とローカル表示の同期**: `Url{url}`選択時、`resolve_bsky_embed`は選択されたURLのOGP（`og:title`/`og:description`/`og:image`、`crate::net::fetch_ogp`、SSRF対策込み）を同期取得して`app.bsky.embed.external`を組み立てると同時に、同じデータを`post_link_cards`（`position=0`）へINSERTする。`fetch_ogp`はoEmbed discoveryも同時に行うため、ホワイトリスト判定を通過すれば`embed_src`/`embed_type`も同じINSERTで保存される（`app.bsky.embed.external`自体にはiframe概念が無いためBsky配送ペイロードには影響しない、seiranローカル表示専用）。これは、静止画/GIF/動画の選択と異なりURLは「選んで初めてカード化する」ものであり、ローカル（seiran自身のNoteCard/LinkCard表示）でも選択結果を反映するための明示的な永続化（マイケル指摘）。本文からそのURLを削除しても選択自体は孤児として有効なまま残る（フロントは他の選択肢を選ぶまでラジオボタンリストにそのURL項目を残し続ける）。OGP取得に失敗しても選択は常に尊重し、素の`External`（title/description空）でコミットする。
- **URL選択のActivityPub配送への反映（`fedi_url_append_needed`）**: Fedi（AP）にはBskyのembed概念が無く、本文に書かれたURLでしか参照先を示せない。`Url{url}`選択時、選択したURLが投稿本文に既に含まれていれば何もしないが、含まれない場合（本文からそのURLを削除した後の孤児選択等）は、AP配送用の本文（DBの`posts.body`自体は変更しない、`ApDeliveryKind::PostToFollowers.body`の上書きのみ）の末尾に`\n\n{url}`を追記する。これはクロスプロトコル引用（`ApQuote::AppendUrl`）と同じ「AP配送時だけ上書きする」仕組みの流用。引用投稿（`bsky_quote_embed`がSome）は`bsky_embed_choice`自体が無視されるため対象外。
- **引用投稿と静止画添付の共存（`app.bsky.embed.recordWithMedia`）**: 引用投稿（`bsky_quote_embed`がSome、`BskyEmbed::Record`）に静止画添付があれば、`bsky_embed_choice`の明示選択に関わらず常に先頭4枚を`app.bsky.embed.recordWithMedia`のmedia側として引用と一緒に配送する（`delivery::collect_bsky_quote_images`、マイケル指摘。添付物が画像だけの投稿は選択の余地が無いのに黙って画像が欠落していた不具合の修正、2026-09-01）。動画/GIF/URLリンクカード/アンケートは対象外（`bsky_embed_choice`同様に無視、動画はBskyPostCommitDeferredとの結合が複雑なため当面非対応）。Fediリモート引用のフォールバック（`BskyEmbed::External`）はrecordWithMediaの対象外（externalはmedia側と併記できない）。
- **動画パイプライン結合待ち（`Job::BskyPostCommitDeferred`の簡素化）**: 選択（または自動選択）が指す動画/音声添付がBsky動画パイプライン結合未確定（`bsky_video_status`が`ready`/`failed`のいずれでもない）の場合のみ、`resolve_bsky_embed`は`Pending(media_file_id)`を返し、その1件のIDだけをジョブへ渡してコミットを遅延させる（画像/URL選択、または既に確定済みの動画/音声選択は即座にコミットする）。ジョブは対象1件の状態だけを再確認し、`ready`ならVideo embed、`failed`または`SETTLE_TIMEOUT_SECS`（70秒）超過ならフォールバックURL（`/api/media/{id}/watch`、簡易視聴ページへのリンクカード）でコミットする。
- **ラジオボタンリストを出せない場合のURLリンクカード添付（`CreateNoteRequest.link_card_urls`、`delivery::attach_link_cards_from_urls`）**: Bsky embed選択のラジオボタンリスト（単一選択）は「Bsky配送オンかつCW中でない」場合にしか表示されない。それ以外（Bsky配送オフ、またはCW中）でも本文中にURLがあれば、seiranは（Bskyと違い）1投稿に複数のURLリンクカードを同時に持てるため、フロントエンドは代わりにチェックボックスリスト（複数選択）を表示する。チェックしたURLは`link_card_urls`として指定順どおり送られ、各URLを`fetch_ogp`で取得して`post_link_cards`へ`position=0..N`で保存する（`resolve_url_embed`と異なりBsky embed用のサムネイル再ホストは行わない、seiranローカル表示専用のため）。本文からそのURLを削除してもチェック自体は孤児として有効なまま残る（ラジオボタン版のURL孤児化と同じ仕様）。チェックボックスリストが出せる状態からラジオボタンリストを出せる状態（Bsky配送オン かつ CWオフ）へ切り替わった瞬間、チェック済みURLのうち最もインデックスの小さいものが`bskyEmbedChoice`のURL選択へ引き継がれる。
- **投稿直後のレスポンス・WebSocketブロードキャストへの反映**: `create_regular_post`は`deliver_regular_post`完了後（＝ラジオ選択・チェックボックス選択いずれの`post_link_cards`保存も完了した後）に`fetch_link_cards_map`で読み戻し、`NoteResponse.link_cards`へ設定する。これにより投稿者自身の即時タイムライン挿入・他ユーザーへのWebSocketブロードキャストの両方で、リロードなしにURLリンクカードが反映される。
- **チェックボックス選択URLのActivityPub配送への反映（`fedi_link_card_urls_append_needed`）**: `link_card_urls`はBsky配送オフ・CW中でも使われる仕組みのため、Fedi配送への本文追記（`fedi_url_append_needed`と同じ「AP配送用の上書き本文にだけ追記、`posts.body`自体は変更しない」方式）はBsky配送状態・CW状態に関わらず常に判定する。本文（引用なら`quote_body`、無ければ`posts.body`）に含まれないチェック済みURLを出現順に集め、既存の`fedi_append_url`（Bsky embed選択URLの単一追記）と合わせて改行区切りで本文末尾へ追記する。

### アンケート（#228）
Fediverse仕様のアンケート（選択肢2〜10・単一/複数選択・期限なし/日時指定/経過時間）。`posts.poll`（`{multiple, options:[{name,votes}], endTime}`）・`poll_votes`・投票API（`POST /api/notes/:id/poll-vote`）・投票UIはFedi受信Question用に既に実装済みで、ローカル作成でも同じ列・同じ経路をそのまま流用する（追加のスキーマ変更は無い）。

- **受理・保存（`CreateNoteRequest.poll`、`PollCreateRequest`）**: 選択肢2〜10件（`validate_poll_choices`、各1〜100書記素）。期限は絶対時刻（`expiresAtIso`、seiranネイティブUI）／Unix epochミリ秒（`expiresAt`、Misskey本家互換）／送信時刻からの相対秒数（`expiresInSeconds`、seiranネイティブUIの期限プリセット）のいずれかから絶対`endTime`を計算する（優先順はこの順）。`visibility=="direct"`（DM）ではアンケート作成自体を`POLL_NOT_ALLOWED_FOR_DM`で拒否する。メディア添付との併用は禁止しない（Misskey互換方針）。
- **AP `Question`配送（`ap/deliver/activity.rs::apply_poll_to_note_object`）**: `posts.poll`がある投稿は`Create(Note)`の`object.type`を`"Note"`ではなく`"Question"`にし、`multiple`に応じて`oneOf`（単一選択）/`anyOf`（複数選択）へ選択肢を`{"type":"Note","name","replies":{"type":"Collection","totalItems"}}`の形で列挙、`endTime`があれば設定する（受信側`normalize_ap_poll`と対称）。同じ変換は`GET /notes/:id`のAP直接取得（`handlers::notes::get_note_ap`）でも必須：フォロワーでないリモート（Mastodon等）は`Create`配送を受け取らず、投稿URLを検索・閲覧した際にこのエンドポイントを直接GETしてobjectを取得するため、ここで`"Note"`のまま返すと本文だけでアンケートが添付されていないように見える。リモートユーザーがこのアンケートに投票した際の受信処理は`handle_poll_vote`（`inbound_activity_process`）が`find_id_and_actor_by_ap_object_id`でローカル/リモート問わず汎用的に処理するため追加実装不要。DM配送（`deliver_direct_message_to_ap`）はアンケート作成自体が禁止されているため常に`poll: None`。
- **Bsky向け自己URLリンクカード（`resolve_bsky_embed`の`Target::Poll`、`resolve_poll_embed`）**: ATPにはアンケート概念が無いため、`url`をこのポスト自身の詳細ページ（`https://{local_domain}/notes/{post_id}`、`ap_object_id`と同一形式）、`title`は空文字列（投稿の言語が決定できないため言語依存の見出しは付けない）、`description`は選択肢名だけのプレーンテキスト箇条書き（`- 選択肢A\n- 選択肢B`、HTMLタグ無し・得票バー無し。作成時点の得票は常に0でBsky embedは再コミットされず得票を反映できないため）、`thumb`は無し、で`app.bsky.embed.external`を組み立てる。`post_link_cards`へのINSERTは行わない（投稿自身が`NoteResponse.poll`経由で既にリッチなアンケートUIを表示するため、自分自身を指すリンクカードを重ねるのは冗長・表示上不自然）。Bskyユーザーは現状この詳細ページに来ても投票できない（Bskyクレデンシャルでのログイン投票は将来のステップ）。
- **優先順位**: アンケートは「Bsky embed候補」の中で常に最優先（静止画より前）。本文にURLがあってもアンケートと競合するラジオボタンリストの1候補として並ぶだけで、アンケート自体は自動的には他候補に優先される。

#### リモートアンケートの生存監視

`posts.poll`は取り込み時点のスナップショットのため、そのままでは以後の投票増加が反映されない。
push（`Update(Question)`受理）とpull（フォールバック再フェッチ）の2経路で追従させる。
スキーマの詳細は`docs/database.md`「リモートアンケートの生存監視」参照。

- **push**: `jobs::inbound_activity_process::update::handle_update`が`Update`アクティビティのうち
  `object.type == "Question"`のみを受理する（本文再編集の`Update`は別件、今回は非対応）。
  `delete.rs`と同じなりすまし対策（`activity.actor`が投稿者本人と一致するか）を行った上で
  `normalize_ap_poll`で正規化し、`posts.poll`・`poll_update_received=true`・
  `poll_fetched_at=now()`を更新する。
- **pull（フォールバック）**: `Update(Question)`を送ってこない実装への保険。投稿を表示用に
  読み込む経路（`handlers::notes::queries::enqueue_stale_poll_fetches`、17箇所ある
  `attach_remote_instance_info`呼び出しすべてに隣接して呼ぶ）が、renote/quote越しも含めて
  「pollを持つ・リモート投稿・`poll_update_received=false`」な投稿ごとにしきい値を計算し
  `PostRepository::find_stale_remote_poll_post_ids`へ照会し、対象を`Job::PollFetch{post_id}`
  として積む。しきい値は締切前は
  「`poll_fetched_at`が直近10分より古いか」、締切後（`poll.closed`優先、無ければ`endTime`）は
  「`poll_fetched_at`が締切時刻より古いか（＝締切後まだ一度も取得できていないか）」で、
  締切済みでも一律除外しない（締切前に取り逃した票数を締切後も取り戻せるようにするため）。
  ジョブは`ap_client.fetch_object`（`resolve_reference`と同じシステム署名鍵）で対象Noteを
  再GETし、`normalize_ap_poll`で正規化して`posts.poll`・`poll_fetched_at`を更新する。取得結果
  が`Question`でなくなっていた場合は`poll_fetched_at`だけ進めて以後叩き直さない。
- **反映**: push/pullいずれの経路も最後に`broadcast_poll_update`（`streaming.rs`）で
  `pollUpdated` WebSocketイベントを配信する。ローカル投票時と同じイベントのため、
  フロントエンド（`usePollState`）は無改修で追従する。

### CW（閲覧注意、#229）
`posts.content_warning`は既存列（Fedi受信CW用）で、ローカル作成でも同じ列をそのまま流用する
（スキーマ変更なし）。CWは「Bsky embed候補」の1つとして並ぶのではなく、**すべてを無条件で
上書きする**点がアンケート（#228）と異なる。

- **受理・保存（`CreateNoteRequest.content_warning`、Misskey本家`cw`パラメータもエイリアス）**:
  `validate_cw`が空文字（trim後）禁止・100書記素以内を検証する。
  `visibility=="direct"`（DM）ではCW作成自体を`CW_NOT_ALLOWED_FOR_DM`で拒否する。
- **AP `summary`配送**: `posts.content_warning`があれば`Create(Note)`の`object.summary`に
  そのままセットする（Mastodon/Misskey互換のCW表現）。本文（`content`）・添付・アンケート・
  引用は通常通り配送し、Fedi側クライアントが`summary`の有無でCW UIを出し分ける。
- **Bsky配送（`deliver_regular_post`のCW分岐、`build_cw_bsky_embed`）**: CWが設定されている
  投稿は、画像/GIF/動画/URL/アンケートの候補選択（`resolve_bsky_embed`）も引用embed
  （`bsky_quote_embed`）も一切参照せず、常に次の1件だけをコミットする:
  - 本文（`app.bsky.feed.post`の`text`）: 投稿本文ではなく**CWガイド文**（メンション変換・
    facet生成もガイド文に対して行う）
  - embed: `url`はこのポスト詳細ページに`#open_cw`ハッシュ（開いた状態を表すフラグメント）を
    付けたもの、`title`は言語非依存の固定文字列`"Open"`、`description`・`thumb`は無し
  - 引用投稿であってもCWが優先される（「隠された本文・添付物・引用すべてを見るには
    URLリンクカードからseiranの記事詳細ページへ飛ぶ」という設計を引用にも適用する拡張）
  - Bsky embed選択のURL追記ロジック（`fedi_url_append_needed`）もCW中は呼ばない
    （`bsky_embed_choice`自体を無視するため）

### フォロワー検知ポーリング（`seiran-atp-repo::bsky_follower_poll`）
リモート Bsky アクターがローカルユーザーをフォローしたことを検知する経路。Jetstream の `wantedDids` は投稿・Likeの「発行者DID」でのフィルタであり、フォロー元（＝新規に自分をフォローしてきたアクター）を事前に知る手段が無いため、Jetstream購読では検知できない。そのため `app.bsky.graph.getFollowers`（AppView公開エンドポイント、認証不要）をローカルBskyリンク済みユーザーごとに`BSKY_FOLLOWER_POLL_INTERVAL_SECS`環境変数（デフォルト60秒）間隔でポーリングし、`follows`テーブルの既存フォロワー集合との差分から新規フォローを検知する常駐タスク（`seiran-atp-repo::run`内で`tokio::spawn`）。

- **baseline seed機構**: 機能導入時点で既に実フォロー済みの全フォロワーが初回ポーリングで一斉に「新規フォロー」と誤検出され通知が大量発生するのを防ぐため、`actors.bsky_followers_baseline_done_at`（NULL=未シード）をアクター単位のマーカーとして使う。未シードのユーザーは初回ポーリングで全フォロワーページを辿って `follows` へ無通知でINSERTするだけに留め、完了後にマーカーを立てる。以降のポーリングはbaseline済みとして扱い、新規フォロワーのみ通知する。
- **ページング**: `getFollowers`は新しい順で返る前提で、baseline済みなら既知フォロワーに到達した時点でそのユーザーの処理を打ち切る（`STEADY_STATE_MAX_PAGES=20`が安全上限）。未baselineなら`HARD_MAX_PAGES=1000`まで辿り切る。
- 新規フォロワーはDID未知なら`getFollowers`のレスポンス（handle/displayName/avatar）でそのまま`upsert_remote_bsky`する（`fetch_bsky_profile`への追加往復は不要）。通知は`source_uri`に`bsky-follow:{follower_actor_id}:{local_actor_id}`を付与し、複数インスタンス同時ポーリング時の重複INSERTを部分ユニークインデックス経由で防ぐ。
- **スコープ外**: Bsky側のアンフォロー検出（`follows`からの削除）は未実装。

### 生年月日（`actors.birth_date`/`birth_date_public`）
Fediverse（AP）とBluesky（ATP）では生年月日の可視性の位置づけが異なるため、同じ`actors.birth_date`から配送先ごとに別ルールで連合する。

- **AP側**: `birth_date_public=true`の場合のみActorオブジェクトに`vcard:bday`（`@context`に`"vcard": "http://www.w3.org/2006/vcard/ns#"`を追加）として含める。表現はMisskeyの`ApRendererService`実装に合わせている（`packages/backend/src/core/activitypub/ApRendererService.ts`）。`birth_date_public`のデフォルトは`false`で、Misskey本家自体にはこの可視性切り替えが無いseiran独自拡張。Pull取得（`GET /users/:username`、`crates/seiran-federation-inbox/src/handlers/actor.rs`）とPush配信（`Update(Person)`、`crates/seiran-common/src/ap/deliver/activity.rs`の`build_person_object`）の両方で同じ条件分岐を行う。
- **ATP側**: `app.bsky.actor.defs#personalDetailsPref`（`docs/protocols.md`3節「クライアント設定」参照）は`birth_date_public`と無関係に常に非公開（accessJwt認証済みの本人のみ`getPreferences`で取得可）。`putPreferences`で`#personalDetailsPref`を受け取ると`actors.birth_date`を更新するが、`birth_date_public`（Fediverse公開設定）自体は変更しない。

### ポストの言語（`app.bsky.feed.post`の`langs`）
ポストは言語プロパティ（ISO 639-1、2文字コード）を持てる。Bsky配送（`app.bsky.feed.post`の`langs`フィールド、1言語のみ）にのみ意味を持ち、AP配送では使わない。

- **選択肢（`CreateNoteRequest.language`）**: `seiran_common::SUPPORTED_LANGUAGES`（ja/en/zh/ko/es/de/frの7言語）。表示言語設定（`users.language_preference`、`seiran_common::SUPPORTED_DISPLAY_LANGUAGES`）とは異なり中国語のバリエーション（`zh-Hant`/`zh-Hans`）を持たず`zh`単一。`crates/seiran-api/src/handlers/notes/creation.rs::create_regular_post`が`seiran_common::is_supported_language`で検証し、未対応言語は`UNSUPPORTED_LANGUAGE`で拒否する。省略可能で、その場合は`posts.language`が`NULL`のまま、Bskyコミット時に`langs`フィールド自体を省略する（Misskey互換APIクライアント等、本フィールドを送らないクライアントとの後方互換）。
- **保存とBskyコミットへの反映**: `posts.language`に保存し、`AtpCommitService::commit_post`/`commit_quote`の`lang`引数として`encode_bsky_feed_post`（`crates/seiran-common/src/atp/repo.rs`）へ渡す。`Some`なら`langs`を1件の配列として設定し、`None`ならフィールド自体を省略する。動画パイプライン結合待ち（`Job::BskyPostCommitDeferred`）でコミットが遅延する場合も、ジョブが`posts.language`を都度読み直して同じ`lang`を渡す。
- **フロントエンドのデフォルト**: 投稿フォームの言語選択ボタン（`docs/ui_spec.md` 2.4b節）の初期値は現在の表示言語（`i18n.language`）を`i18n.postLanguageBase()`でポスト言語（7言語）へ丸めた値で、送信のたびに変わる「最後に送信した値」方式（Fedi/Bsky配送先トグル等の`composerDefaults`）とは異なる。表示言語が`zh-Hant`（繁體中文）/`zh-Hans`（简体中文）のどちらでも、丸め後の値は`zh`になる。

### アルゴリズムレコメンドからの除外（`app.bsky.actor.contentVisibilityDeclaration`）
設定画面「プライバシー」のチェックボックス1件から、Bsky Discoverフィード等のアルゴリズムレコメンドから自分の投稿を除外するよう要求する。`GET`/`POST /api/account/content-visibility`（`crates/seiran-api/src/handlers/account.rs`）がローカルキャッシュ`actors.hide_from_algorithmic_recommendations`（`docs/database.md`参照）を読み書きし、更新時に`AtpCommitService::commit_content_visibility`（`crates/seiran-common/src/atp/service.rs`）が`app.bsky.actor.contentVisibilityDeclaration/self`（rkey固定`self`、フィールドは`hideFromAlgorithmicRecommendations`のみ）をPDSへコミットする。既に`chat.bsky.actor.declaration`（DM受信可否設定）と同じ「単一boolean値のself-keyレコード」パターンで、`atp_records`の既存有無でcreate/updateを判定する。レコードが存在しない場合は`false`として扱われる（Bluesky公式仕様）。ActivityPub側に対応する概念が無いため、この設定はBsky限定。

## 4. クロスプロトコル配送ルール

中核ロジックは `seiran-api::handlers::notes::delivery`。`classify_post` が元ポストの出自を判定する: `actors.domain == local_domain` ならローカル、それ以外は `(ap_object_id有無, at_uri有無)` から `FediRemote`/`BskyRemote`/`LocalOrSeiran`（両方あり＝他seiranサーバー）に分類する。

- **リプライ**: 配信先制御（`resolve_reply_context`内`reply_delivery_allowed`）は`classify_post`の分類を使わず、元ポストの`ap_object_id`/`at_uri`の実体の有無を直接見る。`ap_object_id`が無ければ Fedi 配信しない、`at_uri`が無ければ Bsky 配信しない（ローカル投稿でも`deliver_to_bsky=false`等で`at_uri`を持たない場合を含む。実体を持たないプロトコルへ配信すると親と無関係な独立ポストとして誤配信されるため）。親の可視性が `followers_only` ならリプライも継承する。
- **引用**: 元ポストの `at_uri`/`at_cid` が揃っていれば、Bsky側は `app.bsky.embed.record` でネイティブ引用する（引用元投稿自身に静止画添付があれば `app.bsky.embed.recordWithMedia` として画像も一緒に配送する、「Bsky embed選択」節「引用投稿と静止画添付の共存」参照）。Fediリモートのみの場合や、AP/ATPの両IDを持っていても `at_cid` が未取得の場合は、投稿者名・本文・先頭画像（なければアバター）を持つ `app.bsky.embed.external` URLカードへフォールバックする。AP側は `ap_object_id` があればMisskey互換の `quoteUrl` / `_misskey_quote` として配送し、Bskyにしか実体がない投稿は受信サーバーがAPオブジェクトとして解決できないため、bsky.app URLを本文末尾へ追記する。配送する Create の埋め込み Note と `/notes/:id` の公開 AP Note は同じ本文・引用フィールドを返し、受信側による再取得でも不整合を起こさない。ローカル・Fedi・Bskyのどの投稿も、Fedi/Bskyの両配送先を個別選択して引用できる。
- **引用受信（#116）**: APは `quoteUrl`、`_misskey_quote`、`tag[].rel=https://misskey-hub.net/ns#_misskey_quote` の順に引用URIを抽出し、Misskey/Fedibird/kmyblueが本文へ自動付加する同一投稿を指すフォールバック表現を除去する。判定は完全一致に加え、ホストと末尾のstatus ID（英数字6文字以上）が両方含まれていれば同一とみなす（`quote_uri_matches`）。Fedibirdは`quoteUrl`にAPオブジェクトID形式（`/users/{user}/statuses/{id}`）を使う一方、本文中のフォールバックリンクには表示用URL形式（`/@{user}/{id}`）を使うことがあり、完全一致だけでは検出漏れするため（実例: #117195910938631045。ActivityPub仕様には別表記URLを同一オブジェクトと判定する正規化手続きが無く、WebFingerもアクター発見用でありNote単位のURL正規化には使えないため、Mastodon/Fedibird系実装の命名規則に頼ったヒューリスティック）。フォールバック表現には`RE:`/`QT:`（コロン付き、Fedibird/Misskey）と`RE `/`QT `（コロン無し、kmyblueの一部投稿、実例: kb.mu7ou.com）の両方があり、位置も本文**末尾**（Fedibird/Misskey、kmyblueの一部）と本文**先頭**（kmyblue標準、`<p class="quote-inline">...</p>`として自動生成、実例: kblue.10rino.net・kmy.blue）の両方があるため、末尾・先頭それぞれで独立に判定する。`class="quote-inline"`は位置に関わらず確実な検出手がかりになるが、`sanitize_ap_content_html`が全タグから`class`を剥がすため、サニタイズ**前**の生HTMLの段階でのみ使える（`strip_quote_inline_paragraph_html`）。本文とフォールバック表現が同一行に同居する投稿（ユーザーが手動で引用URLを書いた等）は本文を巻き込むリスクがあるため対象外。Bsky Jetstreamは `app.bsky.embed.record.record.uri` と `recordWithMedia.record.record.uri` を抽出する。引用先がローカルDBに存在すれば `quote_of_post_id` を設定し、無ければ`resolve_reference`が1段階だけフェッチを試みる（#230/#231）。それでも解決できなければ`quote_of_ap_uri`/`quote_of_ref_status`（`pending`/`gone`）に未解決状態を記録した上で通常投稿として保存する（フォールバック行は引用URI抽出さえできていれば解決可否に関わらず除去する）。リプライ（`inReplyTo`）の解決も同じ`resolve_reference`を使い、同様に`reply_to_ap_uri`/`reply_to_ref_status`へ未解決状態を記録する。1段階フェッチで新たに取得したノート自身が持つリプライ/引用/リポスト参照はさらに辿らず、DB照合のみで`pending`/`gone`を記録する（無限再帰防止）。
- **リポスト**: 元ポストが `ap_object_id` を持つなら Fedi へは `Announce`。持たず `at_uri` のみ(Bskyリモート)ならテキスト投稿（「🔁 author: bsky.app URL」）にフォールバック。Fediリモート投稿をBskyへ配送する場合は、本文を「🔁」のみとし、元の`ap_object_id`を`app.bsky.embed.external`（URLカード）で添付する。カードのtitleは元投稿者の表示名とID、descriptionは元投稿本文、thumbは先頭の添付画像（画像がなければ投稿者アイコン）をPDS配信可能なblobとして設定する。`visibility` が `followers_only`/`direct` の場合、フォロワー限定配信を持たない Bsky へのリポストはスキップする。`Announce`/`Undo(Announce)`はフォロワーのinboxに加え、元ポストがFediリモートなら元投稿者のinboxにも配送し（`cc`にも元投稿者のactor URIを含める）、相手サーバーでのブースト数反映・通知を成立させる（リアクション配送と同じ方針）。`Announce`のAP `id`は`/notes/:id`ではなく`/announces/:id`（`docs/architecture.md` 8.1節参照、リモートユーザーがブラウザで踏んだ場合は`/notes/:id`へリダイレクトする）。
- **投稿削除**（`DELETE /api/notes/:id`、本人のみ）: DB上は論理削除（`posts.deleted_at`）のみで、リアクション・他ユーザーによるリポスト・通知等の関連行はカスケード削除しない（読み取り側が一貫して`deleted_at IS NULL`を見る設計）。配送は「実際に配送済みだった経路」にのみ行う: `deliver_fedi`が真かつ`visibility != 'direct'`なら`ApDeliveryKind::DeleteNote`（フォロワー全員へ`Delete(Note)`）をenqueue、`at_rkey`が保存済みならBsky側レコードを`delete_atp_post`で削除。`direct`（DM）投稿は`DeleteNote`がフォロワー配送しか持たないため配送対象外（本来の宛先には届かない、既知の制約）。

## 5. 重複排除・マージ（水際防御）

同じ内容の投稿が複数ルートで自サーバーに届くケースへの対処。3シナリオ:

1. **ループバック**（自サーバー投稿の逆輸入）: 受信Noteの `id`/`url` が `https://{local_domain}/notes/{id}` パターンに一致すれば `parent_original_post_id` にセットしてINSERT（重複許容 + リンク）。
2. **他seiranサーバー間マージ**: 送信側は投稿作成時に `seiran_post_uuid`（UUID v4）を生成しAP Noteに `seiranUuid` として埋め込む。受信側は `find_by_seiran_uuid` で既存行を検索し、あれば新規INSERTせず `ap_object_id` をUPDATEするのみ。
   - **既知の制約**: `seiran_post_uuid` は ATP 側（Bskyレコード本体）には埋め込まれていない。そのため Jetstream 経由で先に取り込まれた投稿に後から AP の `Create` が届いても `find_by_seiran_uuid` は一致せず、**別行として新規INSERTされる**（マージされない）。現状「AP側が先」の場合のみ機能する。
3. **一般ブリッジ重複**: Noteの `url` が `https://bsky.app/profile/{did}/post/{rkey}` 形式なら `at://` URIへ変換し既存ポストを検索、あれば `parent_original_post_id` にリンク（重複許容 + リンク）。

**Actor解決の自ドメインガード**: リモートActor URI解決処理（`upsert_remote_fedi_actor`/`resolve_fedi`）は、URIが `https://{local_domain}/users/{username}` 形式で自ドメインを指す場合、`seiran_common::ap::extract_local_username` で判定してローカル行をそのまま返す（新規 `fedi` 行は作らない）。ローカル行は `insert_local` が設定する `ap_uri`（`https://{domain}/users/{username}`）を持つため、万一このガードを経由しなくても `find_by_ap_uri`/`upsert_remote_fedi` の `ON CONFLICT (ap_uri)` により重複INSERTは自然に防がれる（二重防御）。

**Bsky側Actor解決の自DIDガード**: Bskyリモートアクター解決処理（`follow_bsky`/`resolve_bsky`/`fetch_bsky_profile_from_appview`/`persist_appview_posts`等）は、`fetch_bsky_profile`等で得たDIDが `find_by_did` でローカル行にヒットした場合、そのローカル行をそのまま返し `upsert_remote_bsky` を呼ばない。ローカルユーザーの完全なBskyハンドル表記（`{username}.{local_domain}`、`.`を含み`@`を含まないため `create_follow`/`resolve_and_upsert_target` のターゲット判別ロジック上はBsky ATPハンドルと区別できない）を宛先文字列としてこの経路に入ることがあり、ガードを欠くと `upsert_remote_bsky` の `ON CONFLICT (at_did) DO UPDATE` でローカル行の `username` 列がAppView側のハンドル表記（ドット付き）で上書きされてしまう（実際に発生した事故: `actors.username` がDNSラベルとして不正な形になり `seiran_common::username::is_valid_local_username` の前提が壊れる）。フォロワー検知ポーリング（`bsky_follower_poll`）・Jetstream受信（`firehose::resolve_or_upsert_bsky_actor`）・検索結果取り込み（`search::persist_appview_posts`）は元々 `find_by_did` 経由のローカル判定を先に行う実装だったため影響を受けない。

## 6. 本文中のリンク・メンション表現

Bluesky facet・ActivityPub `<a href>` が示すリンク情報を、Misskey API互換（`NoteResponse.text`はプレーンテキストのまま）を保ちつつ画面上でクリック可能にするため、Misskey本家のMFM同様「`text`フィールドの中に内部リンクマーカーを埋め込み、フロントがパースする」方式を採る。

### 内部リンクマーカー
`[表示テキスト](URL)`（Markdownリンク記法）をURLリンクのマーカーとして使う。`URL`が`/`始まり（`//`除く）ならフロント（`RichText`コンポーネント、`frontend/src/components/note/RichText.tsx`）は内部ルーティング、`https?://`ならタブ外部リンクとして描画する。

- **Bsky `#link` facet**: `crates/seiran-common/src/atp/facets.rs` の `apply_link_facets` が、facetの `byteStart`/`byteEnd` が指すテキスト範囲を `[元テキスト](facet.uri)` に書き換えてから `posts.body` へ保存する（受信時に確定。URLは不変なので都度解決不要）。Bskyの投稿を保存する全経路（Jetstream経由のリアルタイム受信 `seiran-atp-repo::firehose`、新規フォロー時の過去ログ同期・ピン留め投稿同期・検索結果保存 `seiran-common::atp::client`）がこの共通関数を通す。AppViewの `getAuthorFeed`/`getPosts` レスポンスにも `record.facets` が含まれるため、`BskyPost::facets` として保持し `apply_bsky_post_facets` 経由で同じ処理にかける。
- **AP `<a href>`**: `crates/seiran-common/src/jobs/inbound_activity_process` の `ap_content_to_markdown_body` が `content` のHTMLをタグ除去する際、`<a href="URL">text</a>` を `[text](URL)` に変換する（Mention以外のアンカー。ハッシュタグアンカーもここに含まれ、リモートインスタンスのタグページへの外部リンクになる）。`<br>`/`</p>`/`</div>` は改行として保持し（`\n`/`\n\n`）、Mastodon等がcontentを複数段落のHTMLで表現しても本文の改行が失われないようにする（`tag_break_text`/`normalize_whitespace_preserving_newlines`）。タグ除去後は名前付き・10進・16進のHTML文字参照をデコードする（例: `&apos;`、`&#039;`、`&#x27;` はすべて `'`）。文字参照表記を`posts.body`へ残さない。

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
  - **未知DIDは先行解決しない**: ローカル `actors` に無いDIDは能動的にupsertしない（`docs/database.md` の `bsky_actor_is_engaged` 参照、issue #216）。フォロー・DM等の他経路で既に保存済みのDIDのみハンドルへ解決され、未知のDIDは投稿時点の表示テキストのまま返る。

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
  - Note の `content` HTML は `crates/seiran-common/src/ap/deliver/text.rs` の `plain_to_html_with_mentions` が上記スパンを `<a href="...">` へ変換して組み立てる（`is_mention` なスパンのみ `class="mention u-url"` を付ける）。`is_mention` なスパンは `tag[]`（`{"type":"Mention","href":...,"name":...}`）にも追加する。この変換・`tag[]` 組み立ては push配送（`deliver_post_to_ap_followers`、`override_body` 未指定時のみ）と pull取得（AP直接フェッチ `get_note_ap`）の両方で共有し、両者が食い違わないようにしている。リポストのフォールバックテキスト（`override_body` 指定時）はメンション変換をせずプレーンにHTML化する。

### Bsky向け本文の文字数上限と受理タイミング
`app.bsky.feed.post` の本文上限は書記素クラスタ数300・バイト数3000。メンション変換（`@user` → `@user.example.com` 等）でテキストが伸びうるため、`crates/seiran-api/src/handlers/notes/creation.rs` の `create_regular_post` は投稿をDBへINSERTする前に `convert_mentions_for_bsky` を同期的に実行し、**変換後テキスト**に対してこの上限を検証する（`validate_text_length`）。超過時は投稿自体を作らず `TEXT_TOO_LONG` エラーを返す。Bsky非配信時は元の入力テキストに対する緩い上限（3000書記素・10000バイト、Fedi向け）のみ検証する。

### 既知の制約
- ローカル投稿者が生テキストに書いた `@mention` は、`posts.body` 自体は書き換わらない（AP/Bsky配送用のコピーにのみ `mention.rs` の変換がかかる）。フロントの `RichText` が本文中のプレーンな `@handle` パターンを直接検出してリンク化することでこれを補っている。
- 内部リンクマーカーとして `[text](url)` を採用しているため、投稿本文がたまたまこの形式の文字列を含む場合は意図せずリンク化されうる（許容している）。
- 送信時に生URLの自動リンク化（`app.bsky.richtext.facet#link` / AP `<a>`）は対応済みだが、ユーザーが手書きした `[text](url)`（Markdownリンク記法）のリンク化には未対応。

### ハッシュタグ
ハッシュタグはポストとm:nの関係を持つ永続化オブジェクト（`hashtags`/`post_hashtags`、`docs/database.md` 参照）として扱い、検索の即席表示ではなく専用のハッシュタイムライン（`GET /api/hashtags/:name/timeline`）の主軸にする。

- **送信側（ローカル投稿 → Bsky/AP）**: `convert_mentions_for_bsky`/`convert_mentions_for_ap`（`mention.rs`）は本文中の `@`（メンション）・`h`（URL）に加えて `#`（ハッシュタグ）もスキャンする（共通ヘルパー `scan_hashtag`。境界・除外ルールは `extract_hashtags` と同じだが、表示用テキストなので大文字小文字は保持する）。
  - Bsky: `app.bsky.richtext.facet#tag`（`tag` フィールドは `#` を除いた本体、大文字小文字保持）を本文中の出現位置ごとに付与する。
  - AP: `<a href="https://{local_domain}/tags/{正規化タグ}" class="mention hashtag" rel="tag">#タグ</a>` というアンカー（リンク先は自インスタンスのハッシュタイムライン）と、`tag[]` への `{"type":"Hashtag","href":...,"name":"#タグ"}` エントリを追加する。アンカー組み立て・`tag[]` 組み立ては push配送（`ap::deliver`）と pull取得（`get_note_ap`）の両方で `ap_inline_mentions_to_tag_json` を共有する。
- **受信側の分類**: Mastodon等はハッシュタグアンカーにも `class="mention hashtag"` を付与する（メンションと `mention` トークンを共有する）ため、`class` だけでメンション判定すると `#foo` が壊れたメンション文字列（`@#foo@sender_domain`）に誤変換される。`ap_content_to_markdown_body`（`inbound_activity_process`）は `rel="tag"` または `class` の `hashtag` トークンを検出したら、メンション解決ロジックより先に「ハッシュタグである」と判定して通常のURLリンク（`[#foo](url)`）として扱う。
- **抽出（DB永続化）**: プロトコル別の特別処理を持たず、ローカル投稿・AP受信・Bsky受信いずれも「最終的な `posts.body` テキストを1回スキャンする」共通経路（`seiran_common::hashtag::extract_hashtags`）でハッシュタグを抽出し `HashtagRepository::link_post` でDBへリンクする。AP由来のハッシュタグアンカーは `[#foo](リモートのタグページURL)` というMarkdownリンクに変換されるが、リンクテキスト部分に `#foo` が残るためこの共通スキャンだけで取りこぼしなく検出できる。Bsky受信の `app.bsky.richtext.facet#tag`（`JetstreamFacetFeature::Tag`）は本文中に既に `#foo` がプレーンで載っているため、facet自体の値は今も参照しない。
- **表示側**: フロントの `RichText` は `#foo` パターンとMarkdownリンクのリンクテキストが `#タグ` 形状の場合の両方を検出し、いずれも自インスタンスの `/tags/foo` へのリンクとして描画する（AP由来のハッシュタグアンカーもリモートのタグページへは飛ばさず、同列に扱う）。
- **ホーム画面への追加**: `pinned_hashtags` にユーザーごとのピン留めを保存し、ホーム画面のフィードタブとして表示する（`pinned_posts`/`lists` と同じ「ユーザーごとの永続ショートカット」の設計思想）。

### seiran Web UIでのリッチ表示（`content_html`）

`body`（上記の内部リンクマーカー方式のプレーンテキストもどき）とは別に、リモートFedi投稿は
`sanitize_ap_content_html`（`crates/seiran-common/src/jobs/inbound_activity_process`）が
AP `Note.content`（HTML）から意味的構造を保持したままクレンジングした `posts.content_html` を
持つ（`docs/database.md` の `posts` セクション参照）。`body`はMisskey互換API・Bsky配送・検索・
ハッシュタグ抽出が前提とする唯一のフォーマットなので一切変更しない。`content_html`はseiran Web
UIの表示専用の追加チャンネルで、フロントは値があれば`RichHtml`コンポーネントで描画し、無ければ
`body`の`RichText`描画にフォールバックする。

**許可タグ**: `br p div a b i s code pre blockquote ruby rt rp h1 h2 figure img ul ol li small center`。
**許可属性**: `a`→`href`のみ、`img`→`src alt width height`のみ、全タグ共通で`style`（`text-align:
left|right|center|justify` の1プロパティのみ許可、それ以外のCSSプロパティは属性ごと除去）。
`class`はどのタグからも除去する。`href`/`src`は`http`/`https`スキームのみ許可（`ammonia`クレート）。
`rel`/`target`は一切保持しない（信用できるのはこちらが強制する値だけであるべきなので、フロントの
`RichHtml`が`<a>`に`target="_blank" rel="nofollow noopener noreferrer"`を固定で付与する）。

**メンション/ハッシュタグの`<a>`**: `body`生成時と同じ解決ロジック（`resolve_ap_mention_text`等、
上記「メンションは内部リンクマーカーで包まない」節参照）を`rewrite_mention_hashtag_hrefs`が再利用し、
`<a>`の`href`だけをseiran内部パス（メンション→`/@user@host`、ハッシュタグ→
`/tags/{urlencode(小文字化タグ)}`）へ書き換える。タグ構造・内側HTML・他の属性は一切変更しない。
`RichHtml`はこの内部パス形状（`/@`/`/tags/`始まり）を検出したら`<Link>`によるアプリ内遷移、
それ以外は通常の外部`<a>`として描画する。

**MFM装飾関数（`blur`/`spin`/`jelly`/`shake`/`twitch`/`flip`/`x2`/`position`等）**: Misskey側の
HTML変換時点でこれらは全て`<i>`（イタリック）に縮退しており、`content_html`経由では相互に区別
できない（実機確認: `$[blur ...]`も`$[spin ...]`も同じ`<i>...</i>`になる）。`ruby`（`$[ruby 本文
ルビ]` → `<ruby><rt>...</rt></ruby>`）だけは意味のある変換が得られるため許可タグに含めている。

**引用フォールバック行の除去**: Misskey/Fedibird/kmyblueが引用時に自動付加するフォールバック
表現は、`body`と同様`content_html`側でも除去する。除去は3段階のフォールバックで試みる
（`note_save::save_ap_note_core`）:
1. `strip_quote_inline_paragraph_html`（サニタイズ**前**の生HTML限定。`<p class="quote-inline">`
   を位置問わず検出・除去する、kmyblue標準の先頭パターン向け）
2. `strip_quote_fallback_line_html_leading`（本文**先頭**のブロックをテキストベースで判定。
   `class`が無いkmyblue系にも対応するclass非依存のフォールバック）
3. `strip_quote_fallback_line_html`（本文**末尾**のブロックをテキストベースで判定。
   Fedibird/Misskey、およびkmyblueの末尾パターン向け）

行/段落区切りの近似には、直近の`<br>`と`<p>`開始タグ（先頭版は`</p>`）のうち近い方を使う
（`<br>`のみを見ると、本文が段落単位で区切られ`<br>`を一度も含まない投稿で本文全体を
「最後（または最初）の行」と誤認し丸ごと消してしまうため）。マーカー行頭は`RE:`/`QT:`
（コロン付き）と`RE `/`QT `（コロン無し、kmyblueの一部）の両方を見る
（`starts_with_quote_marker`）。一致判定は「6 プロトコル仕様」節「引用受信」の
`quote_uri_matches`と共通。リンクカード化対象URLの抽出（`extract_link_card_urls`）はこの除去後の
`body`を対象にするため、除去に失敗するとフォールバック行のURLがそのままリンクカード化され、
引用ボックスとURLカードが二重表示される。

**既知の制約**: 元の生HTMLはDBに永続化しないため、既存投稿（この機能実装前に受信した行）の
`content_html`は`NULL`のまま（バックフィル不可）。ローカル投稿・Bsky投稿も常に`NULL`
（コンポーザーがMarkdown風記法を持たないため構造保持の余地がない）。

## 6.1 投稿検索とBluesky AppView

`GET /api/notes/search`は、初回検索でローカルDBと`api.bsky.app`の`app.bsky.feed.searchPosts`の双方から`limit`件ずつ取得する。AppView結果はURI照合だけで捨てず、authorを`actors`、post viewを`posts`へupsertしたうえで、ローカル結果とsnowflake ID降順にマージ・重複排除して`limit`件を返す。AppView障害時はHTTPステータスをログへ記録し、ローカルDB結果だけへ縮退する。検索結果には保存したactorアバターも含める。

Misskeyクライアント向けの`POST /api/notes/search`も同じDB・AppView検索を行い、Misskey形式のノート配列を返す。JSONの`query`、`limit`、`untilId`を受け付ける。Aria等のハッシュタグ画面向けには`POST /api/notes/search-by-tag`を提供し、JSONの`tag`、`limit`、`sinceId`、`untilId`を受け付け、専用ハッシュタイムラインをMisskey形式で返す。

`until_id`指定時は対象postの`created_at`をRFC 3339の`until`としてAppViewへ渡し、DBにも`p.id < until_id`を適用して同様にブレンドする。`since_id`指定はMisskey互換の逆方向ページングであり、AppViewへ問い合わせずDBの`p.id > since_id`だけを返す。既存frontendの過去掘りは互換維持のため`session_id`バッファも引き続き利用できる（#146）。

## 7. Misskey API 互換レイヤー

`middleware::misskey_auth_bridge` が `Authorization` ヘッダー未指定時にJSONボディ/クエリの `i` を検出し `Authorization: Bearer` を合成する。`handlers::misskey`（`endpoints.rs`/`convert.rs`/`types.rs`）が Misskey ワイヤー形式のエンドポイントを提供する。`POST /api/drive/files/create`（`handlers::drive::create_drive_file`）は multipart/form-data のためこのブリッジの対象外であり、ハンドラ内で multipart フィールドの `i` を個別に読み取り `Authorization` ヘッダーが無い場合のフォールバックとして使う（misskey_dart の `postWithBinary` はアクセストークンを multipart フィールドとして送るため）。

対応済み: `POST /api/meta`（サーバー検出）、MiAuthフロー、`POST /api/i`、`/api/users/show`、`/api/users/notes`（プロフィール画面のノートタブ、`timeline_by_actor`を使用）、`POST /api/users/following`・`followers`（フォロイー/フォロワー一覧、カスタムAPIの同パス`GET`とルート共存。`follower`/`followee`のどちらのキーで包むかは`MisskeyFollowRelation`の`#[serde(skip_serializing_if)]`で片方だけ出す）、`POST /api/notes/reactions`（リアクションしたユーザー一覧、`type`省略時はseiran側の集計実装が単一絵文字指定前提のため空配列を返す）、`/api/notes/show`、`/api/notes/local-timeline`・`timeline`、`/api/notes/hybrid-timeline`（ソーシャルタイムライン＝自分+フォロー中+ローカル全体、`PostRepository::social_timeline`）・`/api/notes/global-timeline`（`posts`全件、カスタムAPIの同パス`GET`と共存、#78）、`/api/notes/reactions/create`・`delete`、`/api/notes/unrenote`、`/api/following/create`・`delete`、`/api/i/notifications`（DB永続化、`untilId`/`sinceId`カーソル）、`GET /api/emojis`（未認証公開）、`POST /api/drive/files/create`（`handlers::drive::DriveFileResponse` はseiran独自形式のフィールドに加え、misskey_dart `DriveFile.fromJson` が必須とする `createdAt`/`name`/`type`/`md5`/`isSensitive`/`properties{width,height}` も同居させて返す。専用のMisskey形式レスポンス型を別途持たず1つの構造体で両対応している。画像は別途縮小サムネイルを持たないため `thumbnailUrl` は本体 `url` と同値を返す。ファイル名は `name`という独立したmultipartテキストフィールドで受け取る＝misskey_dartの`postWithBinary`はfile添付にContent-Dispositionのfilenameを付与しない仕様のため、`file`側のfilename属性より優先する）。Note本文のカスタム絵文字は、保存済み`posts.emoji_map`と`actors.emoji_map`を統合してMisskey形式の`emojis`（コロンなしshortcode→画像URL）へ変換して返す。ActivityPub投稿では本文中の絵文字がactor側mapに保持される場合があるため、投稿側だけを参照するとAriaでショートコード表示へ退行する（#88, #156）。

`POST /api/notes/create`（`handlers::notes::create_note`）はMisskey専用の別ラッパーを持たず、seiranネイティブのエンドポイントをMisskeyクライアントとも共用する。そのため`handlers::notes::dto::CreateNoteRequest`は本家の`fileIds`/`replyId`/`renoteId`/`visibleUserIds`をそれぞれ`attachment_ids`/`reply_to_id`/`renote_id`/`recipient_actor_ids`の`#[serde(alias)]`として受け付ける。`visibility`は本家語彙（`"public"/"home"/"followers"/"specified"`）とseiran語彙（`"public"/"unlisted"/"followers_only"/"direct"`）の両方を受け付け、`ReplyContext::resolve_visibility`（`handlers::notes::delivery`）の入口で`normalize_misskey_visibility`が本家語彙をseiran語彙へ正規化してから可視性継承ロジックに渡す（`home`→`unlisted`、`followers`→`followers_only`、`specified`→`direct`。レスポンス側の`to_misskey_visibility`と対称）。

書き込み系は既存の `handlers::notes`/`handlers::follows` をそのまま呼び出し、レスポンスだけMisskey形状に整形する。

**既知の非互換点**: 書き込み系のエラー形状はMisskey本家のエラーID体系を再現していない。（ストリーミングのチャンネル購読方式は8節参照、対応済み）

**`MisskeyNote.uri`/`url` の算出**（`handlers::misskey::convert::to_misskey_note`）: `uri` はActivityPub Object IDで、Misskey本家準拠のためローカルノートでは常に `null`、Fedi受信ノートのみ非null。seiranはローカル投稿にもFederation配送用の自己参照的な `posts.ap_object_id`（`https://{local_domain}/notes/{id}`）を常に持たせているため、`ap_object_id` の有無だけでは出自を判定できず、`domain == local_domain` で判定する（実機確認: Ariaがこれを見てローカルノートをリモート扱いする不具合の原因だった）。`url`は人間向けURLで、Fedi（`ap_object_id`）優先、無ければBsky（`at_uri`→bsky.app URL）にフォールバックする。ローカルノートは両方 `null`。

**`misskey_dart`（Aria等）の non-nullable 直接キャスト対策**: `misskey_dart` の生成コード（`*.g.dart`）は本家スキーマの必須フィールドを `as String`/`as num` 等で直接キャストするため、JSONでキーが欠けたり `null` だと Dart 側で未処理の `TypeError` となりクライアントが落ちる（サーバー側のバリデーションエラーとは別の失敗モード）。`MisskeyMeDetailed`（`notesCount` 等）に続き `MisskeyDriveFile`（`createdAt`/`md5`/`size`/`isSensitive`/`properties`）、`MisskeyUserDetailed`（`/api/users/show`・`/api/i` 共通、`notesCount`/`followersCount`/`followingCount`）でも踏んだため、Misskey互換型を追加・変更する際は本家スキーマの必須/任意を都度 `misskey_dart` のソースで確認すること。`md5` は seiran 内部で持つ `sha256` を代用し、リモート添付など元データが無い場合は空文字列/0を返す（クライアントは値を検証せず保持するだけのため実害はない）。

**`MisskeyUserDetailed`の関係フィールド（`isFollowing`等）**: `misskey_dart`の`UserDetailed.fromJson`はレスポンスJSONに`isFollowing`キーが存在するかどうかで`UserDetailedNotMe`（関係情報なし）/`UserDetailedNotMeWithRelations`（関係情報あり）のどちらにパースするかを判定する（キー自体の有無で分岐、値のnull/非nullではない）。seiranは閲覧者の`viewer_actor_id`が解決できる場合（ログイン済み、`/api/users/show`・`/api/users/following`・`/api/users/followers`）のみ`MisskeyUserRelations`（`types.rs`）を`Some`にし`#[serde(flatten)]`でJSON上にフラット展開する。`isFollowing`等8フィールドは`UserDetailedNotMeWithRelations`側で`required bool`のため、`Some`の場合は値がnullであってはならない（他のnon-nullable直接キャスト問題と同種）。`hasPendingFollowRequestToYou`は常に`false`（seiranはローカルアカウントの鍵アカウント機能自体を持たず、ローカルviewerへの受信フォローは常に即accepted）。`/api/i`用の`build_me_detailed`は本家`MeDetailed`に関係フィールドが存在しないため常に`viewer_actor_id: None`固定で呼ぶ。

**`POST /api/following/create`・`delete`のレスポンス形状**: 本家Misskeyはこれらを`204 No Content`ではなく対象ユーザーの`UserLite`で応答する仕様（`misskey_dart`の`MisskeyFollowing.create`/`delete`は`post<Map<String, dynamic>>`で戻り値を直接キャストする）。`204`のまま返すと空ボディがJSONデコードで文字列扱いになり、クライアント側で`type 'String' is not a subtype of type 'FutureOr<Map<String, dynamic>>'`という未処理例外になる（実機確認済み、Aria）。`following_create`/`following_delete`（`handlers::misskey::endpoints`）は成功時、共通ヘルパー`misskey_user_lite_response`で`build_user_detailed(state, actor, None).lite`を`Json`で返す（`viewer_actor_id: None`固定＝`isFollowing`等の関係フィールドは含めない、本家`UserLite`にも存在しないため）。`following/invalidate`・`update`は未実装。

**`MisskeyNote.renote`/`MisskeyNote.reply`（リノート元/引用元/返信先ノート本体の埋め込み）**: `renoteId`/`replyId` だけでは `misskey_dart` 等のクライアントが参照先ノートを解決できず「削除されたノート」のプレースホルダー表示になる（実機確認、Aria）。`embed_referenced_notes`（`handlers::misskey::convert`、カスタムAPI側の `handlers::notes::queries::embed_renotes` と同じ可視性フィルタ・一括フェッチ方針）が `build_notes` の最後で `renoteId`/`replyId` 両方から対象ID集合を作り、1回のクエリで一括取得して `renote`/`reply` へ埋め込む（同じノートが両方の対象になっても二重フェッチしない）。孫リノート・孫リプライは埋め込まない（埋め込む側の `renote`/`reply` は常に `None`）。`cw`（Content Warning）は `posts.content_warning` をそのまま返し、`MisskeyDriveFile.isSensitive` も `post_attachments.is_sensitive` をそのまま返す（いずれもAriaで非表示/非機能の不具合として発覚し対応、埋め込みノート・本体ノート双方に反映される）。

**通知の `user.avatarUrl` 解決**: `build_notifications` の通知起点ユーザー取得クエリは、ローカルユーザーのアバターを `actors.avatar_media_id → media_files → storage_providers` 経由で解決する（`build_user_detailed` 等、他の全ユーザー取得クエリと同じ `COALESCE(rtrim(sp.public_url,'/')||'/'||mf.storage_key, a.avatar_url)` パターン）。以前は `actors.avatar_url` を直接参照していたためローカルユーザーのアバターが常に欠落していた（同カラムはリモートアクター用の生URL格納にのみ使われるため）。

**ローカル actor の代替アバター（#211）**: Misskey互換 API のユーザー変換では、ローカル actor の解決済みアバター URL が空なら `https://{LOCAL_DOMAIN}/api/avatars/{actor_id}` を `avatarUrl` に設定する。ActivityPub の Actor ドキュメントおよびプロフィール Update(Person) も同じ URLを `icon`（`mediaType: image/svg+xml`）として配送し、API の `GET /api/avatars/:actor_id` が SVG を返す。リモート actor の未設定アバターは送信元の状態を尊重し、代替しない。

**`MisskeyUserDetailed.followersVisibility`/`followingVisibility`**: 本家Misskeyのフォロー/フォロワー一覧・数の公開範囲設定に相当するフィールド。seiranはこの設定自体に未対応のため常に `"public"` を返す。値が欠落しているとクライアントは非公開とみなし、`followersCount`/`followingCount`（値自体は正しく集計されている）の数値表示を鍵アイコンに置き換える。

`POST /api/endpoints`は実装済みのMisskey互換API名を配列で返す。Ariaはこの一覧に`emojis`がある場合だけ`POST /api/emojis`を呼ぶため、絵文字一覧は既存の`GET /api/emojis`と同じ`fetch_public_emojis`からGET/POST両対応で返す（#145）。
`POST /api/notes/reactions/create`が受け取る`reaction`は、そのまま`ReactRequest.content`として`validate_reaction_content`（`handlers/notes/validation.rs`）に渡す。seiranの内部表現自体が本家Misskey準拠の`:shortcode@.:`（ローカル）になったため、Misskeyクライアントが送る`:shortcode@.:`もbackend API向けの生`:shortcode:`もAPI境界での変換なしにそのまま受理できる（`normalize_local_reaction`は廃止済み、#145）。リモートホスト付き（`:shortcode@remote.example:`）はローカル絵文字ピッカーから選べないため`INVALID_REACTION_CONTENT`で拒否する。

`POST /api/meta`の`mediaProxyUrl`は、`site_settings.media_proxy_url`（管理画面の外部プロキシ設定）が未設定なら`https://{local_domain}/proxy`（自インスタンスの`GET /proxy?url=...`、SSRF対策済み）へフォールバックする。空文字列のまま返すと、Ariaの画像URL組み立て（`{mediaProxyUrl}/image.webp?url=...`という形式でリクエストを構築する）が不正なURLになり、リモートインスタンスのファビコン等の画像取得が軒並み失敗する（実機確認）。

代替アバターの実体とActivityPub `icon.mediaType` は `image/png` とする。Misskey互換APIの `avatarUrl` はPNG URL（`?v=5`）を返す。AT ProtocolのプロフィールはURL型アバターを格納できないためavatar blob未設定のままだが、`ATP_BACKFILL_UNSET_AVATAR_PROFILES_ONCE=1` の一回限りの起動処理で画像未設定ユーザーの `app.bsky.actor.profile/self` を再コミットし、各コミット後の `requestCrawl` によりRelayへ新しい #commit の取得を促す。

## 8. 通知・リアルタイム配信

`seiran-common::streaming::StreamHub`（プロセス内 `tokio::broadcast`、容量512）が `{"type":kind,"body":body}` を配信する。`GET /api/streaming?token=<JWT>` でWebSocket接続する。配信方式は2系統ある。

- **`recipients`方式**（通知・DM・`noteUpdated`・`pollUpdated`）: `StreamEvent.recipients`（`HashSet<i64>`）に自分の actor_id が含まれるイベントのみ、各コネクションが自前フィルタして転送される。従来からの方式で購読操作は不要（認証済み接続には自動的に届く）。`pollUpdated`（`streaming::broadcast_poll_update`、ローカル投票`handlers::notes::vote_poll`とAP受信`jobs::inbound_activity_process::handle_poll_vote`の両方から送出）は`{"postId","poll":<posts.pollそのもの>}`を投稿の著者+著者をフォロー中のローカルアクターへ配信し、アンケート結果（票数）のリアルタイム反映に使う（`broadcast_reaction_update`と同じ配信先ロジック）。`poll`に`votedByMe`は含めない（閲覧者ごとに異なるため）。フロントは`noteUpdated`のようなNoteCardごとの個別リスナー登録を経由せず、共有ストア`frontend/src/stores/pollVoteStore.ts`（`usePollState`で`useSyncExternalStore`購読）を`StreamingContext`が直接更新するだけで、表示中の全NoteCardへ伝播する（自分の投票済み選択肢はローカル状態を保ったまま票数のみ差し替える）。
- **チャンネル方式**（タイムライン新着ノート、Misskey互換）: クライアントが`{"type":"connect","body":{"channel":"localTimeline","id":"<uuid>","params":{}}}`を送ると、以後そのチャンネルに該当する新着ノートが`{"type":"channel","body":{"id":"<uuid>","type":"note","body":{...}}}`で届く。`{"type":"disconnect","body":{"id":"<uuid>"}}`で購読解除する。対応チャンネル: `homeTimeline`/`localTimeline`/`hybridTimeline`(social)/`globalTimeline`/`userList`(`params.listId`必須)/`hashtag`(`params.tag`必須)。publish側（`seiran-api::handlers::notes::delivery::broadcast_new_note`、`seiran-common::jobs::inbound_activity_process::handle_create_note`、`seiran-atp-repo::firehose::save_bsky_post`）は投稿ごとに1回`ChannelScope`（`is_local`・`visibility`・`home_recipients`（著者+承認済みローカルフォロワーの集合）・所属リストID集合・本文由来ハッシュタグ集合）を組み立てて`publish_channel_note`で送出し、各WSコネクションが自分の購読チャンネル一覧に対し`ChannelScope::matches`でO(1)照合する（コネクションごとのDB再問い合わせは発生しない）。各チャンネルの配信条件は対応するRESTタイムラインクエリ（`repository::post`の`home_timeline`/`local_timeline`/`social_timeline`/`global_timeline`、`repository::list::timeline`、`repository::hashtag::timeline`）のスコープに合わせている。`userList`チャンネルへの`connect`は所有者本人または公開リストのみ許可する（`GET /api/lists/:id`と同じ判定）。`home_recipients`はリプライ投稿の場合、著者のフォロワーのうちリプライ先投稿者もフォロー中（または本人）のものだけに絞り込む（`FollowRepository::find_home_recipient_ids`がDB関数`post_reply_target_followed`を使ってREST側の`home_timeline`/`social_timeline`と判定基準を共有する、`docs/database.md`参照）。この絞り込みは`home_recipients`を通じて`homeTimeline`/`hybridTimeline`両チャンネルに反映される。
  - **既知の制限**: ブロック/ミュート（`actor_is_hidden_for_viewer`）はチャンネル配信では考慮しない（コネクション数に比例するDBコストになるため。通知系は`notifications`テーブルINSERT時にのみチェックされる）。リストタイムラインは10節の通り「viewer概念が無い」設計のため、`userList`チャンネルもメンバーシップのみで判定し（`visibility != 'direct'`のみ除外）、フォロー関係やブロックは考慮しない。
  - DM（`visibility="direct"`）は引き続き`recipients`方式の`publish_note`のまま配信される（チャンネル購読は不要）。

`notifications` テーブルへの書き込みは、ローカルユーザー間のフォロー成立・ローカルリアクション作成・AP/ATP inbound（Follow/Accept/Reaction）の各経路から行われる。ローカルフォローは `follows` への新規挿入時だけ `Follow` 通知を生成し、既存関係への再リクエストでは重複させない。種別は `Follow`/`Reaction`/`FollowRequestAccepted`/`Mention`/`Reply`/`Repost`/`Quote`/`MoveRefollowed`/`MoveAlreadyFollowing` の9種（最後の2つはMisskey APIに無いMove受信専用のseiran独自拡張、2節「アカウント引っ越し（Move）の受信」参照）。WebSocketは基本的に「新着があった」というシグナル配信のみに用い、実データは常に `POST /api/i/notifications`（REST、`sinceId`付き）から再取得する（一覧表示とスキーマを統一するため）。

### リアクション通知の重複排除（`reaction_id`）
ローカルユーザーが ATP 実体（`at_uri`/`at_cid`）を持つ投稿へリアクションすると、(1) `notes::create_reaction` がその場でローカル通知を即時INSERTし、(2) 同じリアクションを非同期で `AtpCommitService::commit_like` が `app.bsky.feed.like` としてコミットし、それが自分自身の firehose 受信（`seiran-atp-repo::firehose::handle_inbound_like_create`）で戻ってきて再度通知INSERTを試みる、という2経路が走る。この2つは「経路が違うだけの同一操作」であり、素朴に両方INSERTすると通知が重複表示される。

これを防ぐため、`reactions.id`（`posts`/`notifications`と同じsnowflake ID名前空間、`docs/database.md`参照）を「リアクション実体の識別子」として2経路で共有する。ローカルINSERT時に採番された `reactions.id` を (a) `notifications.reaction_id` に保存し、(b) `commit_like` が `app.bsky.feed.like` レコードの非標準拡張フィールド `seiranReactionId` として埋め込む（`emoji` 拡張フィールドと同じ流儀）。自分自身の firehose 受信時、このLikeが `seiranReactionId` を持っていればそれをそのまま `notifications.reaction_id` として渡し、`idx_notifications_reaction_id`（`reaction_id IS NOT NULL` の部分UNIQUEインデックス、`ON CONFLICT DO NOTHING`）で2つ目のINSERTが弾かれる。

`source_uri` によるUNIQUE制約（既存）とは目的が異なる: `source_uri` は「他人発のイベントの複線受信対策」（Doc6既知の課題）だが、`reaction_id` は「自分が起点の同一操作が別経路で戻ってくることの対策」。他人（他インスタンスのMisskey/Mastodonユーザーや他のBskyユーザー）からのリアクションには `seiranReactionId` が付かないため `reaction_id` は常に `NULL` で、同じ投稿に複数の絵文字で連投する（通知欄に文章のようなものを書く遊び）動作は妨げない。

例外として `followAccepted`（`jobs::inbound_activity_process::handle_accept`、Fediフォローリクエストが相手から承諾された）はペイロード（`actor.username`/`actor.domain`）自体をフロントエンドが利用する。Fediフォロー（`handlers::follows::follow_fedi`）は、相手のActorが鍵アカウント（AS2 `manuallyApprovesFollowers: true`）の場合のみ `pending` で開始し、相手の `Accept` が非同期で届くまで承認待ち状態が続く。非鍵アカウント（フィールド省略時も含む）が相手の場合は、本家Misskey準拠でFollow送信と同時にDB上を即座に `accepted` として確定する（相手サーバーのAccept返信を待たない楽観的確定。Ariaはフォロー操作後に一度だけ・1秒後に再取得してボタン状態を更新する設計のため、`pending`のまま留まると実際のAccept受信までボタンが「処理中」表示に固まって見える不具合があった、実機確認済み）。鍵アカウント宛の`pending`→`accepted`遷移では、`StreamingContext` が`followAccepted`受信時に `stores/followStatusStore`（`username`+`domain` を正規化したキー、`lib/format.ts` の `profileQuery` と同じロジック）を直接更新し、その場で切り替える。手動リロードや通知一覧の再取得を待たずに反映するための例外であり、通知の永続化・一覧表示自体は他の種別と同じ経路を通る。フォロー状態の表示側（`frontend/src/pages/ProfilePage.tsx` のフォローボタン、`frontend/src/components/note/NoteCard.tsx` のタイムライン上のフォロースイッチ）はいずれもこの共有ストアを `useSyncExternalStore` で参照する設計のため、自分の操作・WebSocket経由の承認のいずれでも、同一アクターを表示中の全コンポーネントが同時に反映される（詳細は `docs/architecture.md` のフロントエンド構成節）。

### メンション通知
本文中で `@username` 形式によりローカルユーザーが言及された場合、`notifications`（`type="mention"`, `note_id`=言及元投稿）を作る。配信設定（Bsky/AP接続の有無）とは無関係に、投稿の出自（ローカル/Fedi受信/Bsky受信）ごとに以下で解決する。自己メンションは通知しない。

- **ローカル投稿**（`handlers::notes::create_regular_post`）: `mention::extract_local_mention_actor_ids` が本文を走査し、`@username`（ドメイン省略）・`@username.{local_domain}`（AT Protocol ハンドル表記）・`@username@{local_domain}`（Fediverse表記）のいずれかで書かれたローカルアクターの `actor_id` を重複除去して返す。6節の配信用メンション変換（`convert_mentions_for_bsky`/`convert_mentions_for_ap`）は配信対象プロトコルが有効な場合のみ呼ばれるため、これとは独立した専用スキャンとして常に実行する。
- **Fedi受信**（`jobs::inbound_activity_process::handle_create_note`）: `tag[]` の `Mention` エントリのうち、`href` が `https://{local_domain}/users/{username}` を指すものを、DM宛先解決と同じ `seiran_common::ap::extract_local_username`（ホスト名まで含めて自ドメインのURIかを検証してからusernameを取り出す）で判定する。URI末尾のセグメントだけを見て判定すると、リモートの同名ユーザー（例: `https://fedibird.com/users/momozou`）宛のメンションをローカルの同名ユーザー宛と取り違えるため、必ずホスト名の一致確認を経由する。
- **Bsky受信**（`seiran-atp-repo::firehose::save_bsky_post`）: 保存済みの `mention_facets`（6節）の各 `did` を `actors.at_did` で引き、`actor_type = 'local'` なら通知する。

いずれの経路も `source_uri` は渡さない（1投稿に複数の宛先がありうるため、投稿の一意識別子を共有すると2人目以降が `notifications.source_uri` の部分UNIQUEインデックスで弾かれてしまう。posts 自体の重複排除は各経路で別途完結しているため、このブロックへの到達自体が新規保存時のみに限られ、重複INSERT対策は不要）。

### リプライ通知
自分の投稿に返信が付いた場合、`notifications`（`type="reply"`, `note_id`=返信投稿自体）を作る。可視性・配信設定とは無関係に常に処理し、リプライ先投稿者がローカルユーザーの場合のみ通知する。自己リプライは通知しない。本文中に相手への `@username` を書いた場合はメンション通知とは別に両方生成されうる（Misskey/Mastodon等と同様の挙動）。

- **ローカル投稿**（`handlers::notes::create_regular_post`）: リプライ先解決（`resolve_reply_context`）が返す `ReplyContext::parent_local_actor_id`（リプライ先投稿の `PostDeliveryMeta::domain` が自ドメインの場合のみ `Some`）を宛先に使う。
- **Fedi受信**（`jobs::inbound_activity_process::handle_create_note`）: `note["inReplyTo"]` から解決した `reply_to_post_id` の投稿者を `PostRepository::find_delivery_meta` で引き、`domain` が自ドメインなら通知する。
- **Bsky受信**（`seiran-atp-repo::firehose::save_bsky_post`）: `record.reply.parent.uri` から解決した `reply_to_post_id` の投稿者が `actor_type = 'local'` なら通知する。

いずれの経路も `source_uri` は渡さない（1リプライにつき宛先は常に1人だが、メンション通知と実装を揃えるため統一している）。

### リポスト・引用通知

ローカル投稿が他ユーザーにリポストまたは引用された場合、`notifications` にそれぞれ `type="repost"` / `type="quote"` を作る。`note_id` は新しく作られたリポスト／引用投稿を指す。自己リポスト・自己引用は通知せず、リモート投稿者宛のローカル通知も作らない。ローカル作成経路（`handlers::notes::create_repost` / `create_regular_post`）に加え、ActivityPub 受信経路（`inbound_activity_process::handle_announce` / `handle_create_note`）でも、対象投稿の `PostDeliveryMeta` がローカルアクターを指す場合に通知を作る。同一 `Announce` の再配送はリポストの重複チェック、同一 `Create/Note` の再配送は投稿の重複排除によって通知生成前に終了する。

Bsky受信ではJetstreamの `app.bsky.feed.repost` を購読し、`subject.uri` がローカルユーザーの投稿を指す場合にリポスト通知を生成する。取り込み対象のBsky投稿に `embed.record.uri` がある場合は引用先を解決して引用通知を生成する。いずれも通知者をDIDから解決し、自己通知を除外する。リポストレコード／引用投稿の `at://` URIを `notifications.source_uri` に保存して多重受信を重複排除する。

`type="repost"` の `notifications.note_id` が指すのは本文を持たないリポストラッパー投稿自体で、その可視性はリポスト元とは独立（Fedi受信時はFollowers限定になりうる）である。`build_notifications`（`handlers::misskey::convert.rs`）は Misskey本家（`NotificationEntityService#packInternal`）と同じ方針で、ラッパー投稿自体には独自の可視性チェックをかけずそのまま `note` として pack する（通知は既に受信者向けに絞られたエントリのため）。リポスト元投稿の埋め込み（`note.renote`）は通常のノートpack処理と共有する `embed_renotes` に任せ、そちらのSQLで可視性チェック（投稿者本人 or フォロワー限定でなければ許可 or 閲覧者がフォロワー）を行う。`note`（ラッパー、`text: null` で `renoteId` を持つ）と `note.renote`（リポスト元の実体投稿）という入れ子構造は Misskey 本家のレスポンスと一致させており、崩すとRenoteとして描画できず「不明」表示になる（Aria等で実機確認済みの回帰）。`type="quote"` の `note_id` は引用コメント本文を持つ実体の投稿なのでこの解決は不要。フロントエンド（`NotificationsPanel.tsx` の `resolveTargetNoteId`）は `repost`（Misskey API上は `renote`）通知について `note.renote` があれば優先し、なければ `note` 自身を使う。

リポストが取り消し（アンリポスト、`DELETE /api/notes/:id/repost`）されるとラッパー投稿は論理削除されるが、`notifications` 行自体は削除しない（過去の出来事の記録として残す設計）。`build_notifications` はラッパー投稿の取得に `PostRepository::find_by_id_including_deleted`（`deleted_at` を問わない）を使う。通常の `find_by_id`（`deleted_at IS NULL` 必須）を使うと取り消し済みラッパーが取得できず `note` 全体が `null` になり、生きているはずの `note.renote`（リポスト元の実体投稿）まで失われてポップアップ・遷移が機能しなくなる（過去に実際発生した回帰）。

`notifications.type` の値自体は seiran 内部の語彙（`repost`/`quote`/`reaction`/`follow`/`followRequestAccepted`/`mention`/`reply`）で、DB・Rustコード全体でこの表記に統一している。Misskey本家の `notificationTypes`（`packages/backend/src/types.ts`）には `repost` という値は存在せず `renote` が正式名称のため、`build_notifications` は Misskey API（`POST /api/i/notifications`）のレスポンス直前で `repost` → `renote` にのみ変換する（`to_misskey_notification_type`、他の種別は綴りが一致）。ここがズレるとMisskey互換クライアント（Aria等）が種別を判別できず通知が「不明」表示になる。フロントエンド（seiranクライアント自身もこのAPIの一利用者）はAPIレスポンスの値に合わせ `"renote"` で判定する。

## 9. ダイレクトメッセージ

`visibility='direct'`の投稿をそのまま`posts`に格納する方式でDMを実現する（`docs/database.md`の「ダイレクトメッセージ関連」節も参照）。Misskey APIクライアントも同じ投稿テーブルを読み書きするため、Bsky DMも含めてMisskey互換の投稿・タイムライン取得APIでそのまま扱える。

### 宛先・スレッド・タイムライン除外
- 宛先は`post_recipients`（post_id/actor_id）に持つ。投稿作成API（`POST /api/notes/create`、Misskey互換では`visibleUserIds`も同じ意味で受け付ける）が`visibility=direct`のとき`recipient_actor_ids`必須。
- スレッド起点（`posts.thread_root_post_id`）は再帰クエリではなく伝播コピー方式。新規direct投稿作成時、親（`reply_to_post_id`）が`direct`ならその`thread_root_post_id`をそのままコピーし、親が`direct`でなければ自分自身のIDを設定する。
- 各タイムライン系クエリ（`home_timeline`/`local_timeline`/`timeline_by_actor`等）の`direct`閲覧制御は「投稿者本人 or `post_recipients`の宛先」のみ（`followers_only`とは異なりフォロワーには見せない）。`exclude_direct`クエリパラメータ（Misskey互換のためデフォルト`false`）を付けると宛先者でも一切表示しない。seiranフロントエンドは常にこれを付与する。`followers_only`/`direct`両方の判定はSQL関数`post_is_visible_to`に集約されている（`docs/database.md`参照）。
- リスト・ピン留めタイムラインは閲覧者情報を持たない/宛先チェックの構造上の理由から、`direct`を無条件で除外する（`repository::list::timeline`、`repository::pinned_post::list_timeline_by_actor`）。

### Fedi受信（`jobs::inbound_activity_process::note_save::save_ap_note_core`、Create直接受信時のみ）
`to`/`cc`から`classify_ap_visibility`が`direct`と判定した場合、通常投稿受信経路とは別に以下を行う。
参照解決経由（リプライ/引用/リポスト対象の1段階フェッチ）で保存された投稿は、実際にはinboxへ
配送されていないためDM宛先情報を信頼できず、以下は常にスキップされる。
- `note["inReplyTo"]`から`reply_to_post_id`を解決する（`find_id_by_ap_or_at_uri`。DM以外の通常投稿にも設定するようになった。以前はFedi受信投稿は`reply_to_post_id`を一切保存しない実装だった）。
- `to`に含まれるローカルアクターURIから宛先を解決し`post_recipients`へ保存する。ローカルユーザーの`actors.ap_uri`は登録時に設定されない（都度`https://{local_domain}/users/{username}`として動的組み立てされる）ため`find_by_ap_uri`では引っかからない。`seiran_common::ap::extract_local_username`でホスト名まで含めて自ドメインのURIかを検証してからusernameを取り出し`find_by_username_domain`で解決する（末尾セグメントだけでは同名リモートユーザーと取り違える）。
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

### ミュート・ブロック相手のDMの扱い
`DmRepository::sessions`（一覧）・`unread_session_count`（未読バッジ）は、スレッドの参加者（自分以外）が1人以上いて、かつ全員が次のいずれかに該当する場合そのスレッドを除外する: (1) 自分視点でミュート済み（`mutes.muter_actor_id`=自分）、(2) 自分との間にブロック関係がある（`blocks`、方向を問わない。10節のブロック方針「相互完全非表示」に合わせる）。参加者のうち1人でも対象外（未ミュート・未ブロック）がいれば表示する（グループDMは全員が対象の場合のみ非表示）。`thread_messages`（個別スレッドの閲覧・既読処理）自体は`is_participant`のみのチェックで変更しておらず、スレッドURLを直接踏めば引き続き閲覧できる（一覧・バッジからの発見を防ぐのみ）。

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
- **ドメイン単位のレート制限**（`inbound_activity_process` 向け）: 未実装。現状 `actor_history_sync` キューのみドメイン単位の同時実行制限を持つ。
- **リモートFedi/Bskyユーザー自身の公開リストのオンデマンド取得**: 未実装（`public_lists` はローカルユーザーのみ対象）。
- **ブロック・ミュート関連の未実装項目**: 10節「スコープ外」参照（リアクション一覧でのブロック/ミュート除外、公開リストタイムラインでのフィルタリング）。
# ActivityPubアンケート回答

リモートの `Question` へのローカル回答は、選択肢ごとに
`Create(Note)`（`name` が選択肢名、`inReplyTo` がQuestion ID）として投稿者Inboxへ配送する。
同形式の回答を受信した場合は `poll_votes` に冪等保存し、Questionのローカル集計を更新する。

# Fediverseリレー参加

管理者が登録したHTTPSのinbox URLへ、専用ローカルactor
`https://{domain}/users/relay-agent` からHTTP署名付きFollowを送る。Accept/Rejectは
Follow activity IDと照合して状態更新し、離脱時は元Followを内包したUndoを送る。
一部のリレー実装はAcceptを返さず配送を開始するため、登録inboxと同一originの
リレー鍵で正しく署名された配送を受信した場合も参加成立（`accepted`）とする。
通常のInbox受信では署名者と `activity.actor` の一致を必須とするが、リレー配送は
元投稿者を `activity.actor` に保ったままリレー自身が署名するため、この登録済み
同一originの場合に限って不一致を許可する。
管理APIはSnowflake IDをJavaScriptで丸めないよう文字列として返し、離脱APIのパスにもその文字列をそのまま使用する。
`accepted` のリレーには `visibility='public'` のローカル投稿だけを通常配送と同じ署名・
再試行経路で追加配送し、限定・フォロワー限定・DMは配送しない。

# データベース設計

対象読者: このプロジェクトで DB スキーマに触れる開発者（未来の自分自身も含む）。
正確な DDL は `crates/seiran-common/migrations/` が正であり、ここには書かない。
ここに書くのは「なぜこの形にしたか」という設計判断と、テーブル間の関係。

マイグレーションは `cargo sqlx migrate run` で適用する（`psql -f` で直接流してはいけない。理由は `/home/yuba/seiran/CLAUDE.md` 参照）。

## 1. 全体設計思想

seiran の DB は「ローカル・ActivityPub(Fedi)・AT Protocol(Bsky) という3つの宇宙のアクター・投稿・フォロー関係を1つのテーブルに統一して格納する」ことを核とする。`actors` / `posts` / `follows` / `lists` / `list_members` はいずれもこのパターンで、プロトコル固有の識別子（`ap_uri` / `ap_object_id` / `at_did` / `at_uri` / `at_rkey` 等）を NULL 許容カラムとして併存させている。「ローカル用テーブル」「Fedi用テーブル」のように分けていない。

ID 採番は2系統ある。
- **アプリ側 Snowflake 採番**（`generate_snowflake_id()`、タイムスタンプ内包の BIGINT）: `actors` / `posts` / `media_files` / `custom_emojis` / `notifications` / `lists` / `email_verifications` / `email_changes` / `password_resets` / `atp_blobs`。`posts.id` はタイムライン表示順のソート主軸そのものであり、これが `docs/concept.md` の「統一ポストID」にあたる。
- **DB 側 `GENERATED ALWAYS AS IDENTITY`**: `users` / `reactions` / `follows` / `storage_providers` / `list_members` / `pinned_posts`。順序に意味を持たせる必要がない補助テーブル。

## 2. テーブル一覧

| テーブル | 役割 |
|---|---|
| `users` | ローカルアカウント（メール/パスワード認証、ロール、凍結状態） |
| `actors` | ローカル/リモート(Fedi・Bsky・ブリッジ)を統一するアクター（公開プロフィール実体） |
| `posts` | 投稿・リプライ・リポスト・引用を統一するポストテーブル |
| `reactions` | 投稿への絵文字/いいねリアクション |
| `follows` | フォロー関係（リクエスト中/成立） |
| `remote_follow_snapshots` | リモートFediアクターのfollowers/following全件スナップショット（AP経由の直接取得キャッシュ、`follows`とは独立） |
| `blocks` | ブロック関係（Bsky準拠：フォロー強制解除＋相互完全非表示） |
| `mutes` | ミュート関係（ローカル効果のみ、AP/ATP配送なし） |
| `notifications` | 永続化された通知 |
| `media_files` | アップロード/受信済みメディア実体 |
| `post_attachments` | 投稿とメディアの中間テーブル |
| `custom_emojis` | ローカルのカスタム絵文字定義 |
| `remote_emojis` | AP受信で見つけたリモートカスタム絵文字のカタログ（インポート前提、画像は`media_files`に未取込） |
| `fediverse_relays` | 参加するFediverseリレーのinbox URL・Follow活動ID・承認状態 |
| `storage_providers` | メディア保存先オブジェクトストレージ(S3互換)の設定 |
| `lists` / `list_members` | ユーザーごとのリスト（Bsky `app.bsky.graph.list` 相当） |
| `pinned_posts` | プロフィールへのピン留め投稿 |
| `hashtags` / `post_hashtags` | ハッシュタグ（正規化済みタグ名）とポストのm:n関係 |
| `pinned_hashtags` | ユーザーごとのハッシュタグタブのピン留め（ホーム画面への追加） |
| `post_recipients` | direct投稿（DM）の宛先アクター一覧 |
| `dm_read_states` | DMスレッド別の最終既読ポストID（未読バッジ算出用） |
| `bsky_convo_links` | DMスレッド起点と Bsky `chat.bsky.convo` の convoId の対応キャッシュ |
| `atp_records` | ATP の非 post レコード（`app.bsky.actor.profile` 等）の管理 |
| `atp_blocks` | ATP MST の CAR ブロックストア（CID → バイト列） |
| `atp_repo_events` | ATP `subscribeRepos` 配信用のイベントログ（commit/identity） |
| `atp_blobs` | ATP `uploadBlob` で受信したバイナリ |
| `site_settings` | サイト全体の Key-Value 設定（SMTP 設定、Jetstream カーソル等の汎用格納庫） |
| `instance_domain` | 自ホストドメインの確定値（単一行のみ、一度確定したら不変） |
| `remote_instance_meta` | リモートインスタンス（`actors.domain`単位）のnodeinfoキャッシュ（NoteCardリモートサーバー表示用） |
| `email_verifications` / `email_changes` / `password_resets` | 認証系のワンタイムトークン |
| `user_totp` / `user_totp_recovery_codes` / `totp_disable_requests` | TOTP設定、使い切りリカバリーコード、メール経由の解除トークン |
| `user_passkeys` / `passkey_challenges` | 複数WebAuthn credentialと短命な登録・認証チャレンジ |
| `app_tokens` | MiAuth経由で発行されたアプリトークンの一覧・無効化管理 |

## 3. 主要テーブルの設計判断

### 認証・ユーザー操作レート制限（#223）

- `auth_attempt_log`: ログイン/TOTPの識別子・資格情報をkeyed hashで記録する。平文資格情報は保持しない。制限超過行は`rejected`でIPブロック集計対象になる。
- `auth_ip_blocks`: `INET`主キーごとの認証ブロック期限・理由。期限内の行を管理画面に表示し、個別削除で解除する。
- `account_creation_log`: 同一IPからのアカウント作成成功時刻。
- `user_contact_log`: DM以外のメンション・返信・引用について、送信actorと宛先actorを記録し、1時間内のユニーク宛先数を算出する。
- `search_log`: 検索実行時刻を`actor_id`単位で記録する（初回検索のみ、スクロールによるページング取得は記録しない）。1時間あたりの検索回数レート制限に使う。
- `users.last_login_success_at`: 直近ログイン成功時刻。ブルートフォース判定ウィンドウの起点（パスワードリセット時刻と合わせ、より新しい方を採用）に使う。投稿数・フォロー数・リスト数/人数の各制限は専用ログテーブルを持たず、既存の`posts`（`actor_id`+`created_at`）・`follows`（`follower_actor_id`+`created_at`）・`lists`（`owner_actor_id`）・`list_members`（`list_id`）を直接COUNTして判定する。

### `users.role`（ENUM `user_role`）

`user` / `emoji-editor` / `moderator` / `admin` の4値（#179で `emoji-editor` を追加）。権限の強さは
`admin > moderator > emoji-editor > user`。管理画面（`/admin`）へのアクセス可否・表示タブは
トピック単位（ユーザー管理・サイト設定・ストレージ・絵文字・通報・リレー）でロールごとに決まり、
フロント `frontend/src/lib/roles.ts` の `getAdminTopics` とバックエンド
`crates/seiran-api/src/middleware/auth.rs` の `require_admin`（admin専用）/
`require_emoji_admin`（admin・moderator・emoji-editor）/
`require_report_moderator`（admin・moderator）が対応する。`moderator` は調停者として
「通報」（凍結・投稿削除・連合転送を含む）と「絵文字」の2トピックにアクセス可能、
`emoji-editor` は「絵文字」トピックのみアクセス可能。

### `users` / `actors` の分離
「魂（`users`、当サーバーの住民としての認証アカウント）」と「肉体（`actors`、各プロトコル宇宙での登場人物）」を分離している（`docs/concept.md` 参照）。1つの `users` 行に対し、ローカルユーザーなら基本的に1つの `actors` 行（AP/ATP 両方の識別子を1行に持つ）が対応する。`actors.user_id` は `users` への参照で、ローカルユーザー以外は NULL。

### Bsky流入アクターの保存方針（`bsky_actor_is_engaged`）
JetStreamは「ローカルユーザーのフォロー中/リストメンバーのBsky DID」だけを`wantedDids`として購読する（`crates/seiran-atp-repo/src/firehose.rs`）が、それらの投稿本文中のメンションfacetに現れる無関係な第三者まで`actors`へ永続化してしまうと、自インスタンスと一切関わりのない行が際限なく増える（issue #216、実測で全477,511行中467,603行がbsky型、うち467,008行が投稿0件）。

`bsky_actor_is_engaged(actor_id)`（SQL関数、`LANGUAGE sql STABLE`）が「保存すべきか」を判定する唯一の場所。判定条件（いずれか1つでも真なら保存）: (1) 投稿を1件以上保存した、(2) ローカルユーザーのフォロワーかフォロイーである、(3) リストに含まれる、(4) ローカルポストへの返信・引用・リポスト・リアクション主である、(5) ローカルユーザーとのDM送受信がある、に加えて `blocks`/`mutes`/`poll_votes`/`reports` からの参照（構造的にFK制約があるため）。この関数はJetStream経由の受動的発見（`firehose.rs`の`resolve_or_upsert_bsky_actor`系呼び出し）にのみ適用され、`follows.rs`/`users.rs`/`target_resolve.rs`/`search.rs`のようにユーザーが能動的に参照（フォロー・プロフィール閲覧・「開く」・検索）した経路は無条件で保存する（そちらまで絞ると、関与ゼロから始まる新規フォロー等ができなくなってしまうため）。

メンションfacet由来の未知DID（`posts.mention_facets`に記録されるが`actors`には存在しない）は先行解決・永続化しない。表示時（`NoteResponse`生成時）に他経路で既に保存済みのDIDのみハンドルへ解決され、未解決のまま残ることを許容する。

`actors.actor_type`（ENUM `actor_type_enum`）は6種:
`local` / `remote_seiran` / `fedi` / `bsky` / `fedi_bridge_to_bsky` / `bsky_bridge_to_fedi`

- `seiran_pair_actor_id`: 他 seiran サーバーユーザーの「同じ魂を持つ AP/ATP 両アクター」を相互リンクするための自己参照。**現状、これを書き込む処理は実装されていない**（ゼロトラストハンドシェイクが未実装のため常に NULL。`docs/roadmap.md` 参照）。
- `bridge_real_actor_id`: ブリッジ経由の影武者アクターから本尊アクターへのリンク。

ローカルアクターは `avatar_media_id`/`banner_media_id`（自前 `media_files` 参照）、リモートアクターは `avatar_url`/`banner_url`（URL直持ち）という排他的な使い分けをしている。

`actors.ap_uri`（UNIQUE）は `local` 行も含め全アクター種別が保持する。ローカル行は `https://{local_domain}/users/{username}` を持つ（自ドメインを名乗る Actor URI を誤ってリモートアクター解決経路に渡しても、`find_by_ap_uri`/`upsert_remote_fedi` の `ON CONFLICT (ap_uri)` により `actor_type='fedi'` の影の重複行が生成されない）。リモートActor URI解決処理（`resolve_fedi`/`upsert_remote_fedi_actor`/`RemoteActorResolve`/`follow_fedi`）はこれとは別に、URIが自ドメイン形式に一致する場合は `find_by_username_domain` でローカル行へ解決する明示的なガードも入口に持つ（`docs/protocols.md` 参照）。

自ホストドメイン未確定（シングルホストモード、`instance_domain`参照）の間に作成されたローカルユーザーは `domain='localhost'` で、`at_did`/`at_signing_key_pem` は両方 `NULL`（PLC genesisを行わないため、AT Protocol非対応のローカルユーザーとして存在する）。両カラムは元々 `UNIQUE` かつ `NOT NULL` 制約が無いためスキーマ変更なしでこの状態を表現できる。

`20260728020000_repair_duplicate_fedi_actors.sql` は、物理的に破損した
`actors.ap_uri` / `posts.ap_object_id` UNIQUE index が既存行を見落としていた環境を
修復するデータマイグレーションである。同じAP URIのリモートFedi actorを最小IDへ
統合し、最新プロフィールを維持したまま、投稿・フォロー・リアクション・リスト・
DM等の全外部キー参照を付け替える。複合UNIQUEは正規化後のキーで重複排除し、
同時期に分裂したAP投稿とactor統合で重複するrepostも統合してから両UNIQUE indexを
再構築する。ローカル/ATP identityを含む重複は自動統合せずmigrationを停止する。

`users.language_preference`（設定画面「表示」＞「言語」）: `ja` / `en` / `zh` / `ko` / `es` / `de` / `fr` のいずれか、`NULL` は「自動」（ブラウザの言語設定に従う）を意味する。

`users.token_valid_after`: この時刻より前に発行されたJWTを一括失効させる基準時刻（`docs/architecture.md` 認証節参照）。`NULL` は「制約なし」。パスワード変更・パスワードリセット時に現在時刻へ更新する。

### TOTP関連（`user_totp` / `user_totp_recovery_codes` / `totp_disable_requests`）

`user_totp`はユーザーごとに最大1行を持つ。セットアップ開始時は`enabled=false`で暗号化済みbase32シークレットを保存し、入力された初回コードの検証と10件のリカバリーコード発行が成功したトランザクション内で`enabled=true`にする。シークレットの暗号化は共通のAES-256-GCM鍵を使う。

`user_totp_recovery_codes`は平文を保持せずArgon2ハッシュだけを保存し、使用時に`used_at`を原子的に設定する。`totp_disable_requests`は登録メールアドレスへ送る1時間有効のUUIDワンタイムトークンで、消費時に行を削除してから`user_totp`を削除する。いずれも`users`削除時にCASCADEされる。

### パスキー（`user_passkeys` / `passkey_challenges`）

- `user_passkeys`: `user_id`ごとに複数行を許可し、表示名、WebAuthn credential JSON、登録日時、最終利用日時を保持する。ユーザー削除時はCASCADE削除。
- `passkey_challenges`: 登録または認証のWebAuthn stateをUUIDトークンに対応づける。5分で失効し、finish時に`DELETE ... RETURNING`で一度だけ消費する。

### `posts` の設計
統一ポストID（`id`）はタイムスタンプ内包の Snowflake で、`sinceId`/`untilId` ページネーションの主軸になる。

- `deleted_at`: 物理削除ではなく論理削除（Tombstone）。ATP は MST 上の署名付き履歴を壊せないため。`atp_tombstone_cid` に削除証明の CID を保持する。
- `metadata`（JSONB）: プロトコル別の変形レシピなど拡張情報の汎用格納庫。
- `emoji_map`（JSONB）: 本文中のカスタム絵文字 `:shortcode:` → 画像URL のマップ。Fedi 受信時は AP の `tag` 配列から、ローカル投稿作成時は本文中のショートコード候補（`extract_shortcode_candidates`）を `custom_emojis` と一括照合して、それぞれ投稿作成時に解決・保存する（表示側で都度解決しない静的スナップショット）。
- `mention_facets`（JSONB、デフォルト `[]`）: Bsky投稿のメンションfacet位置情報 `[{"byteStart":N,"byteEnd":M,"did":"did:plc:..."}]`。`emoji_map`とは対照的に、DIDのハンドル解決は保存時ではなく表示時（`NoteResponse`生成時）に都度行う（Bskyハンドルは可変なため。`docs/protocols.md` 6節参照）。ローカル投稿・Fedi受信は常に空配列。
- `is_local`（非正規化 + トリガー）: ローカルタイムライン取得がリモート投稿優勢な環境で劣化する問題への対策。`BEFORE INSERT` トリガー `trg_posts_set_is_local` が `actors.actor_type` から自動導出するため、書き込み漏れの心配がない。
- 重複排除・マージに使うカラム: `seiran_post_uuid`（他 seiran サーバー間マージのキー。**ATP側レコードには埋め込まれていないため、Bsky経由で先に取り込まれた投稿は AP 側の同一投稿と現状マージされない** — 既知の制約）、`parent_original_post_id`（ループバック・一般ブリッジ重複のハードリンク）。
- `visibility`（ENUM `post_visibility_enum`: `public`/`unlisted`/`followers_only`/`direct`）と `deliver_fedi`/`deliver_bsky`（配信先トグル）は独立した軸。リプライは親の可視性を継承する。
- `thread_root_post_id`: `direct`投稿（DM）のスレッド起点ポストID。DM関連テーブルの節を参照。`direct`以外の投稿では常にNULL。
- `reply_count`/`quote_count`/`repost_count`（非正規化 + トリガー）: このポストへの返信・引用・リポストの件数（NoteCardのアクション行に表示）。都度 `COUNT()` するとタイムライン1件ごとにN+1クエリが発生するため、`is_local` と同様に非正規化カウンタ + トリガー方式にしている。`AFTER INSERT` トリガー `trg_posts_relation_counts_insert` が `reply_to_post_id`/`quote_of_post_id`/`repost_of_post_id` を持つ行の INSERT 時に親側のカウンタを+1し、`AFTER UPDATE OF deleted_at` トリガー `trg_posts_relation_counts_delete` が論理削除（`deleted_at` が NULL→非NULL）時に-1する。ローカル作成・Fedi受信・ATP受信・DM同期など `posts` への挿入経路が複数あるため、Rust側の各挿入関数に増減処理を個別実装せずDBトリガーへ一元化している（経路追加時の実装漏れを防ぐため）。

### ダイレクトメッセージ関連（`post_recipients` / `dm_read_states` / `bsky_convo_links`)
DMは`visibility='direct'`の投稿をそのまま`posts`に格納する方式で実現し、Misskey APIクライアントからも読み書きできるようにしている（フロントエンドはタイムライン取得時に`direct`を除外するパラメータを付与することで、Misskey本家の`specified`投稿がタイムラインに現れうる挙動と両立させている）。

- `post_recipients`: `direct`投稿の宛先アクター一覧（`post_id`/`actor_id`のUNIQUE）。Bsky宛先が絡む場合は1対1のみ許可というアプリ側バリデーション（DB制約では表現しない）が別途かかる。
- `thread_root_post_id`（`posts`本体のカラム）: 「スレッド起点ポストを同じくするdirect投稿の集合」をメッセージセッションの単位とするための識別子。通常ポストへの返信として最初のdirect投稿が付いた場合、その最初のdirect投稿自身が起点になる。新規insert時は都度再帰クエリで遡らず、親（`reply_to_post_id`）の`thread_root_post_id`をそのままコピーする伝播コピー方式（親がdirectでない/存在しなければ自分自身のIDを設定）。中央ペインのメッセージ履歴はこの値で束ねて`id`昇順（時刻順）に並べ、ツリー表示はしない。
- `dm_read_states`: `(actor_id, thread_root_post_id)`をPKに持つスレッド別の最終既読ポストID。未読バッジは「未読のあるセッション数」で算出する。
- `bsky_convo_links`: DMスレッド起点とBsky `chat.bsky.convo`のconvoIdの対応キャッシュ（`getConvoForMembers`呼び出し回数を減らすため）。Bsky宛先が絡むスレッドのみ行を持つ。`last_synced_message_id`はBsky DM受信ポーリング（`chat.bsky.convo.getMessages`）が直近まで取り込み済みのBsky側メッセージIDを保持するカーソル。
- `posts.bsky_message_id`（`posts`本体のカラム、Bsky受信DMのみ設定）: Bsky側メッセージIDを保持し部分UNIQUEインデックスを張ることで、DM受信ポーリングの再実行（DB瞬断等での中断からの再開）によるメッセージの重複取り込みを防ぐ冪等キーとして使う。

### `reactions`
`UNIQUE(post_id, actor_id)` — 1投稿につき1ユーザー1リアクション（Misskey 準拠）。切り替え時は `ON CONFLICT DO UPDATE`。`content` は Unicode 絵文字文字列、またはカスタム絵文字の場合 `:shortcode:` 形式。`emoji_url` はカスタム絵文字の画像URL（ローカル送信は `custom_emojis` から解決、Fedi 受信は activity の `tag` から解決、ATP 自己firehose再受信も `custom_emojis` から再解決、Unicode 絵文字は NULL）。`ON CONFLICT DO UPDATE` は `emoji_url` も無条件で上書きするため、insert元となる3経路（`create_reaction`／AP受信の`handle_reaction`／ATP受信の`handle_inbound_like_create`）は全て、`content` が `:shortcode:` 形式なら emoji_url を解決してから渡す必要がある（未解決のまま `None` を渡すと既存の正しい値を消してしまう）。`id`（`GENERATED ALWAYS AS IDENTITY`）は集計用途ではなく、`notifications.reaction_id`（リアクション通知の重複排除トークン、下記参照）としても使う。

### `remote_emojis`
AP受信（投稿本文・表示名・絵文字リアクションのいずれか）で見つけたカスタム絵文字を`(shortcode, domain)`単位で`upsert_seen`し、`first_seen_at`/`last_seen_at`を更新するカタログテーブル。`tags`にはAP Emoji tagの`aliases`/`tags`/`keywords`、`license`にはMisskey拡張`_misskey_license.freeText`を保存し、再受信時に空の値で既知メタデータを消さない。画像は`media_files`へ取り込まない（表示は既存のメディアプロキシ経由でリモートURLを直接参照する）。管理画面「リモート」タブおよびNoteCard右クリックのインポート導線がここを起点に、選ばれた1件だけを`fetch_validated`でダウンロードし`custom_emojis`へ登録する。

### `follows`
`status`（`pending`/`accepted`）を持つ。パフォーマンス上重要な2つの部分インデックスがある: フォロワー取得・AP配送方向の `(target_actor_id, follower_actor_id) WHERE status='accepted'` と、自分のフォロー先取得用のカバリングインデックス `(follower_actor_id) INCLUDE (target_actor_id) WHERE status='accepted'`。

### `remote_follow_snapshots`
`follows` はseiranが認知している関係（ローカルアクターが片方に絡む場合のみ）しか持たない。本テーブルはそれとは独立に、リモートFediアクターのfollowers/following OrderedCollectionをAP経由で直接取得した結果を、`actor_id`×`direction`（`following`/`followers`）単位で丸ごと上書きキャッシュする（`UNIQUE(actor_id, direction)`）。`actor_uris` は取得できたactor URIのJSONB配列、`complete` は上限件数に達せずコレクション全体を取得しきれたか。`docs/protocols.md` 2節参照。

### `blocks` / `mutes`
`follows` と同型（`blocker_actor_id`/`blocked_actor_id`、`muter_actor_id`/`muted_actor_id` の有向関係 + `UNIQUE`制約）。ブロックはBsky準拠の定義（フォロー関係の強制解除＋相互完全非表示）を採用しており、相手がBskyなら `app.bsky.graph.block` コミット後の `atp_rkey` を保存、相手がFediならAP `Block` を配送する（`docs/protocols.md` 参照）。ミュートはFedi/Bsky共通でローカル効果のみ（AP/ATP配送なし）のため `atp_rkey` 相当のカラムを持たない。

タイムライン・通知の相互非表示は、両テーブルを1箇所でOR判定する SQL 関数 `actor_is_hidden_for_viewer(viewer_id, other_id)` に集約している。ブロックは `blocks` テーブルの存在だけでミュート相当のローカル非表示も兼ねる設計（ブロック専用の `mutes` 行を別途作らない）。

投稿の`visibility`（`followers_only`/`direct`）判定も同様にSQL関数 `post_is_visible_to(viewer_id, post_actor_id, post_visibility, post_id, exclude_direct)` に集約している（`docs/code_audit_2026-08-05.md` R-2）。`home_timeline`/`local_timeline`/`social_timeline`/`global_timeline`/`timeline_by_actor`/`find_by_id_for_viewer`/`context_before`/`context_after`の9箇所に同一のOR判定が一字一句コピペされていたのを1関数へ集約した。呼び出し側が既にJOIN済みの`p.actor_id`/`p.visibility`/`p.id`をそのまま渡す設計（関数内で`posts`を再取得しない）で、`actor_is_hidden_for_viewer`と同じ`LANGUAGE sql STABLE`方式によりプランナのインライン展開・既存インデックス（`idx_posts_actor_id`等）でのフィルタ適用を妨げない（EXPLAIN ANALYZEで確認済み）。`local_timeline`/`global_timeline`は関数呼び出しの手前で`p.visibility NOT IN ('unlisted', 'followers_only')`を別途課しており、これらの経路では関数内の`followers_only`分岐は常に到達しない（ローカル/グローバルタイムラインは公開投稿のみを表示する設計のため意図的）。

### `instance_domain`
自ホストドメインの確定値を持つ。`site_settings`（いつでもPATCH経由で書き換え可能な汎用設定）とは別の専用テーブルにしているのは、ドメイン変更が`actors.domain`・PLC DID Document・`ap_uri`に深く食い込みDB整合性を崩壊させるため、「一度確定したら不変」を構造的に保証したいから。`id SMALLINT PRIMARY KEY DEFAULT 1 CHECK (id = 1)`で単一行のみを許容し、このテーブルへの`UPDATE`文はコード上どこにも書かない設計にしている（レビューで機械的に確認できる）。`repository::InstanceDomainRepository::confirm`は`INSERT ... ON CONFLICT (id) DO NOTHING`による冪等な確定操作のみを提供する。Rust側の実行時表現は`seiran_common::LocalDomain`（`Arc<OnceLock<String>>`ラッパー、起動時に一度だけこのテーブルを読み込む）。

### `remote_instance_meta`

`domain`（`actors.domain`と同一値）をPKに持つ、Fedi/seiran間連合の相手サーバー1台につき1行のnodeinfoキャッシュ。`software_name`/`node_name`/`theme_color`/`icon_url`を保持し、更新はしない（`ON CONFLICT (domain) DO UPDATE`の全列上書き、`jobs::remote_instance_info_resolve`が唯一の書き込み元）。`theme_color`は「リモートが宣言した値、または未宣言時に既知フォーク（fedibird/kmyblue/mitra/akkoma）固有色・汎用デフォルト（薄いグレー）へフォールバックした最終表示値」であり、nodeinfo未対応サーバーも含め常に非NULLになる（フロントエンド・Misskey互換クライアントはこの値をそのまま描画すればよく、software別の色分けロジックを持たない設計、Misskey API `UserLite.instance.themeColor` 上位互換）。`node_name`はnodeinfoの`metadata.nodeName`を優先し、未宣言ならトップページの`<title>`タグへフォールバックする（`node_name`列自体がNULLなら、読み出し側`build_instance_info`がドメイン名を暫定表示名にする）。`icon_url`はサーバーホームページの`<link rel="icon">`（無ければ`/favicon.ico`を実際に取得できるか検証した上で）から解決したサーバーアイコンURLで、取得できなければNULL（フロントエンドはアイコン無し表示、🌐等へのフォールバックはしない）。Bskyはこのテーブルを使わず、notes API側で`{name: "Bluesky", softwareName: "bluesky"}`を固定値合成する（PDSごとのnodeinfoが存在しないため）。

notes API / Misskey互換API がノート一覧を組み立てる際、対象ドメインが未キャッシュならその場で`RemoteInstanceInfoResolve`ジョブを積み、今回のレスポンスにはドメイン名を暫定表示名としたフォールバック値を返す（次回以降のリクエストで正式なnodeName/themeColor/icon_urlに置き換わる、`remote_actor_resolve`と同じ「表示のリッチ化はベストエフォート」方針）。加えて`seiran-api::spawn_startup_tasks`が起動のたびに、`remote_instance_meta`未登録、または`icon_url`/`node_name`のいずれかが未取得の既存リモートドメインをまとめて解決ジョブへ積む（新規デプロイ直後の大量未解決状態や、アイコン取得・titleタグフォールバックのような機能追加以前に解決済みだった行の再取得漏れを防ぐための起動時バックフィル）。

### `app_tokens`
MiAuth（`/api/miauth/:session_id/authorize`）認可成立時、または設定画面から直接発行（`POST /api/account/app-tokens`、MiAuth連携を介さない即時発行）した際に生成するJWTは、いずれも自社ログインと同じ`LocalAuthProvider::generate_app_token`を再利用しており、専用のトークン形式を持たない。本テーブルはそのJWTの`jti`（クレームに追加済み）をキーに、クライアント名・発行日時・無効化日時を記録する管理台帳で、JWT自体の検証ロジックには関与しない。認証ミドルウェア（`extract_auth`）はトークン検証成功後に必ず`app_tokens.is_revoked(jti)`を照会し、`revoked_at`が立っていれば拒否する。**このテーブルに行が無いjti（自社ログイン・setup等）は「管理対象外」として常に有効**として扱う（全トークンを網羅する台帳ではない）。設定画面の一覧・無効化操作は本人（`user_id`一致）のみ可能。生のトークン文字列自体はDBに保存されない（`jti`のみ）ため、直接発行APIのレスポンスでのみ一度だけ返す。

### `notifications`
`type` はフォロー・リアクション・メンション・返信に加えて、ローカル投稿へのリポスト（`repost`）と引用（`quote`）を保持する。リポスト・引用通知の `note_id` は通知の契機になった新しいリポスト／引用投稿を指す。

`source_uri`（発生源イベントの一意識別子、ATP Like の `at_uri` や AP の `ap_activity_id`）に部分 UNIQUE インデックスを張り、Jetstream/AP の複線受信による重複 INSERT を防いでいる。`reaction_emoji_url` は通知発生時点の絵文字画像URLをスナップショット保存する非正規化カラム（`reactions` が1人1リアクションのため、後から絵文字を切り替えると過去のリアクション内容を復元できなくなる問題への対処）。

`reaction_id`（`reactions.id` を保存、部分 UNIQUE インデックス）は `source_uri` とは別目的の重複排除トークン。ローカルユーザーが ATP 実体を持つ投稿へリアクションすると「ローカル即時通知」と「その ATP コミットが自分自身の firehose 経由で戻ってきた再受信通知」の2経路が走ってしまうため、両方に同じ `reactions.id` を持たせて UNIQUE 制約で片方を弾く（`docs/protocols.md` 8節）。他人発のリアクションでは常に `NULL` になり、同じ投稿への複数回の連続リアクション（絵文字を変えて通知欄に文章を書く等）を妨げない設計を維持している。

### `hashtags` / `post_hashtags` / `pinned_hashtags`
ハッシュタグは検索結果の即席表示ではなく、ポストとm:nの関係を持つ永続化オブジェクトとして扱う。`hashtags.name` は正規化済み（先頭`#`除去・小文字化、グルーピング用の内部表現）。表示上の大文字小文字は各投稿の `posts.body` 原文に委ねる（`hashtags` テーブル自体は表示用の値を持たない）。

抽出はプロトコル別の特別処理を持たず、ローカル投稿・AP受信・Bsky受信いずれも「最終的な `posts.body` テキストを1回スキャンする」共通経路（`seiran_common::hashtag::extract_hashtags`）で行う。AP由来のハッシュタグアンカーは `[#foo](リモートのタグページURL)` というMarkdownリンクに変換されるが、リンクテキスト部分に `#foo` がそのまま残るため、この共通スキャンだけで3ソースとも取りこぼしなく抽出できる（`docs/protocols.md` 6節参照）。抽出・リンクは投稿INSERT直後のベストエフォート処理で、失敗しても投稿自体は成立させる。

`pinned_hashtags` は「ホーム画面に追加」操作の永続化（`pinned_posts` と同じ設計思想）。ハッシュタイムライン自体は `post_hashtags` を介した検索であり、ピン留めの有無に関係なく誰でも `/tags/:name` で閲覧できる。ハッシュタイムラインは `visibility IN ('public', 'unlisted')` のみを対象にする（特定アクター向けの閲覧制御が要るフィードではなく発見用の公開フィードのため、`followers_only` の例外は設けない）。

### メディア関連（`media_files` / `post_attachments` / `atp_blobs`)
`media_files` は画像専用として始まったため `width`/`height`/`blurhash` は NULL 許容(動画・音声はこれらを持たない)。`bsky_video_*` 系カラムは Bluesky 公式動画パイプライン（`app.bsky.video.uploadVideo`）との連携状態を追跡する。`(sha256, blurhash)` の複合 UNIQUE でグローバル重複排除。

`post_attachments` は `media_file_id`（ローカル添付）と `remote_url`/`remote_mime_type`/`remote_thumbnail_url`（リモート受信添付）が排他的に埋まる設計。
ActivityPub受信添付の`is_sensitive`は画像単位の`attachment[].sensitive`を保存し、投稿全体の
`sensitive=true`も全添付へ安全側に伝播する。`posts.content_warning`はAP `summary`、
`posts.poll`はAP `Question`の`oneOf`/`anyOf`・票数・締切を表示用JSONとして保存する。
`post_attachments.is_gif`はGIFアニメ由来（Tenor/Klipy GIFピッカー、またはBsky動画パイプラインが
`presentation:"gif"`を付与するGIFファイル直接アップロード。`docs/protocols.md`参照）を示し、
フロントは動画添付を自動再生・ミュート・ループ・コントロール無しで表示する（`HlsVideo`の
`isGif` prop）。デフォルト`FALSE`、既存のTenor/Klipy由来行はURLパターン
（`t.gifs.bsky.app`/`k.gifs.bsky.app`）でバックフィル済み。
`atp_blobs` は `uploadBlob` で受信した任意バイナリ（Bsky動画パイプラインが提出してくるトランスコード済み動画等）を保存する。`sha256` に UNIQUE を張り、content-addressable な重複排除を行う。

### `post_link_cards`（URLカード）
1投稿につき0件以上のURLカードを`post_id`/`position`で保持する（`id`はGENERATED ALWAYS AS IDENTITYの補助テーブル、順序は`position`が担う）。Bskyは`app.bsky.embed.external`（GIFピッカー由来のTenor/Klipyを除く。GIFは`post_attachments`側で動画添付として扱うため排他）由来で常に`position=0`の最大1件。Fediは本文中の複数リンクぶん複数件になりうる（`docs/protocols.md`参照）。`title`/`description`は空文字列を許容し、`thumbnail_url`のみNULL許容。取得は`fetch_link_cards_map`（`post_id`一覧→`HashMap<i64, Vec<LinkCardResponse>>`、`crates/seiran-api/src/handlers/notes/queries.rs`）で一括解決し、`NoteResponse.link_cards`へ差し込む（`post_attachments`の`fetch_attachments_map`と同じ構造）。

### ATP リポジトリ関連（`atp_records` / `atp_blocks` / `atp_repo_events`)
seiran は自前 PDS としてローカルユーザーの ATP リポジトリ（MST）を管理する。`app.bsky.feed.post` は `posts` テーブルで一元管理し、`atp_records` にはそれ以外のコレクション（`app.bsky.actor.profile` 等）だけを持つ。`atp_blocks` は CAR ブロックの実体、`atp_repo_events` は Relay へブロードキャストする `subscribeRepos` フレームのログで、`id`（BIGSERIAL）がそのまま Relay カーソル(seq)になる。`frame_bytes` にコミット時点で生成したフレームのバイト列をそのまま保存しており、再送時に再構築しない（バイト列差異による Relay 切断を避けるため）。

## 4. 典型的なクエリパターン

- **ホーム/ローカルタイムライン**: `posts` を `id`（降順）でページネーションするだけの単純な SQL。フォロー時点で相手の過去ログを丸ごと自サーバー DB に取り込んでいるため、外部 API 呼び出しを伴わない（`docs/concept.md` 「タイムラインは自前の池」参照）。
- **ソーシャル/グローバルタイムライン（#78）**: `PostRepository::social_timeline`（自分+フォロー中+ローカル全体、home_timelineのLATERAL方式候補とlocal_timelineの`is_local`候補をUNIONしてから外側で再度LIMIT）・`global_timeline`（local_timelineから`is_local`条件のみ外したもの）。新規テーブルは無く、`home_timeline`/`local_timeline`と同じインデックス（`idx_posts_actor_id`・`is_local`列）で完結する。ひかえめ（`unlisted`）・プライベート（`followers_only`）投稿は、投稿者本人やフォロワーが閲覧してもローカル/グローバルには出さず、ホームとソーシャルにだけ表示する（#91, #105）。
- **引用関係（#116）**: `posts.quote_of_post_id` はローカル作成だけでなく、APの `quoteUrl` / `_misskey_quote` / Misskey Hub互換Link tag、およびBskyの `app.bsky.embed.record` / `recordWithMedia` 受信時にも、保存済み投稿の `ap_object_id` / `at_uri` を引いて設定する。`20260728000000_backfill_quote_of_post_id.sql` は旧AP投稿本文末尾の `RE:` / `QT:` フォールバックURLから既存関係を復元し、復元できた重複行を本文から除去する。
- **検索**: ローカル DB の投稿本文検索（`idx_posts_body_bigm`、pg_bigm）と AppView 検索の結果をマージする。pg_bigm は `LIKE` 演算子のみ最適化対象で `ILIKE` には対応しないため、投稿検索 SQL は `LOWER(body) LIKE LOWER(pattern)` 形式とし、インデックスも `LOWER(body)` に張っている（#97）。アクター検索は用途別に分離する。リスト編集・DM宛先の`GET /api/actors/search`は、`COALESCE(display_name, '')`とFedi/Bsky/ローカルの全ハンドル表記を改行区切りで連結した式へ生入力を部分一致させ、`idx_actors_search_bigm`（pg_bigm GIN式インデックス）を使う。投稿欄の`GET /api/actors/suggest`は表示名を対象にせずハンドル前方一致だけを行い、`idx_actors_handle_prefix`と`idx_actors_local_bsky_handle_prefix`（`text_pattern_ops` B-tree式インデックス）を個別走査して`UNION`する。環境依存のローカルドメインをmigrationへ焼き込まないため、式中のローカル判定には同義の`actor_type = 'local'`を使う。セッション管理の詳細は `docs/architecture.md` の検索セッション節を参照。
# アンケート回答

`poll_votes` はActivityPub `Question`への回答を、投稿・回答Actor・選択肢番号単位で保持する。
複数回答では同一Actorが複数行を持ち、`ap_activity_id` の一意制約でリモートからの再配送を
冪等化する。集計表示用の票数は `posts.poll` にも反映する。認証付きの投稿読取APIは
`poll_votes` から回答者自身の選択肢番号を `poll.votedByMe` として付与し、クライアントが
リロード後も回答済み状態と選択内容を復元できるようにする。

### 通報（`reports` / `report_comments`）

`reports` はローカル・Fedi・Bsky共通の管理台帳で、通報者、対象Actor、任意の対象Post、
Bluesky公式（`tools.ozone.report.defs`）準拠の理由分類、自由記述、`destination`/`remote_host`
（対象Actorがリモートかどうかをサーバー側で自動算出した値）と処理状態を保持する。通報者は
送信先を選ばず、通報は常にローカル管理者へ届く。`destination='remote'` の通報のみ、管理者が
任意にFedi/Bskyへ転送できる。
理由分類は「なぜこの投稿をレビューする必要がありますか？」（8カテゴリ）→「理由を選択」
（カテゴリごとに4〜7項目、計39種）の2段階選択で、`reason_type` にはOzoneのトークン名
（例: `reasonMisleadingSpam`）をそのまま保存する。カテゴリはトークン名から一意に導出できるため
別カラムは持たない。投稿通報では対象投稿が後から削除されても調査履歴を残すため `subject_post_id` は
`ON DELETE SET NULL` とし、投稿種別では削除後のNULLも許容する。自由記述はDB制約でも300書記素相当（PostgreSQLの文字数）かつ
1000バイト以下に制限する。リモート転送の成功時刻は `forwarded_at` に記録し、再送判断に使う。

`report_comments` は管理者・モデレーターだけが読み書きできる内部メモで、通報削除時は
CASCADE削除する。通報自体は監査履歴として物理削除せず、`status` と `closed_at` で
オープン/クローズを管理する。

### Fediverseリレー（`fediverse_relays`）

管理者が登録したリレーを `inbox_url`（UNIQUE）単位で保持する。`status` は
`pending` / `accepted` / `rejected`、`follow_activity_id` は参加時のFollowと
Accept/Rejectを照合し、離脱時のUndoにも再利用する。公開投稿の配送先には
`accepted` の行だけを使う。

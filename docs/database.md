# データベース設計

対象読者: DB スキーマに触れる開発者。正確な DDL は `crates/seiran-common/migrations/` が正で、ここには設計判断とテーブル間の関係だけを書く。

マイグレーションは `cargo sqlx migrate run` で適用する（`psql -f` で流さない。理由は `CLAUDE.md`）。

## 1. 全体設計思想

ローカル・ActivityPub・AT Protocol の3つの世界のアクター・投稿・フォロー関係を、1つのテーブルに統一して格納する。`actors`/`posts`/`follows`/`lists`/`list_members` はプロトコル固有の識別子（`ap_uri`/`ap_object_id`/`at_did`/`at_uri`/`at_rkey` 等）を NULL 許容の列として併存させ、プロトコル別のテーブルに分けない。

ID の採番は2系統:
- **アプリ側 Snowflake**（`generate_snowflake_id()`、タイムスタンプ内包の BIGINT）: `actors`/`posts`/`media_files`/`custom_emojis`/`notifications`/`reactions`/`lists`/`email_verifications`/`email_changes`/`password_resets` 等。`posts.id` はタイムラインの並び順そのもの（`docs/concept.md` の「統一ポストID」）。
- **DB 側 `GENERATED ALWAYS AS IDENTITY`**: `users`/`follows`/`storage_providers`/`list_members`/`pinned_posts`。順序に意味の無い補助テーブル。

## 2. テーブル一覧

| テーブル | 役割 |
|---|---|
| `users` | ローカルアカウント（メール/パスワード、ロール） |
| `actors` | ローカル/リモート（Fedi・Bsky・ブリッジ）を統一するアクター |
| `posts` | 投稿・リプライ・リポスト・引用 |
| `reactions` | 絵文字/いいねリアクション |
| `dm_bsky_reactions` / `dm_hidden_messages` | Bsky 宛 DM 専用のリアクション・個人非表示フラグ |
| `follows` | フォロー関係（申請中/成立） |
| `remote_follow_snapshots` | リモート Fedi アクターの followers/following 全件のキャッシュ（`follows` とは独立） |
| `follow_import_requests` / `follow_import_items` | フォローインポートの実行単位と対象ごとの状態 |
| `blocks` / `mutes` / `repost_mutes` | ブロック（Bsky 準拠）・ミュート・リポストミュート |
| `notifications` | 通知 |
| `media_files` / `post_attachments` | メディア実体と投稿との中間テーブル |
| `custom_emojis` / `remote_emojis` | ローカルのカスタム絵文字、AP 受信で見つけたリモート絵文字のカタログ |
| `fediverse_relays` | 参加する Fediverse リレー |
| `storage_providers` | S3 互換ストレージの設定 |
| `lists` / `list_members` | リスト（`app.bsky.graph.list` 相当） |
| `actor_also_known_as` | プロフィールの「別のアカウント」 |
| `pinned_posts` | ピン留め投稿 |
| `hashtags` / `post_hashtags` / `pinned_hashtags` | ハッシュタグ、ポストとの m:n、ホームへのピン留め |
| `post_recipients` / `dm_read_states` / `bsky_convo_links` | DM の宛先、既読カーソル、Bsky convoId の対応 |
| `atp_records` / `atp_blocks` / `atp_repo_events` | ATP の非 post レコード、MST のブロックストア、`subscribeRepos` のイベントログ |
| `atp_app_passwords` / `atp_refresh_tokens` / `atp_preferences` | ATP のアプリパスワード、refreshJwt の `jti`、クライアント設定 |
| `at_migration_requests` / `at_migration_records` / `at_migration_blobs` | DID 転入の状態機械とステージング |
| `site_settings` | サイト全体の Key-Value 設定（SMTP・Jetstream カーソル等） |
| `instance_domain` | 自ホストドメインの確定値（1行、不変） |
| `remote_instance_meta` | リモートインスタンスの nodeinfo キャッシュ |
| `email_verifications` / `email_changes` / `password_resets` | 認証系のワンタイムトークン |
| `email_short_codes` | ATP セッション2FA・PLC 操作署名のメール6桁コード |
| `user_totp` / `user_totp_recovery_codes` / `totp_disable_requests` | TOTP |
| `user_passkeys` / `passkey_challenges` | パスキー |
| `app_tokens` | MiAuth/設定画面で発行したアプリトークン |
| `auth_attempt_log` / `auth_ip_blocks` / `account_creation_log` / `user_contact_log` / `search_log` | レート制限 |
| `reports` / `report_comments` | 通報 |

## 3. 主要テーブルの設計判断

### レート制限（#223）

- `auth_attempt_log`: ログイン/TOTP の識別子・資格情報を keyed hash で記録する（平文は持たない）。拒否された行は `rejected` で IP ブロックの集計に使う。
- `auth_ip_blocks`: `INET` 主キーごとの遮断期限・理由。管理画面で一覧・個別解除する。
- `account_creation_log`: 同一IPからのアカウント作成時刻。登録処理の冒頭で `reserve_account_creation` が IP 単位のアドバイザリロック下で「窓内の件数が上限未満なら1行予約」し、登録に失敗したら `cancel_account_creation` で消す（成功した登録だけが残る）。数えてから（PLC 登録で数秒かかる）記録すると、並列の登録がすべて上限判定をすり抜けるため。
- `user_contact_log`: DM 以外のメンション・返信・引用の送信者と宛先。1時間内のユニーク宛先数に使う。
- `search_log`: 初回検索の時刻（ページングは記録しない）。
- `search_log`・`user_contact_log` の「数えて上限未満なら記録」はトランザクション内で actor 単位のアドバイザリロック（`pg_advisory_xact_lock(hashtextextended(<テーブル名>, actor_id))`、`rate_limit::lock_actor_rate_limit`）で直列化する。
- `users.last_login_success_at`: ブルートフォース判定の窓の起点（パスワードリセット時刻と新しい方）。
- 投稿数・フォロー数・リスト数/人数の制限は専用ログを持たず、`posts`/`follows`/`lists`/`list_members` を直接数える。

### `users.role`（ENUM `user_role`）

`user` < `emoji-editor` < `moderator` < `admin`。管理画面のトピック（ユーザー管理・サイト設定・ストレージ・絵文字・通報・リレー）ごとにロールで可否が決まり、フロントの `lib/roles.ts::getAdminTopics` とバックエンドの `require_admin`（admin）/`require_emoji_admin`（admin・moderator・emoji-editor）/`require_report_moderator`（admin・moderator）が対応する。`moderator` は「通報」（凍結・投稿削除・転送を含む）と「絵文字」、`emoji-editor` は「絵文字」だけ。

### `users` / `actors` の分離
「魂」（`users`、このサーバーの認証アカウント）と「肉体」（`actors`、各プロトコルでの登場人物）を分ける（`docs/concept.md`）。ローカルユーザーは基本的に1つの `actors` 行（AP/ATP 両方の識別子を持つ）に対応する。`actors.user_id` はローカル以外 NULL。

ローカルの `users` 行と `actors` 行は `repository::create_local_account` が1トランザクションで作る。別々に作ると、同名の同時登録で `actors` 側だけが一意制約に違反したとき actor の無い `users` 行が残り、そのメールアドレスで二度と登録もログインもできなくなる。一意制約違反は 409（`EMAIL_ALREADY_REGISTERED`/`USERNAME_TAKEN`）。

- `users.language_preference`: 表示言語（`SUPPORTED_DISPLAY_LANGUAGES` の8言語）。NULL は「自動」。
- `users.token_valid_after`: これより前に発行された JWT を一括失効させる基準時刻（NULL は制約なし）。

### `actors`

`actor_type`（ENUM `actor_type_enum`）: `local` / `remote_seiran` / `fedi` / `bsky` / `fedi_bridge_to_bsky` / `bsky_bridge_to_fedi`。

- **アバター・バナー**: ローカルは `avatar_media_id`/`banner_media_id`（`media_files` 参照）、リモートは `avatar_url`/`banner_url`（URL 直持ち）。表示時は `find_avatar_url`/`find_banner_url` が `media_id` 優先の COALESCE で解決する。リモートの値はプロフィール表示のたびに `Job::RemoteProfileRefresh` が更新する。バナーの代替生成は無い（無ければ非表示）。
- **`ap_uri`（UNIQUE）**: ローカル行も `https://{local_domain}/users/{username}` を持つので、自ドメインの Actor URI がリモート解決経路に入っても `ON CONFLICT (ap_uri)` で影の `fedi` 行はできない（入口のガードと二重防御、`docs/protocols.md` 5節）。
- **退会済みの除外**: `find_by_username_domain` は `withdrawn_at` のあるアクターを除く（プロフィール・フォロー解決・検索等）。退会済みも解決できないと処理が失敗する内部処理（AP 受信の同一性検証等）は、あえて不自然な名前の `find_including_withdrawn_by_username_domain` を使う（呼び出し側にどちらかを意識させるため）。
- **シングルホストモード**（`instance_domain` 参照）で作られたローカルユーザーは `domain='localhost'`、`at_did`/`at_signing_key_pem` は NULL（PLC genesis をしない）。
- `birth_date`/`birth_date_public`: Misskey 互換の `birthday`。`birth_date_public`（既定 `false`、seiran 独自）は `vcard:bday` で Fedi に出すかどうか。Bsky の `#personalDetailsPref` は常に非公開（`docs/protocols.md` 3節）。
- `hide_from_algorithmic_recommendations`: `contentVisibilityDeclaration` のローカルキャッシュ（`docs/protocols.md` 3節）。
- `is_locked`: フォロー承認制。投稿の公開範囲には影響しない（`docs/protocols.md` 2節）。
- `at_rotation_key_pem`: アカウント単位の PLC ローテーションキー（`docs/account_migration.md` 6節）。NULL は、この列より前に作られた/転入したアカウントの取り残しだけ。
- `did_moved_out_at`: DID 転出済みの時刻（`submitPlcOperation` 成功か `deactivateAccount` で設定）。読み取りは通常どおりで、書き込みだけ拒否する。
- `claimed_ap_uri`/`claimed_at_did`: 相互申告マージ（#236）用の、まだ確認できていない自己申告（`bsky` 行が申告する AP Actor URI、`fedi` 行が申告する DID）。相互一致で結婚したら `remote_seiran` に昇格して NULL に戻す。相互に申告し合う2行の共存は複合 UNIQUE `actors_mutual_claim_key` で禁止する（`docs/protocols.md` 11節、`seiran_actor_merge`・`unique_retry`）。
- `at_handle`: ATP ハンドル。`remote_seiran` の `username` は Fedi 側のまま固定されるので、Bsky ID の表示にはこれを使う。発見・再訪問のたびに最新にする（`upsert_remote_bsky`・`discover_bsky_actor`）。
- `bridge_real_actor_id`: ブリッジユーザーから実ユーザーへのリンク。
- `seiran_pair_actor_id`: 使っていない（常に NULL）。削除を検討中。

### `actors.notes_count` / `followers_count` / `following_count`（非正規化カウンタ）
Misskey 互換 `users/show` の一括取得やプロフィールの件数は、COUNT ではなくこれらを読む。
- `notes_count`: `repository/post.rs` の INSERT 系と `soft_delete_by_*` が data-modifying CTE（`WITH x AS (INSERT/UPDATE ... RETURNING ...) UPDATE actors ...`）で、実際に行が変化したときだけ加減する（`GREATEST(count - 1, 0)` で負にしない）。対象は `deleted_at IS NULL` の投稿（リポストも1件）。
- `followers_count`/`following_count`: `trg_follows_sync_counts` トリガーが `follows` の変更のたびに、該当アクターの `status='accepted'` を COUNT し直して SET する。増減方式は一度どこかで更新が漏れると差が積み上がって戻らないので、書き込み頻度が `posts` ほど高くない `follows` は毎回数え直す（インデックスで十分速い）。

### Bsky流入アクターの保存方針（`bsky_actor_is_engaged`）
Jetstream はフォロー中・リストメンバーの DID だけを購読するが、その投稿のメンション facet に出てくる無関係な第三者まで `actors` に保存すると、関わりの無い行が際限なく増える。

`bsky_actor_is_engaged(actor_id)`（SQL 関数）が保存するかどうかを決める唯一の場所。いずれかが真なら保存する: 投稿を保存済み、ローカルユーザーのフォロワー/フォロイー、リストのメンバー、ローカル投稿への返信・引用・リポスト・リアクションの主、ローカルユーザーとの DM、`blocks`/`mutes`/`poll_votes`/`reports` からの参照（FK があるため）。適用するのは Jetstream 経由の受動的な発見（`firehose.rs` の `resolve_or_upsert_bsky_actor` 系）だけで、ユーザーが能動的に参照した経路（フォロー・プロフィール閲覧・「開く」・検索）は無条件に保存する（関与ゼロからの新規フォローができなくなるため）。

メンション facet の未知 DID は先に解決・保存しない。表示時に、他の経路で保存済みの DID だけがハンドルになる。

### `actors.suspended_at`（ユーザー凍結）
ローカル・リモート共通の凍結状態。`withdrawn_at` と同じく actors 側に持つことで、enforcement を1本の列・クエリ経路に揃える。

- **API**: `extract_auth` が凍結中のローカルユーザーの全 API を `ACCOUNT_SUSPENDED` で拒否する。例外は `GET /api/auth/me`（`extract_auth_allow_suspended`）で、フロントが凍結専用画面（ユーザー名とログアウトボタンだけ）に切り替える入口。ログイン自体は成功する。
- **表示**: `actor_is_hidden_for_viewer`（SQL 関数）が無条件に凍結を判定するので、タイムライン・通知・リアクション一覧・ハッシュタグ・フォロー一覧・ピン留めから除かれる。単体取得（`find_by_id`/`find_by_id_for_viewer`）はこの関数を通らないので、パーマリンクやスレッドの遡りでは実データを返す。
- **参照の埋め込み**: 引用・リポスト・返信先の `NoteResponse.author_suspended` を立て、フロント（`NoteCard` の `isMainSubject`）が埋め込み表示のときだけ本文を「凍結されたユーザーのポストです」に差し替える。
- **AP 受信**: inbox が署名者の凍結で 403、ジョブが `activity.actor` の凍結で Follow/Create/Like/EmojiReact/Announce を破棄する（Undo/Delete/Update は対象外）。
- **ATP 受信**: firehose が新規投稿の対象判定に `suspended_at IS NULL` を含め、リポスト・いいねも確認する。
- **管理 API**: `POST /api/admin/actors/:id/suspend`・`unsuspend`（actor_id 起点）、`GET /api/admin/suspended-actors`。ユーザー管理の `POST /api/admin/users/:id/suspend` と通報画面の `suspend-user` も同じ列を更新する。

マイグレーション `20260728020000_repair_duplicate_fedi_actors.sql` は、壊れた UNIQUE index のせいで同じ AP URI のリモート Fedi actor・投稿が複数行に分かれた環境の修復用。最小 ID に統合して全 FK を付け替え、index を作り直す（ローカル/ATP の identity を含む重複は自動統合せず停止する）。

### TOTP（`user_totp` / `user_totp_recovery_codes` / `totp_disable_requests`）

- `user_totp`: ユーザーごとに最大1行。セットアップ開始時は `enabled=false` で暗号化済みシークレットを保存し、初回コードの検証と10件のリカバリーコード発行が成功したトランザクションで `enabled=true` にする。
- `user_totp_recovery_codes`: Argon2 ハッシュだけを保存し、使用時に `used_at` を原子的に設定する。
- `totp_disable_requests`: 登録メールへ送る1時間有効のワンタイムトークン。消費時に行を消してから `user_totp` を消す。
- いずれも `users` の削除で CASCADE。

### パスキー（`user_passkeys` / `passkey_challenges`）

- `user_passkeys`: ユーザーごとに複数行。表示名・credential JSON・登録日時・最終利用日時。
- `passkey_challenges`: WebAuthn の state を UUID に対応づける。5分で失効し、完了時に `DELETE ... RETURNING` で一度だけ消費する。`user_id` は NULL 可（ユーザー名なしログインの開始時は未確定）。

### `posts` の設計
`id`（Snowflake）が `sinceId`/`untilId` ページネーションの主軸。`TimelinePost` を返すクエリは `timeline_post_columns!()`/`timeline_post_joins!()` で列を揃える（`docs/coding_rules.md` 2節）。

- `deleted_at`: 論理削除（Tombstone）。ATP は MST 上の署名付き履歴を壊せないため。`atp_tombstone_cid` に削除証明の CID を持つ。
- `metadata`（JSONB）: プロトコル別の拡張情報の格納庫。
- `emoji_map`（JSONB）: 本文の `:shortcode:` → 画像URL。Fedi 受信は AP の `tag`、ローカル投稿は本文の候補（`extract_shortcode_candidates`）を `custom_emojis` と照合して、作成時に解決して保存する（表示時には解決しない）。
- `content_html`（nullable）: リモート Fedi 投稿だけが持つ、許可リストでサニタイズした HTML。`body` は Misskey 互換 API・Bsky 配送・検索・ハッシュタグ抽出が前提とする唯一の形式なので変えず、構造を保った表示用にこれを別に持つ（`docs/protocols.md` 6節）。ローカル・Bsky・この列より前の投稿は NULL。
- `mention_facets`（JSONB、既定 `[]`）: Bsky 投稿のメンション facet（`[{"byteStart","byteEnd","did"}]`）。ハンドルは可変なので `emoji_map` と違い表示時に解決する。
- `is_local`（非正規化）: ローカル TL がリモート投稿の多い環境で遅くならないように持つ。`BEFORE INSERT` トリガー `trg_posts_set_is_local` が `actors.actor_type` から導出するので書き漏れない。
- `parent_original_post_id`: ループバック・一般ブリッジ重複のハードリンク（`docs/protocols.md` 5節）。`seiran_post_uuid` は使っていない（削除検討中）。
- ブリッジポスト（`docs/protocols.md` 5節）: `bridge_of_post_id`（解決済みの元ポスト）、`bridged_original_uri`（元ポストの識別子の生値。ブリッジポストかどうかはこちらの NOT NULL で判定）、`ap_bridge_post_id`/`atp_bridge_post_id`（元ポスト側から見たブリッジポスト）。部分インデックス `idx_posts_bridge_pending`（`bridge_of_post_id IS NULL AND bridged_original_uri IS NOT NULL`）で未解決のブリッジを探す。
- `claimed_ap_object_id`/`claimed_at_uri`: 他 seiran 間の投稿マージ（#237）用の、`seiranPost.counterpartPostId` で申告されたまだ未確認の相手側ID。相互一致でマージしたら NULL に戻す。相互に申告し合う2行の共存は複合 UNIQUE `posts_mutual_claim_key`（`deleted_at IS NULL` の部分インデックス）で禁止する。
- `visibility`（ENUM: `public`/`unlisted`/`followers_only`/`direct`）と `deliver_fedi`/`deliver_bsky`（配送先）は独立した軸。リプライは親の可視性を継承する。
- `thread_root_post_id`: DM のスレッド起点（`direct` 以外は NULL）。
- `reply_count`/`quote_count`/`repost_count`（非正規化）: 都度 COUNT するとタイムラインで N+1 になるため。`trg_posts_relation_counts_insert`（INSERT で親を +1）と `trg_posts_relation_counts_delete`（`deleted_at` の NULL → 非NULL で -1）が管理する。挿入経路が多いので、Rust 側ではなくトリガーに一元化している。
- `pending_bsky_media_file_id`: `BskyPostCommitDeferred` が待っている `media_files.id`。起動時リカバリが `resolve_bsky_embed` の優先順位判定を再現せずに済むよう、作成時に保存する。コミット成功で NULL に戻す（`docs/architecture.md` 5節）。
- `bsky_reply_allow`（JSONB、NULL = 制限なし、`[]` = 投稿者以外は返信不可）/`bsky_quote_disabled`: リモート Bsky 投稿の threadgate の `allow` と、postgate の `#disableRule` の有無（`docs/protocols.md` 3節）。
- `language`: ISO 639-1。Bsky の `langs` にだけ使う。許可値（`SUPPORTED_LANGUAGES`、7言語）はアプリ層で検証する。NULL なら `langs` を省略する。
- `poll`/`poll_update_received`/`poll_fetched_at`: 下記「アンケート」。
- `{reply_to,quote_of,repost_of}_ap_uri`/`_ref_status`（ENUM `post_reference_status`: `pending`/`gone`）: 取り込み時に解決できなかった参照（`docs/protocols.md` 1節・4節）。

### ダイレクトメッセージ関連（`post_recipients` / `dm_read_states` / `bsky_convo_links`)
DM は `visibility='direct'` の投稿として `posts` に格納し、Misskey 互換クライアントからも読み書きできる（フロントはタイムライン取得時に `direct` を除外するパラメータを付ける）。

- `post_recipients`: 宛先（`post_id`/`actor_id` の UNIQUE）。Bsky 宛は1対1のみという制約はアプリ層で検証する。
- `posts.thread_root_post_id`: 同じ起点を持つ `direct` 投稿の集合をメッセージセッションの単位にする。INSERT 時に再帰で遡らず、親が `direct` なら親の値をコピーし、そうでなければ自分の ID（伝播コピー）。メッセージ履歴はこの値で束ねて `id` 昇順に並べる（ツリーにしない）。
- `dm_read_states`: `(actor_id, thread_root_post_id)` PK のスレッド別の最終既読ポストID。
- `bsky_convo_links`: スレッド起点と Bsky convoId の対応（`getConvoForMembers` の呼び出しを減らす）。`last_synced_message_id` は受信ポーリングのカーソル。
- `posts.bsky_message_id`: Bsky 側のメッセージID。部分 UNIQUE で受信ポーリングの再実行による重複取り込みを防ぐ。送信したメッセージにも `sendMessage` の応答から書き戻す（`addReaction` に必要なため）。

### `dm_bsky_reactions` / `dm_hidden_messages`
Bsky 宛の DM は制約が違うので専用テーブルで扱う（fedi/local のみの DM は `reactions` と通常の削除、`docs/protocols.md` 9節）。

- `dm_bsky_reactions`: `UNIQUE (post_id, actor_id, content)`。Bluesky のチャット API は1ユーザーが異なる Unicode 絵文字を複数付けられる（メッセージ全体で最大5件）ので、`reactions` の「1人1個」とは別モデル。相手発のリアクションは `bsky_dm_poll` が `sync_bsky_reactions` でメッセージ単位に置き換える。
- `dm_hidden_messages`: `(actor_id, post_id)` PK の閲覧者ごとの非表示フラグ（`deleteMessageForSelf` 相当）。Bsky 宛でないメッセージにも使える。`thread_messages` は `NOT EXISTS` で除く。

### `reactions`
`UNIQUE(post_id, actor_id)`（Misskey と同じ1人1個）。
- 記録・切り替えは `ReactionRepository::upsert` が1トランザクションで行う（`INSERT ... ON CONFLICT DO NOTHING` で新規を試み、既存なら `SELECT ... FOR UPDATE` で旧値を読んで `UPDATE` し、旧値を返す）。取り消し（`delete_local`）は `DELETE ... RETURNING` で消した行の `ap_activity_id`/`at_uri`/`emoji_url` を返す。旧値（AP の Undo や ATP の Like 削除の対象）を別の文で読むと、連打や同時操作で実際に上書き・削除した行とずれ、リモートに取り消されない Like が残る。
- `content`: Unicode 絵文字、またはカスタム絵文字の `:shortcode@host:`（Misskey と同じ）。ローカルは `:shortcode@.:`、Fedi 受信は reactor の解決済みドメイン（ワイヤ上のホストは信用しない）。ホストの無い `:shortcode:` はリモートから来た古いデータで、読み出し側でローカル相当と解釈する（ローカルの古いデータはマイグレーションで `@.` に書き換え済み）。AP の `tag[].name` はホスト無しの shortcode（`docs/protocols.md` 2節）。
- `emoji_url`: カスタム絵文字の画像URL（Unicode は NULL）。upsert は `emoji_url` も上書きするので、書き込む3経路（`create_reaction`・AP の `handle_reaction`・ATP の `handle_inbound_like_create`）はカスタム絵文字なら必ず解決してから渡す（`None` を渡すと正しい値を消す）。
- `id`: `posts`/`notifications` と同じ Snowflake（呼び出し側で採番）。`notifications.reaction_id` の重複排除と、プロフィールの投稿＋リアクション混合フィード（`GET /api/users/posts?includeReactions=true`）の時系列マージに使う。切り替え時は `id`/`created_at` も更新する（新しいイベントとして先頭に来る）。

### `remote_emojis`
AP 受信（本文・表示名・リアクション）で見つけたカスタム絵文字を `(shortcode, domain)` ごとに `upsert_seen` するカタログ（`first_seen_at`/`last_seen_at`）。`tags` は Emoji tag の `aliases`/`tags`/`keywords`、`license` は Misskey の `_misskey_license.freeText`。再受信時に空の値で既知の値を消さない。画像は取り込まない（表示はメディアプロキシ経由）。

### `follows`
`status`（`pending`/`accepted`）。
- `upsert_pending` は既存行の状態を変えず（`ON CONFLICT DO UPDATE SET status = follows.status` で行ロックを取りつつ現在値を返す）、`Inserted`/`AlreadyPending`/`AlreadyAccepted` を返す。承認済みのフォローを再操作で `pending` に戻さないため。
- ATP の follow レコードを伴うフォロー（ローカル・Bsky 宛）は、先に `atp_rkey` が NULL の accepted 行を確保し（`insert_accepted_with_rkey(.., None)`、`ON CONFLICT DO NOTHING`）、確保できたリクエストだけが ATP にコミットして `set_atp_rkey` する（`follow_exec::establish_atp_follow`、失敗したら行を消す）。先にコミットすると、連打で follow レコードが2件でき片方の rkey が失われる。
- 部分インデックス: フォロワー取得・AP 配送用の `(target_actor_id, follower_actor_id) WHERE status='accepted'` と、フォロー先取得用のカバリング `(follower_actor_id) INCLUDE (target_actor_id) WHERE status='accepted'`。
- `pending_follow_activity`（JSONB）: 承認制のローカルアクター宛に Fedi から届いた生の Follow。承認/拒否のときに Accept/Reject で `id` 等を参照するため（ローカル間では NULL）。

### `bsky_remote_list_membership_cache`
`list_uri` PK の、リモート Bsky リストの全メンバー DID（`member_dids`、`checked_at` から24時間）。threadgate の `#listRule` 評価（`queries::is_list_member`）専用。無い・期限切れなら `Job::BskyListMembershipResolve` が丸ごと置き換え、その場は制限なしとして応答する。

### `remote_follow_snapshots`
`follows` は seiran が関わる関係しか持たないので、リモート Fedi アクターの followers/following を AP で直接取った結果を `(actor_id, direction)` ごとに丸ごと上書きキャッシュする。`actor_uris`（JSONB）、`complete`（上限に達せず全体を取れたか）。

### フォローインポート（`follow_import_requests` / `follow_import_items`）
- `follow_import_requests`: 実行1回＝1行（`running`/`completed`/`cancelled`）。`UNIQUE (actor_id) WHERE status='running'` で実行中を1本に制限する。
- `follow_import_items`: 対象1件＝1行（`pending`/`succeeded`/`already_following`/`failed`）。既にフォロー済みだったものを `succeeded` と分け、「成功」の件数を新規 `follows` の数と一致させる。
- 進捗の件数はリクエスト側に持たず、items を `COUNT(*) FILTER` で数える（2テーブル間の不整合を起こさない）。キャンセルはリクエストの `status` を変えるだけで、残りの `pending` はそのまま。

### `blocks` / `mutes` / `repost_mutes`
`follows` と同型の有向関係 + UNIQUE。ブロックは相手が Bsky ならコミットの `atp_rkey` を保存し、Fedi なら AP `Block` を送る。ミュート・リポストミュートはローカル効果だけなので `atp_rkey` を持たない。

- 退会時はこの3テーブルの当該アクターが関わる行（両方向）を物理削除する（「退会済みアクターは他者から存在しない」）。一覧クエリ（`list_blocked`/`list_muted`）にも `withdrawn_at IS NULL` を課す（物理削除より前に残った行へのフェイルセーフ）。
- タイムライン・通知の相互非表示は `actor_is_hidden_for_viewer(viewer_id, other_id)`（ブロック・ミュート・退会済み・凍結を OR 判定）に集約する。ブロックはミュート相当の非表示も兼ねる（別途 `mutes` 行を作らない）。`viewer_id` が NULL（未ログイン）ではこの関数を通らない箇所があるので、`list_following`/`list_followers` 等は `withdrawn_at IS NULL` を独立の条件としても課す。
- `repost_mutes` は通常投稿を出したままリポストだけを隠す。リポストの判定は `p.repost_of_post_id IS NOT NULL OR p.repost_of_ap_uri IS NOT NULL`（未解決のリポストは `repost_of_post_id` が NULL なので）。`repost_is_muted_for_viewer(viewer_id, reposter_id)` はホーム/ローカル/ソーシャル/グローバルの4つのタイムラインだけに適用し、プロフィールや個別投稿・スレッドには適用しない。
- 各リポジトリは、タイムラインの関係フラグ付与（`attach_relationship_flags`）用に「1閲覧者×複数ID」の一括判定（`find_statuses_among`/`list_muted_among`/`find_relationships_among`）を持つ。

### 可視性判定の SQL 関数
- `post_is_visible_to(viewer_id, post_actor_id, post_visibility, p_post_id, exclude_direct)`: `followers_only`/`direct` の判定を1か所に集約する。呼び出し側が JOIN 済みの `p.actor_id`/`p.visibility`/`p.id` を渡す（関数内で `posts` を引き直さない、`LANGUAGE sql STABLE`）。第4引数を `p_post_id` にしているのは、`post_id` だと関数内の `post_recipients.post_id` 列が優先解決され、`direct` の `EXISTS` が自己参照で常に真になるため。DM の `thread_messages` もメッセージごとにこれを通す（`is_participant` だけだと、スレッド起点を共有する別のメッセージまで見えてしまう）。`local_timeline`/`global_timeline` は手前で `unlisted`/`followers_only` を除くので、関数の `followers_only` 分岐には到達しない。
- `post_reply_target_followed(viewer_id, p_reply_to_post_id)`: 通常投稿は常に真、リプライは親の投稿者が本人かフォロー中のときだけ真。ホーム/ソーシャルの「フォロー中」部分（ソーシャルのローカル全体部分には適用しない）と、WebSocket のホーム配信（`find_home_recipient_ids`）が共有する。引数名の理由は上と同じ（`posts.reply_to_post_id` との衝突回避）。
- リプライ作成（`resolve_reply_context`）は返信先を `find_by_id_for_viewer` で取り、見えない投稿へのリプライを `REPLY_TARGET_NOT_FOUND` で拒否する（ブロックは `check_not_blocked` が別に見る）。リポスト・引用は `followers_only`/`direct` を一律禁止する。
- `post_is_visible_to`/`post_reply_target_followed`/`repost_is_muted_for_viewer` は `COST 10000` を指定する。既定の100だとプランナが関数の呼び出しコストを過小評価し、ホーム/ソーシャルの LATERAL 内で `idx_posts_actor_id` の Index Scan ではなく Bitmap Heap Scan を選ぶことがある。Bitmap Heap Scan は `LIMIT` を押し下げられないので、フォロイーの投稿をほぼ全件読み直すことになり、フォロー数が多いほど致命的に遅くなる。あわせて `jit = off` にしている（COST を上げるとJIT閾値を超えやすくなり、OLTP では恩恵の薄いコンパイルのオーバーヘッドが乗るため）。

### `instance_domain`
自ホストドメインの確定値。いつでも書き換えられる `site_settings` と分けているのは、ドメインの変更が `actors.domain`・DID 文書・`ap_uri` に深く食い込み整合性を壊すので、「一度確定したら不変」を構造で保証したいから。`id SMALLINT PRIMARY KEY DEFAULT 1 CHECK (id = 1)` で1行だけを許し、`UPDATE` はコードのどこにも書かない。`InstanceDomainRepository::confirm` は `INSERT ... ON CONFLICT (id) DO NOTHING` だけを提供する。実行時は `LocalDomain`（`Arc<OnceLock<String>>`、起動時に1回読む）。

### `remote_instance_meta`
`domain` PK の、連合相手サーバーごとの nodeinfo キャッシュ（`software_name`/`node_name`/`theme_color`/`icon_url`）。書き込み元は `jobs::remote_instance_info_resolve` だけ（`ON CONFLICT DO UPDATE` で全列上書き）。
- `theme_color` は宣言値か、未宣言なら既知の software の固有色、それも無ければ汎用デフォルト（薄いグレー）の最終表示値で、常に非 NULL（フロントや Misskey 互換クライアントはそのまま描けばよい）。
- `node_name` は `metadata.nodeName`、無ければトップページの `<title>`。NULL なら読み出し側（`build_instance_info`）がドメイン名を出す。
- `icon_url` はトップページの `<link rel="icon">`、無ければ実際に取得できた `/favicon.ico`。取れなければ NULL（アイコン無しで表示）。
- Bsky はこのテーブルを使わず `{name: "Bluesky", softwareName: "bluesky"}` を合成する（PDS ごとの nodeinfo が無い）。
- ノート一覧を組み立てるときに未キャッシュのドメインがあれば `RemoteInstanceInfoResolve` を積み、今回はドメイン名を仮の表示名にする。起動時の `spawn_startup_tasks` も、未登録や一部が未取得の行、汎用デフォルト色のまま固有色表に載った software の行をまとめて積む（`docs/protocols.md` 2節）。

### `app_tokens`
MiAuth の認可、または設定画面からの直接発行（`POST /api/account/app-tokens`）で作る JWT はどちらも `generate_app_token` で、専用の形式は無い。このテーブルは JWT の `jti` をキーにクライアント名・発行日時・無効化日時を持つ台帳。`extract_auth` は検証後に `is_revoked(jti)` を照会する。行の無い `jti`（自社ログイン等）は常に有効（全トークンの台帳ではない）。操作は本人のみ。トークン本体は保存しないので、直接発行の応答で一度だけ返す。

### `notifications`
`type`: フォロー・リアクション・メンション・返信・リポスト・引用・`moveRefollowed`/`moveAlreadyFollowing`（seiran 独自）等。リポスト・引用の `note_id` は新しいリポスト/引用投稿。

- `source_uri`（ATP Like の `at_uri`、AP の activity id 等）の部分 UNIQUE で、Jetstream/AP の複線受信による重複を防ぐ。
- `reaction_emoji_url`: 通知時点の絵文字画像URL（リアクションは1人1個なので、後で切り替えると過去の内容が分からなくなるため）。
- `related_actor_id`: `move*` 専用の2つ目のアクター（移転先。`notifier_actor_id` が移転元）。
- `reaction_id`（`reactions.id`、部分 UNIQUE）: 自分のリアクションが firehose で戻ってきたときの重複排除（`docs/protocols.md` 8節）。他人のリアクションでは NULL なので、絵文字の連投は妨げない。

### `actor_also_known_as`
プロフィールの「別のアカウント」。`owner_actor_id` が「`target_actor_id` も自分だ」と申告する片方向の関係（target は解決済みの `actors.id`）。owner はローカルユーザー（本人が API で追加/削除、`resolve_and_upsert_target` で解決）かリモート Fedi アクター（`jobs::also_known_as_sync` が Actor 文書の `alsoKnownAs` を同期）。Bsky アクターは owner にも target にもならない。

`verified`/`last_checked_at` は相手側（ローカル・Fedi のみ）も逆向きに申告しているかの検証結果のキャッシュで、表示時再検証（`AlsoKnownAsVerify`/`RemoteAlsoKnownAsSync`）で更新する（`docs/protocols.md` 2節）。

### `hashtags` / `post_hashtags` / `pinned_hashtags`
ハッシュタグはポストと m:n の永続オブジェクト。`hashtags.name` は正規化済み（先頭 `#` を除き小文字化）で、表示上の大文字小文字は各投稿の本文に任せる。

- 抽出は出自を問わず、最終的な `posts.body` を `hashtag::extract_hashtags` で1回スキャンする（AP 由来の `[#foo](url)` もリンクテキストに `#foo` が残るので拾える、`docs/protocols.md` 6節）。INSERT 直後のベストエフォートで、失敗しても投稿は成立する。
- `pinned_hashtags` は「ホーム画面に追加」の永続化（`pinned_posts` と同じ考え方）。ハッシュタイムラインはピン留めと無関係に誰でも `/tags/:name` で見られる。対象は `visibility IN ('public', 'unlisted')` だけ（発見用の公開フィードなので `followers_only` の例外を設けない）。

### メディア関連（`media_files` / `post_attachments`)
- `media_files`: `width`/`height`/`blurhash` は NULL 可（動画・音声は持たない）。`bsky_video_*` は Bsky 動画パイプラインとの連携状態。`(sha256, blurhash)` の複合 UNIQUE（`NULLS NOT DISTINCT`）で全体の重複を排除する。
  - 登録は seiran UI・uploadBlob・DID 転入のすべてが `MediaFileRepository::upsert`（`INSERT ... ON CONFLICT (sha256, blurhash) DO UPDATE SET last_uploaded_at = now() RETURNING ...`）を使い、確認から挿入までを原子的にする。uploadBlob 由来は `blurhash = NULL`。
  - `last_uploaded_at`: 最後にアップロードされた時刻（重複ヒットも含む）。孤立ファイル GC の生存判定はこちらを見る。`created_at` だと、古い孤児が再アップロードで再利用された直後に GC が消してしまう。
  - `is_animated_image`: アニメーション画像由来か（`ImagePipeline::AnimatedPassthrough` のときだけ `TRUE`）。Bsky embed 選択で静止画とアニメGIFを分けるのに使う。
- `post_attachments`: `media_file_id`（ローカル）と `remote_url`/`remote_mime_type`/`remote_thumbnail_url`（リモート）のどちらかが埋まる。
  - `is_sensitive`: AP の `attachment[].sensitive`。投稿全体の `sensitive=true` は全添付に伝播させる。
  - `is_gif`: GIF アニメ由来（GIF ピッカー、または `presentation:"gif"`）。フロントは自動再生・ミュート・ループ・コントロール無しで表示する。
- **孤立ファイル GC**（`run_media_gc`、7日周期）: `post_attachments`/`actors`/`custom_emojis` のどこからも参照されない行を消す。参照の有無は `NOT EXISTS` で判定する（`post_attachments.media_file_id` はリモート添付で NULL の行が大半で、`NOT IN` だと結果が常に UNKNOWN になり1件も消せない）。削除は、その瞬間に孤立条件を再評価する単一の `DELETE ... RETURNING` で DB を先に確定させてから S3 を消す（DB → S3）。これで「行があれば実体もある」を保ち、途中で参照が増えるレースにも安全（削除0件になるだけ）。S3 の削除に失敗するとゴミが残るが実害は小さい。

### `post_link_cards`（URLカード）
投稿ごとに0件以上のカードを `(post_id, position)`（UNIQUE）で持つ。
- Bsky は `embed.external`（GIF ピッカー由来を除く）から `position=0` の最大1件。ローカル投稿はチェックボックス選択（`link_card_urls`）で複数、Fedi は本文の複数リンクで複数になりうる。
- `title`/`description` は空文字列可、`thumbnail_url` だけ NULL 可。
- `embed_src`/`embed_type`: oEmbed で解決した iframe src と `type`。許可ドメイン（`site_settings.oembed_allowed_domains`）で判定済みの値だけが入る。Fedi/ローカルは `Job::OgpFetch` が INSERT 時に埋め、Bsky は `external` に iframe 情報が無いので `Job::LinkCardEmbedResolve` が後から UPDATE する。
- 取得は `fetch_link_cards_map`（`post_id` 一覧 → `HashMap<i64, Vec<LinkCardResponse>>`）で一括して `NoteResponse.link_cards` に入れる。

### ATP リポジトリ関連（`atp_records` / `atp_blocks` / `atp_repo_events`)
`app.bsky.feed.post` は `posts` で管理し、`atp_records` にはそれ以外のコレクション（`app.bsky.actor.profile` 等）だけを持つ。`atp_blocks` は CAR ブロックの実体。`atp_repo_events` は Relay に配る `subscribeRepos` フレームのログで、`id`（BIGSERIAL）がそのままカーソル（seq）になる。`frame_bytes` にコミット時のフレームをそのまま保存し、再送時に作り直さない（バイト列の差で Relay が切断するのを避ける）。

### ATP セッション認証関連（`atp_app_passwords` / `atp_refresh_tokens`）
- `atp_app_passwords`: `createAppPassword` のアプリパスワードの argon2 ハッシュと `revoked_at`。
- `atp_refresh_tokens`: refreshJwt の `jti` の `expires_at`/`revoked_at`。更新のたびに古い `jti` を失効させる（ワンタイム）。
- JWT 自体は保存しない。メインパスワードかアプリパスワードかは JWT の `privileged` クレームに持つ（`docs/protocols.md` 3節）。

### `atp_preferences`
`getPreferences`/`putPreferences` の不透明な配列（JSONB）を `actor_id` ごとに1行（全置換）。中身は解釈せず、`users` とも同期しない。

### 既存DID転入（`at_migration_requests` / `at_migration_records` / `at_migration_blobs`）
フォローインポートと同じ親子テーブル＋進捗は COUNT の設計。状態遷移の詳細は `docs/account_migration.md`。

- `at_migration_requests`: 転入1回＝1行。`status`（ENUM `at_migration_status`）が状態機械で、`awaiting_source_2fa` → `fetching_repo` → `requesting_plc_signature` → `awaiting_plc_token` → `submitting_plc` → `importing_data` → `deactivating_source` → `completed`。ほかに `failed`（`submitPlcOperation` 前のみ。リトライ・別DID・新規DID切替が可）、`failed_post_submit`（成功後専用、リトライのみ）、`abandoned`（`plc_submitted_at IS NULL` の間だけ選べる打ち切り）。`awaiting_seiran_email` は ENUM に残るが到達しない。
  - `request_token_hash`: 匿名段階の認可トークンの SHA-256（生値は1回だけ返す）。
  - `plc_submitted_at`: 不可逆境界のマーカー。`new_signing_key_pem`（転入後の repo 署名鍵）と `new_rotation_key_pem`（専用ローテーションキー）はこの成功と同時に保存する（後のアカウント作成が失敗しても鍵を失わないよう、`actor_id`/`user_id` の確定とは別ステップ）。
- `at_migration_records`: 取得した生レコード1件＝1行（`request_id, collection, rkey` の UNIQUE）。`bytes` は DAG-CBOR のまま。`imported_at` は `posts`/`atp_records` への実体化の完了。`app.bsky.graph.follow` だけは `follow_materialized_at` で `follows` への反映を別に追跡する（ATP リポジトリへの複製と seiran の社会グラフへの反映は別の処理だから）。
- `at_migration_blobs`: `listBlobs` の CID（`request_id, cid` の UNIQUE）。`imported_at` は `media_files` への保存の完了。

### メール短命コード（`email_short_codes`）
ATP セッションのメール2FA（`purpose='atp_session_2fa'`）と PLC 操作署名の確認（`purpose='plc_operation_signature'`）が共有する6桁コード。リンク型（`email_verifications` 等）と違いユーザーが手入力する値なので、コードをハッシュ化して保存する。消費時は一致・不一致を問わず同じ `actor_id`+`purpose` の行を全部消す（古いコードの再利用防止）。

### アンケート（`poll_votes`・`posts.poll`）
- `posts.poll`: `{multiple, options:[{name,votes}], endTime}`。AP の `Question`（`oneOf`/`anyOf`・票数・締切）もローカル作成も同じ形。CW は `posts.content_warning`（AP の `summary`）。
- `poll_votes`: 投稿・回答者・選択肢番号ごとの回答。複数選択では同じ回答者が複数行を持つ。`ap_activity_id` の UNIQUE でリモートの再配送を冪等にする（ローカルの投票は NULL）。
- 票数の加算は、ローカル・リモートとも `repository::poll::increment_poll_votes`（`jsonb_set` の単一 UPDATE、`poll_votes` への記録と同じトランザクション）で行う。読んで+1して書き戻すと同時投票で加算が失われる。ローカル投票（`vote_poll`）は投稿行を `FOR UPDATE` でロックしてから投票済み判定・記録・加算をする。
- 認証付きの読み取りは `poll_votes` から自分の選択を `poll.votedByMe` として付ける。
- **リモートアンケートの生存監視**: `posts.poll` は取り込み時のスナップショットなので、2列で追従する（処理は `docs/protocols.md` 3節）。
  - `poll_update_received`: `Update(Question)` を一度でも受理したか。真なら push 型の実装とみなし、再取得の対象から外す。
  - `poll_fetched_at`: poll を最後に取得・反映した時刻。取り込み時（`set_fedi_content_metadata`）は `created_at` で初期化する。
  - 再取得の判定は `find_stale_remote_poll_post_ids` に `(post_id, しきい値)` を渡して行う。締切前は10分周期、締切後は締切以降に一度も取得していないものだけ。

### 通報（`reports` / `report_comments`）
- `reports`: ローカル・Fedi・Bsky 共通の台帳。通報者・対象 Actor・任意の対象 Post・理由分類・自由記述・`destination`/`remote_host`（対象がリモートかをサーバーが算出）・処理状態を持つ。通報者は送信先を選ばず、常にローカル管理者へ届き、`destination='remote'` だけ管理者が任意に転送できる。
  - 理由分類は8カテゴリ → 計39種の2段階で、`reason_type` に Ozone のトークン名（例: `reasonMisleadingSpam`）を保存する（カテゴリはトークンから導出できるので持たない）。
  - `subject_post_id` は `ON DELETE SET NULL`（投稿が消えても調査履歴を残す）。
  - 自由記述は DB 制約でも300文字かつ1000バイト以下。
  - `forwarded_at`: 転送成功時刻（再送の判断用）。
  - 物理削除せず、`status`/`closed_at` でオープン/クローズを管理する。
- `report_comments`: 管理者・モデレーターだけの内部メモ（通報の削除で CASCADE）。

### Fediverseリレー（`fediverse_relays`）
`inbox_url`（UNIQUE）ごとの参加状態（`pending`/`accepted`/`rejected`）。`follow_activity_id` で Accept/Reject を照合し、離脱時の Undo にも使う。公開投稿の配送先は `accepted` だけ。

## 4. 典型的なクエリパターン

- **`TimelinePost` の共通部品**: SELECT 列は `timeline_post_columns!()`、結合は `timeline_post_joins!()`（`repository/post.rs`）を `concat!` で埋め込む。`#[sqlx(default)]` は `actor_suspended_at`（参照埋め込みと単体取得でだけ後置する列）以外に付けないので、列の書き漏れは実行時エラーになる。参照の `pending`/`gone` の6列は、単体取得（`find_by_id`/`find_by_id_for_viewer`）でだけ取得し、タイムラインでは `None`。
- **可視性**: 閲覧者ごとの可視性は必ず `post_is_visible_to` で判定する。引用・リポスト・返信先の埋め込み（frontend API の `embed_renotes`/`embed_quotes`、Misskey 互換の `fetch_referenced_notes`）は `find_visible_posts_by_ids` を使う。
- **サブクエリ**: `IN (SELECT ...)`/`NOT IN (SELECT ...)` は使わず `EXISTS`/`NOT EXISTS`（`docs/coding_rules.md` 2節、`tests/sql_style.rs` が検出する）。
- **ホーム/ローカル**: `posts` を `id` 降順でページングするだけ。フォロー時に相手の過去ログを取り込んでいるので外部 API を呼ばない（`docs/concept.md`「タイムラインは自前の池」）。
- **ソーシャル/グローバル**: `social_timeline` は自分＋フォロー中＋ローカル全体（`home_timeline` の LATERAL 候補と `local_timeline` の `is_local` 候補を UNION して外側で LIMIT）、`global_timeline` は `local_timeline` から `is_local` 条件を外したもの。同じインデックス（`idx_posts_actor_id`・`is_local`）で足りる。`unlisted`・`followers_only` は、本人やフォロワーが見てもローカル/グローバルには出さず、ホームとソーシャルにだけ出す。リプライ先フォロー条件（`post_reply_target_followed`）はフォロー中の候補にだけ課し、ローカル全体の候補には課さない（ローカルの全投稿を出す設計）。
- **引用関係**: `quote_of_post_id` はローカル作成に加え、AP の `quoteUrl`/`_misskey_quote`/Misskey Hub 互換の Link tag、Bsky の `embed.record`/`recordWithMedia` の受信時にも、保存済み投稿を引いて設定する。
- **参照の pending/gone**: `*_post_id` の NULL は「参照なし」と「未解決」の両方がありうるので、`{reply_to,quote_of,repost_of}_ap_uri` と `_ref_status` を対で持つ（`*_post_id` が非 NULL なら見ない）。`pending` は取得未完了/一時失敗（再取得の余地あり）、`gone` は 404/410 で確定。`resolve_reference` が DB 照合 → 1段だけ取得 → 成功/404・410/その他の3分岐で設定する（取得したノート自身の参照は `resolve_reference_db_only` で DB 照合だけ）。`pending` は `resolve_pending_reference_with_timeout` で再解決する（`docs/protocols.md` 1節）。
- **検索**: 本文検索は pg_bigm（`idx_posts_body_bigm`）。pg_bigm は `LIKE` だけを最適化し `ILIKE` は対象外なので、`LOWER(body) LIKE LOWER(pattern)` とし、インデックスも `LOWER(body)` に張る。アクター検索は用途別: `GET /api/actors/search`（リスト・DM）は表示名と全ハンドル表記を改行で連結した式への部分一致（`idx_actors_search_bigm`、pg_bigm GIN の式インデックス）、`GET /api/actors/suggest`（投稿欄）はハンドルの前方一致だけで、`idx_actors_handle_prefix` と `idx_actors_local_bsky_handle_prefix`（`text_pattern_ops` の B-tree 式インデックス）を個別に走査して UNION する。環境依存のドメインをマイグレーションに焼き込まないよう、式中のローカル判定は `actor_type = 'local'` を使う。

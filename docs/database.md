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
| `email_verifications` / `email_changes` / `password_resets` | 認証系のワンタイムトークン |
| `app_tokens` | MiAuth経由で発行されたアプリトークンの一覧・無効化管理 |

## 3. 主要テーブルの設計判断

### `users` / `actors` の分離
「魂（`users`、当サーバーの住民としての認証アカウント）」と「肉体（`actors`、各プロトコル宇宙での登場人物）」を分離している（`docs/concept.md` 参照）。1つの `users` 行に対し、ローカルユーザーなら基本的に1つの `actors` 行（AP/ATP 両方の識別子を1行に持つ）が対応する。`actors.user_id` は `users` への参照で、ローカルユーザー以外は NULL。

`actors.actor_type`（ENUM `actor_type_enum`）は6種:
`local` / `remote_seiran` / `fedi` / `bsky` / `fedi_bridge_to_bsky` / `bsky_bridge_to_fedi`

- `seiran_pair_actor_id`: 他 seiran サーバーユーザーの「同じ魂を持つ AP/ATP 両アクター」を相互リンクするための自己参照。**現状、これを書き込む処理は実装されていない**（ゼロトラストハンドシェイクが未実装のため常に NULL。`docs/roadmap.md` 参照）。
- `bridge_real_actor_id`: ブリッジ経由の影武者アクターから本尊アクターへのリンク。

ローカルアクターは `avatar_media_id`/`banner_media_id`（自前 `media_files` 参照）、リモートアクターは `avatar_url`/`banner_url`（URL直持ち）という排他的な使い分けをしている。

`users.language_preference`（設定画面「表示」＞「言語」）: `ja` / `en` のいずれか、`NULL` は「自動」（ブラウザの言語設定に従う）を意味する。

### `posts` の設計
統一ポストID（`id`）はタイムスタンプ内包の Snowflake で、`sinceId`/`untilId` ページネーションの主軸になる。

- `deleted_at`: 物理削除ではなく論理削除（Tombstone）。ATP は MST 上の署名付き履歴を壊せないため。`atp_tombstone_cid` に削除証明の CID を保持する。
- `metadata`（JSONB）: プロトコル別の変形レシピなど拡張情報の汎用格納庫。
- `emoji_map`（JSONB）: 本文中のカスタム絵文字 `:shortcode:` → 画像URL のマップ（Fedi 受信時に解決して保存。表示側で都度解決しない静的スナップショット）。
- `mention_facets`（JSONB、デフォルト `[]`）: Bsky投稿のメンションfacet位置情報 `[{"byteStart":N,"byteEnd":M,"did":"did:plc:..."}]`。`emoji_map`とは対照的に、DIDのハンドル解決は保存時ではなく表示時（`NoteResponse`生成時）に都度行う（Bskyハンドルは可変なため。`docs/protocols.md` 6節参照）。ローカル投稿・Fedi受信は常に空配列。
- `is_local`（非正規化 + トリガー）: ローカルタイムライン取得がリモート投稿優勢な環境で劣化する問題への対策。`BEFORE INSERT` トリガー `trg_posts_set_is_local` が `actors.actor_type` から自動導出するため、書き込み漏れの心配がない。
- 重複排除・マージに使うカラム: `seiran_post_uuid`（他 seiran サーバー間マージのキー。**ATP側レコードには埋め込まれていないため、Bsky経由で先に取り込まれた投稿は AP 側の同一投稿と現状マージされない** — 既知の制約）、`parent_original_post_id`（ループバック・一般ブリッジ重複のハードリンク）。
- `visibility`（ENUM `post_visibility_enum`: `public`/`unlisted`/`followers_only`/`direct`）と `deliver_fedi`/`deliver_bsky`（配信先トグル）は独立した軸。リプライは親の可視性を継承する。
- `thread_root_post_id`: `direct`投稿（DM）のスレッド起点ポストID。DM関連テーブルの節を参照。`direct`以外の投稿では常にNULL。

### ダイレクトメッセージ関連（`post_recipients` / `dm_read_states` / `bsky_convo_links`)
DMは`visibility='direct'`の投稿をそのまま`posts`に格納する方式で実現し、Misskey APIクライアントからも読み書きできるようにしている（フロントエンドはタイムライン取得時に`direct`を除外するパラメータを付与することで、Misskey本家の`specified`投稿がタイムラインに現れうる挙動と両立させている）。

- `post_recipients`: `direct`投稿の宛先アクター一覧（`post_id`/`actor_id`のUNIQUE）。Bsky宛先が絡む場合は1対1のみ許可というアプリ側バリデーション（DB制約では表現しない）が別途かかる。
- `thread_root_post_id`（`posts`本体のカラム）: 「スレッド起点ポストを同じくするdirect投稿の集合」をメッセージセッションの単位とするための識別子。通常ポストへの返信として最初のdirect投稿が付いた場合、その最初のdirect投稿自身が起点になる。新規insert時は都度再帰クエリで遡らず、親（`reply_to_post_id`）の`thread_root_post_id`をそのままコピーする伝播コピー方式（親がdirectでない/存在しなければ自分自身のIDを設定）。中央ペインのメッセージ履歴はこの値で束ねて`id`昇順（時刻順）に並べ、ツリー表示はしない。
- `dm_read_states`: `(actor_id, thread_root_post_id)`をPKに持つスレッド別の最終既読ポストID。未読バッジは「未読のあるセッション数」で算出する。
- `bsky_convo_links`: DMスレッド起点とBsky `chat.bsky.convo`のconvoIdの対応キャッシュ（`getConvoForMembers`呼び出し回数を減らすため）。Bsky宛先が絡むスレッドのみ行を持つ。`last_synced_message_id`はBsky DM受信ポーリング（`chat.bsky.convo.getMessages`）が直近まで取り込み済みのBsky側メッセージIDを保持するカーソル。
- `posts.bsky_message_id`（`posts`本体のカラム、Bsky受信DMのみ設定）: Bsky側メッセージIDを保持し部分UNIQUEインデックスを張ることで、DM受信ポーリングの再実行（DB瞬断等での中断からの再開）によるメッセージの重複取り込みを防ぐ冪等キーとして使う。

### `reactions`
`UNIQUE(post_id, actor_id)` — 1投稿につき1ユーザー1リアクション（Misskey 準拠）。切り替え時は `ON CONFLICT DO UPDATE`。`content` は Unicode 絵文字文字列、またはカスタム絵文字の場合 `:shortcode:` 形式。`emoji_url` はカスタム絵文字の画像URL（ローカル送信は `custom_emojis` から解決、Fedi 受信は activity の `tag` から解決、ATP 自己firehose再受信も `custom_emojis` から再解決、Unicode 絵文字は NULL）。`ON CONFLICT DO UPDATE` は `emoji_url` も無条件で上書きするため、insert元となる3経路（`create_reaction`／AP受信の`handle_reaction`／ATP受信の`handle_inbound_like_create`）は全て、`content` が `:shortcode:` 形式なら emoji_url を解決してから渡す必要がある（未解決のまま `None` を渡すと既存の正しい値を消してしまう）。`id`（`GENERATED ALWAYS AS IDENTITY`）は集計用途ではなく、`notifications.reaction_id`（リアクション通知の重複排除トークン、下記参照）としても使う。

### `follows`
`status`（`pending`/`accepted`）を持つ。パフォーマンス上重要な2つの部分インデックスがある: フォロワー取得・AP配送方向の `(target_actor_id, follower_actor_id) WHERE status='accepted'` と、自分のフォロー先取得用のカバリングインデックス `(follower_actor_id) INCLUDE (target_actor_id) WHERE status='accepted'`。

### `remote_follow_snapshots`
`follows` はseiranが認知している関係（ローカルアクターが片方に絡む場合のみ）しか持たない。本テーブルはそれとは独立に、リモートFediアクターのfollowers/following OrderedCollectionをAP経由で直接取得した結果を、`actor_id`×`direction`（`following`/`followers`）単位で丸ごと上書きキャッシュする（`UNIQUE(actor_id, direction)`）。`actor_uris` は取得できたactor URIのJSONB配列、`complete` は上限件数に達せずコレクション全体を取得しきれたか。`docs/protocols.md` 2節参照。

### `blocks` / `mutes`
`follows` と同型（`blocker_actor_id`/`blocked_actor_id`、`muter_actor_id`/`muted_actor_id` の有向関係 + `UNIQUE`制約）。ブロックはBsky準拠の定義（フォロー関係の強制解除＋相互完全非表示）を採用しており、相手がBskyなら `app.bsky.graph.block` コミット後の `atp_rkey` を保存、相手がFediならAP `Block` を配送する（`docs/protocols.md` 参照）。ミュートはFedi/Bsky共通でローカル効果のみ（AP/ATP配送なし）のため `atp_rkey` 相当のカラムを持たない。

タイムライン・通知の相互非表示は、両テーブルを1箇所でOR判定する SQL 関数 `actor_is_hidden_for_viewer(viewer_id, other_id)` に集約している。ブロックは `blocks` テーブルの存在だけでミュート相当のローカル非表示も兼ねる設計（ブロック専用の `mutes` 行を別途作らない）。

### `app_tokens`
MiAuth（`/api/miauth/:session_id/authorize`）認可成立時に発行するJWTは自社ログインと同じ`LocalAuthProvider`を再利用しており、専用のトークン形式を持たない。本テーブルはそのJWTの`jti`（クレームに追加済み）をキーに、クライアント名・発行日時・無効化日時を記録する管理台帳で、JWT自体の検証ロジックには関与しない。認証ミドルウェア（`extract_auth`）はトークン検証成功後に必ず`app_tokens.is_revoked(jti)`を照会し、`revoked_at`が立っていれば拒否する。**このテーブルに行が無いjti（自社ログイン・setup等）は「管理対象外」として常に有効**として扱う（全トークンを網羅する台帳ではない）。設定画面の一覧・無効化操作は本人（`user_id`一致）のみ可能。

### `notifications`
`source_uri`（発生源イベントの一意識別子、ATP Like の `at_uri` や AP の `ap_activity_id`）に部分 UNIQUE インデックスを張り、Jetstream/AP の複線受信による重複 INSERT を防いでいる。`reaction_emoji_url` は通知発生時点の絵文字画像URLをスナップショット保存する非正規化カラム（`reactions` が1人1リアクションのため、後から絵文字を切り替えると過去のリアクション内容を復元できなくなる問題への対処）。

`reaction_id`（`reactions.id` を保存、部分 UNIQUE インデックス）は `source_uri` とは別目的の重複排除トークン。ローカルユーザーが ATP 実体を持つ投稿へリアクションすると「ローカル即時通知」と「その ATP コミットが自分自身の firehose 経由で戻ってきた再受信通知」の2経路が走ってしまうため、両方に同じ `reactions.id` を持たせて UNIQUE 制約で片方を弾く（`docs/protocols.md` 8節）。他人発のリアクションでは常に `NULL` になり、同じ投稿への複数回の連続リアクション（絵文字を変えて通知欄に文章を書く等）を妨げない設計を維持している。

### `hashtags` / `post_hashtags` / `pinned_hashtags`
ハッシュタグは検索結果の即席表示ではなく、ポストとm:nの関係を持つ永続化オブジェクトとして扱う。`hashtags.name` は正規化済み（先頭`#`除去・小文字化、グルーピング用の内部表現）。表示上の大文字小文字は各投稿の `posts.body` 原文に委ねる（`hashtags` テーブル自体は表示用の値を持たない）。

抽出はプロトコル別の特別処理を持たず、ローカル投稿・AP受信・Bsky受信いずれも「最終的な `posts.body` テキストを1回スキャンする」共通経路（`seiran_common::hashtag::extract_hashtags`）で行う。AP由来のハッシュタグアンカーは `[#foo](リモートのタグページURL)` というMarkdownリンクに変換されるが、リンクテキスト部分に `#foo` がそのまま残るため、この共通スキャンだけで3ソースとも取りこぼしなく抽出できる（`docs/protocols.md` 6節参照）。抽出・リンクは投稿INSERT直後のベストエフォート処理で、失敗しても投稿自体は成立させる。

`pinned_hashtags` は「ホーム画面に追加」操作の永続化（`pinned_posts` と同じ設計思想）。ハッシュタイムライン自体は `post_hashtags` を介した検索であり、ピン留めの有無に関係なく誰でも `/tags/:name` で閲覧できる。ハッシュタイムラインは `visibility IN ('public', 'unlisted')` のみを対象にする（特定アクター向けの閲覧制御が要るフィードではなく発見用の公開フィードのため、`followers_only` の例外は設けない）。

### メディア関連（`media_files` / `post_attachments` / `atp_blobs`)
`media_files` は画像専用として始まったため `width`/`height`/`blurhash` は NULL 許容(動画・音声はこれらを持たない)。`bsky_video_*` 系カラムは Bluesky 公式動画パイプライン（`app.bsky.video.uploadVideo`）との連携状態を追跡する。`(sha256, blurhash)` の複合 UNIQUE でグローバル重複排除。

`post_attachments` は `media_file_id`（ローカル添付）と `remote_url`/`remote_mime_type`/`remote_thumbnail_url`（リモート受信添付）が排他的に埋まる設計。

`atp_blobs` は `uploadBlob` で受信した任意バイナリ（Bsky動画パイプラインが提出してくるトランスコード済み動画等）を保存する。`sha256` に UNIQUE を張り、content-addressable な重複排除を行う。

### ATP リポジトリ関連（`atp_records` / `atp_blocks` / `atp_repo_events`)
seiran は自前 PDS としてローカルユーザーの ATP リポジトリ（MST）を管理する。`app.bsky.feed.post` は `posts` テーブルで一元管理し、`atp_records` にはそれ以外のコレクション（`app.bsky.actor.profile` 等）だけを持つ。`atp_blocks` は CAR ブロックの実体、`atp_repo_events` は Relay へブロードキャストする `subscribeRepos` フレームのログで、`id`（BIGSERIAL）がそのまま Relay カーソル(seq)になる。`frame_bytes` にコミット時点で生成したフレームのバイト列をそのまま保存しており、再送時に再構築しない（バイト列差異による Relay 切断を避けるため）。

## 4. 典型的なクエリパターン

- **ホーム/ローカルタイムライン**: `posts` を `id`（降順）でページネーションするだけの単純な SQL。フォロー時点で相手の過去ログを丸ごと自サーバー DB に取り込んでいるため、外部 API 呼び出しを伴わない（`docs/concept.md` 「タイムラインは自前の池」参照）。
- **検索**: ローカル DB 全文検索（`idx_posts_body_trgm`, pg_trgm）と AppView 検索の結果をマージする。セッション管理の詳細は `docs/architecture.md` の検索セッション節を参照。

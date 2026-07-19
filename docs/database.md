# データベース設計

対象読者: このプロジェクトで DB スキーマに触れる開発者（未来の自分自身も含む）。
正確な DDL は `crates/seiran-common/migrations/` が正であり、ここには書かない。
ここに書くのは「なぜこの形にしたか」という設計判断と、テーブル間の関係。

マイグレーションは `cargo sqlx migrate run` で適用する（`psql -f` で直接流してはいけない。理由は `/home/yuba/seiran/CLAUDE.md` 参照）。

## 1. 全体設計思想

seiran の DB は「ローカル・ActivityPub(Fedi)・AT Protocol(Bsky) という3つの宇宙のアクター・投稿・フォロー関係を1つのテーブルに統一して格納する」ことを核とする。`actors` / `posts` / `follows` / `lists` / `list_members` はいずれもこのパターンで、プロトコル固有の識別子（`ap_uri` / `ap_object_id` / `at_did` / `at_uri` / `at_rkey` 等）を NULL 許容カラムとして併存させている。「ローカル用テーブル」「Fedi用テーブル」のように分けていない。

ID 採番は2系統ある。
- **アプリ側 Snowflake 採番**（`generate_snowflake_id()`、タイムスタンプ内包の BIGINT）: `actors` / `posts` / `media_files` / `custom_emojis` / `notifications` / `lists` / `email_verifications` / `password_resets` / `atp_blobs`。`posts.id` はタイムライン表示順のソート主軸そのものであり、これが `docs/concept.md` の「統一ポストID」にあたる。
- **DB 側 `GENERATED ALWAYS AS IDENTITY`**: `users` / `reactions` / `follows` / `storage_providers` / `list_members` / `pinned_posts`。順序に意味を持たせる必要がない補助テーブル。

## 2. テーブル一覧

| テーブル | 役割 |
|---|---|
| `users` | ローカルアカウント（メール/パスワード認証、ロール、凍結状態） |
| `actors` | ローカル/リモート(Fedi・Bsky・ブリッジ)を統一するアクター（公開プロフィール実体） |
| `posts` | 投稿・リプライ・リポスト・引用を統一するポストテーブル |
| `reactions` | 投稿への絵文字/いいねリアクション |
| `follows` | フォロー関係（リクエスト中/成立） |
| `notifications` | 永続化された通知 |
| `media_files` | アップロード/受信済みメディア実体 |
| `post_attachments` | 投稿とメディアの中間テーブル |
| `custom_emojis` | ローカルのカスタム絵文字定義 |
| `storage_providers` | メディア保存先オブジェクトストレージ(S3互換)の設定 |
| `lists` / `list_members` | ユーザーごとのリスト（Bsky `app.bsky.graph.list` 相当） |
| `pinned_posts` | プロフィールへのピン留め投稿 |
| `atp_records` | ATP の非 post レコード（`app.bsky.actor.profile` 等）の管理 |
| `atp_blocks` | ATP MST の CAR ブロックストア（CID → バイト列） |
| `atp_repo_events` | ATP `subscribeRepos` 配信用のイベントログ（commit/identity） |
| `atp_blobs` | ATP `uploadBlob` で受信したバイナリ |
| `site_settings` | サイト全体の Key-Value 設定（SMTP 設定、Jetstream カーソル等の汎用格納庫） |
| `email_verifications` / `password_resets` | 認証系のワンタイムトークン |

## 3. 主要テーブルの設計判断

### `users` / `actors` の分離
「魂（`users`、当サーバーの住民としての認証アカウント）」と「肉体（`actors`、各プロトコル宇宙での登場人物）」を分離している（`docs/concept.md` 参照）。1つの `users` 行に対し、ローカルユーザーなら基本的に1つの `actors` 行（AP/ATP 両方の識別子を1行に持つ）が対応する。`actors.user_id` は `users` への参照で、ローカルユーザー以外は NULL。

`actors.actor_type`（ENUM `actor_type_enum`）は6種:
`local` / `remote_seiran` / `fedi` / `bsky` / `fedi_bridge_to_bsky` / `bsky_bridge_to_fedi`

- `seiran_pair_actor_id`: 他 seiran サーバーユーザーの「同じ魂を持つ AP/ATP 両アクター」を相互リンクするための自己参照。**現状、これを書き込む処理は実装されていない**（ゼロトラストハンドシェイクが未実装のため常に NULL。`docs/roadmap.md` 参照）。
- `bridge_real_actor_id`: ブリッジ経由の影武者アクターから本尊アクターへのリンク。

ローカルアクターは `avatar_media_id`/`banner_media_id`（自前 `media_files` 参照）、リモートアクターは `avatar_url`/`banner_url`（URL直持ち）という排他的な使い分けをしている。

### `posts` の設計
統一ポストID（`id`）はタイムスタンプ内包の Snowflake で、`sinceId`/`untilId` ページネーションの主軸になる。

- `deleted_at`: 物理削除ではなく論理削除（Tombstone）。ATP は MST 上の署名付き履歴を壊せないため。`atp_tombstone_cid` に削除証明の CID を保持する。
- `metadata`（JSONB）: プロトコル別の変形レシピなど拡張情報の汎用格納庫。
- `emoji_map`（JSONB）: 本文中のカスタム絵文字 `:shortcode:` → 画像URL のマップ（Fedi 受信時に解決して保存。表示側で都度解決しない静的スナップショット）。
- `mention_facets`（JSONB、デフォルト `[]`）: Bsky投稿のメンションfacet位置情報 `[{"byteStart":N,"byteEnd":M,"did":"did:plc:..."}]`。`emoji_map`とは対照的に、DIDのハンドル解決は保存時ではなく表示時（`NoteResponse`生成時）に都度行う（Bskyハンドルは可変なため。`docs/protocols.md` 6節参照）。ローカル投稿・Fedi受信は常に空配列。
- `is_local`（非正規化 + トリガー）: ローカルタイムライン取得がリモート投稿優勢な環境で劣化する問題への対策。`BEFORE INSERT` トリガー `trg_posts_set_is_local` が `actors.actor_type` から自動導出するため、書き込み漏れの心配がない。
- 重複排除・マージに使うカラム: `seiran_post_uuid`（他 seiran サーバー間マージのキー。**ATP側レコードには埋め込まれていないため、Bsky経由で先に取り込まれた投稿は AP 側の同一投稿と現状マージされない** — 既知の制約）、`parent_original_post_id`（ループバック・一般ブリッジ重複のハードリンク）。
- `visibility`（ENUM `post_visibility_enum`: `public`/`unlisted`/`followers_only`/`direct`）と `deliver_fedi`/`deliver_bsky`（配信先トグル）は独立した軸。リプライは親の可視性を継承する。

### `reactions`
`UNIQUE(post_id, actor_id)` — 1投稿につき1ユーザー1リアクション（Misskey 準拠）。切り替え時は `ON CONFLICT DO UPDATE`。`emoji_url` は Fedi 受信のカスタム絵文字画像URL（Unicode 絵文字/Like 由来は NULL）。

### `follows`
`status`（`pending`/`accepted`）を持つ。パフォーマンス上重要な2つの部分インデックスがある: フォロワー取得・AP配送方向の `(target_actor_id, follower_actor_id) WHERE status='accepted'` と、自分のフォロー先取得用のカバリングインデックス `(follower_actor_id) INCLUDE (target_actor_id) WHERE status='accepted'`。

### `notifications`
`source_uri`（発生源イベントの一意識別子、ATP Like の `at_uri` や AP の `ap_activity_id`）に部分 UNIQUE インデックスを張り、Jetstream/AP の複線受信による重複 INSERT を防いでいる。`reaction_emoji_url` は通知発生時点の絵文字画像URLをスナップショット保存する非正規化カラム（`reactions` が1人1リアクションのため、後から絵文字を切り替えると過去のリアクション内容を復元できなくなる問題への対処）。

### メディア関連（`media_files` / `post_attachments` / `atp_blobs`)
`media_files` は画像専用として始まったため `width`/`height`/`blurhash` は NULL 許容(動画・音声はこれらを持たない)。`bsky_video_*` 系カラムは Bluesky 公式動画パイプライン（`app.bsky.video.uploadVideo`）との連携状態を追跡する。`(sha256, blurhash)` の複合 UNIQUE でグローバル重複排除。

`post_attachments` は `media_file_id`（ローカル添付）と `remote_url`/`remote_mime_type`/`remote_thumbnail_url`（リモート受信添付）が排他的に埋まる設計。

`atp_blobs` は `uploadBlob` で受信した任意バイナリ（Bsky動画パイプラインが提出してくるトランスコード済み動画等）を保存する。`sha256` に UNIQUE を張り、content-addressable な重複排除を行う。

### ATP リポジトリ関連（`atp_records` / `atp_blocks` / `atp_repo_events`)
seiran は自前 PDS としてローカルユーザーの ATP リポジトリ（MST）を管理する。`app.bsky.feed.post` は `posts` テーブルで一元管理し、`atp_records` にはそれ以外のコレクション（`app.bsky.actor.profile` 等）だけを持つ。`atp_blocks` は CAR ブロックの実体、`atp_repo_events` は Relay へブロードキャストする `subscribeRepos` フレームのログで、`id`（BIGSERIAL）がそのまま Relay カーソル(seq)になる。`frame_bytes` にコミット時点で生成したフレームのバイト列をそのまま保存しており、再送時に再構築しない（バイト列差異による Relay 切断を避けるため）。

## 4. 典型的なクエリパターン

- **ホーム/ローカルタイムライン**: `posts` を `id`（降順）でページネーションするだけの単純な SQL。フォロー時点で相手の過去ログを丸ごと自サーバー DB に取り込んでいるため、外部 API 呼び出しを伴わない（`docs/concept.md` 「タイムラインは自前の池」参照）。
- **検索**: ローカル DB 全文検索（`idx_posts_body_trgm`, pg_trgm）と AppView 検索の結果をマージする。セッション管理の詳細は `docs/architecture.md` の検索セッション節を参照。

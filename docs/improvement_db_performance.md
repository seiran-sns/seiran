# DB パフォーマンス診断レポート

> 診断日: 2026-07-02  
> 対象ブランチ: main  
> 診断者: Claude Sonnet 4.6（SQL 専門家モード）

---

## 1. 診断サマリー

| 重大度 | 件数 |
|--------|------|
| 高     | 7    |
| 中     | 5    |
| 低     | 3    |
| **計** | **15** |

### 診断対象ファイル

- `crates/seiran-common/migrations/` — 19 件のマイグレーションファイル
- `crates/seiran-common/src/repository/` — actor / atp / follow / media_file / post / storage_provider / user
- `crates/seiran-api/src/handlers/` — notes / users / follows / admin/users / admin/emojis / xrpc/repo / xrpc/sync
- `crates/seiran-common/src/ap/deliver.rs` / `outbox.rs`
- `crates/seiran-common/src/atp/service.rs`

---

## 2. インデックス追加・変更案（優先度順）

### [高-1] `actors(user_id)` インデックス欠落

**該当テーブル**: `actors`  
**問題**: `user_id` にインデックスが存在しない。認証済みリクエストすべてで呼ばれる以下のクエリが逐次スキャンになる。

```sql
-- find_local_by_user_id（全認証エンドポイントで毎回実行）
SELECT ... FROM actors WHERE user_id = $1 AND actor_type = 'local' LIMIT 1

-- update_profile でも同パターンを 2 回実行（SELECT + UPDATE）
SELECT ... FROM actors WHERE user_id = $1 AND actor_type = 'local' LIMIT 1
UPDATE actors SET ... WHERE user_id = $4 AND actor_type = 'local'
```

**推奨インデックス**:

```sql
-- ローカルアクターへの絞り込みが常に伴うため部分インデックスが最適
CREATE UNIQUE INDEX idx_actors_user_id_local
  ON actors(user_id)
  WHERE actor_type = 'local' AND user_id IS NOT NULL;
```

---

### [高-2] `actors(username, domain)` 複合インデックス欠落

**該当テーブル**: `actors`  
**問題**: WebFinger / プロフィール検索 / フォロー解決など、最頻繁に呼ばれる組み合わせ検索にインデックスがない。

```sql
-- find_by_username_domain / find_did_by_username_domain
SELECT ... FROM actors WHERE username = $1 AND domain = $2 LIMIT 1
```

**推奨インデックス**:

```sql
CREATE INDEX idx_actors_username_domain ON actors(username, domain);
```

---

### [高-3] `follows(follower_actor_id, status)` INCLUDE カバリングインデックス欠落

**該当テーブル**: `follows`  
**問題**: ホームタイムラインのサブクエリは毎リクエスト実行される。現在は `idx_follows_follower(follower_actor_id)` だけで `status` による絞り込みが index filter どまり。`target_actor_id` まで含めたカバリングインデックスにすれば index-only scan になる。

```sql
-- home_timeline サブクエリ（全ホームTL リクエストで評価）
SELECT target_actor_id FROM follows
WHERE follower_actor_id = $1 AND status = 'accepted'
```

**推奨インデックス**:

```sql
-- 既存 idx_follows_follower を以下で置き換え
DROP INDEX IF EXISTS idx_follows_follower;
CREATE INDEX idx_follows_follower_status
  ON follows(follower_actor_id, status)
  INCLUDE (target_actor_id)
  WHERE status = 'accepted';
```

---

### [高-4] `posts(actor_id, id)` WHERE 削除済み除外 複合インデックス欠落

**該当テーブル**: `posts`  
**問題**: 以下の 4 クエリがすべて `(actor_id, id, deleted_at)` の複合条件で ORDER BY id を使うが、個別インデックスしかなく、Postgres が最適プランを選べない。

```sql
-- recent_by_actor / context_before / context_after
WHERE actor_id = $1 AND deleted_at IS NULL ORDER BY id DESC/ASC LIMIT $2
```

**推奨インデックス**:

```sql
CREATE INDEX idx_posts_actor_id_active
  ON posts(actor_id, id DESC)
  WHERE deleted_at IS NULL;
```

---

### [中-1] `media_files(storage_provider_id)` インデックス欠落

**該当テーブル**: `media_files`  
**問題**: 容量チェック時に毎回フルスキャンまたは PK スキャンが発生する。

```sql
SELECT SUM(size)  FROM media_files WHERE storage_provider_id = $1
SELECT COUNT(*)   FROM media_files WHERE storage_provider_id = $1
```

**推奨インデックス**:

```sql
CREATE INDEX idx_media_files_provider ON media_files(storage_provider_id);
```

さらに `size` を INCLUDE すれば SUM クエリが index-only scan になる:

```sql
CREATE INDEX idx_media_files_provider_size
  ON media_files(storage_provider_id)
  INCLUDE (size);
```

---

### [中-2] `actors` に `username` 単独インデックス追加（ログイン用）

**該当テーブル**: `actors`  
**問題**: ユーザー名ログインクエリが actors をフルスキャンする。

```sql
-- find_login_by_username
JOIN actors a ON a.user_id = u.id AND a.actor_type::text = 'local'
WHERE a.username = $1
```

`idx_actors_username_domain` (高-2) が作成されれば `(username, domain)` 複合インデックスの前方一致でカバーされるため、追加インデックスは不要。ただし **高-2 が前提**。

---

### [中-3] `follows(target_actor_id, status)` 複合インデックス（AP 配送用）

**該当テーブル**: `follows`  
**問題**: AP 配送時にフォロワーの inbox URL を取得するクエリが `target_actor_id` + `status` で絞り込む。

```sql
-- deliver_post_to_ap_followers
SELECT a.ap_inbox_url FROM follows f
JOIN actors a ON a.id = f.follower_actor_id
WHERE f.target_actor_id = $1 AND f.status = 'accepted' AND a.actor_type = 'fedi'
  AND a.ap_inbox_url IS NOT NULL
```

既存 `idx_follows_target(target_actor_id)` は status フィルタをカバーしない。

**推奨インデックス**:

```sql
CREATE INDEX idx_follows_target_accepted
  ON follows(target_actor_id)
  WHERE status = 'accepted';
```

---

### [低-1] `custom_emojis(category)` インデックス

カテゴリ別絞り込みが将来追加された場合に備える。現在は影響軽微。

```sql
CREATE INDEX idx_custom_emojis_category ON custom_emojis(category)
  WHERE category IS NOT NULL;
```

---

## 3. クエリ最適化案

### [高-5] ホームタイムライン: 相関サブクエリを CTE + JOIN に書き換え

**場所**: `crates/seiran-common/src/repository/post.rs` — `home_timeline()`  
**問題**: `OR p.actor_id IN (SELECT ...)` 形式の相関サブクエリは、外側の posts テーブルの行数に応じて繰り返し評価される可能性がある。フォロー数が増えると `IN (...)` リストも大きくなり、プランナが Hash Join を選べないケースもある。

**現在のクエリ**:

```sql
SELECT p.id, p.body, p.created_at, a.id as actor_id, a.username, a.domain, a.display_name
FROM posts p JOIN actors a ON a.id = p.actor_id
WHERE p.deleted_at IS NULL
  AND ($2::bigint IS NULL OR p.id < $2)
  AND ($3::bigint IS NULL OR p.id > $3)
  AND (p.actor_id = $1 OR p.actor_id IN (
        SELECT target_actor_id FROM follows
        WHERE follower_actor_id = $1 AND status = 'accepted'))
ORDER BY p.id DESC LIMIT $4
```

**推奨クエリ**:

```sql
WITH followed_actors AS (
    SELECT target_actor_id AS actor_id
    FROM follows
    WHERE follower_actor_id = $1 AND status = 'accepted'
    UNION ALL
    SELECT $1::bigint
)
SELECT p.id, p.body, p.created_at, a.id AS actor_id, a.username, a.domain, a.display_name
FROM posts p
JOIN actors a ON a.id = p.actor_id
JOIN followed_actors fa ON fa.actor_id = p.actor_id
WHERE p.deleted_at IS NULL
  AND ($2::bigint IS NULL OR p.id < $2)
  AND ($3::bigint IS NULL OR p.id > $3)
ORDER BY p.id DESC
LIMIT $4
```

`UNION ALL + JOIN` により Postgres が Hash Join または Merge Join を選択でき、大量フォロー時のスケーラビリティが改善される。

---

### [高-6] `commit_record_inner` の atp_blocks N+1 INSERT を一括 INSERT に変更

**場所**: `crates/seiran-common/src/atp/service.rs` — `commit_record_inner()`  
**問題**: 新規コミット時に MST ブロック数（投稿数が増えるにつれ増大）分だけ DB ラウンドトリップが発生する。投稿数 1,000 件のアクターなら 1 コミットあたり数十〜数百回の INSERT になりうる。

**現在のコード**:

```rust
for (cid, bytes) in &new_blocks {
    sqlx::query(
        "INSERT INTO atp_blocks (cid, actor_id, bytes) VALUES ($1, $2, $3)
         ON CONFLICT (cid, actor_id) DO NOTHING",
    )
    .bind(cid_to_string(cid))
    .bind(actor_id)
    .bind(bytes.as_slice())
    .execute(&self.pool)
    .await?;
}
```

**推奨**: `COPY` プロトコルまたは `unnest` による一括 INSERT に変更する。

```sql
-- unnest を用いた一括挿入（sqlx では query! マクロ非対応のため query を使用）
INSERT INTO atp_blocks (cid, actor_id, bytes)
SELECT unnest($1::text[]), $2, unnest($3::bytea[])
ON CONFLICT (cid, actor_id) DO NOTHING
```

Rust 側では `cid_strings: Vec<String>` と `bytes_vec: Vec<Vec<u8>>` を収集してから 1 クエリで送信する。

---

### [高-7] `load_atp_entries` の全件取得 — MST インクリメンタル更新への移行

**場所**: `crates/seiran-common/src/atp/service.rs` — `load_atp_entries()`  
**問題**: コミットのたびにアクターの全投稿を posts テーブルから取得して MST を全再構築している。投稿数 10,000 件のアクターでは毎コミット 10,000 行を読み込む。

**現在のコード**:

```sql
SELECT at_rkey, at_cid FROM posts
WHERE actor_id = $1 AND at_rkey IS NOT NULL AND at_cid IS NOT NULL AND deleted_at IS NULL
```

**推奨アーキテクチャ**:

1. `actors` テーブルに `at_mst_entries_json JSONB` カラムを追加し、MST のエントリリスト（`{key: cid}` のマップ）をキャッシュとして保持する。
2. コミット時は差分（追加/削除レコード）のみ適用してキャッシュを更新し、MST 再構築のたびに全件 SELECT しない。

これにより `load_atp_entries` を `O(n)` から `O(1)` に改善できる。

---

### [中-4] ローカルタイムライン: actors JOIN の型キャスト廃止

**場所**: `crates/seiran-common/src/repository/post.rs` — `local_timeline()`  
**問題**: `WHERE a.actor_type = 'local'` は ENUM と文字列リテラルを比較しており、PostgreSQL は暗黙キャストするが、`admin/users.rs` の `find_login_by_username` では `a.actor_type::text = 'local'` と不要なキャストを行っている。ENUM 型のキャストはインデックス利用を妨げることがある。

**推奨**: `actor_type::text = 'local'` を `actor_type = 'local'` に統一する（sqlx 側で `'local'::actor_type_enum` とバインドするか、`::text` キャストを除去）。

**影響箇所**:
- `handlers/admin/users.rs` の `list_users` クエリ
- `handlers/users.rs` の `find_login_by_username`

---

### [中-5] `find_blocks_by_actor` の全件取得とページネーション不在

**場所**: `crates/seiran-common/src/repository/atp.rs` — `find_blocks_by_actor()`  
**問題**: `getRepo` XRPC で actor の全ブロックを一括取得する。アクターの投稿が増えると BYTEA データの総量が数十 MB になりうる。

```sql
SELECT cid, bytes FROM atp_blocks WHERE actor_id = $1
-- LIMIT なし
```

**推奨**:
- `getRepo` の用途（CAR ファイル生成）には全ブロックが必要なため、ストリーミング or COPY 出力を検討する。
- PostgreSQL の `TOAST` による自動圧縮は有効だが、`atp_blocks.bytes` に `STORAGE EXTERNAL` を明示して `pg_toast` テーブルへの分離を保証することで、メインテーブルのシーケンシャルスキャンコストを下げる。

```sql
ALTER TABLE atp_blocks ALTER COLUMN bytes SET STORAGE EXTERNAL;
```

---

### [低-2] `list_users` の `LIMIT 100` ハードコードと型キャスト

**場所**: `crates/seiran-api/src/handlers/admin/users.rs` — `list_users()`  
**問題**: ユーザー数が 100 を超えた場合に一覧が途切れる。カーソルベースページネーションに移行すべき。

```sql
SELECT u.id, u.email, u.role::text AS role, u.suspended_at, a.username
FROM users u
LEFT JOIN actors a ON a.user_id = u.id AND a.actor_type::text = 'local'
ORDER BY u.id
LIMIT 100
```

**推奨**: クエリパラメータ `after_id` を追加し、`WHERE u.id > $after_id ORDER BY u.id LIMIT 50` 形式のカーソルページネーションに変更。

---

### [低-3] `users.count()` のフルスキャン

**場所**: `crates/seiran-common/src/repository/user.rs` — `count()`  
**問題**: `SELECT COUNT(*) FROM users` はサーバー起動時のセットアップチェックのみで使われるため現在は影響小。将来的にポーリングが増えた場合は問題になる。  
**推奨**: セットアップフラグを環境変数または別テーブルのフラグカラムで管理し、全件 COUNT を廃止する。

---

## 4. キャッシング・非同期化の提案

### [高] Redis によるホームタイムラインキャッシュ

ホームタイムラインは SNS で最も読み取り頻度の高いエンドポイントであり、かつ最もコストの高いクエリ（follows サブクエリ + posts JOIN actors）を毎リクエスト実行する。

**推奨設計**:

```
キー: home_tl:{actor_id}
型:   Redis Sorted Set（score = post_id, value = JSON シリアライズした TimelinePost）
TTL:  60 秒〜300 秒（設定可能）
```

- 新規投稿時: 投稿者のフォロワーリストを Redis から取得し、各フォロワーの Sorted Set の先頭に push する（fan-out on write）。
- タイムラインフェッチ: Redis に十分なデータがあれば DB を触らない。キャッシュミス時のみ DB から補完。
- 削除時: 該当 post_id を Sorted Set から削除。

**実装上の注意**: フォロワー数が非常に多いアカウント（ローカルでは稀だが Relay 経由で増えた場合）は fan-out on write が重くなるため、一定フォロワー数を超えたら fan-out on read に切り替えるハイブリッド戦略を検討する。

---

### [高] Redis によるアクター情報キャッシュ

`find_local_by_user_id`、`find_by_username_domain`、`find_by_did` はほぼすべてのリクエストで呼ばれる。アクター情報は変更頻度が低いため Redis キャッシュが非常に効果的。

**推奨設計**:

```
キー: actor:user_id:{user_id}
キー: actor:username_domain:{username}:{domain}
キー: actor:did:{at_did}
型:   Redis Hash または JSON 文字列
TTL:  300 秒（プロフィール更新時に明示的に無効化）
```

---

### [高] AP 配送の並列化

**場所**: `crates/seiran-common/src/ap/deliver.rs` — `deliver_post_to_ap_followers()`  
**問題**: フォロワー全員への HTTP POST を逐次実行している。フォロワー数 100 人でそれぞれ 200ms かかると合計 20 秒の処理時間になる。すでに `tokio::spawn` 内で実行されているが、スポーン内部が同期的。

**推奨**:

```rust
use futures::stream::{self, StreamExt};

// 最大同時接続数を制限しながら並列配送
let results = stream::iter(inboxes)
    .map(|inbox| {
        let client = ap_client.clone();
        let body = body_str.clone();
        let key_id = actor_key_id.clone();
        let pem = pem.clone();
        async move {
            client.sign_and_post(&inbox, &body, &key_id, &pem).await
        }
    })
    .buffer_unordered(20)  // 最大 20 並列
    .collect::<Vec<_>>()
    .await;
```

---

### [中] Redis による AP アクター情報のキャッシュ（WebFinger 結果）

WebFinger + AP Actor ドキュメント取得は外部 HTTP リクエストを伴い、レスポンス時間に影響する。

**推奨設計**:

```
キー: ap_actor:{uri}
型:   JSON 文字列（ApActor のシリアライズ）
TTL:  3600 秒（1 時間）
```

フォロー・フォロワー一覧ページのプロフィール表示などで大幅に高速化できる。

---

### [中] Redis によるカスタム絵文字リストキャッシュ

`list_emojis` は管理画面に限られるが、将来フロントエンドの絵文字ピッカーから叩かれると高頻度になる。

**推奨設計**:

```
キー: custom_emojis:all
型:   JSON 文字列（Vec<EmojiResponse> のシリアライズ）
TTL:  600 秒（絵文字追加・削除時に INVALIDATE）
```

---

### [中] ATP コミットの非同期ジョブキュー化

**場所**: `crates/seiran-api/src/handlers/notes.rs` — `create_note()`  
**問題**: `atp_service.commit_post()` は以下の処理を同期的に実行しており、HTTP レスポンスを遅延させる。

1. DB から全投稿を取得（MST 再構築）
2. MST 構築（CPU 処理）
3. P-256 署名
4. 複数回の DB 書き込み（blocks N+1 INSERT + actors UPDATE + events INSERT）
5. WebSocket ブロードキャスト

**推奨**: 既に実装済みのジョブキュー基盤（Phase 3）を活用し、ATP コミットをバックグラウンドジョブとして非同期化する。投稿 DB への INSERT 完了を持って HTTP 200 を返し、ATP コミットは非同期で処理する。

```
create_note() → INSERT posts → enqueue("atp_commit", {actor_id, post_id, text}) → 200 OK
                                       ↓ async
                               commit_record_inner()
```

---

### [低] `atp_repo_events` の car_bytes 外部ストレージ

**問題**: `atp_repo_events.car_bytes` は差分 CAR データを BYTEA で inline 保存している。大量のイベントが蓄積されると `subscribeRepos` のバックフィルクエリがロー幅の大きいテーブルをスキャンする。

**推奨**:
1. `atp_blocks.bytes` と同様に `SET STORAGE EXTERNAL` を設定して TOAST への分離を保証。
2. 長期的には S3/R2 などのオブジェクトストレージに CAR を格納し、`atp_repo_events` には URL のみを保存する設計を検討。

---

## 5. マイグレーション適用手順

上記の推奨インデックスを適用する場合は、新規マイグレーションファイルを作成し `cargo sqlx migrate run` で適用すること（`psql -f` 直接実行禁止）。

```sql
-- 例: 20260703000001_perf_indexes.sql

-- [高-1] actors.user_id 部分ユニークインデックス
CREATE UNIQUE INDEX idx_actors_user_id_local
  ON actors(user_id)
  WHERE actor_type = 'local' AND user_id IS NOT NULL;

-- [高-2] actors.username + domain 複合インデックス
CREATE INDEX idx_actors_username_domain ON actors(username, domain);

-- [高-3] follows カバリングインデックス（既存置き換え）
DROP INDEX IF EXISTS idx_follows_follower;
CREATE INDEX idx_follows_follower_status
  ON follows(follower_actor_id, status)
  INCLUDE (target_actor_id)
  WHERE status = 'accepted';

-- [高-4] posts per-actor 複合部分インデックス
CREATE INDEX idx_posts_actor_id_active
  ON posts(actor_id, id DESC)
  WHERE deleted_at IS NULL;

-- [中-1] media_files ストレージプロバイダー別集計用
CREATE INDEX idx_media_files_provider_size
  ON media_files(storage_provider_id)
  INCLUDE (size);

-- [中-3] follows フォロワー配送用部分インデックス
CREATE INDEX idx_follows_target_accepted
  ON follows(target_actor_id)
  WHERE status = 'accepted';

-- TOAST 外部ストレージ設定
ALTER TABLE atp_blocks ALTER COLUMN bytes SET STORAGE EXTERNAL;
ALTER TABLE atp_repo_events ALTER COLUMN car_bytes SET STORAGE EXTERNAL;
```

> **注意**: `CREATE INDEX CONCURRENTLY` オプションを使えば本番稼働中のテーブルロックを回避できる（`CONCURRENTLY` は `CREATE UNIQUE INDEX` でも使用可能）。ただし sqlx マイグレーション内ではトランザクション外実行が必要なため、`-- sqlx: no-transaction` アノテーションを付与すること。

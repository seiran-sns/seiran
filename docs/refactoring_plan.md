# seiran リファクタリング計画書

> 作成日: 2026-06-30
> 改訂日: 2026-06-30（Phase 4.3/4.5 現状に合わせて全面改訂）
> 改訂日2: 2026-07-10（実コード再検証 + What/How分離・未実装機能への拡張余地の観点で追加分析、§3 参照）
> 対象リビジョン: c3b0946

---

## 0. 完了済みタスク（参照のみ）

| タスク | 状態 | 主な変更ファイル |
|--------|------|-----------------|
| H-1（旧）: AtpCommitService 作成 | 完了 | `seiran-common/src/atp/service.rs` |
| H-2（旧）: seiran-api main.rs 分割 | 完了 | `seiran-api/src/handlers/` 配下に各ファイル作成、main.rs 185行化 |
| M-2（旧）: plc.rs HTTP クライアント注入 | 完了 | `PlcGenesis`/`submit_plc_genesis(client)` 引数追加 |
| Cloudflare クライアント抽出 | 完了 | `seiran-api/src/cloudflare.rs` |
| AP 配送ロジック抽出 | 完了 | `seiran-common/src/ap/deliver.rs` |
| ApiError 定義 | 完了 | `seiran-api/src/error.rs` |
| 認証ミドルウェア分離 | 完了 | `seiran-api/src/middleware/auth.rs` |
| Auth0 廃止 | 完了 | `seiran-common/src/auth/` から auth0.rs 削除 |
| **H-B step1**: `unwrap_or(0)` → `?` 置換 | **完了** | `handlers/follows.rs`, `handlers/notes.rs`, `handlers/users.rs` |
| **M-D**: `miauth_authorize` ハードコード修正 | **完了** | `handlers/miauth.rs` |
| **L-A**: `strip_html`/`plain_to_html` テスト追加 | **完了** | `federation-inbox/src/handlers/inbox.rs`, `seiran-common/src/ap/deliver.rs` |
| **H-A**: `seiran-federation-inbox/src/main.rs` 分割 | **完了** | `federation-inbox/src/handlers/` ディレクトリ作成、main.rs 68行化 |
| **M-A**: AP クライアントへの `reqwest::Client` 注入 | **完了** | `ap/client.rs` ほか |
| **M-B**: `ApClient` 構造体化・グローバルキャッシュ廃止 | **完了** | `ap/client.rs`, 各 `AppState` |
| **M-C**: pub API の `Result<_, String>` → typed error | **完了** | `ap/*`, `auth/local.rs` |
| **H-B step2-3**: Repository 層導入・ハンドラから SQL 排除 | **完了** | `seiran-common/src/repository/`, 全ハンドラ |
| **M-E**: `atp_repository_publish.rs` 役割明確化・HTTP クライアント共有化 | **完了** | `atp/client.rs`, `jobs/atp_repository_publish.rs`, `seiran-atp-repo/` |
| **L-B**: HTTP Signatures ユニットテスト追加 | **完了** | `ap/client.rs` の `parse_signature_header` / `build_signing_string` テスト 7 件 |

---

## 1. 現状の問題点（Phase 4.3/4.5 現状分析）

### 問題 H-A: `seiran-federation-inbox/src/main.rs` が 912 行の単一ファイル（HIGH）

旧 `seiran-api/src/main.rs` と同じ構造問題がそのまま `seiran-federation-inbox` に残っている。

直接 `sqlx::query` を呼んでいるハンドラ（計 18 箇所以上）:

| 関数 | クエリ数 |
|------|---------|
| `handle_follow` | 5（ローカルアクター検索 + リモート upsert + follows INSERT 等） |
| `handle_create_note` | 2（リモートアクター upsert + posts INSERT） |
| `handle_accept` | 3（ローカル/リモートアクター検索 + follows UPDATE） |
| `handle_undo` | 3（follower/target 検索 + follows DELETE） |
| `webfinger_handler` | 1 |
| `actor_handler` | 1 |
| `nodeinfo_handler` | 2 |
| `outbox_handler` | 3 |

加えて:
- 全ハンドラが `Result<(), String>` を返す（型付きエラーなし）
- `AppState` が `ap_private_key_pem: String` を bare String で持つ（`Arc<Secrets>` を使っていない）

分割後のディレクトリ構成:

```
crates/seiran-federation-inbox/src/
├── main.rs          # エントリポイント + AppState + Router のみ（目標 80 行以内）
├── error.rs         # FederationError 型（thiserror ベース）
└── handlers/
    ├── mod.rs
    ├── inbox.rs     # inbox_handler + handle_follow / handle_create_note / handle_accept / handle_undo
    ├── webfinger.rs # webfinger_handler
    ├── actor.rs     # actor_handler
    ├── nodeinfo.rs  # nodeinfo_discovery_handler + nodeinfo_handler
    └── outbox.rs    # outbox_handler
```

`error.rs` の設計:

```rust
#[derive(Debug, thiserror::Error)]
pub enum FederationError {
    #[error("DB エラー: {0}")]
    Db(#[from] sqlx::Error),
    #[error("AP エラー: {0}")]
    Ap(String),
    #[error("フィールド不足: {0}")]
    MissingField(&'static str),
}
```

**期待効果**: 912 行 → main.rs 80 行 + 各ファイル 100〜150 行。型付きエラーに統一。

---

### 問題 H-B: seiran-api ハンドラ層に直接 SQL が 31 箇所残存（HIGH）

seiran-api は main.rs 分割済みだが、Repository 層が未実装のため各ハンドラが直接 `state.db` に `sqlx::query` を呼んでいる。

| ファイル | クエリ数 |
|----------|---------|
| `handlers/auth.rs` | 6 |
| `handlers/notes.rs` | 10 |
| `handlers/follows.rs` | 3 |
| `handlers/users.rs` | 5 |
| `handlers/xrpc/sync.rs` | 3 |
| `handlers/xrpc/server.rs` | 2 |
| `handlers/xrpc/repo.rs` | 1 |
| `main.rs`（起動タスク） | 1 |

特に緊急な `unwrap_or(0)` バグ（**ステップ 1** として即座に修正すること）:

| ファイル:行 | リスク |
|------------|-------|
| `handlers/follows.rs:46` | `local_actor_id = 0` で誰のフォローか不明になる |
| `handlers/notes.rs:187` | `actor_id = 0` でホーム TL に全員の投稿が混在する |
| `handlers/users.rs:98` | `actor_id = 0` でフォロー状態判定が全員分おかしくなる |

Repository trait の段階的導入:

```
crates/seiran-common/src/repository/
├── mod.rs
├── actor.rs    # ActorRepository trait + PgActorRepository
├── post.rs     # PostRepository trait + PgPostRepository
└── follow.rs   # FollowRepository trait + PgFollowRepository
```

---

### 問題 M-A: AP/ATP クライアントが `reqwest::Client` を毎回生成（MEDIUM）

`crates/seiran-common/src/ap/client.rs` の `fetch_actor()`、`sign_and_post()` 等が `reqwest::Client::builder().build()` をローカルで生成しており `AppState.http_client` の Connection Pool が AP フェデレーション通信で使われない。テスト時にモックサーバーを差し込めない。

修正方針: `fetch_actor(uri, client: &reqwest::Client)` のように引数追加。`seiran-federation-inbox` の `AppState` にも `http_client: Arc<reqwest::Client>` を追加。

---

### 問題 M-B: `ApClient` 構造体がなく `PUBLIC_KEY_CACHE` がプロセスグローバル（MEDIUM）

`crates/seiran-common/src/ap/client.rs` の `PUBLIC_KEY_CACHE: OnceLock<...>` がテスト間で共有されるため AP 署名検証のテストが書けない。

修正方針:

```rust
pub struct ApClient {
    http: Arc<reqwest::Client>,
    key_cache: Arc<RwLock<HashMap<String, String>>>,
}
```

`AppState` に `ap_client: Arc<ApClient>` を追加。

---

### 問題 M-C: `Result<_, String>` が pub API に多数残存（MEDIUM）

| モジュール | 影響関数 |
|-----------|---------|
| `ap/client.rs` | `fetch_actor`, `verify_signature`, `sign_and_post` 等 |
| `ap/deliver.rs` | `deliver_post_to_ap_followers` |
| `auth/local.rs` | `hash_password`, `verify_password`, `generate_token`, `verify_token` |
| `jobs/` 各ファイル | `handle` 系関数群 |

---

### 問題 M-D: `miauth_authorize` に `user_id=1` ハードコード（MEDIUM・本番バグ）

`crates/seiran-api/src/handlers/miauth.rs` に `user_id = Some(1)` / `username = Some("test_user")` のハードコードが残っており、MiAuth フローを使うと全員が `user_id=1` で連携される。

修正方針: `miauth_page` で Bearer トークン認証を要求し、認証済みユーザーの `user_id` をセッションに保存。`miauth_authorize` ではセッションの `user_id` から DB で username を取得。

---

### 問題 M-E: `atp_repository_publish.rs` の役割が不明確（MEDIUM）

`crates/seiran-common/src/jobs/atp_repository_publish.rs` は「外部 bsky.social へのミラーリング」と「自サーバー ATP リポジトリへのコミット（`AtpCommitService`）」の役割が混在している可能性がある。未使用コードを整理し、`AtpCommitService` との重複を解消する。

---

### 問題 L-A: 純粋関数のユニットテストが皆無（LOW）

即座に書けるテスト対象:

| 関数 | ファイル |
|------|---------|
| `strip_html` | `seiran-federation-inbox/src/main.rs`（分割後は `handlers/inbox.rs`） |
| `plain_to_html` | `seiran-common/src/ap/deliver.rs` |

最低限追加すべきテスト:

```rust
// strip_html
#[test]
fn test_strip_html_simple() {
    assert_eq!(strip_html("<p>hello <b>world</b></p>"), "hello world");
}
#[test]
fn test_strip_html_entities() {
    assert_eq!(strip_html("&amp; &lt; &gt;"), "& < >");
}

// plain_to_html
#[test]
fn test_plain_to_html_paragraph() {
    assert_eq!(plain_to_html("a\n\nb"), "<p>a</p><p>b</p>");
}
#[test]
fn test_plain_to_html_newline() {
    assert_eq!(plain_to_html("a\nb"), "<p>a<br>b</p>");
}
#[test]
fn test_plain_to_html_no_xss() {
    assert!(!plain_to_html("<script>alert(1)</script>").contains("<script>"));
}
```

---

### 問題 L-B: HTTP Signatures 関連のテストなし（LOW）

`ap/client.rs` の `parse_signature_header`、`build_signing_string` 等に `#[cfg(test)] mod tests` を追加する（M-A/M-B 完了後に実施）。

---

## 2. 実施順序

```
H-B ステップ 1（unwrap_or(0) → ? 置換）     ← 緊急・バグリスク（30分）
    ↓
M-D（miauth スタブ修正）                      ← 本番バグ（1〜2時間）
    ↓
L-A（strip_html / plain_to_html テスト追加）  ← 低リスク・高価値（1時間）
    ↓
H-A（federation-inbox 分割）                  ← 912行ファイル解消（3〜4時間）
    ↓
M-A（AP クライアント HTTP 注入）              ← H-A 完了後に統合（2時間）
    ↓
M-B（ApClient 構造体化・OnceLock 解消）      ← M-A 完了後（1時間）
    ↓
M-C（String エラー → typed error）           ← M-A/M-B 完了後（2〜3時間）
    ↓
H-B ステップ 2〜3（Repository trait 導入）   ← 5〜8時間
    ↓
M-E（atp_repository_publish 整理）           ← H-B 完了後
    ↓
L-B（HTTP Signatures テスト）               ← M-B 完了後
```

各ステップで `cargo build` + `cargo test` を実行し、コンパイルエラーのない状態を維持すること。

---

## 3. 2026-07-10 再検証と追加分析

`docs/improvement_code_quality.md` および本書 §0〜2 は前回改訂（2026-06-30/07-02）時点の記述であり、その後のコミットで一部は解消済み、一部は未解消のまま残っていた。実コードを再検証した結果を以下にまとめる。

### 3-1. 前回レポートの記述と実コードの差分（検証結果）

| 項目 | 前回レポートの記述 | 実際の状態（2026-07-10 検証） |
|------|-------------------|------------------------------|
| [高-1] ATP commit のトランザクション化 | 未対応として記載 | **対応済み**。`atp/service.rs` の `commit_record_inner`（後続の `commit_repost` 等含む）は `pool.begin()` 〜 `tx.commit()` で囲われている（`atp/service.rs:192,301` 等）。roadmap の記述と一致。 |
| `ApClient` の構造体化・`OnceLock` 廃止 | 未対応として記載 | **対応済み**。`ap/client.rs:80` の `ApClient` が `key_cache: Arc<RwLock<HashMap<..>>>` を保持。グローバル `OnceLock` は残っていない。**ただし TTL は依然未実装**（[中-4] は引き続き有効）。 |
| `handle_follow`（inbox.rs） | Repository 未経由 | **対応済み**。`actor_repo` / `follow_repo` を全面的に使用し生 SQL なし。 |
| `handle_create_note`（inbox.rs） | Repository 未経由 | **一部対応**。リモートアクター upsert は `actor_repo` 経由になったが、投稿の重複排除判定・INSERT・添付保存・フォロワー配信は生 `sqlx::query` のまま（§3-2 参照）。 |

### 3-2. 新たに判明した問題

#### [新-1] `AppState.post_repo` が inbox.rs に注入されているのに一度も使われていない（デッドコード）

`crates/seiran-federation-inbox/src/lib.rs` の `AppState` は `post_repo: Arc<dyn PostRepository>` を持ち `init_state()` で初期化しているが、`handlers/inbox.rs` は `post_repo` を一切参照せず、代わりに `state.db` に対して 15 箇所の生 `sqlx::query` / `sqlx::query!` を発行している（重複排除判定・投稿 INSERT・添付 INSERT・フォロワー取得・reactions CRUD・Undo(Announce) の論理削除等）。Repository を導線としてもコード上不変条件が保てておらず、変更点が二重管理になるリスクが高い。

#### [新-2] `create_note`（`seiran-api/src/handlers/notes.rs`）が約500行の単一関数

コーディングルール（`docs/coding_rules.md` §1「ハンドラ 1 関数の目安は 30 行以内」）に対し `create_note` は 327〜832 行目の約500行。中身は以下がすべて 1 関数に同居している:

- リポスト分岐（元ポスト種別判定 SQL・INSERT・AP Announce 配送・ATP repost 配送・realtime broadcast）
- リプライ分岐（元ポスト種別判定 SQL・ATP reply フィールド組み立て）
- 引用分岐（元ポスト種別判定 SQL・Bsky embed / AP quoteUrl 組み立て）
- 通常投稿の文字数バリデーション・INSERT・添付 INSERT・メンション変換・ATP commit 起動・AP 配送起動・realtime broadcast

「元ポストの種別を取得する」という同一パターンの SQL（`ap_object_id, at_uri, at_cid, domain, ...` を posts JOIN actors で取得）がリポスト・リプライ・引用の 3 箇所にほぼ同一の形でベタ書きされている。What（元ポスト情報を取得する）と How（JOIN SQL の書き方）が分離されておらず、3 箇所とも将来の同時修正漏れリスクを抱える。

#### [新-3] 「accepted なローカルフォロワー」取得 SQL が 3 箇所に一字一句重複

```sql
SELECT f.follower_actor_id FROM follows f
 JOIN actors a ON a.id = f.follower_actor_id
 WHERE f.target_actor_id = $1 AND f.status = 'accepted' AND a.actor_type = 'local'
```

`seiran-api/src/handlers/notes.rs:506`（リポスト時 broadcast）、`notes.rs:818`（通常投稿時 broadcast）、`seiran-federation-inbox/src/handlers/inbox.rs:419`（リモート投稿受信時 broadcast）の 3 箇所に完全同一の SQL が独立して存在する。「新規投稿をローカルフォロワーへ realtime 配信する」という同じ What が 3 通りの How で実装されており、`FollowRepository` に本来あるべきメソッドが欠けている。

#### [新-4] `handle_create_note` が `state.local_domain` を使わず `std::env::var("LOCAL_DOMAIN")` を直読み

`inbox.rs:306` で `let local_domain = std::env::var("LOCAL_DOMAIN").unwrap_or_default();` としているが、`AppState` は既に `local_domain: String` を持っている（他の全ハンドラはそちらを使用）。起動設定と環境変数がずれた場合、ループバック検知（シナリオ1の重複排除）だけが誤動作する潜在バグ。`docs/improvement_code_quality.md` [中-5] と同種の問題（`AtpCommitService::commit_profile` の env 直読み）が別箇所にも存在していたことになる。

### 3-3. 未実装機能への拡張余地の観点

`docs/06_development_roadmap.md` で未実装（`[ ]`）のまま残っている項目のうち、今回のリファクタリングが土台として関係するもの:

- **4.4 リモート seiran ハンドシェイク / 4.5 特権初期同期**: どちらも「リモートアクターの検証」「投稿の一括インポート」を新規実装することになるが、`handle_create_note` 相当のロジック（重複排除判定・INSERT・添付保存）がハンドラに直書きのままだと、特権同期エンドポイント側でも同じ SQL を再度書き写す羽目になる。**Repository 層に処理を寄せておくことが、4.5 の「相手サーバーから生データをバルク取得・直接インポートする処置」を実装する際の再利用可能な土台になる**。
- **7.4 サードパーティ製クライアント互換（bio 末尾へのURL自動挿入）**: プロフィール系ハンドラ（`users.rs`）に同様の直書き SQL が残っており、直接の依存関係はないが同じ Repository 経由方針を適用すべき対象として記録しておく。
- **RedisJobQueue / RedisSessionStore（フェーズ8）**: `outbound_post_delivery.rs` / `inbound_activity_process.rs` が今も sleep するだけのスタブで、実処理は各ハンドラの `tokio::spawn` に直接書かれている。ジョブキュー経由に一本化する際は、まさに本節で Repository 化した「投稿保存」「フォロワー取得」ロジックがジョブハンドラ側からも呼べる形になっている必要がある。今回の変更はその前提を満たす。

### 3-4. 今回実施したリファクタリング（このコミットでの変更）

1. `FollowRepository::find_accepted_local_follower_ids` を追加し、[新-3] の 3 重複 SQL を一本化。
2. `PostRepository` に以下を追加し、[新-1]（inbox.rs の post_repo 未使用）・[新-2]（create_note の重複 SQL）を解消:
   - `find_delivery_meta` — 「元ポストの配送用メタ情報を取得する」という What を表すメソッド。リポスト・リプライ・引用の 3 箇所から共通利用。
   - `insert_full` / `insert_repost` / `attach_media` / `find_repost_undo_info` / `soft_delete_by_id`
   - `find_by_seiran_uuid` / `update_ap_object_id` / `find_id_by_at_uri` / `find_id_by_ap_or_at_uri` / `find_id_and_actor_by_ap_object_id` / `insert_remote_with_dedup`
3. `ReactionRepository` を新設し、inbox.rs の `reactions` テーブル直叩き（INSERT / DELETE / 対象ポスト検索）を移行。
4. `inbox.rs` の `handle_create_note` / `handle_undo` / `handle_reaction` / `handle_announce` / `fetch_and_save_note` を上記 Repository 経由に書き換え、`std::env::var("LOCAL_DOMAIN")` 直読みを `state.local_domain` に統一（[新-4] 解消）。
5. `notes.rs` の `create_note` を「リポスト作成」「通常投稿/リプライ作成」の経路ごとに意味のある関数へ分割し、元ポストのメタ情報取得を `find_delivery_meta` 呼び出しに統一。`delete_repost` / `note_context` の生 SQL も Repository 呼び出しへ置換。
6. 純粋関数（`classify_post`, `at_uri_to_bsky_app_url`, `bsky_app_url_to_at_uri`, `strip_html_tags`）にユニットテストを追加。

### 3-5. 今回のスコープ外（次回以降の課題として記録）

- `handlers/auth.rs`（7箇所）・`handlers/users.rs`（4箇所）・`handlers/admin/*.rs` に残る生 SQL の Repository 移行。
- `eprintln!`（241箇所）から `tracing` への移行（クレート自体が未導入）。
- `outbound_post_delivery.rs` / `inbound_activity_process.rs` のスタブ実装の解消（フェーズ4/5 のキュー統合待ち、§3-3 参照）。
- `ApClient.key_cache` への TTL 導入（[中-4]、引き続き未対応）。

---

## 4. 2026-07-10 外部 API 呼び出しの整理（SQL の Repository 化と同型の問題）

SQL は Repository 層に集約済みだが、外部 HTTP 呼び出し（AP/ATP/plc.directory/Cloudflare/S3 等）は同様の規律が適用されていない箇所が残っていた。SQL のときの「post_repo が注入されているのに使われていない」「同じクエリが3箇所に独立して書かれている」と同型の問題が見つかったため、同じ要領で整理した。

### 4-1. 発見した問題

| 問題 | 該当箇所 |
|------|---------|
| Bsky AppView `getProfile` 呼び出しが2箇所に独立重複（自前 struct もフィールドは同一で名前だけ違う: `BskyResp` / `AppViewGetProfileResp`） | `handlers/follows.rs::follow_bsky`, `handlers/users.rs::fetch_bsky_profile_from_appview` |
| AppView `searchPosts` 呼び出しがハンドラ内に直書き（3件目の独立実装） | `handlers/search.rs::search_appview`（private fn） |
| `"https://public.api.bsky.app"` が上記3箇所に文字列リテラルとして重複 | 同上 |
| 呼び出しのたびに `reqwest::Client::builder()` で新規クライアントを生成（コネクションプール再利用なし）。かつ呼び出し元が**ゼロ**のデッドコード | `atp/did.rs::resolve_did`（`DidDocument`/`DidService` ごと未使用） |
| ジョブキューのワーカーが `seiran-server/main.rs` で作った共有 `http_client` とは別に独自の `reqwest::Client` を生成。`all` ロールでも共有されない | `queue/worker.rs::JobContext::new()` |

### 4-2. 実施した整理

1. **`atp/client.rs` に `fetch_bsky_profile()` / `search_appview_posts()` を追加** — 既存の `fetch_atp_history` 等と同じ「Bsky AppView クライアント」モジュールに集約。`BskyProfile` 型で重複していた2つの struct を統一。
2. **`handlers/follows.rs` / `handlers/users.rs` / `handlers/search.rs` を上記関数呼び出しに置換** — ハンドラ内の URL 組み立て・エラーハンドリングのベタ書きを削除。
3. **`atp/did.rs` を丸ごと削除** — `resolve_did` / `DidDocument` / `DidService` は呼び出し元ゼロを確認した上で削除（`mod.rs` の `pub use` も除去）。
4. **`JobContext::new()` / `WorkerEngine::new()` を `Arc<ApClient>` 注入方式に変更** — 内部での `reqwest::Client::builder()` 生成を廃止。`seiran_federation_worker::run(ap_client)` もシグネチャ変更し、`seiran-server/main.rs` の `Role::All` では api ロールと同じ `ap_client` を共有、独立起動の `Role::Worker` のみプロセス冒頭で1回だけ生成する。

整理後、`reqwest::Client::builder()` の呼び出し箇所はプロセスのエントリポイント（`seiran-server/main.rs`）の2箇所（`Role::Worker` 単独起動用、`Role::Api`/`Role::Federation`/`Role::All` 共有用）のみに減少した（`rg 'reqwest::Client::(new|builder)\(\)' crates/` で確認）。

### 4-3. 今回のスコープ外

- `ap/webfinger.rs`, `mention.rs`, `cloudflare.rs`, `ap/client.rs` は元々 `&reqwest::Client` を引数で受け取る作法が徹底されており対応不要だった。
- plc.directory への呼び出し（`atp/plc.rs`）や S3 呼び出し（`aws-sdk-s3`）は今回未調査。次回の対象候補。

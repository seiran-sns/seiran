# seiran リファクタリング計画書

> 作成日: 2026-06-30
> 改訂日: 2026-06-30（Phase 4.3/4.5 現状に合わせて全面改訂）
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

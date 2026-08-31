# seiran コーディングルール

> 作成日: 2026-06-30  
> このドキュメントは seiran プロジェクトのすべての Rust コードに適用される（末尾のフロントエンド向けセクションを除く）。

---

## 1. レイヤー設計

seiran は **Handler → Service → Repository** の 3 層構造を採用する。

```
HTTP リクエスト
    │
    ▼
[Handler]        crates/seiran-api/src/handlers/
  ・リクエストのデシリアライズ
  ・入力バリデーション
  ・認証チェック（middleware を呼ぶ）
  ・Service を呼ぶ
  ・レスポンスのシリアライズ
    │
    ▼
[Service]        crates/seiran-common/src/atp/service.rs, etc.
  ・ビジネスルールの実装
  ・複数の Repository を組み合わせるトランザクション制御
  ・外部サービス（plc.directory 等）との連携
    │
    ▼
[Repository]     crates/seiran-common/src/repository/
  ・データベースへの CRUD 操作のみ
  ・SQL はここにしか書かない
  ・trait で抽象化しテストで差し替え可能にする
```

### 各層の責務（詳細）

#### Handler 層

- **やること**: HTTP 入出力の変換・バリデーション・レスポンス組み立て
- **やらないこと**: SQL、HTTP クライアント呼び出し、ビジネスロジック
- ハンドラ関数の戻り値は `impl IntoResponse` とし、`ApiError` を使う
- ハンドラ 1 関数の目安は 30 行以内。超える場合はビジネスロジックが混入している

```rust
// Good: Service を呼んでレスポンスを返すだけ
async fn create_note(
    headers: HeaderMap,
    State(state): State<AppState>,
    Json(req): Json<CreateNoteRequest>,
) -> Result<Json<NoteResponse>, ApiError> {
    let auth_user = state.auth_middleware.extract(&headers).await?;
    let note = state.note_service.create(&auth_user, &req.text).await?;
    Ok(Json(NoteResponse::from(note)))
}

// Bad: ハンドラに SQL が直接ある
async fn create_note(...) -> impl IntoResponse {
    let row = sqlx::query("SELECT id FROM actors WHERE ...").fetch_one(&state.db).await;
    // ...
}
```

#### Service 層

- **やること**: ビジネスロジック・複数 Repository の組み合わせ・外部 API 呼び出し
- **やらないこと**: HTTP の概念（StatusCode・HeaderMap 等）に触れること
- `struct XxxService { repo_a: Arc<dyn ARepo>, repo_b: Arc<dyn BRepo>, ... }` の形で依存を受け取る
- テストでは `MockXxxRepository` を差し込めるようにする

#### Repository 層

- **やること**: `trait` を定義し、`PgXxx` 実装で PostgreSQL への CRUD を提供する
- **やらないこと**: ビジネスロジック（WHERE 条件以上の判断をしない）
- `async_trait` を使い、trait を `Send + Sync` にする
- SQL はすべて Repository の `impl` ブロック内にのみ記述する

---

## 2. 禁止事項（絶対守ること）

| # | 禁止 | 代替 |
|---|------|------|
| 1 | ハンドラ関数内で `sqlx::query` を呼ぶ | Repository trait のメソッドを呼ぶ |
| 2 | ハンドラ関数内で `reqwest::Client` を使う | Service の依存として注入する |
| 3 | `Result<_, String>` を公開 API に使う | `thiserror` で定義した typed error を使う |
| 4 | `unwrap()` / `expect("")` を本番コードに使う | `?` または適切なエラー型に変換する |
| 5 | `unwrap_or(0)` / `unwrap_or_default()` で取得失敗を隠す | `?` で早期リターンするか `ok_or(Error::...)` で明示的にエラーにする |
| 6 | `main.rs` に 100 行を超えるコードを書く | 対応するハンドラファイルに移動する |
| 7 | `seiran-api` の Cargo.toml に `sqlx` を使う | Repository 呼び出しは `seiran-common` 経由にする（中長期目標） |
| 8 | `reqwest::Client::new()` を関数内でローカルに生成する | `AppState` または引数から受け取る |
| 9 | ビジネスロジック関数でファイルパスに `main.rs` を選ぶ | `handlers/` または `common/src/*/service.rs` に置く |
| 10 | スタブ値（`user_id = 1`, `username = "test_user"`）を本番コードに残す | セッションまたはトークンから実際のユーザーを取得する |
| 11 | 新規クエリを `sqlx::query`/`query_as`（実行時検証）で書く | `sqlx::query!`/`query_as!`/`query_scalar!`（コンパイル時検証）を使う。SELECT列と構造体のズレを実行時ではなくビルド時に検出できる（docs/code_audit_2026-08-05.md R-6）。`cargo sqlx prepare --workspace`の実行を忘れないこと |
| 12 | DBから取得済みのActor/投稿レコードに対して `domain == local_domain`（または`state.local_domain`）でローカル/リモート判定する | `actor_type == "local"` を使う（`actors.actor_type`列、`insert_local`の不変条件によりlocal⇔domain=local_domainは常に一致）。Actor/TimelinePostは既にactor_typeを保持、他の型はSELECTに`a.actor_type::text AS actor_type`を1列足すだけで済むことが多い。Hostヘッダー・WebFingerクエリ・ユーザー入力文字列など、DBレコードではない外部入力のdomain比較は対象外（そちらは元々SQL化できない） |
| 13 | `jobs::*::handle()` が既存の `Result<(), String>` のまま新規エラー分岐を追加する | 一時的障害（ネットワーク・タイムアウト等）と恒久的失敗（不正な入力・鍵未設定等）を区別できる場合は `Result<(), JobError>`（`traits::JobError`）を返す。`String`は`From`で自動的に`Transient`扱いになるため、触っていないジョブは変更不要。配送・外部API呼び出し系ジョブから優先的に移行する（`jobs::ap_delivery`が実例） |
| 14 | `actors.notes_count`/`followers_count`/`following_count` を `repository/post.rs` 以外から直接 `UPDATE` する、または `posts`/`follows` への都度の `COUNT(*)` で代替する | `notes_count`の増減は`repository/post.rs`の既存の書き込みメソッド（`insert_full`等）内のCTEパターン（`docs/database.md`「非正規化カウンタ」参照）に倣うこと。`followers_count`/`following_count`は`trg_follows_sync_counts`トリガーが`follows`への書き込みから自動的に再計算するため、`repository/follow.rs`側でカウンタ更新を意識する必要はない（新しいフォロー状態遷移を追加する場合も、素朴な`follows`へのINSERT/UPDATE/DELETEを書くだけでよい） |

---

## 3. エラーハンドリングの統一方針

### 基本方針

- すべての `pub` 関数は `Result<T, E>` を返す（`panic` や `unwrap` ではなく）
- エラー型 `E` は必ず `thiserror::Error` derive の typed error を使う
- `String` エラーを `pub` API に露出させない

### エラー型の定義場所

| 層 | エラー型 | 定義場所 |
|----|---------|---------|
| Handler 層 | `ApiError` | `crates/seiran-api/src/error.rs` |
| ATP コミット | `AtpCommitError` | `crates/seiran-common/src/atp/service.rs` |
| ATP リポジトリ計算 | `RepoError` | `crates/seiran-common/src/atp/repo.rs`（既存） |
| PLC 登録 | `PlcError` | `crates/seiran-common/src/atp/plc.rs`（既存） |
| DB 操作 | `sqlx::Error` をそのまま `#[from]` で包む | 各 Repository の error モジュール |
| ジョブハンドラ | 各ジョブ固有の Error 型 | `crates/seiran-common/src/jobs/{name}.rs` |

### エラー伝播のパターン

```rust
// Good: ? 演算子で伝播させる
pub async fn commit_post(&self, actor_id: i64, ...) -> Result<(), AtpCommitError> {
    let actor = self.actor_repo.find_by_id(actor_id).await?;  // sqlx::Error → AtpCommitError
    let did = actor.at_did.ok_or(AtpCommitError::ActorConfig("at_did が未設定"))?;
    // ...
    Ok(())
}

// Bad: map_err で String に変換する
pub async fn commit_post(...) -> Result<(), String> {
    let actor = self.pool.fetch_one(...).await.map_err(|e| format!("取得失敗: {}", e))?;
    // ...
}
```

### `ApiError` の `IntoResponse` 実装

`ApiError` は `axum::response::IntoResponse` を実装し、適切な HTTP ステータスコードにマップする:

```rust
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, message) = match &self {
            ApiError::Unauthorized(msg) => (StatusCode::UNAUTHORIZED, *msg),
            ApiError::NotFound => (StatusCode::NOT_FOUND, "リソースが見つかりません"),
            ApiError::BadRequest(msg) => (StatusCode::BAD_REQUEST, *msg),
            ApiError::Internal(msg) => {
                eprintln!("[ERROR] {}", msg);
                (StatusCode::INTERNAL_SERVER_ERROR, "内部エラーが発生しました")
            }
        };
        (status, message).into_response()
    }
}
```

---

## 4. テストの書き方ガイドライン

### テストファイルの配置

| 対象 | テスト配置場所 |
|------|--------------|
| 純粋計算関数（repo.rs 等） | 同ファイル末尾の `#[cfg(test)] mod tests { ... }` |
| Repository 実装 | `tests/` ディレクトリ（`sqlx::test` を使った統合テスト） |
| Service 層 | 同ファイル末尾の `#[cfg(test)]`（Mock Repository を使う） |
| Handler 層 | `crates/seiran-api/tests/` ディレクトリ |

### ユニットテストの書き方（純粋計算）

```rust
// crates/seiran-common/src/atp/repo.rs の末尾
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encode_bsky_feed_post_deterministic() {
        let (cbor1, cid1) = encode_bsky_feed_post("hello", "2024-01-01T00:00:00.000Z").unwrap();
        let (cbor2, cid2) = encode_bsky_feed_post("hello", "2024-01-01T00:00:00.000Z").unwrap();
        assert_eq!(cbor1, cbor2);
        assert_eq!(cid1, cid2);
    }

    #[test]
    fn test_build_mst_sorted_entries() {
        let (_, cid_a) = encode_bsky_feed_post("a", "2024-01-01T00:00:00.000Z").unwrap();
        let (_, cid_b) = encode_bsky_feed_post("b", "2024-01-01T00:00:00.000Z").unwrap();
        // ソート済みエントリを渡す
        let entries = vec![
            ("app.bsky.feed.post/aaaa".to_string(), cid_a),
            ("app.bsky.feed.post/bbbb".to_string(), cid_b),
        ];
        let (root, blocks) = build_mst(&entries).unwrap();
        assert!(!blocks.is_empty());
        // root が blocks の中に存在する
        assert!(blocks.iter().any(|(cid, _)| *cid == root));
    }
}
```

### Mock Repository の書き方

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    pub struct MockActorRepository {
        actors: Mutex<Vec<LocalActor>>,
    }

    impl MockActorRepository {
        pub fn new(actors: Vec<LocalActor>) -> Self {
            Self { actors: Mutex::new(actors) }
        }
    }

    #[async_trait]
    impl ActorRepository for MockActorRepository {
        async fn find_by_user_id(&self, user_id: i64) -> Result<Option<LocalActor>, sqlx::Error> {
            Ok(self.actors.lock().unwrap()
                .iter()
                .find(|a| a.user_id == user_id)
                .cloned())
        }
        // ...
    }

    #[tokio::test]
    async fn test_note_service_create_post() {
        let actor = LocalActor { user_id: 1, id: 100, username: "alice".to_string(), ... };
        let actor_repo = Arc::new(MockActorRepository::new(vec![actor]));
        let service = NoteService::new(actor_repo, ...);
        let result = service.create(1, "hello world").await;
        assert!(result.is_ok());
    }
}
```

### 統合テスト（DB 接続あり）の書き方

`sqlx::test` マクロを使う（`sqlx` の `features = ["runtime-tokio"]` が必要）。

```rust
// crates/seiran-common/tests/actor_repository_test.rs
#[sqlx::test(migrations = "../../migrations")]
async fn test_pg_actor_repository_find_by_user_id(pool: PgPool) {
    // テスト用データを挿入
    sqlx::query("INSERT INTO users (id, email, ...) VALUES (1, 'test@example.com', ...)")
        .execute(&pool).await.unwrap();

    let repo = PgActorRepository::new(pool);
    let result = repo.find_by_user_id(1).await.unwrap();
    assert!(result.is_some());
}
```

### 非推奨のテストパターン

```rust
// Bad: 実際の外部サービスを呼ぶテスト
#[tokio::test]
async fn test_register_did_plc() {
    let key = SigningKey::random(&mut OsRng);
    // 本物の plc.directory を呼んでいる → 外部依存・副作用あり
    let result = register_did_plc("test", "example.com", &key).await;
    assert!(result.is_ok());
}

// Good: mockito でモックサーバーを使う
#[tokio::test]
async fn test_register_did_plc_with_mock() {
    let mut server = mockito::Server::new_async().await;
    let _mock = server.mock("POST", "/did:plc:...").with_status(200).create_async().await;
    let key = SigningKey::random(&mut OsRng);
    let client = reqwest::Client::new();
    let result = register_did_plc_with_url("test", "example.com", &key, &client, &server.url()).await;
    assert!(result.is_ok());
}
```

---

## 5. 新機能追加時の手順

新しい API エンドポイントを追加する場合は以下の順序で実装する。

### ステップ 1: Repository を実装する

`crates/seiran-common/src/repository/` に必要な trait メソッドを追加または新規 trait を作成する。まずインターフェースを決め、次に `Pg` 実装を書く。

```
crates/seiran-common/src/repository/
└── {entity}.rs   ← trait 定義 + PgXxxRepository 実装
```

### ステップ 2: Service を実装する

ビジネスロジックを Service に書く。Repository を通じて DB にアクセスする。外部 HTTP 呼び出しがある場合は `reqwest::Client` を引数または `Arc<reqwest::Client>` として受け取る。

```
crates/seiran-common/src/
└── {domain}/service.rs   ← XxxService 実装
```

### ステップ 3: Handler を実装する

`crates/seiran-api/src/handlers/` に対応するファイルを作成または既存ファイルに追加する。

```rust
// handlers/{domain}.rs
async fn create_xxx(
    headers: HeaderMap,
    State(state): State<AppState>,
    Json(req): Json<CreateXxxRequest>,
) -> Result<Json<XxxResponse>, ApiError> {
    let auth_user = state.auth.extract(&headers).await?;
    // バリデーション
    if req.field.is_empty() {
        return Err(ApiError::BadRequest("field は空にできません"));
    }
    // Service 呼び出し
    let result = state.xxx_service.create(&auth_user, req).await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    Ok(Json(XxxResponse::from(result)))
}
```

### ステップ 4: ルートを登録する

`crates/seiran-api/src/lib.rs` の `Router::new()` にルートを追加する。

**`/xrpc/*`・`/.well-known/*`（AT Protocol XRPCエンドポイント）を追加する場合、CORS設定は不要**。`lib.rs`のCORS `AllowOrigin::predicate`が`path.starts_with("/xrpc/")`または`/.well-known/`で無条件にオリジンを許可する設計になっており、新規追加したXRPCルートも自動的にこの対象に含まれる（個別ルートごとのCORS設定・許可オリジン追加は不要かつ禁止。bsky.app等の外部ATクライアントがブラウザから直接叩く前提の公開APIのため、AT Protocol関連エンドポイントは常に全オリジン許可が正しい）。

### ステップ 5: `AppState` を更新する

新しい Service が必要な場合は `AppState` に `Arc<XxxService>` を追加し、`main()` 内で初期化する。

### ステップ 6: テストを書く

- Repository の mock テスト（`#[cfg(test)]` 内）
- Service のユニットテスト（mock Repository を使用）
- 統合テストが必要な場合は `tests/` ディレクトリに追加

### ステップ 7: ビルドと設計文書の確認

```bash
cargo build
```

コンパイルエラーがないことを確認した後、CLAUDE.md のルールに従い対応する設計文書を更新する。

---

## 6. ファイル・モジュール命名規則

| 種別 | 命名 | 例 |
|------|------|----|
| Handler ファイル | 機能ドメイン名 | `handlers/auth.rs`, `handlers/notes.rs` |
| Service 構造体 | `{Domain}Service` | `AtpCommitService`, `NoteService` |
| Repository trait | `{Entity}Repository` | `ActorRepository`, `PostRepository` |
| Repository 実装 | `Pg{Entity}Repository` | `PgActorRepository` |
| エラー型 | `{Domain}Error` | `AtpCommitError`, `PlcError`, `ApiError` |
| リクエスト型 | `{Action}{Entity}Request` | `CreateNoteRequest`, `RegisterRequest` |
| レスポンス型 | `{Entity}Response` | `NoteResponse`, `AuthResponse` |

---

## 7. `AppState` の設計ルール

`AppState` は axum の `State` エクストラクタで各ハンドラに渡される。以下のルールを守る。

```rust
#[derive(Clone)]
pub struct AppState {
    // Repository 直参照は禁止。Service 経由でアクセスする。
    // pub db: PgPool,  ← NG（外からは見えない）

    // OK: Service を Arc で持つ
    pub auth_service: Arc<AuthService>,
    pub note_service: Arc<NoteService>,
    pub atp_service: Arc<AtpCommitService>,

    // OK: 認証プロバイダー（Service ではなくミドルウェア相当）
    pub local_auth: Arc<LocalAuthProvider>,

    // OK: WebSocket ブロードキャスト（インフラ層）
    pub atp_event_tx: Arc<broadcast::Sender<AtpCommitEvent>>,

    // OK: 設定値（変更されない）
    pub local_domain: String,
    pub secrets: Arc<Secrets>,

    // OK: HTTP クライアント（外部 API 呼び出し用、再利用のため共有）
    pub http_client: Arc<reqwest::Client>,
}
```

`PgPool` は `AppState` の `pub` フィールドに置かない。Repository の `impl` 内にのみ閉じ込める。

---

## 8. 依存関係の方向

```
seiran-api
    └─ depends on ─→ seiran-common

seiran-common
    └─ depends on ─→ (外部クレート: sqlx, reqwest, tokio, p256, ...)

seiran-api
    └─ NEVER depends on ─→ seiran-api (循環禁止)
```

`seiran-api` が `sqlx` を Cargo.toml に持つこと自体は現状許容するが、  
**ハンドラ内での直接使用は禁止**（Handler → Repository を経由すること）。  
中長期的には `seiran-api/Cargo.toml` から `sqlx` 依存を削除することを目標とする。

---

## 9. ログ出力の方針

現在 `eprintln!` を使っているが、以下のフォーマットを守る（将来 `tracing` クレートへの移行を想定）。

```rust
// フォーマット: [モジュール名] メッセージ: 詳細
eprintln!("[atp] commit 完了: at_uri={}, cid={}", at_uri, commit_cid_str);
eprintln!("[register] ハッシュ失敗: {}", e);

// エラーログは必ず e（エラー内容）を含める
eprintln!("[create_note] INSERT 失敗: {}", e);  // Good
eprintln!("[create_note] INSERT 失敗");          // Bad（原因不明）
```

成功パスのログは `[モジュール名]`、失敗パスのログは `[ERROR][モジュール名]` プレフィクスを使うことを推奨する（将来の `tracing` 移行時に level に対応させやすくなる）。

---

## 10. `seiran-federation-inbox` 固有ルール

`seiran-federation-inbox` は ActivityPub の受信エンドポイント専用クレートである。以下のルールを守る。

### AppState

```rust
pub struct AppState {
    pub db: PgPool,           // Repository 移行完了後は非公開にする
    pub http_client: Arc<reqwest::Client>,   // AP 配送・fetch に使う
    pub secrets: Arc<seiran_common::Secrets>, // AP 秘密鍵はここから取得
    pub local_domain: String,
}
```

`ap_private_key_pem: String` のような bare String フィールドは使わない。`Secrets` 経由で取得する。

### エラー型

ハンドラの内部関数は `Result<T, FederationError>` を返す。`FederationError` は `crates/seiran-federation-inbox/src/error.rs` に定義する。

```rust
#[derive(Debug, thiserror::Error)]
pub enum FederationError {
    #[error("DB エラー: {0}")]
    Db(#[from] sqlx::Error),
    #[error("AP エラー: {0}")]
    Ap(String),
    #[error("フィールド不足: {field}")]
    MissingField { field: &'static str },
    #[error("署名検証失敗")]
    SignatureInvalid,
}
```

Axum のハンドラ（`async fn inbox_handler(...) -> impl IntoResponse`）は `FederationError` を `StatusCode` にマップして返す。

### HTTP 署名検証

`verify_http_signature` は必ず inbox 受信時に呼ぶ。検証をスキップした場合は `[WARN]` ログを出力し、将来的には必須エラーにする。

### ファイル分割規則

`main.rs` が 100 行を超えた時点でハンドラを `handlers/` に移動すること。1 ファイルの上限は 200 行。

## 4. コメントの書き方

実装意図が自明でない箇所（隠れた制約・不変条件・特定バグの回避策・読み手が驚く挙動）には理由を
1行で書く。`#NNN`（GitHub issue 番号）だけを書いて済ませない — issue 番号は「なぜそのコードが
あるか」を説明しない（GitHub を開かないと分からない上、番号だけでは検索性も低い）。理由そのものを
書いた上で、参照として issue 番号を併記するのはよい。既存コメントの一斉置換は不要、触った箇所から
直す。

## 5. フロントエンド: テキスト入力要素の `font-size`

`input[type="text"|"search"|"email"|"url"|"tel"|"password"|"number"]` や `textarea` など、
テキストカーソルが立つフォーカス可能な入力要素の `font-size` は必ず 16px 相当（ルート
font-size が 16px の環境では `1rem`）以上にする。iOS Safari はこれらの要素にフォーカスした
際、`font-size` が 16px 未満だとページ全体を自動的にズームインし、ユーザーが手動で戻すまで
UI が崩れて見える。見た目を小さくしたい場合は `font-size` を下げるのではなく `transform:
scale()` 等で対処する。

`type="radio"`/`type="checkbox"`/`type="date"`/`type="datetime-local"`/`<select>` はネイティブ
ピッカー UI になりテキストカーソルを持たないため、この制約の対象外。

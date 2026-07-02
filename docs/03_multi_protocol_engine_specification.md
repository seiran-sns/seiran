# Doc 3. マルチプロトコル・同期 ＆ バックグラウンド通信エンジン仕様 (Multi-Protocol Engine & Background Process)

## 1. フォロー時の非同期フルインポート（初期同期）

ユーザーが外部アクターをフォローした際、ユーザー体験（APIレスポンス）をブロックしないよう、フォロー関係の保存のみを即座に完了し、過去ログのインポートはバックグラウンドジョブ（`actor_history_sync` キュー）に移譲する。

### 1.1 AT Protocol (Bluesky) 同期シーケンス（外部アクターの過去ログ取得）

> **注意**: ここで説明するのは「外部 Bluesky ユーザーをフォローしたときの過去ログ取得」である。
> seiran ローカルユーザーの投稿配信（seiran PDS → Relay）は [2. バックグラウンド・キュー設計] の
> `atp_repository_publish` を参照。

1. フォローAPIの正常完了後、`actor_history_sync` キューに対象アクターの `at_did` をジョブ引数としてエンキューする。
2. ワーカーは、Bluesky 公開 AppView（`public.api.bsky.app`）の `app.bsky.feed.getAuthorFeed` を呼び出す（認証不要）。
3. レスポンスの `cursor` を用いて、過去方向へページネーションループを回す。
4. **インポート制限（上限キャップ）:** ストレージ圧迫と外部APIレートリミット消費を防ぐため、**「過去30日間」または「最大300件」**のどちらか早い方に達した時点でループを安全にブレイクする。
5. 取得したポストは、`posts` テーブルへインサートする（`at_uri` / `at_cid` に格納）。

### 1.2 ActivityPub (Fediverse) 同期シーケンス
1. `actor_history_sync` キューに対象アクターの `ap_uri`（Webfinger解決済み）をジョブ引数としてエンキューする。
2. ワーカーは、対象アクターオブジェクトの `outbox` コレクションのURLを取得し、HTTP GET リクエストを送信する。
3. `outbox` 非公開や過去ログ配送拒否の場合は **ベストエフォート（失敗してもジョブを正常終了とし、フォローは維持）** として扱う。
4. 取得可能な場合、ATPと同様に「過去30日間 / 最大300件」のキャップをかけ、ドメイン単位の同時実行制限（Concurrency Limit）を守りつつフェッチし、パースしてローカルDBへインサートする。

### 1.3 ノート詳細画面からのオンデマンド同期

フォローしていないリモートユーザーのノート詳細画面を表示する際に、周辺投稿を同期取得するシナリオ。

1. フロントエンドが `GET /api/notes/:id/context` を呼び出す。
2. APIはそのノートの投稿者が **リモートアクター** であることを確認する。
3. リクエスト元が認証済みかつ対象アクターをフォロー中（`follows` テーブルで `status = 'accepted'`）の場合 → Inbox 経由で投稿が届いているため追加フェッチは行わない。
4. 未認証 or 未フォローの場合 → AP Outbox から最大50件・過去30日分を **5秒タイムアウト付きで同期フェッチ** し、`posts` テーブルに `ON CONFLICT (ap_object_id) DO NOTHING` でインサート。
5. DB からノードID前後の投稿を各10件取得してレスポンスとして返す。
6. Outbox フェッチが失敗・タイムアウトした場合は DB にある分だけを返す（ベストエフォート）。

> **注意**: このシナリオは「過去30日間 / 最大50件」で打ち切り、ドメイン単位の同時実行制限は適用しない（ユーザーアクションに直結するため短時間で完結させる）。大量フェッチが必要な場合は「1.1/1.2 フォロー時同期」を使う。

### 1.4 リモート seiran ユーザー同士の特権初期同期シーケンス
1. フォロー対象アクターが `remote_seiran` であると判定（Bioシグネチャおよびハンドシェイク検証完了）した場合、通常の外部プロトコル（AP/ATP）経由での低速フェッチは行わない。
2. 相手方サーバーの seiran 特権同期エンドポイント（例: `/api/seiran/v1/posts/export`）に対し、認証トークンを伴って直接リクエストを送信する。
3. 相手方バックエンドは、そのユーザーが保持している「オリジナル投稿プレーンテキスト（body）」と「プロトコル別変形レシピ（metadata）」を、生データのままバルク（最大300件）で一括返却する。
4. 自サーバーはこれを `posts` テーブルにそのまま格納する。

---

## 2. バックグラウンド・キュー設計 (`JobQueue` の分類)

システム全体の分散通信および同期を支えるため、バックエンドに以下の5つの非同期キューを実装する。これらは `JobQueue` Trait で抽象化され、オンメモリでの実装からRedis（`apalis` 等）への移行を容易にする。

| キュー（ジョブ）名 | 処理内容 | 優先度 | リトライ & レート制限（レートリミット）戦略 |
| :--- | :--- | :--- | :--- |
| **1. 過去ログ同期キュー**<br>`actor_history_sync` | 新規フォローされたアクターの過去ログ（最大300件）をページネーションしながら順次取得・DB保存する。 | **低 (Low)** | ・**ドメイン単位の同時実行制限 (Concurrency Limit)**: 同一APインスタンスに対する同時フェッチを1件に制限。<br>・リクエスト間に 1〜3秒のジッター（揺らぎ）を挿入。<br>・失敗時は指数バックオフで最大3回リトライ。 |
| **2. 投稿配送キュー**<br>`outbound_post_delivery` | 自サーバーで作成された投稿を、APフォロワーの各リモートサーバー（Inbox）へ配送する（APのみ。ATPはPDS自律処理のため不要）。 | **高 (High)** | ・ユーザーのアクションに直結するため、最高優先度で即時実行。<br>・相手サーバーのダウン時は、数時間にわたる指数バックオフで最大10回程度リトライ（段階的に間隔を広げる）。 |
| **3. 配送受け入れ（インバウンド）キュー**<br>`inbound_activity_process` | 外部（APのInbox等）から届いたアクティビティ（投稿、リアクション等）を非同期でパースする。Note（ポスト）本体のフェッチ、未知アクターの解決、関連する親スレッドの補完フェッチ、DB保存処理など。 | **中 (Medium)** | ・外部サーバーへのフェッチ（Noteの解決やアクター解決）が頻発するため、ドメイン単位のレート制限を適用。<br>・依存リソース（アクターや親投稿）が一時的に解決できない場合、短いスパンで再スケジュール（指数バックオフで最大5回リトライ）。 |
| **4. アクター検証・メタデータ取得キュー**<br>`actor_metadata_resolve` | リモートseiranアクターのハンドシェイク検証（`verify-actor`）や、Webfingerによる解決、アバター画像やOGP情報のプロキシ・キャッシュ化。 | **中 (Medium)** | ・タイムアウト時は短いスパンで再試行（最大3回）。<br>・画像取得失敗等は非クリティカルとして扱い、デフォルト画像でフォールバック。 |
| **5. ATPリポジトリコミット・配信キュー**<br>`atp_repository_publish` | seiran PDS としてローカルユーザーの AT Protocol リポジトリを更新する。具体的には、投稿・削除等のレコードを DAG-CBOR 形式で MST にコミットし、P-256 秘密鍵で署名した後、Relay サーバーへ配信する。外部 PDS（bsky.social 等）は使用しない。 | **極高 (Critical)** | ・順序性の維持が不可欠（同じDIDに対するコミット順が前後してはならない）。<br>・アクターID単位の **FIFO（先入れ先出し）** キュー、またはシリアル排他ロック制御が必要。 |

---

## 3. リモート seiran ユーザー専用ハンドシェイク（ゼロトラスト検証）

プロフィール文字列に書かれたシグネチャ（公開情報）のなりすましを防ぐため、バックエンド間で直接答え合わせを行う。

### 3.1 ペアリング検証フロー
1. 外部アクター（例: Fedi宇宙の `hanako@aozora.social`）をインポートする際、Bio文末に `seiran_signature: [ATP_DID]` というパターンを検出する。
2. これをトリガーとし、`seiran` バックエンドは `hanako@aozora.social` のドメイン（`aozora.social`）に対して、**seiran専用の検証エンドポイント**（例: `/.well-known/seiran/verify-actor`）を直接叩きに行く。
3. **エンドポイントの応答:**
   相手側サーバーの `seiran` エンジンが、「はい、当サーバーの `hanako` は、確かにそのBluesky DID（`[ATP_DID]`）と同一人物（同一の魂）です」という暗号署名付きJSON、または相互参照のステータスを返却する。
4. 検証が100%成功した場合に限り、ローカルDBの `actor_type` を `remote_seiran` に昇格させ、`seiran_pair_actor_id` を結合する。検証失敗、またはタイムアウトした場合は、単なる独立した `fedi` ユーザーとして安全にフォールバックする。

---

## 4. 重複投稿の識別・マージとリンク設計（水際防御）

APとATPが絡み合うマルチプロトコル環境において、同じ内容の投稿が複数経路から届く重複現象に対し、シナリオ別の水際防御およびマージを行います。

### 4.1 重複の3大シナリオと処理ルール

#### ① 自サーバー発の投稿の逆輸入（ループバック：シナリオ1）
* **現象**: 自住民の投稿が、外部ブリッジ（Brid.gy等）で翻訳され、外部プロトコルを経由して再び自サーバーに新着として戻ってくる現象。
* **処理ルール（重複許容 ＆ リンク）**: レコード自体は破棄せず、DBに受け入れます。ただし、インサート時に `parent_original_post_id` カラムに自サーバー内のオリジナル投稿IDを格納し、関係をハードリンクします。

#### ② 他seiranサーバーアクターからのマルチプロトコル投稿（シナリオ2）
* **現象**: 他のseiranユーザー（AP版とATP版の肉体が魂のペアリング済み）の投稿が、AP直接とATP直接の両方の経路で配送され、2重に受信する現象。
* **処理ルール（1レコードへのマージ）**: タイムラインの重複を完全に防ぐため、**DB上で1つのレコードにマージ（統合）**します。
  1. 送信側 `seiran` は、投稿作成時に一意の `seiran_post_uuid` を生成し、APのNoteカスタム拡張フィールド、およびATPのPostレコードのカスタムフィールドの双方に同じUUIDを埋め込んで送信します。
  2. 受信側 `seiran` は、投稿受信時に `seiran_post_uuid` を検索。
  3. 未登録なら新規インサート（受信した側のプロトコルIDを埋める）。
  4. すでに登録済みなら新規インサートは行わず、**既存レコードに対して、新しく届いた方のプロトコルID（`ap_object_id` または `at_uri`/`at_cid`）を `UPDATE` で追記書き込み**し、1つの統合レコードに融合させます。

#### ③ 一般ブリッジユーザーの投稿（シナリオ3）
* **現象**: 一般のAPまたはATPユーザーの投稿が、外部ブリッジによって他方に投影され、その両方を自サーバーが受信する現象。
* **処理ルール（重複許容 ＆ リンク）**: 重複したままDBに受け入れます。インサート時に、ブリッジが変換時にメタデータに埋め込んでいるオリジナル投稿URL（`source_url`等）を辿り、すでにオリジナル（本尊）がDBに存在すれば、`parent_original_post_id` に本尊の投稿IDをセットしてハードリンクを張ります。

---

## 6. 配信先トグル仕様

投稿作成時にユーザーが独立して ON/OFF できる 2 つのトグルで配信先を制御する。

### 6.1 フィールド定義

| フィールド | 型 | 既定値 | 説明 |
| :--- | :--- | :--- | :--- |
| `deliver_to_fedi` | `bool` | `true` | ActivityPub（Fediverse）フォロワーへ配送するか |
| `deliver_to_bsky` | `bool` | `true` | AT Protocol Relay へコミットするか |

### 6.2 組み合わせと文字数制限

| `deliver_to_fedi` | `deliver_to_bsky` | 配信先 | UTF-8 バイト上限 | Grapheme 上限 |
| :--- | :--- | :--- | :--- | :--- |
| OFF | OFF | ローカルのみ | 10,000 | 3,000 |
| ON  | OFF | Fediverse のみ | 10,000 | 3,000 |
| OFF | ON  | Bluesky のみ | 3,000 | 300 |
| ON  | ON  | 両方 | 3,000 | 300 |

`deliver_to_bsky` が ON のときのみ厳しい制限が適用される。

Grapheme カウントは `Intl.Segmenter`（フロントエンド）および `unicode-segmentation` クレート（バックエンド）を使用する。
バイト数カウントは `TextEncoder`（フロントエンド）および `str::len()`（バックエンド、UTF-8 ネイティブ）を使用する。

### 6.3 バックエンド処理条件

| 処理 | 実行条件 |
| :--- | :--- |
| ATP リポジトリコミット（`atp_repository_publish` キュー） | `deliver_to_bsky == true` |
| AP 配送（`outbound_post_delivery` キュー） | `deliver_to_fedi == true` |

### 6.4 フロントエンド残り文字数表示ロジック

残り文字数は以下の式で算出し、負の値になった場合は投稿ボタンを無効化して黄色ガイドメッセージを表示する。

```
remaining = min(maxGraphemes - graphemes, floor((maxBytes - bytes) / 3))
```

`deliver_to_bsky` ON の場合（厳しい制限）:
```
remaining = min(300 - graphemes, floor((3000 - bytes) / 3))
```

`deliver_to_bsky` OFF の場合（緩い制限）:
```
remaining = min(3000 - graphemes, floor((10000 - bytes) / 3))
```

---

## 7. Bluesky 向けメンション変換（Facet 生成）

Bsky 配信モード（`deliver_to_bsky = true`）での投稿本文には、AT Protocol の `RichText` Facet としてメンションを埋め込む必要がある。そのため投稿本文中の `@xxx` 形式の識別子を Bsky ハンドルに変換してから `commit_post` に渡す。

### 7.1 変換ルール

#### 1. ローカルユーザー `@yuba`（ドメイン部なし）

- `@yuba.{LOCAL_DOMAIN}` に展開（例: `@yuba.beta.seiran.org`）
- `actors` テーブルで `username = 'yuba'` かつローカルアクターであることを確認してから置換

#### 2. Fediverse リモートユーザー `@yuba@reax.work`

- brid.gy Federated Bridge のハンドル命名規則: `{username}.{instance}.ap.brid.gy`
  例: `@yuba@reax.work` → `yuba.reax.work.ap.brid.gy`
- `GET https://bsky.social/xrpc/com.atproto.identity.resolveHandle?handle=yuba.reax.work.ap.brid.gy` でハンドル解決を試みる
- 解決成功（200 + did 返却）→ `@yuba.reax.work.ap.brid.gy` に置き換え
- 解決失敗（404 等）→ URL リンク `https://reax.work/@yuba` に変換

### 7.2 文字数の考慮

変換後の本文でバイト数・grapheme 数がチェックされる。変換によって文字数が増加するため、UI 側カウントでは余裕があっても API が `TEXT_TOO_LONG` を返す場合がある（クライアントはこのエラーを受け取り表示する実装済み）。

### 7.3 処理タイミング

`create_note` ハンドラ内でバリデーション後・`commit_post` 呼び出し前に変換を実施する。`deliver_to_bsky = false` の場合は変換不要。

---

## 5. Misskey 互換レイヤー仕様（MiAuth & `/api/meta`）

Aria・Miria・ZonePane 等の Misskey クライアントから seiran を Misskey サーバーとして利用可能にするための互換エンドポイント仕様。

### 5.1 サーバー検出エンドポイント: `POST /api/meta`

Misskey クライアントは、ホスト名を入力した直後に `POST /api/meta` を呼び出してサーバー種別を判定する。
`features.miauth` が `true` でなければ、クライアントは MiAuth フローへ進まずトークン手動入力画面を表示する。

**リクエスト**: `POST /api/meta`  
**ボディ**: `{}` （空 JSON）

**レスポンス（最小構成）**:
```json
{
  "uri": "https://seiran.example.com",
  "name": "seiran",
  "version": "0.1.0",
  "features": {
    "registration": true,
    "miauth": true
  }
}
```

| フィールド | 必須 | 説明 |
|---|---|---|
| `features.miauth` | **必須** | `true` でなければ MiAuth フローに進まない |
| `features.registration` | 推奨 | アカウント登録が可能かどうか |
| `uri` | 推奨 | クライアントが URL 正規化に使用する |
| `name` | 推奨 | サーバー名（クライアント UI に表示） |
| `version` | 推奨 | バージョン文字列 |

### 5.2 MiAuth セッション開始

クライアントは以下の URL を WebView/ブラウザで開き、ユーザーに認可を求める。

```
GET /miauth/{sessionId}
  ?name={アプリ名}
  &permission={カンマ区切りの権限リスト}
  [&callback={コールバック URL}]   ← Android のみ
```

- `sessionId`: クライアントが生成する UUID v4
- `name`: クライアントアプリ名（例: `"Aria"`）
- `permission`: カンマ区切りの権限文字列（Aria は 39 種送信。seiran は現状無視して全権付与）
- `callback`: 認可完了後のリダイレクト先（例: `aria://aria/miauth`）。省略時は HTML で「認可されました」を表示

**現在の seiran 実装**:  
`GET /miauth/:session_id?name=...&callback=...` を受け付け、HTML の認可ページを返す。
`permission` クエリパラメータは `MiAuthQuery` 構造体に存在しないため無視される（許容）。

### 5.3 MiAuth 完了確認: `POST /api/miauth/{sessionId}/check`

ユーザーが認可ボタンを押した後、クライアントはこのエンドポイントをポーリングして結果を取得する。

**リクエスト**: `POST /api/miauth/{sessionId}/check`  
**ボディ**: なし

**レスポンス（認可済み）**:
```json
{
  "ok": true,
  "token": "<アクセストークン文字列>",
  "user": {
    "id": "123456789",
    "name": "表示名",
    "username": "username",
    "host": null,
    "avatarUrl": null
  }
}
```

**レスポンス（未認可・セッション未完了）**:
```json
{ "ok": false }
```

> **注意**: 現在の seiran 実装は `POST /api/miauth/check`（ボディに `{"session": "..."}` を受け取るパス固定形式）になっている。
> Aria が期待する `POST /api/miauth/{sessionId}/check`（パスにセッション ID）とは URL が異なるため、
> ルーティングの修正が必要（フェーズ 2.4 で対応予定）。

### 5.4 メールアドレス確認フロー（Email Verification）

ユーザー登録は「メールアドレス入力 → 確認メールクリック → パスワード等の残情報入力」の 2 ステップ構成とする。

**フロー**:
1. クライアントが `POST /api/auth/verify-email` にメールアドレスを送信
2. サーバーが `email_verifications` テーブルに確認トークン（UUID, TTL 24h）を保存し、確認メールを送信
3. ユーザーが確認メールのリンク（`GET /auth/verify?token=...`）をクリック
4. トークンが有効なら、その `token` をクライアントに返す（またはセッションに記録）
5. クライアントが `POST /api/auth/register` に確認済みトークン + パスワード + ユーザー名を送信して登録完了

**追加テーブル** (`email_verifications`):
```sql
CREATE TABLE email_verifications (
  id         BIGINT PRIMARY KEY,
  email      TEXT NOT NULL,
  token      UUID NOT NULL UNIQUE DEFAULT gen_random_uuid(),
  expires_at TIMESTAMPTZ NOT NULL DEFAULT now() + INTERVAL '24 hours',
  verified_at TIMESTAMPTZ,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
```

**SMTP 設定（`.env`）**:
```env
SMTP_HOST=smtp.example.com
SMTP_PORT=587
SMTP_USERNAME=no-reply@example.com
SMTP_PASSWORD=your_password
SMTP_FROM=no-reply@example.com
SMTP_TLS=starttls   # starttls | tls | none
```

Rust 実装は `lettre` クレートを使用する（`lettre = { version = "0.11", features = ["tokio1", "smtp-transport"] }`）。

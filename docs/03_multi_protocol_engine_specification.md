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
- ブリッジユーザーの探索は以下の2段階で行う:
  1. **DB ルックアップ（第1段階）**: `actors` テーブルで `username = 'yuba'` かつ `domain = 'reax.work'` に対応するブリッジアクター（`at_identifier` または `ap_uri` が brid.gy ドメインを持つ）をまず探す
  2. **外部問い合わせ（第2段階）**: DB に存在しない場合のみ、`GET https://bsky.social/xrpc/com.atproto.identity.resolveHandle?handle=yuba.reax.work.ap.brid.gy` でハンドル解決を試みる
  3. **タイムアウト**: 外部問い合わせは **2秒** でタイムアウトし、失敗時は URL リンクフォールバックに移行する。投稿体験を損なわないための上限。
- 解決成功（200 + did 返却）→ `@yuba.reax.work.ap.brid.gy` に置き換え
- 解決失敗（404 等またはタイムアウト）→ URL リンク `https://reax.work/@yuba` に変換

### 7.2 文字数の考慮

変換後の本文でバイト数・grapheme 数がチェックされる。変換によって文字数が増加するため、UI 側カウントでは余裕があっても API が `TEXT_TOO_LONG` を返す場合がある（クライアントはこのエラーを受け取り表示する実装済み）。

### 7.3 処理タイミング

`create_note` ハンドラ内でバリデーション後・`commit_post` 呼び出し前に変換を実施する。`deliver_to_bsky = false` の場合は変換不要。

---

## 8. AP（Fediverse）向け ATP 形式 ID 変換

Fedi 配信モード（`deliver_to_fedi = true`）での投稿本文に `@yuba.bsky.social` のような ATP ハンドル形式の識別子が含まれる場合、AP では直接メンションとして扱えないため変換処理が必要。

### 8.1 変換ルール

**ATP ハンドルの検出**:  
`@` が1つで後続がドメイン形式（例: `@yuba.bsky.social`、`@alice.bsky.team`）をパターンとして検出。`@user@domain` の Fediverse 形式と区別する。

**2段階ブリッジ探索**（タイムアウト: 2秒）:

1. **DB ルックアップ（第1段階）**: `actors` テーブルで ATP ハンドルに対応するブリッジアクターを探す。brid.gy 経由でインポート済みのアクターは `domain = 'bsky.brid.gy'`、`username = 'yuba.bsky.social'` 等で登録されている想定。
2. **外部 WebFinger（第2段階）**: DB に存在しない場合、`@yuba.bsky.social@bsky.brid.gy` の WebFinger（`https://bsky.brid.gy/.well-known/webfinger?resource=acct:yuba.bsky.social@bsky.brid.gy`）で AP アクターの存在を確認する。

**解決成功時**: 投稿本文の `@yuba.bsky.social` を `@yuba.bsky.social@bsky.brid.gy` という AP メンション形式に置き換える。

**解決失敗時（フォールバック）**: AP プロトコルに URL リンク概念はないが、`content` フィールドは HTML なので `<a>` タグとして埋め込める。本文を Markdown 形式 `[yuba.bsky.social](https://bsky.app/profile/yuba.bsky.social)` に変換し、`plain_to_html` の Markdown → HTML 変換で `<a href="...">` タグになる。

### 8.2 処理タイミング

`deliver_post_to_ap_followers` 呼び出し前に変換を実施。`deliver_to_fedi = false` の場合は変換不要。

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

> **現在の seiran 実装**: `POST /api/miauth/{sessionId}/check`（Aria 等が期待するパスベース形式）と
> `POST /api/miauth/check` + `{"session": "..."}`（seiran 独自フロントエンドが使うボディベース形式、
> 後方互換として残置）の両方を提供する（フェーズ 2.4 で対応済み）。

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

### 5.5 互換性監査（2026-07-13）と対応状況

フロントエンド向け API 全体を実際の Misskey API 仕様と突き合わせた監査を実施した。
結論: **MiAuth ログイン導線と `/api/meta` によるサーバー検出は互換だが、ログイン後の実操作
（ノート作成・タイムライン取得・リアクション・フォロー等）は Misskey の実際のワイヤープロトコル
とは非互換**。無改造の Aria/Miria/ZonePane 等はログインはできても、その後の画面はほぼ動作しない。
主な非互換点（詳細はフェーズ 7.4 のバックログを参照）:

- **認証方式**: Misskey は `i`（アクセストークン）を JSON ボディ or クエリに含める。seiran は
  `Authorization: Bearer` ヘッダーのみを見る（`middleware/auth.rs::extract_auth`）。
- **HTTPメソッド**: Misskey の API エンドポイントは原則すべて `POST`（`GET`/`DELETE` は使わない）。
  seiran はタイムライン系を `GET`、リポスト取消・リアクション取消を `DELETE` で実装している。
- **レスポンス形状**: Misskey の `Note`/`UserLite` スキーマ（`visibility`, `cw`, `reactions`
  （絵文字→件数のマップ）, `renoteCount`, `user.avatarUrl`, `user.host` 等）と seiran の
  `NoteResponse`/`NoteUserInfo`（配列ベースの `reactions`、`domain` フィールド等）は別物。
- **未実装の標準エンドポイント**: `/api/i`, `/api/users/show`, `/api/notes/show`,
  `/api/notes/reactions/create`, `/api/notes/reactions/delete`, `/api/following/create`,
  `/api/following/delete`, `/api/i/notifications`, `/api/notes/unrenote` など。
- **ストリーミングプロトコル**: Misskey はチャンネル購読方式（`connect`/`channel`）、seiran は
  認証ユーザー宛てブロードキャストのみの単純な WebSocket。

**今回の監査で追加・修正した互換性向上（既存フロントエンドへの破壊的変更なし）**:

- `GET /api/emojis`（新規・未認証）: Misskey クライアントの絵文字ピッカーが参照するカスタム
  絵文字一覧を Misskey 互換の形状（`{emojis: [{id, aliases, name, category, host, url, license}]}`）
  で公開する（`handlers/emojis.rs`）。従来は `/api/admin/emojis`（要 admin 認証）でしか取得できず、
  一般ユーザー・第三者クライアントからは参照不能だった。
- `POST /api/meta` に `emojis`（上記と同じ絵文字一覧）・`maxNoteTextLength`
  （`notes::BSKY_MAX_TEXT_GRAPHEMES` と同期）・`disableRegistration` を追加。
- エラーレスポンスに Misskey 風の入れ子 `error: {code, message}` を追加（既存の平坦な `code`
  フィールドは後方互換のためそのまま維持）。`id`/`kind`（Misskey のエラー種別 UUID）は
  seiran 側にレジストリが無く偽装になるため付与していない。
- 5.3 の「ルーティング修正が必要」という記載は、パスベース `check` エンドポイントの実装により
  解消済みだったため、注記を実装済みに更新した。

**マイケルの方針決定（2026-07-13）**: フル互換を目指す。最終的には Misskey スキーマへ統一するが、
移行中は既存のカスタム API・自社フロントエンド（`frontend/`）を無理に同時に壊さず、有利なら
並存させてよい。Misskey 本家の設計自体に非効率・不自然な HTTP メソッド等がある箇所は、互換用
エンドポイントと実用エンドポイントの両方を持たせる判断も許容する。これを受けて Phase 1・2 を
本セッションで実装した（詳細は §5.6）。Phase 3（ストリーミングのチャンネル購読化）・Phase 4
（フロントエンドの追従改修・旧エンドポイント整理）は未着手（フェーズ 7.4 バックログ）。

### 5.6 Phase 1・2 実装内容（2026-07-13）

**Phase 1 — `i` トークン認証ブリッジ**: 新設ミドルウェア `middleware::misskey_auth_bridge::bridge`
をルーター最前段に追加。`Authorization` ヘッダーが無いリクエストに限り、JSON ボディの `i` または
クエリの `i` を検出し、`Authorization: Bearer <token>` ヘッダーを合成してから次段へ渡す。
既存の `extract_auth` 呼び出し箇所（約15箇所）は無改修のまま `i` 認証を受理できるようになった。
ボディは `axum::body::to_bytes`（上限2MB）で一度バッファし、同じバイト列で `Body` を再構築して
ダウンストリームの `Json<T>` extractor に渡す。優先順位はヘッダー＞ボディ`i`＞クエリ`i`。

**Phase 2 — Misskey 準拠エンドポイントの追加（既存 API と並存）**: `handlers::misskey` モジュール
配下に、実際の Misskey パス・POSTオンリー規約・JSONスキーマに合わせた**新規**エンドポイントを
追加した（既存のカスタムエンドポイントは変更・削除していない）。

| 追加エンドポイント | 内容 |
|---|---|
| `POST /api/i` | 自分自身の `UserDetailed` |
| `POST /api/users/show` | `{userId}` または `{username,host}` でユーザー取得 |
| `POST /api/notes/show` | `{noteId}` で単一ノート取得（`MisskeyNote` 形状） |
| `POST /api/notes/local-timeline` / `POST /api/notes/timeline` | ボディで `limit`/`sinceId`/`untilId` を受理するタイムライン（既存の `GET` 版は残置） |
| `POST /api/notes/reactions/create` / `delete` | `{noteId, reaction}` / `{noteId}`。成功時 `204 No Content` |
| `POST /api/notes/unrenote` | `{noteId}`。成功時 `204 No Content` |
| `POST /api/following/create` / `delete` | `{userId}`（seiran の actors.id）。成功時 `204 No Content` |

書き込み系（リアクション・アンリノート・フォロー）は既存の `handlers::notes`/`handlers::follows`
の関数を直接呼び出し、AP/ATP配送・ストリーミング配信などの副作用ロジックをそのまま再利用する
（レスポンスだけ Misskey 流に整形）。読み取り系は `handlers::misskey::convert` が
`TimelinePost`/`Actor` から `MisskeyNote`/`MisskeyUserLite`/`MisskeyUserDetailed`
（`handlers::misskey::types`）を組み立てる。renote数/reply数は既存リポジトリに集計メソッドが
無いため `convert::fetch_counts_map` で都度 SELECT している。

**既知の簡略化・未対応点**（いずれも「Phase 2 は完全再現ではなく実用最小部分集合」という前提で
意図的に許容している）:
- `visibility` は常に `"public"`（seiran はまだ公開範囲の概念を持たない）
- `cw` は常に `null`（Content Warning 未対応）
- リアクション作成/削除・アンリノート・フォロー作成/削除のエラーレスポンスは Misskey 本家の
  エラーID体系ではなく、既存の seiran `ApiError` 形状をそのまま透過する
- `emojis`（本文中のカスタム絵文字インライン表示用マップ）は常に空

### 5.7 実機（Aria）デバッグで判明した MiAuth の実装不備（2026-07-13）

Aria（third-party Misskey クライアント、`poppingmoon/aria`）での実機テストを通じて、Phase 1/2 の
MiAuth 実装に3つの不備が見つかり修正した。いずれも「サーバーは200を返しているのにクライアントが
フリーズする」形で顕在化し、原因究明は Aria 本体・依存パッケージ `misskey_dart`・実物の Misskey
（`/home/yuba/misskey` にローカルチェックアウト済み）のソースを直接読んで特定した。

1. **`/api/miauth/:sessionId/check` の未認可時レスポンスが非2xxだった**: 本家 Misskey は
   未認可・不明セッションでも常に `200 {"ok": false}` を返すが、seiran は `400 Bad Request` を
   返していた。Aria の `check()` 呼び出しは Dio を try/catch 無しで呼ぶ実装（go_router の
   `redirect` コールバック内）のため、非2xx で未処理例外となりアプリがフリーズしていた。
   → 常に `200` を返すよう修正。あわせてセッションを認可成立後に一度きりで消費する
   （本家の `access_tokens.fetched` と同等の一回性）よう変更。
2. **MiAuth が発行する「アクセストークン」が実体のないダミー文字列だった**: 以前は
   `format!("miauth-token-{}", Uuid::new_v4())` という、どこにも登録されないランダム文字列を
   返していた。タイムライン閲覧（未認証で見られる）は動くが、投稿等の要認証操作は
   `extract_auth` がこれを JWT として検証できず 401 になっていた。
   → 自社ログイン（`/api/auth/login`）と同じ `LocalAuthProvider::generate_token` で本物の
   JWT を発行するよう変更（新しいトークン検証経路を追加しない）。既知の制約: アプリ単位の
   失効・権限スコープは自社ログインのトークンと同じく未対応。
3. **`/api/i` が `/api/users/show` と同じ小さいスキーマを返していた**: `misskey_dart` は
   `/api/i` のレスポンスを `MeDetailed`（`UserDetailedNotMe` よりずっと大きい、自分専用の
   スキーマ）としてパースし、`notesCount`/`isModerator`/`isAdmin`/`alwaysMarkNsfw`/
   `carefulBot`/`autoAcceptFollowed` 等を non-nullable 必須として直接キャストする
   （欠けると Dart の生の `TypeError` で未処理例外）。
   → `/api/i` 専用の `MisskeyMeDetailed`（`handlers::misskey::types`）を新設し、
   `notesCount`/`followersCount`/`followingCount` は実集計、`isAdmin`/`isModerator` は
   実際の `users.role` から算出するようにした。`/api/users/show` は引き続き
   `MisskeyUserDetailed`（`UserDetailedNotMe` 相当）のまま。

いずれも `handlers::miauth.rs`・`handlers::misskey::{types,convert,endpoints}` にフィールド名を
固定する回帰テストを追加済み。

---

## 9. リポスト・引用・リプライのクロスプロトコル配信ルール

### 9.0 リポストの重複制約

同一ユーザーが同一ポストを **取り消し（undo）前に再リポストすることはできない**。
DB レベルで `UNIQUE INDEX (actor_id, repost_of_post_id) WHERE deleted_at IS NULL` で強制する。
取り消し（論理削除）後は `deleted_at` が設定されてインデックスから外れるため、再リポストが可能になる。

---

### 9.1 リポストのクロスプロトコル配信マトリクス

UI では Fedi リモートポストをリポストしようとすると、以下の3アクションから選ぶ：

| UI アクション | Fedi 配信 | Bsky 配信 |
|---|---|---|
| **リポスト**（両方） | AP `Announce` を送信 | 後述のブリッジ探索ロジックで配信 |
| **Fediにだけリポスト** | AP `Announce` を送信 | 配信しない |
| **引用** | 後述の引用ロジックで配信 | 後述の引用ロジックで配信 |

Misskey API (`/api/notes/create` に `renoteId` 指定) 経由のリノートは「リポスト（両方）」として扱う。

ポストの種別ごとの配信先：

| 元ポストの種別 | Fedi 配信先 | Bsky 配信先 |
|---|---|---|
| ローカルポスト / リモート seiran ポスト | AP `Announce` | ATP `app.bsky.feed.repost` |
| Fedi リモートポスト | AP `Announce` | ブリッジ探索（9.2節）|
| Bsky リモートポスト | ブリッジ探索（9.3節）| ATP `app.bsky.feed.repost` |

---

### 9.2 Fedi リモートポストを Bsky にリポストする際のブリッジ探索

```
Fedi リモートポストのリポスト → Bsky 配信
  ↓
【第1段階】DB ルックアップ（タイムアウトなし）
  posts テーブルで repost_of_post_id に対応するレコードの at_uri を確認
  （brid.gy 経由でインポート済みのポストは at_uri が設定されている）
  ↓ at_uri あり
  → ATP app.bsky.feed.repost でリポスト
  ↓ at_uri なし
【第2段階】ブリッジユーザー経由の探索（タイムアウト: 2秒）
  1. 元ポストの投稿者（Fedi ユーザー）に対応する brid.gy ブリッジユーザーを探す
     （Section 7 の2段階ルックアップと同じ手順）
  2. ブリッジユーザーが見つかった場合:
     AT Protocol API で当該ユーザーの投稿一覧を取得し、
     ap_object_id に一致するポストの at_uri を探す
  ↓ at_uri 取得成功
  → ATP app.bsky.feed.repost でリポスト
  ↓ 取得失敗 / タイムアウト
【フォールバック】
  → 元ポストの URL を external embed（リンクカード）として持つ新規ポストを Bsky に投稿
     本文例: "🔁 {投稿者表示名}: {元ポストURL}"
```

**タイムアウトと失敗時の動作**
- 外部問い合わせは合計 **2秒以内** でタイムアウトし、フォールバックに移行する
- Bsky 配信の成否にかかわらず、ローカルのリポスト記録（`posts` テーブル）は維持する

**初期リリースの簡易実装**
- 第1段階（DB ルックアップ）のみ実装し、第2段階のブリッジユーザー探索は後続フェーズで追加

---

### 9.3 Bsky リモートポストを Fedi にリポストする際のブリッジ探索

```
Bsky リモートポストのリポスト → Fedi 配信
  ↓
【第1段階】DB ルックアップ（タイムアウトなし）
  posts テーブルで repost_of_post_id に対応するレコードの ap_object_id を確認
  （brid.gy 経由でインポート済みのポストは ap_object_id が設定されている）
  ↓ ap_object_id あり
  → AP Announce アクティビティを送信
  ↓ ap_object_id なし
【第2段階】ブリッジポスト探索（タイムアウト: 2秒）
  1. 元ポストの投稿者（Bsky ユーザー）に対応する brid.gy ブリッジ AP アクターを探す
     （Section 8 の2段階ルックアップと同じ手順）
  2. ブリッジアクターが見つかった場合:
     そのアクターの Outbox または AP API で元ポストに対応する AP オブジェクトを探す
  ↓ AP オブジェクト取得成功
  → AP Announce アクティビティを送信
  ↓ 取得失敗 / タイムアウト
【フォールバック】
  → 元ポストの bsky.app URL をメンション付き通常投稿として Fedi に配信
     本文例: "🔁 {投稿者ハンドル}: {bsky.app/profile/.../post/...}"
```

---

### 9.4 引用投稿のクロスプロトコル配信

引用（quote）は元ポストをインライン参照する形式で、リポストより単純。

| 元ポストの種別 | Fedi 配信 | Bsky 配信 |
|---|---|---|
| ローカル / リモート seiran | AP `Create(Note)` + quoteUrl | ATP `app.bsky.feed.post` + `app.bsky.embed.record` |
| Fedi リモート | AP `Create(Note)` + quoteUrl | ATP `app.bsky.feed.post` + **external embed**（元ポスト URL をカード表示）|
| Bsky リモート | AP `Create(Note)` + quoteUrl（bsky.app URL）| ATP `app.bsky.feed.post` + `app.bsky.embed.record`（元ポストの at_uri） |

Fedi 引用 → Bsky では `app.bsky.embed.record` ではなく `app.bsky.embed.external` を使う（ブリッジポストが存在しても at_uri 取得コストが高いため、URLカードで代替）。

---

### 9.5 リプライのクロスプロトコル配信ルール

リプライ先が存在しない世界にリプライだけ配送しても意味がないため、元ポストの種別で配信先を制限する。

| 元ポスト（`reply_to_post_id` が指す投稿）の種別 | Fedi 配信 | Bsky 配信 |
|---|---|---|
| ローカルポスト | ✅ AP `Create(Note)` | ✅ ATP `app.bsky.feed.post`（`reply` 付き）|
| リモート seiran ポスト | ✅ AP `Create(Note)` | ✅ ATP `app.bsky.feed.post`（`reply` 付き）|
| Fedi リモートポスト | ✅ AP `Create(Note)` | ❌ 配信しない |
| Bsky リモートポスト | ❌ 配信しない | ✅ ATP `app.bsky.feed.post`（`reply` 付き）|

**「リモート seiran ポスト」の判定**
- `posts.at_uri` が `at://did:plc:…` で、かつ `actors.domain` が seiran インスタンスのドメインであるポスト
- このポストは Fedi 側にも AP Note として存在し（seiran が AP + ATP 両対応のため）Bsky 側にも ATP レコードとして存在するため、両方に配送できる

---

## 10. 画像 embed の DAG-CBOR 形式仕様

### 10.1 blob ref の CBOR エンコーディング

AT Protocol において画像 blob への参照（blob ref）は DAG-CBOR の **CBOR Tag 42** で直接 CID を指定する。

```
// 正しい形式
blob.ref = Ipld::Link(cid)  →  CBOR tag42( <CIDv1 bytes> )

// 誤った形式（使用禁止）
blob.ref = Ipld::Map({"$link": Ipld::Link(cid)})  →  {"$link": tag42(...)}
```

`Ipld::Link` を `serde_ipld_dagcbor` でシリアライズすると tag42 になる。`{"$link": ...}` ラップは AT Protocol の JSON 表現であり、CBOR では使用しない。

### 10.2 `app.bsky.embed.images` レコード構造

フィールドのキー順序は DAG-CBOR のカノニカル順（文字列長昇順 → 辞書順）に従う：

```
embed:
  $type: "app.bsky.embed.images"  (5)
  images: [...]                    (6)

images[n]:
  alt: ""              (3)
  image: <blob>        (5)
  aspectRatio: {...}   (11)

blob:
  ref: tag42(CID)     (3)   ← Ipld::Link で直接指定
  size: <bytes>        (4)
  $type: "blob"        (5)
  mimeType: "..."      (8)

aspectRatio:
  width: <px>   (5)
  height: <px>  (6)
```

### 10.3 `#commit` フレームの `blobs` フィールド

`subscribeRepos` の `#commit` イベントには `blobs` フィールドがあり、そのコミットで参照する blob の CID リストを含める必要がある。このフィールドが空配列だと AppView が blob の存在を認識できず、画像 embed を持つ投稿をインデックスしない。

```
blobs: [ Ipld::Link(blob_cid1), Ipld::Link(blob_cid2), ... ]
```

seiran の実装では `commit_post` 呼び出し時に `CommitRecord.blob_cids` に blob CID のリストを渡し、`build_commit_frame` が `blobs` フィールドに展開する。

### 10.4 `com.atproto.sync.getBlob` エンドポイント

AppView は投稿をインデックスする際に PDS の `getBlob` エンドポイントから blob を取得する。seiran は画像を外部ストレージ（R2/S3）に保存しているため、`getBlob` は CDN URL への HTTP 307 リダイレクトを返す。

```
GET /xrpc/com.atproto.sync.getBlob?did=<DID>&cid=<CIDv1>
→ 307 Redirect: https://media.seiran.org/media/<storage_key>
```

CID の sha2-256 ダイジェストを `media_files.sha256` と照合して対応する CDN URL を返す。

### 10.5 `app.bsky.actor.profile` レコードのアバター・自己紹介

`app.bsky.actor.profile`（rkey 固定 `self`）にも 10.1 の blob ref と同じエンコーディングでアバター画像を含める。フィールドのカノニカル順（文字列長昇順 → 辞書順）は以下の通り：

```
profile:
  $type: "app.bsky.actor.profile"  (5)
  avatar: <blob>                    (6)   ← 未設定時はフィールド自体を省略
  createdAt: "..."                  (9)
  description: "..."                (11)  ← 未設定時はフィールド自体を省略
  displayName: "..."                (11)  ← "description" < "displayName"（5文字目 e < i）
```

`avatar` blob の内部構造（`ref`/`size`/`$type`/`mimeType`）は 10.2 の画像 embed と同一。`encode_bsky_actor_profile`（`crates/seiran-common/src/atp/repo.rs`）は画像 embed 用の blob ref 構築ロジック（`build_blob_ipld`）を共有する。

## 11. ローカルユーザーのプロフィール配信（AP Actor icon/summary ＆ ATP profile 再コミット）

ローカルユーザーが設定したアバター画像・自己紹介文（`actors.avatar_media_id` / `actors.bio`）は、以下の経路で AP・ATP 双方の対外公開データに反映される。

### 11.1 ActivityPub Actor（`GET /users/:username`）

`crates/seiran-federation-inbox/src/handlers/actor.rs` の `actor_handler` は、`actors` を `media_files` / `storage_providers` と LEFT JOIN してアバターの公開URLと `bio` を取得し、`Person` オブジェクトに以下を含める：

- `icon`: `{"type": "Image", "mediaType": <mf.mime_type、不明時は "image/jpeg">, "url": <アバター公開URL>}`（アバター未設定時はフィールド自体を省略）
- `summary`: `bio` の内容（未設定時はフィールド自体を省略）

アバターURL解決は `users.rs` の内部プロフィールAPI（`build_profile_response`）と同じ `COALESCE(storage_providers.public_url || storage_key, actors.avatar_url)` パターンを用いる。この Actor エンドポイントは HTTP GET のたびに DB を都度参照するため、プロフィール編集後は次回フェッチで自動的に最新値が返る。

### 11.2 `Update{Person}` の即時プッシュ配信

新規フェッチでは 11.1 の都度参照で十分だが、**既にフォロー中**のリモートインスタンスは Actor 情報をキャッシュしているため、フェッチ待ちでは更新が反映されない。これを解消するため、`crates/seiran-common/src/ap/deliver.rs` の `deliver_update_actor` が `Update{Person}` アクティビティをフォロワー全員の inbox へ即時配送する。

- `object` は 11.1 の `actor_handler` と同一構造の `Person` オブジェクト（`icon`/`summary`含む、`publicKey`も同梱）を都度DBから再構築したもの。
- `activity.id` は配送のたびに一意（`update-actor-<actor_id>-<epoch_millis>`）にする。固定IDだと一部AP実装が2回目以降のUpdateを重複とみなして無視するため。
- `crates/seiran-api/src/handlers/users.rs` の `update_profile` から `tokio::spawn` でバックグラウンド実行し、APIレスポンスはブロックしない。配送失敗はログのみでAPI自体は成功として返す（DB更新は既に完了しているため）。

### 11.3 AT Protocol `app.bsky.actor.profile` の再コミット

`crates/seiran-api/src/handlers/users.rs` の `update_profile` はプロフィール更新後、`AtpService::commit_profile(actor_id, display_name, description, avatar_media, now)` を呼び出し、`app.bsky.actor.profile/self` レコードを再コミット（MST再構築・署名・`atp_repo_events` 記録・`subscribeRepos` への `#commit` フレーム配信）する。これにより Bsky Relay/AppView が更新を検知できる。

- `commit_profile` は `atp_records` に既存の `self` レコードがあるかを見て `action`（`create`/`update`）を自動判定する。
- **重要**: 同一 rkey（`self`）を2回目以降コミットする際、MST構築用のエントリロード（`load_atp_entries`）が返す既存エントリと新エントリでキーが重複しないよう、`commit_record_inner` は push 前に同一キーの古いエントリを `retain` で除去する。これを行わないと MST に重複キーが入り、AppView に拒否される不正な木になる。

新規登録時（`setup.rs` / `auth.rs`）はアバター・bioがまだ無いため `description: None, avatar_media: None` で呼ばれる。

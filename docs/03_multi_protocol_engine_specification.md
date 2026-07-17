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

システム全体の分散通信および同期を支えるため、バックエンドに以下の6つの非同期キューを実装する。これらは `JobQueue` Trait で抽象化されており、モノリスモード（`--role all`）では `InMemoryJobQueue`、split-role 構成では `REDIS_URL` 設定時に自前実装の `RedisJobQueue`（優先度付き Sorted Set + `BZPOPMIN` + Lua スクリプトによる遅延リトライ昇格）へ切り替わる（バックエンド選択の詳細は `docs/02_architecture_and_overall_design.md` §4.1 ③）。

| キュー（ジョブ）名 | 処理内容 | 優先度 | リトライ & レート制限（レートリミット）戦略 |
| :--- | :--- | :--- | :--- |
| **1. 過去ログ同期キュー**<br>`actor_history_sync` | 新規フォローされたアクターの過去ログ（最大300件）をページネーションしながら順次取得・DB保存する。 | **低 (Low)** | ・**ドメイン単位の同時実行制限 (Concurrency Limit)**: 同一APインスタンスに対する同時フェッチを1件に制限。<br>・リクエスト間に 1〜3秒のジッター（揺らぎ）を挿入。<br>・失敗時は指数バックオフで最大3回リトライ。 |
| **2. AP 配送キュー**<br>`ap_delivery` | 自サーバーで発生した AP アクティビティ（投稿 Create のほか、Announce/Undo(Announce)/Like・EmojiReact/Undo(リアクション)/Update(Person)/Delete(Actor)）を、APフォロワーの各リモートサーバー（Inbox）へ配送する（APのみ。ATPはPDS自律処理のため不要）。配送内容は `ApDeliveryKind` enum で表現し、API ハンドラはこれを enqueue するだけでよい。 | **高 (High)** | ・ユーザーのアクションに直結するため、最高優先度で即時実行。<br>・相手サーバーのダウン時は、数時間にわたる指数バックオフで最大10回程度リトライ（段階的に間隔を広げる）。<br>・宛先が1件以上あり全滅した場合のみ `Err`（リトライ対象）。部分成功時は重複配送を避けるため `Ok` とする。 |
| **3. 配送受け入れ（インバウンド）キュー**<br>`inbound_activity_process` | 外部（APのInbox）から届いたアクティビティ（Follow/Create/Accept/Undo/Announce/Like・EmojiReact 全種別）を非同期で処理する。未知アクターの解決（upsert）、Note本体の保存、リアクション記録、フォロー状態更新、リアルタイム配信（`stream_hub`）まで含む（`seiran_common::jobs::inbound_activity_process`。2026-07リファクタリングで `federation-inbox` 側の `tokio::spawn` 直接処理から全面移行）。 | **中 (Medium)** | ・指数バックオフで最大3回リトライ（2s → 4s → …、上限60s）。<br>・ドメイン単位のレート制限は未実装（今後の課題。現状 `ActorHistorySync` キューのみ適用）。 |
| **4. アクター検証・メタデータ取得キュー**<br>`actor_metadata_resolve` | リモートseiranアクターのハンドシェイク検証（`verify-actor`）や、Webfingerによる解決、アバター画像やOGP情報のプロキシ・キャッシュ化。 | **中 (Medium)** | ・タイムアウト時は短いスパンで再試行（最大3回）。<br>・画像取得失敗等は非クリティカルとして扱い、デフォルト画像でフォールバック。 |
| **5. ATPリポジトリコミット・配信キュー**<br>`atp_repository_publish` | seiran PDS としてローカルユーザーの AT Protocol リポジトリを更新する。具体的には、投稿・削除等のレコードを DAG-CBOR 形式で MST にコミットし、P-256 秘密鍵で署名した後、Relay サーバーへ配信する。外部 PDS（bsky.social 等）は使用しない。 | **極高 (Critical)** | ・順序性の維持が不可欠（同じDIDに対するコミット順が前後してはならない）。<br>・アクターID単位の **FIFO（先入れ先出し）** キュー、またはシリアル排他ロック制御が必要。 |
| **6. Bsky動画パイプライン結合キュー**<br>`bsky_video_poll` | 動画添付アップロード時にBluesky公式動画パイプライン（`app.bsky.video.uploadVideo`）へ投げたジョブの完了（`app.bsky.video.getJobStatus`）を待つ。完了したら`media_files.bsky_video_cid`/`bsky_video_status='ready'`を更新し、以降の`commit_post`が`app.bsky.embed.video`を使えるようにする。 | **高 (High)** | ・1回の実行で1回だけ`getJobStatus`を叩き、未完了なら`Err`を返してリトライさせる（固定3秒間隔・最大10回=30秒）。<br>・30秒以内に完了しなければ`bsky_video_status`は`'pending'`のまま放置され、該当ポストは`app.bsky.embed.external`にフォールバックする（待たない設計）。 |

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
| AP 配送（`ap_delivery` キュー） | `deliver_to_fedi == true` |

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
  `/api/following/delete`, `/api/notes/unrenote` など。（`/api/i/notifications` は
  §5.8 で実装済み）
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

### 5.8 `POST /api/i/notifications`（通知永続化, 2026-07-15）

マイケルの要望: 「クイック通知が一度見ると消えてしまうのはもったいない。新着順に並び、
過去分も無限スクロールで遡れるようにしてほしい。あくまで Misskey API 互換で」。
従来の通知（フォロー・リアクション・フォロー承認）は WebSocket ライブ配信のみで、
フロントエンドのインメモリ配列（`StreamingContext`、最大100件、リロードで消滅）に頼っていた。
これを DB 永続化（`notifications` テーブル、Doc1 §1.12）し、本家 Misskey の
`POST /api/i/notifications` と同じワイヤープロトコルで実装した。

**バックエンド**: `NotificationRepository`（`seiran-common::repository::notification`）が
`insert`/`list`（`until_id`/`since_id` カーソル）/`mark_all_read` を提供。書き込みは
リアクション作成（ローカル `handlers::notes::create_reaction`、AP/ATP inbound とも）、
AP `Follow`/`Accept(Follow)` 受信の各経路から行う。`AppState.notifications`
（`Arc<dyn NotificationRepository>`）・`InboxContext.notification_repo` として
DI する（既存の `PostRepository` 等と同じパターン）。ハンドラ本体は
`handlers::misskey::endpoints::i_notifications`。paramDef（`limit`/`sinceId`/`untilId`/
`markAsRead`/`includeTypes`/`excludeTypes`）は本家 Misskey に合わせたが、`sinceDate`/
`untilDate` は未対応。レスポンス型 `MisskeyNotification`（`handlers::misskey::types`）・
組み立ては `convert::build_notifications`（通知者・ノートを ID ごとに一括解決してから
`MisskeyNote`/`MisskeyUserLite` へ変換）。

**フロントエンド**: `NotificationsPanel.tsx` は初回マウント時に `api.notifications.list()`
で新着20件を取得し、以降は下端到達で `IntersectionObserver` により `untilId` カーソルで
過去分を追加取得する（無限スクロール）。WebSocket ライブ通知（`StreamingContext` の
`registerNotifArrived`）は「新着があった」というシグナルにのみ使い、実データは常に
`sinceId` 付きで REST から再取得する（WS ペイロードのアドホックな整形をやめ、一覧表示と
同じ ID 体系・スキーマに統一するため）。`unread` バッジのカウント・既読化ロジック自体は
変更していない。

**カスタム絵文字リアクションの画像表示**: 初回実装時、`notifications.reaction` は
`:shortcode:`/Unicode の生文字列のみで画像URLを持たず、通知一覧上のカスタム絵文字が
テキスト表示に退行する不具合があった（マイケル指摘・2026-07-15）。本家 Misskey の
`NotificationEntityService`/`NoteEntityService` を確認したところ、本家も通知オブジェクト
自体には画像URLを持たせず、同梱する packed `Note` 側の
`reactionEmojis: populateEmojis(reactionEmojiNames, host)`（`{shortcode→url}`）を
クライアントが参照する設計だった。これに合わせ `MisskeyNote` に `reactionEmojis`
（`handlers::misskey::convert::to_misskey_note` が既存の `fetch_reactions_map` の
`ReactionSummary.emoji_url` から構築）を追加し、フロントは
`n.note?.reactionEmojis?.[n.reaction]` を解決できた場合のみ `img` 表示するよう修正した。

**上記修正の残存バグと再修正（同日）**: 上記実装は投稿の「現在の」リアクション集計
（`reactions` テーブル）から都度解決する方式だったため、同じアクターが短時間に何度も
別の絵文字へ切り替えると（`reactions.UNIQUE(post_id, actor_id)` により過去の行が
上書きされる）、切り替え前の古い通知が画像解決できなくなり `:shortcode:` 表示に戻って
しまう不具合が再度発生した（マイケル指摘・2026-07-15、「一部は画像表示されている」＝
直近の1件だけ現在の集計と一致して解決できていた）。マイグレーション
`20260716020000_notifications_reaction_emoji_url.sql` で `notifications.reaction_emoji_url`
を追加し、通知 INSERT 時点（`jobs::inbound_activity_process::handle_reaction`）で確定
している画像URLを非正規化保存するよう変更。`convert::build_notifications` は、ノート単位で
共有キャッシュした `reactionEmojis` マップにこの通知固有の値を上書き挿入することで、
他の通知・投稿の現在状態に関わらず恒久的に正しい画像URLを返すようにした（Doc1 §1.12 参照）。

**既知の制約**:
- 上記マイグレーション適用前に作成された既存の通知データは `reaction_emoji_url` が
  `NULL` のままのため遡って修正されない。当該投稿の現在のリアクション内容とたまたま
  一致する場合のみ従来の都度解決にフォールバックする。
- ローカル同士のフォローは AP を経由しないため通知が発生しない（リモートからのフォローのみ対応）。

---

### 5.9 ホーム / ローカル / リストタイムラインの無限スクロール（2026-07-16）

`GET /api/notes/home-timeline` / `local-timeline` / `GET /api/lists/:id/timeline` は
実装当初から `until_id`/`since_id` カーソルページネーションに対応済みで（`TimelineQuery`,
`handlers/notes/dto.rs`、DB側は `PgPostRepository::home_timeline`/`local_timeline` の
`$N::bigint IS NULL OR p.id < $N` パターン、§4.3.1参照）、フロントの `client.ts` も
`until_id`/`since_id` パラメータをすでに受け付けていた。しかし呼び出し元
（`HomePage.tsx`, `ListDetailPage.tsx`）は `limit: 30` のみで初回ページしか取得しておらず、
「もっと読み込む」導線が存在しなかった。§5.8 で実装した `NotificationsPanel.tsx` の
`IntersectionObserver` + `untilId` カーソルパターンをタイムライン表示にも移植し、
`NoteList.tsx`（`HomePage.tsx`・`ListDetailPage.tsx`・`SearchPage.tsx` で共用）に
`onLoadMore`/`hasMore`/`loadingMore` の3つの任意 props を追加する形で無限スクロールを
共通化した（未指定なら従来通り無効、`SearchPage.tsx` は変更なし）。

**発見したバグ（sentinel要素の observe し忘れ）**: `NotificationsPanel.tsx` と同じ
`useRef` + `useEffect(() => { ... observer.observe(el) ...}, [onLoadMore, hasMore, items.length])`
パターンをそのまま踏襲したところ、タブ切り替え（ホーム→ローカル等）直後にスクロールすると
追加読み込みが一切発生しないバグが再現した。原因は次の通り:

1. タブ切り替え直後、`feed` state は新タブの値に変わるが、対応する `useEffect`
   （`setLoading(true)` 等）はまだ commit 後の非同期実行を待っている状態のため、
   「新しい `feedKey`・古い `notes`/`loading=false`」という一瞬の不整合レンダーが発生する。
   このレンダーで `feedKey(feed)` に依存する `loadMore` の `useCallback` が新しい関数
   として再生成され、`NoteList` の `useEffect` が新しい観測対象（実際にはまだ古い DOM
   ノードのまま）に対して再セットアップされる。
2. 直後に `loading=true` になり `NoteList` が早期 return（`<p>読み込み中...</p>`）する
   ことで、sentinel の `<div>` を含む DOM ツリーごとアンマウントされる。IntersectionObserver
   はこの切断を検知して1回 `isIntersecting=false` で発火するが、observer 自体は
   disconnect されないまま残る。
3. 新タブのデータ取得が完了し `loading=false` に戻ると、sentinel は新しい DOM ノードとして
   再生成される。しかしこのタイミングで `useEffect` の依存配列
   `[onLoadMore, hasMore, notes.length]` の値（`onLoadMore`・`hasMore`・`notes.length`）
   が、直前の（不整合レンダー時点の）値とたまたま完全に一致すると（例: 新旧どちらの
   フィードも初回ページがちょうど `PAGE_SIZE` 件で `hasMore=true` のまま、件数も同じ）、
   Reactは「変化なし」と判定して `useEffect` を再実行しない。結果、新しい sentinel 要素は
   誰にも `observe()` されず、無限に見えて実際には発火しない状態になる。

`items.length` が初回ロードで必ず 0→N と変化する `NotificationsPanel.tsx` ではこの条件に
当たらず顕在化しなかったが、タブ切り替えという「別の内容に置き換わるが偶然同じ件数になり
うる」操作を持つタイムラインでは再現する。`useEffect` + `ref` オブジェクトの組み合わせは
「DOM要素の生成・破棄」と「依存配列の値の変化」が必ずしも1対1で対応しないことが根本原因。

**修正**: `NoteList.tsx` の sentinel 監視は `useEffect` ではなく **callback ref**
（`useCallback` でラップした ref 関数を `<div ref={sentinelRef}>` に渡す）に変更した。
callback ref は DOM ノードが実際にアタッチ / デタッチされるたびに React が必ず呼ぶため、
依存配列の値の一致・不一致に関わらず新しいノードへの `observe()` 漏れが起きない。
`onLoadMore`/`hasMore` が変わった場合は `useCallback` の再生成により古い ref 呼び出し
（`null` で cleanup）→ 新しい ref 呼び出し（新要素で observe）が保証される。
`NotificationsPanel.tsx` 自体は同じ潜在バグを理論上抱えているが、初回ロードで
`items.length` が必ず変化する現状の使い方では発現しないため、今回は変更していない。

**プロフィール画面の投稿一覧への拡張（同日、#64）**: `GET /api/users/profile` の
`recent_posts` は従来 `PostRepository::timeline_by_actor(actor_id, 20)` 固定・カーソル
引数なしの一発取得だった。他のタイムライン系クエリと同じ `until_id`/`since_id` カーソル
規約に拡張し（`$N::bigint IS NULL OR p.id < $N` パターン、§4.3.1）、新規エンドポイント
`GET /api/users/posts?actor_id=...&until_id=...&limit=...`（`handlers::users::user_posts`）
で追加ページを取得できるようにした。`ProfileResponse` に `actor_id`（文字列化 Snowflake ID）
を追加し、フロントの起点にする。DB 未登録のリモートアクター（`fetch_bsky_profile_from_appview`
で未フォローかつ AppView 直取得のみのケース）は永続 ID を持たないため `actor_id: None`
（この場合 `recent_posts` も常に空なので実害なし）。フロントは `ProfilePage.tsx` の
「最近の投稿」を `NoteCard` の直接 map から `NoteList.tsx` 経由に置き換え、上記で
callback ref 化した無限スクロール機構をそのまま流用した。ピン留め投稿（`pinned_posts`）は
件数が少なく無限スクロール対象外のため、引き続き `NoteCard` 直接 map のまま。

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
  → 元ポストの URL を本文に含むテキストポストを Bsky に投稿
     本文例: "🔁 {投稿者表示名}: {元ポストURL}"
     このテキストポストは "新規ポスト" として別行を作らず、リポストラッパー行
     （`posts.repost_of_post_id` が設定された行）自身を PDS 上のテキストポスト
     として commit する（`commit_post` に repost_of_post_id 側の post_id を渡し、
     同一行の at_uri/at_cid/at_rkey を更新する）。これにより自前 Jetstream の
     自己エコー（`ON CONFLICT (at_uri) DO NOTHING`）が重複行を作らず、ローカル
     には元のリポスト1行だけが残る。リポスト取り消し時はこのテキストポストも
     `delete_atp_post` で retract する。
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
     （AP Create(Note)、id は `https://{domain}/notes/{post_id}`）
```

**リポスト取り消し時の Fedi 側配信**
- 元ポストが Fedi 実体を持つ（AP Announce を送っていた）場合: `Undo(Announce)` を配送する
- 元ポストが Bsky ネイティブ（上記フォールバックで Create(Note) を送っていた）場合:
  `Undo(Announce)` ではなく、その Note（`https://{domain}/notes/{post_id}`）を対象にした
  `Delete(Note)` を配送する

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

### 9.6 リアクション（絵文字/Like）の配送先

`POST /api/notes/:id/reactions` でローカルユーザーが絵文字リアクションを付けた際、AP `Like`/`EmojiReact`
アクティビティ（取消時は `Undo`）を以下 2 種類の宛先の **和集合（重複排除）** へ配送する
（`crates/seiran-common/src/ap/deliver.rs` の `deliver_ap_reaction` / `deliver_ap_undo_reaction`）。

1. **対象ポストの著者**（`actors.actor_type = 'fedi'` の場合のみ、`ap_inbox_url` 宛）
   - リアクション対象のポストがあるサーバーに確実に通知が届くようにするため
2. **リアクションした本人（ローカルアクター）の Fedi フォロワー全員**（`follows.status = 'accepted'` かつ
   フォロワーが `actor_type = 'fedi'`、`ap_inbox_url` 宛）
   - フォロワーのタイムライン/通知でリアクションの事実を確認できるようにするため。他のアクティビティ
     （Create/Announce/Update 等）と同じ「フォロワー全員へバラ配送（sharedInbox 未対応）」パターンに揃えている

対象ポストが `ap_object_id` を持たない（Bsky 由来でブリッジされていない等）場合は AP 上の実体が無いため配送しない。

**Like か EmojiReact かの判定**（`reaction_activity_type`）
- リアクション内容が `❤️` の場合 → `Like`（EmojiReact 未対応の Mastodon 等とも広く互換）
- それ以外 → `EmojiReact`（`content` フィールドにリアクション内容を格納。Misskey 系フォーク互換のため
  非標準の `_misskey_reaction` フィールドも併記）

**切替時の Undo**
- 1投稿1ユーザー1リアクションの制約（9.0 節同様の Misskey 準拠ルール、`reactions.UNIQUE(post_id, actor_id)`）により、
  既存のリアクションを異なる絵文字に切り替える場合がある。旧リアクションが AP へ配送済み
  （`reactions.ap_activity_id` が設定済み）であれば、新リアクションの送信前に旧リアクションの `Undo` を
  同じ宛先集合へ配送してから送り直す
- `reactions.ap_activity_id` はローカルで発行した `https://{domain}/activities/reactions/{post_id}-{actor_id}-{epoch_millis}`
  形式の URI で、AP 側での Undo 対象特定に使う（ATP 側の `at_uri`/rkey 保存と同じ役割）

**INBOUND（受信側）の内容解決とカスタム絵文字対応**
- 受信側は `crates/seiran-federation-inbox/src/handlers/inbox.rs` の `handle_reaction`。**Misskey は絵文字
  リアクション（Unicode・カスタム絵文字とも）でも AP の `type` を `"Like"` 固定で送り、`EmojiReact` 型は
  使わない**（実際の内容は `content`/`_misskey_reaction` フィールドに載る）。そのため種別判定に wire type
  （`activity["type"]`）は使わず、`content`（無ければ `_misskey_reaction`）を読んで判定する。どちらも
  存在しない場合のみ Mastodon 等の素のお気に入りとみなし `content = "❤️"` とする
  （`reaction_type` は `content == "❤️"` なら `'like'`、それ以外は `'emoji'`）
- `content` が `:shortcode:` 形式（カスタム絵文字）の場合、activity の `tag` 配列から画像 URL を解決する
  （`extract_emoji_tag_url`）。Misskey 等が Like/EmojiReact に添付する形式:
  ```json
  {
    "type": "Like",
    "content": ":blobcat:",
    "_misskey_reaction": ":blobcat:",
    "tag": [
      { "id": "https://misskey.example/emojis/blobcat", "type": "Emoji", "name": ":blobcat:",
        "icon": { "type": "Image", "mediaType": "image/png", "url": "https://misskey.example/files/blobcat.png" } }
    ]
  }
  ```
  `tag` 配列から `type == "Emoji" && name == content` の要素を探し `icon.url` を `reactions.emoji_url`
  に保存する。見つからなければ（Unicode 絵文字、または `tag` 未添付のリモート実装）`NULL` のまま
- これにより、受信した Like/EmojiReact は「Unicode 絵文字」「カスタム絵文字（画像URL解決込み）」
  「素の Like（❤️固定）」の3パターンをすべて正しく `reactions` テーブルへ反映する。この節の OUTBOUND
  実装と合わせて Like/EmojiReact の送受信が双方向になった

**送信内容のバリデーション（Unicode 絵文字限定）**
- `POST /api/notes/:id/reactions` の `validate_reaction_content`（`crates/seiran-api/src/handlers/notes.rs`）は、
  「絵文字リアクション」という名の通り Unicode 絵文字（単体・肌色/性別修飾・ZWJ結合・国旗・キーキャップ等の
  RGI シーケンスを含む）以外の文字列を拒否する。判定は Unicode 公式の `emoji-test.txt` 準拠データから
  生成された `emojis` crate による完全一致で行い、`:shortcode:` のようなカスタム絵文字ショートコードや
  プレーンテキストは通さない（カスタム絵文字ピッカー自体が未実装のため、ローカル送信経路では意図的に
  絵文字のみへ制限している）
- フロントエンド（`frontend/src/lib/reaction.ts` の `isValidReactionEmoji`、npm パッケージ `emoji-regex`
  使用）でも同じ方針で自由入力欄を事前チェックする。これはあくまで UX 向上のための先取りチェックであり、
  最終的な正当性判定は API 側（バックエンド）が行う
- 上記の制約は **ローカルからの送信（outbound）にのみ適用**する。AP から受信する `EmojiReact.content`
  （9章冒頭・DBスキーマ文書参照）は他インスタンスのカスタム絵文字ショートコードを含みうるため、
  INBOUND 側では従来通りバリデーションせずそのまま保存する

---

### 9.7 投稿本文・表示名中のカスタム絵文字（`:shortcode:`）表示

9.6 節はリアクション（Like/EmojiReact）自体が持つカスタム絵文字の話だったが、Note 本文や Person の
表示名の中に埋め込まれた `:shortcode:`（例: 「わこつ:blobcatwave:」）も同じ AP `tag` 配列の仕組みで
画像化できる。Misskey は Note・Person いずれにもトップレベルの `tag` 配列（`type:"Emoji"`）を添付する:

```json
{
  "type": "Note",
  "content": "わこつ :blobcatwave:",
  "tag": [
    { "id": "https://misskey.example/emojis/blobcatwave", "type": "Emoji", "name": ":blobcatwave:",
      "icon": { "type": "Image", "url": "https://misskey.example/files/wave.png" } }
  ]
}
```
Person（表示名・自己紹介中の絵文字）も同形式。Update(Person) 受信時も同じ経路で再解決される。

**共通ヘルパー**: `seiran_common::ap::build_emoji_map(tags: &[Value]) -> Value`（`crates/seiran-common/src/ap/client.rs`）
が `tag` 配列から `{shortcode: 画像URL}` のオブジェクトを構築する。9.6 節の `extract_emoji_tag_url`（単一
shortcode の解決、リアクション用）もこの関数を内部で使うよう統一した。`ApActor::emoji_map()` は
`ApActor.tag`（新設フィールド）からこのマップを返す。

**保存**:
- `handle_create_note`（`crates/seiran-federation-inbox/src/handlers/inbox.rs`）が `note["tag"]` から
  `build_emoji_map` で本文用マップを作り `posts.emoji_map` に保存する（`PostRepository::insert_remote_with_dedup`
  に `emoji_map` 引数を追加）
- `upsert_remote_fedi_actor` が `remote_ap.emoji_map()`（Person の `tag` 由来）を `actors.emoji_map` に保存する
  （`ActorRepository::upsert_remote_fedi` に `emoji_map` 引数を追加。`follows.rs` の
  フォロー時アクター解決からも同様に呼ばれる）
- ローカル投稿・ローカルアクターは常に空オブジェクト（カスタム絵文字ショートコードをローカルから
  送信する経路が無いため。9.6 節のバリデーションと同じ理由）

**API 公開**: `to_note_response`（`crates/seiran-api/src/handlers/notes.rs`）が `posts.emoji_map` と
投稿者の `actors.emoji_map` を統合し、`NoteResponse.emojis: Record<string,string>` として返す（本文・
表示名の両方をこの1つのマップで解決できるようにする簡略設計。空なら省略）。`TimelinePost` に
`post_emoji_map`/`actor_emoji_map` フィールドを追加し、対応する全SQLクエリ（`home_timeline` 等6箇所 +
`embed_renotes`）の SELECT 句に `p.emoji_map`/`a.emoji_map` を追加した。

**フロントエンド**: `frontend/src/components/note/EmojiText.tsx` が `text`/`emojis` を受け取り、本文・
表示名中の `:[a-zA-Z0-9_+-]+:` パターンを `emojis` マップと突き合わせて、解決できるものだけ `<img>`
に置換する（解決できないコロン記法はテキストのまま）。`NoteCard`（本文・投稿者表示名・リポスト元の
表示名）で使用。ラップ要素を持たないため呼び出し側の `<span>`/`<p>` のスタイルはそのまま活きる。

- **境界条件**: 左端（`:` の直前）は何が接触していても良い（例: 「わこつ:blobcatwave:」）。右端
  （閉じ `:` の直後）だけは英数字・アンダースコアが接触していないことを条件にする（`file:name_here:12345`
  のような無関係なコロン記法を誤って絵文字化しないため）。右端の接触チェックに失敗した場合は、
  そのショートコードが `emojis` マップに存在するかに関わらずマッチ自体を無効化しテキストのまま残す。
- **サイズ**: `<img>` の高さのみ行の高さに合わせて固定し（`height: 1.2em`）、幅は `width: auto` で
  画像本来の縦横比に追従させる（正方形とは限らないカスタム絵文字を歪ませないため。横長の絵文字は
  そのぶん横に長く表示される）。

**通知（クイック通知欄）のカスタム絵文字リアクション**: `handle_reaction` は既に `emoji_url` を計算済み
だったが、WS `"reaction"` イベントのペイロードに配線されていなかった不具合を修正し `"emojiUrl"` を
追加した。`frontend/src/contexts/StreamingContext.tsx` の `Notif.body.emojiUrl` を経由して
`NotificationsPanel.tsx` が `img` 表示する（`ReactionChips`/`EmojiText` と同じ「URL があれば画像、
無ければテキスト」パターン）。

**既知の制約**: プロフィールページ単体の表示名表示（`ProfilePage.tsx`）と `ReplyIndicator` の
「返信先」表示名は今回のスコープ外（`NoteResponse` を経由しないため）。将来対応する場合は
`ProfileResponse` にも `emojis` フィールドを追加する形になる。

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

### 10.6 Bsky 配送不安定性の原因と修正（`prevData` フィールド・ECDSA low-S 正規化）

2026-07-17、Bsky 公式リレー実装（`bluesky-social/indigo`、`cmd/relay`）をローカルで動かし実際のコミットを検証させることで、配送が特定タイミングで途絶する不具合の主因を2件特定した。

**① `#commit` フレームの `prevData` フィールド欠落**

AT Protocol Sync 1.1 は `subscribeRepos` の `#commit` イベントに `prevData`（前回コミット時点の MST root CID）を含めることを要求する（`indigo` の `cmd/relay/relay/verify.go` の `VerifyCommitMessageStrict`）。そのDIDの**初回コミットのみ** `prevData` 省略が許容され、**2回目以降のコミットは `prevData` が無いと `"missing prevData field"` で即リジェクトされる**。seiran の `build_commit_frame`（`crates/seiran-common/src/atp/repo.rs`）はこのフィールドを送っていなかったため、実質的に2回目以降のほぼ全コミットがリレーに拒否されていた。

修正: `actors` テーブルに前回コミットの MST root CID を保持する `at_repo_data_cid` カラムを追加し（マイグレーション `20260717000000_actors_at_repo_data_cid.sql`）、`commit_record_inner` / `delete_atp_repost` / `delete_atp_like` / `commit_delete_follow` / `delete_atp_record_generic` の各コミット処理で、コミットのたびに新しい MST root CID をこのカラムへ保存し、次回コミット時に読み出して `build_commit_frame` の新しい `prev_data` 引数へ渡すようにした。

**② ECDSA 署名の "low-S" 未正規化（約50%の確率でランダムに失敗）**

AT Protocol の署名検証（`indigo` の `atcrypto.PublicKeyP256.HashAndVerify`）は ECDSA 署名のマレアビリティ対策として "low-S" 形式の署名を必須とする。RustCrypto の `p256::ecdsa::SigningKey::sign` はデフォルトで low-S 正規化を行わないため、生成される署名の s 値は数学的性質上ほぼ50%の確率で high-S になり、その場合 Bsky 側の検証で `"cryptographic signature invalid"` として拒否される。既存メモにあった「配送が特定タイミングで途絶える」という不規則な症状は、実際にはタイミングとは無関係で、コミットのたびにほぼコイントスの確率でランダムに発生していたものだった。

修正: `crates/seiran-common/src/atp/repo.rs` の `create_commit`、および `crates/seiran-common/src/atp/plc.rs` の PLC genesis operation 署名の両方で、署名生成後に `Signature::normalize_s()` を呼んで low-S に正規化するよう変更した。

両修正はローカルの `indigo`/`cmd/relay` を用いた実地検証で効果を確認済み（署名検証エラー・`missing prevData field` エラーとも解消）。デプロイ直後は各アカウントの最初の1コミットのみ、リレー側に残る旧状態の影響で `missing prevData field` が再発する可能性があるが、2回目以降は正常に処理される見込み。

### 10.7 `com.atproto.repo.getRecord` の CID リンク（embed）デコード（2026-07-17）

`crates/seiran-api/src/handlers/xrpc/repo.rs` の `xrpc_get_record` は、`atp_blocks` に
保存された実際のコミット済み DAG-CBOR レコードを取得して JSON で返す。かつては
`serde_ipld_dagcbor::from_slice::<serde_json::Value>(&cbor_bytes)` のように直接
`serde_json::Value` へデコードしていたが、embed の blob ref のような **CID リンク
（DAG-CBOR tag 42）を含むレコードでは `Msg("invalid type: newtype struct, expected
any valid JSON value")` で失敗する**（`serde_json::Value` にはタグ付き値を表現する
概念が無いため）。失敗時は例外にせず何も返さない実装だった `app.bsky.feed.post`
専用パス（`get_record_post`）では、これが原因で embed が常に欠落していた
（2026-07-17 マイケル指摘で発覚。実際の firehose 配信・relay 検証には影響しない
表示専用のバグだった）。

修正: 一度 `ipld_core::ipld::Ipld` にデコードしてから、AT Protocol の JSON 表現規約
（CID リンク → `{"$link": "<cid>"}`、バイト列 → `{"$bytes": "<base64>"}`）に沿って
手動で `serde_json::Value` に変換するヘルパー `ipld_to_json`（同ファイル）を追加し、
`get_record_post`・`get_record_from_atp_records` の両方で使うようにした。

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

`crates/seiran-api/src/handlers/users.rs` の `update_profile` はプロフィール更新後、`AtpService::commit_profile(actor_id, display_name, description, avatar_media, pinned_post, now)` を呼び出し、`app.bsky.actor.profile/self` レコードを再コミット（MST再構築・署名・`atp_repo_events` 記録・`subscribeRepos` への `#commit` フレーム配信）する。これにより Bsky Relay/AppView が更新を検知できる。`pinned_post`（`(uri, cid)`）はピン留め投稿への strongRef で、`update_profile` は編集時点の既存ピン留め状態（`resolve_bsky_pinned_post`）を引き直して渡し、編集操作でピン留めが消えないようにする。詳細は §13。

- `commit_profile` は `atp_records` に既存の `self` レコードがあるかを見て `action`（`create`/`update`）を自動判定する。
- **重要**: 同一 rkey（`self`）を2回目以降コミットする際、MST構築用のエントリロード（`load_atp_entries`）が返す既存エントリと新エントリでキーが重複しないよう、`commit_record_inner` は push 前に同一キーの古いエントリを `retain` で除去する。これを行わないと MST に重複キーが入り、AppView に拒否される不正な木になる。

新規登録時（`setup.rs` / `auth.rs`）はアバター・bioがまだ無いため `description: None, avatar_media: None` で呼ばれる。

### 11.4 プロフィールのキーバリュー項目（#62）

Mastodon 等の「プロフィールのメタデータ欄」（AP Actor の `attachment[type=PropertyValue]`）
相当の機能。DB は `actors.profile_fields`（Doc1 §1.2、`[{"name", "value"}, ...]` 配列、
最大 `seiran_common::MAX_PROFILE_FIELDS`（4）件）。

**送信: Fedi（AP Actor `attachment`）**
- `crates/seiran-federation-inbox/src/handlers/actor.rs` の `actor_handler` が
  `profile_fields` を `ApPropertyValue { type: "PropertyValue", name, value }` の配列に
  変換し `attachment` として返す。`value` は `property_value_html` で HTML エスケープし、
  `http(s)://` で始まる場合は `<a href="..." rel="me nofollow noopener noreferrer">` で
  ラップする（Mastodon 等のクライアントは `value` を HTML としてレンダリングするため、
  素のテキストのままだとリンクにならない）。

**送信: Bsky（`description` 末尾への追記）**
- Bsky の `app.bsky.actor.profile` には構造化フィールドが無いため、`crates/seiran-api/src/handlers/notes/mod.rs`
  の `fetch_atp_profile_material`（pin/unpin 時の再コミットと `update_profile` の両方が
  共用する、ATP 再コミット材料を DB から読み直す関数）が `append_profile_fields_to_bio`
  で bio の末尾に `ラベル: 値` を改行区切りで追記した文字列を組み立て、それを
  `commit_profile` の `description` 引数として渡す。項目が無ければ bio をそのまま返す。

**受信: リモート Fedi アクターの PropertyValue 取り込み**
- `crates/seiran-common/src/ap/client.rs` の `ApActor` に `attachment: Vec<serde_json::Value>`
  を追加し、`property_values()`（`type: "PropertyValue"` の要素を `(name, value)` に変換）・
  `profile_fields_json()`（`MAX_PROFILE_FIELDS` 件に切り詰め、`value` を `strip_html` して
  DB 保存用の JSON 配列を組み立てる）を追加。
- `ActorRepository::upsert_remote_fedi`（`emoji_map` と同じ UPSERT パターン）に
  `profile_fields` 引数を追加し、リモートアクターを upsert する全経路
  （`jobs::inbound_activity_process::upsert_remote_fedi_actor`、`handlers::follows` の
  Fedi フォロー時の解決、`handlers::users::fetch_remote_profile` の未認知アクター初回
  アクセス時 upsert）で `ap_actor.profile_fields_json()` を渡す。
- **既知の制約**: 一度取り込んだ値は、そのアクターへの Follow 受信等で再度 upsert される
  タイミングまで更新されない（`bio` と同じ扱い。プロフィール表示のたびに Actor 自体を
  再フェッチする設計にはしていない。ピン留めの featured collection のみ例外的に毎回
  同期している、§13.3 参照）。Bsky 側の受信（リモート Bsky アクターの `description` に
  埋め込まれたキーバリュー文字列のパース）は対象外（要望に含まれていないため未実装）。
- **fix（マイケル報告・2026-07-15）**: 本機能の実装過程で `ActorRepository::upsert_remote_fedi`
  が **`bio` カラムを一切設定しない既存の欠陥**を発見した。#61（ピン留め機能）で
  `fetch_remote_profile`（未認知アクター初回アクセス経路）を「都度 `ap_actor.summary` を
  そのまま返す非永続表示」から「upsert してから `build_profile_response` に委譲」する
  設計に変更した際、この欠陥に気づかずに切り替えてしまい、以後リモート Fedi アクターの
  自己紹介文が一切表示されなくなる退行を生んでいた。`upsert_remote_fedi` に `bio: Option<&str>`
  引数を追加し（`avatar_url` と同じ `COALESCE(EXCLUDED.bio, actors.bio)` パターンで
  `None` の場合は既存値を保持）、`AP summary` を `strip_html` でプレーンテキスト化して渡す
  よう全呼び出し経路を修正した。**既知の制約**: この修正より前に upsert 済みだったアクターの
  `bio` は遡って修正されない（再度 Follow 受信等で upsert されるまで空のまま）。

## 12. Bsky公式動画パイプライン結合（`app.bsky.embed.video`）

seiranから投稿した動画をBluesky公式アプリで再生可能にするため、Bluesky公式の
動画処理サービス（`video.bsky.app`）と連携する。この節は実機検証で判明した、
一般に公開ドキュメント化されていない挙動を記録する。

### 12.1 サービス間認証JWT（Inter-Service Auth）の実際の仕様

`com.atproto.server.getServiceAuth`相当のJWT（`iss`/`aud`/`lxm`/`exp`）を
P-256署名鍵（`at_signing_key_pem`）で自己署名して使う
（`crates/seiran-common/src/atp/service_auth.rs`の`sign_service_auth_jwt`）。
`jsonwebtoken`クレートの`Algorithm::ES256`がそのまま使える
（seiranはsecp256k1ではなくP-256を使用しているため）。

**重要（公式ドキュメント未記載・実機検証で判明）**:
- `aud`は「呼び出し先サービス（`video.bsky.app`）のDID」ではなく、
  **自分のPDS自身のDID**（`did:web:{local_domain}`）を指定する。
  誤ると`invalid token audience`エラーになる。
- `lxm`は「今まさに呼び出しているメソッド名」ではなく、**このトークンが
  最終的に使われる自PDS側のアクション名**（`com.atproto.repo.uploadBlob`）を
  指定する。誤ると`invalid token lexicon method`エラーになる。
- Bluesky動画サービスは`uploadVideo`呼び出し時に受け取ったこのJWTを保持し、
  トランスコード完了後、自PDSの`com.atproto.repo.uploadBlob`エンドポイントを
  呼び戻す際に**同じトークンをそのまま再利用**する（実機で`Authorization`
  ヘッダを確認済み）。

### 12.2 uploadVideo〜getJobStatusのフロー

1. `POST https://video.bsky.app/xrpc/app.bsky.video.uploadVideo?did={did}&name=...`
   （動画バイナリ、`Content-Type: video/mp4`、上記12.1のJWT）→
   `{"did":..., "jobId":..., "state":"JOB_STATE_CREATED"}`（成功時はフラット構造）。
2. `GET https://video.bsky.app/xrpc/app.bsky.video.getJobStatus?jobId=...`
   （同様のJWT、`lxm=app.bsky.video.getJobStatus`）をポーリング。
   完了時 `{"jobStatus":{"state":"JOB_STATE_COMPLETED","blob":{"$type":"blob",
   "ref":{"$link":<CID>},"mimeType":"video/mp4","size":...}}}`
   （エラー・完了時は`jobStatus`にネストされる点に注意）。
3. 並行して、Bluesky動画サービスが自PDSの`com.atproto.repo.uploadBlob`へ
   トランスコード済みバイナリをPOSTしてくる（詳細は12.3）。

このアカウントが一度もBlueskyのAppViewにインデックスされたことが無い場合
（例: seiran内部でのみ有効なテスト用アカウント）、`uploadVideo`は
`profile_not_found`エラーを返す。実際にBlueskyと交信歴のある
ペアリング済みアカウントでのみ機能する。

### 12.3 `com.atproto.repo.uploadBlob`受け口（PDS側）

`crates/seiran-api/src/handlers/xrpc/repo.rs`の`xrpc_upload_blob`
（`POST /xrpc/com.atproto.repo.uploadBlob`）で受信する。

- `Authorization: Bearer <jwt>`を検証する（`iss`のDIDからAT Protocol
  verification key を解決し署名検証、`aud == did:web:{local_domain}`、
  `lxm == "com.atproto.repo.uploadBlob"`、`exp`未失効を確認）。
- 受信バイト列のSHA-256からCIDを計算し（`cid_from_sha256_hex`）、
  `{"blob":{"$type":"blob","ref":{"$link":CID},"mimeType":...,"size":...}}`
  を返す。
- 実測: 計算したCIDは`getJobStatus`が報告するCIDと完全一致する。

**2026-07-17 修正（重要）**: 以前は「ローカル/Fedi配信は常にアップロード時の
オリジナルファイルを使うため不要」として受信バイト列を読み捨てていたが、これは
誤った設計判断だった。Bsky公式動画パイプライン（`video.bsky.app`）は
トランスコード完了後、**このエンドポイントへ自ら代理POSTしてきて**
トランスコード済みバイナリを渡してくる（12.1参照）。`video.bsky.app`自身が
後で動画再生のためにこのCIDを`getBlob`で取得しようとするため、読み捨てていると
Bsky公式アプリ上で「ビデオが見つかりません」となり再生できない
（マイケル実機確認・`video.bsky.app/watch/.../playlist.m3u8`が404）。

現在は`atp_blobs`テーブル（Doc1 §1.x）に実際に保存する。`media_files`とは
意図的に別テーブルにしている（`media_files`は「ユーザーが投稿に添付した
ファイル」、`atp_blobs`は「サーバー間連携で受信した生blob」で意味が異なり、
所有権・ライフサイクルも別）。ただし分離に伴うリスクとして以下3点を安全対策済み:

1. **正当性検証**: 呼び出し元は「有効な自己署名JWT（DID本人の鍵で誰でも作れる）」
   さえあれば技術的に誰でも叩けてしまうため、対応する
   `media_files.bsky_video_status='pending'`のジョブが実在するアクターからの
   呼び出しでなければ保存を拒否する（`store_uploaded_blob`）。これが無いと
   無制限回数・任意サイズでS3を消費されうる。
2. **クロステーブル重複排除**: 保存前に`media_files.sha256`も照合し、既に
   同じバイト列があれば`atp_blobs`への新規保存をスキップする（`getBlob`は
   両テーブルをUNIONで検索するため、スキップしても解決可能なまま）。
3. **GC**: `crates/seiran-api/src/lib.rs`の`run_atp_blobs_gc`が、
   `media_files`のGC（`run_media_gc`）と同じ1時間ごとのタスクの中で、
   7日以上経過し`media_files.bsky_video_cid`から参照されなくなった
   `atp_blobs`行をS3ごと削除する。

Content-Type について: `video.bsky.app`からの代理POSTは実機で
`Content-Type: */*`という無効なワイルドカード値を送ってくることがあるため、
ヘッダーをそのまま信用せず、`*`を含む・空の場合はマジックバイトから
`sniff_mime_type`で実際のMIME typeを判定する。

### 12.4 `bsky_video_poll`ジョブと`commit_post`への反映

動画アップロード時（`deliver_to_bsky`が有効な場合）、アップロードAPI内で
同期的に`uploadVideo`を1回叩いて`jobId`を取得し、`media_files`に
`bsky_video_job_id`/`bsky_video_status='pending'`を保存した上で
`Job::BskyVideoPoll`をJobQueueに積む（§2の6番目のキュー参照）。
このジョブは`getJobStatus`を1回叩いて未完了なら`Err`を返し、
`WorkerEngine`の既存リトライ機構（固定3秒間隔・最大10回=30秒）に
乗せて再試行させる。完了したら`bsky_video_cid`/`bsky_video_status='ready'`
を保存する。

投稿作成時（`commit_post`、`crates/seiran-common/src/atp/service.rs`）は
`media_files.bsky_video_status`を見て、`'ready'`なら`app.bsky.embed.video`、
そうでなければ（`pending`のまま・`failed`・音声等）`app.bsky.embed.external`
にフォールバックする。

**2026-07-17 修正**: 従来は投稿API自体が`bsky_video_status`の完了を待たず
（アップロード猶予の間に完了していればラッキー、という設計）、投稿ボタンを
押すタイミングが早いと常に`external`フォールバックに固定され、以後
再コミットもされないため video embed 化されないままになる不具合があった
（マイケル実機確認・再現）。動画添付があり`bsky_video_status`が未確定
（`NULL`/`pending`）の場合、Bskyコミット自体を`Job::BskyPostCommitDeferred`
（`crates/seiran-common/src/jobs/bsky_post_commit_deferred.rs`）に委譲する
ようにした。固定3秒間隔・最大20回（60秒）リトライし、`media_files.created_at`
からの経過時間が70秒を超えたらタイムアウトとして未確定のままフォールバック
コミットする（`crates/seiran-api/src/handlers/notes/delivery.rs`の
`has_pending_video`/`deliver_regular_post`）。

## 13. ポストのピン留め（#61）

ローカルユーザーは自分のポストを最大5件までピン留めでき、5件を超えると最古のもの
から自動的に外れる。ピン留めは Fedi 向けプロフィール（AP Actor の `featured`
collection）・Bsky 向けプロフィール（`app.bsky.actor.profile` の `pinnedPost`）の
両方に反映する（送信）。加えて、Fedi/Bsky の**リモートアクター**のプロフィールを
seiran で閲覧した際、そのアクター自身のピン留めを取り込んで表示する（受信）。DB
設計は Doc1 §1.13 を参照。

### 13.1 送信: Fedi featured collection

- `crates/seiran-federation-inbox/src/handlers/actor.rs` の Actor ドキュメントに
  `featured: "https://{domain}/users/{username}/collections/featured"` を含める。
- `GET /users/:username/collections/featured`（`handlers::featured::featured_handler`）が
  `pinned_posts` を `pinned_at DESC` で結合し、`OrderedCollection` を都度動的生成して返す
  （`orderedItems` に Note オブジェクトを `Create` でラップせず直接列挙、最大5件のため
  ページングなし）。Mastodon 等の実装はプロフィール取得時にこの URL を都度フェッチする
  想定で、`Add`/`Remove` Activity のフォロワーへの配送は行わない（スコープ外、将来課題）。

### 13.2 送信: Bsky `pinnedPost`

- `app.bsky.actor.profile` レコード（`encode_bsky_actor_profile`,
  `crates/seiran-common/src/atp/repo.rs`）に `pinnedPost`（`com.atproto.repo.strongRef` =
  `{uri, cid}`）フィールドを追加。DAG-CBOR canonical 順は
  `$type(5) < avatar(6) < createdAt(9) < pinnedPost(10) < description(11) < displayName(11)`
  （キー長→同長は辞書順）。
- `AtpCommitService::commit_profile` に `pinned_post: Option<(String, String)>` 引数を追加。
  Bsky はピン留め1件までのため、`pinned_posts` テーブルの先頭（`pinned_at` 最新）のみを渡す。
  対象ポストに `at_uri`/`at_cid` が無い（ATP 上に存在しない）場合は `None` を渡す。
- `handlers::notes::pin_note`/`unpin_note` は pin/unpin のたびに
  `handlers::notes::sync_bsky_pinned_post` を呼び、現在の display_name/bio/avatar を
  DB から読み直した上で `commit_profile` を再コミットする（display_name 等は維持し
  `pinnedPost` のみ更新するため）。

### 13.3 受信: リモート Fedi アクターの featured 取り込み

- `crates/seiran-common/src/ap/client.rs` の `ApActor` に `featured: Option<String>` を追加。
- `crates/seiran-common/src/ap/outbox.rs` の `fetch_ap_featured` が `featured` URL の
  `OrderedCollection` を取得し、`orderedItems` の各要素を `ApNote` にデコードする
  （`type: "Note"` 直下・`type: "Create"` でラップの両方に対応、`extract_note_flexible`）。
  取得・パース失敗時はベストエフォートで空配列（プロフィール表示自体は失敗させない）。
- `upsert_ap_note` が各 `ApNote` を `posts` テーブルへ反映し（`ap_object_id` で重複排除、
  `ActorHistorySync` の `save_ap_notes` と同じ INSERT パターン）、ローカル `post_id` を返す。
- `crates/seiran-api/src/handlers/users.rs` の `sync_remote_fedi_pinned` が
  `build_profile_response` 内で（対象が `actor_type == "fedi"` かつリモートの場合のみ）
  上記フェッチ→取り込み→`pinned_posts.sync_from_remote` を同期的に実行する。
- **DB 未登録の未知アクター（初回アクセス）でも同期する**（マイケルの要望・2026-07-15:
  「初回アクセス時も同期するよう拡張する」）。`fetch_remote_profile`（WebFinger 解決経路）は
  `ApClient::fetch_actor` で取得した `ApActor` を即座に `ActorRepository::upsert_remote_fedi`
  でDBへ登録してから `build_profile_response` に委譲する（`jobs::inbound_activity_process`
  の `upsert_remote_fedi_actor` と同じ upsert パターン）。これにより「一度も見たことがない
  リモートFediアクター」でも初回表示からピン留め・`recent_posts` が反映される（以前は
  `recent_posts`/`pinned_posts` とも空を返す非永続表示だった）。upsert 失敗時は従来通りの
  非永続フォールバック表示。
- **既知の不具合修正（本機能の実装中に発見）**: `upsert_ap_note` が保存する `body` は AP
  Note の `content`（HTML、Mastodon 等は `<p>`/`<a>` 等でラップして送る）をそのまま使って
  いたため、フロントでHTMLタグが素のまま表示される不具合があった。他の受信経路
  （`handle_create_note`）と同じ `strip_html`（`jobs::inbound_activity_process`）を通す
  よう修正。同じ問題は既存の `ActorHistorySync::save_ap_notes`（フォロー時の過去ログ
  一括取り込み）にもあったため、合わせて修正した。

### 13.4 受信: リモート Bsky アクターの `pinnedPost` 取り込み

- `crates/seiran-common/src/atp/client.rs` の `BskyProfile` に
  `pinned_post: Option<BskyPinnedPostRef>`（`{uri, cid}`）を追加。
  `app.bsky.actor.getProfile` のレスポンスをそのままデコードする。
- `pinned_post` があれば既存の `fetch_single_bsky_post`（`app.bsky.feed.getPosts`）で
  本文を取得し、`upsert_bsky_post` が `posts` テーブルへ反映（`at_uri` で重複排除）して
  ローカル `post_id` を返す。
- `crates/seiran-api/src/handlers/users.rs` の `sync_remote_bsky_pinned` が
  `fetch_bsky_profile_from_appview` の DB 登録済みアクター分岐から呼ばれ、
  `pinned_posts.sync_from_remote` で0〜1件に同期する。DB 未登録（AppView 直接表示）の
  場合は対象外（`recent_posts` も空のパスと同じ扱い）。

### 13.5 API 公開・フロントエンド表示

- `ProfileResponse.pinned_posts`（`NoteResponse[]`、`recent_posts` と同形式）として返す。
  自分自身のプロフィールを見ている場合のみ、各 `NoteResponse` に `pinned_by_me` を付与する。
- フロントエンドはプロフィール画面の中央ペイン（本人プロフィールカード直下）にピン留め
  セクションを表示し、右ペインには従来通り最新ポストを時系列表示する（`ProfilePage.tsx`）。
  右ペインが無い狭幅（`AppShell.module.css` の `max-width: 1400px` ブレークポイントと
  同期した `matchMedia` 判定）では、中央ペインにピン留め→最新ポストの順で連続表示する。
- `NoteCard.tsx` は投稿者が自分自身（`actorType === "local"` かつ `username` 一致）の場合
  のみピン留めトグルボタンを表示し、`POST`/`DELETE /api/notes/:id/pin` を叩く。

## 14. リスト機能（#63）

ユーザーごとに複数のリストを持て、ローカル/リモートFedi/Bskyのアクターを混在して
メンバー登録できる機能。DB設計はDoc1 §1.14を参照。公開リストはFediverseにはAP
Collectionとして、BlueskyにはATP `app.bsky.graph.list`/`listitem`として実際にPDS
リポジトリへコミットし、双方のプロトコルからネイティブに閲覧できる。

### 14.1 プロキシアクター（list-relay）によるFedi代理フォロー

誰にもフォローされていないリモートFediユーザーの投稿を受信するため、seiranは
`actor_type='local'`・`user_id=NULL`の仮想アクター`list-relay`を持つ
（`seiran_common::system_actor::ensure_system_proxy_actor`が起動時（`seiran-api::
init_state`）に冪等生成し、`actor_id`を`site_settings`キー`system_proxy_actor_id`に
記録）。AP署名はローカルアクター共通のサーバー単一RSA鍵（`Secrets.ap_private_key_pem`）
を流用するため専用鍵ペア生成は不要。`follows`テーブル・`FollowRepository`はフォロワー
種別を一切区別しない汎用実装のため、プロキシのフォロー管理も既存リポジトリがそのまま
使える。

- **参照カウント方式**: フォロー要否は`ListRepository::actor_referenced_by_any_list`
  （`list_members`テーブルからの動的COUNT）で判定する。メンバー追加時、追加前の時点で
  どのリストからも参照されていなければ（0→1遷移）`Job::ProxyFollowSync{want_follow:
  true}`をenqueueする。メンバー削除・リスト削除時、削除後に参照が0件になれば
  （1→0遷移）`want_follow: false`をenqueueする。
- **ジョブハンドラ**: `seiran_common::jobs::proxy_follow_sync::handle`が、list-relayの
  `actor_id`を`site_settings`から解決し、対象アクターへFollow/Undo Followアクティビティ
  を組み立てて送信、`follows`テーブルを更新する。既存のフォロー状態（`find_status`）を
  確認してから送信するため、複数リストからの重複呼び出しに対しても冪等に振る舞う。
- **予約ユーザー名**: `list-relay`は一般ユーザーが登録できない予約名として`register()`
  が明示的に拒否する（`seiran_common::username::RESERVED_LOCAL_USERNAMES`）。ユーザー名の
  DNSラベル準拠バリデーション（ピリオド・アンダースコア禁止）も同モジュールで実装し、
  register()に組み込んでいる（Doc1 §1.2参照）。

### 14.2 Bluesky受信フィルタ（Jetstream）

`crates/seiran-atp-repo/src/firehose.rs`のJetstreamハンドラは、DIDが以下いずれかを
満たす場合のみ投稿を保存・WebSocket配信する（`INNER JOIN follows`を`WHERE`句のOR
条件に変更）:

```sql
WHERE a.at_did = $1
  AND (
    EXISTS (
      SELECT 1 FROM follows f JOIN actors follower ON follower.id = f.follower_actor_id
      WHERE f.target_actor_id = a.id AND f.status = 'accepted' AND follower.actor_type = 'local'
    )
    OR EXISTS (SELECT 1 FROM list_members lm WHERE lm.actor_id = a.id)
  )
```

WebSocket配信対象（ローカルフォロワー）も同様に、その投稿者をリストに含めている
リスト所有者をUNIONで加える。過去に無関係投稿の取り込みで`posts`テーブルが104万行超まで
膨張した事故があるため、この条件以外での取り込みは行わない。上記のDB側フィルタは
`wantedDids`導入後も保険として残している（後述、絞り込みリスト更新の反映ラグの間に
無関係な投稿が届いても、ここで弾かれる）。

**cursorによるバックフィル（2026-07-16）**: 従来は`JETSTREAM_URL`に`cursor`パラメータを
付与しておらず、プロセス起動・再接続のたびに「今この瞬間」からのライブ配信として接続
していたため、サーバー停止中（デプロイ・クラッシュ等）に発生したイベントを再起動後に
取得できず取りこぼす問題があった。対処として、受信メッセージに全種別共通で付与される
`time_us`（マイクロ秒Unixタイムスタンプ）を5秒間隔で`site_settings`（Doc1 §1.11、
キー`jetstream_cursor`）に保存し、`connect_and_process`の接続開始時にこの値を読み出して
`&cursor=<time_us>`をURLに付与する。cursor未保存（初回起動等）の場合のみ従来通り
cursorなしで接続する。専用テーブルは設けず既存の汎用KVテーブルに間借りしている。

**`wantedDids`によるサーバー側絞り込み（2026-07-16）**: 従来はJetstream側でDID絞り込みを
行わず（`wantedCollections`のみ）、Bluesky全体のpost/likeイベント全件に対して上記のDB側
フィルタ（投稿ごとに1〜数回のクエリ）を発行していたため、フォロイー数に関わらずグローバル
なイベント量に比例したDB負荷が発生していた。対処として`load_wanted_dids`
（`crates/seiran-atp-repo/src/firehose.rs`）が、ローカルユーザーのフォロー先（ATPフォロー）
またはいずれかのリストメンバーであるBskyアクターのDID一覧を取得し、`wantedDids`パラメータ
としてJetstream接続URLに付与する（Jetstream側の上限は1接続あたり10,000 DID）。退会済み
（`actors.withdrawn_at`設定済み）ローカルユーザーのフォロー・所有リストは対象から除外する。
対象DIDが1件も無い場合は絞り込みなし（従来通り全世界のpost/like）で接続するフォールバックと
する。当初`actors`（`at_did`を持つ全件）から出発し`follows`/`list_members`をEXISTSで判定する
書き方で実装したところ、既知アクター数十万件規模のフルスキャンになり実測1秒近くかかったため、
`follows`/`list_members`（少数行）を起点にJOINでDIDを引く書き方に修正した（実測0.5ms台まで
短縮）。

このDID集合はフォロー・リストメンバーの増減で動的に変わるため、`seiran_common::
jetstream_control`（`touch_jetstream_wanted_dids`/`fetch_wanted_dids_touch`）が
`site_settings`（キー`jetstream_wanted_dids_touch`）の`updated_at`を「変更バージョン」
として使い、DBを介してプロセス間に変更を通知する。`firehose`ロールは split-role構成では
`api`ロールと別プロセス（別コンテナ）で動くため、プロセス内通知（`tokio::sync::Notify`等）
は届かない前提で、`connect_and_process`の受信ループが30秒間隔でこの値をポーリングし、
接続時点の値と異なれば`Ok(())`を返して再接続（新しいDID集合で`wantedDids`を再構築）する。
`touch_jetstream_wanted_dids`の呼び出し箇所: ATPフォロー作成・解除
（`crates/seiran-api/src/handlers/follows.rs`の`follow_bsky`/`delete_follow`）、リスト
メンバー追加・削除・リスト削除（`crates/seiran-api/src/handlers/lists.rs`の`add_member`/
`remove_member`/`delete_list`）、ローカルユーザー退会（`crates/seiran-api/src/handlers/
account.rs`の`withdraw`）。「凍結」（`users.suspended_at`）はログイン制限のみで
フォロー配信に影響しない別軸の機能のため、トリガーには含めない。

**退会時のフォロイー一括アンフォロー（2026-07-16）**: 従来は退会処理（`account::withdraw`）
がフォロワー側（Delete(Actor)配送、Doc1 §1.2参照）とATP #accountイベントのみで、自分が
フォローしていた相手（フォロイー）側へは何も通知しておらず、リモート側にフォロー関係が
残り続ける不整合があった。フォロー数に比例して時間がかかりうる処理のため、Delete(Actor)
配送（`ApDelivery`ジョブ）・list-relayの代理フォロー同期（`ProxyFollowSync`ジョブ）と同様に
Workerのジョブとして実装する（新設`Job::AccountWithdrawUnfollowAll`、
`seiran_common::jobs::account_withdraw_unfollow_all`）。`withdraw`ハンドラは
`AppState::enqueue_account_withdraw_unfollow_all`でジョブを積むのみで、実処理はWorkerが
リトライ機構（最大10回、5秒〜1時間の指数バックオフ、`ApDelivery`/`ProxyFollowSync`と同じ
設定）付きで実行する。当初は`tokio::spawn`で非同期化する実装だったが、プロセスクラッシュ時に
タスクごと失われリトライもされない点を指摘され（2026-07-16 マイケル）、既存のジョブキュー
パターン（`Job`/`WorkerEngine`/`JobContext`）に載せる設計に変更した。

ジョブハンドラ内では`FollowRepository::find_accepted_target_ids`で自分がフォローしていた
全ての`target_actor_id`を取得し、各々についてATPフォロー解除コミット・AP Undo Follow配送・
`follows`テーブル削除を行う。ATPコミット用の`AtpCommitService`はジョブ専用の使い捨て
`broadcast`チャンネルで構築する（`subscribeRepos`のリアルタイム購読者には届かないが、
`atp_repo_events`テーブルへの記録自体は行われるため、他のRelayが再購読すれば最終的には
一貫する。退会時のフォロー解除にリアルタイム性は必須ではないと判断）。`follows`行は
ターゲットごとに処理の最後で削除するため、リトライ時は既に処理済みのターゲットが
自然にスキップされる（冪等）。
実機確認: ローカルフォロー（裏でATP followレコードも作成される設計、Doc1 §1.5）を持つ
テストユーザーを退会させ、`Worker`ログでのジョブ実行（`attempt 1/10`）・ATP
`follow delete commit`実行（rkey一致）・`follows`行の削除を確認済み（2026-07-16）。

**Likeの通知重複の対処（2026-07-16）**: `notifications`テーブルに`source_uri`カラムと
部分ユニークインデックスを追加（Doc1 §1.12参照）。firehose/federation-workerの複数起動
（下記「Jetstream接続の排他制御」参照）で同一イベントが複線受信されても、通知は
`ON CONFLICT ... DO NOTHING`により1行に収束する。

**Jetstream接続の排他制御（リーダー選出、2026-07-16）**: `docker-compose.mono.yml`の
`--scale seiran-server=N`（無停止バージョンアップ中の一時的な複数起動）や`firehose`ロールの
複数インスタンス起動では、対策が無いとJetstream WebSocket接続がインスタンス数だけ重複して
張られる。`seiran_common::jetstream_leader`（`JetstreamLeaderElector`）がRedisのTTL付き
リース（キー`seiran:jetstream:leader`、`SET NX EX 10`）でリーダーを1つに絞り、`firehose.rs`
の制御ループ（`run`）が5秒間隔でリースの取得・延長を試みて、成否に応じてJetstream接続タスク
を`tokio::spawn`/`abort`で起動・停止する。プロセスIDではなくUUIDでリーダーを識別する
（Dockerコンテナ間でPID 1が衝突するため）。TTL延長は「現在の値が自分のUUIDと一致する場合
のみ延長」を`EVAL`（Luaスクリプト）でアトミックに行う。GET→SETの2ステップに分けると、TTL
失効の瞬間に他プロセスが横取りした直後に古いGET結果を根拠にSETしてしまいsplit-brainになる
理論的な穴があるため。Redis呼び出し（接続確立・リース確認）には3秒のタイムアウトを設定
（`redis`クレートの`ConnectionManager::new`がRedis無応答時に内部リトライで長時間ブロック
し、ポーリングループ自体が停止する不具合を実機検証で発見したため）。

Redis未設定・通信失敗時（両者は「Redisと通信できない」の特殊ケースとして同一視）は、ロール
に応じてフェイルオープン/フェイルクローズする。`all`ロールはJetstream接続を維持する
（monolithの複数起動時の非効率は許容する方針）。`firehose`ロール（split-role構成）は切断
する（Redisが死ねばジョブキュー等の他機能も共倒れになるため、firehose接続だけ動かしても
意味がない）。Redisダウンによる多台体制の崩壊からの自動復帰は要求していない（人手での
インフラ復旧が前提。マイケル判断）。

### 14.3 AP Collection公開

`crates/seiran-federation-inbox/src/handlers/lists.rs`が、`featured_handler`
（Doc3 §13.1）と同型のパターンで実装する。

- `GET /users/:username/lists` — そのユーザーの公開リスト一覧を`OrderedCollection`
  で返す（`orderedItems`は各リストのCollection URLを列挙、`featured`と異なりアイテムを
  インライン展開しない）。
- `GET /users/:username/lists/:list_id` — 個別リストのメンバーを`OrderedCollection`
  で返す。`orderedItems`は`actor_type <> 'bsky'`でフィルタしたメンバーのActor URI
  （ローカルは`https://{domain}/users/{username}`を動的生成、リモートFediは`ap_uri`を
  そのまま使用）。非公開リスト・存在しないリストはいずれも404（非公開リストの存在を
  漏らさない）。
- Actor ドキュメント（`actor_handler`）に`lists: "https://{domain}/users/{username}/lists"`
  フィールドを追加（`featured`と同じ位置づけ、Mastodonにはない独自拡張のためリモート側が
  無視しても実害はない）。

### 14.4 ATP `app.bsky.graph.list` / `app.bsky.graph.listitem`

`crates/seiran-common/src/atp/repo.rs`に、既存の`encode_bsky_graph_follow`と同型で
`encode_bsky_graph_list`/`encode_bsky_graph_listitem`を実装。DAG-CBOR canonical
フィールド順（キーのバイト長昇順、同長ならバイト列辞書順）に注意:

- `app.bsky.graph.list`: `name`(4) < `$type`(5) < `purpose`(7) < `createdAt`(9)。
  `purpose`は常に`app.bsky.graph.defs#curatelist`固定（モデレーションリストは対象外）。
- `app.bsky.graph.listitem`: `list`(4) < `$type`(5) < `subject`(7) < `createdAt`(9)。
  `list`は対象リストの`at://did:.../app.bsky.graph.list/<rkey>`形式URI。

`AtpCommitService`（`crates/seiran-common/src/atp/service.rs`）に以下を追加:

- `commit_graph_list(actor_id, name, now) -> (rkey, at_uri, cid)` — `commit_follow`と
  同型の薄いラッパー（内部で共通の`commit_record_inner`を呼ぶ）。
- `commit_graph_listitem(actor_id, list_uri, subject_did, now) -> (rkey, at_uri)`。
- `delete_atp_graph_list`/`delete_atp_graph_listitem` — `delete_atp_repost`と同型の
  削除処理を`delete_atp_record_generic`という私設汎用ヘルパー（collection名を引数化）
  に集約し、2つの薄いラッパーとして提供する。

呼び出し元（`crates/seiran-api/src/handlers/lists.rs`）の方針:

- リスト作成時・非公開→公開トグル時: `commit_graph_list`を呼び、`lists.at_rkey/at_uri/
  at_cid`に保存する。非公開→公開トグル時は既存メンバー（`actor_type <> 'fedi'`かつ
  `at_did`を持つもののみ）もまとめて`commit_graph_listitem`する。
- メンバー追加時（**公開リストかつ**対象が`actor_type <> 'fedi'`の場合のみ）:
  `commit_graph_listitem`を呼び、`list_members.at_rkey/at_uri`に保存する。非公開リストや
  Fedi専用メンバーには一切ATPレコードを書かない。
- メンバー削除・リスト削除・公開→非公開トグル時: 対応する`delete_atp_graph_listitem`/
  `delete_atp_graph_list`を呼び、DB側のATPフィールドをクリアする。CASCADE削除で
  `list_members`行が消える前（リスト削除時）に、削除すべき`(actor_id, rkey)`一覧を
  事前に読み出しておく必要がある点に注意。
- **既知の制約**: 公開済みリストの名前変更はATP側レコードの内容に追従しない
  （rkeyは作成時に固定され、再エンコードのタイミングが無いため）。非公開→公開の
  再トグルで初めて最新の名前が反映される。

### 14.5 API・フロントエンド

- `POST/GET/PATCH/DELETE /api/lists(/:id)`、`POST/DELETE /api/lists/:id/members(/:actor_id)`、
  `GET /api/lists/:id/timeline`（`crates/seiran-api/src/handlers/lists.rs`）。
- リストタイムラインは`PostRepository::home_timeline`と同じtargets-LATERAL集約パターンを
  `ListRepository::timeline`として再利用する（targetsの元を「自分+フォロー中」から
  `list_members`に差し替えるのみ）。
- フロントエンド: `ListsSettingsPage.tsx`（`/settings/lists`、自分のリスト管理）、
  `HomePage.tsx`のタブ拡張（リストごとのタブ、横スクロール対応）、`ProfilePage.tsx`の
  `listsSection`（他人の公開リスト一覧）、`ListDetailPage.tsx`（`/lists/:id`、リスト
  タイムライン＋メンバー一覧の閲覧専用ページ）。
- 上限: `MAX_LISTS_PER_OWNER=30`、`MAX_MEMBERS_PER_LIST=500`（提案値、
  `crates/seiran-common/src/repository/list.rs`の定数）。

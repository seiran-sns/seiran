# Doc 4. 検索ステート ＆ セッションマネージャー仕様 (Search State & Session)

## 1. `SearchSession` のライフサイクル定義とプラガブル管理

HTTPプロトコルはステートレスであり、フロントエンドがいつ検索画面を閉じたかをバックエンドは検知できない。そのため、バックエンドのメモリ（または外部Redis）上に「砂時計（セッション）」を定義し、リソースを自律管理する。

### 1.1 セッションオブジェクト構造体
```rust
pub struct SearchSession {
    pub query: String,                             // 検索クエリ文字列
    pub appview_cursor: Option<String>,            // AppViewから返ってきた次回用カーソル
    pub unreturned_appview_posts: Vec<Post>,       // 取得済みだがフロントに未返却のポストリスト
    pub last_accessed_at: DateTime<Utc>,           // 最終アクセス日時（寿命延長の主軸）
    pub appview_exhausted: bool,                   // AppView側の過去ログを掘り尽くしたか
}
```

### 1.2 寿命管理（スライディングタイムアウト）
* **セッションの寿命:** **10分間**。
* フロントエンドから検索リクエストが届くたびに、残り寿命を10分後まで一律延長（スライディング）する。
* メモリ/Redisの期限切れ自動削除（TTL）機能を利用して、10分以上アクセスのないセッションは自動的に破棄される。

### 1.3 メモリ高負荷時のEviction（追い出し）ポリシー
* インメモリ保存時、同時接続ユーザー数が閾値を超えメモリが逼迫した場合、セッションストアは最終アクセス時刻が最も古いセッション（LRU方式）から順にメモリ空間から強制パージ（Eviction）する。
* パージされたセッションに対してフロントから追加スクロール（`untilId`）が届いた場合は、[2.3 セッション消滅時フォールバック] が即座に発動し、安全にローカルDB検索へと着地させる。

### 1.4 プラガブルな保存先抽象化 (`SessionStore` Trait)
開発初期はRedisなし（インメモリ）で動作させ、将来スケールアウトする際にRedisへシームレスに切り替えられるよう、マネージャー層は `SessionStore` Trait を介してセッションを読み書きする。

```rust
#[async_trait]
pub trait SessionStore: Send + Sync {
    async fn get_session(&self, session_id: &Uuid) -> Result<Option<SearchSession>, StoreError>;
    async fn set_session(&self, session_id: Uuid, session: SearchSession, ttl: Duration) -> Result<(), StoreError>;
    async fn delete_session(&self, session_id: &Uuid) -> Result<(), StoreError>;
}
```
* **InMemorySessionStore (初期)**: Rustの `dashmap` または `RwLock<HashMap>` 等を用いてオンメモリで保持。
* **RedisSessionStore (スケール時)**: ジョブデータを JSON 等にシリアル化し、Redis の `GET` / `SETEX` を用いて保持（Redis側で自動TTL制御）。

---

## 2. 検索ページネーション・ブレンドアルゴリズム

フロントエンド（Misskey API互換）の「IDベースの要求」を、AppViewの「カーソルベースの要求」へと動的にマッピング・翻訳する。

### 2.1 初回リクエスト時（新規検索）
1. セッションID（UUID）を新規発行。
2. ローカルDB（インデックス検索）から `limit 30` 件、AppView（`app.bsky.feed.searchPosts`）から `limit 30` 件を同時に非同期フェッチ（`tokio::join!`）する。
3. AppViewから取得した30件のポストをローカルDBにインサートし、統一IDを付与する。
4. ローカルDB分とAppView分を、統一ポストID（タイムスタンプ）の降順でソート（織り合わせマージ）する。
5. 上位30件（`limit` 分）をフロントへ返却する。
6. 残った下位のポスト群（未返却バッファ）と、AppViewから受け取った次ページ用 `cursor` を `SearchSession` に格納し、メモリに保存する。

### 2.2 過去掘りリクエスト時（`untilId` アクセス）
1. フロントからセッションIDと `untilId` が届く。セッションの寿命を10分延長。
2. **バッファ残数評価:** セッション内の `unreturned_appview_posts` の件数が、要求された `limit`（30件）よりも**少ない**場合、保持している `appview_cursor` を使ってAppViewへ追加リクエスト（追記フェッチ）を即座に実行する。
3. 新たに返ってきたリザルトをDB格納の上、バッファの末尾に追加マージし、カーソルを更新する。
4. ローカルDBから `untilId` より過去の検索結果を `limit` 件追加取得する。
5. 潤沢になったバッファリストと、ローカルDBの追加取得分をタイムスタンプで再度織り合わせ、上位30件を返却。漏れた分は次回のバッファとしてセッションに再格納する。

### 2.3 未来掘りリクエスト時（`sinceId` アクセス）＆ セッション消滅時
* **`sinceId` アクセス時:** AppView側への問い合わせは行わず、**ローカルDBの検索のみで完結させる（完全信頼フォールバック）**。過去の検索フェーズを通過したAppViewポストはすでに100%ローカルDBにインサート済みであるため、`WHERE id > :since_id AND body % :query` を掘るだけで、取りこぼしなく新着差分を網羅できる。
* **セッション消滅時:** スライディングタイムアウト等でセッションが既にメモリから消滅している状態でリクエストが届いた場合、エラーを返さず、**通常のローカルDB検索に自動フォールバック**してベストエフォートで結果を返し続ける。

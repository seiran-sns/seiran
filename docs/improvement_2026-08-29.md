# 改善大会 指示書（2026-08-29）

対象: `main` の `a90585c`。実測は原則ローカル開発 DB（`seiran-db-1`、事実上唯一の本番相当データ）。
前回の包括監査 `code_audit_2026-08-05.md` 以降 144 コミット。本書はその監査項目の**消化状況を反映し、
未解消・新規のみ**を実装タスクとして並べる。実装は Sonnet を想定。Fable/Opus 級が要る項目は §7 に明示。

## 0. 前回監査からの消化状況（再掲しない項目）

以下は現行コードで対応済みを確認した。指示対象から除外する。

- S-1 の一部（`/api/auth/login`・register・password-reset のレート制限、`rate_limit.rs`）／S-2 トークン一括失効（`token_valid_after`）／S-4 連合 HTTP の timeout（`net.rs`・`http_client`）／S-5 react-router 7.18.2 移行／S-6 ログインのダミーハッシュ照合／S-7 DB ポートの `127.0.0.1` 限定／S-9 `cargo audit` の CI 追加
- P-3 AP 配送の並列化（`deliver.rs` の `buffer_unordered`）／P-4 の N+1（`find_by_ids` 一括取得）／P-8 プール（`DB_MAX_CONNECTIONS` + `acquire_timeout` 5s）
- R-1 管理ルートの `route_layer` 認可／R-2 可視性 SQL の関数化（`post_is_visible_to` / `post_reply_target_followed`）／R-5 `atp_blocks` 一括 INSERT の集約／R-9 の `/health`

前回「次回候補」だった `ListsSettingsPage.tsx` は 101 行に分割済みで消化。

---

## 1. セキュリティ

### [SEC-1・高] `createSession` にレート制限がなく、ログインの総当たり対策を迂回できる

- 場所: `handlers/xrpc/server.rs` `xrpc_create_session`
- `/api/auth/login` は `rate_limit::check_and_record_credential_attempt` で試行を絞るが、
  同じくパスワードで認証する AT Protocol の `com.atproto.server.createSession`
  （`/xrpc/com.atproto.server.createSession`）には一切レート制限がない。しかも本アカウントの
  **メインパスワードでの認証を許可している**（`find_login_by_username` 経由）ため、この経路は
  login と同等の資格を、絞りなしで総当たりできる裏口になっている。
- 対応: `xrpc_create_session` の入口で `check_and_record_credential_attempt`（`AttemptKind` は
  既存 enum に `Login` があればそれ、無ければ新設）を、identifier→actor 解決後の識別子で呼ぶ。
  ユーザー不在時も試行として記録する（login と同じ扱い）。

### [SEC-2・中] CORS が `allow_origin(Any)` + `allow_headers(Any)` のまま（前回 S-3 未対応）

- 場所: `lib.rs:631` 付近 `CorsLayer::new().allow_origin(Any)`
- 認証は `Authorization` ヘッダーで `allow_credentials` 無しのため古典的 CSRF は成立しないが、
  任意サイトの JS がユーザーのブラウザを踏み台に公開 API を無制限に叩ける。SEC-1 を塞いでも、
  分散スクレイピング・分散試行の踏み台余地が残る。
- 対応: `FRONTEND_ORIGIN` と自ドメインのみ許可するリストへ。連合・Misskey 互換クライアント向けに
  開放が要る経路があっても `allow_headers` は `authorization, content-type` に絞れる。

### [SEC-3・中] `atproto-proxy` XRPC の転送先 SSRF 余地を確認・封じる

- 場所: `handlers/xrpc/proxy.rs`（`resolve_service_endpoint(target_did, service_id)`）
- ユーザーが `atproto-proxy: <did>#<service>` ヘッダーで指定した DID の serviceEndpoint へ
  サーバーがサービス JWT 付きで転送する。転送先は DID ドキュメント由来だが、**攻撃者管理下の DID の
  serviceEndpoint を内部アドレス（メタデータサーバー等）に向ければ SSRF になりうる**。他の外部フェッチ
  （`net.rs` の `validate_url` + `resolve_to_addrs` + `Policy::none`）が備える private/link-local 拒否・
  リダイレクト固定を、この経路が通っているか確認する。
- 対応: `resolve_service_endpoint` の結果 URL を `net.rs` の検証（IP レンジ拒否）に通してから転送する。
  転送クライアントもリダイレクトを追わない設定にする。※実装前に proxy.rs の現状挙動を精読して要否を判断。

### [SEC-4・対応済み] backend コンテナの非 root 実行（前回 S-8）

- 2026-08-29 に確認したところ、commit `9fb7e89`「seiran-server/frontendコンテナを非rootユーザーで
  実行する」で**既に完了済み**（08-06）。`docker/Dockerfile` で uid 10001 の `seiran` ユーザーを
  作成し、`docker/entrypoint.sh` が起動時に root で `/app/config` の所有権を揃えたのち
  `gosu seiran` で降格して本体を exec する。本改善大会の診断時点でこれも見落としていた。

---

## 2. RDB パフォーマンス

### [PERF-1・対応済み] 計測基盤（前回 P-9）

- 2026-08-29 に確認したところ、`docker-compose.yml`/`docker-compose.mono.yml` の
  `command: ["postgres", "-c", "shared_preload_libraries=pg_bigm,pg_stat_statements"]`と
  マイグレーション `20260805030000_enable_pg_stat_statements.sql` で**既に対応済み**
  （08-05、監査当日中に着手されていた）。ローカルDBで `pg_extension` に存在確認済み。
  本改善大会の診断時点でこの事実を見落として「未着手」と誤記していた。以後の PERF 診断は
  `pg_stat_statements` を実測に使える前提で進めてよい。

### [PERF-2・対応済み（主要部）] `actors` の Bsky 流入分肥大（前回 P-5）

- 主要対応（永続化条件を関与ベースに絞る）は commit `127ae32`
  「Bsky流入アクターの保存を関与ベースへ絞る（issue #216）」で**既に完了済み**
  （`bsky_actor_is_engaged` 関数を保存判定の唯一の場所とし、無条件保存経路
  `Job::ResolveBskyMention` も削除済み）。本改善大会の診断時点でこれも見落としていた。
  2026-08-29 実測: actors 18,890 行 / 24MB（前回監査時 47 万行 / 292MB から大幅減）。
- 残作業（低優先度、将来の再肥大化への予防）:
  1. `ALTER TABLE actors SET (autovacuum_vacuum_scale_factor = 0.02)` — 2026-08-29 対応済み
     （マイグレーション `20260829060000_actors_autovacuum_tuning.sql`）。
  2. ユーザー検索インデックスの部分インデックス化: 現状の行数（18,890）では効果が
     ほぼ無いため見送り。actors が再び大きく増えた場合に再検討する。
- 注意: 現在の実測件数を先に取り直すこと（144 コミット分で状況が変わっている可能性）。

### [PERF-3・中] 大胆な整合性緩和 — リモート投稿取り込みの耐久性を落として書き込みスループットを稼ぐ

Jetstream 経由のリモート投稿 INSERT が posts 書き込みの主成分。**リモート投稿は失っても再取得可能**
（source of truth は相手 PDS / Relay）なので、この経路だけ耐久性を緩められる。

前提: firehose の取り込みは既に「カーソルの間引き保存（処理地点より常に手前）＋冪等 INSERT
（`ON CONFLICT DO NOTHING`）＋再接続時の DB 再読込」で、クラッシュ後に飛んだ区間を自動で
取り直せる設計になっている（`firehose.rs`）。よって案 A の追加コストは小さい。

- 判断順（マイケルと合意済み・2026-08-29）: **PERF-1 の計測で fsync/コミット待ちが実際に
  上位コストだと確認できてから**着手する。効いていなければ入れない。
- 案 A（推奨）: firehose の DB 接続に `synchronous_commit = off` を設定する（接続フックで
  `SET`）。クラッシュ時に直近 1 秒弱のリモート投稿が消え得るが、上記のカーソル設計で自動回復する。
  **注意**: モノリス（`all`）構成では firehose と api が DB プールを共有しているため、
  セッションレベルで SET するなら firehose 専用の小プールを分離してから行うこと。
  split-role の firehose は専用プールなのでそのままでよい。
- 案 A で不足し、かつ耐久性を落としたくない場合の代替: バッチコミット（複数投稿+カーソルを
  1 トランザクションに集約）。実装は重くなるが耐久性を保ったまま fsync 回数を減らせる。
- 案 B（`atp_repo_events` 等の `UNLOGGED` 化）: クラッシュでテーブル全消失・将来のレプリカに
  乗らない制約があるため、明確に再構築可能なデータ限定。優先度は案 A より下。

### [PERF-4・対応済み] フォロー/投稿カウントの非正規化（前回 P-4 の第 2 案）

- `actors` に `notes_count`/`followers_count`/`following_count` を追加（マイグレーション
  `20260829070000_actors_denormalized_counts.sql`、既存データからバックフィル済み）。
  `build_users_detailed`（Misskey互換）と`count_relations`（プロフィール画面）を読み替え、
  毎回のCOUNT(*)クエリ（`posts`全件・`follows`全件）を廃止した。
- 書き込み経路を全て洗い出して対応（`repository/post.rs`の`insert`/`insert_full`/`insert_repost`/
  `insert_repost_bsky`/`insert_remote`/`insert_remote_with_dedup`と`soft_delete_by_id`/
  `soft_delete_by_ap_object_id`/`soft_delete_by_at_uri`、`repository/follow.rs`の
  `insert_accepted`/`insert_accepted_bsky`/`accept`/`delete_by_actors`）。各メソッドは
  data-modifying CTE（`WITH x AS (INSERT/UPDATE/DELETE ... RETURNING ...) UPDATE actors ...`）で
  「実際に行が変化した場合のみ」単一SQL文内で加減算する設計にし、`ON CONFLICT DO NOTHING`で
  スキップされた場合や既に削除済みの行への重複操作で二重カウントしないようにした
  （`soft_delete_by_ap_object_id`/`soft_delete_by_at_uri`は元々`deleted_at IS NULL`ガードが
  無い/片方のみだった実装上の穴も同時に修正）。詳細は`docs/database.md`「非正規化カウンタ」・
  `coding_rules.md` #14参照。実DB上でCTEパターンの単体動作・冪等性・`follow_state_transition_integration`/
  `post_visibility_integration`の全既存テスト通過を確認済み。

### [PERF-5・対応済み] `atp_repo_events` の CAR バイト列を定期 NULL 化（前回 P-6）

- 2026-08-29 対応済み。ジョブキュー経由ではなく、`spawn_gc_tasks`（`crates/seiran-api/src/lib.rs`、
  media_files/atp_blobs の孤立ファイルGCと同じ1時間ごとの`tokio::time::interval`ループ）に
  `run_atp_repo_events_car_bytes_gc`を追加する形にした（既存の定期実行パターンに合わせた。
  ジョブキューには「一定間隔で永続的に走り続ける」ためのdelay付き再投入の仕組みはあるが
  自己再enqueue型は「完了したら止まる」チェーン用途で使われており、GC専用の
  `tokio::time::interval`ループの方が既存踏襲として自然だった）。
  72時間経過した`car_bytes`をUPDATE一括でNULL化、行・`ops_json`は残す。

### [PERF-6・見送り（判断材料不足）] 未使用インデックスの棚卸し（前回 P-2）

- 2026-08-29 に `pg_stat_user_indexes` で `idx_scan = 0` のインデックスを洗い出したが、
  **統計収集期間が `pg_postmaster_start_time()` 基準で 10 日間しかなく**、判断材料として
  不十分と判断し実施を見送った。候補に挙がった約30本は `idx_actors_search_bigm`
  （`GET /api/actors/search`）・`idx_password_resets_token`・`idx_reports_*`・
  `idx_poll_votes_*`・`idx_atp_blobs_*` 等で、いずれも実装上は正当な低頻度機能
  （検索・パスワードリセット・通報・投票・ATPブロブ）のインデックスであり、
  「10日間たまたま踏まれなかっただけ」の可能性が高い。前回 P-2 で実際に無駄と判明した
  `idx_follows_target_follower`/`idx_follows_follower_accepted`（計155MB）は既に
  DROP済みで現存しない（対応時期不明、本改善大会以前）。
- 再判断の目安: 統計収集期間が最低でも1〜2ヶ月に達してから、`idx_scan=0`が継続している
  ものだけを対象にする。DROP前に必ず`EXPLAIN`でプランナが実際に不採用であることを
  確認する運用は維持する。
- 別件として、`follows_pkey`（43MB, idx_scan=2）・`follows_follower_actor_id_target_actor_id_key`
  （75MB, idx_scan=606）はテーブル本体（400kB）に対し明らかに肥大化したままだった
  （過去の大量書き込みによるB-tree空きページ、前回 P-1 と同種の事象）。これはPK/UNIQUE制約
  実装なのでDROP対象外だが、`REINDEX TABLE CONCURRENTLY follows`で縮小可能。本改善大会の
  スコープ外（前回P-1として既出、今回未着手）として次回に残す。

---

## 3. リファクタリング（how と what の分離を主軸に）

### [REF-1・高] 巨大ファイルの再分割（how/what 混在の解消）

| ファイル | 行数 | 状態 |
|---|---|---|
| `jobs/inbound_activity_process.rs` | 3,788 | **対応済み**（`jobs/inbound_activity_process/` へ15ファイル分割。`handle_create_note`を検証/永続化・配送のhow/whatに分離） |
| `handlers/notes/mod.rs` | 2,478 | **対応済み**（`handlers/notes/{creation,timelines,retrieval,deletion,reactions,pins,poll,profile_material}.rs`へ分割。`create_regular_post`を`validate_create_regular_post_input`(what)/`persist_regular_post`(how)に分離） |
| `ap/deliver.rs` | 2,277 | **対応済み**（`ap/deliver/{infra,activity,note,announce,actor,reaction,text}.rs`へ分割。`infra`=配送機構(how)、`activity`=JSON構築純関数(what)、他は種別別オーケストレーション(how)） |
| `frontend/src/api/client.ts` | 2,113 | **対応済み**（`api/{core,types,webauthn,auth,notes,users,admin,follows,lists,misc}.ts`へ分割。`client.ts`は再エクスポート+`api`オブジェクト組み立てのみの薄い集約ファイルとして残し、既存の`import ... from "../api/client"`は無変更で動く） |

- 方針（how/what 分離）:
  - `inbound_activity_process.rs` → `activity_type` ごとにモジュール分割（`inbound/{follow,create,announce,...}.rs`）。
    ディスパッチ関数は「どのハンドラを呼ぶか」(what) だけを持ち、各処理(how) を各モジュールへ。
  - `notes/mod.rs` の `create_regular_post`/`create_reaction` 等は、
    「何を満たせば作成できるか」(what=バリデーション/権限) を純粋関数へ、
    「どう保存し配送するか」(how) をリポジトリ・ジョブ呼び出しへ分け、ハンドラは両者の配線に徹する。
    切り出した what 部分にユニットテストを付ける。
  - `client.ts` → `api/{notes,users,admin,drive,dm}.ts` にドメイン別、`ApiError`/`throwIfError`/
    `parseJsonBody` を `api/core.ts` に残す（前回監査 R-3 の素案どおり）。
- 規模が大きく文脈保持力を要するため §7 参照。

### [REF-2・対応済み（着手分）] `Result<_, String>` の型付け（前回 R-4）

- `traits::JobError { Transient(String), Permanent(String) }` を新設（`From<String>`で
  未移行のジョブは自動的にTransient扱いになり後方互換）。`execute_with_retry`（`queue/worker.rs`）
  がPermanentなら残り試行回数に関わらず即座に諦めるよう変更。
- 配送・外部API呼び出し系の代表として `jobs::ap_delivery` を移行し、`ap::client::ApError`
  から`JobError`への分類（`Json`/`Signature`→Permanent、`Http`/`FetchActor`/`Other`→Transient）を
  実装・テスト済み。`coding_rules.md` #13 に移行方針を明記。
- 残り（他のジョブへの展開）は一括置換しない方針のまま。触った箇所から順次移行する。

### [REF-3・対応済み] `sqlx::query!` 採用方針の明記（前回 R-4 関連）

- 2026-08-09 時点で既に `coding_rules.md` #11「新規クエリは `query_as!` を既定とする」が
  明記済みだった（本改善大会の診断時点でこれも見落としていた。既存の実行時検証クエリの
  一斉移行はコスト過大なので対象外のまま、触った箇所から移行する運用を継続）。

### [REF-4・対応済み（着手分）] テストが手薄な中心部（前回 R-8）

- 可視性判定（`post_is_visible_to`/`actor_is_hidden_for_viewer`、followers_only/direct/
  block/mute の組み合わせ）と `repository/follow.rs` の状態遷移（pending→accepted→取り消し、
  リモートFollow受信の`insert_accepted`冪等性）を結合テストとして追加
  （`crates/seiran-api/tests/{post_visibility_integration,follow_state_transition_integration}.rs`）。
  `tests/support/mod.rs` に軽量ハーネス `test_db_pool()`（axumルータ・認証を経由せず
  `PgPool` のみ取得）を新設。FK制約（`follows`/`blocks`/`mutes`/`post_recipients`が
  `actors`/`posts`を参照）があるため、テスト専用の固定ID fixtureを`ON CONFLICT DO NOTHING`で
  用意し、各テストが実際に検証する状態変更はトランザクション内で行いcommitしないことで
  ロールバックする設計にした。
- 残り: `fan_out_activity`（`ap/deliver/infra.rs`）の部分失敗集計はモックHTTPサーバーが
  要るため未着手。

---

## 4. ドキュメント整理（読み手＝次セッションの自分）

方針: docs/ は「今の状態」と「これからやること」だけを持つ。過去のスナップショット診断・実施報告・
廃止経緯は、次セッションが現状把握する際にノイズになるため統廃合する。

### 棚卸し表

| ファイル | 最終更新 | 判定 |
|---|---|---|
| `architecture.md` `database.md` `protocols.md` `ui_spec.md` `roadmap.md` | 08-28 | **維持**。現状仕様。CLAUDE.md の同期対象。 |
| `concept.md` `coding_rules.md` `roles.md` `skill_atp_rust_programming.md` | 07〜08 | **維持**。思想・規約・リファレンス。 |
| `improvement_security.md`(07-02) `improvement_db_performance.md`(07-10) `improvement_code_quality.md`(07-15) | — | **アーカイブ/削除候補**。大半が消化済み。未消化分は本書に取り込んだ。過去診断のスナップショットを現状把握用 docs に残す意味は薄い。 |
| `code_audit_2026-07-26.md` | 07-27 | **削除候補**。08-05 監査に置換済み。 |
| `code_audit_2026-08-05.md` | 08-05 | 当面**維持**（本書が参照する消化元）。本書消化後は削除してよい。 |
| `refactoring_report_2026-07.md` | 07-15 | **削除候補**。実施済み作業の報告＝過去経緯。ただし `notes/queries.rs` 非昇格の設計判断（B-7）だけは `coding_rules.md` か当該ファイル冒頭コメントに移してから削除。 |
| `refactoring_plan.md` | 07-18 | **要確認**。未着手項目が残るなら本書へ集約して削除、全消化なら削除。 |

- 実行タスク: 上表の「削除候補」を削除し、生きている未消化項目は本書へ寄せる。
  `improvement_*.md` 3 本は本書に統合済みとして削除してよい（マイケルの最終判断を仰ぐ）。
  結果として「診断/改善は本書 1 本、仕様は architecture/database/protocols/ui_spec」に集約する。

### コード内コメント

- 経緯コメント（「以前は〜」「〜だったが」）の機械検索では顕著な該当は無く、良好。
- ただし `#NNN`（issue 番号）参照コメントが `users.rs` 37・`lib.rs` 25 等に多数。issue 番号は
  **なぜそのコードがあるか**を説明しない（GitHub を引かないと分からない）。実装意図が自明でない箇所は
  issue 番号ではなく理由を 1 行で書く方針を `coding_rules.md` に追記。既存の全置換は不要、
  触った箇所から。

---

## 5. 実装順の提案

1. **PERF-1**（計測基盤）— 他の効果測定の前提。最初に。
2. **SEC-1**（createSession レート制限）— 明確な裏口、1 ハンドラで塞げる。
3. **SEC-2**（CORS 限定）／**SEC-3**（proxy SSRF 確認）／**SEC-4**（コンテナ非 root）
4. **PERF-4**（カウント非正規化）／**PERF-5**（car_bytes 掃除）
5. **PERF-2**（actors 保存方針）— 実測を取り直してから。
6. **REF-2 / REF-3 / REF-4**（型・テスト、触った箇所から漸進）
7. **ドキュメント整理**（§4）— どのタイミングでも可、コード変更と別コミットで。
8. **PERF-3**（整合性緩和）と **REF-1**（巨大ファイル分割）は §7 の注意付き。

---

## 6. 実装前に必ず確認すること

- DB 破壊的操作（インデックス DROP・大量掃除・`ALTER TABLE`）は、この開発 DB が事実上の本番相当
  である前提で慎重に。件数・統計は着手直前に取り直す（本書の数値は 08-05 実測の再掲を含む）。
- マイグレーションは `cargo sqlx migrate run`。`sqlx::query!` 追加時は `cargo sqlx prepare --workspace`。
- E2E（`cd e2e && npm test`）で回帰確認。

## 7. モデル選択の注記（Sonnet 実装の前提で）

- **§1〜§5 の大半は Sonnet で問題ない。** 定型的なガード追加・インデックス・掃除ジョブ・
  型付け・テスト追加はいずれも局所的。
- **PERF-3（整合性緩和）**: 実装コード自体は軽い（`SET LOCAL` 1 行、UNLOGGED 化）が、
  **どのデータの耐久性をどこまで捨てるかはプロダクト判断**。Sonnet に実装させる前にマイケルが
  緩める範囲を確定させること。ここは実装力ではなく判断の問題。
- **REF-1（巨大ファイル分割）**: 3,788 行 / 2,478 行を how/what で割る作業は、
  分割中に配送・可視性・冪等性などの不変条件を壊さないための**広い文脈保持**が要る。
  過去、並列サブエージェントでの大規模リファクタが古いベースで作業しコンフリクトした前例もある。
  この 1 項目に限っては **Fable/Opus で行うか、Sonnet なら 1 ファイルずつ・E2E を都度回す**運用を推奨。
  他項目と混ぜて一括委任しない方が安全。

# コード改善大会（2026-09-26）

対象: `main` の `dde26bc1`。観点はテスト・リファクタリング・セキュリティ・パフォーマンスに加え、
(1) DRY違反（特に Misskey 互換 API とカスタム API の重複）、(2) レースコンディション、
(3) NULL を考慮しない SQL（`IN (SELECT` / `NOT IN (SELECT`）、(4) 関数の責務過多
（`too_many_arguments` 抑制・長大関数）。詳細な調査レポートは会話中のドキュメントにあり、
本書は実施内容と残課題の記録である。

## 1. 実施した修正

### 不具合・脆弱性

| # | 内容 | 対応 |
|---|------|------|
| S1 | 引用・リポスト・返信先の参照埋め込み（`embed_renotes`/`embed_quotes`/Misskey `fetch_referenced_notes`）が手書きの可視性条件で `direct` を `followers_only` と同じ扱いにしており、宛先外のフォロワーへ DM 本文が漏れる | `repository::find_visible_posts_by_ids`（`post_is_visible_to`）に一本化 |
| S2 | 連合用 HTTP クライアントに非公開 IP 拒否が無く、受信署名の `keyId`・フォロー対象・`ap/show` の `uri` 等で内部ネットワークへ接続できる（SSRF） | `net::federation_client_builder`（公開IP限定リゾルバ＋リダイレクト検査）と `net::ensure_public_url`（IPリテラル検査） |
| S3 | `net.rs` が chunked 応答を上限なく全量読み込む | `read_body_limited` でストリーム読み込み中に打ち切る |
| S4 | IPv6 に埋め込まれた非公開 IPv4（NAT64・6to4・IPv4互換）を公開扱い | `embedded_ipv4` で取り出して判定。`admin/relays.rs` の劣化コピーも統合 |
| S5 | Misskey `notes/search` に検索回数制限が掛かっていない | カスタム API と同じ `check_search_rate_limit` を適用 |
| S6 | Turnstile 検証がタイムアウト無しのクライアント | 共有クライアントを使用 |
| D2 | `TimelinePost` の列リストが 21 箇所に手書きされ、`#[sqlx(default)]` が書き漏れを黙認。リスト TL・ピン留め・検索結果で CW が効かず、各 TL でリモート投稿の `ap_object_id`/`at_uri` が欠落 | `timeline_post_columns!()`/`timeline_post_joins!()` に集約し、`#[sqlx(default)]` を撤去 |
| D3 | Misskey `notes/search` がブリッジポスト解決を漏らしていた | `search::search_post_ids_by_cursor` をカスタム API と共有 |
| R1 | 投票が「JSON を読んで +1 して書き戻す」形で、同時投票で票が消える（ローカル・リモート受信の両方） | `repository::poll::increment_poll_votes`（`jsonb_set` の単一 UPDATE）、ローカル投票は `FOR UPDATE` のトランザクション |
| R2 | 承認制アカウントの再フォロー操作で accepted が pending に降格 | `upsert_pending` を状態を変えない3値返却に |
| R3 | リアクション切替・取消で旧値の読み出しと上書き・削除が別文 | `ReactionRepository::upsert`（トランザクション）・`delete_local`（`DELETE ... RETURNING`） |
| R4 | 登録時の users/actors 挿入が非原子的（同名同時登録で actor の無い users 行が残る） | `repository::create_local_account` |
| R5 | 検索回数・コンタクト数制限が「数えてから記録」 | `pg_advisory_xact_lock` で直列化 |
| R6 | フォロー連打で ATP follow レコードが二重コミット | `follow_exec::establish_atp_follow`（行を先に確保してからコミット） |
| R7 | ピン留めの追加と上限超過分削除が別文 | 1トランザクション＋アクター行ロック |
| — | リモート Fedi アクターのプロフィール組み立てが8箇所に重複し、`preferredUsername` 欠落時の扱い・バナー保存有無が経路ごとに食い違う | `FediActorProfile::from_ap_actor` に一本化 |
| — | `commit_quote` が `commit_post` の完全なコピー | 統合（`PostCommit`） |

### DRY・構造

- Misskey 互換 API（`handlers/misskey/endpoints.rs`）: 認証の手書き（`extract_auth`→`find_local_by_user_id`）を `AuthedUser`/`MaybeAuthedUser` 抽出子へ、カーソル解析を `CursorParams`→`repository::Page` へ、`users/following`・`followers` を `follow_relations` へ、`notes/reactions/delete` をカスタム API の `remove_reaction` へ統合。
- Misskey 変換（`convert.rs`）: `build_notes`/`fetch_referenced_notes` の重複を `convert_rows`＋`NoteMaterials` に統合。通知一覧のノート1件ずつの取得・変換（N+1、limit=100 で最大約1,000クエリ）を一括化。リスト一覧のメンバー取得を一括化。
- 引数の束の構造体化: `NewNotification`（10引数・17箇所）、`NewReaction`、`NewLocalActor`、`FediActorProfile`/`BskyActorProfile`、`PostCommit`、`Page`、`ActorSummary`、`NoteMaterials`。`#[allow(clippy::too_many_arguments)]` は 37 → 22 箇所。
- `IN (SELECT` を全廃し `EXISTS` に書き換え。`crates/seiran-common/tests/sql_style.rs` で機械的に検出する。
- ルール: `docs/coding_rules.md` 2節 #15〜#19、`CLAUDE.md`「コーディングルール（抜粋）」。

## 2. 第2弾で実施した修正

| # | 内容 |
|---|------|
| A | 署名付き／無署名の Actor 取得の重複（13箇所）を `ApClient::fetch_actor_with_key` に集約 |
| B | AP 配送関数11個の共通5引数を `ap::deliver::ApSender` に集約 |
| C | Bsky アクター発見手順の重複（3箇所）を `seiran_actor_merge::discover_bsky_profile` に集約（発見時にバナーも保存） |
| D | カスタム API のフォロー／アンフォローが `actorId` を受け付けるようにし、Misskey `following/create`・`delete` はそのまま委譲（Misskey 側の逆算処理を削除） |
| E | アカウント作成数制限を「冒頭で枠を予約し、失敗時に取り消す」形に変更（成功した登録だけを数える仕様は維持）。登録処理を手順ごとの関数に分割し、初期セットアップで ATP 初期レコード（chat declaration・相互申告用の自己申告・`#identity`）が漏れていた不具合を共通化で解消 |
| F | `#[allow(clippy::too_many_arguments)]` を全廃（0件）。`LikeCommit`・`ProfileCommit`・`CommitFrame`・`LocalProfileUpdate`・`RemoteAttachment`・`NewMigrationRequest`・`IncomingBskyPost`・`BskyPostRow`・`InboundSubjectRecord`・`LocalChatAccount`。残っていた抑制のうち7箇所は既に7引数以下で不要だった |
| G | `router`（861行）を領域別サブルーター（`routes.rs`）へ分割。frontend API のノート組み立て手順（約11箇所に手書き）を `notes::queries::build_note_responses` に統合し、ハッシュタグ・リスト TL の投票済み状態、検索結果のリアクション・引用/リポスト埋め込み・投票状態、プロフィールのリポスト済み状態、単体取得・スレッドの関係フラグの欠落を解消。`build_profile_response_inner`・`deliver_regular_post` を手順ごとの関数に分割 |
| H | DB 結合テストを CI（E2E ジョブ）で実行。ハーネスがマイグレーション適用とテストユーザー作成を自動で行う。開発DBの `storage_providers(id=1)` に依存していたテストを修正。同時投票の E2E（`poll-concurrency.spec.ts`）を追加 |

## 3. テスト整備（第2弾の後）

| 種別 | 追加したテスト | 固定する内容 |
|------|----------------|--------------|
| 結合（`repository_integration.rs`） | `every_timeline_post_query_decodes_rows` | `TimelinePost`を返す全クエリ（TL4種・プロフィール・メンション・スレッド・単体取得・ピン留め・ハッシュタグ・リスト・DM）が実際に行を返しデコードできる（列の書き漏れの検出） |
| 結合 | リアクション切替・取消 | 切替は旧値を返し、取消は削除した行を返す。同時切替で各呼び出しが報告する旧値が重複しない |
| 結合 | 投票・作成枠・ピン留め・同名登録・フォロー | 同時投票で加算が失われない／作成枠は上限までしか予約できない／ピン留めは上限を超えない／同名同時登録で孤立した users 行が残らない／同時の再フォロー操作で accepted が降格しない |
| 単体 | `net.rs` | 連合用リゾルバが内部アドレスに解決されるホストを拒否し、連合用クライアントが localhost へ接続しない |
| 単体 | Misskey `CursorParams` | `limit`の丸め・不正IDの無視・camelCase ボディの解釈 |
| E2E | `hashtag.spec.ts`・`search.spec.ts` | ハッシュタグTLの投票済み状態、検索結果のリアクション・引用元埋め込み |

## 4. 第3弾（残課題の解消）

| # | 内容 |
|---|------|
| I | DM のノート組み立て（`handlers::dm::sessions`・`thread_messages`）を `build_note_responses` に統合し、Bsky 側リアクションの合算と宛先一覧だけを上乗せする形にした。DM 内の引用カードが表示されない不具合（`embed_quotes` 未呼び出し）を解消。E2E `dm.spec.ts` に引用埋め込みのテストを追加 |
| J | frontend のフォロー API 呼び出しを、actor 行を持つ画面（対ユーザー操作メニュー・フォローボタンのホバー切替）から `actorId` 指定に切り替え（`followTargetOf`）。あわせてカスタム API の `actorId` 解決が `at_did` を `ap_uri` より優先しており、Misskey クライアントからリモート seiran アクターをフォローすると ATP のみで成立していた不具合を修正（プロフィール画面と同じ `ap_uri` 優先に統一） |
| K | 長大関数を手順ごとの関数に分割: `firehose::save_bsky_post`・`process_message`、`note_save::save_ap_note_core`、`creation::validate_create_regular_post_input`、`bsky_dm_poll::sync_convo`、`outbox_handler`、`migration::submit_plc_token`、`at_migration::process_import`、`seiran-server` の `main`。分割は既存の処理ブロックをそのまま移す形で行い、挙動は変えていない（下記の不具合修正を除く） |

分割の過程で見つけた不具合:

- **outbox の可視性漏れ**: `GET /users/:username/outbox` が可視性を見ずに全投稿を `to: Public` の Create として並べており、フォロワー限定投稿・DM の本文を誰でも取得できた。featured と同じ条件で除外し、添付・Note 組み立てを `handlers::ap_collection` に共通化。E2E `dm-privacy.spec.ts` に固定テストを追加。
- **DM 宛先に存在しない actorId を指定すると 500**: `resolve_dm_recipients` で事前に検証し 400 `INVALID_RECIPIENT_ACTOR_ID` を返す。
- **Node 25 以降で frontend 単体テストが失敗**: Node 組み込みの `localStorage` が jsdom のものを覆い隠すため、vitest の setupFile で jsdom 側を割り当て直す（CI の Node 20 では発生しない）。

## 5. 残課題

- 長大関数の分割は、受信・転入経路の網羅的な E2E が無いまま「処理ブロックを移すだけ」に留めた。個々の手順関数（`resolve_dm_addressing`・`resolve_quote_and_strip_fallback` 等）の単体テストは今後の課題。

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

## 4. 残課題

1. **長大関数**（行数）: `firehose::save_bsky_post`（約480）・`process_message`（約350）、`inbound_activity_process::note_save::save_ap_note_core`（約470）、`notes::creation::validate_create_regular_post_input`（約320）、`bsky_dm_poll::sync_convo`（約310）、`federation-inbox` の `outbox_handler`（約300）、`migration::submit_plc_token`（約300）、`jobs::at_migration::process_import`（約300）、`seiran-server` の `main`（約280）。いずれも受信・転入の中核処理で、分割にはそれぞれの経路の E2E／結合テストの拡充が先に必要。
2. **DM のノート組み立て**: `handlers::dm` は Bsky 側リアクション（`dm_bsky_reactions`）の合算など固有処理があるため `build_note_responses` に統合していない。引用の埋め込み（`embed_quotes`）を呼んでいないため、DM 内の引用カードの表示経路を確認する必要がある。
3. **フロントエンドのフォロー API 呼び出し**: カスタム API が `actorId` を受け付けるようになったが、frontend は従来どおり `target` 文字列で呼んでいる。actor 行を持っている画面から `actorId` 指定に切り替えると、文字列の組み立て・解析の分岐が不要になる（フォロー成立時の相手プロフィール取得自体は最新化のため残る）。

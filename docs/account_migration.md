# 既存Bluesky DID転入フロー

対象読者: seiran のコード全体に手を入れる開発者。「今のシステムがどう動いているか」だけを書く。

新規登録時に必ず新しいDIDを発行する通常のアカウント作成とは別に、既存のBluesky/AT Protocolアカウント（bsky.social等でホストされている）を、そのDID・投稿・フォロー関係・blobごとseiranへ「転入」させる登録経路（1〜5節）、およびその逆方向——seiranから他PDSへ既存DIDを転出させる際、seiranが転出元として応答するサーバー側API（6節）。技術的な実体はPDS間移行。実装本体は`crates/seiran-api/src/handlers/migration.rs`・`crates/seiran-common/src/jobs/at_migration.rs`・`crates/seiran-common/src/atp/{car,mst_walk,migration_client,plc}.rs`・`crates/seiran-api/src/handlers/xrpc/identity.rs`、フロントエンドは`frontend/src/pages/auth/MigratePanel.tsx`（ログインカルーセルの一部、`frontend/src/pages/auth/AuthCarouselPage.tsx`）と`frontend/src/pages/MigrationImportingPage.tsx`。関連ドキュメント: `docs/architecture.md`（ジョブ・匿名段階認可）、`docs/database.md`（テーブル定義）、`docs/protocols.md` 3節（PDS Aとの通信・SSRF対策）。

## 1. 認証方式（ID/PW、OAuth不採用）

PDS Aへの認証は`com.atproto.server.createSession`（ID/PW直叩き）を使う。OAuth（AT Protocolのclient-id-as-URL方式）は採用していない。理由: bsky.socialのOAuth entrywayが公開するスコープ（`atproto`/`transition:email`/`transition:generic`/`transition:chat.bsky`）には、PLCオペレーション署名（`account`スコープ相当）やアカウント無効化に必要なスコープが含まれない。これは実機検証済みの制約であり、Bluesky公式クライアント自身もアカウント移行機能をID/PW認証で実装している。

メールアドレスはPDS Aの`createSession`応答（`email`/`emailConfirmed`）からそのまま取得して使う。PDS Aへのパスワード認証成功が既にアカウント所有の強い証跡であるため、seiran独自のメール実在確認（`require_email_verification`）は転入フローでは挟まない——通常registerのメール確認とは別チャネル。PDS Aのメール2FA（`authFactorToken`、ハンドル・パスワード入力直後の`createSession`に対するもの）とはさらに別物。

PDS Aがメールを返さない場合（実機で判明: Bluesky公式アプリ経由で発行したapp password認証では`createSession`応答に`email`/`emailConfirmed`が含まれない）は、`SOURCE_EMAIL_REQUIRED`エラーを返しフロントにメール入力欄を追加表示させて再試行させる（`AUTH_FACTOR_TOKEN_REQUIRED`と同じ「エラーで欄を追加して再送」パターン）。この場合のメールは検証なしでそのまま使う。

## 2. ユーザーフロー

`/register/migrate`（ログインカルーセルの「Blueskyから転入」パネル、`MigratePanel`）で移行元ハンドル・パスワードを入力すると開始する。以降は同じパネル内で「いま何を待っているか」の表示切り替えにより進行する（別画面への遷移はしない）。各ステップは入力欄0〜1個＋リトライボタンで構成される。

1. `/register/migrate`でハンドル・パスワードを入力し送信
2. （PDS Aがメール2FAを要求する場合のみ）確認コード入力
3. PDS Aのリポジトリ取得（自動、待機表示のみ）
4. PLCオペレーション署名要求→PDS A登録メール宛の確認コード入力
5. `submitPlcOperation`実行（★不可逆境界、後述）。成功と同時にJWTが発行されログイン状態になる
6. データ取り込み中（`MigrationImportingPage`、`is_suspended`と同型で他画面をバイパス。`submit-plc-token`成功時にlocalStorageのmigration_token/idをクリアせず残しておき、`/api/migration/:id/status`のポーリングを引き続き行うことで、`importing_data`中の取り込み件数/全体件数（`import_done`/`import_total`）を表示する。完了検知時にクリアする）
7. 完了、通常のSNS画面へ

## 3. 状態遷移（`at_migration_status`）

| status | 意味 | 実行主体 |
|---|---|---|
| `awaiting_source_2fa` | PDS Aがメール2FAを要求、`authFactorToken`待ち | ユーザー入力待ち |
| `fetching_repo` | `getRepo`(CAR)+`listBlobs`取得中 | `Job::MigrationFetchRepo` |
| `requesting_plc_signature` | `requestPlcOperationSignature`呼び出し中 | `Job::MigrationRequestPlcSignature` |
| `awaiting_plc_token` | PDS Aメールの確認コード入力待ち | ユーザー入力待ち |
| `submitting_plc` | `signPlcOperation`→`submitPlcOperation`実行中。★成功時にDB確定（`users`/`actors`作成）も行う | 同期処理（APIハンドラ内） |
| `importing_data` | ステージング済みレコード/blobの実体化中 | `Job::MigrationImportProcess`（自己再enqueue） |
| `deactivating_source` | PDS A側旧アカウントの無効化中（ベストエフォート） | `Job::MigrationDeactivateSource` |
| `completed` | 転入完了 | — |
| `failed` | `submitting_plc`到達前の失敗。リトライ／別DIDで再開／新規DID立ち上げへ切替のいずれも可 | ユーザー選択待ち |
| `failed_post_submit` | `submitting_plc`成功後の失敗。リトライのみ可 | ユーザー選択待ち |
| `abandoned` | ユーザーによる打ち切り（`plc_submitted_at IS NULL`の間のみ選択可） | — |

`Job::MigrationImportFollows`（フォロー関係の`follows`テーブルへの反映）は上記の`status`に含まれない。`importing_data`の完了時（`deactivating_source`への遷移と同時）にenqueueされるが、それ自体は`at_migration_requests.status`を見ず、未反映の`app.bsky.graph.follow`レコードが尽きるまで独立して動作し続ける結果整合処理。転入の完了判定（`completed`）はこのジョブの完了を待たない。

## 4. 不可逆境界（`submitPlcOperation`）

`com.atproto.identity.submitPlcOperation`の成功が転入フロー全体で唯一の不可逆操作で、DIDのservice endpointが実際にseiranへ切り替わる。この前後で選べる選択肢を変える設計原則:

- **成功前**（`plc_submitted_at IS NULL`）: 「リトライ」「別DIDで再開」「新規DID立ち上げに切替」の3択。PDS Aが応答しない場合に備え、転入自体を諦めて通常の新規登録へ切り替えられる
- **成功後**（`plc_submitted_at`設定済み）: 「リトライ」のみ。DIDのservice endpointが既にseiranを指した状態で「別DID/新規DIDへ切替」を選ばせると、元のDIDがどのPDSにも正しく紐づかない壊れた状態のまま放置されることになるため、意図的に選択肢を絞っている

`mark_plc_submitted`（PLC提出成功直後、新しい署名鍵PEMを保存）と`confirm_account_created`（ローカルの`users`/`actors`作成成功後）を別ステップに分離しているのも同じ原則の実装: PLC提出は成功したがその後のローカルDB確定が失敗した場合でも、既に提出済みのPLCオペレーションに埋め込まれた新しい署名鍵を失わない（失うと、DIDはseiranを指しているのに対応する鍵がどこにも無い状態になり、そのDIDは永久に使用不能になる）。

## 5. データ取り込みの範囲

`app.bsky.feed.post`コレクションのみ`posts`テーブルへ構造化パースする。それ以外のコレクションは`atp_records`への生バイト列複製が基本方針（`docs/database.md`「ATP リポジトリ関連」参照、既存の投稿コミットパイプラインと共通のテーブル設計）。

- **`seiranPost`拡張オブジェクト（転入元もseiranの場合）**: 転入元PDSがseiranインスタンスであれば、取り込んだ`app.bsky.feed.post`レコードは`seiranPost`拡張オブジェクト（他seiranサーバー間の投稿完全再現、`docs/protocols.md` 5節）を持つ。検出できた場合、本文（`body`）・絵文字マップ（`emojiMap`）・CW（`contentWarning`）・投票（`poll`）・公開範囲（`visibility`）・URLカードの申告値（`linkCards`）をATP標準フィールドの変換値より優先して`posts`へ反映する。リモートseiranポストのATP受信（`seiran-atp-repo::firehose::save_bsky_post`）と同じ優先順位。

- **画像・動画添付**: `app.bsky.feed.post`の`embed`（画像・動画）は、DID＋blob CIDのみからBluesky CDN/動画パイプラインのURLを決定的に組み立てる既存ロジック（`seiran_common::atp::parse_bsky_embed_attachments`、他の受信Bsky投稿と共通）で復元する。転入元PDSから取得した実バイト列（`at_migration_blobs`→`atp_blobs`）を再ホストするのではなく、Bluesky公式のCDN/動画配信を指す点は他の受信Bsky投稿の添付表示と同じ扱い。
- **フォロー関係**: `app.bsky.graph.follow`はATPリポジトリへの複製に加え、`Job::MigrationImportFollows`がseiran自身の`follows`テーブルへも反映する（3節参照）。フォロー先ごとにAppView `getProfile`でリモートアクターを解決するため、レート制限は適用しない（新規フォローではなく既存関係の復元のため）。
- **リプライ・引用先の解決、facet解析**: スコープ外。`reply_to_post_id`/`quote_of_post_id`は設定しない。
- **DM（1:1）**: 転入固有の実装は無い。Bsky DM（`chat.bsky.convo`）はAT Protocolリポジトリに含まれない別サービス（`api.bsky.chat`）のデータで、認証は現在のDIDドキュメントの署名鍵（自己署名service-auth JWT）に基づくため、転入後のアカウントも既存の`BskyDmPoll`（`docs/protocols.md` 9節）が`actor_type='local'`かつ`at_did`/`at_signing_key_pem`設定済みの全アクターを対象に自動的に対象へ含める。会話ごとの初回同期はcursorページングで遡って取り込む（同節参照）。
- **グループチャット**: 非対応。seiran自体にグループチャット機能が無い。

## 6. 転出元API対応（seiranが転出元として振る舞う経路）

上記1〜5は常にseiranが転入先（destination）として動く経路。ここではその逆方向——他PDS（別のseiranインスタンス、bsky.social等）がseiranから既存DIDを引き出す際、seiranが転出元（source）として応答するサーバー側APIを扱う。実装本体は`crates/seiran-api/src/handlers/xrpc/identity.rs`（`com.atproto.identity.*`）と`crates/seiran-api/src/handlers/xrpc/server.rs`の`checkAccountStatus`/`deactivateAccount`/`createSession`。

### アカウント単位PLCローテーションキー

seiranが自前でジェネシスDIDを発行するローカルアカウントは、`actors.at_rotation_key_pem`にアカウント専用のローテーションキーを持つ。DIDの`rotationKeys`配列は`[アカウント専用鍵（主）, サーバー全体共有鍵（副＝recovery用）]`の2本構成（`crates/seiran-common/src/atp/plc.rs::prepare_plc_genesis`）。共有鍵（`secrets.toml`の`atproto_private_key_pem`）は全ローカルアカウント共通のrecovery用としてのみ残り、通常の署名操作（`signPlcOperation`等）ではアカウント専用鍵のみを使う。

既存DID転入で作成されたアカウントは、転入完了時（`submit_plc_token`）にseiranが新規発行する専用ローテーションキーのみをDIDの鍵とする（転入元PDSの既存ローテーションキーは引き継がない。転入元PDS運営者に恒久的な支配権を残さないため）。

### `com.atproto.identity.*`/`com.atproto.server.*`（転出元エンドポイント）

| メソッド | 認可 | 用途 |
|---|---|---|
| `com.atproto.server.checkAccountStatus` | ATP accessJwt | 読み取りのみ。`activated`/`repoCommit`/`indexedRecords`等を返す |
| `com.atproto.identity.getRecommendedDidCredentials` | ATP accessJwt | 読み取りのみ。現在の`rotationKeys`/`alsoKnownAs`/`verificationMethods`/`services`を返す |
| `com.atproto.identity.requestPlcOperationSignature` | ATP accessJwt | 登録メールへ6桁確認コードを送信（`email_short_codes`、`purpose='plc_operation_signature'`） |
| `com.atproto.identity.signPlcOperation` | ATP accessJwt + 確認コード | 要求された内容のPLC更新オペレーションをアカウント専用ローテーションキーで署名して返す（提出はしない） |
| `com.atproto.identity.submitPlcOperation` | ATP accessJwt | ★不可逆境界。plc.directoryへ提出し、`#identity`/`#account`イベント発火・`did_moved_out_at`設定 |
| `com.atproto.server.deactivateAccount` | ATP accessJwt | `submitPlcOperation`の有無にかかわらず`did_moved_out_at`を設定 |

`com.atproto.server.createSession`のメール2FA（`authFactorToken`）も同じ`email_short_codes`機構（`purpose='atp_session_2fa'`）を使う。SMTP未設定インスタンスでは2FA自体を常にスキップする。`requestPlcOperationSignature`/`signPlcOperation`も同じ原則: SMTP未設定なら`requestPlcOperationSignature`はコードを発行・送信せず空応答のみ返し、`signPlcOperation`も検証をスキップする。SMTP設定はあるが実際のメール送信自体が失敗した場合（実機で発見: 別インスタンスへの転入検証中に`smtp_host`未設定のまま気づかず遭遇）も、発行済みコードを`revoke`して同じ「未発行」扱いに帰着させる——「SMTP設定の有無」ではなく「有効なコードが実在するか」（`has_pending`）で検証要否を判定する。

### DID転出済み状態（`did_moved_out_at`）

`is_suspended`（凍結）や本ドキュメント3節の`migration_status`ゲートとは異なる第三の状態。`submitPlcOperation`成功時、または`deactivateAccount`呼び出し時に`actors.did_moved_out_at`が設定される。この状態では:

- タイムライン等の**読み取りは通常通り**表示する（専用画面へのバイパスはしない）
- 投稿・リアクション・リポスト・フォロー・リスト操作・DM送信等の**書き込みは全て拒否**（`AuthedUser::require_not_did_moved_out()`、`crates/seiran-api/src/middleware/authed_user.rs`。フロントエンドは`user.did_moved_out`で主要な書き込みUIを無効化、`frontend/src/components/note/PostComposer.tsx`・`frontend/src/hooks/useNoteCardActions.ts`）
- ActivityPub側の転出（AP自体の引っ越し）は未実装

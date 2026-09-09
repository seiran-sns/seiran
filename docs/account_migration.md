# 既存Bluesky DID転入フロー

対象読者: seiran のコード全体に手を入れる開発者。「今のシステムがどう動いているか」だけを書く。

新規登録時に必ず新しいDIDを発行する通常のアカウント作成とは別に、既存のBluesky/AT Protocolアカウント（bsky.social等でホストされている）を、そのDID・投稿・フォロー関係・blobごとseiranへ「転入」させる登録経路。技術的な実体はPDS間移行（PDS A＝転入元、seiran＝転入先）。実装本体は`crates/seiran-api/src/handlers/migration.rs`・`crates/seiran-common/src/jobs/at_migration.rs`・`crates/seiran-common/src/atp/{car,mst_walk,migration_client}.rs`、フロントエンドは`frontend/src/pages/{MigrateRegister,MigrationStatusPage,MigrationImportingPage}.tsx`。関連ドキュメント: `docs/architecture.md`（ジョブ・匿名段階認可）、`docs/database.md`（テーブル定義）、`docs/protocols.md` 3節（PDS Aとの通信・SSRF対策）。

## 1. 認証方式（ID/PW、OAuth不採用）

PDS Aへの認証は`com.atproto.server.createSession`（ID/PW直叩き）を使う。OAuth（AT Protocolのclient-id-as-URL方式）は採用していない。理由: bsky.socialのOAuth entrywayが公開するスコープ（`atproto`/`transition:email`/`transition:generic`/`transition:chat.bsky`）には、PLCオペレーション署名（`account`スコープ相当）やアカウント無効化に必要なスコープが含まれない。これは実機検証済みの制約であり、Bluesky公式クライアント自身もアカウント移行機能をID/PW認証で実装している。

seiran自身の`require_email_verification`設定（seiranのメール確認、後述）とPDS Aのメール2FA（`authFactorToken`）は完全に別チャネル。

## 2. ユーザーフロー

`/register/migrate`で移行元ハンドル・パスワードを入力すると開始する。以降は単一の汎用状態画面（`MigrationStatusPage`）に統一されており、画面遷移ではなく「いま何を待っているか」の表示切り替えで進行する。各ステップは入力欄0〜1個＋リトライボタンで構成される。

### `require_email_verification = OFF`の場合
1. `/register/migrate`でハンドル・パスワードを入力し送信
2. （PDS Aがメール2FAを要求する場合のみ）確認コード入力
3. PDS Aのリポジトリ取得（自動、待機画面のみ）
4. PLCオペレーション署名要求→PDS A登録メール宛の確認コード入力
5. `submitPlcOperation`実行（★不可逆境界、後述）。成功と同時にJWTが発行されログイン状態になる
6. データ取り込み中（`MigrationImportingPage`、`is_suspended`と同型で他画面をバイパス）
7. 完了、通常のSNS画面へ

### `require_email_verification = ON`の場合
上記3と4の間に、seiran自身のメール確認ステップが挿入される（`email_verifications`の`registration_token`を消費する通常の登録メール確認と同じ仕組み）。

## 3. 状態遷移（`at_migration_status`）

| status | 意味 | 実行主体 |
|---|---|---|
| `awaiting_source_2fa` | PDS Aがメール2FAを要求、`authFactorToken`待ち | ユーザー入力待ち |
| `fetching_repo` | `getRepo`(CAR)+`listBlobs`取得中 | `Job::MigrationFetchRepo` |
| `awaiting_seiran_email` | `require_email_verification=ON`時のみ、seiranのメール確認待ち | ユーザー入力待ち |
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

- **画像・動画添付**: `app.bsky.feed.post`の`embed`（画像・動画）は、DID＋blob CIDのみからBluesky CDN/動画パイプラインのURLを決定的に組み立てる既存ロジック（`seiran_common::atp::parse_bsky_embed_attachments`、他の受信Bsky投稿と共通）で復元する。転入元PDSから取得した実バイト列（`at_migration_blobs`→`atp_blobs`）を再ホストするのではなく、Bluesky公式のCDN/動画配信を指す点は他の受信Bsky投稿の添付表示と同じ扱い。
- **フォロー関係**: `app.bsky.graph.follow`はATPリポジトリへの複製に加え、`Job::MigrationImportFollows`がseiran自身の`follows`テーブルへも反映する（3節参照）。フォロー先ごとにAppView `getProfile`でリモートアクターを解決するため、レート制限は適用しない（新規フォローではなく既存関係の復元のため）。
- **リプライ・引用先の解決、facet解析**: スコープ外。`reply_to_post_id`/`quote_of_post_id`は設定しない。
- **DM（1:1）**: 転入固有の実装は無い。Bsky DM（`chat.bsky.convo`）はAT Protocolリポジトリに含まれない別サービス（`api.bsky.chat`）のデータで、認証は現在のDIDドキュメントの署名鍵（自己署名service-auth JWT）に基づくため、転入後のアカウントも既存の`BskyDmPoll`（`docs/protocols.md` 9節）が`actor_type='local'`かつ`at_did`/`at_signing_key_pem`設定済みの全アクターを対象に自動的に対象へ含める。会話ごとの初回同期はcursorページングで遡って取り込む（同節参照）。
- **グループチャット**: 非対応。seiran自体にグループチャット機能が無い。

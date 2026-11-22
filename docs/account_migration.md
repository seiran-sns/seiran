# 既存Bluesky DID転入フロー

対象読者: seiran のコード全体に手を入れる開発者。現在の動作だけを書く。

新規登録時に必ず新しいDIDを発行する通常のアカウント作成とは別に、既存のBluesky/AT Protocolアカウント（bsky.social等でホストされている）を、そのDID・投稿・フォロー関係・blobごとseiranへ「転入」させる登録経路（1〜5節）、およびその逆方向——seiranから他PDSへ既存DIDを転出させる際、seiranが転出元として応答するサーバー側API（6節）。技術的な実体はPDS間移行。実装本体は`crates/seiran-api/src/handlers/migration.rs`・`crates/seiran-common/src/jobs/at_migration.rs`・`crates/seiran-common/src/atp/{car,mst_walk,migration_client,plc}.rs`・`crates/seiran-api/src/handlers/xrpc/identity.rs`、フロントエンドは`frontend/src/pages/auth/MigratePanel.tsx`（ログインカルーセルの一部、`frontend/src/pages/auth/AuthCarouselPage.tsx`）と`frontend/src/pages/MigrationImportingPage.tsx`。関連ドキュメント: `docs/architecture.md`（ジョブ・匿名段階認可）、`docs/database.md`（テーブル定義）、`docs/protocols.md` 3節（PDS Aとの通信・SSRF対策）。

## 1. 認証方式（ID/PW、OAuth不採用）

PDS Aへの認証は`com.atproto.server.createSession`（ID/PW直叩き）を使う。OAuth（AT Protocolのclient-id-as-URL方式）は採用していない。理由: bsky.socialのOAuth entrywayが公開するスコープ（`atproto`/`transition:email`/`transition:generic`/`transition:chat.bsky`）には、PLCオペレーション署名（`account`スコープ相当）やアカウント無効化に必要なスコープが含まれない。Bluesky公式クライアント自身もアカウント移行をID/PW認証で実装している。

メールアドレスはPDS Aの`createSession`応答（`email`/`emailConfirmed`）からそのまま取得して使う。PDS Aへのパスワード認証成功が既にアカウント所有の強い証跡であるため、seiran独自のメール実在確認（`require_email_verification`）は転入フローでは挟まない——通常registerのメール確認とは別チャネル。PDS Aのメール2FA（`authFactorToken`、ハンドル・パスワード入力直後の`createSession`に対するもの）とはさらに別物。

**アプリパスワードは受け付けない**: アプリパスワードでの`createSession`が返すトークンは権限が`com.atproto.appPass`（または`com.atproto.appPassPrivileged`）に絞られ、PLC 操作の署名要求（`requestPlcOperationSignature`）やアカウントの無効化が`InvalidToken - Bad token scope`で拒まれる。`start`は accessJwt の`scope`クレームを読み（署名は検証しない。権限の判定は PDS A がするので、案内のために読むだけ）、アプリパスワードなら`SOURCE_APP_PASSWORD`を返す（`migration_client::is_app_password_token`）。入力欄の下にも「アプリパスワードではなく Bluesky にログインするときのパスワード」と案内を出す。アプリパスワードで始まってしまった転入（この判定が入る前のもの、または`scope`クレームを持たない PDS で権限不足になったもの）は、`requesting_plc_signature`のジョブがトークンの`scope`か`Bad token scope`の応答で気づいて`awaiting_source_password`にし、転入画面で本来のパスワードを入れ直してもらって続きから進める（3節）。

PDS Aがメールを返さない場合（app password での`createSession`応答には`email`/`emailConfirmed`が含まれない）は、`SOURCE_EMAIL_REQUIRED`エラーを返しフロントにメール入力欄を追加表示させて再試行させる（`AUTH_FACTOR_TOKEN_REQUIRED`と同じ「エラーで欄を追加して再送」パターン）。この場合のメールは検証なしでそのまま使う。

## 2. ユーザーフロー

`/register/migrate`（ログインカルーセルの「Blueskyから転入」パネル、`MigratePanel`）で移行元ハンドル・パスワードを入力すると開始する。以降は同じパネル内で「いま何を待っているか」の表示切り替えにより進行する（別画面への遷移はしない）。各ステップは入力欄0〜1個＋リトライボタンで構成される。

1. `/register/migrate`でハンドル・パスワードを入力し送信
2. （PDS Aがメール2FAを要求する場合のみ）確認コード入力
3. PDS Aのリポジトリ取得（自動、待機表示のみ）
4. PLCオペレーション署名要求→PDS A登録メール宛の確認コード入力
5. `signPlcOperation`で署名済みの PLC 操作を得て保存し、`_atproto` TXT の公開を確かめてから`submitPlcOperation`実行（★不可逆境界、後述）。その場で確定できれば応答で JWT が発行されログイン状態になる。DNS TXT の公開待ちが残った場合（Cloudflare の障害中など）は「公開待ち」を表示して`/status`をポーリングし、アカウントができたら`POST /api/migration/:id/session`（`X-Migration-Token`で認可）で JWT を受け取ってログインする
6. データ取り込み中（`MigrationImportingPage`、`is_suspended`と同型で他画面をバイパス。`submit-plc-token`成功時にlocalStorageのmigration_token/idをクリアせず残しておき、`/api/migration/:id/status`のポーリングを引き続き行うことで、`importing_data`中の取り込み件数/全体件数（`import_done`/`import_total`）を表示する。完了検知時にクリアする）
7. 完了、通常のSNS画面へ

途中で画面を離れても、次のどちらかで続きに戻れる（4節「途中の転入への復帰」）。

- ログイン画面で、手順1で決めた希望のユーザー名（またはメールアドレス）と seiran 用のパスワードを入力する
- 入力待ちに入ったときと打ち切りの警告で届く案内メールの再開リンク（`/register/migrate?resume=...`）を開く

入力待ちのまま48時間たつと転入は打ち切られ、確保していたユーザー名とメールアドレスは空く（4節「入力待ちの期限と案内メール」）。

「通常の新規登録に切り替える」は、転入を打ち切ってから登録画面へ移る（打ち切らずに離れると、同じユーザー名で登録できないまま期限まで待つことになる）。

## 3. 状態遷移（`at_migration_status`）

| status | 意味 | 実行主体 |
|---|---|---|
| `awaiting_source_2fa` | PDS Aがメール2FAを要求、`authFactorToken`待ち | ユーザー入力待ち |
| `fetching_repo` | `getRepo`(CAR)+`listBlobs`取得中 | `Job::MigrationFetchRepo` |
| `requesting_plc_signature` | `requestPlcOperationSignature`呼び出し中 | `Job::MigrationRequestPlcSignature` |
| `awaiting_source_password` | 移行元にアプリパスワードでログインしていて PLC 操作に署名させられない。本来のパスワードの入れ直し待ち。入力されたら`POST /api/migration/:id/submit-source-password`が DID を識別子に`createSession`をやり直し（メール2FAを求められたら`start`と同じく`AUTH_FACTOR_TOKEN_REQUIRED`で確認コードを足して再送）、新しいセッションを保存して`requesting_plc_signature`へ戻す | ユーザー入力待ち |
| `awaiting_plc_token` | PDS Aメールの確認コード入力待ち。コードは PDS A 側で15分で切れるので、転入画面の「確認コードを送り直す」（`POST /api/migration/:id/resend-plc-token`）で`requesting_plc_signature`へ戻して送り直せる | ユーザー入力待ち |
| `awaiting_dns_txt` | `signPlcOperation`済み（署名済み操作と新しい鍵を`staged_*`に保存）。`_atproto` TXT が全権威 NS に公開されるのを待つ | `seiran_common::did_provision::migration`（`submit-plc-token`内の1段・直後のキック・firehose ロールの定時処理・`/status`のポーリング） |
| `submitting_plc` | `submitPlcOperation`実行中。★成功時にDB確定（`users`/`actors`作成）も行う | `did_provision::migration`（再入時はAPIハンドラ） |
| `importing_data` | ステージング済みレコード/blobの実体化中 | `Job::MigrationImportProcess`（自己再enqueue） |
| `deactivating_source` | PDS A側旧アカウントの無効化中（ベストエフォート） | `Job::MigrationDeactivateSource` |
| `completed` | 転入完了 | — |
| `failed` | `submitting_plc`到達前の失敗。リトライ／別DIDで再開／新規DID立ち上げへ切替のいずれも可 | ユーザー選択待ち |
| `failed_post_submit` | `submitting_plc`成功後の失敗。リトライのみ可 | ユーザー選択待ち |
| `abandoned` | 打ち切り。利用者が選ぶ（`plc_submitted_at IS NULL`かつ`submitting_plc`でない間のみ選択可。PLC 送信開始と競合しないよう、状態の条件付き遷移で行う）か、入力待ちのまま期限が来た（`expired_at`が入る）。どちらもユーザー名・メールの確保と再開リンクを同じ文で解く | — |

`awaiting_dns_txt` を置くのは、PLC 送信で DID のハンドルが seiran 側に切り替わった時点で Relay/AppView が解決しにくるため（新規アカウントと同じ理由、`docs/protocols.md` 3節）。署名済みの PLC 操作は、間に他の操作が入らない限り後から送っても有効なので、メールのワンタイムトークンの期限（15分）とは無関係に待てる。plc.directory が操作を拒んだ（4xx。転入元で別の操作が入った等）場合は `failed`、通信の失敗なら `awaiting_dns_txt` に戻して送り直す。Cloudflare が未設定なら待たずに送る（`submit-plc-token` の中で確定まで終わる）。

`Job::MigrationImportFollows`（フォロー関係の`follows`テーブルへの反映）は上記の`status`に含まれない。`importing_data`の完了時（`deactivating_source`への遷移と同時）にenqueueされるが、それ自体は`at_migration_requests.status`を見ず、未反映の`app.bsky.graph.follow`レコードが尽きるまで独立して動作し続ける結果整合処理。転入の完了判定（`completed`）はこのジョブの完了を待たない。

## 4. 不可逆境界（`submitPlcOperation`）

`com.atproto.identity.submitPlcOperation`の成功が転入フロー全体で唯一の不可逆操作で、DIDのservice endpointが実際にseiranへ切り替わる。この前後で選べる選択肢を変える設計原則:

- **成功前**（`plc_submitted_at IS NULL`）: 「リトライ」「別DIDで再開」「新規DID立ち上げに切替」の3択。PDS Aが応答しない場合に備え、転入自体を諦めて通常の新規登録へ切り替えられる
- **成功後**（`plc_submitted_at`設定済み）: 「リトライ」のみ。DIDのservice endpointが既にseiranを指した状態で「別DID/新規DIDへ切替」を選ばせると、元のDIDがどのPDSにも正しく紐づかない壊れた状態のまま放置されることになるため、意図的に選択肢を絞っている

`mark_plc_submitted`（PLC提出成功直後、新しい署名鍵PEMを保存）と`materialize_account`（ローカルの`users`/`actors`作成）を別ステップに分離しているのも同じ原則の実装: PLC提出は成功したがその後のローカルDB確定が失敗した場合でも、既に提出済みのPLCオペレーションに埋め込まれた新しい署名鍵を失わない（失うと、DIDはseiranを指しているのに対応する鍵がどこにも無い状態になり、そのDIDは永久に使用不能になる）。`materialize_account`は`users`/`actors`の作成・名簿の付け替え・`importing_data`への遷移を1トランザクションで行うので、失敗しても途中の行は残らず、`submit-plc-token`の再入でそのままやり直せる。

### 名前とメールの確保

転入は`start`から PLC 提出後のアカウント作成まで長くかかる（リポジトリの取得、利用者が確認コードを入れるまでの時間）。その間に同じユーザー名やメールアドレスで通常登録されると、PLC 提出後にアカウントを作れず、DID だけが seiran を指したまま止まる。これを防ぐため、`start`の時点でユーザー名とメールアドレスを名簿（`local_username_claims`/`local_email_claims`、`docs/database.md`）に確保し、アカウントができるか打ち切られるまで持ち続ける。

- 通常登録（`create_local_account`）も同じ名簿へ同じトランザクションで INSERT するので、重複の判定は名簿の主キーだけで決まる（`users`/`actors`と転入リクエストをアプリ側で突き合わせると、READ COMMITTED では同時登録をすり抜ける）。ユーザー名は小文字で持ち、大文字小文字違いも同じ名前とみなす（ATP ハンドルは小文字に揃えるため）。
- メールアドレスの変更も名簿の付け替えと同じトランザクションで行い、転入中のアドレスへは変えられない。
- 打ち切り（利用者による`abandon`・期限切れ）は、状態の遷移と同じ文で確保を解く。
- PLC 送信の直前（`submitting_plc`へ移した直後）に、この転入が名前とメールを確保できているか、名簿に載らずに作られたアカウント（名簿を知らない版のサーバーが無停止更新の間に作ったもの）と重ならないかを確かめ（`ensure_claims_before_submit`）、だめなら PLC を送らずに`failed`にする。

### 入力待ちの期限と案内メール

利用者の入力・選択を待つ状態（`awaiting_source_password`・`awaiting_plc_token`・`failed`、いずれも PLC 提出前）に入った時刻を`input_wait_since`に記録し、そこから時間を数える。ジョブが動いている状態と`awaiting_dns_txt`（利用者はやることを済ませている）は数えない。PLC 提出後の未完了の転入は、名前を手放すと DID だけが移った状態を自分から作ることになるので、期限なしで確保し続ける。

| 経過 | 処理 |
|---|---|
| 入力待ちに入った時点 | 案内メール（次にやること・再開リンク・打ち切りの時刻） |
| 24時間 | 打ち切りの警告メール（再開リンク付き） |
| 48時間 | 打ち切り（`abandoned`・`expired_at`）、確保の解放、取り消しの通知メール |

- 状態の遷移はすべて、入力待ちに入れば時刻を記録し、同じ入力待ちのままなら据え置き、抜ければ消す（`at_migration::input_wait_set`）。確認コードを送り直して`awaiting_plc_token`に入り直すと、期限も案内も新しくなる。
- 処理は API ロールの定期処理（1分ごと、`run_migration_input_wait_sweep`）が行う。打ち切り→警告→案内の順に処理するので、24時間を過ぎてから見つかった入力待ちには警告だけが届く。各段階は1文の UPDATE で「自分が担当する転入」を取るので、複数のプロセスで動いても同じメールを二重に送らない。送信に失敗しても段階は戻さない。SMTP が未設定なら送らずに段階だけ進める。
- 入力待ちなのに`input_wait_since`の無い転入（この仕組みより前から入力待ちだったもの、記録しない版のサーバーが入力待ちにしたもの）は、定期処理が見つけた時刻を起点にする。入力待ちに入った本来の時刻を使うと、案内も警告も無いまま打ち切られるため。
- メールの文面は日本語（多言語化は issue #309）。

### 途中の転入への復帰

アカウントができるまでの認可は`X-Migration-Token`だけなので（`docs/architecture.md`「DID 転入中の認可」）、トークンを保存したブラウザを失うと戻れなくなる。次の2つの経路でトークンを発行し直す。どちらも発行し直すとそれまでのトークンは無効になり、古い画面は`MIGRATION_TOKEN_INVALID`を受けて「別の場所で再開された」と表示する。

- **ログイン**（`POST /api/auth/login`）: 識別子に一致するアカウントが無ければ、名簿でその名前（またはメール）を確保している、アカウント未作成の転入を探し、`start`で決めたパスワードと照合する。一致すれば`{ migration_resume: { request_id, request_token } }`を返し、フロントは転入画面へ移る。PLC 提出後にアカウント作成の前で止まった転入も対象にする（ここで戻れないと、ブラウザを失った利用者がアカウント作成をやり直す手段が無い）。期限切れで打ち切られた転入とパスワードが一致すれば`MIGRATION_EXPIRED`。どの経路も Argon2 の照合をちょうど1回行い、パスワードが違えば同じ`INVALID_CREDENTIALS`を返すので、転入中の名前があるかどうかは漏れない。試行回数の制限は通常のログインと共通。
- **再開リンク**（`POST /api/migration/resume`）: 案内メールごとに使い捨てのトークンを作り（`at_migration_resume_tokens`、期限は打ち切りの時刻）、リンクに載せる。PLC 提出前の転入でしか使えない。宛先のメールは確認されていないことがあり、提出後にアカウント作成の前で止まった転入でリンクを有効にすると、リンクを手にした人が`submit-plc-token`の再入でアカウントを作らせ JWT まで受け取れるため。提出前なら、第三者ができるのは状態の閲覧と打ち切りくらいに留まる（確認コードや転入元のパスワードは転入元の本人しか持たない）。

## 5. データ取り込みの範囲

`app.bsky.feed.post`コレクションのみ`posts`テーブルへ構造化パースする。それ以外のコレクションは`atp_records`への生バイト列複製が基本方針（`docs/database.md`「ATP リポジトリ関連」参照、既存の投稿コミットパイプラインと共通のテーブル設計）。

- **投稿 id と表示順**: `posts.id` はレコードの`createdAt`から採番する（受信Bsky投稿と同じ。`createdAt`が無い・読めない場合は取り込み時刻）。タイムラインは id 順に並ぶので、取り込み時刻で採番すると過去の投稿が新着としてローカルタイムラインの先頭を占めるため。取り込んだ投稿はどのストリーミングチャンネルにも流さない（新規投稿ではないため）。

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
| `com.atproto.identity.requestPlcOperationSignature` | ATP accessJwt（メインパスワード） | 登録メールへ6桁確認コードを送信（`email_short_codes`、`purpose='plc_operation_signature'`） |
| `com.atproto.identity.signPlcOperation` | ATP accessJwt（メインパスワード）+ 確認コード | 要求された内容のPLC更新オペレーションをアカウント専用ローテーションキーで署名して返す（提出はしない） |
| `com.atproto.identity.submitPlcOperation` | ATP accessJwt（メインパスワード） | ★不可逆境界。plc.directoryへ提出し、`#identity`/`#account`イベント発火・`did_moved_out_at`設定 |
| `com.atproto.server.deactivateAccount` | ATP accessJwt（メインパスワード） | `submitPlcOperation`の有無にかかわらず`did_moved_out_at`を設定 |

「メインパスワード」の操作は、メインパスワードでログインしたセッション（JWT の `privileged` クレーム）に限り、アプリパスワードのセッションには 403 `APP_PASSWORD_NOT_PERMITTED` を返す。アプリパスワードはサードパーティに渡すものなので、許すと渡した相手が DID を乗っ取れる。

`com.atproto.server.createSession`のメール2FA（`authFactorToken`）も同じ`email_short_codes`機構（`purpose='atp_session_2fa'`）を使う。SMTP未設定インスタンスでは2FA自体を常にスキップする。`requestPlcOperationSignature`/`signPlcOperation`も同じ原則: SMTP未設定なら`requestPlcOperationSignature`はコードを発行・送信せず空応答のみ返し、`signPlcOperation`も検証をスキップする。SMTP設定はあるがメール送信自体が失敗した場合も、発行済みコードを`revoke`して「未発行」扱いにする（届かないコードの入力を求め続けないため）。検証要否は「SMTP設定の有無」ではなく「有効なコードが実在するか」（`has_pending`）で判定する。SMTP未設定のサーバーでは、メインパスワードだけがDID移行の防壁になることを運営者が受け入れている前提。

### DID転出済み状態（`did_moved_out_at`）

`is_suspended`（凍結）や本ドキュメント3節の`migration_status`ゲートとは異なる第三の状態。`submitPlcOperation`成功時、または`deactivateAccount`呼び出し時に`actors.did_moved_out_at`が設定される。この状態では:

- タイムライン等の**読み取りは通常通り**表示する（専用画面へのバイパスはしない）
- 投稿・リアクション・リポスト・フォロー・リスト操作・DM送信等の**書き込みは全て拒否**（`AuthedUser::require_not_did_moved_out()`、`crates/seiran-api/src/middleware/authed_user.rs`。フロントエンドは`user.did_moved_out`で主要な書き込みUIを無効化、`frontend/src/components/note/PostComposer.tsx`・`frontend/src/hooks/useNoteCardActions.ts`）
- ActivityPub側の転出（AP自体の引っ越し）は未実装

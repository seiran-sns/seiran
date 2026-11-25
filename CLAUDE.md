# seiran プロジェクト固有ルール ＆ 環境前提

## 開発環境の前提条件
- **DB関連ツール**: システムに `docker compose`、`psql`、および `pg_config` がインストール済みであり、ローカル開発環境で利用可能です。
- **フロントエンド開発サーバー**: HMR付きdevサーバー（`npm run dev`、5173番）ではなく、`npm run build:watch`（ファイル変更のたびに本番相当ビルド）を常駐させ、その出力 `frontend/dist` を seiran-server（3000番）が `SEIRAN_FRONTEND_DIR` で配るのを標準とする（`vite preview` は使わない）。詳細は `docs/architecture.md` 8節参照。

## テスト用ユーザー（API動作確認用）

ローカル環境でのAPI動作確認（curlでの認証が必要なエンドポイントのテスト等）のために、
`seiran1`, `seiran2`, `seiran3`, ... という名前のユーザーを **自由に作成してよい**。
- パスワードは全て `seiranda` に統一する。
- メールアドレス確認は必須にしていないため、登録後すぐログインできる。
- `seiran{n}` という名前のユーザーが既に存在する場合、そのアカウントを断りなく自由に使ってよい
  （パスワードは `seiranda` で固定なので毎回作り直す必要はない）。
- 実データ・本番相当のユーザー（マイケル本人のアカウント等）には触れないこと。

## 開発フロー・コミットルール

### 設計文書の同期（必須）
コードに変更を加えた場合、**必ず** 対応する設計文書を同期してからコミットすること。

- **アーキテクチャ・設計の変更** → `docs/architecture.md` を更新
- **DBスキーマの変更** → `docs/database.md` を更新
- **プロトコル仕様（AP/ATP・クロスプロトコル配送）の変更** → `docs/protocols.md` を更新
- **UI/3ペイン仕様の変更** → `docs/ui_spec.md` を更新
- **プロダクト思想・メンタルモデルの変更** → `docs/concept.md` を更新
- **新機能の完了** → `docs/roadmap.md` の該当チェックボックスに `[x]` を入れる

> **ルール**: 設計文書の更新とコードの変更は **同じコミット** に含めること。設計文書を後回しにしない。

### コーディングルール（抜粋、全文は `docs/coding_rules.md` 2節）
- SQL はリポジトリ層（`crates/seiran-common/src/repository/`）にだけ書く。ハンドラ・ジョブ・firehose からは型付きのリポジトリ関数を呼ぶ（`sqlx::query*`・`QueryBuilder`・`sqlx::Row` の使用を `crates/seiran-common/tests/sql_style.rs` が検出する）。
- SQL で `IN (SELECT ...)` / `NOT IN (SELECT ...)` を使わない。`EXISTS` / `NOT EXISTS` で書く（NULL を含むサブクエリで `NOT IN` が常に偽になる事故の再発防止。`crates/seiran-common/tests/sql_style.rs` が機械的に検出する）。NULL 許容列の比較では NULL 行の扱いを明示する。
- 「読んでから書く」「書いてから読む」を別々の文で行わない。`RETURNING`・`ON CONFLICT`・`SET col = f(col)` で1文にするか、トランザクション内で `FOR UPDATE` / アドバイザリロックで直列化する。外部 API 呼び出しはトランザクションに含めない。
- `TimelinePost` を返すクエリは `timeline_post_columns!()` / `timeline_post_joins!()` を使い、可視性は `post_is_visible_to` で判定する（手書きしない。例外はホーム/ソーシャル/リストの候補抽出 `repository/feed.rs` だけで、関数と一致を検証済みの展開マクロを使う）。
- id カーソルは `p.id < COALESCE($2::bigint, 9223372036854775807)` と書く（`($2 IS NULL OR p.id < $2)` は generic plan でインデックスが効かない。`sql_style.rs` が検出する）。
- マイグレーション後のスキーマは、互換を保つ範囲のすべての版のサーバーがそのまま動けるものにする。範囲は 1.0.0 以降は「同じメジャー版のそれより前のすべての版」と「1つ前のメジャー版の最終版」、0.x では「1つ前の版」。列・テーブルを消す・名前を変える・型を変えるのは、範囲のどの版も使わなくなってからにし、守れない変更はメジャー版を上げる（管理者が版を大きく飛ばして更新しても、各メジャー版の最終版を経由すれば済むようにするため。新しい版は起動時にマイグレーションを適用し、無停止更新の間は古い版が新しいスキーマで動き続ける）。
- `TimelinePost` からレスポンスへの組み立ては、frontend API は `handlers::notes::build_note_responses`、Misskey 互換 API は `misskey::convert::build_notes` を使う（添付・リアクション・埋め込み・投票状態等の付与手順を個別に書かない）。
- `#[allow(clippy::too_many_arguments)]` を新たに書かない。引数の束は構造体にし、長い関数は手順ごとの関数に分ける。
- Misskey 互換 API とカスタム API で同じ処理を別々に書かない。データ取得・検証・副作用は共通関数にし、差分はレスポンス整形だけに閉じ込める。
- コメントと設計文書には「なぜ」と現在の動作だけを書く。一見奇異な実装は「そうしないと何が起きるか」を書き、不具合の経緯（「〇〇のバグがあったので」「以前は〜」）・日付・実機確認の記録は書かない（経緯は `git log`）。

### コミット前の確認フロー

ローカルで必ず行う検査は **gitleaks だけ** にする。秘密情報は push した時点で外に出て取り返せないので、CI で見つかってからでは遅い。ビルド・clippy・fmt・単体/結合テスト・E2E は PR の CI が同じコマンドで実行し直すため、ローカルで完了を待つのは時間の二重払いになる。push して PR の CI に任せる（CI は検査対象の中身ごとに合格を記録し、PR で通った中身は `main` に入っても検査をやり直さない。`docs/architecture.md` 9節）。

1. コード実装・修正
2. 新しいマイグレーションファイルを追加した場合は `cargo sqlx migrate run` でローカル DB に適用する（後述）
3. Rust を変更した場合は `cargo fmt --all` を流す（数秒で済み、CI の fmt 検査落ちを避けられる）。`cargo build`・`cargo clippy`・テストの完了は待たない
4. `frontend/package.json` を変更した場合（依存追加・更新・削除）は、CI と同じ Node バージョン（26系）で `frontend/` 内から `npm ci` を実行し、`package-lock.json` が package.json と整合していることを確認する（`npm install` は lockfile のずれを黙って許容してしまい検出できない。`npm ci` は CI が実際に叩くコマンドそのものなので、ローカルで通れば CI でも通る）
5. 関連する設計文書を更新（上記の対応表に従う）
6. `docs/roadmap.md` の進捗チェックを更新
7. **gitleaks（必須）**: CI と同じ版（`.github/workflows/ci.yml` の `GITLEAKS_VERSION`）を mise 経由で実行し、終了コード0を確かめてからコミット・push する。素の `gitleaks` は PATH に無いことがあるので `mise exec` で呼ぶ。出力を `| tail` 等へパイプして `&&` で commit/push に繋がない（gitleaks が失敗・不在でもパイプ先の終了コードで先へ進んでしまう）
   - コミット前（ステージ分）: `git add -A && mise exec gitleaks@8.30.1 -- gitleaks git --staged --redact --verbose --exit-code 1 .`
   - push 前（CI と同じ）: `mise exec gitleaks@8.30.1 -- gitleaks detect --source . --redact --exit-code 1`
8. 作業ブランチを push して `main` 向けの PR を作り、PR の CI（ビルド・clippy・fmt・テスト・E2E）の完了を待つ。E2E はローカルで実行しない。CI が落ちたら、失敗した検査・テストだけをローカルで再現して直してよい（E2E側はバックエンド3100・フロントエンド5273という専用ポート（`e2e/ports.ts`）で動くため、`scripts/dev-up.sh` 等のローカル確認用サーバー（バックエンド3000・フロントエンドは3000番が`build:watch`の出力を配る）を止める必要はない）
   - この検証のための PR は、マイケルに頼まれなくても作ってよい（作業ブランチから `main` 向けに作る）。
   - 開発機での画面・動作確認のためにサーバーを動かす場合のビルドは必要なので行う（検査としてのビルド完了待ちとは別）。
9. PR の CI が通り、マイケルの画面・動作確認がOKになってから、PR を `main` へマージする
10. バージョンのリビジョン桁を上げてプッシュしたら、開発機へのデプロイし直しを忘れないこと（後述「バージョン運用」節）

### バージョン運用

frontend/backendで単一のシステムバージョン（`Cargo.toml`の`[workspace.package].version`と
`frontend/package.json`の`version`）を共有する。詳細な仕組みは `docs/architecture.md` 2.1節参照。

- `main`ブランチ上での開発では、開発機へデプロイし直してマイケルの画面・動作確認がOKになった
  時点で、(1) リビジョン桁（`x.y.z`の`z`）を1つ進める → (2) `main`へコミット・プッシュする →
  (3) **プッシュ後、開発機へ改めてデプロイし直す**、の順で行う。バージョンを上げてプッシュした
  だけで終わらせず、必ず最後にデプロイし直すこと（プッシュとデプロイは別作業であり、片方をやった
  からといってもう片方が自動で終わるわけではない）。
  - デプロイし直すとは、具体的には: バックエンド（`target/debug/seiran-server`のネイティブ
    プロセス）を再起動する（既存プロセスを止めて `cargo run -p seiran-server` を再度起動する。
    再起動自体はマイケルへの確認不要）、およびフロントエンドを `cd frontend && npm run build`
    で明示的にビルドし直す（`npm run build:watch` はファイル変更検知を静かに見失うことがあるため
    バージョンbump時のような単発の変更では自動反映を信用せず明示的にビルドする）の両方を指す。
  - (1)(2) は GitHub 上でもできる: main の CI が通ると、Bump ワークフローの
    「Bump revision」ジョブが承認待ちになり、マイケルが承認すると版を上げて main へ push する
    （`docs/architecture.md` 3.1節「版上げの承認」）。この場合も (3) は自動では行われないので、
    `git pull` してデプロイし直す。
- 例外: 依存の脆弱性修正（Dependabot のセキュリティ更新）は、CI が通れば確認を待たずに自動で
  マージ・版上げ・公開イメージの push まで進む（`docs/architecture.md` 3.1節「依存の脆弱性修正の
  自動リリース」）。このとき開発機へのデプロイし直しは自動では行われないので、気づいたら
  `git pull` してデプロイし直す。
- 「対応する対向の最低バージョン」（`SERVER_MIN_PEER_VERSION`/`FRONTEND_MIN_PEER_VERSION`）を
  引き上げる必要が生じる非互換な変更をする場合は、実装前に必ずマイケルに相談すること
  （無断で引き上げない）。
- バージョン番号（`version`本体・`version_suffix`/`versionSuffix`）だけを変更するコミットは、
  コードの挙動に影響しないため、PR を作らず `main` へ直接コミットしてよい（gitleaks は通す。
  CI も版の番号を除いて検査対象の中身を比べるので、直前が通っていれば検査を省く）。

### マイグレーションの適用方法（必読）

**`psql -f migration.sql` で直接流してはならない。** これをやると `_sqlx_migrations` テーブルに記録が残らず、API コンテナ起動時に「未適用」と判断されて再実行しようとして失敗する。

**必ず以下のコマンドを使う:**

```bash
cargo sqlx migrate run
```

このコマンドは:
- スキーマを DB に適用する
- `_sqlx_migrations` に記録を残す（API コンテナ起動時に「適用済み」と認識される）

